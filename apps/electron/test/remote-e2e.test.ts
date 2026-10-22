/**
 * Remote Control v1, end to end: the REAL web viewer (`apps/web/src/remote`)
 * against the REAL desktop host (`RemoteControlService`, `HostConnection`,
 * `HostSignaling`, `ApprovalDialog`, `DeviceRegistry`, `SessionGateway`,
 * `RemoteCommands`, `SessionRuntimeManager` and real `SessionRuntime`s), joined
 * only by the encrypted RTDB tunnel.
 *
 * Why it lives here. The viewer's connection code imports nothing but relative
 * files and `@lingxi/bridge-client/remote`, which Electron resolves through its
 * own dependency (the built `dist/`), so the viewer runs from this package
 * without any new dependency. `remoteClient.ts` reaches the shared renderer
 * reducer through `@lingxi/electron/*` (an alias of the web build) and
 * `@lingxi/bridge-client{,/toolview}` (which the web build maps to source files);
 * a Node resolve hook, registered below for the duration of the imports,
 * answers the same specifiers the way the web build does. Nothing is added to
 * a tsconfig or package.json, and an apps/web test would have had to import
 * the Electron main process instead, whose dependencies the web CI job does not
 * install.
 *
 * What is fake: the engine (a `FakeClient` behind each real `SessionRuntime`),
 * the native approval dialog (`show` is scripted), the filesystem behind the
 * device registry, and WebRTC (the host is forced onto the tunnel; the viewer
 * has no PeerConnection or a failing one). Everything else is production code.
 *
 * Two backends: `MemoryRtdb` driven by a `ManualClock` (always), and a real
 * `RtdbClient` against the RTDB emulator with `infra/firebase/database.rules.json`
 * installed (only when FIREBASE_DATABASE_EMULATOR_HOST is set), e.g.
 *
 *   firebase emulators:exec --only database --project demo-lingxi-backend \
 *     "node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs --test test/remote-e2e.test.ts"
 */

import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import type { MessageDto } from '@lingxi/bridge-client';
import {
  ManualClock,
  MemoryRtdb,
  RemoteRpcError,
  RtdbClient,
  RtdbError,
  b64uEncode,
  hostPath,
  randomId,
  systemClock,
  toHostPath,
  toViewerPath,
  utf8Encode,
  type Clock,
  type Rtdb,
  type RtdbEvent,
  type RtdbListenError,
  type RtdbSubscription,
} from '@lingxi/bridge-client/remote';

import { SessionRuntime } from '../src/main/bridge.js';
import type { SessionRef } from '../src/main/bridgeTypes.js';
import { ApprovalDialog, type ApprovalMessageBox } from '../src/main/remoteControl/approvalDialog.js';
import { DeviceRegistry } from '../src/main/remoteControl/deviceRegistry.js';
import { RemotePolicy } from '../src/main/remoteControl/policy.js';
import { RemoteControlService } from '../src/main/remoteControl/RemoteControlService.js';
import { RemoteCommands, type RemotePromptEcho } from '../src/main/remoteControl/remoteCommands.js';
import { SessionGateway } from '../src/main/remoteControl/sessionGateway.js';
import { LIVE_BATCH_MS } from '../src/main/remoteControl/sessionAttachment.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { FakeAccount, FakePeers, FakeSettings as FakeServiceSettings, MemoryFiles, START, diagnosticsLog, drive, elapse, testHostIdentity } from './remote-helpers.js';
import { FakeSettings as FakePolicySettings, SESSION_A, permissionRequest } from './remote-session-helpers.js';

// ─────────────────────────────────────────────────────────────────────────────
// Loading the viewer: the web build's aliases, answered for plain Node
// ─────────────────────────────────────────────────────────────────────────────

const electronSrc = fileURLToPath(new URL('../src/', import.meta.url));
const aliasHooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith('@lingxi/electron/')) {
      return next(pathToFileURL(path.join(electronSrc, specifier.slice('@lingxi/electron/'.length))).href, context);
    }
    // Files under apps/web have no node_modules of their own; this package's dependency is the same `dist/` the host uses.
    if (specifier === '@lingxi/bridge-client' || specifier.startsWith('@lingxi/bridge-client/')) {
      return next(specifier, { ...context, parentURL: import.meta.url });
    }
    return next(specifier, context);
  },
});
const { ViewerConnection } = await import('../../web/src/remote/viewerConnection');
const { RemoteClient } = await import('../../web/src/remote/remoteClient');
const { DeviceKeys, MemoryDeviceStore } = await import('../../web/src/remote/deviceKey');
aliasHooks.deregister();

type ViewerConnectionT = InstanceType<typeof ViewerConnection>;
type RemoteClientT = InstanceType<typeof RemoteClient>;
type DeviceKeysT = InstanceType<typeof DeviceKeys>;

// ─────────────────────────────────────────────────────────────────────────────
// The engine behind a real SessionRuntime
// ─────────────────────────────────────────────────────────────────────────────

/** What the bridge client of one runtime does: record what the runtime sends, and let the test play the engine. */
class FakeClient extends EventEmitter {
  readonly prompts: Array<{ text: string; opts: any }> = [];
  readonly approved: unknown[] = [];
  readonly denied: number[] = [];
  sendCommand(): void {}
  sendPrompt(text: string, opts: unknown): void { this.prompts.push({ text, opts }); }
  approvePermission(id: number, response: unknown): void { this.approved.push([id, response]); }
  denyPermission(id: number): void { this.denied.push(id); }
  async requestPermissionScope(): Promise<{ background_owned: boolean }> { return { background_owned: false }; }
  emitEvent(event: unknown): void { this.emit('event', event); }
}

const PROJECT = '/workspace';
const engine = { clients: new Map<string, FakeClient>(), history: new Map<string, MessageDto[]>() };
const originalStart = SessionRuntime.prototype.start;
const originalResume = SessionRuntime.prototype.resumeOwnedSession;

before(() => {
  // Starting a runtime would spawn the engine; here it connects a FakeClient the way `bridge.test.ts` does by hand.
  SessionRuntime.prototype.start = async function (this: SessionRuntime) {
    const self = this as any;
    await self.opts.launchConfig();
    if (self.client) return;
    const client = new FakeClient();
    engine.clients.set(self.sessionId, client);
    self.generation = 1;
    self.client = client;
    self.state = { status: 'connected' };
    self.activeWorkspace = PROJECT;
    self.activeWorkspaceTrusted = true;
    self.wireClient(client, 1);
  };
  // Resuming replays the history as the engine's `session_resumed`.
  SessionRuntime.prototype.resumeOwnedSession = async function (this: SessionRuntime) {
    engine.clients.get(this.sessionId)!.emitEvent({ type: 'session_resumed', session_id: this.sessionId, mode: 'default', messages: engine.history.get(this.sessionId) ?? [] });
  };
});

after(() => {
  SessionRuntime.prototype.start = originalStart;
  SessionRuntime.prototype.resumeOwnedSession = originalResume;
});

const HISTORY: MessageDto[] = [
  { role: 'user', blocks: [{ type: 'text', text: 'earlier question' }] },
  { role: 'assistant', blocks: [{ type: 'text', text: 'earlier answer' }] },
] as MessageDto[];

// ─────────────────────────────────────────────────────────────────────────────
// The network between the two sides
// ─────────────────────────────────────────────────────────────────────────────

interface Written { side: 'host' | 'viewer'; op: 'put' | 'patch'; path: string; value: unknown }

/** A switch that cuts one side off from the database: requests fail, streams go silent and resume with a full snapshot. */
class Network {
  down = false;
  private readonly taps = new Set<TapRtdb>();
  add(tap: TapRtdb): void { this.taps.add(tap); }
  set(down: boolean): void {
    this.down = down;
    for (const tap of this.taps) tap.apply();
  }
}

interface Sub { path: string; onEvent: (event: RtdbEvent) => void; onError?: (error: RtdbListenError) => void; inner?: RtdbSubscription; closed: boolean }

/** Wraps a database for one side: records what it writes (to check for plaintext) and can be partitioned. */
class TapRtdb implements Rtdb {
  private readonly subs = new Set<Sub>();

  constructor(private readonly inner: Rtdb, private readonly side: 'host' | 'viewer', private readonly network: Network, private readonly log: Written[]) {
    network.add(this);
  }

  private check(): void {
    if (this.network.down) throw new RtdbError(0, 'network is down');
  }

  async get(path: string): Promise<unknown> { this.check(); return this.inner.get(path); }

  async put(path: string, value: unknown): Promise<void> {
    this.check();
    await this.inner.put(path, value);
    this.log.push({ side: this.side, op: 'put', path, value: structuredClone(value) });
  }

  async patch(path: string, value: Record<string, unknown>): Promise<void> {
    this.check();
    await this.inner.patch(path, value);
    this.log.push({ side: this.side, op: 'patch', path, value: structuredClone(value) });
  }

  async remove(path: string): Promise<void> { this.check(); return this.inner.remove(path); }

  listen(path: string, onEvent: (event: RtdbEvent) => void, onError?: (error: RtdbListenError) => void): RtdbSubscription {
    const sub: Sub = { path, onEvent, ...(onError ? { onError } : {}), closed: false };
    this.subs.add(sub);
    this.open(sub);
    return { close: () => { sub.closed = true; sub.inner?.close(); sub.inner = undefined; this.subs.delete(sub); } };
  }

  private open(sub: Sub): void {
    if (this.network.down || sub.closed || sub.inner) return;
    sub.inner = this.inner.listen(sub.path, (event) => { if (!this.network.down && !sub.closed) sub.onEvent(event); }, sub.onError);
  }

  apply(): void {
    for (const sub of this.subs) {
      if (this.network.down) { sub.inner?.close(); sub.inner = undefined; } else this.open(sub); // a reconnect repeats the full snapshot
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Backends
// ─────────────────────────────────────────────────────────────────────────────

interface Backend {
  readonly kind: 'memory' | 'emulator';
  readonly clock: Clock;
  readonly uid: string;
  readonly written: Written[];
  readonly network: Network;
  readonly hostDb: Rtdb;
  readonly viewerDb: Rtdb;
  /** An untapped handle for the test's own reads. */
  readonly probe: Rtdb;
  until(condition: () => boolean | Promise<boolean>, label: string): Promise<void>;
  /** Await a promise whose completion needs time to pass (fake time for the memory backend). */
  run<T>(promise: Promise<T>): Promise<T>;
  /** Let `ms` pass (fake time for the memory backend). */
  pass(ms: number): Promise<void>;
  close(): Promise<void>;
}

function memoryBackend(): Backend {
  const clock = new ManualClock(START);
  const memory = new MemoryRtdb({ clock });
  const written: Written[] = [];
  const network = new Network();
  return {
    kind: 'memory',
    clock,
    uid: 'uid-e2e',
    written,
    network,
    hostDb: new TapRtdb(memory, 'host', new Network(), written),
    viewerDb: new TapRtdb(memory, 'viewer', network, written),
    probe: memory,
    until: async (condition, label) => {
      for (let turn = 0; turn < 40_000; turn++) {
        if (await condition()) return;
        await new Promise<void>((resolve) => setImmediate(resolve));
        clock.advance(25);
      }
      throw new Error(`timed out waiting for ${label}`);
    },
    run: async (promise) => {
      let outcome: { ok: true; value: any } | { ok: false; error: unknown } | undefined;
      promise.then((value) => { outcome = { ok: true, value }; }, (error: unknown) => { outcome = { ok: false, error }; });
      await drive(clock, () => outcome !== undefined, 'a promise to settle', 40_000);
      if (!outcome!.ok) throw outcome!.error;
      return outcome!.value;
    },
    pass: (ms) => elapse(clock, ms, 250),
    close: async () => undefined,
  };
}

const emulatorHost = process.env['FIREBASE_DATABASE_EMULATOR_HOST'];
const emulatorSkip = emulatorHost === undefined || emulatorHost === '' ? 'set FIREBASE_DATABASE_EMULATOR_HOST (host:port) to run against the RTDB emulator' : false;

/** An unsigned ID token for `uid`: what the emulator accepts in place of a signed one. */
function unsignedIdToken(uid: string): string {
  const part = (value: unknown): string => b64uEncode(utf8Encode(JSON.stringify(value)));
  const now = Math.floor(Date.now() / 1000);
  const claims = { iss: 'https://securetoken.google.com/demo-lingxi-backend', aud: 'demo-lingxi-backend', auth_time: now, user_id: uid, sub: uid, iat: now, exp: now + 3600, firebase: { identities: {}, sign_in_provider: 'password' } };
  return `${part({ alg: 'none', typ: 'JWT' })}.${part(claims)}.`;
}

async function emulatorBackend(): Promise<Backend> {
  const origin = `http://${emulatorHost}`;
  // A namespace of its own, with the repository's rules: the test fails if those rules refuse what the stack writes.
  const namespace = `demo-lingxi-backend-e2e-${randomUUID()}`;
  const rules = readFileSync(new URL('../../../infra/firebase/database.rules.json', import.meta.url), 'utf8');
  const response = await fetch(`${origin}/.settings/rules.json?ns=${encodeURIComponent(namespace)}`, { method: 'PUT', headers: { Authorization: 'Bearer owner' }, body: rules });
  assert.equal(response.status, 200, `installing the rules: ${await response.text()}`);
  const uid = `e2e-${randomId()}`;
  const client = (): RtdbClient => new RtdbClient({ databaseUrl: origin, namespace, getIdToken: async () => unsignedIdToken(uid) });
  const written: Written[] = [];
  const network = new Network();
  return {
    kind: 'emulator',
    clock: systemClock,
    uid,
    written,
    network,
    hostDb: new TapRtdb(client(), 'host', new Network(), written),
    viewerDb: new TapRtdb(client(), 'viewer', network, written),
    probe: client(),
    until: async (condition, label) => {
      const deadline = Date.now() + 20_000;
      while (!(await condition())) {
        if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
        await new Promise<void>((resolve) => setTimeout(resolve, 10));
      }
    },
    run: (promise) => promise,
    pass: (ms) => new Promise<void>((resolve) => setTimeout(resolve, ms)),
    close: async () => undefined,
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// The desktop
// ─────────────────────────────────────────────────────────────────────────────

/** The native dialog, scripted: it shows what the user would read and waits for the test to press a button. */
class Dialog {
  readonly shown: ApprovalMessageBox[] = [];
  private readonly waiting: Array<(result: { response: number }) => void> = [];

  show = (box: ApprovalMessageBox): Promise<{ response: number }> => new Promise((resolve) => {
    this.shown.push(box);
    this.waiting.push(resolve);
    box.signal.addEventListener('abort', () => resolve({ response: 1 }), { once: true });
  });

  /** The six digits the dialog tells the user to compare. */
  code(index = this.shown.length - 1): string {
    const match = /配对码：(\d{3}) (\d{3})/u.exec(this.shown[index]!.detail);
    assert.ok(match, 'the dialog shows a pairing code');
    return `${match[1]}${match[2]}`;
  }

  answer(allow: boolean): void {
    const resolve = this.waiting.shift();
    assert.ok(resolve, 'a dialog is waiting');
    resolve({ response: allow ? 0 : 1 });
  }
}

interface Desktop {
  service: RemoteControlService;
  registry: DeviceRegistry;
  dialog: Dialog;
  manager: SessionRuntimeManager;
  gateway: SessionGateway;
  policy: RemotePolicy;
  echoes: RemotePromptEcho[];
  log: ReturnType<typeof diagnosticsLog>;
  hostId: string;
  hostKey: string;
  ref: SessionRef;
  /** The runtime of the shared session, once something has opened it. */
  runtime(): SessionRuntime;
  client(): FakeClient;
  dispose(): Promise<void>;
}

async function startDesktop(backend: Backend, options: { shareSession?: boolean } = {}): Promise<Desktop> {
  const identity = await testHostIdentity();
  const { clock } = backend;
  const log = diagnosticsLog();
  const files = new MemoryFiles();
  const registry = new DeviceRegistry({ path: '/virtual/devices.json', files, clock, diagnostics: log.diagnostics });
  const dialog = new Dialog();
  const approval = new ApprovalDialog({ show: dialog.show, clock });

  engine.clients.clear();
  engine.history.clear();
  engine.history.set(SESSION_A, HISTORY);
  const manager = new SessionRuntimeManager({
    launchConfig: (candidate: { sessionId?: string } | undefined) => ({ workspace: PROJECT, sessionId: candidate?.sessionId, trusted: true }),
  } as never);
  const settings = new FakePolicySettings();
  const policy = new RemotePolicy({ settings });
  const echoes: RemotePromptEcho[] = [];
  let turn = 4000;
  const commands = new RemoteCommands({ allowAlwaysAllow: () => false, echo: (echo) => echoes.push(echo), clock, randomTurnId: () => ++turn });
  const gateway = new SessionGateway({
    runtimes: manager as never,
    policy,
    commands,
    titleOf: async () => 'Fix the build',
    modelOf: () => 'model-one',
    clock,
    diagnostics: (_level, message) => log.diagnostics('info', message),
  });
  const ref: SessionRef = { projectPath: PROJECT, sessionId: SESSION_A };
  if (options.shareSession !== false) policy.setSessionEnabled(ref, true);

  const account = new class extends FakeAccount {
    override async getIdToken(): Promise<string> { return backend.kind === 'emulator' ? unsignedIdToken(backend.uid) : 'token'; }
  }(backend.uid);
  const service = new RemoteControlService({
    account,
    settings: new FakeServiceSettings(true),
    policyAllows: () => policy.allows(),
    gateway,
    // The host never builds a PeerConnection: this is the tunnel path.
    config: { forceTunnel: true, forceEnabled: false, stunUrls: ['stun:stun.test:3478'] },
    identity: async () => identity,
    createRtdb: () => backend.hostDb,
    registry,
    approval,
    peers: new FakePeers(),
    host: () => ({ name: 'E2E Mac', platform: 'darwin', app_version: '1.2.3' }),
    clock,
    diagnostics: log.diagnostics,
  });
  await service.start();
  await backend.until(() => service.status().state === 'online', 'the host to be online');
  return {
    service, registry, dialog, manager, gateway, policy, echoes, log, ref,
    hostId: identity.hostId,
    hostKey: identity.hostKey,
    runtime: () => manager.get(SESSION_A)!,
    client: () => engine.clients.get(SESSION_A)!,
    dispose: async () => {
      gateway.dispose();
      await backend.run(service.dispose());
      await manager.dispose();
    },
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// The browser
// ─────────────────────────────────────────────────────────────────────────────

interface Browser {
  keys: DeviceKeysT;
  connection: ViewerConnectionT;
  client: RemoteClientT;
  /** Every notification the desktop sent, whichever connection carried it. */
  received: Array<{ method: string; params: any }>;
  phases: string[];
  stop(): void;
}

function openBrowser(backend: Backend, desktop: Desktop, options: { keys?: DeviceKeysT; peerConnections?: unknown } = {}): Browser {
  const keys = options.keys ?? new DeviceKeys({ store: new MemoryDeviceStore() });
  const connection = new ViewerConnection({
    uid: backend.uid,
    hostId: desktop.hostId,
    rtdb: backend.viewerDb,
    keys,
    clock: backend.clock,
    random: () => 0,
    ...(options.peerConnections === undefined ? {} : { peerConnections: options.peerConnections as never }),
    deviceName: 'E2E Browser',
    appVersion: 'e2e',
  });
  const client = new RemoteClient(connection, { clock: backend.clock });
  const received: Browser['received'] = [];
  const phases: string[] = [];
  connection.onNotification((method, params) => received.push({ method, params }));
  connection.subscribe((state) => { if (phases.at(-1) !== state.phase) phases.push(state.phase); });
  return { keys, connection, client, received, phases, stop: () => { client.dispose(); connection.stop(); } };
}

const phaseOf = (browser: Browser): string => browser.connection.state.phase;

async function pair(backend: Backend, desktop: Desktop, browser: Browser): Promise<void> {
  const dialogs = desktop.dialog.shown.length;
  browser.connection.start();
  await backend.until(() => phaseOf(browser) === 'awaiting_approval', 'the pairing code');
  await backend.until(() => desktop.dialog.shown.length === dialogs + 1, 'the desktop dialog');
  assert.equal(desktop.dialog.code(), browser.connection.state.pairingCode);
  desktop.dialog.answer(true);
  await backend.until(() => phaseOf(browser) === 'ready', 'the browser to be approved');
}

async function leftovers(backend: Backend, desktop: Desktop): Promise<unknown[]> {
  return [
    await backend.probe.get(toHostPath(backend.uid, desktop.hostId)),
    await backend.probe.get(toViewerPath(backend.uid, desktop.hostId)),
  ].filter((value) => value !== null && value !== undefined);
}

/** Every value that went into the database on a frame path must be a base64url string: no JSON, no method names. */
function assertFramesAreCiphertext(backend: Backend): void {
  const frames: string[] = [];
  const collect = (path: string, value: unknown): void => {
    if (/\/(up|down)(\/|$)/u.test(path) && typeof value === 'object' && value !== null) {
      for (const [key, child] of Object.entries(value)) collect(`${path}/${key}`, child);
      return;
    }
    if (/\/(up|down)\/f\d+$/u.test(path) && typeof value === 'string') frames.push(value);
  };
  for (const entry of backend.written) {
    if (entry.op === 'patch' && /\/(up|down)$/u.test(entry.path)) {
      // Deleting processed frames: all nulls.
      for (const child of Object.values(entry.value as Record<string, unknown>)) assert.equal(child, null);
      continue;
    }
    collect(entry.path, entry.value);
  }
  assert.ok(frames.length >= 10, `expected a conversation's worth of frames, saw ${frames.length}`);
  for (const frame of frames) {
    assert.match(frame, /^[A-Za-z0-9_-]+$/u, 'a frame is base64url text');
    const raw = Buffer.from(frame, 'base64url').toString('latin1');
    for (const needle of ['jsonrpc', '"method"', 'session.', 'sessions.', 'initialize', 'pairing_code', 'hello from', 'earlier answer', 'E2E Mac']) {
      assert.ok(!frame.includes(needle) && !raw.includes(needle), `plaintext ${needle} in a stored frame`);
    }
  }
}

function transcriptText(browser: Browser, sessionId = SESSION_A): string[] {
  return browser.client.session(sessionId).getState().transcript.conversation.items
    .flatMap((item: any) => (item.type === 'narration' || item.type === 'thinking' ? [item.text as string] : []));
}

/** The engine speaks one turn on the fake client of the session. */
function engineTurn(client: FakeClient, turnId: number, parts: string[]): void {
  client.emitEvent({ type: 'turn_started', turn_id: turnId });
  for (const text of parts) client.emitEvent({ type: 'text_delta', text });
  client.emitEvent({ type: 'message_complete', stop_reason: 'end_turn', message: { role: 'assistant', blocks: [{ type: 'text', text: parts.join('') }] } });
  client.emitEvent({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: { total_usd: 0, input_tokens: 1, output_tokens: 1, api_calls: 1, session_duration_secs: 1, formatted: '$0' } });
}

/** The records a connection received for a session, in arrival order. */
function sessionEvents(browser: Browser): Array<{ seq: number; record: any }> {
  return browser.received.filter((item) => item.method === 'session.event').map((item) => ({ seq: item.params.seq, record: item.params.record }));
}

const eventType = (record: any): string => (record.type === 'event' ? record.event.type : record.type);

/** Attach the session the way the page does and wait for the live state. */
async function attachSession(backend: Backend, browser: Browser): Promise<ReturnType<RemoteClientT['session']>> {
  const controller = browser.client.session(SESSION_A);
  controller.activate();
  await backend.until(() => controller.getState().status === 'live', 'the session to be live');
  return controller;
}

// ─────────────────────────────────────────────────────────────────────────────
// Scenario bodies (shared by both backends)
// ─────────────────────────────────────────────────────────────────────────────

async function firstPairing(backend: Backend): Promise<void> {
  const desktop = await startDesktop(backend);
  const browser = openBrowser(backend, desktop);
  try {
    browser.connection.start();
    await backend.until(() => phaseOf(browser) === 'awaiting_approval', 'the viewer to verify the answer and wait');
    assert.equal(browser.connection.state.transport, 'tunnel');
    await backend.until(() => desktop.dialog.shown.length === 1, 'the desktop dialog');

    // Both sides computed the same six digits, from the two signed records.
    const viewerCode = browser.connection.state.pairingCode!;
    assert.match(viewerCode, /^\d{6}$/);
    assert.equal(desktop.dialog.code(), viewerCode, 'the dialog and the browser show the same code');
    assert.match(desktop.dialog.shown[0]!.message, /E2E Browser/);
    // Nothing is trusted yet, on either side.
    assert.equal(await browser.keys.pinnedHost(desktop.hostId), undefined);
    assert.deepEqual(desktop.registry.list(backend.uid), []);
    await assert.rejects(browser.connection.request('sessions.list', {}), (error: unknown) => error instanceof RemoteRpcError && error.code === 'closed');

    desktop.dialog.answer(true);
    await backend.until(() => phaseOf(browser) === 'ready', 'auth.approved');
    assert.equal(browser.connection.state.pairingCode, undefined);
    assert.equal(browser.connection.state.host?.name, 'E2E Mac');
    assert.deepEqual(browser.phases, ['connecting', 'awaiting_approval', 'ready']);

    // The browser pinned the key the desktop proved; the desktop stored the browser.
    const pinned = await browser.keys.pinnedHost(desktop.hostId);
    assert.equal(pinned?.host_key, desktop.hostKey);
    assert.equal(pinned?.name, 'E2E Mac');
    const device = (await browser.keys.identity()).id;
    assert.deepEqual(desktop.registry.list(backend.uid).map((entry) => [entry.device_id, entry.name, entry.platform]), [[device, 'E2E Browser', 'web']]);
    assert.deepEqual(desktop.service.status().connections.map((c) => [c.device_id, c.transport, c.approved]), [[device, 'tunnel', true]]);
    assert.deepEqual(await backend.run(browser.connection.request('sessions.list', {})), {
      sessions: [{ session_id: SESSION_A, title: 'Fix the build', project_label: 'workspace', status: 'dormant', viewers: 0, model: 'model-one' }],
    });

    // A second connection from the same browser is approved at once, with no dialog.
    browser.connection.stop();
    await backend.until(() => desktop.service.status().connections.length === 0, 'the first connection to end on the desktop');
    const again = openBrowser(backend, desktop, { keys: browser.keys });
    try {
      again.connection.start();
      await backend.until(() => phaseOf(again) === 'ready', 'the second connection');
      assert.deepEqual(again.phases, ['connecting', 'ready'], 'it never waited for approval');
      assert.equal(desktop.dialog.shown.length, 1, 'no second dialog');
      assert.equal(again.connection.state.transport, 'tunnel');
    } finally {
      again.stop();
    }
    await backend.until(() => desktop.service.status().connections.length === 0, 'the second connection to end');
    await backend.until(async () => (await leftovers(backend, desktop)).length === 0, 'the connection nodes to be removed');
  } finally {
    browser.stop();
    await desktop.dispose();
  }
}

/** A session that is already open on the desktop, with a finished turn in its retained events. */
async function warmSession(desktop: Desktop): Promise<void> {
  await desktop.manager.openSession(desktop.ref, false, undefined, false);
  desktop.client().emitEvent({ type: 'model_changed', model: 'model-one' });
  engineTurn(desktop.client(), 1, ['earlier ', 'streamed answer']);
}

async function sessionFlow(backend: Backend): Promise<void> {
  const desktop = await startDesktop(backend);
  await warmSession(desktop);
  const browser = openBrowser(backend, desktop);
  try {
    await pair(backend, desktop, browser);

    // The list: one shared session, already open on the desktop.
    await backend.until(() => browser.client.getState().sessions !== undefined, 'sessions.watch');
    assert.deepEqual(browser.client.getState().sessions!.map((s) => [s.session_id, s.title, s.status]), [[SESSION_A, 'Fix the build', 'idle']]);

    // Attach: the snapshot arrives as begin, messages, records, end, and covers the history and the retained turn.
    const controller = await attachSession(backend, browser);
    const phases = browser.received.filter((item) => item.method === 'session.snapshot').map((item) => item.params.phase);
    assert.deepEqual(phases, ['begin', 'messages', 'records', 'end']);
    const texts = transcriptText(browser);
    assert.deepEqual(texts.slice(0, 2), ['earlier question', 'earlier answer']);
    assert.ok(texts.some((text) => text.includes('earlier streamed answer')), 'the retained turn is in the snapshot');
    assert.equal(controller.getState().turn.running, false);
    assert.equal(desktop.gateway.attachedViewers(SESSION_A), 1);

    // The engine streams a turn: the records arrive in order, once each.
    const seen = sessionEvents(browser).length;
    engineTurn(desktop.client(), 7, ['Hel', 'lo ', 'there']);
    await backend.until(() => sessionEvents(browser).slice(seen).some((item) => eventType(item.record) === 'turn_ended'), 'the turn to finish');
    const live = sessionEvents(browser).slice(seen);
    assert.deepEqual(live.map((item) => eventType(item.record)), ['turn_started', 'text_delta', 'message_complete', 'turn_ended']);
    assert.equal(live[1]!.record.event.text, 'Hello there', 'the three deltas were sent as one');
    assert.deepEqual(live.map((item) => item.seq), [...live.map((item) => item.seq)].sort((a, b) => a - b));
    assert.equal(new Set(live.map((item) => item.seq)).size, live.length);
    assert.equal(controller.getState().turn.running, false);

    // A prompt from the browser reaches the engine as a prompt of the device, and the desktop journals and echoes it.
    await backend.run(controller.sendPrompt('hello from the browser'));
    const client = desktop.client();
    assert.deepEqual(client.prompts.map((prompt) => [prompt.text, prompt.opts.turnId]), [['hello from the browser', 4001]]);
    assert.deepEqual(desktop.runtime().remoteView().prompts.map((entry) => [entry.text, entry.origin, entry.deviceName, entry.turnId]), [['hello from the browser', 'device', 'E2E Browser', 4001]]);
    assert.deepEqual(desktop.echoes.map((echo) => [echo.sessionId, echo.text, echo.origin, echo.deviceName, echo.turnId]), [[SESSION_A, 'hello from the browser', 'device', 'E2E Browser', 4001]]);
    await backend.until(() => sessionEvents(browser).some((item) => item.record.type === 'prompt'), 'the prompt record');
    const promptRecord = sessionEvents(browser).find((item) => item.record.type === 'prompt')!.record;
    assert.deepEqual({ ...promptRecord }, { type: 'prompt', turn_id: 4001, text: 'hello from the browser', origin: 'device', device_name: 'E2E Browser' });
    assert.deepEqual(controller.getState().outgoing, [], 'the pending prompt was matched with its record');
    engineTurn(client, 4001, ['an answer']);
    await backend.until(() => transcriptText(browser).includes('an answer'), 'the answer');
    assert.equal(transcriptText(browser).filter((text) => text === 'hello from the browser').length, 1);

    assertFramesAreCiphertext(backend);

    browser.stop();
    await backend.until(() => desktop.service.status().connections.length === 0, 'the connection to end');
    await backend.until(async () => (await leftovers(backend, desktop)).length === 0, 'the connection nodes to be removed');
  } finally {
    browser.stop();
    await desktop.dispose();
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. First pairing over the tunnel
// ─────────────────────────────────────────────────────────────────────────────

test('1. first pairing over the tunnel: same code on both sides, approval, pin, and a second connection without a dialog', async () => {
  await firstPairing(memoryBackend());
});

test('1b. a browser whose WebRTC fails dials the tunnel; a direct offer to a host that forces the tunnel is downgraded', async () => {
  // (a) No PeerConnection can be made: the offer says "tunnel" from the start.
  {
    const backend = memoryBackend();
    const desktop = await startDesktop(backend);
    const browser = openBrowser(backend, desktop, { peerConnections: () => { throw new Error('no WebRTC here'); } });
    try {
      await pair(backend, desktop, browser);
      assert.equal(browser.connection.state.transport, 'tunnel');
      const offer = (backend.probe as MemoryRtdb).read(hostPath(backend.uid, desktop.hostId));
      assert.ok(offer, 'the host record is still there');
    } finally {
      browser.stop();
      await desktop.dispose();
    }
  }
  // (b) A real-looking direct offer; the host forces the tunnel (`forceTunnel`) and answers `tunnel` on the same conn_id.
  {
    const backend = memoryBackend();
    const desktop = await startDesktop(backend);
    const peers: FakePeerConnection[] = [];
    const browser = openBrowser(backend, desktop, { peerConnections: () => { const peer = new FakePeerConnection(); peers.push(peer); return peer; } });
    try {
      await pair(backend, desktop, browser);
      assert.equal(browser.connection.state.transport, 'tunnel', 'the downgrade was accepted');
      assert.equal(peers.length, 1, 'one PeerConnection, one connection id: no second offer');
      assert.ok(peers[0]!.closed, 'the abandoned PeerConnection was closed');
      assert.equal(peers[0]!.channels[0]?.label, 'lingxi-agent');
      const offers = backend.written.filter((entry) => /\/offer$/u.test(entry.path));
      assert.equal(offers.length, 1);
      assert.equal((offers[0]!.value as any).transport, 'webrtc');
      assert.deepEqual(await backend.run(browser.connection.request('sessions.list', {}).then((r) => r.sessions.length)), 1);
    } finally {
      browser.stop();
      await desktop.dispose();
    }
  }
});

/** The few parts of RTCPeerConnection the viewer calls, enough to produce an offer that is never connected. */
class FakePeerConnection {
  iceGatheringState: RTCIceGatheringState = 'new';
  connectionState: RTCPeerConnectionState = 'new';
  localDescription: { sdp: string } | null = null;
  onicegatheringstatechange: ((event: Event) => void) | null = null;
  onconnectionstatechange: ((event: Event) => void) | null = null;
  readonly channels: Array<{ label: string; closed: boolean; [key: string]: unknown }> = [];
  closed = false;

  createDataChannel(label: string): never {
    const channel = { label, closed: false, readyState: 'connecting', bufferedAmount: 0, bufferedAmountLowThreshold: 0, binaryType: 'blob', onopen: null, onclose: null, onerror: null, onmessage: null, onbufferedamountlow: null, send() {}, close() { channel.closed = true; } };
    this.channels.push(channel);
    return channel as never;
  }

  async createOffer(): Promise<RTCSessionDescriptionInit> { return { type: 'offer', sdp: 'v=0\r\na=fingerprint:sha-256 AA:BB\r\n' }; }

  async setLocalDescription(description: RTCSessionDescriptionInit): Promise<void> {
    this.localDescription = { sdp: `${description.sdp ?? ''}a=candidate:1 1 udp 1 192.0.2.1 9 typ host\r\n` };
    this.iceGatheringState = 'complete';
  }

  async setRemoteDescription(): Promise<void> { throw new Error('the host chose the tunnel: no answer SDP expected'); }

  close(): void { this.closed = true; }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Denied pairing, revoked device
// ─────────────────────────────────────────────────────────────────────────────

test('2. a denied pairing ends the connection; a revoked device is told so by a signed answer and can pair again with a new key', async () => {
  const backend = memoryBackend();
  const desktop = await startDesktop(backend);
  const denied = openBrowser(backend, desktop);
  const trusted = openBrowser(backend, desktop);
  try {
    // Denied: auth.denied, the failure is final, the link is gone on both sides, and nothing is stored.
    denied.connection.start();
    await backend.until(() => desktop.dialog.shown.length === 1 && phaseOf(denied) === 'awaiting_approval', 'the dialog and the code');
    desktop.dialog.answer(false);
    await backend.until(() => phaseOf(denied) === 'failed', 'the denial');
    assert.deepEqual({ ...denied.connection.state.failure }, { kind: 'denied', detail: 'denied', auto: false });
    await backend.until(() => desktop.service.status().connections.length === 0, 'the connection to close on the desktop');
    assert.equal(await denied.keys.pinnedHost(desktop.hostId), undefined);
    assert.deepEqual(desktop.registry.list(backend.uid), []);
    await assert.rejects(denied.connection.request('sessions.list', {}), (error: unknown) => error instanceof RemoteRpcError && error.code === 'closed');

    // Another browser pairs, then the user removes it on the desktop: the live connection gets auth.denied(revoked).
    await pair(backend, desktop, trusted);
    const deviceId = (await trusted.keys.identity()).id;
    assert.equal((await trusted.keys.pinnedHost(desktop.hostId))?.host_key, desktop.hostKey);
    await backend.run(desktop.service.revokeDevice(deviceId));
    await backend.until(() => phaseOf(trusted) === 'failed', 'the revocation');
    assert.deepEqual({ ...trusted.connection.state.failure }, { kind: 'revoked', auto: false });
    assert.equal(await trusted.keys.pinnedHost(desktop.hostId), undefined, 'the pin is cleared');
    assert.deepEqual(desktop.registry.list(backend.uid), []);

    // Dialling again with the same key: a signed `rejected/revoked` answer, and no dialog.
    const dialogs = desktop.dialog.shown.length;
    const failures = trusted.phases.length;
    trusted.connection.retry();
    await backend.until(() => trusted.phases.length > failures && phaseOf(trusted) === 'failed', 'the second refusal');
    assert.deepEqual({ ...trusted.connection.state.failure }, { kind: 'revoked', auto: false });
    assert.equal(desktop.dialog.shown.length, dialogs, 'a revoked device is never asked about again');
    const answers = backend.written.filter((entry) => /\/answer$/u.test(entry.path)).map((entry) => entry.value as any);
    assert.deepEqual(answers.at(-1) && { status: answers.at(-1).status, reason: answers.at(-1).reason }, { status: 'rejected', reason: 'revoked' });

    // "Pair again": a new key is a new device, and pairs like a first one.
    const before = trusted.phases.length;
    const repaired = trusted.connection.repair();
    await repaired;
    await backend.until(() => phaseOf(trusted) === 'awaiting_approval' && trusted.phases.length > before, 'the new pairing');
    await backend.until(() => desktop.dialog.shown.length === dialogs + 1, 'the dialog for the new key');
    assert.equal(desktop.dialog.code(), trusted.connection.state.pairingCode);
    desktop.dialog.answer(true);
    await backend.until(() => phaseOf(trusted) === 'ready', 'the new key to be approved');
    assert.notEqual((await trusted.keys.identity()).id, deviceId);
  } finally {
    denied.stop();
    trusted.stop();
    await desktop.dispose();
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// 3. The session flow
// ─────────────────────────────────────────────────────────────────────────────

test('3. session flow: list, attach, snapshot, a streamed turn, a prompt from the browser, and only ciphertext in the database', async () => {
  await sessionFlow(memoryBackend());
});

// ─────────────────────────────────────────────────────────────────────────────
// 4. First answer wins
// ─────────────────────────────────────────────────────────────────────────────

test('4. first answer wins: the browser or the desktop answers a permission request, exactly one reaches the engine', async () => {
  const backend = memoryBackend();
  const desktop = await startDesktop(backend);
  const browser = openBrowser(backend, desktop);
  try {
    await pair(backend, desktop, browser);
    const controller = await attachSession(backend, browser);
    const client = desktop.client();
    const runtime = desktop.runtime();
    client.emitEvent({ type: 'turn_started', turn_id: 30 });
    assert.equal(runtime.turnActive, true);
    for (const id of [1, 2, 3]) client.emit('permission', permissionRequest(id));
    await backend.until(() => controller.getState().pending.permissions.length === 3, 'three pending requests');
    assert.ok(sessionEvents(browser).some((item) => item.record.type === 'permission_request'));

    // The browser answers 1 first: it reaches the engine; the desktop's later answer fails.
    await backend.run(controller.respondPermission(1, { type: 'allow_once' }));
    assert.deepEqual(client.approved, [[1, { type: 'allow_once' }]]);
    assert.throws(() => runtime.approvePermission(1), /not pending/);
    assert.throws(() => runtime.denyPermission(1), /not pending/);
    await backend.until(() => controller.getState().pending.permissions.length === 2, 'the answered request to leave the pending set');

    // The desktop answers 2 first: the browser's later answer is `not_pending`, shown quietly.
    runtime.denyPermission(2);
    assert.deepEqual(client.denied, [2]);
    await backend.until(() => controller.getState().pending.permissions.length === 1, 'the desktop answer to reach the browser');
    await assert.rejects(
      backend.run(browser.connection.request('permission.respond', { session_id: SESSION_A, request_id: 2, decision: { type: 'allow_once' } })),
      (error: unknown) => error instanceof RemoteRpcError && error.code === 'not_pending',
    );
    assert.equal(controller.getState().answeredElsewhere, false);
    await backend.run(controller.respondPermission(2, { type: 'allow_once' }));
    assert.equal(controller.getState().answeredElsewhere, true);

    // Both at once on 3: the desktop's call is synchronous, the browser's crosses the tunnel, so the desktop wins; either way one answer.
    const race = await backend.run(Promise.allSettled([
      controller.respondPermission(3, { type: 'allow_once' }),
      Promise.resolve().then(() => runtime.approvePermission(3)),
    ]));
    assert.deepEqual(race.map((outcome) => outcome.status), ['fulfilled', 'fulfilled']);
    assert.equal(client.approved.filter((entry) => (entry as unknown[])[0] === 3).length + client.denied.filter((id) => id === 3).length, 1, 'exactly one answer to request 3 reached the engine');
    assert.equal(controller.getState().answeredElsewhere, true);
    assert.equal(client.approved.length, 2, 'request 1 and request 3');
    assert.equal(client.denied.length, 1);
    await backend.until(() => controller.getState().pending.permissions.length === 0, 'nothing pending');
  } finally {
    browser.stop();
    await desktop.dispose();
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// 5. Reconnect and resume
// ─────────────────────────────────────────────────────────────────────────────

test('5. a dropped tunnel: the browser reconnects on a new connection, resumes with a delta, and a new base resets it', async () => {
  const backend = memoryBackend();
  const desktop = await startDesktop(backend);
  await warmSession(desktop);
  const browser = openBrowser(backend, desktop);
  try {
    await pair(backend, desktop, browser);
    const controller = await attachSession(backend, browser);
    const client = desktop.client();
    const begins = (): any[] => browser.received.filter((item) => item.method === 'session.snapshot' && item.params.phase === 'begin').map((item) => item.params);
    assert.equal(begins().length, 1);
    const firstConn = desktop.service.status().connections.map((c) => c.conn_id);
    assert.equal(firstConn.length, 1);

    // Mid-turn: one delta arrives, the network drops, the engine keeps talking.
    client.emitEvent({ type: 'turn_started', turn_id: 50 });
    client.emitEvent({ type: 'text_delta', text: 'one ' });
    await backend.until(() => transcriptText(browser).some((text) => text.includes('one ')), 'the first delta');
    backend.network.set(true);
    client.emitEvent({ type: 'text_delta', text: 'two ' });
    await backend.pass(LIVE_BATCH_MS * 4);
    client.emitEvent({ type: 'text_delta', text: 'three ' });
    await backend.until(() => phaseOf(browser) === 'reconnecting', 'the browser to notice');
    assert.equal(controller.getState().status, 'offline');
    assert.ok(!transcriptText(browser).some((text) => text.includes('two ')), 'it has not seen what happened while it was cut off');
    client.emitEvent({ type: 'text_delta', text: 'four ' });
    await backend.pass(LIVE_BATCH_MS * 4);

    // The network returns: a new connection id, the same device, a delta and not a snapshot.
    backend.network.set(false);
    await backend.until(() => phaseOf(browser) === 'ready' && controller.getState().status === 'live', 'the browser to resume');
    client.emitEvent({ type: 'text_delta', text: 'five ' });
    engineTurn(client, 50, []);
    await backend.until(() => controller.getState().turn.running === false && transcriptText(browser).some((text) => text.includes('five ')), 'the turn to finish');

    const deltas = sessionEvents(browser).filter((item) => eventType(item.record) === 'text_delta');
    assert.equal(deltas.map((item) => item.record.event.text).join(''), 'one two three four five ', 'no record lost, none repeated');
    const seqs = sessionEvents(browser).map((item) => item.seq);
    assert.deepEqual(seqs, [...seqs].sort((a, b) => a - b), 'in order across the two connections');
    assert.equal(new Set(seqs).size, seqs.length, 'no sequence number twice');
    assert.equal(begins().length, 1, 'no second snapshot: the resume was a delta');
    assert.ok(transcriptText(browser).some((text) => text.includes('one two three four five ')), 'the transcript reads straight through');
    const secondConn = desktop.service.status().connections.map((c) => c.conn_id);
    assert.ok(secondConn.some((id) => !firstConn.includes(id)), 'a new connection id');

    // The abandoned connection is noticed by the keepalive and its nodes are removed; the session has one viewer.
    await backend.until(() => desktop.service.status().connections.length === 1, 'the stale connection to be closed by the desktop');
    assert.equal(desktop.gateway.attachedViewers(SESSION_A), 1);

    // A new base (`/clear`): the viewer is reset and sent a fresh snapshot with a new epoch.
    const resets = browser.received.filter((item) => item.method === 'session.reset').length;
    client.emitEvent({ type: 'session_started', session_id: SESSION_A, mode: 'default' });
    await backend.until(() => begins().length === 2 && controller.getState().status === 'live', 'the fresh snapshot');
    assert.equal(browser.received.filter((item) => item.method === 'session.reset').length, resets + 1);
    assert.notEqual(begins()[1].epoch, begins()[0].epoch);
    assert.deepEqual(transcriptText(browser), [], 'the old conversation is gone');
    client.emitEvent({ type: 'text_delta', text: 'fresh' });
    await backend.until(() => transcriptText(browser).includes('fresh'), 'events on the new base');

    // A new base while the browser is away: its old position is useless, so the resume is answered with a snapshot.
    backend.network.set(true);
    await backend.until(() => phaseOf(browser) === 'reconnecting', 'the second drop');
    client.emitEvent({ type: 'session_resumed', session_id: SESSION_A, mode: 'default', messages: [{ role: 'assistant', blocks: [{ type: 'text', text: 'after the restart' }] }] });
    backend.network.set(false);
    await backend.until(() => begins().length === 3 && controller.getState().status === 'live' && phaseOf(browser) === 'ready', 'the snapshot of the new base');
    assert.notEqual(begins()[2].epoch, begins()[1].epoch);
    assert.deepEqual(transcriptText(browser), ['after the restart']);

    browser.stop();
    await backend.until(() => desktop.service.status().connections.length === 0, 'every connection to end');
    await backend.until(async () => (await leftovers(backend, desktop)).length === 0, 'no connection nodes to remain');
  } finally {
    backend.network.set(false);
    browser.stop();
    await desktop.dispose();
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// 6. Secrets withheld
// ─────────────────────────────────────────────────────────────────────────────

test('6. events the disposition table withholds never reach a browser, live or in a snapshot', async () => {
  const backend = memoryBackend();
  const desktop = await startDesktop(backend);
  await warmSession(desktop);
  const browser = openBrowser(backend, desktop);
  let late: Browser | undefined;
  try {
    await pair(backend, desktop, browser);
    const controller = await attachSession(backend, browser);
    const client = desktop.client();
    client.emitEvent({ type: 'settings_snapshot', effective_json: '{"apiKey":"sk-secret-settings"}', provenance_json: '{}' });
    client.emitEvent({ type: 'provider_credential_status', operation_id: 1, configured_provider_ids: ['anthropic'], storage_encrypted: true, credential_previews: { anthropic: 'sk-secret-preview' } });
    client.emitEvent({ type: 'mcp_configuration_snapshot', snapshot_json: '{"env":{"TOKEN":"sk-secret-mcp"}}' });
    client.emitEvent({ type: 'text_delta', text: 'visible after the secrets' });
    await backend.until(() => transcriptText(browser).some((text) => text.includes('visible after the secrets')), 'the next event to arrive');
    const everything = (b: Browser): string => JSON.stringify(b.received);
    for (const needle of ['sk-secret', 'settings_snapshot', 'provider_credential_status', 'mcp_configuration_snapshot']) {
      assert.ok(!everything(browser).includes(needle), `${needle} reached the live viewer`);
    }
    assert.equal(controller.getState().status, 'live');

    // A browser that attaches afterwards is sent a snapshot built from the same retained events.
    late = openBrowser(backend, desktop, { keys: browser.keys });
    late.connection.start();
    await backend.until(() => phaseOf(late!) === 'ready', 'the second connection');
    await attachSession(backend, late);
    assert.ok(late.received.some((item) => item.method === 'session.snapshot' && item.params.phase === 'end'));
    assert.ok(transcriptText(late).some((text) => text.includes('visible after the secrets')));
    for (const needle of ['sk-secret', 'settings_snapshot', 'provider_credential_status', 'mcp_configuration_snapshot']) {
      assert.ok(!everything(late).includes(needle), `${needle} reached a snapshot`);
    }
  } finally {
    browser.stop();
    late?.stop();
    await desktop.dispose();
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// 7. The RTDB emulator, with the repository's rules
// ─────────────────────────────────────────────────────────────────────────────

test('7a. over a real RtdbClient and the repository rules: first pairing, a second connection, clean-up', { skip: emulatorSkip }, async () => {
  await firstPairing(await emulatorBackend());
});

test('7b. over a real RtdbClient and the repository rules: the session flow, ciphertext only, nothing left behind', { skip: emulatorSkip }, async () => {
  await sessionFlow(await emulatorBackend());
});

test('7c. the namespace under test really enforces the repository rules (so 7a and 7b passing means the stack satisfies them)', { skip: emulatorSkip }, async () => {
  const backend = await emulatorBackend();
  const host = 'h'.repeat(22);
  await assert.rejects(backend.probe.put(hostPath(backend.uid, host), { v: 1, name: 'No key' }), (error: unknown) => error instanceof RtdbError && (error.status === 401 || error.status === 403));
  await assert.rejects(backend.probe.put(`/remote-control/${backend.uid}/to_host/${host}/${'c'.repeat(22)}/up/not-a-frame`, 'x'), (error: unknown) => error instanceof RtdbError);
  assert.equal(await backend.probe.get(hostPath(backend.uid, host)), null);
});
