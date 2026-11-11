import { useBridge } from '../../src/renderer/bridge/useBridge';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { createRuntimeEventReplayBuffer, requestRuntimeEventReplay } from '../../src/preload/event-replay';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import '../../src/renderer/global.css';

const projectPath = '/fixture/cold-restore';
const active = { projectPath, sessionId: 'cold-session', mode: 'code' as const };
const samples: unknown[] = [];
let attempts = 0;
let bridge: ReturnType<typeof useBridge>;
const snapshot = () => ({
  revision: 1, activeSession: active,
  settings: { version: 1, projects: [projectPath], activeProject: projectPath, activeSession: active,
    pinnedSessions: [], archivedSessions: [], theme: 'light' },
  workspace: { path: projectPath, trusted: false }, connection: { status: 'connected' },
  conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: [projectPath], selectedProject: projectPath, activeSession: active }),
  runtimes: [{ ...active, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions: [] } }, providerAccountSnapshot: fixtureAccountSnapshot(['openai']), diagnostics: [],
});
const base = { sessionId: active.sessionId, sequence: 1, event: { type: 'session_resumed', session_id: active.sessionId, mode: 'code',
  messages: [
    { role: 'user', blocks: [{ type: 'text', text: 'Reply briefly, without calling tools.' }] },
    { role: 'assistant', blocks: [{ type: 'text', text: 'Custom provider verified.' }] },
  ] } };
(window as any).lingxi = {
  platform: 'darwin', isElectron: true, bootstrap: async () => snapshot(), settings: async () => snapshot().settings,
  reportSessionRecovery: (sample: unknown) => samples.push(sample),
  onEvent: (listener: (event: any) => void) => {
    const buffer = createRuntimeEventReplayBuffer<any>(listener);
    const stop = requestRuntimeEventReplay<any>(() => {
      if (++attempts === 1 && location.search.includes('reject')) return Promise.reject(new Error('injected replay rejection'));
      if (attempts === 1 && location.search.includes('stall')) return new Promise(() => {});
      return Promise.resolve([base]);
    }, buffer, undefined, sample => samples.push(sample));
    return () => { stop(); buffer.dispose(); };
  },
  onConversationShown: () => () => {}, onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, listProjectSessions: async () => ({ projectPath, sessions: [] }),
  scheduled: { scopes: async () => [], context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
};
Object.assign(window, { coldRestoreFixture: () => ({ ready: bridge?.connected, attempts, samples,
  itemCount: bridge?.conversation.items.length }) });
function Fixture() {
  bridge = useBridge();
  return <Theme.Provider value={tokens(false)}>
    <main className="desktop-main" style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <Stage liveItems={bridge.conversation.items} sessionKey={bridge.conversation.sessionKey} />
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
