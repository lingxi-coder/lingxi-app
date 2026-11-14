import { useState } from 'react';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { createRoot } from 'react-dom/client';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';

setLanguagePreference('en');

const sessions = ['alpha', 'beta', 'gamma'].map((uuid, index) => ({ uuid, title: uuid, modified_rfc3339: `2026-09-${22 - index}T00:00:00Z`, message_count: 1, mode: 'code' as const, path: `/fixture/${uuid}.jsonl` }));
const events = { opened: [] as string[], preferences: [] as unknown[], touched: [] as string[], sessions,
  setLoading: (_loading: boolean) => {} };
Object.assign(window, { sessionGesturesFixture: events });
function Fixture() {
  const [sessionLoading, setSessionLoading] = useState(false);
  events.setLoading = setSessionLoading;
  const [sidebar, setSidebar] = useState({ organization: 'project', chatSort: 'updated', manualSessionOrder: {} });
  const [catalogSessions, setCatalogSessions] = useState(sessions);
  events.sessions = catalogSessions;
  const bridge = {
    sessionLoading, projectCreationOpen: false,
    bootstrap: {
      settings: { projects: ['/fixture'], pinnedSessions: [{ projectPath: '/pinned', sessionId: 'pinned', title: 'pinned' }], activeProject: '/fixture', sidebar },
      workspace: { path: '/fixture' },
      conversationSpaces: fixtureConversationSpaces({
        mode: 'code', projects: ['/fixture'], selectedProject: '/fixture',
        sessions: [
          ...catalogSessions.map(({ uuid }) => ({ projectPath: '/fixture', sessionId: uuid })),
          { projectPath: '/pinned', sessionId: 'pinned', projectId: null },
        ],
      }),
      projectCatalogs: { '/fixture': { sessions: catalogSessions }, '/pinned': { sessions: [{ uuid: 'pinned', title: 'pinned', mode: 'code' }] } },
    },
    sessionRuntimeStatus: () => ({ connection: { status: 'connected' } }), listProjectSessions: async () => ({ sessions: [] }),
    openSession: async (_project: string, id: string) => { events.opened.push(id); },
    updateSidebarPreferences: async (next: typeof sidebar) => { events.preferences.push(next); setSidebar(next); },
    touchSession: async (_project: string, id: string) => {
      events.touched.push(id);
      setCatalogSessions(current => current.map(session => session.uuid === id ? { ...session, modified_rfc3339: '2026-09-23T00:00:00Z' } : session));
    },
  } as unknown as UseBridge;
  return <Theme.Provider value={tokens(false)}>
    <div className="desktop-shell" style={{ display: 'flex', height: '100vh' }}>
      <BetaSidebar bridge={bridge} onOpenSettings={() => {}}>
        <main>Isolated session gesture fixture</main>
      </BetaSidebar>
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
