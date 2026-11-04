import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { ArchiveChatDialog } from '../../src/renderer/components/ArchiveChatDialog';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Test labels are deterministic across developer system languages.
setLanguagePreference('en');

// UI-only callbacks: no real chat, cron, preload, or engine is accessed.
function Fixture() {
  const sidebarAttempts = useRef(0);
  const [sidebarCalls, setSidebarCalls] = useState(0);
  const metadata = useRef({ captures: 0, preflights: [] as string[], archives: [] as string[], renames: [] as string[], releases: [] as string[] });
  const bridge = {
    bootstrap: {
      settings: { projects: ['/fixture'], pinnedSessions: [], activeProject: '/fixture' },
      workspace: { path: '/fixture' },
      conversationSpaces: fixtureConversationSpaces({ mode: 'chat', projects: ['/fixture'], selectedProject: '/fixture' }),
      projectCatalogs: { '/fixture': { sessions: [{ uuid: 'fixture-session', title: 'Sidebar chat', mode: 'chat', path: '/fixture/transcript.jsonl' }] } },
    },
    sessionRuntimeStatus: () => undefined, listProjectSessions: async () => ({ sessions: [] }),
    prepareSessionMetadata: async (_project: string, _id: string, selection: { scope: string; transcriptPath: string }) => {
      metadata.current.captures++;
      return { ...selection, sourceToken: `token-${metadata.current.captures}` };
    },
    releaseSessionMetadata: async (selection: { sourceToken: string }) => { metadata.current.releases.push(selection.sourceToken); },
    preflightSessionArchive: async (_project: string, _id: string, selection: { sourceToken: string }) => { metadata.current.preflights.push(selection.sourceToken); return []; },
    archiveSession: async (_project: string, _id: string, selection: { sourceToken: string }) => {
      metadata.current.archives.push(selection.sourceToken); setSidebarCalls(value => value + 1);
      if (sidebarAttempts.current++ === 0) throw new Error('Fixture archive failure');
    },
    renameSession: async (_project: string, _id: string, _title: string, selection: { sourceToken: string }) => { metadata.current.renames.push(selection.sourceToken); },
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
    state: () => ({ open, confirms, closes, sidebarCalls, metadata: metadata.current }),
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
