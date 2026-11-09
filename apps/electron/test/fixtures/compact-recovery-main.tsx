import { createRoot } from 'react-dom/client';
import { useBridge } from '../../src/renderer/bridge/useBridge';
import { BetaComposer } from '../../src/renderer/components/BetaDesktop';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { defaultNativeAudioSnapshot } from '../../src/shared/nativeAudio';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import '../../src/renderer/global.css';

setLanguagePreference('en');
const projectPath = '/compact-recovery';
const sessionId = '11111111-2222-4333-8444-555555555555';
const session = { projectPath, projectId: projectPath, sessionId, mode: 'code' as const };
let revision = 1;
const snapshot = () => ({
  revision,
  settings: { projects: [projectPath], activeProject: projectPath, activeSession: session, pinnedSessions: [], archivedSessions: [] },
  activeSession: session, workspace: { path: projectPath, trusted: true },
  connection: { status: 'connected' }, diagnostics: [], projectCatalogs: {},
  runtimes: [{ ...session, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: [projectPath], selectedProject: projectPath, activeSession: session }),
  providerAccountSnapshot: fixtureAccountSnapshot(['openai']),
});
let bridge: ReturnType<typeof useBridge>;
let emitEvent: (event: unknown) => void;
let emitState: (event: unknown) => void;
const prompts: Array<{ sessionId: string; text: string }> = [];
const commands: Array<{ sessionId: string; type: string }> = [];
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  bootstrap: async () => snapshot(),
  onEvent: (listener: typeof emitEvent) => { emitEvent = listener; return () => {}; },
  onConnectionStateChanged: (listener: typeof emitState) => { emitState = listener; return () => {}; },
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  sendPrompt: async (owner: string, text: string) => { prompts.push({ sessionId: owner, text }); },
  command: async (owner: string, command: { type: string }) => { commands.push({ sessionId: owner, type: command.type }); },
  audio: {
    request: async () => ({ snapshot: defaultNativeAudioSnapshot() }),
    onRealtimeState: () => () => {}, onEvent: () => () => {},
  },
};
function Fixture() {
  bridge = useBridge();
  const workspace = bridge.bootstrap?.workspace;
  const providerConfigured = bridge.bootstrap?.providerAccountSnapshot?.modelAvailability.some(model => model.eligibleAccountIds.length > 0);
  const ready = Boolean(!bridge.loading && bridge.hosted && workspace?.path && workspace.trusted
    && providerConfigured && bridge.connected && !bridge.sessionLoading);
  return <Theme.Provider value={tokens(false)}>
    <BetaComposer bridge={bridge} ready={ready} onOpenSettings={() => {}} onOpenSettingsPage={() => {}}
      onSetTheme={() => {}} onOpenProviderSettings={() => {}} />
  </Theme.Provider>;
}
const root = createRoot(document.getElementById('root')!);
root.render(<Fixture />);
const delay = () => new Promise(resolve => setTimeout(resolve, 20));
async function wait(predicate: () => unknown) {
  for (let i = 0; i < 200; i++) { if (predicate()) return; await delay(); }
  throw new Error(`Compact recovery fixture did not settle: ${document.body.innerText}`);
}
function check(value: unknown, message: string) { if (!value) throw new Error(message); }
const connection = (owner: string, status: string) => emitState({ sessionId: owner, event: { status } });
const event = (value: unknown) => emitEvent({ sessionId, event: value });
const sendButton = () => document.querySelector<HTMLButtonElement>('.composer-send-presence .composer-submit-button');
const draft = 'Synthetic draft retained after compact failure';
// Reuse the hidden Electron runtime fixture driver; no real host or provider is contacted.
(window as any).runtimeLifecycleFixture = {
  run: async () => {
    await wait(() => bridge?.connected && !bridge.loading && !!document.querySelector('.beta-rich-prompt'));
    event({ type: 'text_delta', text: 'Retained conversation history' });
    await wait(() => bridge.conversation.items.some(item => item.type === 'narration' && item.text === 'Retained conversation history'));
    const historyId = bridge.conversation.items[0].id;
    const editor = document.querySelector<HTMLElement>('.beta-rich-prompt')!;
    editor.textContent = draft;
    editor.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: draft }));
    await wait(() => sendButton() && !sendButton()!.disabled);

    await bridge.forceCompact();
    await wait(() => bridge.conversation.activeCompactionId);
    const compactId = bridge.conversation.activeCompactionId;
    connection('another-session', 'disconnected');
    await delay();
    check(bridge.connected && bridge.conversation.activeCompactionId === compactId, 'A background disconnect affected the selected compact');

    connection(sessionId, 'disconnected');
    await wait(() => !bridge.connected && !bridge.conversation.activeCompactionId && sendButton()?.disabled);
    check(!bridge.running && !bridge.conversation.pendingSlashName, 'Compact failure retained a foreground claim');
    check(bridge.conversation.items.some(item => item.type === 'compaction' && item.status === 'error'), 'Connection loss did not settle the compact row');
    check(editor.textContent === draft, 'Disconnect discarded the composer draft');
    check(bridge.conversation.items.some(item => item.id === historyId), 'Disconnect discarded the conversation history');
    check(prompts.length === 0, 'Disconnect submitted the draft');

    revision++;
    connection(sessionId, 'connected');
    await wait(() => bridge.connected && !sendButton()?.disabled);
    check(editor.textContent === draft, 'Reconnect discarded the composer draft');
    check(bridge.conversation.items.some(item => item.id === historyId), 'Reconnect replaced the conversation history');
    check(prompts.length === 0, 'Reconnect replayed the draft');
    check(commands.filter(command => command.type === 'force_compact').length === 1, 'Reconnect replayed compact');
    sendButton()!.click();
    await wait(() => prompts.length === 1);
    check(prompts[0].sessionId === sessionId && prompts[0].text === draft, 'Explicit Send did not reach the recovered session');
    await wait(() => editor.textContent === '');
    root.unmount();
    return { draftPreserved: true, historyPreserved: true, backgroundIsolated: true, compactSettled: true, sendReenabled: true, noAutomaticReplay: true, explicitSendDeliveredOnce: true };
  },
};
