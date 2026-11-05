import { createRoot } from 'react-dom/client';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import '../../src/renderer/global.css';

setLanguagePreference('en');

// UI-only fixture: never connects to an engine or a real session.
const sessions = [
  { uuid: 'pinned', title: 'Pinned agent' },
  { uuid: 'short', title: 'Short' },
  { uuid: 'long', title: 'A very long running session title that must truncate before its progress bar' },
  { uuid: 'pending', title: 'Waiting for input' },
  { uuid: 'error', title: 'Session error' },
  { uuid: 'idle', title: 'Idle session' },
].map((session) => ({ ...session, mode: 'code' as const, path: `/fixture/${session.uuid}.jsonl` }));
let finishOpen: (() => void) | undefined;
Object.assign(window, { sidebarProgressFixture: { finishOpen: () => finishOpen?.() } });
const bridge = {
  bootstrap: {
    settings: { projects: ['/fixture'], pinnedSessions: [{ projectPath: '/fixture', sessionId: 'pinned', title: 'Pinned agent' }], activeProject: '/fixture' },
    conversationSpaces: fixtureConversationSpaces({
      mode: 'code', projects: ['/fixture'], selectedProject: '/fixture',
      sessions: sessions.map(({ uuid }) => ({ projectPath: '/fixture', sessionId: uuid })),
    }),
    workspace: { path: '/fixture' }, projectCatalogs: { '/fixture': { sessions } },
  },
  sessionRuntimeStatus: (id: string) => ({
    connection: { status: id === 'error' ? 'error' : 'connected' },
    turnActive: id === 'short', backgroundAgentsRunning: id === 'long' || id === 'pinned',
    pendingInteractions: id === 'pending' ? 1 : 0,
  }),
  openSession: () => new Promise<void>(resolve => { finishOpen = resolve; }),
  listProjectSessions: async () => ({ sessions: [] }),
} as unknown as UseBridge;
createRoot(document.getElementById('root')!).render(<Theme.Provider value={tokens(false)}><div className="desktop-shell" style={{ display: 'flex', height: '100vh' }}><BetaSidebar bridge={bridge} onOpenSettings={() => {}} /><main style={{ padding: 32 }}>Isolated sidebar progress fixture</main></div></Theme.Provider>);
