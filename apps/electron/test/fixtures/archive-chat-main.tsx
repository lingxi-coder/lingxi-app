import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { ArchiveChatDialog } from '../../src/renderer/components/ArchiveChatDialog';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Test labels are deterministic across developer system languages.
localStorage.setItem(LANGUAGE_STORAGE_KEY, 'en');

// UI-only callbacks: no real chat, cron, preload, or engine is accessed.
function Fixture() {
  const sidebarAttempts = useRef(0);
  const nextHeldArchive = useRef<'success' | 'failure' | null>(null);
  const heldArchive = useRef<(() => void) | null>(null);
  const completedHeldArchives = useRef(0);
  const [sidebarCalls, setSidebarCalls] = useState(0);
  const [sidebarSessions, setSidebarSessions] = useState([
    { uuid: 'fixture-session', title: 'Sidebar chat' },
    { uuid: 'fixture-neighbor', title: 'Neighbor chat' },
  ]);
  const [openedSession, setOpenedSession] = useState('');
  const bridge = {
    bootstrap: {
      settings: { projects: ['/fixture'], pinnedSessions: [], activeProject: '/fixture' },
      workspace: { path: '/fixture' },
      scheduledWorkspace: '/scheduled',
      projectCatalogs: {
        '/fixture': { sessions: sidebarSessions.filter((session) => session.uuid === 'fixture-session') },
        '/scheduled': { sessions: sidebarSessions.filter((session) => session.uuid === 'fixture-neighbor') },
      },
    },
    sessionLoading: false, sessionRuntimeStatus: () => undefined, preflightSessionArchive: async () => [],
    openSession: async (_project: string, sessionId: string) => { setOpenedSession(sessionId); },
    archiveSession: async (_project: string, sessionId: string) => {
      setSidebarCalls(value => value + 1);
      const heldOutcome = nextHeldArchive.current;
      if (heldOutcome) {
        nextHeldArchive.current = null;
        await new Promise<void>((resolve) => { heldArchive.current = resolve; });
        completedHeldArchives.current += 1;
        if (heldOutcome === 'failure') throw new Error('Delayed archive A failed');
        setSidebarSessions((sessions) => sessions.filter((session) => session.uuid !== sessionId));
        return;
      }
      if (sidebarAttempts.current++ === 0) throw new Error('Fixture archive failure');
      setSidebarSessions((sessions) => sessions.filter((session) => session.uuid !== sessionId));
    },
  } as unknown as UseBridge;
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [confirms, setConfirms] = useState(0);
  const [closes, setCloses] = useState(0);
  Object.assign(window, { archiveChatFixture: {
    configure: ({ loading = false, busy = false, error = '' } = {}) => {
      setLoading(loading); setBusy(busy); setError(error);
    },
    holdNextSidebarArchive: (outcome: 'success' | 'failure') => { nextHeldArchive.current = outcome; },
    releaseSidebarArchive: () => { heldArchive.current?.(); heldArchive.current = null; },
    resetSidebar: () => {
      sidebarAttempts.current = 0;
      setSidebarCalls(0);
      setOpenedSession('');
      setSidebarSessions([
        { uuid: 'fixture-session', title: 'Sidebar chat' },
        { uuid: 'fixture-neighbor', title: 'Neighbor chat' },
      ]);
    },
    state: () => ({ open, confirms, closes, sidebarCalls, openedSession,
      sidebarArchiveHeld: heldArchive.current !== null, completedHeldArchives: completedHeldArchives.current }),
  } });
  return <div className="desktop-shell" style={{ display: 'flex', position: 'relative', width: '100vw', height: '100vh' }}><BetaSidebar bridge={bridge} onOpenSettings={() => {}} /><main className="desktop-main" style={{ padding: 40, flex: 1, position: 'relative' }}>
    <h1>Project chat</h1><button id="opener" onClick={() => setOpen(true)}>Archive chat</button>
    {open && <ArchiveChatDialog title="Project chat" jobs={[{ id: 'fixture-task', cron: '0 9 * * 1', prompt: '# Weekly project summary\n\nSummarize recent project progress.', recurring: true }]}
      loading={loading} busy={busy} error={error}
      onClose={() => { setCloses((value) => value + 1); setOpen(false); }}
      onConfirm={() => { setConfirms((value) => value + 1); setOpen(false); }} />}
  </main></div>;
}
const palette = tokens(false);
createRoot(document.getElementById('root')!).render(<Theme.Provider value={palette}><Fixture /></Theme.Provider>);
