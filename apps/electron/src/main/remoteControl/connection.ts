/**
 * One viewer connection, from its offer to its end. The host checks who is
 * asking (signature, policy, capacity), picks the transport, answers, waits
 * for the link to come up, and then runs the handshake over it:
 *
 *   offer ─ verify ─ policy ─ transport ─ answer ─ link up ─ `initialize`
 *     ─ approved ──────────────────────────────────────────→ gateway
 *     └ unknown device ─ pairing code ─ dialog ─ allow ───→ gateway
 *                                                └ deny ───→ closed
 *
 * Until a device is approved every method but `initialize` and `ping` answers
 * `not_approved`. Whatever ends the connection (the link, the keepalive, a
 * revocation, the policy, the app) takes the one way out, `close()`.
 */

import {
  CLIENT_PROTOCOL_MAJOR,
  MAX_HOST_CONNECTIONS,
  MessageTransport,
  FileChannel,
  REMOTE_FEATURE_BOTS,
  REMOTE_PROTOCOL_MAJOR,
  RemoteProtocolError,
  RemoteRpcError,
  RPC_DEFAULT_TIMEOUT_MS,
  RpcPeer,
  TunnelLink,
  b64uDecode,
  b64uEncode,
  deriveTunnelKeys,
  downPath,
  generateEphemeralKey,
  isB64u,
  pairingCodeFor,
  parseOfferRecord,
  randomBytes,
  signAnswer,
  upPath,
  verifyOffer,
  x25519,
  type AnswerDraft,
  type AnswerRejectReason,
  type Clock,
  type DevicePlatform,
  type FragmentLink,
  type InitializeResult,
  type OfferRecord,
  type RemoteMediaCapabilities,
  type RemoteMediaMode,
  type RemoteMediaSourceKind,
  type RemoteHostInfo,
  type Rtdb,
  type SigningContext,
  type TransportKind,
  type TunnelKeys,
} from '@lingxi/bridge-client/remote';

import type { RtcCandidateType } from '../../shared/remoteRtc.js';
import type { ApprovalDialog } from './approvalDialog.js';
import type { DeviceRegistry } from './deviceRegistry.js';
import { describeError, type RemoteDiagnostics } from './diagnostics.js';
import type { RemoteSessionGateway, RemoteViewer, RemoteViewerChannel } from './gateway.js';
import type { HostIdentity } from './hostIdentity.js';
import type { PeerHost, PeerLink } from './peerHost.js';
import type { HostSignaling } from './signalingHost.js';

export interface RemoteConnectionStatus {
  conn_id: string;
  device_id: string;
  device_name: string;
  platform: DevicePlatform;
  /** The transport in use, which can differ from the one the viewer asked for. */
  transport: TransportKind;
  approved: boolean;
  /** For WebRTC: how the two ends reached each other (`host` is the same network, `relay` a TURN server). */
  candidate_type?: RtcCandidateType;
}

export interface HostConnectionDeps {
  uid: string;
  identity: HostIdentity;
  /** What an approved viewer is told about this computer. */
  hostInfo: RemoteHostInfo;
  connId: string;
  rtdb: Rtdb;
  signaling: Pick<HostSignaling, 'writeAnswer' | 'removeOffer' | 'removeConnection'>;
  peers: Pick<PeerHost, 'answer' | 'link'>;
  registry: Pick<DeviceRegistry, 'isApproved' | 'approve' | 'isRevoked' | 'touch'>;
  approval: Pick<ApprovalDialog, 'request'>;
  gateway: RemoteSessionGateway;
  config: { forceTunnel: boolean; stunUrls: readonly string[] };
  /** Whether Remote Control is switched on and allowed right now. */
  enabled(): boolean;
  /** How many connections have already been accepted, this one not counted. */
  acceptedCount(): number;
  clock: Clock;
  diagnostics: RemoteDiagnostics;
  /** What the status shows about this connection changed. */
  onChange(): void;
  /** The connection ended (called once, before its cleanup finishes). */
  onClosed(connection: HostConnection): void;
}

// How long the link may take to come up once the answer is written.
const DATA_CHANNEL_WAIT_MS = 20_000;
const TUNNEL_FIRST_FRAME_WAIT_MS = 30_000;
// How long a link that is up may take to deliver `initialize`.
const INITIALIZE_WAIT_MS = 15_000;
// How long a denial or revocation notice may take to leave before the link is closed anyway.
const NOTICE_WAIT_MS = 2_000;
// How long closing waits for a tunnel to flush its last frames before the connection's nodes are deleted.
const FLUSH_WAIT_MS = 3_000;
// Tunnel frames kept for a connection whose tunnel does not exist yet.
const MAX_EARLY_FRAMES = 1024;
const MAX_PUSH_ID_CHARS = 256;
const CONTROL = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;
const PLATFORMS: readonly string[] = ['web', 'android', 'ios'];

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The nonce of an offer written by a newer protocol (`v` above 1): it cannot be verified, but it can be told to upgrade. */
function newerOfferNonce(record: unknown): string | undefined {
  if (!isObject(record)) return undefined;
  const { v, nonce } = record;
  return typeof v === 'number' && Number.isFinite(v) && v > 1 && isB64u(nonce, 16) ? nonce : undefined;
}

interface InitializeRequest {
  pushInstallationId?: string;
  mediaCapabilities?: RemoteMediaCapabilities;
  botAware?: true;
}

const MAX_FEATURES = 32;
const MAX_FEATURE_CHARS = 64;

/** Whether the viewer declared `REMOTE_FEATURE_BOTS`; the other strings it lists are none of the host's business. */
function parseBotAware(value: unknown, invalid: (what: string) => RemoteRpcError): boolean {
  if (value === undefined) return false;
  if (!Array.isArray(value) || value.length > MAX_FEATURES
    || !value.every(feature => typeof feature === 'string' && feature.length <= MAX_FEATURE_CHARS)) {
    throw invalid('bad app.features');
  }
  return value.includes(REMOTE_FEATURE_BOTS);
}

function parseMediaCapabilities(value: unknown, invalid: (what: string) => RemoteRpcError): RemoteMediaCapabilities | undefined {
  if (value === undefined) return undefined;
  if (!isObject(value) || value['version'] !== 1 || !Array.isArray(value['modes']) || !Array.isArray(value['source_kinds'])) {
    throw invalid('bad app.media_capabilities');
  }
  const modes = value['modes'];
  const sourceKinds = value['source_kinds'];
  const validModes = modes.every(mode => mode === 'audio' || mode === 'video' || mode === 'audio_video');
  const validKinds = sourceKinds.every(kind => kind === 'microphone' || kind === 'camera' || kind === 'screen');
  if (!validModes || !validKinds || modes.length < 1 || modes.length > 3 || sourceKinds.length < 1 || sourceKinds.length > 3
    || new Set(modes).size !== modes.length || new Set(sourceKinds).size !== sourceKinds.length) {
    throw invalid('bad app.media_capabilities');
  }
  const audioModes = (modes as unknown[]).some(mode => mode === 'audio' || mode === 'audio_video');
  const videoModes = (modes as unknown[]).some(mode => mode === 'video' || mode === 'audio_video');
  if (audioModes !== sourceKinds.includes('microphone')
    || videoModes !== sourceKinds.some(kind => kind === 'camera' || kind === 'screen')) {
    throw invalid('inconsistent app.media_capabilities');
  }
  return {
    version: 1,
    modes: modes as RemoteMediaMode[],
    source_kinds: sourceKinds as RemoteMediaSourceKind[],
  };
}

function parseInitialize(params: unknown): InitializeRequest {
  const invalid = (what: string): RemoteRpcError => new RemoteRpcError('invalid_params', `initialize: ${what}`);
  if (!isObject(params)) throw invalid('params must be an object');
  const { protocol, client_protocol_version: client, app, push_installation_id: push } = params;
  if (typeof protocol === 'number' && protocol !== REMOTE_PROTOCOL_MAJOR) {
    throw new RemoteRpcError('upgrade_required', `this computer speaks lingxi-rc/${REMOTE_PROTOCOL_MAJOR}; the app speaks lingxi-rc/${protocol}`);
  }
  if (protocol !== REMOTE_PROTOCOL_MAJOR) throw invalid('bad protocol');
  if (typeof client !== 'number' || !Number.isInteger(client)) throw invalid('bad client_protocol_version');
  if (client !== CLIENT_PROTOCOL_MAJOR) {
    throw new RemoteRpcError('upgrade_required', `this computer speaks client protocol ${CLIENT_PROTOCOL_MAJOR}; the app speaks ${client}`);
  }
  if (!isObject(app) || typeof app['version'] !== 'string' || app['version'].length < 1 || app['version'].length > 64) throw invalid('bad app.version');
  if (typeof app['platform'] !== 'string' || !PLATFORMS.includes(app['platform'])) throw invalid('bad app.platform');
  const mediaCapabilities = parseMediaCapabilities(app['media_capabilities'], invalid);
  const request: InitializeRequest = mediaCapabilities === undefined ? {} : { mediaCapabilities };
  if (parseBotAware(app['features'], invalid)) request.botAware = true;
  if (push !== undefined) {
    if (typeof push !== 'string' || push.length < 1 || push.length > MAX_PUSH_ID_CHARS || CONTROL.test(push)) throw invalid('bad push_installation_id');
    request.pushInstallationId = push;
  }
  return request;
}

/** `promise`, or `undefined` once `ms` have passed on `clock`. */
function within<T>(clock: Clock, ms: number, promise: Promise<T>): Promise<T | undefined> {
  return new Promise<T | undefined>((resolve) => {
    const timer = clock.setTimeout(() => resolve(undefined), ms);
    promise.then(
      (value) => { clock.clearTimeout(timer); resolve(value); },
      () => { clock.clearTimeout(timer); resolve(undefined); },
    );
  });
}

export class HostConnection {
  readonly connId: string;
  /** Resolves when the connection has ended and its leftovers in the database are gone (or given up on). */
  readonly finished: Promise<void>;

  private readonly abort = new AbortController();
  private readonly early = new Map<number, unknown>();
  private readonly channel: RemoteViewerChannel;
  private resolveFinished!: () => void;
  private offer: OfferRecord | undefined;
  private answered: { host: { id: string; key: string }; nonce: string } | undefined;
  private kind: TransportKind | undefined;
  private peer: PeerLink | undefined;
  private tunnel: TunnelLink | undefined;
  private rpc: RpcPeer | undefined;
  private fileChannel: FileChannel | undefined;
  private viewer: RemoteViewer | undefined;
  private pairingCode: string | undefined;
  private transportTimer: unknown;
  private initializeTimer: unknown;
  private transportIsUp = false;
  private handshake: Promise<void> | undefined;
  private accepted = false;
  private approved = false;
  private wasApproved = false;
  private closed = false;
  /** Application traffic stops immediately, while a final denial may flush. */
  private disconnecting = false;
  private viewerReleased = false;
  // Whether this connection's nodes in the database are to be removed once it ends.
  private removeNodes = false;

  constructor(private readonly deps: HostConnectionDeps) {
    this.connId = deps.connId;
    this.finished = new Promise<void>((resolve) => { this.resolveFinished = resolve; });
    const connection = this;
    this.channel = {
      get files() {
        const link = connection.peer?.files;
        if (!connection.approved || connection.kind !== 'webrtc' || !link?.connected) return undefined;
        return connection.fileChannel ??= new FileChannel(link);
      },
      notify: (method, params) => {
        if (this.closed || this.disconnecting) return;
        this.rpc?.notify(method, params).catch((error: unknown) => {
          this.deps.diagnostics('warn', `a notification to a viewer could not be sent (${describeError(error)})`);
        });
      },
      request: (method, params) => {
        if (this.closed || this.disconnecting || !this.approved) {
          return Promise.reject(new RemoteRpcError('not_approved', 'this device has not been approved'));
        }
        const rpc = this.rpc;
        if (rpc === undefined || rpc.closed) {
          return Promise.reject(new RemoteRpcError('closed', 'the connection is closed'));
        }
        return rpc.request(method, params, { timeoutMs: RPC_DEFAULT_TIMEOUT_MS });
      },
    };
  }

  /** Whether this connection has been accepted (it counts towards the capacity). */
  get isAccepted(): boolean {
    return this.accepted && !this.closed;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  get deviceId(): string | undefined {
    return this.offer?.device.id;
  }

  /** What the status shows; `undefined` until the connection is accepted and has a transport. */
  status(): RemoteConnectionStatus | undefined {
    const { offer } = this;
    if (this.closed || !this.accepted || offer === undefined || this.kind === undefined) return undefined;
    const status: RemoteConnectionStatus = {
      conn_id: this.connId,
      device_id: offer.device.id,
      device_name: offer.device.name,
      platform: offer.device.platform,
      transport: this.kind,
      approved: this.approved,
    };
    const candidate = this.peer?.candidate;
    if (candidate !== undefined) status.candidate_type = candidate;
    return status;
  }

  /** Start handling the value read from `…/offer`. */
  handleOffer(record: unknown): void {
    this.run(record).catch((error: unknown) => {
      this.deps.diagnostics('warn', `a connection failed (${describeError(error)})`);
      this.close('error');
    });
  }

  /** A tunnel frame the viewer wrote at `up/f{seq}`. */
  deliverFrame(seq: number, value: unknown): void {
    if (this.closed) return;
    if (this.tunnel !== undefined) this.tunnel.deliverFrame(seq, value);
    else if (this.kind === undefined && this.early.size < MAX_EARLY_FRAMES) this.early.set(seq, value);
  }

  /** End the connection. Idempotent; the in-memory part is done when this returns, the database cleanup follows. */
  close(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    this.fileChannel?.close();
    this.deps.clock.clearTimeout(this.transportTimer);
    this.deps.clock.clearTimeout(this.initializeTimer);
    this.abort.abort();
    this.early.clear();
    const { rpc, tunnel, viewer } = this;
    if (rpc !== undefined) rpc.close(reason);
    else tunnel?.close(reason);
    this.releaseViewer();
    if (this.wasApproved && viewer !== undefined && this.offer !== undefined) this.deps.registry.touch(this.deps.uid, this.offer.device.id);
    this.deps.onClosed(this);
    this.deps.onChange();
    void this.cleanUp().finally(() => this.resolveFinished());
  }

  private releaseViewer(): void {
    if (this.wasApproved && this.viewer !== undefined && !this.viewerReleased) {
      this.viewerReleased = true;
      try {
        this.deps.gateway.viewerClosed(this.viewer);
      } catch (error) {
        this.deps.diagnostics('warn', `the session gateway failed to release a viewer (${describeError(error)})`);
      }
    }
  }

  /** Tell the viewer why (best effort, briefly), then end the connection. */
  async disconnect(reason: string, notice: string): Promise<void> {
    if (this.closed || this.disconnecting) return;
    this.disconnecting = true;
    this.approved = false;
    this.fileChannel?.close();
    this.releaseViewer();
    this.deps.onChange();
    const { rpc } = this;
    if (rpc !== undefined && this.transportIsUp) await within(this.deps.clock, NOTICE_WAIT_MS, rpc.notify('auth.denied', { reason: notice }));
    this.close(reason);
  }

  // ───────────────────────────────────────────────────────────────────────────
  // From offer to answer
  // ───────────────────────────────────────────────────────────────────────────

  private async run(record: unknown): Promise<void> {
    const { uid, identity, clock, config } = this.deps;
    const context: SigningContext = { uid, hostId: identity.hostId, connId: this.connId };

    const newer = newerOfferNonce(record);
    if (newer !== undefined) return this.reject(context, newer, 'upgrade_required');

    const verdict = await verifyOffer({ ...context, record, now: clock.now() });
    if (this.closed) return;
    if (!verdict.ok) {
      // A malformed record is dropped without an answer; a forged or stale one is told no.
      const parsed = verdict.reason === 'bad_shape' ? undefined : parseOfferRecord(record);
      if (parsed?.ok) return this.reject(context, parsed.value.nonce, verdict.reason === 'expired' ? 'expired' : 'bad_signature');
      this.deps.diagnostics('info', `an offer was dropped (${verdict.reason}${verdict.detail ? `: ${verdict.detail}` : ''})`);
      this.removeNodes = true;
      return this.close('dropped');
    }
    const offer = verdict.value;
    this.offer = offer;

    if (!this.deps.enabled()) return this.reject(context, offer.nonce, 'disabled');
    if (this.deps.registry.isRevoked(uid, offer.device.id)) return this.reject(context, offer.nonce, 'revoked');
    if (this.deps.acceptedCount() >= MAX_HOST_CONNECTIONS) return this.reject(context, offer.nonce, 'capacity');
    this.accepted = true;
    this.deps.onChange();

    const nonce = b64uEncode(randomBytes(16));
    const host = { id: identity.hostId, key: identity.hostKey };
    let draft: AnswerDraft | undefined;

    if (offer.transport === 'webrtc' && !config.forceTunnel) {
      try {
        const sdp = await this.deps.peers.answer(this.connId, offer.sdp!, config.stunUrls);
        const link = this.deps.peers.link(this.connId);
        if (this.closed) return link.close('connection_closed');
        this.peer = link;
        this.adopt(link, 'webrtc');
        link.onState((state) => {
          if (state !== 'connected') return; // a failed or closed link ends this connection through its transport
          this.deps.onChange();
          this.transportUp();
        });
        draft = { v: 1, status: 'accepted', transport: 'webrtc', sdp, host, nonce };
      } catch (error) {
        // The hidden window is an optimization: a failure there only means the viewer gets the tunnel.
        this.deps.diagnostics('info', `WebRTC was not available for a connection (${describeError(error)}); using the tunnel`);
        if (this.closed) return;
      }
    }

    if (draft === undefined) {
      let ephemeral: Awaited<ReturnType<typeof generateEphemeralKey>>;
      let keys: TunnelKeys;
      try {
        ephemeral = await generateEphemeralKey();
        keys = await deriveTunnelKeys(await x25519(ephemeral.privateKey, b64uDecode(offer.eph)), this.connId);
      } catch (error) {
        if (this.closed) return;
        if (error instanceof RemoteProtocolError) return this.reject(context, offer.nonce, 'bad_signature');
        throw error;
      }
      if (this.closed) return;
      this.adoptTunnel(keys);
      draft = { v: 1, status: 'accepted', transport: 'tunnel', host, eph: b64uEncode(ephemeral.publicKey), nonce };
    }

    this.answered = { host, nonce };
    const answer = await signAnswer(context, identity.privateKey, offer.nonce, draft);
    if (this.closed) return;
    this.removeNodes = true;
    await this.deps.signaling.writeAnswer(this.connId, answer);
    try {
      await this.deps.signaling.removeOffer(this.connId);
    } catch (error) {
      this.deps.diagnostics('info', `an answered offer could not be removed (${describeError(error)})`);
    }
    if (this.closed) return;
    this.waitForTransport();
  }

  /** Answer `rejected` and end the connection; the viewer deletes the answer once it has read it. */
  private async reject(context: SigningContext, offerNonce: string, reason: AnswerRejectReason): Promise<void> {
    const { identity } = this.deps;
    try {
      const draft: AnswerDraft = {
        v: 1,
        status: 'rejected',
        reason,
        host: { id: identity.hostId, key: identity.hostKey },
        nonce: b64uEncode(randomBytes(16)),
      };
      const answer = await signAnswer(context, identity.privateKey, offerNonce, draft);
      if (this.closed) return;
      await this.deps.signaling.writeAnswer(this.connId, answer);
      await this.deps.signaling.removeOffer(this.connId);
    } catch (error) {
      this.deps.diagnostics('warn', `a rejection could not be delivered (${describeError(error)})`);
    }
    this.close(`rejected:${reason}`);
  }

  // ───────────────────────────────────────────────────────────────────────────
  // The link
  // ───────────────────────────────────────────────────────────────────────────

  private adopt(link: FragmentLink, kind: TransportKind): void {
    this.kind = kind;
    const transport = new MessageTransport(link);
    const rpc = new RpcPeer(transport, { clock: this.deps.clock });
    rpc.onRequest((method, params) => this.onRequest(method, params));
    rpc.onNotification((method, params) => this.onNotification(method, params));
    transport.onClose((reason) => this.close(reason));
    this.rpc = rpc;
    this.deps.onChange();
  }

  private adoptTunnel(keys: TunnelKeys): void {
    const { uid, identity, rtdb, clock } = this.deps;
    const tunnel = new TunnelLink({
      rtdb,
      connId: this.connId,
      sendPath: downPath(uid, identity.hostId, this.connId),
      sendDir: 'down',
      sendKey: keys.down,
      recvPath: upPath(uid, identity.hostId, this.connId),
      recvDir: 'up',
      recvKey: keys.up,
      subscribe: false,
      clock,
    });
    this.tunnel = tunnel;
    const up = (): void => this.transportUp();
    // The tunnel is up when its first frame arrives; the wrapper notices.
    this.adopt({
      maxFragmentBytes: tunnel.maxFragmentBytes,
      sendFragment: (fragment) => tunnel.sendFragment(fragment),
      onFragment: (callback) => tunnel.onFragment((fragment) => { up(); callback(fragment); }),
      onClose: (callback) => tunnel.onClose(callback),
      close: (reason) => tunnel.close(reason),
      whenWritable: () => tunnel.whenWritable(),
    }, 'tunnel');
    for (const [seq, value] of this.early) tunnel.deliverFrame(seq, value);
    this.early.clear();
  }

  private waitForTransport(): void {
    if (this.transportIsUp || this.closed) return;
    const tunnel = this.kind === 'tunnel';
    this.transportTimer = this.deps.clock.setTimeout(
      () => this.close(tunnel ? 'tunnel_timeout' : 'datachannel_timeout'),
      tunnel ? TUNNEL_FIRST_FRAME_WAIT_MS : DATA_CHANNEL_WAIT_MS,
    );
  }

  private transportUp(): void {
    if (this.closed || this.transportIsUp) return;
    this.transportIsUp = true;
    this.deps.clock.clearTimeout(this.transportTimer);
    // initialize can arrive before the renderer's asynchronous connected report.
    // A valid handshake (including pending approval) already met this deadline.
    if (this.viewer === undefined) {
      this.initializeTimer = this.deps.clock.setTimeout(() => this.close('initialize_timeout'), INITIALIZE_WAIT_MS);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // The handshake and what follows
  // ───────────────────────────────────────────────────────────────────────────

  private onRequest(method: string, params: unknown): Promise<unknown> | unknown {
    if (this.closed || this.disconnecting) throw new RemoteRpcError('closed', 'the connection is closing');
    if (method === 'initialize') return this.initialize(params);
    if (method === 'ping') return {};
    const { viewer } = this;
    if (!this.approved || viewer === undefined) throw new RemoteRpcError('not_approved', 'this device has not been approved yet');
    return this.deps.gateway.handle(viewer, method, params, this.channel).catch((error: unknown) => {
      if (!(error instanceof RemoteRpcError)) this.deps.diagnostics('warn', `a remote request failed (${method}: ${describeError(error)})`);
      throw error;
    });
  }

  private onNotification(method: string, params: unknown): void {
    const { viewer } = this;
    if (this.closed || this.disconnecting || !this.approved || viewer === undefined) return;
    Promise.resolve().then(() => {
      if (this.closed || this.disconnecting || !this.approved) return;
      return this.deps.gateway.handle(viewer, method, params, this.channel);
    }).catch((error: unknown) => {
      this.deps.diagnostics('warn', `a remote notification failed (${method}: ${describeError(error)})`);
    });
  }

  private async initialize(params: unknown): Promise<InitializeResult> {
    const request = parseInitialize(params);
    // The first call does the work; a repeat (or a concurrent one) waits for it and gets the current answer.
    this.handshake ??= this.beginHandshake(request);
    await this.handshake;
    if (this.closed || this.disconnecting) throw new RemoteRpcError('closed', 'the connection is closing');
    return this.approved
      ? { status: 'approved', host: this.deps.hostInfo }
      : { status: 'pending_approval', pairing_code: this.pairingCode! };
  }

  private async beginHandshake(request: InitializeRequest): Promise<void> {
    const { offer, answered } = this;
    if (offer === undefined || answered === undefined) throw new RemoteRpcError('internal', 'the connection is not ready');
    const { uid, registry } = this.deps;
    this.deps.clock.clearTimeout(this.initializeTimer);
    this.viewer = {
      account_uid: uid,
      host_id: this.deps.identity.hostId,
      device_id: offer.device.id,
      device_name: offer.device.name,
      platform: offer.device.platform,
      ...(request.mediaCapabilities === undefined ? {} : { media_capabilities: request.mediaCapabilities }),
      ...(request.botAware === undefined ? {} : { bot_aware: true as const }),
      conn_id: this.connId,
      ...(request.pushInstallationId === undefined ? {} : { push_installation_id: request.pushInstallationId }),
    };
    // A valid request also proves the transport opened, even if its stats report stalls.
    this.transportUp();
    if (registry.isApproved(uid, offer.device.id, offer.device.key)) {
      this.approve();
      registry.touch(uid, offer.device.id);
      return;
    }
    this.pairingCode = await pairingCodeFor(offer, answered);
    void this.pair(this.pairingCode);
  }

  private approve(): void {
    this.approved = true;
    this.wasApproved = true;
    this.rpc?.startKeepalive(this.kind === 'tunnel' ? 'tunnel' : 'datachannel');
    this.deps.onChange();
  }

  /** Ask the user, then tell the viewer. */
  private async pair(code: string): Promise<void> {
    if (this.closed || this.disconnecting) return;
    const { uid, registry, approval } = this.deps;
    const offer = this.offer!;
    const outcome = await approval.request({
      deviceId: offer.device.id,
      deviceName: offer.device.name,
      platform: offer.device.platform,
      pairingCode: code,
      signal: this.abort.signal,
      alreadyApproved: () => registry.isApproved(uid, offer.device.id, offer.device.key),
    });
    if (this.closed || this.disconnecting) return;
    if (outcome.allowed) {
      registry.approve(uid, { device_id: offer.device.id, key: offer.device.key, name: offer.device.name, platform: offer.device.platform });
      this.approve();
      void this.rpc?.notify('auth.approved', { host: this.deps.hostInfo });
      return;
    }
    await within(this.deps.clock, NOTICE_WAIT_MS, this.rpc?.notify('auth.denied', { reason: outcome.reason }) ?? Promise.resolve());
    this.close(`denied:${outcome.reason}`);
  }

  // ───────────────────────────────────────────────────────────────────────────
  // After the end
  // ───────────────────────────────────────────────────────────────────────────

  private async cleanUp(): Promise<void> {
    if (this.tunnel !== undefined) await within(this.deps.clock, FLUSH_WAIT_MS, this.tunnel.whenClosed());
    if (!this.removeNodes) return;
    try {
      await this.deps.signaling.removeConnection(this.connId);
    } catch (error) {
      this.deps.diagnostics('info', `a connection's leftovers could not be removed (${describeError(error)})`);
    }
  }
}
