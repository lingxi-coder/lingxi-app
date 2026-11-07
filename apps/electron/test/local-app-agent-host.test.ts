import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import { build } from 'esbuild';
import { LocalAppStore } from './fixtures/local-app-store.ts';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.ts';
import { LocalAppAgentBroker } from '../src/main/local-apps/agentBroker.ts';
import { parseManifest } from '../src/main/local-apps/manifest.ts';
import type { LocalAppAgentCatalog, LocalAppAgentSettings } from '../src/shared/localAppAgent.ts';
import type { DesktopAuthoringRef } from '../src/shared/localApps.ts';
import { AuthoringSourceIntents } from '../src/main/local-apps/authoringSourceIntents.ts';

const require = createRequire(import.meta.url);
const model = { providerId: 'configured', modelId: 'vision', label: 'Vision', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true };
const builtin = { id: 'builtin:Read', kind: 'builtin' as const, name: 'Read', label: 'Read files' };
const mcp = { id: 'mcp:stable', kind: 'mcp' as const, serverConfigId: 'server-one', configRevision: 'rev1', rawToolName: 'nutrition', definitionDigest: 'digest1', label: 'Nutrition' };
const settings: LocalAppAgentSettings = { enabled: true, model, tools: [builtin, mcp], skills: ['nutrition'], background: false, maxTurns: 20, timeoutMs: 600000 };
const AGUI_FIXTURE = JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8')) as any;
function runInput(runId: string, threadId = `thread-${runId}`) { const input = structuredClone(AGUI_FIXTURE); input.runId = runId; input.threadId = threadId; return input; }
const bundledHost = build({
  entryPoints: [new URL('../src/main/local-apps/host.ts', import.meta.url).pathname],
  bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'], metafile: true,
  plugins: [{ name: 'bridge-client-test-resolution', setup(builder) {
    builder.onResolve({ filter: /^@lingxi\/bridge-client(?:\/|$)/ }, args => ({ path: fileURLToPath(import.meta.resolve(args.path)) }));
  } }],
}).then(async result => {
  if (process.env.LINGXI_TEST_ARTIFACT_DIR) await fs.writeFile(join(process.env.LINGXI_TEST_ARTIFACT_DIR, 'standalone-agent-host-inputs.json'), JSON.stringify({
    bridgeClientEntry: import.meta.resolve('@lingxi/bridge-client'), inputs: Object.keys(result.metafile!.inputs),
  }, null, 2));
  return result;
});
async function until(check: () => boolean | Promise<boolean>) { for (let i = 0; i < 1000; i++) { if (await check()) return; await new Promise(resolve => setTimeout(resolve, 1)); } throw new Error('Timed out waiting for test state'); }
let windowSequence = 100;
class TestWindow extends EventEmitter {
  static getAllWindows() { return []; }
  private destroyed = false;
  webContents = Object.assign(new EventEmitter(), { id: ++windowSequence, mainFrame: { url: '' }, send() {}, getURL: () => this.webContents.mainFrame.url, setWindowOpenHandler() {} });
  constructor(_options?: unknown) { super(); }
  async loadURL(url: string) { this.webContents.emit('did-start-navigation', {}, url, false, true); this.webContents.mainFrame.url = url; }
  isDestroyed() { return this.destroyed; }
  show() {} focus() {}
  close() { this.destroyed = true; this.emit('closed'); }
  destroy() { this.close(); }
}
async function loadHost(mockElectron: unknown): Promise<any> {
  const result = await bundledHost;
  const module = { exports: {} };
  const sandbox: Record<string, unknown> = { module, exports: module.exports, require: (id: string) => id === 'electron' ? mockElectron : require(id),
    Buffer, process, console, structuredClone, URL, Response, Request, Headers, fetch, crypto: globalThis.crypto,
    ReadableStream, WritableStream, TransformStream, TextEncoder, TextDecoder, Blob, FormData, Event, EventTarget, MessageEvent, DOMException,
    AbortController, AbortSignal, setTimeout, clearTimeout };
  sandbox.global = sandbox;
  runInNewContext(result.outputFiles[0]!.text, sandbox);
  return (module.exports as any).LocalAppHost;
}
async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-agent-host-'));
  const catalog: LocalAppAgentCatalog = { models: [model], tools: [builtin, mcp], skills: [{ id: 'nutrition', label: 'Nutrition' }] };
  const sent: unknown[] = [];
  const mainFrame = { url: '' };
  const contents = { id: 91, mainFrame, send: (...args: unknown[]) => sent.push(args), getURL: () => mainFrame.url };
  const window = { webContents: contents, isDestroyed: () => false, close() {}, destroy() {}, show() {}, focus() {} };
  const LocalAppHost = await loadHost({ BrowserWindow: TestWindow, ipcMain: { removeHandler() {} },
    dialog: { showMessageBox: async () => ({ response: 1 }) }, session: { fromPartition: () => ({ clearStorageData: async () => {}, clearCache: async () => {}, setPermissionCheckHandler() {}, setPermissionRequestHandler() {}, protocol: { handle() {} } }) } });
  const host = new LocalAppHost({ core: { onDisconnected() {} }, configHome: root, appProjectsRoot: join(root, '.apps'), currentConversationMode: () => 'code', preload: '/unused', mainWindow: () => window, edit: async () => {}, agentCatalog: async () => structuredClone(catalog) });
  host.store = new LocalAppStore(join(root, 'fixture-apps')); host.runtime.store = host.store;
  host.store.publicationIntent = async () => undefined;
  const row = await host.store.publish({ name: 'Agent test', html: '<!doctype html>' });
  row.manifest = parseManifest({ schema: 2, targets: [{ platform: 'macos', environment: 'web' }], name: row.name, entry: { type: 'static', path: 'index.html' }, agent: { tools: [builtin.id, mcp.id], skills: [{ id: 'nutrition', path: 'skills/nutrition/SKILL.md' }] } });
  row.releaseDigest = 'd'.repeat(64);
  row.agent = structuredClone(settings); row.grants = ['agent']; await host.store.save(row);
  (host.store as any).core = { request: async (input: any) => input.op === 'getRelease'
    ? { digest: row.releaseDigest, files: [{ path: 'skills/nutrition/SKILL.md', sha256: 'a'.repeat(64), bytes: 18, executable: false }] }
    : {}};
  mainFrame.url = `lingxi-app://${row.id}/`;
  host.windowRuns.set(window, 'invocation-one');
  host.windows.set(row.id, window); host.launchGrants.set(row.id, ['agent']);
  host.runtime.phase = () => 'running'; host.runtime.revision = () => 1; host.runtime.invocationId = () => 'invocation-one'; host.runtime.manifest = () => row.manifest;
  host.runtime.owner = () => ({ appId: row.id, runId: 'invocation-one', revision: 1, releaseDigest: row.releaseDigest! });
  const owner = { appId: row.id, releaseDigest: row.releaseDigest!, invocationId: 'invocation-one', webContentsId: 91 };
  const event = { sender: contents, senderFrame: mainFrame };
  t.after(async () => { if (host.changeTimer) clearTimeout(host.changeTimer); await host.runtime.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  return { root, host, row, owner, event, contents, mainFrame, catalog, sent };
}

function mockNativeAuthoring(host: any) {
  const sources = new Map<string, string>();
  const owners = new Map<string, string>();
  const grants = new Map<string, { sourceId: string; authoring: DesktopAuthoringRef; sessionMode: 'code'; ownerAppId?: string }>();
  const calls: Array<{ authoring: DesktopAuthoringRef; setCurrent: boolean; requestId: string }> = [];
  let beforeRegister: (authoring: DesktopAuthoringRef) => Promise<void> = async () => {};
  host.store.ensureAuthoringSource = async (path: string, _project: unknown, requestId: string, targetAppId?: string) => {
    if (targetAppId && owners.has(path) && owners.get(path) !== targetAppId) throw new Error('Source belongs to another app');
    const sourceId = sources.get(path) ?? randomUUID(); sources.set(path, sourceId);
    return { sourceId, sourceDirectory: path, sourcePath: path, requestId, ...(targetAppId ? { targetAppId } : {}) };
  };
  host.store.findAuthoringSource = async (path: string) => {
    const sourceId = sources.get(path);
    return sourceId ? { sourceId, sourceDirectory: path, sourcePath: path, ...(owners.has(path) ? { ownerAppId: owners.get(path) } : {}) } : null;
  };
  host.store.getAuthoringGrant = async (path: string, _project: unknown, sessionId: string) => {
    const grant = grants.get(`${path}:${sessionId}`);
    if (!grant) throw Object.assign(new Error('No owner receipt'), { code: 'notFound' });
    return structuredClone(grant);
  };
  host.store.registerAuthoring = async (sourceId: string, _requestId: string, authoring: DesktopAuthoringRef, setCurrent: boolean) => {
    calls.push({ authoring: structuredClone(authoring), setCurrent, requestId: _requestId });
    await beforeRegister(authoring);
    if (authoring.appId) owners.set(authoring.sourceDirectory, authoring.appId);
    grants.set(`${authoring.sourceDirectory}:${authoring.authoringSessionId}`, {
      sourceId, authoring: structuredClone(authoring), sessionMode: 'code', ownerAppId: authoring.appId,
    });
    if (setCurrent && authoring.appId) await host.store.bindAuthoring(authoring.appId, authoring);
    return { ...structuredClone(authoring), sessionMode: 'code' };
  };
  return { calls, grants, sources, setBeforeRegister: (callback: typeof beforeRegister) => { beforeRegister = callback; } };
}

test('admitted Code authoring keeps its exact parked session and cannot replace a newer App owner', async t => {
  const { root, host, row } = await fixture(t);
  const a = join(root, 'parked-a'), b = join(root, 'active-b');
  await fs.mkdir(a); await fs.mkdir(b);
  const sourceA = await fs.realpath(a), sourceB = await fs.realpath(b);
  const sessionA = randomUUID(), sessionB = randomUUID();
  host.options.resolveAuthoringBinding = async (_id: unknown, ref: { projectPath: string; sessionId: string }) => ({
    sourceDirectory: ref.projectPath, projectRef: { kind: 'desktopPath', path: ref.projectPath }, authoringSessionId: ref.sessionId,
  });
  const native = mockNativeAuthoring(host);
  const operation = host.beginAuthoringSession({ name: row.name, appId: row.id, setCurrent: true });
  await operation.allocate({ projectPath: sourceA, sessionId: sessionA });
  await operation.recordSource({ scope: 'current', transcriptPath: join(root, `${sessionA}.jsonl`) });
  const pending = await host.request({ action: 'pendingAuthoringSessions' });
  assert.equal(pending.length, 1);
  host.options.currentConversationMode = () => 'chat';
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceB, sessionId: sessionB }, setCurrent: true }, undefined, { kind: 'native', mode: 'code' });
  await operation.finish({ projectPath: sourceA, sessionId: sessionA });
  assert.deepEqual(native.calls.map(call => [call.authoring.authoringSessionId, call.setCurrent]), [[sessionB, true], [sessionA, false]]);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB);
  assert.equal((await host.getAuthoringSession(pending[0].requestId)).state, 'ready');
  await assert.rejects(host.request({ action: 'resumeAuthoringSession', requestId: pending[0].requestId }), /local_app_development_requires_code|Code/);
  await host.completeAuthoringSession({ projectPath: sourceA, sessionId: sessionA });
  assert.equal((await host.request({ action: 'pendingAuthoringSessions' })).length, 0);
});

test('manifest Agent declaration contains tool requirements only and never authorizes execution', () => {
  const base = { schema: 2, targets: [{ platform: 'macos', environment: 'web' }], name: 'Photo', entry: { type: 'static', path: 'index.html' } };
  assert.deepEqual(parseManifest({ ...base, agent: { tools: ['builtin:Read'] } }).agent, { tools: ['builtin:Read'], skills: [] });
  assert.deepEqual(parseManifest({ ...base, agent: {} }).capabilities, ['agent']);
  assert.throws(() => parseManifest({ ...base, agent: { tools: ['builtin:Read', 'builtin:Read'] } }), /Duplicate/);
  assert.throws(() => parseManifest({ ...base, agent: { tools: ['arbitrary-tool'] } }), /catalog/);
  assert.throws(() => parseManifest({ ...base, agent: { tools: [], background: true } }), /only accept/);
  assert.throws(() => parseManifest({ ...base, agent: { tools: [], model: 'user-model' } }), /only accept/);
});

test('publishing a revision preserves host-selected Agent settings without importing manifest grants', async t => {
  const { host, row } = await fixture(t);
  await host.store.publish({ id: row.id, html: '<!doctype html><title>Updated</title>' });
  const updated = await (host.store as LocalAppStore).get(row.id);
  assert.equal(updated.revision, 2);
  assert.deepEqual(JSON.parse(JSON.stringify(updated.agent)), settings);
  assert.equal(updated.manifest.agent, undefined);
});

test('authoring verification and publish hold the app lock against a concurrent rebind', async t => {
  const { root, host, row } = await fixture(t);
  const requestedA = join(root, 'source-a'), requestedB = join(root, 'source-b');
  await fs.mkdir(requestedA); await fs.mkdir(requestedB);
  const sourceA = await fs.realpath(requestedA), sourceB = await fs.realpath(requestedB);
  const sessionA = randomUUID(), sessionB = randomUUID();
  const bindingA = { workspaceId: randomUUID(), sourceDirectory: sourceA,
    projectRef: { kind: 'desktopPath' as const, path: sourceA }, authoringSessionId: sessionA, appId: row.id };
  const bindingB = { workspaceId: randomUUID(), sourceDirectory: sourceB,
    projectRef: { kind: 'desktopPath' as const, path: sourceB }, authoringSessionId: sessionB, appId: row.id };
  await host.store.bindAuthoring(row.id, bindingA);
  const sourceId = randomUUID();
  let verificationStarted!: () => void, releaseVerification!: () => void;
  const verifying = new Promise<void>(resolve => { verificationStarted = resolve; });
  const gate = new Promise<void>(resolve => { releaseVerification = resolve; });
  host.store.findAuthoringSource = async () => { verificationStarted(); await gate; return { sourceId, sourceDirectory: sourceA, sourcePath: sourceA }; };
  host.store.getAuthoringGrant = async () => ({ sourceId, authoring: bindingA, sessionMode: 'code', ownerAppId: row.id });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    assert.equal(session.sessionId, sessionA);
    return { sourceDirectory: sourceA, projectRef: { kind: 'desktopPath', path: sourceA }, authoringSessionId: sessionA };
  };
  const publishing = host.request({ action: 'publish', id: row.id, expectedRevision: row.revision,
    html: '<!doctype html><title>Author A</title>', source_path: sourceA,
    source_session: { projectPath: sourceA, sessionId: sessionA, requestId: randomUUID() } });
  let rebindStarted = false;
  let rebinding: Promise<void> | undefined;
  try {
    await verifying;
    rebinding = host.runtime.exclusive(row.id, async () => {
      rebindStarted = true;
      await host.store.bindAuthoring(row.id, bindingB);
    });
    await Promise.resolve();
    assert.equal(rebindStarted, false, 'the selected owner cannot change between verification and publication');
  } finally {
    releaseVerification();
  }
  const published = await publishing;
  await rebinding;
  assert.equal(published.revision, 2);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB,
    'a later explicit rebind remains current after the admitted publication');
});

test('a late authoring verification retains its receipt without replacing the newer owner', async t => {
  const { root, host, row } = await fixture(t);
  const requestedA = join(root, 'late-a'), requestedB = join(root, 'late-b');
  await fs.mkdir(requestedA); await fs.mkdir(requestedB);
  const sourceA = await fs.realpath(requestedA), sourceB = await fs.realpath(requestedB);
  const sessionA = randomUUID(), sessionB = randomUUID();
  const native = mockNativeAuthoring(host);
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === sessionA) { started(); await gate; }
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  const a = host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceA, sessionId: sessionA }, setCurrent: true });
  try {
    await waiting;
    await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceB, sessionId: sessionB }, setCurrent: true });
  } finally { release(); }
  await a;
  assert.deepEqual(native.calls.map(call => [call.authoring.authoringSessionId, call.setCurrent]), [[sessionB, true], [sessionA, false]]);
  assert.ok(native.grants.has(`${sourceA}:${sessionA}`), 'the older operation still owns a durable receipt');
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB);
});

test('a rejected Chat authoring owner cannot supersede an already admitted Code owner', async t => {
  const { root, host, row } = await fixture(t);
  const path = await fs.realpath(root), codeSession = randomUUID(), chatSession = randomUUID();
  const native = mockNativeAuthoring(host);
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === chatSession) throw new Error('Switch to Code');
    started(); await gate;
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  const admitted = host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: path, sessionId: codeSession }, setCurrent: true });
  await waiting;
  await assert.rejects(host.request({ action: 'bindAuthoring', id: row.id,
    session: { projectPath: path, sessionId: chatSession }, setCurrent: true }), /Switch to Code/);
  release(); await admitted;
  assert.equal(native.calls[0].authoring.authoringSessionId, codeSession);
  assert.equal(native.calls[0].setCurrent, true);
});

test('an admitted authoring registration finishes before the newer owner and cold repair keeps current', async t => {
  const { root, host, row } = await fixture(t);
  const requestedA = join(root, 'queued-a'), requestedB = join(root, 'queued-b');
  await fs.mkdir(requestedA); await fs.mkdir(requestedB);
  const sourceA = await fs.realpath(requestedA), sourceB = await fs.realpath(requestedB);
  const sessionA = randomUUID(), sessionB = randomUUID(), repairedSession = randomUUID();
  const native = mockNativeAuthoring(host);
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  native.setBeforeRegister(async authoring => { if (authoring.authoringSessionId === sessionA) { started(); await gate; } });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) =>
    ({ sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId });
  const a = host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceA, sessionId: sessionA }, setCurrent: true });
  let b!: Promise<unknown>;
  try {
    await waiting;
    const firstQueue = host.runtime.queues.get(row.id);
    b = host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceB, sessionId: sessionB }, setCurrent: true });
    await until(() => host.runtime.queues.get(row.id) !== firstQueue);
    assert.equal(native.calls.length, 1, 'the newer mutation waits for the admitted SDK call');
  } finally { release(); }
  await Promise.all([a, b]);
  await host.request({ action: 'bindAuthoring', id: row.id,
    session: { projectPath: sourceB, sessionId: repairedSession }, setCurrent: false });
  assert.deepEqual(native.calls.map(call => [call.authoring.authoringSessionId, call.setCurrent]),
    [[sessionA, true], [sessionB, true], [repairedSession, false]]);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB);
});

test('publication uses the originating receipt after a rebind, and a later rebind wins during publication', async t => {
  const { root, host, row } = await fixture(t);
  const requestedA = join(root, 'publish-a'), requestedB = join(root, 'publish-b');
  await fs.mkdir(requestedA); await fs.mkdir(requestedB);
  const sourceA = await fs.realpath(requestedA), sourceB = await fs.realpath(requestedB);
  const sessionA = randomUUID(), sessionB = randomUUID(), sessionC = randomUUID();
  const native = mockNativeAuthoring(host);
  let pauseA: (() => Promise<void>) | undefined;
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === sessionA && pauseA) await pauseA();
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  const bind = (projectPath: string, sessionId: string) => host.request({ action: 'bindAuthoring', id: row.id,
    session: { projectPath, sessionId }, setCurrent: true });
  await bind(sourceA, sessionA); await bind(sourceB, sessionB);
  const publications: Array<{ authoring: DesktopAuthoringRef; setCurrent: boolean }> = [];
  const publish = host.store.publish.bind(host.store);
  host.store.publish = async (input: any) => {
    if (input.nativeAuthoring) {
      publications.push(input.nativeAuthoring);
      if (input.nativeAuthoring.setCurrent) await host.store.bindAuthoring(row.id, input.nativeAuthoring.authoring);
    }
    return publish(input);
  };
  await host.request({ action: 'publish', id: row.id, expectedRevision: 1, html: '<!doctype html>one',
    source_session: { projectPath: sourceA, sessionId: sessionA, requestId: randomUUID() } });
  assert.equal(publications[0]?.authoring.authoringSessionId, sessionA, 'the publisher is not inferred from current B');
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB, 'publishing does not change the preferred authoring owner');

  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  pauseA = async () => { started(); await gate; };
  const a = host.request({ action: 'publish', id: row.id, expectedRevision: 2, html: '<!doctype html>two',
    source_session: { projectPath: sourceA, sessionId: sessionA, requestId: randomUUID() } });
  let c!: Promise<unknown>;
  try {
    await waiting;
    c = bind(sourceB, sessionC);
    await c;
  } finally { release(); }
  await Promise.all([a, c]);
  assert.equal(publications[1]?.authoring.authoringSessionId, sessionA);
  assert.equal(publications[1]?.setCurrent, false, 'the admitted publication does not replace the later explicit owner');
  assert.ok(native.grants.has(`${sourceA}:${sessionA}`));
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionC);
});

test('Chat rejects all Local App development requests before touching source or library state', async t => {
  const { host, row } = await fixture(t);
  host.options.currentConversationMode = () => 'chat';
  let writes = 0;
  host.options.resolveAuthoringBinding = async () => { writes++; throw new Error('unexpected owner lookup'); };
  host.store.get = async () => { writes++; throw new Error('unexpected library read'); };
  host.options.edit = async () => { writes++; };
  for (const action of ['prepareAuthoring', 'bindAuthoring', 'publish', 'read', 'edit', 'restoreSource', 'remix', 'marketPublish']) {
    await assert.rejects(host.request({ action, id: row.id }), /local_app_development_requires_code|Code/);
  }
  assert.equal(writes, 0);
});

test('an admitted Code authoring registration completes under its captured owner after switching to Chat', async t => {
  const { root, host, row } = await fixture(t);
  const projectPath = join(root, 'owned-code-source'); await fs.mkdir(projectPath);
  const path = await fs.realpath(projectPath), sessionId = randomUUID();
  const native = mockNativeAuthoring(host);
  let mode = 'code'; host.options.currentConversationMode = () => mode;
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    started(); await gate;
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  const admitted = host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: path, sessionId }, setCurrent: true });
  await waiting;
  mode = 'chat';
  await assert.rejects(host.request({ action: 'restoreSource', id: row.id }), /local_app_development_requires_code|Code/);
  release();
  await admitted;
  assert.equal(native.calls[0]?.authoring.authoringSessionId, sessionId);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionId);
});

test('tool source read verifies the originating owner; installed app listing and status omit source context', async t => {
  const { root, host, row } = await fixture(t);
  const path = await fs.realpath(root), chatSession = randomUUID(), codeSession = randomUUID();
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === chatSession) throw new Error('Switch to Code');
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  await host.store.bindAuthoring(row.id, { workspaceId: randomUUID(), sourceDirectory: path,
    projectRef: { kind: 'desktopPath', path }, authoringSessionId: codeSession, appId: row.id });
  const tool = { kind: 'tool' };
  await assert.rejects(host.request({ action: 'read', id: row.id,
    source_session: { projectPath: path, sessionId: chatSession, mode: 'code' } }, undefined, tool), /Switch to Code/);
  const listed = await host.request({ action: 'list', source_session: { projectPath: path, sessionId: chatSession } }, undefined, tool);
  const status = await host.request({ action: 'status', id: row.id }, undefined, tool);
  assert.equal(listed.apps[0].authoring, undefined);
  assert.equal(status.authoring, undefined);
  assert.equal(status.sourceSnapshotPath, undefined);
  const read = await host.request({ action: 'read', id: row.id,
    source_session: { projectPath: path, sessionId: codeSession, mode: 'code' } }, undefined, tool);
  assert.ok(read.sourceSnapshotPath);
  host.options.currentConversationMode = () => 'chat';
  const chatList = await host.request({ action: 'list' });
  assert.equal(chatList.apps[0].authoring, undefined);
});

test('pending publications remain visible in Chat but recovery requires Code and retains its admitted operation', async t => {
  const { host, row } = await fixture(t);
  const requestId = randomUUID();
  let mode = 'chat'; host.options.currentConversationMode = () => mode;
  const pending = { requestId, appId: row.id, sourceDirectory: '/private/source', expectedRevision: 2, state: 'pending' };
  let lookup = 0;
  host.store.pendingPublications = async () => [pending];
  host.store.publicationIntent = async () => { lookup++; return pending; };
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.store.resumePublication = async (value: string) => { assert.equal(value, requestId); started(); await gate; return row; };
  const listed = await host.request({ action: 'pendingPublications' });
  assert.equal(listed[0].requestId, requestId);
  assert.equal(listed[0].sourceDirectory, undefined);
  await assert.rejects(host.request({ action: 'resumePublication', requestId }), /local_app_development_requires_code|Code/);
  assert.equal(lookup, 0);
  mode = 'code';
  const admitted = host.request({ action: 'resumePublication', requestId });
  await waiting; mode = 'chat'; release();
  assert.equal((await admitted).id, row.id);
});

test('source copy recovery reuses its saved request and never changes a newer preferred owner', async t => {
  const { root, host, row } = await fixture(t);
  const sourcePath = join(await fs.realpath(root), 'restored-source');
  const saved = await host.authoringSourceIntents.createCopy({ purpose: 'restoreSource', appId: row.id, revision: row.revision, sourcePath });
  host.authoringSourceIntents = new AuthoringSourceIntents(join(root, 'local-apps', 'v2'));
  const calls: any[] = [];
  host.store.core.request = async (input: any) => {
    calls.push(input); await fs.mkdir(sourcePath, { recursive: true });
    return { requestId: input.requestId, sourceId: randomUUID(), sourceDirectory: sourcePath, sourcePath, targetAppId: row.id };
  };
  let mode = 'chat', edit: any;
  host.options.currentConversationMode = () => mode;
  host.options.edit = async (request: any) => { edit = request; };
  const visible = await host.request({ action: 'pendingSourceCopies' });
  assert.equal(visible[0].requestId, saved.requestId);
  assert.equal(visible[0].sourcePath, undefined);
  await assert.rejects(host.request({ action: 'resumeSourceCopy', requestId: saved.requestId }), /Code/);
  assert.equal(calls.length, 0);
  mode = 'code';
  await host.request({ action: 'resumeSourceCopy', requestId: saved.requestId });
  assert.equal(calls[0].op, 'copyAuthoringSource');
  assert.equal(calls[0].requestId, saved.requestId);
  assert.equal(calls[0].sourcePath, sourcePath);
  assert.equal(edit.authoringRequestId, saved.requestId);
  assert.equal(edit.authoringSetCurrent, false);
  assert.equal((await host.authoringSourceIntents.getCopy(saved.requestId)).state, 'ready');
});

test('an explicit remix creates one new source and a cold continuation preserves its saved identity', async t => {
  const { host, row } = await fixture(t);
  const targetId = `a${'2'.repeat(32)}`, sourceId = randomUUID(), calls: any[] = [];
  const originalGet = host.store.get.bind(host.store);
  host.store.get = async (id: string) => id === targetId ? { ...row, id: targetId, name: 'Custom app' } : originalGet(id);
  host.store.core.request = async (input: any) => {
    assert.equal(input.op, 'remixAuthoringSource'); calls.push(input);
    await fs.mkdir(input.sourcePath, { recursive: true });
    return { requestId: input.requestId, sourceId, sourceDirectory: input.sourcePath, sourcePath: input.sourcePath,
      record: { appId: targetId, latestRevision: 1 }, release: { appId: targetId, revision: 1 } };
  };
  const edits: any[] = [];
  host.options.edit = async (request: any) => { edits.push(request); };
  await host.request({ action: 'remix', id: row.id });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].appId, row.id);
  assert.equal(calls[0].revision, row.revision);
  assert.equal(edits[0].id, targetId);
  assert.equal(edits[0].authoringSetCurrent, true);
  const requestId = edits[0].authoringRequestId;
  assert.equal((await host.authoringSourceIntents.getCopy(requestId)).targetAppId, targetId);
  await host.request({ action: 'resumeSourceCopy', requestId });
  assert.equal(calls.length, 1, 'a confirmed copy is not created again');
  assert.equal(edits[1].id, targetId);
  assert.equal(edits[1].authoringRequestId, requestId);
  assert.equal(edits[1].authoringSetCurrent, false, 'cold repair cannot replace a newer preferred owner');
});

test('ordinary Code projects can publish generically but managed sources require a native receipt', async t => {
  const { root, host, row } = await fixture(t);
  const ordinary = join(root, 'ordinary'), managed = join(root, '.apps', 'new');
  await fs.mkdir(ordinary); await fs.mkdir(managed, { recursive: true });
  const sessionId = randomUUID(), ordinaryPath = await fs.realpath(ordinary), managedPath = await fs.realpath(managed);
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) =>
    ({ sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId });
  host.store.findAuthoringSource = async () => null;
  const inputs: any[] = [];
  host.store.publish = async (input: any) => { inputs.push(input); return row; };
  const request = (projectPath: string) => ({ action: 'publish', name: 'App', html: '<p>App</p>',
    source_session: { projectPath, sessionId, requestId: randomUUID() } });
  await host.request(request(ordinaryPath));
  assert.equal(inputs[0].nativeAuthoring, undefined);
  assert.equal(inputs[0].publication.sourceDirectory, ordinaryPath);
  await assert.rejects(host.request(request(managedPath)), /managed Local App source.*receipt/);
  assert.equal(inputs.length, 1);
});

test('a committed first publication can replay its own request without allowing a second new app', async t => {
  const { root, host, row } = await fixture(t);
  const path = await fs.realpath(root), sessionId = randomUUID(), requestId = randomUUID(), sourceId = randomUUID();
  const authoring = { workspaceId: randomUUID(), sourceDirectory: path, projectRef: { kind: 'desktopPath', path }, authoringSessionId: sessionId };
  host.options.resolveAuthoringBinding = async () => ({ sourceDirectory: path, projectRef: { kind: 'desktopPath', path }, authoringSessionId: sessionId });
  host.store.findAuthoringSource = async () => ({ sourceId, sourceDirectory: path, sourcePath: path, ownerAppId: row.id });
  host.store.getAuthoringGrant = async () => ({ sourceId, authoring, sessionMode: 'code', ownerAppId: row.id });
  host.store.publicationIntent = async (id: string) => id === requestId
    ? { requestId, sourceDirectory: path, authoringSessionId: sessionId, native: { sourceId, setCurrent: true } } : undefined;
  host.store.core.request = async () => null;
  const calls: any[] = [];
  host.store.publish = async (input: any) => { calls.push(input); return row; };
  const request = (id: string) => ({ action: 'publish', name: 'App', html: '<p>App</p>',
    source_session: { projectPath: path, sessionId, requestId: id } });
  await host.request(request(requestId));
  assert.equal(calls[0].publication.requestId, requestId);
  await assert.rejects(host.request(request(randomUUID())), /belongs to another application/);
  assert.equal(calls.length, 1);
});

test('a committed publication replays its historical Code receipt after its source was removed', async t => {
  const { root, host, row } = await fixture(t);
  const path = join(await fs.realpath(root), 'source-now-missing'), sessionId = randomUUID(), requestId = randomUUID(), sourceId = randomUUID();
  const publisher = { workspaceId: randomUUID(), sourceDirectory: path,
    projectRef: { kind: 'desktopPath', path }, authoringSessionId: sessionId };
  host.options.resolveAuthoringBinding = async () => { throw new Error('Do not reopen the missing source'); };
  host.options.currentConversationMode = () => 'chat';
  host.store.publicationIntent = async () => ({ requestId, sourceDirectory: path, authoringSessionId: sessionId,
    native: { sourceId, setCurrent: true }, state: 'completed' });
  let lookups = 0;
  host.store.core.request = async (input: any) => {
    lookups++;
    assert.equal(input.op, 'getPublication'); assert.equal(input.requestId, requestId);
    return { sourceId, publisher, record: { appId: row.id } };
  };
  let observed: any;
  host.store.publish = async (input: any) => { observed = input; return row; };
  const result = await host.request({ action: 'publish', name: 'App', html: '<p>original</p>',
    source_session: { projectPath: path, sessionId, mode: 'code', requestId } }, undefined, { kind: 'tool' });
  assert.equal(result.id, row.id);
  assert.equal(observed.nativeAuthoring.sourceId, sourceId);
  assert.equal(observed.nativeAuthoring.authoring.authoringSessionId, sessionId);
  assert.equal(observed.publication.requestId, requestId);
  await assert.rejects(host.request({ action: 'publish', name: 'App', html: '<p>original</p>',
    source_session: { projectPath: path, sessionId, mode: 'chat', requestId } }, undefined, { kind: 'tool' }), /local_app_development_requires_code|Code/);
  assert.equal(lookups, 1, 'Chat cannot even look up a historical Code publication');
});

test('a durable Code tool publication retains source A after its session changes cwd to B', async t => {
  const { root, host, row } = await fixture(t);
  const path = join(root, 'original-source'); await fs.mkdir(path);
  const source = await fs.realpath(path), sessionId = randomUUID(), requestId = randomUUID();
  const native = mockNativeAuthoring(host);
  host.options.resolveAuthoringBinding = async (_id: unknown, ref: { projectPath: string; sessionId: string }) => ({
    sourceDirectory: ref.projectPath, projectRef: { kind: 'desktopPath', path: ref.projectPath }, authoringSessionId: ref.sessionId,
  });
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: source, sessionId }, setCurrent: true });
  const sourceId = native.sources.get(source)!;
  host.options.resolveAuthoringBinding = async () => { throw new Error('The session cwd is now B'); };
  let state = 'pending';
  host.store.publicationIntent = async (id: string) => id === requestId ? ({ requestId, state, sourceDirectory: source,
    authoringSessionId: sessionId, appId: row.id, native: { sourceId, setCurrent: false } }) : undefined;
  host.store.core.request = async (input: any) => { assert.equal(input.op, 'getPublication'); return null; };
  const writes: any[] = [];
  host.store.publish = async (input: any) => { writes.push(input); return row; };
  const input = { action: 'publish', id: row.id, expectedRevision: 1, html: '<p>captured A</p>', source_path: source,
    source_session: { projectPath: source, sessionId, mode: 'code', requestId } };
  await host.request(input, undefined, { kind: 'tool' });
  assert.equal(writes.length, 1);
  assert.equal(writes[0].publication.sourceDirectory, source);
  assert.equal(writes[0].publication.authoringSessionId, sessionId);
  assert.equal(writes[0].publication.requestId, requestId);
  assert.equal(writes[0].nativeAuthoring.sourceId, sourceId);
  for (const bad of [
    { ...input, source_session: { ...input.source_session, mode: 'chat' } },
    { ...input, source_session: { ...input.source_session, sessionId: randomUUID() } },
    { ...input, source_session: { ...input.source_session, projectPath: join(root, 'other') } },
    { ...input, id: `a${'f'.repeat(32)}` },
  ]) await assert.rejects(host.request(bad, undefined, { kind: 'tool' }));
  state = 'preparing';
  await assert.rejects(host.request(input, undefined, { kind: 'tool' }), /cwd is now B/);
  state = 'pending';
  await assert.rejects(host.request(input, undefined, { kind: 'native', mode: 'code' }), /cwd is now B/);
  await assert.rejects(host.request(input, undefined, { kind: 'native', mode: 'chat' }), /local_app_development_requires_code|Code/);
  native.sources.set(source, randomUUID());
  await assert.rejects(host.request(input, undefined, { kind: 'tool' }), /receipt/);
  assert.equal(writes.length, 1, 'only the original captured owner reaches the publisher');
});

test('a registered source cannot publish through an unrelated or mismatched session receipt', async t => {
  const { root, host, row } = await fixture(t);
  const requested = join(root, 'protected-source'); await fs.mkdir(requested);
  const projectPath = await fs.realpath(requested), ownerSession = randomUUID(), unrelatedSession = randomUUID();
  const native = mockNativeAuthoring(host);
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) =>
    ({ sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId });
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath, sessionId: ownerSession }, setCurrent: true });
  let published = false;
  host.store.publish = async () => { published = true; throw new Error('Unexpected publication'); };
  await assert.rejects(host.request({ action: 'publish', id: row.id, expectedRevision: row.revision,
    html: '<!doctype html>unrelated', source_session: { projectPath, sessionId: unrelatedSession, requestId: randomUUID() } }), /No owner receipt/);
  await assert.rejects(host.request({ action: 'publish', name: 'Duplicate', html: '<!doctype html>duplicate',
    source_session: { projectPath, sessionId: unrelatedSession, requestId: randomUUID() } }), /No owner receipt/);
  const grant = native.grants.get(`${projectPath}:${ownerSession}`)!;
  grant.ownerAppId = `a${'f'.repeat(32)}`;
  await assert.rejects(host.request({ action: 'publish', id: row.id, expectedRevision: row.revision,
    html: '<!doctype html>wrong app', source_session: { projectPath, sessionId: ownerSession, requestId: randomUUID() } }), /belongs to another application/);
  assert.equal(published, false);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, ownerSession);
});

test('captured Local App navigation cannot activate after a newer rebind is admitted', async t => {
  const { root, host, row } = await fixture(t);
  const requestedA = join(root, 'capture-a'), requestedB = join(root, 'capture-b');
  await fs.mkdir(requestedA); await fs.mkdir(requestedB);
  const sourceA = await fs.realpath(requestedA), sourceB = await fs.realpath(requestedB);
  const sessionA = randomUUID(), sessionB = randomUUID();
  mockNativeAuthoring(host);
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === sessionB) { started(); await gate; }
    return { sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId };
  };
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: sourceA, sessionId: sessionA }, setCurrent: true });
  const captured = await host.captureAuthoringSession(row.id, { projectPath: sourceA, sessionId: sessionA }, host.authoringVersions.get(row.id));
  const replacement = host.request({ action: 'bindAuthoring', id: row.id,
    session: { projectPath: sourceB, sessionId: sessionB }, setCurrent: true });
  let activated = false;
  try {
    await waiting;
    const committing = captured.commit(() => { activated = true; });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(activated, false, 'navigation waits for the newer source verification');
    release();
    await assert.rejects(committing, /authoring source changed/);
  } finally { release(); }
  await replacement;
  assert.equal(activated, false);
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, sessionB);
});

test('a rejected replacement does not prevent the captured authoring owner from activating', async t => {
  const { root, host, row } = await fixture(t);
  const source = await fs.realpath(root), sessionA = randomUUID(), sessionB = randomUUID();
  mockNativeAuthoring(host);
  let started!: () => void, release!: () => void;
  const waiting = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) => {
    if (session.sessionId === sessionB) { started(); await gate; throw new Error('Switch to Code'); }
    return { sourceDirectory: source, projectRef: { kind: 'desktopPath', path: source }, authoringSessionId: sessionA };
  };
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath: source, sessionId: sessionA }, setCurrent: true });
  const captured = await host.captureAuthoringSession(row.id, { projectPath: source, sessionId: sessionA }, host.authoringVersions.get(row.id));
  const replacement = assert.rejects(host.request({ action: 'bindAuthoring', id: row.id,
    session: { projectPath: source, sessionId: sessionB }, setCurrent: true }), /Switch to Code/);
  await waiting;
  let activated = false;
  const committing = captured.commit(() => { activated = true; });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(activated, false);
  release(); await replacement; await committing;
  assert.equal(activated, true);
});

test('an explicit source restore keeps current until the saved copy intent becomes a Code receipt', async t => {
  const { root, host, row } = await fixture(t);
  const original = join(root, 'restore-original'); await fs.mkdir(original);
  const projectPath = await fs.realpath(original), originalSession = randomUUID(), restoredSession = randomUUID();
  const native = mockNativeAuthoring(host);
  host.options.resolveAuthoringBinding = async (_appId: string | undefined, session: { projectPath: string; sessionId: string }) =>
    ({ sourceDirectory: session.projectPath, projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId });
  await host.request({ action: 'bindAuthoring', id: row.id, session: { projectPath, sessionId: originalSession }, setCurrent: true });
  let edit: any;
  host.options.edit = async (request: unknown) => { edit = request; };
  host.store.core.request = async (request: any) => {
    assert.equal(request.op, 'copyAuthoringSource');
    await fs.mkdir(request.sourcePath, { recursive: true });
    const sourceId = randomUUID(); native.sources.set(request.sourcePath, sourceId);
    return { requestId: request.requestId, sourceId, sourceDirectory: request.sourcePath, sourcePath: request.sourcePath, targetAppId: row.id };
  };
  await host.request({ action: 'restoreSource', id: row.id });
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, originalSession, 'copy alone does not change current');
  assert.equal(edit.authoringSetCurrent, true);
  const copy = await host.authoringSourceIntents.getCopy(edit.authoringRequestId);
  assert.equal(copy.state, 'ready');
  await host.request({ action: 'bindAuthoring', id: row.id, setCurrent: true, sourceRequestId: copy.requestId,
    session: { projectPath: copy.sourcePath, sessionId: restoredSession } });
  assert.equal(native.calls.at(-1)?.requestId, copy.requestId, 'the receipt reuses the persisted copy operation');
  assert.equal((await host.authoringSourceIntents.getCopy(copy.requestId)).state, 'completed');
  assert.equal((await host.store.get(row.id)).authoring.authoringSessionId, restoredSession);
});

test('host derives Agent identity from exact main frame and running invocation', async t => {
  const { host, owner, event, mainFrame } = await fixture(t);
  let received: unknown;
  host.setAgentBroker({ run: async (identity: unknown, input: unknown) => { received = { identity, input }; return { runId: 'run1' }; } });
  const input = runInput('run-1');
  await host.native(event, { method: 'agent.tasks.start', args: { input, appId: 'forged', releaseDigest: 'forged', invocationId: 'forged' } });
  assert.deepEqual(JSON.parse(JSON.stringify(received)), { identity: owner, input });
  await assert.rejects(host.native({ ...event, senderFrame: { ...mainFrame } }, { method: 'agent.tasks.start', args: { input } }), /Unauthorized/);
  mainFrame.url = 'https://untrusted.example/';
  await assert.rejects(host.native(event, { method: 'agent.tasks.start', args: { input } }), /Unauthorized/);
});

test('authorization checks running manifest, lower task limits and MCP provenance', async t => {
  const { host, row, owner, catalog } = await fixture(t);
  const authorization = await host.authorizeAgent(owner);
  const narrowed = structuredClone(authorization); narrowed.settings.maxTurns = 5; narrowed.settings.timeoutMs = 5000; narrowed.settings.tools = [mcp];
  assert.equal(await host.revalidateAgent(owner, narrowed), true);
  await host.store.publish({ id: row.id, html: '<!doctype html><title>New inactive version</title>' });
  assert.equal(await host.revalidateAgent(owner, narrowed), true, 'published version does not redefine running invocation requirements');
  catalog.tools = [{ ...mcp, configRevision: 'changed' }, builtin];
  assert.equal(await host.revalidateAgent(owner, narrowed), false);
  catalog.tools = [mcp, builtin]; host.runtime.invocationId = () => 'replacement';
  assert.equal(await host.revalidateAgent(owner, narrowed), false);
});

test('grant changes persist catalog-bound identity and refuse undeclared tools or stale MCP definitions', async t => {
  const { host, row, catalog } = await fixture(t);
  const order: string[] = [];
  host.setAgentBroker({ cancelApp: async () => { order.push('cancelled'); } });
  await host.request({ action: 'agent-settings', id: row.id, settings: { ...settings, tools: [builtin], background: true } });
  const saved = await host.store.get(row.id);
  assert.deepEqual(JSON.parse(JSON.stringify(saved.agent.tools)), [{ id: 'builtin:Read', kind: 'builtin', name: 'Read' }]);
  assert.equal(saved.agent.background, true); assert.deepEqual(order, ['cancelled']);
  await assert.rejects(host.request({ action: 'agent-settings', id: row.id, settings: { ...settings, tools: [{ ...mcp, definitionDigest: 'old' }] } }), /Only current tools/);
  catalog.tools.push({ id: 'builtin:Write', kind: 'builtin', name: 'Write', label: 'Write' });
  await assert.rejects(host.request({ action: 'agent-settings', id: row.id, settings: { ...settings, tools: [catalog.tools[2]] } }), /requested/);
});

test('Core releases that omit an empty Skill list can enable Agent access without granting undeclared Skills', async t => {
  const { host, row } = await fixture(t);
  const get = host.store.get.bind(host.store);
  // Rust omits an empty skills array from its serialized release manifest.
  // The disk fixture normalizes it, so reproduce the actual Core DTO boundary.
  host.store.get = async (id: string) => {
    const value = await get(id);
    value.manifest.agent = { tools: [] };
    return value;
  };
  const emptySettings = { ...settings, tools: [], skills: [] };
  await host.request({ action: 'agent-settings', id: row.id, settings: emptySettings });
  const saved = await host.store.get(row.id);
  assert.equal(saved.agent.enabled, true);
  assert.deepEqual(JSON.parse(JSON.stringify(saved.agent.skills)), []);
  const catalog = await host.request({ action: 'agent-catalog', id: row.id });
  assert.deepEqual(JSON.parse(JSON.stringify(catalog.skills)), []);
  await assert.rejects(host.request({ action: 'agent-settings', id: row.id,
    settings: { ...emptySettings, skills: ['nutrition'] } }), /Only Skills declared/);
});

test('stop waits for Agent cancellation before stopping the runtime and denies new admission immediately', async t => {
  const { host, row, owner } = await fixture(t);
  const order: string[] = [];
  let release!: () => void;
  host.setAgentBroker({ cancelApp: async () => { order.push('cancel-start'); await new Promise<void>(resolve => { release = resolve; }); order.push('cancel-done'); } });
  host.runtime.stop = async () => { order.push('runtime-stop'); };
  const stopped = host.request({ action: 'stop', id: row.id });
  await new Promise(resolve => setTimeout(resolve, 0));
  await assert.rejects(host.authorizeAgent(owner), /no longer active/);
  assert.deepEqual(order, ['cancel-start']);
  release(); await stopped;
  assert.deepEqual(order, ['cancel-start', 'cancel-done', 'runtime-stop']);
});

test('ordinary background preference does not authorize Agent background execution', async t => {
  const { host, row, owner } = await fixture(t);
  row.background = true; await host.store.save(row); host.windows.clear();
  await assert.rejects(host.authorizeAgent(owner), /background access/);
  row.agent.background = true; await host.store.save(row);
  assert.equal((await host.authorizeAgent(owner)).settings.background, true);
  host.runtime.phase = () => 'stopped';
  await assert.rejects(host.authorizeAgent(owner), /no longer active/);
  host.backgroundAgentOwners.set(row.id, { revision: 1, invocationId: owner.invocationId });
  assert.equal((await host.authorizeAgent(owner)).settings.background, true, 'Agent can finish while ordinary app processes are stopped');
});

test('Agent access can be disabled after a connector disappeared and is re-enabled only by trusted settings', async t => {
  const { host, row, catalog, owner, event } = await fixture(t);
  host.setAgentBroker({ cancelApp: async () => {} });
  catalog.models = []; catalog.tools = [];
  await host.request({ action: 'agent-settings', id: row.id, settings: { ...settings, enabled: false } });
  const revoked = await host.store.get(row.id);
  assert.equal(revoked.agent.enabled, false); assert.equal(revoked.agent.background, false); assert.equal(revoked.agent.tools.length, 0);
  await assert.rejects(host.native(event, { method: 'agent.tasks.get', args: { runId: 'run1' } }), /not granted|no longer active/);
  catalog.models = [model]; catalog.tools = [builtin, mcp];
  await host.request({ action: 'agent-settings', id: row.id, settings });
  assert.equal((await host.authorizeAgent(owner)).settings.enabled, true);
});

test('closing and reopening an Agent-background app preserves its task and invocation without keeping app services alive', async t => {
  const { host, row, root } = await fixture(t);
  host.windows.clear();
  host.runtime = new LocalAppRuntime(host.store, () => host.changed(), { onRunEnded: (id, run, reason) => host.runEnded(id, run, reason) });
  host.runtime.owner = (id: string) => host.runtime.invocationId(id) ? ({ appId: id, runId: host.runtime.invocationId(id)!, revision: host.runtime.revision(id) ?? 1, releaseDigest: row.releaseDigest! }) : undefined;
  row.agent.background = true; await host.store.save(row);
  let complete!: () => void;
  const held = new Promise<void>(resolve => { complete = resolve; });
  const broker = new LocalAppAgentBroker({ workspaceRoot: join(root, 'workspaces'), root: join(root, 'ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
    execute: async execution => { await held; await execution.assertAuthorized(); return { result: { text: 'Finished after reopening', output: { ok: true }, attachments: [], messageId: `assistant-${execution.runId}` }, usage: { turns: 1 } }; } });
  host.setAgentBroker(broker); t.after(async () => { complete(); await broker.dispose(); });
  await host.open(row.id);
  const first = host.windows.get(row.id), originalInvocation = host.runtime.invocationId(row.id);
  const eventFor = (window: any) => ({ sender: window.webContents, senderFrame: window.webContents.mainFrame });
  const input = runInput('background-reopen');
  input.forwardedProps['com.lingxi.localApp'].skills = ['nutrition'];
  const { runId } = await host.native(eventFor(first), { method: 'agent.tasks.start', args: { input } });
  first.close();
  await until(() => host.runtime.phase(row.id) === 'stopped');
  assert.equal(broker.hasActiveApp(row.id), true);
  await host.open(row.id);
  const reopened = host.windows.get(row.id);
  assert.notEqual(reopened.webContents.id, first.webContents.id);
  assert.equal(host.runtime.invocationId(row.id), originalInvocation);
  assert.ok(['queued', 'running'].includes((await host.native(eventFor(reopened), { method: 'agent.tasks.get', args: { runId } })).state));
  const firstPage = await host.native(eventFor(reopened), { method: 'agent.tasks.events', args: { runId, afterSequence: 0 } });
  assert.equal(firstPage.events[0].type, 'RUN_STARTED');
  complete();
  await until(async () => (await host.native(eventFor(reopened), { method: 'agent.tasks.get', args: { runId } })).state === 'succeeded');
  const finalPage = await host.native(eventFor(reopened), { method: 'agent.tasks.events', args: { runId, afterSequence: firstPage.sequence } });
  assert.equal(finalPage.events.at(-1).type, 'RUN_FINISHED');
  assert.equal(finalPage.events.at(-1).threadId, input.threadId);
  assert.equal(finalPage.events.at(-1).runId, input.runId);
});

test('a Desktop Agent settling after the last remote session releases its retained app run', async t => {
  const { host, row, root } = await fixture(t);
  host.windows.clear();
  host.runtime = new LocalAppRuntime(host.store, () => host.changed(), { onRunEnded: (id, run, reason) => host.runEnded(id, run, reason) });
  row.agent.background = true; await host.store.save(row);
  let live = false, idleChecks = 0, complete!: () => void, entered!: () => void;
  const held = new Promise<void>(resolve => { complete = resolve; });
  const executing = new Promise<void>(resolve => { entered = resolve; });
  host.options.onRemoteAccessEnded = () => { idleChecks++; };
  host.remoteSettings = async () => ({ [row.id]: true });
  host.remoteGateway = {
    open: async (run: any) => { live = true; return { app_id: run.appId, revision: run.revision, url: 'https://example.com/#ticket', expires_at: Date.now() + 60000 }; },
    hasRun: () => live, hasAccess: () => live, revokeApp: () => { live = false; },
  };
  const broker = new LocalAppAgentBroker({ workspaceRoot: join(root, 'workspaces'), root: join(root, 'remote-ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
    execute: async execution => { entered(); await held; await execution.assertAuthorized(); return { result: { text: 'done', output: { ok: true }, attachments: [], messageId: 'assistant' }, usage: { turns: 1 } }; } });
  host.setAgentBroker(broker); t.after(async () => { complete(); await broker.dispose(); });
  await host.open(row.id);
  const window = host.windows.get(row.id), invocation = host.runtime.invocationId(row.id);
  const input = runInput('desktop-remote-cleanup'); input.forwardedProps['com.lingxi.localApp'].skills = ['nutrition'];
  await host.native({ sender: window.webContents, senderFrame: window.webContents.mainFrame }, { method: 'agent.tasks.start', args: { input } });
  await executing;
  await host.remoteOpen(row.id, { accountUid: 'account', deviceId: 'phone', hostId: 'desktop' });
  window.close(); await host.remoteCleaning.get(row.id);
  live = false; await host.remoteAccessEnded(row.id, invocation);
  assert.equal(host.runtime.phase(row.id), 'running');
  const checksBeforeSettlement = idleChecks;
  complete();
  await until(() => host.runtime.phase(row.id) === 'stopped');
  assert.ok(idleChecks > checksBeforeSettlement, 'task settlement rechecks process lifetime');
});

test('remote opening resumes the invocation retained by a Desktop background Agent', async t => {
  const { host, row, root } = await fixture(t);
  host.windows.clear();
  host.runtime = new LocalAppRuntime(host.store, () => host.changed(), { onRunEnded: (id, run, reason) => host.runEnded(id, run, reason) });
  // Isolate the pre-existing stopped-owner behavior: retain the trusted owner
  // just as the existing native reopen regression does.
  host.runtime.owner = (id: string) => host.runtime.invocationId(id) ? ({ appId: id, runId: host.runtime.invocationId(id)!, revision: host.runtime.revision(id) ?? 1, releaseDigest: row.releaseDigest! }) : undefined;
  row.agent.background = true; await host.store.save(row);
  let live = false, complete!: () => void, entered!: () => void;
  const held = new Promise<void>(resolve => { complete = resolve; });
  const executing = new Promise<void>(resolve => { entered = resolve; });
  host.remoteSettings = async () => ({ [row.id]: true });
  host.remoteGateway = { open: async (run: any) => { live = true; return { app_id: run.appId, revision: run.revision, url: 'https://example.com/#ticket', expires_at: Date.now() + 60000 }; },
    hasRun: () => live, hasAccess: () => live, revokeApp: () => { live = false; } };
  const broker = new LocalAppAgentBroker({ workspaceRoot: join(root, 'workspaces'), root: join(root, 'resume-ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
    execute: async execution => { entered(); await held; await execution.assertAuthorized(); return { result: { text: 'done', output: { ok: true }, attachments: [], messageId: 'assistant' }, usage: { turns: 1 } }; } });
  host.setAgentBroker(broker); t.after(async () => { complete(); await broker.dispose(); });
  await host.open(row.id);
  const window = host.windows.get(row.id), invocation = host.runtime.invocationId(row.id);
  const input = runInput('desktop-remote-resume'); input.forwardedProps['com.lingxi.localApp'].skills = ['nutrition'];
  const identity = { appId: row.id, releaseDigest: row.releaseDigest!, invocationId: invocation, webContentsId: window.webContents.id };
  await host.native({ sender: window.webContents, senderFrame: window.webContents.mainFrame }, { method: 'agent.tasks.start', args: { input } });
  await executing;
  // No remote grant exists yet, so closing the window stops only app services.
  window.close(); await until(() => host.runtime.phase(row.id) === 'stopped');
  await host.remoteOpen(row.id, { accountUid: 'account', deviceId: 'phone', hostId: 'desktop' });
  assert.equal(host.runtime.invocationId(row.id), invocation);
  complete(); await until(async () => !['queued', 'running'].includes((await broker.get(identity, input.runId)).state));
  assert.equal((await broker.get(identity, input.runId)).state, 'succeeded');
});

test('trusted host question supports text and multi-choice answers and cancellation clears pending prompt', async t => {
  const { host, row } = await fixture(t);
  const controller = new AbortController();
  const input = { appId: row.id, runId: 'run1', requestId: 7, questions: [{ question: 'Choose ingredients', header: 'Food', options: [{ label: 'Rice', description: '' }, { label: 'Beans', description: '' }], multi_select: true }] };
  const result = host.promptAgentQuestion(input, controller.signal);
  await until(() => host.agentQuestions.size === 1);
  const pending = await host.request({ action: 'agent-questions' });
  assert.equal(pending.length, 1);
  await host.request({ action: 'agent-answer', questionId: pending[0].id, answers: { 'Choose ingredients': 'Rice, Beans, and sauce' } });
  assert.deepEqual(JSON.parse(JSON.stringify(await result)), { answers: [{ question: 'Choose ingredients', answer: 'Rice, Beans, and sauce' }] });
  const cancelled = host.promptAgentQuestion(input, controller.signal);
  await until(() => host.agentQuestions.size === 1); controller.abort();
  assert.equal(await cancelled, null); assert.equal((await host.request({ action: 'agent-questions' })).length, 0);
});

test('host returns persisted standard BaseEvent pages and rejects a navigated application frame', async t => {
  const { host, event, mainFrame, owner } = await fixture(t);
  const officialEvent = { type: 'RUN_STARTED', threadId: 'thread-1', runId: 'run-1', protocolVersion: '1.0' };
  host.setAgentBroker({ events: async (identity: unknown, runId: string, afterSequence: number) => {
    assert.deepEqual(JSON.parse(JSON.stringify(identity)), owner); assert.equal(runId, 'run-1'); assert.equal(afterSequence, 0);
    return { events: [officialEvent], sequence: 1 };
  } });
  const page = await host.native(event, { method: 'agent.tasks.events', args: { runId: 'run-1', afterSequence: 0 } });
  assert.deepEqual(JSON.parse(JSON.stringify(page)), { events: [officialEvent], sequence: 1 });
  mainFrame.url = 'https://untrusted.example/';
  await assert.rejects(host.native(event, { method: 'agent.tasks.events', args: { runId: 'run-1', afterSequence: 0 } }), /Unauthorized/);
});

test('pending Desktop Agent reads cannot reuse authorization after withdrawal and regrant', async t => {
  const reads = [
    ['agent.tasks.get', 'get', { runId: 'task' }],
    ['agent.tasks.list', 'list', {}],
    ['agent.tasks.events', 'events', { runId: 'task' }],
    ['agent.chats.get', 'getChat', { chatId: 'chat' }],
    ['agent.chats.history', 'chatHistory', { chatId: 'chat' }],
  ] as const;
  for (const [method, operation, args] of reads) await t.test(method, async t => {
    const { host, row, event } = await fixture(t);
    let release!: () => void, entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    const privateResult = { content: 'private Agent output' };
    host.setAgentBroker({ cancelApp: async () => {}, [operation]: async () => { entered(); await gate; return privateResult; } });
    const pending = host.native(event, { method, args });
    const rejected = assert.rejects(pending, /Application Agent access changed/);
    await waiting;
    await host.cancelAgentApp(row.id, 'Agent access withdrawn');
    host.blockedAgentApps.delete(row.id);
    release(); await rejected;
    assert.deepEqual(await host.native(event, { method, args }), privateResult);
  });
});

test('preload exposes only host-scoped AG-UI resource methods without event streaming IPC', async () => {
  const result = await build({ entryPoints: [new URL('../src/preload/localApp.ts', import.meta.url).pathname], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'] });
  let bridge: any;
  const requests: unknown[] = [];
  runInNewContext(result.outputFiles[0]!.text, { console, require: () => ({ contextBridge: { exposeInMainWorld: (_name: string, value: unknown) => { bridge = value; } },
    ipcRenderer: { invoke: async (...args: unknown[]) => { requests.push(args); } } }) });
  assert.deepEqual(Object.keys(bridge.agent).sort(), ['chats', 'tasks']);
  assert.deepEqual(Object.keys(bridge.agent.tasks).sort(), ['cancel', 'events', 'get', 'list', 'start']);
  await bridge.agent.tasks.events({ runId: 'run-1', afterSequence: 3 });
  assert.equal(requests.length, 1);
  assert.ok(Array.isArray(requests[0]));
  assert.deepEqual(JSON.parse(JSON.stringify(requests[0][1])), { method: 'agent.tasks.events', args: { runId: 'run-1', afterSequence: 3 } });
  assert.ok(!('subscribe' in bridge.agent.tasks));
});

test('creating chat can discover public tool IDs before a manifest exists, without granting them', async t => {
  const { host, row, catalog, event } = await fixture(t);
  const before = JSON.stringify((await host.store.get(row.id)).agent);
  assert.deepEqual(JSON.parse(JSON.stringify(await host.request({ action: 'agent_catalog' }))), catalog);
  assert.equal(JSON.stringify((await host.store.get(row.id)).agent), before);
  await assert.rejects(host.native(event, { method: 'agent_catalog' }), /Unknown/);
});
