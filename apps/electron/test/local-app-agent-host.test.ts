import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { runInNewContext } from 'node:vm';
import { EventEmitter } from 'node:events';
import { build } from 'esbuild';
import { LocalAppStore } from './fixtures/local-app-store.ts';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.ts';
import { LocalAppAgentBroker } from '../src/main/local-apps/agentBroker.ts';
import { parseManifest } from '../src/main/local-apps/manifest.ts';
import type { LocalAppAgentCatalog, LocalAppAgentSettings } from '../src/shared/localAppAgent.ts';

const require = createRequire(import.meta.url);
const model = { providerId: 'configured', modelId: 'vision', label: 'Vision', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true };
const builtin = { id: 'builtin:Read', kind: 'builtin' as const, name: 'Read', label: 'Read files' };
const mcp = { id: 'mcp:stable', kind: 'mcp' as const, serverConfigId: 'server-one', configRevision: 'rev1', rawToolName: 'nutrition', definitionDigest: 'digest1', label: 'Nutrition' };
const settings: LocalAppAgentSettings = { enabled: true, model, tools: [builtin, mcp], skills: ['nutrition'], background: false, maxTurns: 20, timeoutMs: 600000 };
const AGUI_FIXTURE = JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8')) as any;
function runInput(runId: string, threadId = `thread-${runId}`) { const input = structuredClone(AGUI_FIXTURE); input.runId = runId; input.threadId = threadId; return input; }
const bundledHost = build({ entryPoints: [new URL('../src/main/local-apps/host.ts', import.meta.url).pathname], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'] });
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
  runInNewContext(result.outputFiles[0]!.text, { module, exports: module.exports, require: (id: string) => id === 'electron' ? mockElectron : require(id),
    Buffer, process, console, structuredClone, URL, Response, Headers, fetch, AbortController, setTimeout, clearTimeout });
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
  const host = new LocalAppHost({ core: { onDisconnected() {} }, configHome: root, preload: '/unused', mainWindow: () => window, edit: async () => {}, agentCatalog: async () => structuredClone(catalog) });
  host.store = new LocalAppStore(join(root, 'fixture-apps')); host.runtime.store = host.store;
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
  const broker = new LocalAppAgentBroker({ root: join(root, 'ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
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
  const broker = new LocalAppAgentBroker({ root: join(root, 'remote-ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
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
  const broker = new LocalAppAgentBroker({ root: join(root, 'resume-ledger'), authorize: owner => host.authorizeAgent(owner), revalidate: (owner, authorization) => host.revalidateAgent(owner, authorization),
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
