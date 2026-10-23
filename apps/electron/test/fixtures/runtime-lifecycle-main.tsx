import { createRoot } from 'react-dom/client';
import { useBridge } from '../../src/renderer/bridge/useBridge';

const projectPath = '/runtime-lifecycle';
const snapshot = (sessionId: string, revision: number) => ({
  revision,
  settings: { projects: [projectPath], activeProject: projectPath, activeSession: { projectPath, sessionId }, pinnedSessions: [], archivedSessions: [] },
  activeSession: { projectPath, sessionId }, workspace: { path: projectPath, trusted: true },
  connection: { status: 'connected' }, diagnostics: [], projectCatalogs: {},
  runtimes: [{ projectPath, sessionId, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
});
let state = snapshot('alpha', 1);
let bridge: ReturnType<typeof useBridge>;
let onState: (event: unknown) => void;
let heldBootstrap: ((value: unknown) => void) | undefined;
let holdBootstrap = true;
let holdCommands = false;
const pendingCommands: Array<(error: Error) => void> = [];
const commands: Array<{ sessionId: string; type: string }> = [];
let currentFailure: string | undefined;
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  onConnectionStateChanged: (listener: typeof onState) => { onState = listener; return () => {}; },
  bootstrap: async () => holdBootstrap ? new Promise(resolve => { heldBootstrap = resolve; }) : state,
  openSession: async (_path: string, sessionId: string) => { state = snapshot(sessionId, state.revision + 1); return state; },
  command: async (sessionId: string, command: { type: string }) => {
    commands.push({ sessionId, type: command.type });
    if (holdCommands && ['list_models', 'task_list'].includes(command.type)) {
      return new Promise<void>((_resolve, reject) => { pendingCommands.push(reject); });
    }
    if (command.type === currentFailure) throw new Error(`current ${command.type} failed`);
  },
};
function Fixture() { bridge = useBridge(); return <output>{bridge.activeSession?.sessionId ?? 'none'}:{bridge.error ?? 'ok'}</output>; }
const root = createRoot(document.getElementById('root')!);
root.render(<Fixture />);
const delay = () => new Promise(resolve => setTimeout(resolve, 20));
async function wait(predicate: () => boolean) {
  for (let i = 0; i < 200; i++) { if (predicate()) return; await delay(); }
  throw new Error(`Fixture did not settle: ${document.body.innerText}`);
}
function check(value: unknown, message: string) { if (!value) throw new Error(message); }
const connection = (sessionId: string, status: string, reason?: string) => onState({ sessionId, event: { status, ...(reason ? { reason } : {}) } });
(window as any).runtimeLifecycleFixture = {
  run: async () => {
    await wait(() => !!heldBootstrap);
    connection('retired-background', 'disconnected', 'session runtime disposed');
    heldBootstrap!(state);
    holdBootstrap = false;
    await wait(() => bridge?.connected && commands.some(c => c.type === 'task_list'));
    check(bridge.activeSession?.sessionId === 'alpha', 'Background disposal discarded the only initial snapshot');
    connection('alpha', 'connecting');
    await wait(() => !bridge.connected);
    holdCommands = true;
    holdBootstrap = true;
    connection('alpha', 'connected');
    await wait(() => pendingCommands.length === 2 && !!heldBootstrap);
    const staleSnapshot = state;
    connection('alpha', 'disconnected', 'session runtime disposed');
    await delay();
    check(!bridge.activeSession, 'Disposed owner retained its active session');
    check(!bridge.bootstrap?.settings.activeSession, 'Disposed owner retained its persisted session');
    check(bridge.connection.status === 'idle', 'Disposed owner retained the bootstrap connected fallback');
    check(bridge.bootstrap?.runtimes.length === 0, 'Disposed runtime summary was retained');
    const commandCount = commands.filter(c => !c.type.startsWith('ui_')).length;
    for (const reject of pendingCommands.splice(0)) reject(new Error('obsolete metadata failure'));
    heldBootstrap!(staleSnapshot);
    holdCommands = holdBootstrap = false;
    await delay(); await delay();
    check(!bridge.activeSession && !bridge.error, 'Late bootstrap or metadata revived a disposed owner');
    check(commands.filter(c => !c.type.startsWith('ui_')).length === commandCount, 'Disposal dispatched fresh metadata');

    await bridge.openSession(projectPath, 'beta');
    await wait(() => bridge.activeSession?.sessionId === 'beta' && bridge.connected);
    connection('alpha', 'disconnected', 'session runtime disposed');
    await delay();
    check(bridge.activeSession?.sessionId === 'beta' && bridge.connected, 'Background disposal cleared the current owner');

    // Both direct metadata and task_list must report genuine current-owner errors.
    for (const type of ['list_models', 'task_list']) {
      bridge.clearError();
      connection('beta', 'connecting');
      await wait(() => !bridge.connected);
      currentFailure = type;
      connection('beta', 'connected');
      await wait(() => bridge.error === `current ${type} failed`);
      currentFailure = undefined;
    }
    bridge.clearError();
    connection('beta', 'connecting');
    await wait(() => !bridge.connected);
    holdCommands = true;
    connection('beta', 'connected');
    await wait(() => pendingCommands.length === 2);
    holdCommands = false;
    await bridge.openSession(projectPath, 'gamma');
    await wait(() => bridge.activeSession?.sessionId === 'gamma');
    for (const reject of pendingCommands.splice(0)) reject(new Error('previous owner failed'));
    await delay(); await delay();
    check(!bridge.error, 'Previous owner metadata error painted the new session');
    root.unmount();
    return { initializationPreserved: true, disposedOwnerCleared: true, staleBootstrapIgnored: true, staleMetadataIgnored: true, backgroundOwnerPreserved: true, currentErrorsVisible: true, navigationErrorsOwned: true };
  },
};
