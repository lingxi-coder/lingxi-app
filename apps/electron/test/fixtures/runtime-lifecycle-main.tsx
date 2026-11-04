import { createRoot } from 'react-dom/client';
import { useBridge } from '../../src/renderer/bridge/useBridge';
import { defaultNativeAudioSnapshot } from '../../src/shared/nativeAudio';

const projectPath = '/runtime-lifecycle';
const chatProjectPath = '/chat-only';
const codeProjectPath = '/code-only';
const snapshot = (sessionId: string, revision: number, mode: 'chat' | 'code' = 'chat') => ({
  revision,
  settings: { projects: [projectPath, chatProjectPath, codeProjectPath], activeProject: projectPath, activeSession: { projectPath, projectId: projectPath, sessionId, mode }, pinnedSessions: [], archivedSessions: [] },
  activeSession: { projectPath, projectId: projectPath, sessionId, mode }, workspace: { path: projectPath, trusted: true },
  connection: { status: 'connected' }, diagnostics: [], projectCatalogs: {},
  runtimes: [{ projectPath, projectId: projectPath, sessionId, mode, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  conversationSpaces: {
    activeMode: mode, globalWorkspaces: { chat: '/global/chat', code: '/global/code' }, projects: { [projectPath]: ['chat', 'code'], [chatProjectPath]: ['chat'], [codeProjectPath]: ['code'] },
    navigation: { chat: { projectId: null }, code: { projectId: null } },
    sessions: [{ projectPath, projectId: projectPath, sessionId, mode }],
  },
});
let state = snapshot('alpha', 1);
let bridge: ReturnType<typeof useBridge>;
let onState: (event: unknown) => void;
let onAudioState: (event: unknown) => void;
const prompts: string[] = [];
let heldBootstrap: ((value: unknown) => void) | undefined;
let holdBootstrap = true;
let holdCommands = false;
const pendingCommands: Array<(error: Error) => void> = [];
let finishOldAttach: (() => void) | undefined;
const commands: Array<{ sessionId: string; type: string }> = [];
let currentFailure: string | undefined;
let failNextBootstrap = false;
let switchAttempts = 0;
let scheduledScopesRequests = 0;
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  onConnectionStateChanged: (listener: typeof onState) => { onState = listener; return () => {}; },
  audio: {
    request: async () => ({ snapshot: defaultNativeAudioSnapshot() }),
    onRealtimeState: (listener: typeof onAudioState) => { onAudioState = listener; return () => {}; },
    onEvent: () => () => {},
  },
  scheduled: {
    scopes: async () => {
      scheduledScopesRequests++;
      return [
        { id: 'global', label: 'No project' },
        ...(state.conversationSpaces.activeMode === 'chat'
          ? [{ id: chatProjectPath, projectPath: chatProjectPath, label: 'Chat' }]
          : [{ id: codeProjectPath, projectPath: codeProjectPath, label: 'Code' }]),
      ];
    },
    context: async () => ({ models: [], currentModel: '', sessions: [] }),
  },
  bootstrap: async () => {
    if (failNextBootstrap) { failNextBootstrap = false; throw new Error('bootstrap unavailable'); }
    return holdBootstrap ? new Promise(resolve => { heldBootstrap = resolve; }) : state;
  },
  switchConversationSpace: async (mode: 'chat' | 'code') => {
    switchAttempts++;
    if (switchAttempts <= 2) { failNextBootstrap = true; throw new Error('switch unavailable'); }
    state = snapshot('code-target', state.revision + 1, mode);
    return state;
  },
  openSession: async (_path: string, sessionId: string) => { state = snapshot(sessionId, state.revision + 1); return state; },
  sendPrompt: async (sessionId: string) => { prompts.push(sessionId); },
  command: async (sessionId: string, command: { type: string }) => {
    commands.push({ sessionId, type: command.type });
    if (sessionId === 'alpha' && command.type === 'ui_attach') {
      return new Promise<void>(resolve => { finishOldAttach = resolve; });
    }
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
    await wait(() => bridge?.connected && commands.some(c => c.type === 'task_list') && !!finishOldAttach);
    check(bridge.activeSession?.sessionId === 'alpha', 'Background disposal discarded the only initial snapshot');
    connection('alpha', 'connecting');
    await wait(() => !bridge.connected);
    holdCommands = true;
    holdBootstrap = true;
    connection('alpha', 'connected');
    await wait(() => pendingCommands.length === 2 && !!heldBootstrap);
    const staleSnapshot = state;
    const disposalCommandIndex = commands.length;
    connection('alpha', 'disconnected', 'session runtime disposed');
    await delay();
    check(!commands.slice(disposalCommandIndex).some(c => c.sessionId === 'alpha'), 'Disposal dispatched a command to the removed owner');
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
    finishOldAttach!();
    await delay(); await delay();
    check(!commands.slice(disposalCommandIndex).some(c => c.sessionId === 'alpha'), 'A queued command revived an owner after a new bootstrap');
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
    const navigation = bridge.openSession(projectPath, 'gamma');
    const admitted = bridge.sendTrackedPrompt('admitted voice', [], [], [], { purpose: 'flow', sessionId: 'beta' });
    check(admitted, 'Navigation dropped an already admitted flow input before React changed the selection');
    check(!bridge.sendTrackedPrompt('ordinary input'), 'Navigation allowed a new ordinary input during the session transition');
    await admitted!.queued;
    check(prompts.at(-1) === 'beta', 'An admitted flow input reached the wrong navigation owner');
    await navigation;
    await wait(() => bridge.activeSession?.sessionId === 'gamma');
    for (const reject of pendingCommands.splice(0)) reject(new Error('previous owner failed'));
    await delay(); await delay();
    check(!bridge.error, 'Previous owner metadata error painted the new session');

    await wait(() => commands.some(c => c.sessionId === 'gamma' && c.type === 'ui_attach'));
    const replacementCommandIndex = commands.length;
    connection('gamma', 'disconnected', 'session runtime replaced');
    await wait(() => !bridge.connected);
    await delay(); await delay();
    check(bridge.activeSession?.sessionId === 'gamma', 'Replacement discarded the session identity');
    check(!commands.slice(replacementCommandIndex).some(c => c.sessionId === 'gamma' && c.type.startsWith('ui_')), 'Replacement dispatched a surface command during the runtime gap');
    state = snapshot('gamma', state.revision + 1);
    connection('gamma', 'connected');
    await wait(() => bridge.connected && commands.slice(replacementCommandIndex).some(c => c.sessionId === 'gamma' && c.type === 'ui_attach'));
    await wait(() => scheduledScopesRequests > 0 && bridge.scheduledScopes.some(scope => scope.id === chatProjectPath));
    check(!bridge.scheduledScopes.some(scope => scope.id === codeProjectPath), 'Scheduled scopes included another conversation space');
    const previousScheduledContext = bridge.scheduledContext;
    try { await bridge.switchConversationSpace('code'); throw new Error('Expected switch failure'); }
    catch (error) { check((error as Error).message === 'switch unavailable', 'Switch did not preserve the original error'); }
    await wait(() => !bridge.sessionLoading && bridge.bootstrap?.conversationSpaces.activeMode === 'chat');
    check(bridge.activeSession?.sessionId === 'gamma', 'Failed switch changed the confirmed session');
    try { await bridge.switchConversationSpace('code'); throw new Error('Expected second switch failure'); }
    catch (error) { check((error as Error).message === 'switch unavailable', 'Second switch did not preserve the original error'); }
    // Retry in the same turn, before React needs to render the restored tab.
    const retry = bridge.switchConversationSpace('code');
    check(switchAttempts === 3, 'Immediate retry was suppressed after recovery also failed');
    await retry;
    await wait(() => bridge.bootstrap?.conversationSpaces.activeMode === 'code' && bridge.activeSession?.sessionId === 'code-target');
    await wait(() => bridge.scheduledScopes.some(scope => scope.id === codeProjectPath));
    check(!bridge.scheduledScopes.some(scope => scope.id === chatProjectPath), 'Scheduled scopes retained the previous conversation space');
    check(bridge.scheduledContext !== previousScheduledContext, 'Scheduled editor context did not refresh for the new space');
    onAudioState({ sessionId: 'gamma', generation: 1, phase: 'thinking', detail: 'background audio' });
    await delay();
    check(bridge.audioRealtimeState.phase === 'paused', 'Background audio state appeared on another session');
    const tracked = bridge.sendTrackedPrompt('voice input', [], [], [], { purpose: 'flow', sessionId: 'gamma' });
    check(tracked, 'Background flow input was not accepted for its captured owner');
    await tracked!.queued;
    check(prompts.at(-1) === 'gamma', 'Background flow input reached the selected session');
    await bridge.openSession(projectPath, 'gamma');
    await wait(() => bridge.activeSession?.sessionId === 'gamma' && bridge.audioRealtimeState.phase === 'thinking');
    check(bridge.audioRealtimeState.detail === 'background audio', 'Returning to the audio owner lost its state');
    root.unmount();
    return { initializationPreserved: true, disposedOwnerCleared: true, staleBootstrapIgnored: true, staleMetadataIgnored: true, backgroundOwnerPreserved: true, currentErrorsVisible: true, navigationErrorsOwned: true, replacementGapSkipped: true, replacementReattached: true, failedSpaceSwitchRetry: true, scheduledSpaceRefreshed: true, audioOwnerPreserved: true, admittedFlowDuringNavigation: true };
  },
};
