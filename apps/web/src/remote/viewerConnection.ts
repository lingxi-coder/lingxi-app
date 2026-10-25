import {
  CLIENT_PROTOCOL_MAJOR,
  DATA_CHANNEL_LABEL,
  DATA_CHANNEL_OPEN_TIMEOUT_MS,
  DEFAULT_STUN_URLS,
  ICE_GATHER_TIMEOUT_MS,
  MessageTransport,
  OFFER_ANSWER_TIMEOUT_MS,
  REMOTE_PROTOCOL_MAJOR,
  RemoteProtocolError,
  RemoteRpcError,
  RpcPeer,
  RtdbError,
  RtdbMirror,
  SDP_MAX_CHARS,
  SSE_BACKOFF_MAX_MS,
  SSE_BACKOFF_MIN_MS,
  SSE_BACKOFF_RESET_MS,
  TunnelLink,
  b64uDecode,
  b64uEncode,
  deriveTunnelKeys,
  downPath,
  generateEphemeralKey,
  hostPath,
  offerPath,
  pairingCodeFor,
  parseHostRecord,
  randomBytes,
  randomId,
  signOffer,
  systemClock,
  toHostConnPath,
  toViewerConnPath,
  upPath,
  verifyAnswer,
  x25519,
  answerPath,
  type AnswerRecord,
  type Clock,
  type FragmentLink,
  type OfferDraft,
  type RemoteHostInfo,
  type Rtdb,
  type RtdbSubscription,
  type TransportKind,
  type VerifyFailure,
  type ViewerRequests,
  type WebCryptoKey,
} from '@lingxi/bridge-client/remote';
import { browserDeviceName, type DeviceIdentity, type DeviceKeys, type PinnedHost } from './deviceKey';
import { DataChannelLink, waitForIceGathering, type DataChannelLike, type PeerConnectionFactory, type PeerConnectionLike } from './peerLink';
import { displayText } from './text';
import { TunnelMemory } from './tunnelMemory';
import { webAppVersion } from './env';

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

export type ConnectionPhase =
  /** Nothing running: not started, or stopped. */
  | 'idle'
  /** Dialling: writing the offer, waiting for the answer, opening the channel, handshaking. */
  | 'connecting'
  /** The desktop is asking its user whether to allow this browser; `pairingCode` is shown. */
  | 'awaiting_approval'
  | 'ready'
  /** Connected before and lost it; waiting out the backoff, or dialling again. */
  | 'reconnecting'
  /** Stopped trying; `failure` says why and whether the user can try again. */
  | 'failed';

export type FailureKind =
  /** This browser lacks WebCrypto Ed25519/X25519. */
  | 'unsupported'
  /** First pairing, but `hosts/{host_id}` is missing or malformed. */
  | 'unknown_host'
  /** The Realtime Database could not be reached, or refused. */
  | 'network'
  /** The desktop did not answer within 30 s. */
  | 'no_answer'
  /** The link ended. `detail` is the machine-readable reason. */
  | 'closed'
  | 'capacity'
  | 'busy'
  /** The desktop no longer trusts this browser. The pin is already cleared. */
  | 'revoked'
  /** Remote control is switched off on the desktop. */
  | 'disabled'
  | 'upgrade_required'
  /** The user said no on the desktop. */
  | 'denied'
  /** The desktop thinks this browser's clock is off. */
  | 'expired'
  /** Another reason the desktop gave for refusing. */
  | 'rejected'
  /** The answer did not verify; `detail` is the {@link VerifyFailure}. */
  | 'verification'
  /** The desktop computed a different pairing code, so someone is in the middle. */
  | 'pairing_code_mismatch'
  /** The desktop broke the protocol. */
  | 'protocol';

export interface ConnectionFailure {
  kind: FailureKind;
  detail?: string;
  /** Whether the connection retries by itself (with backoff). */
  auto: boolean;
}

export interface ConnectionState {
  phase: ConnectionPhase;
  hostId: string;
  /** How the current, or the last, connection travels. */
  transport?: TransportKind;
  /** While `awaiting_approval`: the code the desktop's dialog shows as well. */
  pairingCode?: string;
  /** In `failed`, and in `reconnecting` as the reason the link dropped. */
  failure?: ConnectionFailure;
  /** Consecutive failed attempts since the connection was last stable. */
  attempt: number;
  /** When the next attempt starts (`clock.now()` time), while waiting to reconnect. */
  retryAt?: number;
  /** The desktop's description of itself, once approved. */
  host?: RemoteHostInfo;
  /** Whether this object has been `ready` at least once. */
  everReady: boolean;
}

export interface ViewerConnectionDeps {
  uid: string;
  hostId: string;
  rtdb: Rtdb;
  keys: DeviceKeys;
  clock?: Clock;
  /** Jitter source in [0, 1). */
  random?: () => number;
  /** Absent: this browser has no WebRTC, so every connection uses the tunnel. */
  peerConnections?: PeerConnectionFactory;
  stunUrls?: readonly string[];
  /** Always use the tunnel (the `?tunnel=1` debug switch). */
  forceTunnel?: boolean;
  tunnelMemory?: TunnelMemory;
  deviceName?: string;
  appVersion?: string;
}

export type NotificationListener = (method: string, params: unknown) => void;

// ─────────────────────────────────────────────────────────────────────────────
// Constants and small helpers
// ─────────────────────────────────────────────────────────────────────────────

/** How long `initialize` may take; while pairing it answers at once, the wait for the user comes after. */
const HANDSHAKE_TIMEOUT_MS = DATA_CHANNEL_OPEN_TIMEOUT_MS;
/** Failed attempts in a row after which the connection stops retrying on its own. */
const MAX_AUTO_ATTEMPTS = 8;

/** Backoff before retry number `failures` (1-based): 1 s doubling to 30 s, scaled by 0.5 to 1.0. */
export function backoffMs(failures: number, random: () => number): number {
  const base = Math.min(SSE_BACKOFF_MAX_MS, SSE_BACKOFF_MIN_MS * 2 ** Math.max(0, failures - 1));
  return Math.round(base * (0.5 + 0.5 * random()));
}

/** Thrown inside an attempt when `stop()` ended it. */
class Cancelled extends Error {
  constructor() {
    super('cancelled');
    this.name = 'Cancelled';
  }
}

/** An attempt failed. `direct` says the direct path is to blame, so the tunnel is worth a try. */
class AttemptError extends Error {
  constructor(
    readonly failure: ConnectionFailure,
    readonly direct = false,
  ) {
    super(failure.detail ? `${failure.kind}: ${failure.detail}` : failure.kind);
    this.name = 'AttemptError';
  }
}

class Abort {
  private done = false;
  private readonly listeners = new Set<() => void>();

  get aborted(): boolean {
    return this.done;
  }

  abort(): void {
    if (this.done) return;
    this.done = true;
    for (const listener of [...this.listeners]) listener();
    this.listeners.clear();
  }

  on(listener: () => void): () => void {
    if (this.done) {
      listener();
      return () => undefined;
    }
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}

/** `promise`, unless `abort` fires first. */
function raced<T>(abort: Abort, promise: Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const off = abort.on(() => reject(new Cancelled()));
    promise.then(
      (value) => {
        off();
        resolve(value);
      },
      (error: unknown) => {
        off();
        reject(error);
      },
    );
  });
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  promise.catch(() => undefined); // whoever awaits it handles the failure; an unawaited one is not "unhandled"
  return { promise, resolve, reject };
}

function networkFailure(error: unknown): ConnectionFailure {
  if (error instanceof RtdbError) {
    // 401 and 403 mean the security rules refuse this account; asking again will not change that.
    return { kind: 'network', detail: String(error.status), auto: error.status !== 401 && error.status !== 403 };
  }
  return { kind: 'network', detail: error instanceof Error ? error.message : String(error), auto: true };
}

function verificationFailure(reason: VerifyFailure, detail?: string): ConnectionFailure {
  return { kind: 'verification', detail: detail === undefined ? reason : `${reason}: ${detail}`, auto: false };
}

/** What a signed `rejected` answer means for the user. */
function rejectionFailure(reason: AnswerRecord['reason']): ConnectionFailure {
  switch (reason) {
    case 'revoked':
      return { kind: 'revoked', auto: false };
    case 'disabled':
      return { kind: 'disabled', auto: false };
    case 'upgrade_required':
      return { kind: 'upgrade_required', auto: false };
    case 'expired':
      return { kind: 'expired', auto: false };
    case 'capacity':
      return { kind: 'capacity', auto: true };
    case 'busy':
      return { kind: 'busy', auto: true };
    default:
      return { kind: 'rejected', detail: reason, auto: false };
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// One dial
// ─────────────────────────────────────────────────────────────────────────────

/** The resources of one attempt, so that a failure or `stop()` can release all of them. */
class Attempt {
  readonly abort = new Abort();
  connId = '';
  offerWritten = false;
  peer: PeerConnectionLike | undefined;
  dataLink: DataChannelLink | undefined;
  tunnel: TunnelLink | undefined;
  answers: RtdbSubscription | undefined;
  rpc: RpcPeer | undefined;
  transport: MessageTransport | undefined;
  kind: TransportKind = 'tunnel';
  /** Set once `initialize` succeeded and, if needed, the user approved. */
  ready = false;
  readonly approval = deferred<RemoteHostInfo>();
  /** Rejects when the transport closes, so waits on the desktop end early. */
  readonly closed = deferred<never>();
  closeReason: string | undefined;
  disposed = false;
  /** Notifications that arrived after approval but before the connection was published as ready. */
  readonly held: Array<[string, unknown]> = [];
  /** Why the desktop is ending an approved connection (`auth.denied` after approval: revoked, disabled). */
  notice: string | undefined;
}

type AttemptResult =
  | { type: 'ready'; attempt: Attempt; host: RemoteHostInfo }
  | { type: 'failed'; failure: ConnectionFailure; direct: boolean }
  | { type: 'cancelled' };

/**
 * The browser's side of a Remote Control connection to one computer: it dials,
 * proves who it is, falls back to the RTDB tunnel when no direct path works,
 * runs the `initialize` handshake (including pairing), and keeps the link up.
 *
 * Everything with side effects is injected (database, clock, WebRTC, key
 * store), so a test can stand a fake desktop on the other end of a
 * `MemoryRtdb`. `start()` begins, `stop()` ends; in between `state` says where
 * things stand and `subscribe()` reports changes. Each time a link comes up
 * the `onReady` listeners run, and host notifications reach `onNotification`
 * listeners regardless of which link carried them.
 */
export class ViewerConnection {
  private readonly clock: Clock;
  private readonly random: () => number;
  private readonly tunnelMemory: TunnelMemory;
  private readonly stunUrls: readonly string[];
  private readonly deviceName: string;
  private readonly appVersion: string;

  private current: ConnectionState;
  private generation = 0;
  private dialing: Attempt | undefined;
  private live: Attempt | undefined;
  private failures = 0;
  private readyAt = 0;
  private retryTimer: unknown;
  private repairPending: Promise<void> | undefined;
  private readonly stateListeners = new Set<(state: ConnectionState) => void>();
  private readonly readyListeners = new Set<() => void>();
  private readonly notificationListeners = new Set<NotificationListener>();

  constructor(private readonly deps: ViewerConnectionDeps) {
    this.clock = deps.clock ?? systemClock;
    this.random = deps.random ?? Math.random;
    this.tunnelMemory = deps.tunnelMemory ?? new TunnelMemory(this.clock);
    this.stunUrls = deps.stunUrls ?? DEFAULT_STUN_URLS;
    this.deviceName = displayText(deps.deviceName ?? browserDeviceName(), 64) || 'Browser · Web';
    this.appVersion = deps.appVersion ?? webAppVersion();
    this.current = { phase: 'idle', hostId: deps.hostId, attempt: 0, everReady: false };
  }

  get state(): ConnectionState {
    return this.current;
  }

  subscribe(listener: (state: ConnectionState) => void): () => void {
    this.stateListeners.add(listener);
    return () => this.stateListeners.delete(listener);
  }

  /** Called every time a link becomes ready, including after a reconnect. */
  onReady(listener: () => void): () => void {
    this.readyListeners.add(listener);
    return () => this.readyListeners.delete(listener);
  }

  /** Host notifications from the ready link (the handshake's own `auth.*` stay inside). */
  onNotification(listener: NotificationListener): () => void {
    this.notificationListeners.add(listener);
    return () => this.notificationListeners.delete(listener);
  }

  /** Send a request on the ready link. Rejects with `closed` when there is none. */
  request<K extends keyof ViewerRequests>(
    method: K,
    params: ViewerRequests[K]['params'],
    options?: { timeoutMs?: number },
  ): Promise<ViewerRequests[K]['result']> {
    const rpc = this.live?.rpc;
    if (rpc === undefined || rpc.closed) return Promise.reject(new RemoteRpcError('closed', 'not connected'));
    return rpc.request<ViewerRequests[K]['result']>(method, params, options);
  }

  /** Send a notification on the ready link; dropped when there is none. */
  notify(method: string, params?: unknown): void {
    const rpc = this.live?.rpc;
    if (rpc === undefined || rpc.closed) return;
    void rpc.notify(method, params).catch(() => undefined);
  }

  /** Start dialling. Does nothing while already connecting, connected or waiting to reconnect. */
  start(): void {
    if (this.current.phase === 'idle') this.begin(false);
  }

  /** Try again after a failure, at the user's request. */
  retry(): void {
    if (this.current.phase !== 'failed' && this.current.phase !== 'idle') return;
    this.failures = 0;
    this.begin(this.current.everReady);
  }

  /**
   * Called when the page becomes visible again or the network comes back: skip
   * the rest of a backoff, and revive a connection that gave up on its own.
   */
  nudge(): void {
    if (this.current.phase === 'reconnecting' && this.retryTimer !== undefined) {
      this.clock.clearTimeout(this.retryTimer);
      this.retryTimer = undefined;
      this.begin(true);
    } else if (this.current.phase === 'failed' && this.current.failure?.auto === true) {
      this.failures = 0;
      this.begin(this.current.everReady);
    }
  }

  /** After `revoked`: the old device key is useless, so make a new one and ask again. */
  repair(): Promise<void> {
    if (this.repairPending !== undefined) return this.repairPending;
    if (this.current.phase !== 'failed' || this.current.failure?.kind !== 'revoked') return Promise.resolve();
    const generation = this.generation;
    const operation = this.deps.keys.resetIdentity().then(() => {
      // Leaving the page or starting another dial cancels this repair's retry.
      if (generation === this.generation) this.retry();
    }).finally(() => {
      if (this.repairPending === operation) this.repairPending = undefined;
    });
    this.repairPending = operation;
    return operation;
  }

  /** End everything: cancel the dial, close the link, clean up the database. */
  stop(): void {
    this.generation += 1;
    if (this.retryTimer !== undefined) this.clock.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    const dialing = this.dialing;
    const live = this.live;
    this.dialing = undefined;
    this.live = undefined;
    if (dialing !== undefined) this.dispose(dialing, 'cancelled');
    if (live !== undefined) this.dispose(live, 'cancelled');
    if (this.current.phase !== 'idle') {
      this.update({ phase: 'idle', pairingCode: undefined, failure: undefined, retryAt: undefined });
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // State
  // ───────────────────────────────────────────────────────────────────────────

  private update(change: Partial<ConnectionState>): void {
    this.current = { ...this.current, ...change, attempt: this.failures };
    for (const listener of [...this.stateListeners]) {
      try {
        listener(this.current);
      } catch {
        // Listeners are independent of each other.
      }
    }
  }

  private begin(reconnecting: boolean): void {
    if (this.retryTimer !== undefined) this.clock.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    const generation = ++this.generation;
    this.update({ phase: reconnecting ? 'reconnecting' : 'connecting', pairingCode: undefined, retryAt: undefined });
    void this.dial(generation);
  }

  private fail(failure: ConnectionFailure): void {
    this.failures += 1;
    if (failure.auto && this.failures <= MAX_AUTO_ATTEMPTS) {
      const delay = backoffMs(this.failures, this.random);
      this.update({ phase: 'reconnecting', failure, pairingCode: undefined, retryAt: this.clock.now() + delay });
      this.retryTimer = this.clock.setTimeout(() => {
        this.retryTimer = undefined;
        this.begin(true);
      }, delay);
    } else {
      this.update({ phase: 'failed', failure, pairingCode: undefined, retryAt: undefined });
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Dialling
  // ───────────────────────────────────────────────────────────────────────────

  private async dial(generation: number): Promise<void> {
    const { hostId } = this.deps;
    const skipDirect = this.deps.forceTunnel === true || this.deps.peerConnections === undefined || this.tunnelMemory.prefers(hostId);
    let result = await this.attempt(generation, skipDirect ? 'tunnel' : 'webrtc');
    if (result.type === 'failed' && result.direct && generation === this.generation) {
      // The direct path did not work: remember that, and dial again through the tunnel on a new connection id.
      this.tunnelMemory.remember(hostId);
      result = await this.attempt(generation, 'tunnel');
    }
    if (generation !== this.generation) {
      if (result.type === 'ready') this.dispose(result.attempt, 'cancelled');
      return;
    }
    if (result.type === 'cancelled') return;
    if (result.type === 'failed') {
      this.fail(result.failure);
      return;
    }
    const { attempt, host } = result;
    this.live = attempt;
    this.readyAt = this.clock.now();
    this.update({
      phase: 'ready',
      transport: attempt.kind,
      host,
      pairingCode: undefined,
      failure: undefined,
      retryAt: undefined,
      everReady: true,
    });
    for (const [method, params] of attempt.held.splice(0)) this.deliver(method, params);
    for (const listener of [...this.readyListeners]) {
      try {
        listener();
      } catch {
        // Listeners are independent of each other.
      }
    }
  }

  /** One dial: write an offer, get the answer, bring the link up, handshake. */
  private async attempt(generation: number, requested: TransportKind): Promise<AttemptResult> {
    const a = new Attempt();
    this.dialing = a;
    try {
      const host = await this.runAttempt(a, requested);
      if (generation !== this.generation || a.abort.aborted) {
        this.dispose(a, 'cancelled');
        return { type: 'cancelled' };
      }
      this.dialing = undefined;
      return { type: 'ready', attempt: a, host };
    } catch (error) {
      this.dispose(a, error instanceof AttemptError ? error.failure.kind : 'failed');
      if (this.dialing === a) this.dialing = undefined;
      if (error instanceof Cancelled || generation !== this.generation) return { type: 'cancelled' };
      if (error instanceof AttemptError) return { type: 'failed', failure: error.failure, direct: error.direct };
      return { type: 'failed', failure: { kind: 'protocol', detail: error instanceof Error ? error.message : String(error), auto: false }, direct: false };
    }
  }

  private async runAttempt(a: Attempt, requested: TransportKind): Promise<RemoteHostInfo> {
    const { uid, hostId, rtdb, keys } = this.deps;
    if (typeof crypto === 'undefined' || crypto.subtle === undefined) {
      throw new AttemptError({ kind: 'unsupported', auto: false });
    }

    let identity: DeviceIdentity;
    let pin: PinnedHost | undefined;
    let eph: Awaited<ReturnType<typeof generateEphemeralKey>>;
    try {
      identity = await raced(a.abort, keys.identity());
      pin = await raced(a.abort, keys.pinnedHost(hostId));
      eph = await raced(a.abort, generateEphemeralKey());
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      // Ed25519 and X25519 in WebCrypto are recent; a browser without them cannot take part at all.
      throw new AttemptError({ kind: 'unsupported', detail: error instanceof Error ? error.message : String(error), auto: false });
    }
    const expectedHostKey = pin?.host_key ?? (await this.advertisedHostKey(a));

    const connId = (a.connId = randomId());
    const nonce = b64uEncode(randomBytes(16));

    // A direct offer carries the complete SDP; if one cannot be made, the offer is simply a tunnel offer.
    let offered: TransportKind = requested;
    let sdp: string | undefined;
    if (requested === 'webrtc') {
      sdp = await this.prepareDirect(a);
      if (sdp === undefined) {
        offered = 'tunnel';
        this.tunnelMemory.remember(hostId);
      }
    }
    a.kind = offered;

    const draft: OfferDraft = {
      v: 1,
      transport: offered,
      ...(sdp === undefined ? {} : { sdp }),
      device: {
        id: identity.id,
        key: b64uEncode(identity.publicKey),
        name: this.deviceName,
        platform: 'web',
        app_version: displayText(this.appVersion, 64) || 'dev',
      },
      eph: b64uEncode(eph.publicKey),
      nonce,
    };
    const offer = await raced(a.abort, signOffer({ uid, hostId, connId }, identity.privateKey, draft));

    // Listen before writing, so that even an instant refusal cannot be missed.
    const answered = this.listenForAnswer(a, nonce, expectedHostKey);
    try {
      await raced(a.abort, rtdb.put(offerPath(uid, hostId, connId), offer));
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      a.offerWritten = true; // it may have landed even though the reply did not
      throw new AttemptError(networkFailure(error));
    }
    a.offerWritten = true;

    const answer = await this.waitForAnswer(a, answered);
    a.answers?.close();
    a.answers = undefined;
    void rtdb.remove(answerPath(uid, hostId, connId)).catch(() => undefined);

    if (answer.status === 'rejected') {
      const failure = rejectionFailure(answer.reason);
      // The desktop vouched for the refusal with its own signature, so a revoked pairing really is over.
      if (failure.kind === 'revoked') await keys.unpair(hostId).catch(() => undefined);
      throw new AttemptError(failure);
    }
    const chosen = answer.transport;
    if (chosen === undefined || (offered === 'tunnel' && chosen !== 'tunnel')) {
      throw new AttemptError({ kind: 'protocol', detail: 'answer transport does not fit the offer', auto: false });
    }

    // Both sides compute the code from the same records; it is only ever shown from our own computation.
    const pairingCode = await pairingCodeFor(draft, answer);

    let link: FragmentLink;
    if (chosen === 'webrtc') {
      link = await this.openDirect(a, answer);
    } else {
      if (a.peer !== undefined) {
        // The desktop chose the tunnel for a direct offer: same connection id, no new offer.
        this.tunnelMemory.remember(hostId);
        a.dataLink?.close('downgraded');
        a.dataLink = undefined;
        a.peer.close();
        a.peer = undefined;
      }
      link = await this.openTunnel(a, eph.privateKey, answer);
    }
    a.kind = chosen;

    const transport = new MessageTransport(link);
    const rpc = new RpcPeer(transport, { clock: this.clock });
    a.transport = transport;
    a.rpc = rpc;
    rpc.onNotification((method, params) => this.receive(a, method, params));
    transport.onClose((reason) => this.linkClosed(a, reason));
    rpc.startKeepalive(chosen === 'webrtc' ? 'datachannel' : 'tunnel');

    return this.handshake(a, chosen, pairingCode, expectedHostKey, pin);
  }

  /** `hosts/{host_id}.host_key`, the key a first pairing expects the answer to be signed with. */
  private async advertisedHostKey(a: Attempt): Promise<string> {
    const { uid, hostId, rtdb } = this.deps;
    let raw: unknown;
    try {
      raw = await raced(a.abort, rtdb.get(hostPath(uid, hostId)));
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      throw new AttemptError(networkFailure(error));
    }
    const parsed = parseHostRecord(raw);
    if (!parsed.ok) throw new AttemptError({ kind: 'unknown_host', detail: parsed.error, auto: false });
    return parsed.value.host_key;
  }

  /**
   * Create the connection, the DataChannel and the complete offer SDP. Returns
   * `undefined` when none can be made (no WebRTC, no usable SDP), which sends
   * the caller to the tunnel without a failed attempt.
   */
  private async prepareDirect(a: Attempt): Promise<string | undefined> {
    const factory = this.deps.peerConnections;
    if (factory === undefined) return undefined;
    try {
      const peer = factory({ iceServers: [{ urls: [...this.stunUrls] }] });
      a.peer = peer;
      const channel: DataChannelLike = peer.createDataChannel(DATA_CHANNEL_LABEL, { ordered: true });
      a.dataLink = new DataChannelLink(channel);
      const offer = await raced(a.abort, peer.createOffer());
      await raced(a.abort, peer.setLocalDescription(offer));
      await raced(a.abort, waitForIceGathering(peer, this.clock, ICE_GATHER_TIMEOUT_MS));
      const sdp = peer.localDescription?.sdp;
      if (sdp === undefined || sdp.length === 0 || sdp.length > SDP_MAX_CHARS) throw new Error('no usable offer SDP');
      return sdp;
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      a.dataLink?.close('no_offer');
      a.dataLink = undefined;
      a.peer?.close();
      a.peer = undefined;
      return undefined;
    }
  }

  /** Resolve with the verified answer once it appears under `to_viewer/{host}/{conn}/answer`. */
  private listenForAnswer(a: Attempt, offerNonce: string, expectedHostKey: string): Promise<AnswerRecord> {
    const { uid, hostId, rtdb } = this.deps;
    const connId = a.connId;
    const mirror = new RtdbMirror();
    const answered = deferred<AnswerRecord>();
    let claimed = false;
    a.answers = rtdb.listen(
      toViewerConnPath(uid, hostId, connId),
      (event) => {
        if (claimed) return;
        for (const change of mirror.apply(event).at(1)) {
          if (change.path[0] !== 'answer' || change.kind === 'removed') continue;
          claimed = true;
          verifyAnswer({ uid, hostId, connId, record: change.value, offerNonce, expectedHostKey }).then(
            (result) => {
              if (result.ok) answered.resolve(result.value);
              else answered.reject(new AttemptError(verificationFailure(result.reason, result.detail)));
            },
            (error: unknown) => answered.reject(error),
          );
          return;
        }
      },
      (error) => {
        if (claimed) return;
        claimed = true;
        answered.reject(new AttemptError({ kind: 'network', detail: error.kind === 'http' ? String(error.status) : error.kind, auto: error.kind !== 'http' || (error.status !== 401 && error.status !== 403) }));
      },
    );
    return answered.promise;
  }

  private waitForAnswer(a: Attempt, answered: Promise<AnswerRecord>): Promise<AnswerRecord> {
    const timeout = deferred<never>();
    const timer = this.clock.setTimeout(
      () => timeout.reject(new AttemptError({ kind: 'no_answer', auto: this.current.everReady })),
      OFFER_ANSWER_TIMEOUT_MS,
    );
    return raced(a.abort, Promise.race([answered, timeout.promise])).finally(() => this.clock.clearTimeout(timer));
  }

  /** Finish the direct connection: apply the answer and wait for the DataChannel to open. */
  private async openDirect(a: Attempt, answer: AnswerRecord): Promise<FragmentLink> {
    const peer = a.peer;
    const link = a.dataLink;
    if (peer === undefined || link === undefined || answer.sdp === undefined) {
      throw new AttemptError({ kind: 'protocol', detail: 'direct answer without a direct offer', auto: false });
    }
    try {
      await raced(a.abort, peer.setRemoteDescription({ type: 'answer', sdp: answer.sdp }));
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      throw new AttemptError({ kind: 'closed', detail: 'setRemoteDescription', auto: this.current.everReady }, true);
    }
    const opened = deferred<void>();
    const failDirect = (detail: string): void =>
      opened.reject(new AttemptError({ kind: 'closed', detail, auto: this.current.everReady }, true));
    const timer = this.clock.setTimeout(() => failDirect('data_channel_timeout'), DATA_CHANNEL_OPEN_TIMEOUT_MS);
    link.onClose((reason) => failDirect(reason));
    peer.onconnectionstatechange = () => {
      // ICE gave up. (`disconnected` can still recover, and the keepalive judges a link that stays silent.)
      if (peer.connectionState === 'failed') link.close('ice_failed');
    };
    link.onOpen(() => opened.resolve());
    try {
      await raced(a.abort, opened.promise);
    } finally {
      this.clock.clearTimeout(timer);
    }
    return link;
  }

  /** Finish the tunnel: derive the traffic keys from the two ephemeral keys and listen for the host's frames. */
  private async openTunnel(a: Attempt, ephPrivate: WebCryptoKey, answer: AnswerRecord): Promise<FragmentLink> {
    const { uid, hostId, rtdb } = this.deps;
    const connId = a.connId;
    if (answer.eph === undefined) throw new AttemptError({ kind: 'protocol', detail: 'tunnel answer without a key', auto: false });
    let keys;
    try {
      const shared = await raced(a.abort, x25519(ephPrivate, b64uDecode(answer.eph)));
      keys = await raced(a.abort, deriveTunnelKeys(shared, connId));
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      throw new AttemptError({ kind: 'protocol', detail: error instanceof RemoteProtocolError ? error.message : 'tunnel key agreement failed', auto: false });
    }
    const tunnel = new TunnelLink({
      rtdb,
      connId,
      sendPath: upPath(uid, hostId, connId),
      sendDir: 'up',
      sendKey: keys.up,
      recvPath: downPath(uid, hostId, connId),
      recvDir: 'down',
      recvKey: keys.down,
      subscribe: true,
      clock: this.clock,
    });
    a.tunnel = tunnel;
    return tunnel;
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Handshake
  // ───────────────────────────────────────────────────────────────────────────

  private async handshake(
    a: Attempt,
    chosen: TransportKind,
    pairingCode: string,
    expectedHostKey: string,
    pin: PinnedHost | undefined,
  ): Promise<RemoteHostInfo> {
    const rpc = a.rpc!;
    let answered = false;
    // Before `initialize` has an answer, a dead direct link is the direct path's fault; afterwards it is not.
    const direct = (): boolean => chosen === 'webrtc' && !answered;
    const withClose = <T>(promise: Promise<T>): Promise<T> => raced(a.abort, Promise.race([promise, a.closed.promise]));

    let init: ViewerRequests['initialize']['result'];
    try {
      init = await withClose(
        rpc.request('initialize', { protocol: REMOTE_PROTOCOL_MAJOR, client_protocol_version: CLIENT_PROTOCOL_MAJOR, app: { version: displayText(this.appVersion, 64) || 'dev', platform: 'web' } }, { timeoutMs: HANDSHAKE_TIMEOUT_MS }),
      );
    } catch (error) {
      if (error instanceof Cancelled) throw error;
      if (error instanceof AttemptError) throw new AttemptError(error.failure, direct());
      if (error instanceof RemoteRpcError) {
        if (error.code === 'upgrade_required') throw new AttemptError({ kind: 'upgrade_required', auto: false });
        if (error.code === 'capacity') throw new AttemptError({ kind: 'capacity', auto: true });
        if (error.code === 'timeout' || error.code === 'closed') {
          throw new AttemptError({ kind: 'closed', detail: error.code === 'timeout' ? 'handshake_timeout' : a.closeReason, auto: this.current.everReady }, direct());
        }
        throw new AttemptError({ kind: 'protocol', detail: error.code, auto: false });
      }
      throw error;
    }
    answered = true;

    let host: RemoteHostInfo;
    if (init.status === 'approved') {
      host = init.host;
    } else if (init.status === 'pending_approval') {
      if (init.pairing_code !== pairingCode) {
        throw new AttemptError({ kind: 'pairing_code_mismatch', auto: false });
      }
      this.update({ phase: 'awaiting_approval', transport: chosen, pairingCode });
      try {
        host = await withClose(a.approval.promise);
      } catch (error) {
        if (error instanceof AttemptError || error instanceof Cancelled) throw error;
        throw new AttemptError({ kind: 'closed', detail: a.closeReason, auto: false });
      }
    } else {
      throw new AttemptError({ kind: 'protocol', detail: 'unknown initialize status', auto: false });
    }

    // The computer has accepted this browser: remember its key for every later answer.
    const hostName = displayText(host.name, 64) || this.deps.hostId;
    if (pin?.name !== hostName) {
      // Not awaited: notifications may already be arriving, and a slow disk must not hold them up.
      void this.deps.keys
        .pin({ host_id: this.deps.hostId, host_key: expectedHostKey, name: hostName, paired_at: pin?.paired_at ?? this.clock.now() })
        .catch(() => undefined);
    }
    a.ready = true;
    return host;
  }

  private receive(a: Attempt, method: string, params: unknown): void {
    if (method === 'auth.approved') {
      const host = (params as { host?: RemoteHostInfo } | null)?.host;
      if (host !== undefined && host !== null && typeof host.name === 'string') a.approval.resolve(host);
      else a.approval.reject(new AttemptError({ kind: 'protocol', detail: 'auth.approved without host', auto: false }));
      return;
    }
    if (method === 'auth.denied') {
      if (a.ready) {
        // An approved connection is being ended (the device was revoked, remote control was switched off).
        a.notice = displayText((params as { reason?: unknown } | null)?.reason, 40) || 'denied';
        a.rpc?.close(`notice:${a.notice}`);
        return;
      }
      a.approval.reject(new AttemptError({ kind: 'denied', detail: displayText((params as { reason?: unknown } | null)?.reason, 40) || undefined, auto: false }));
      return;
    }
    if (!a.ready) return;
    if (this.live !== a) a.held.push([method, params]); // approved, but not published as ready yet
    else this.deliver(method, params);
  }

  private deliver(method: string, params: unknown): void {
    for (const listener of [...this.notificationListeners]) {
      try {
        listener(method, params);
      } catch {
        // A failing listener must not stop the others.
      }
    }
  }

  private linkClosed(a: Attempt, reason: string): void {
    a.closeReason = reason;
    a.closed.reject(new AttemptError({ kind: 'closed', detail: reason, auto: this.current.everReady }));
    if (this.live !== a) return;
    // A link that was up: clean up and reconnect.
    this.live = undefined;
    this.dispose(a, reason);
    if (a.notice !== undefined) {
      const failure: ConnectionFailure =
        a.notice === 'revoked' ? { kind: 'revoked', auto: false }
        : a.notice === 'disabled' ? { kind: 'disabled', auto: false }
        : { kind: 'denied', detail: a.notice, auto: false };
      if (failure.kind === 'revoked') void this.deps.keys.unpair(this.deps.hostId).catch(() => undefined);
      this.fail(failure);
      return;
    }
    if (this.clock.now() - this.readyAt >= SSE_BACKOFF_RESET_MS) this.failures = 0;
    this.fail({ kind: 'closed', detail: reason, auto: true });
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Cleanup
  // ───────────────────────────────────────────────────────────────────────────

  /** Release everything an attempt holds and remove its database nodes. Safe to call twice. */
  private dispose(a: Attempt, reason: string): void {
    if (a.disposed) return;
    a.disposed = true;
    a.abort.abort();
    a.answers?.close();
    a.approval.reject(new Cancelled());
    a.closed.reject(new Cancelled());
    if (a.rpc !== undefined) a.rpc.close(reason);
    else if (a.tunnel !== undefined) a.tunnel.close(reason);
    else a.dataLink?.close(reason);
    a.peer?.close();
    const { uid, hostId, rtdb } = this.deps;
    if (a.connId === '') return;
    const connId = a.connId;
    const removeNodes = (): void => {
      void Promise.allSettled([
        rtdb.remove(toHostConnPath(uid, hostId, connId)),
        rtdb.remove(toViewerConnPath(uid, hostId, connId)),
      ]);
    };
    // The tunnel flushes its close notice and deletes its own frames first; the rest is removed after.
    if (a.tunnel !== undefined) void a.tunnel.whenClosed().then(removeNodes, removeNodes);
    else if (a.offerWritten) removeNodes();
  }
}
