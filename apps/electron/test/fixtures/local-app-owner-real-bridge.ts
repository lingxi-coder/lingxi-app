import { app } from 'electron';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { CH_CONVERSATION_SPACE } from '../../src/shared/conversationSpaces';
import { CH_SESSION_NEW, HostController } from '../../src/main/host';
import { SessionRuntime } from '../../src/main/bridge';
import { SessionRuntimeManager } from '../../src/main/sessionRuntimeManager';
import { SettingsStore } from '../../src/main/settings';
import { ProjectSessionCatalog } from '../../src/main/session-catalog';
import { DiagnosticBuffer } from '../../src/main/host-utils';
import { LocalAppHost } from '../../src/main/local-apps/host';
import { LocalAppCoreClient } from '../../src/main/local-apps/coreClient';
import { resolveDesktopAuthoringBinding } from '../../src/main/local-apps/authoring';

const root = process.env.LINGXI_TEST_USER_DATA!, brand = process.env.LINGXI_CONFIG_DIR!, serverBin = process.env.LINGXI_BRIDGE_SERVER_BIN!;
app.setPath('userData', join(root, 'profile'));
function deferred() { let resolve!: () => void; const promise = new Promise<void>(yes => { resolve = yes; }); return { promise, resolve }; }

async function main() {
  await app.whenReady(); app.dock?.hide();
  await fs.mkdir(brand, { recursive: true, mode: 0o700 });
  const project = join(root, 'Reading garden'); await fs.mkdir(project, { mode: 0o700 });
  const workspace = await fs.realpath(project), stateDir = join(root, 'desktop-state');
  let settings = new SettingsStore(stateDir);
  settings.addProject(workspace); settings.conversationSpaces.addProject(workspace, 'code'); settings.conversationSpaces.setMode('code');
  let manager: SessionRuntimeManager | undefined, localApps: LocalAppHost | undefined;
  const create = () => {
    const diagnostics = new DiagnosticBuffer();
    const catalog = new ProjectSessionCatalog({ serverBin, configHome: brand });
    manager = new SessionRuntimeManager({ serverBin, bridgeRoot: join(root, 'bridge'), lockfileTimeoutMs: 60_000,
      launchConfig: ref => ({ workspace: ref.projectPath, trusted: true, configHome: brand }),
      accessState: ref => ({ workspace: ref.projectPath, trusted: true }), diagnostics });
    // HostController uses this fixture's in-memory invocation map. The runtime
    // itself still launches the real sidecar and performs its WebSocket work.
    (manager as any).registerIpc = () => {};
    const handlers = new Map<string, (...args: any[]) => any>();
    const host = new HostController(settings, manager, diagnostics, catalog,
      { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler() {} } as any);
    (host as any).bootstrap = async () => ({ settings: settings.getPublic(), conversationSpaces: settings.conversationSpaces.snapshot() });
    const remember = host.rememberCreatedSessionSource.bind(host);
    host.rememberCreatedSessionSource = async (...args) => {
      try { return await remember(...args); }
      catch (error) {
        const state = await args[1].getWorkspaceState().catch(() => undefined);
        process.stderr.write(`OWNER_SOURCE_ERROR:${JSON.stringify({ expected: args[4], cwd: state?.cwd, actual: state?.transcript_path })}\n`);
        throw error;
      }
    };
    localApps = new LocalAppHost({ configHome: brand, appProjectsRoot: join(brand, '.apps'),
      core: new LocalAppCoreClient(serverBin, brand), currentConversationMode: () => host.currentConversationMode(),
      preload: join(root, 'unused-preload.cjs'), mainWindow: () => undefined, edit: async () => undefined,
      resolveAuthoringBinding: async (_id, ref, source) => {
        const remembered = settings.getSessionTranscriptSelection(ref);
        if (remembered && (remembered.catalogProjectPath !== ref.projectPath
          || (source && (source.scope !== remembered.scope || source.transcriptPath !== remembered.transcriptPath)))) {
          throw new Error('The Local App session source changed');
        }
        const exact = source ?? (remembered && { scope: remembered.scope, transcriptPath: remembered.transcriptPath });
        return resolveDesktopAuthoringBinding(catalog, ref, manager!.getActiveDraftSession(ref, settings.getPublic().activeSession ?? undefined), exact);
      } });
    host.attachLocalAppSessions({ begin: intent => localApps!.beginAuthoringSession(intent), get: id => localApps!.getAuthoringSession(id),
      recordSource: (id, source) => localApps!.recordAuthoringSessionSource(id, source), finish: saved => localApps!.finishAuthoringSession(saved) });
    const frame = { url: 'http://127.0.0.1:4242' };
    const sender = Object.assign(new EventEmitter(), { id: 1, mainFrame: frame, isDestroyed: () => false, send() {}, getURL: () => frame.url });
    host.registerWindow(sender as any, frame.url); host.registerIpc();
    return { host, handlers, event: { sender, senderFrame: frame }, catalog };
  };
  let first = create();
  const originalStart = SessionRuntime.prototype.start;
  const ready = deferred(), release = deferred();
  let intercepted = false;
  SessionRuntime.prototype.start = async function (options) {
    await originalStart.call(this, options);
    if (!intercepted) { intercepted = true; ready.resolve(); await release.promise; }
  };
  try {
    const creating = first.handlers.get(CH_SESSION_NEW)!(first.event, workspace, undefined,
      { kind: 'local-app', intent: { name: 'Reading garden', setCurrent: false } });
    await ready.promise;
    const switching = first.handlers.get(CH_CONVERSATION_SPACE)!(first.event, 'chat');
    assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
    release.resolve(); await Promise.all([creating, switching]);
    const [pending] = await localApps!.request({ action: 'pendingAuthoringSessions' }) as Array<{ requestId: string }>;
    assert.ok(pending?.requestId);
    const saved = await localApps!.getAuthoringSession(pending.requestId);
    assert.equal(saved.state, 'ready'); assert.ok(saved.source);
    assert.equal(settings.getPublic().activeSession, undefined);
    assert.equal(settings.conversationSpaces.snapshot().activeMode, 'chat');
    assert.equal((manager as any).activeSessionId, null);
    const rows = (await fs.readFile(saved.source!.transcriptPath, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
    assert.equal(rows[0].type, 'fixed-session-mode');
    assert.equal(rows[0].sessionMode, 'code'); assert.equal(rows[0].sessionId, saved.sessionId);
    assert.ok(rows.some(row => row.type === 'custom-title' && row.customTitle === 'Reading garden' && row.mobileEmptySession === 1));
    const grant = await localApps!.store.getAuthoringGrant(workspace, { kind: 'desktopPath', path: workspace }, saved.sessionId);
    assert.equal(grant.authoring.authoringSessionId, saved.sessionId); assert.equal(grant.sessionMode, 'code');
    await assert.rejects(first.handlers.get(CH_SESSION_NEW)!(first.event, null, undefined,
      { kind: 'recover-local-app', requestId: saved.requestId }), /local_app_development_requires_code|Code/);
    SessionRuntime.prototype.start = originalStart;
    await manager!.dispose(); await localApps!.dispose(); manager = undefined; localApps = undefined;

    // Restart every Host-owned object and recover the exact saved source only
    // after an explicit transition back to Code.
    settings = new SettingsStore(stateDir);
    const second = create();
    await second.handlers.get(CH_CONVERSATION_SPACE)!(second.event, 'code');
    await second.handlers.get(CH_SESSION_NEW)!(second.event, null, undefined,
      { kind: 'recover-local-app', requestId: saved.requestId });
    assert.equal(settings.getPublic().activeSession?.sessionId, saved.sessionId);
    assert.equal(settings.getSessionTranscriptSelection({ projectPath: workspace, sessionId: saved.sessionId })?.transcriptPath, saved.source!.transcriptPath);
    const resumed = manager!.get(saved.sessionId); assert.ok(resumed);
    const state = await resumed.getWorkspaceState(); assert.ok(state.transcript_path);
    const [actualPath, savedPath] = await Promise.all([fs.realpath(state.transcript_path), fs.realpath(saved.source!.transcriptPath)]);
    assert.equal(actualPath, savedPath);
    const [actualFile, savedFile] = await Promise.all([fs.lstat(state.transcript_path), fs.lstat(saved.source!.transcriptPath)]);
    assert.ok(actualFile.isFile() && savedFile.isFile());
    assert.deepEqual([actualFile.dev, actualFile.ino], [savedFile.dev, savedFile.ino]);
    const reopened = await localApps!.store.getAuthoringGrant(workspace, { kind: 'desktopPath', path: workspace }, saved.sessionId);
    assert.equal(reopened.sourceId, grant.sourceId); assert.equal(reopened.authoring.workspaceId, grant.authoring.workspaceId);
    process.stdout.write('OWNER_RESULT:' + JSON.stringify({ codeToChatParked: true, fixedHeader: true,
      title: 'Reading garden', exactSource: true, coldRecovery: true }) + '\n');
  } finally {
    release.resolve(); SessionRuntime.prototype.start = originalStart;
    await manager?.dispose(); await localApps?.dispose();
  }
}
void main().then(() => app.exit(0)).catch(error => { process.stderr.write(`${error?.stack ?? error}\n`); app.exit(1); });
