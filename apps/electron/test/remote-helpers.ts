/**
 * Shared harness of the Remote Control host tests (not a test file itself):
 * fakes for everything the host talks to, and a simulated viewer that dials it
 * over the RTDB tunnel (or over an in-memory link standing in for a
 * DataChannel), speaking the real protocol through `@lingxi/bridge-client/remote`.
 *
 * Time is a `ManualClock`. WebCrypto and the tunnel's frame writes need real
 * event-loop turns, so waits go through `drive`, which gives the loop a turn
 * and then moves the fake clock forward a little.
 */

import {
  CLIENT_PROTOCOL_MAJOR,
  FRAGMENT_MAX_BYTES,
  ManualClock,
  MemoryRtdb,
  MessageTransport,
  REMOTE_PROTOCOL_MAJOR,
  RemoteRpcError,
  RpcPeer,
  TunnelLink,
  answerPath,
  b64uDecode,
  b64uEncode,
  deriveTunnelKeys,
  downPath,
  generateEphemeralKey,
  generateIdentityKey,
  hostPath,
  idFromKey,
  offerPath,
  randomBytes,
  randomId,
  signOffer,
  upPath,
  verifyAnswer,
  x25519,
  type AnswerRecord,
  type DevicePlatform,
  type FragmentLink,
  type IdentityKey,
  type OfferDraft,
  type RemoteErrorCode,
  type Rtdb,
} from '@lingxi/bridge-client/remote';

import type { ApprovalOutcome, ApprovalRequest } from '../src/main/remoteControl/approvalDialog.js';
import { DeviceRegistry, type RegistryFiles } from '../src/main/remoteControl/deviceRegistry.js';
import type { RemoteSessionGateway, RemoteViewer, RemoteViewerChannel } from '../src/main/remoteControl/gateway.js';
import type { HostIdentity } from '../src/main/remoteControl/hostIdentity.js';
import type { PeerLink } from '../src/main/remoteControl/peerHost.js';
import {
  RemoteControlService,
  type RemoteControlAccount,
  type RemoteControlSettings,
  type RemoteControlStatus,
} from '../src/main/remoteControl/RemoteControlService.js';
import type { RtcCandidateType, RtcPeerState } from '../src/shared/remoteRtc.js';

export const START = 1_700_000_000_000;
export const UID = 'uid-1';

// ── Waiting ──────────────────────────────────────────────────────────────────

/** Give promise continuations and WebCrypto jobs a few event-loop turns. */
export async function settle(turns = 5): Promise<void> {
  for (let i = 0; i < turns; i++) await new Promise<void>((resolve) => setImmediate(resolve));
}

/** Wait, in real time, until `condition()` holds. */
export async function until(condition: () => boolean, label = 'condition', timeoutMs = 5000): Promise<void> {
  const start = Date.now();
  while (!condition()) {
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${label}`);
    await new Promise<void>((resolve) => setImmediate(resolve));
  }
}

/** Let real asynchronous work finish while fake time passes `stepMs` per event-loop turn. */
export async function drive(clock: ManualClock, done: () => boolean, label = 'condition', limit = 5000, stepMs = 25): Promise<void> {
  for (let turn = 0; turn < limit; turn++) {
    if (done()) return;
    await new Promise<void>((resolve) => setImmediate(resolve));
    clock.advance(stepMs);
  }
  throw new Error(`timed out waiting for ${label}`);
}

/** Let `ms` of fake time pass in small steps, with real turns in between for the work each step sets off. */
export async function elapse(clock: ManualClock, ms: number, stepMs = 250): Promise<void> {
  for (let passed = 0; passed < ms; passed += stepMs) {
    await new Promise<void>((resolve) => setImmediate(resolve));
    clock.advance(Math.min(stepMs, ms - passed));
  }
  await settle(10);
}

/** Drive the clock until `promise` settles, then return its value or throw its error. */
export async function driven<T>(clock: ManualClock, promise: Promise<T>, label = 'a promise'): Promise<T> {
  let outcome: { ok: true; value: T } | { ok: false; error: unknown } | undefined;
  promise.then((value) => { outcome = { ok: true, value }; }, (error: unknown) => { outcome = { ok: false, error }; });
  await drive(clock, () => outcome !== undefined, label);
  if (!outcome!.ok) throw outcome!.error;
  return outcome!.value;
}

/** Drive the clock until `promise` rejects with a `RemoteRpcError`; its code. */
export async function rpcErrorCode(clock: ManualClock, promise: Promise<unknown>): Promise<RemoteErrorCode> {
  try {
    await driven(clock, promise, 'an error response');
  } catch (error) {
    if (error instanceof RemoteRpcError) return error.code;
    throw error;
  }
  throw new Error('the request succeeded');
}

// ── Plain fakes ──────────────────────────────────────────────────────────────

export interface DiagnosticsLog {
  entries: Array<{ level: string; message: string }>;
  diagnostics: (level: 'info' | 'warn' | 'error', message: string) => void;
  messages(level?: string): string[];
}

export function diagnosticsLog(): DiagnosticsLog {
  const entries: Array<{ level: string; message: string }> = [];
  return {
    entries,
    diagnostics: (level, message) => { entries.push({ level, message }); },
    messages: (level) => entries.filter((entry) => level === undefined || entry.level === level).map((entry) => entry.message),
  };
}

/** An in-memory stand-in for `node:fs` as the device registry uses it. */
export class MemoryFiles implements RegistryFiles {
  readonly store = new Map<string, string>();
  readonly modes = new Map<string, number>();
  readonly log: string[] = [];
  /** Make every write fail. */
  failWrites = false;

  constructor(initial: Record<string, string> = {}) {
    for (const [path, text] of Object.entries(initial)) this.store.set(path, text);
  }

  mkdirSync(path: string, options: { recursive: true; mode: number }): void {
    this.log.push(`mkdir ${path} ${options.mode.toString(8)}`);
  }

  readFileSync(path: string): string {
    const text = this.store.get(path);
    if (text === undefined) throw Object.assign(new Error(`ENOENT: ${path}`), { code: 'ENOENT' });
    return text;
  }

  writeFileSync(path: string, data: string, options: { mode: number }): void {
    if (this.failWrites) throw new Error('disk full');
    this.store.set(path, data);
    this.modes.set(path, options.mode);
    this.log.push(`write ${path}`);
  }

  renameSync(from: string, to: string): void {
    const text = this.store.get(from);
    if (text === undefined) throw Object.assign(new Error(`ENOENT: ${from}`), { code: 'ENOENT' });
    this.store.set(to, text);
    this.store.delete(from);
    this.log.push(`rename ${from} ${to}`);
  }

  rmSync(path: string): void {
    this.store.delete(path);
    this.log.push(`rm ${path}`);
  }
}

export class FakeAccount implements RemoteControlAccount {
  user: { uid: string } | null;
  /** The `forceRefresh` argument of every `getIdToken` call. */
  readonly tokenCalls: Array<boolean | undefined> = [];
  private readonly listeners = new Set<() => void>();

  constructor(uid: string | null = UID) {
    this.user = uid === null ? null : { uid };
  }

  snapshot(): { user: { uid: string } | null } {
    return { user: this.user };
  }

  onChanged(callback: () => void): () => void {
    this.listeners.add(callback);
    return () => { this.listeners.delete(callback); };
  }

  async getIdToken(forceRefresh?: boolean): Promise<string> {
    this.tokenCalls.push(forceRefresh);
    if (this.user === null) throw new Error('请先登录。');
    return `token-${this.user.uid}`;
  }

  /** Switch account (or sign out with `null`) and tell the listeners, in the order the real service does. */
  set(uid: string | null): void {
    this.user = uid === null ? null : { uid };
    for (const listener of [...this.listeners]) listener();
  }
}

export class FakeSettings implements RemoteControlSettings {
  enabled: boolean;
  saves: boolean[] = [];

  constructor(enabled = true) {
    this.enabled = enabled;
  }

  getRemoteControlEnabled(): boolean {
    return this.enabled;
  }

  setRemoteControlEnabled(enabled: boolean): void {
    this.enabled = enabled;
    this.saves.push(enabled);
  }
}

/** Stands in for the approval dialog: answers at once, or holds each request until the test answers it. */
export class FakeApproval {
  readonly requests: ApprovalRequest[] = [];
  disposed = false;
  mode: 'wait' | 'allow' | ApprovalOutcome = 'wait';
  private readonly waiting: Array<(outcome: ApprovalOutcome) => void> = [];

  request(request: ApprovalRequest): Promise<ApprovalOutcome> {
    this.requests.push(request);
    if (this.mode === 'allow') return Promise.resolve({ allowed: true });
    if (this.mode !== 'wait') return Promise.resolve(this.mode);
    return new Promise<ApprovalOutcome>((resolve) => {
      this.waiting.push(resolve);
      request.signal.addEventListener('abort', () => resolve({ allowed: false, reason: 'cancelled' }), { once: true });
    });
  }

  /** Answer the oldest request that is still waiting. */
  answer(outcome: ApprovalOutcome): void {
    const resolve = this.waiting.shift();
    if (!resolve) throw new Error('no approval request is waiting');
    resolve(outcome);
  }

  dispose(): void {
    this.disposed = true;
  }
}

export class FakeGateway implements RemoteSessionGateway {
  readonly calls: Array<{ viewer: RemoteViewer; method: string; params: unknown; channel: RemoteViewerChannel }> = [];
  readonly closed: RemoteViewer[] = [];
  result: (method: string, params: unknown) => unknown = () => ({ ok: true });

  async handle(viewer: RemoteViewer, method: string, params: unknown, channel: RemoteViewerChannel): Promise<unknown> {
    this.calls.push({ viewer, method, params, channel });
    return this.result(method, params);
  }

  viewerClosed(viewer: RemoteViewer): void {
    this.closed.push(viewer);
  }
}

// ── An in-memory link in place of a DataChannel ──────────────────────────────

class Endpoint implements FragmentLink {
  readonly maxFragmentBytes = FRAGMENT_MAX_BYTES;
  closeReason: string | undefined;
  peer!: Endpoint;
  /** Fragments handed to `sendFragment`, for inspection. */
  readonly sent: Uint8Array[] = [];
  private handler: ((fragment: Uint8Array) => void) | undefined;
  private readonly closers: Array<(reason: string) => void> = [];

  sendFragment(fragment: Uint8Array): void {
    if (this.closeReason !== undefined) throw new Error(`link is closed: ${this.closeReason}`);
    const copy = fragment.slice();
    this.sent.push(copy);
    setImmediate(() => this.peer.deliver(copy));
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.handler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    if (this.closeReason === undefined) this.closers.push(callback);
    else queueMicrotask(() => callback(this.closeReason!));
  }

  close(reason: string): void {
    if (this.closeReason !== undefined) return;
    this.closeReason = reason;
    for (const closer of this.closers.splice(0)) closer(reason);
    setImmediate(() => this.peer.close('peer_closed'));
  }

  deliver(fragment: Uint8Array): void {
    if (this.closeReason === undefined) this.handler?.(fragment);
  }
}

/** The host's end of a pretend WebRTC connection. */
export class FakePeerLink extends Endpoint implements PeerLink {
  connected = false;
  candidate: RtcCandidateType | undefined;
  private readonly stateHandlers: Array<(state: RtcPeerState, candidate?: RtcCandidateType) => void> = [];

  constructor(readonly connId: string) {
    super();
  }

  onState(handler: (state: RtcPeerState, candidate?: RtcCandidateType) => void): void {
    this.stateHandlers.push(handler);
    if (this.connected) queueMicrotask(() => handler('connected', this.candidate));
  }

  /** The DataChannel opened. */
  open(candidate?: RtcCandidateType): void {
    this.connected = true;
    this.candidate = candidate;
    for (const handler of this.stateHandlers) handler('connected', candidate);
  }

  override close(reason: string): void {
    if (this.closeReason !== undefined) return;
    super.close(reason);
    for (const handler of this.stateHandlers) handler('closed');
  }
}

/** A host link and the viewer's end of it. */
export function linkPair(connId: string): { host: FakePeerLink; viewer: FragmentLink & { sent: Uint8Array[] } } {
  const host = new FakePeerLink(connId);
  const viewer = new Endpoint();
  host.peer = viewer;
  viewer.peer = host;
  return { host, viewer };
}

/** Stands in for `PeerHost`: answers like a page would, or fails, and hands out the host's link ends. */
export class FakePeers {
  readonly answers: Array<{ connId: string; offerSdp: string; ice: readonly string[] }> = [];
  readonly pairs = new Map<string, ReturnType<typeof linkPair>>();
  failWith: Error | undefined;
  shutdowns = 0;
  disposed = false;

  async answer(connId: string, offerSdp: string, ice: readonly string[]): Promise<string> {
    this.answers.push({ connId, offerSdp, ice });
    if (this.failWith) throw this.failWith;
    this.pairs.set(connId, linkPair(connId));
    return 'v=0\r\ns=answer';
  }

  link(connId: string): PeerLink {
    const pair = this.pairs.get(connId);
    if (!pair) throw new Error('unknown peer connection');
    return pair.host;
  }

  shutdown(): void {
    this.shutdowns++;
    for (const { host } of this.pairs.values()) host.close('host_shutdown');
  }

  dispose(): void {
    this.disposed = true;
    this.shutdown();
  }
}

// ── Keys ─────────────────────────────────────────────────────────────────────

export async function newHostIdentity(): Promise<HostIdentity> {
  const key = await generateIdentityKey({ extractable: false });
  return { hostId: await idFromKey(key.publicKey), hostKey: b64uEncode(key.publicKey), privateKey: key.privateKey };
}

let sharedIdentity: Promise<HostIdentity> | undefined;

/** One host identity for the whole test file; generating keys is the slow part. */
export function testHostIdentity(): Promise<HostIdentity> {
  return sharedIdentity ??= newHostIdentity();
}

// ── The simulated viewer ─────────────────────────────────────────────────────

export interface ViewerEnv {
  rtdb: Rtdb;
  clock: ManualClock;
  uid: string;
  hostId: string;
  hostKey: string;
}

export interface ConnectOptions {
  transport?: 'tunnel' | 'webrtc';
  connId?: string;
  /** The offer SDP of a `webrtc` offer. */
  sdp?: string;
  /** Override the server-stamped `created_at` (the signature does not cover it). */
  createdAt?: number;
  /** Change the signed offer before it is written. */
  tamper?: (offer: Record<string, unknown>) => void;
  /** Write this instead of a signed offer. */
  raw?: unknown;
  /** The viewer's end of a pretend DataChannel, used when the host accepts `webrtc`. */
  link?: FragmentLink;
  /** `v` of the offer; anything but 1 makes it unverifiable. */
  version?: number;
}

export interface InitializeOptions {
  protocol?: number;
  client?: number;
  push?: unknown;
  appPlatform?: string;
  mediaCapabilities?: unknown;
  /** Replace the whole params object. */
  params?: unknown;
}

export class SimViewer {
  readonly notifications: Array<{ method: string; params: unknown }> = [];
  deviceId = '';
  connId = '';
  nonce = '';
  answer: AnswerRecord | undefined;
  rpc: RpcPeer | undefined;
  link: FragmentLink | undefined;
  closedWith: string | undefined;
  private offerRecord: Record<string, unknown> | undefined;

  private constructor(
    private readonly env: ViewerEnv,
    private readonly key: IdentityKey,
    readonly name: string,
    readonly platform: DevicePlatform,
  ) {}

  static async create(env: ViewerEnv, options: { name?: string; platform?: DevicePlatform; key?: IdentityKey } = {}): Promise<SimViewer> {
    const key = options.key ?? await generateIdentityKey({ extractable: false });
    const viewer = new SimViewer(env, key, options.name ?? 'Test Phone', options.platform ?? 'ios');
    viewer.deviceId = await idFromKey(key.publicKey);
    return viewer;
  }

  get deviceKey(): string {
    return b64uEncode(this.key.publicKey);
  }

  /** The offer as written, once {@link connect} has written it. */
  get offer(): Record<string, unknown> {
    if (!this.offerRecord) throw new Error('no offer written yet');
    return this.offerRecord;
  }

  /** Write an offer and wait for the host's answer; for an accepted answer, bring the transport up as well. */
  async connect(options: ConnectOptions = {}): Promise<AnswerRecord> {
    const { env } = this;
    const connId = options.connId ?? randomId();
    this.connId = connId;
    const ephemeral = await generateEphemeralKey();
    this.nonce = b64uEncode(randomBytes(16));
    const transport = options.transport ?? 'tunnel';
    const draft = {
      v: options.version ?? 1,
      transport,
      ...(transport === 'webrtc' ? { sdp: options.sdp ?? 'v=0\r\ns=offer' } : {}),
      device: { id: this.deviceId, key: this.deviceKey, name: this.name, platform: this.platform, app_version: '1.0.0' },
      eph: b64uEncode(ephemeral.publicKey),
      nonce: this.nonce,
    };
    const context = { uid: env.uid, hostId: env.hostId, connId };
    const signed = await signOffer(context, this.key.privateKey, draft as OfferDraft);
    const record: Record<string, unknown> = { ...signed };
    if (options.createdAt !== undefined) record['created_at'] = options.createdAt;
    options.tamper?.(record);
    this.offerRecord = record;
    await env.rtdb.put(offerPath(env.uid, env.hostId, connId), options.raw !== undefined ? options.raw : record);

    const answerAt = answerPath(env.uid, env.hostId, connId);
    await drive(env.clock, () => (env.rtdb as MemoryRtdb).read(answerAt) !== null, 'the host to answer');
    const verdict = await verifyAnswer({
      ...context,
      record: (env.rtdb as MemoryRtdb).read(answerAt),
      offerNonce: this.nonce,
      expectedHostKey: env.hostKey,
    });
    if (!verdict.ok) throw new Error(`the answer did not verify: ${verdict.reason}`);
    const answer = verdict.value;
    this.answer = answer;
    if (answer.status !== 'accepted') return answer;

    let link: FragmentLink;
    if (answer.transport === 'tunnel') {
      const shared = await x25519(ephemeral.privateKey, b64uDecode(answer.eph!));
      const keys = await deriveTunnelKeys(shared, connId);
      link = new TunnelLink({
        rtdb: env.rtdb,
        connId,
        sendPath: upPath(env.uid, env.hostId, connId),
        sendDir: 'up',
        sendKey: keys.up,
        recvPath: downPath(env.uid, env.hostId, connId),
        recvDir: 'down',
        recvKey: keys.down,
        subscribe: true,
        clock: env.clock,
      });
    } else {
      if (!options.link) throw new Error('the host accepted WebRTC but the test gave no link');
      link = options.link;
    }
    this.link = link;
    link.onClose((reason) => { this.closedWith = reason; });
    const rpc = new RpcPeer(new MessageTransport(link), { clock: env.clock });
    rpc.onNotification((method, params) => { this.notifications.push({ method, params }); });
    this.rpc = rpc;
    return answer;
  }

  /** A request, with time driven until the response arrives. */
  async call<T = unknown>(method: string, params?: unknown): Promise<T> {
    if (!this.rpc) throw new Error('not connected');
    return driven(this.env.clock, this.rpc.request<T>(method, params), `the response to ${method}`);
  }

  /** The error code a request fails with. */
  async callError(method: string, params?: unknown): Promise<RemoteErrorCode> {
    if (!this.rpc) throw new Error('not connected');
    return rpcErrorCode(this.env.clock, this.rpc.request(method, params));
  }

  initializeParams(options: InitializeOptions = {}): unknown {
    if (options.params !== undefined) return options.params;
    return {
      protocol: options.protocol ?? REMOTE_PROTOCOL_MAJOR,
      client_protocol_version: options.client ?? CLIENT_PROTOCOL_MAJOR,
      app: {
        version: '1.0.0',
        platform: options.appPlatform ?? this.platform,
        ...(options.mediaCapabilities === undefined ? {} : { media_capabilities: options.mediaCapabilities }),
      },
      ...(options.push === undefined ? {} : { push_installation_id: options.push }),
    };
  }

  initialize<T = unknown>(options: InitializeOptions = {}): Promise<T> {
    return this.call<T>('initialize', this.initializeParams(options));
  }

  /** Wait until the viewer has been sent a notification named `method`. */
  async notification(method: string): Promise<unknown> {
    await drive(this.env.clock, () => this.notifications.some((entry) => entry.method === method), `the ${method} notification`);
    return this.notifications.find((entry) => entry.method === method)!.params;
  }

  /** Wait until the link has closed; the reason it closed with. */
  async closed(): Promise<string> {
    await drive(this.env.clock, () => this.closedWith !== undefined, 'the link to close');
    return this.closedWith!;
  }

  close(): void {
    this.rpc?.close('test_done');
    this.link?.close('test_done');
  }
}

// ── The service with everything around it faked ──────────────────────────────

export interface HarnessOptions {
  uid?: string | null;
  enabled?: boolean;
  forceTunnel?: boolean;
  forceEnabled?: boolean;
  /** Wrap the database, e.g. to make some requests fail. */
  wrapRtdb?: (rtdb: MemoryRtdb, clock: ManualClock) => Rtdb;
  /** Do not call `service.start()`. */
  startService?: boolean;
  identity?: HostIdentity;
  files?: MemoryFiles;
  host?: { name: string; platform: string; app_version: string };
  createIdentity?: () => Promise<HostIdentity>;
}

export class Harness {
  readonly clock = new ManualClock(START);
  readonly memory = new MemoryRtdb({ clock: this.clock });
  readonly account: FakeAccount;
  readonly settings: FakeSettings;
  readonly files: MemoryFiles;
  readonly registry: DeviceRegistry;
  readonly approval = new FakeApproval();
  readonly gateway = new FakeGateway();
  readonly peers = new FakePeers();
  readonly log = diagnosticsLog();
  readonly policy = { allowed: true };
  readonly statuses: RemoteControlStatus[] = [];
  readonly rtdb: Rtdb;
  readonly service: RemoteControlService;
  readonly viewers: SimViewer[] = [];
  identityLoads = 0;
  /** The token provider the service gave the last database it created. */
  tokenProvider: ((forceRefresh?: boolean) => Promise<string>) | undefined;

  private constructor(readonly identity: HostIdentity, options: HarnessOptions) {
    this.account = new FakeAccount(options.uid === undefined ? UID : options.uid);
    this.settings = new FakeSettings(options.enabled ?? true);
    this.files = options.files ?? new MemoryFiles();
    this.registry = new DeviceRegistry({ path: '/virtual/devices.json', files: this.files, clock: this.clock, diagnostics: this.log.diagnostics });
    this.rtdb = options.wrapRtdb ? options.wrapRtdb(this.memory, this.clock) : this.memory;
    const host = options.host ?? { name: 'Test Mac', platform: 'darwin', app_version: '9.9.9' };
    this.service = new RemoteControlService({
      account: this.account,
      settings: this.settings,
      policyAllows: () => this.policy.allowed,
      gateway: this.gateway,
      config: { forceTunnel: options.forceTunnel ?? false, forceEnabled: options.forceEnabled ?? false, stunUrls: ['stun:stun.test:3478'] },
      identity: async () => {
        this.identityLoads++;
        return options.createIdentity ? options.createIdentity() : identity;
      },
      createRtdb: (getIdToken) => { this.tokenProvider = getIdToken; return this.rtdb; },
      registry: this.registry,
      approval: this.approval,
      peers: this.peers,
      host: () => host,
      clock: this.clock,
      diagnostics: this.log.diagnostics,
    });
    this.service.onStatus((status) => this.statuses.push(status));
  }

  static async create(options: HarnessOptions = {}): Promise<Harness> {
    const harness = new Harness(options.identity ?? await testHostIdentity(), options);
    if (options.startService !== false) await harness.service.start();
    return harness;
  }

  get uid(): string {
    return this.account.user?.uid ?? UID;
  }

  get env(): ViewerEnv {
    return { rtdb: this.rtdb, clock: this.clock, uid: this.uid, hostId: this.identity.hostId, hostKey: this.identity.hostKey };
  }

  /** The host record as the database holds it. */
  hostRecord(): Record<string, unknown> | null {
    return this.memory.read(hostPath(this.uid, this.identity.hostId)) as Record<string, unknown> | null;
  }

  /** Wait until the host has published itself as online. */
  async online(): Promise<void> {
    await until(() => this.hostRecord()?.['online'] === true && this.service.status().state === 'online', 'the host to be online');
  }

  async viewer(options: { name?: string; platform?: DevicePlatform; key?: IdentityKey } = {}): Promise<SimViewer> {
    const viewer = await SimViewer.create(this.env, options);
    this.viewers.push(viewer);
    return viewer;
  }

  /** Pre-approve a viewer's device, as an earlier pairing would have. */
  approveDevice(viewer: SimViewer): void {
    this.registry.approve(this.uid, { device_id: viewer.deviceId, key: viewer.deviceKey, name: viewer.name, platform: viewer.platform });
  }

  async dispose(): Promise<void> {
    for (const viewer of this.viewers) viewer.close();
    await this.service.dispose();
  }
}
