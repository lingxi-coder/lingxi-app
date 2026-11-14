import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { useBridge } from '../../src/renderer/bridge/useBridge';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes';
import { restoreSessionPickerFocus, SessionPicker } from '../../src/renderer/components/SessionPicker';
import type { SessionCatalogRow, SessionOpenSelection } from '../../src/shared/sessionWorkspace';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';
import '../../src/renderer/global.css';
import '../../src/renderer/components/SessionPicker.css';

localStorage.setItem(LANGUAGE_STORAGE_KEY, 'en');
const projectPath = '/fixture/session-optimization';
const id = (index: number) => `00000000-0000-4000-8000-${index.toString(16).padStart(12, '0')}`;
let identityReads = 0;
const rows: SessionCatalogRow[] = Array.from({ length: 5000 }, (_, index) => ({
  get uuid() { identityReads++; return id(index); },
  title: `History ${String(index).padStart(5, '0')}`, path: `/fixture/history/${index}.jsonl`,
  modified_rfc3339: new Date(Date.UTC(2026, 10, 10) - index * 1000).toISOString(),
  message_count: index ? 4 : 0, mode: 'code',
}));
const duplicateId = id(6000);
const chosenPath = '/fixture/Exact Case/中文 chosen copy.jsonl';
rows.push(
  { ...rows[4999]!, uuid: duplicateId, title: 'Duplicate older source', path: '/fixture/other copy.jsonl', modified_rfc3339: '2025-01-02T00:00:00Z' },
  { ...rows[4999]!, uuid: duplicateId, title: 'Duplicate chosen source', path: chosenPath, modified_rfc3339: '2025-01-01T00:00:00Z' },
  ...Array.from({ length: 100 }, (_, index) => ({ ...rows[4999]!, uuid: id(7000 + index), mode: 'chat' as const, title: 'Other mode' })),
);
let active = { projectPath, sessionId: id(0), mode: 'code' as const };
let revision = 1;
let catalogs = { [projectPath]: { sessions: rows } };
const eventListeners = new Set<(event: unknown) => void>();
const shownListeners = new Set<(snapshot: unknown) => void>();
const opened: Array<{ projectPath: string; sessionId: string; selection?: SessionOpenSelection }> = [];
let deferOpening = false;
const pendingOpens = new Map<string, () => void>();
let bridge: UseBridge;
let catalogChanges = 0;
let renders = 0;
let previousCatalogs: unknown;

function snapshot() {
  return {
    revision, activeSession: active,
    settings: { version: 1, projects: [projectPath], activeProject: projectPath, activeSession: active,
      pinnedSessions: [], archivedSessions: [], theme: 'light' },
    workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
    conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: [projectPath], selectedProject: projectPath, activeSession: active }),
    runtimes: [{ ...active, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
    projectCatalogs: catalogs, providerAccountSnapshot: fixtureAccountSnapshot(['openai']), diagnostics: [],
  };
}

(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  bootstrap: async () => snapshot(), settings: async () => snapshot().settings,
  onEvent: (listener: (event: unknown) => void) => { eventListeners.add(listener); return () => eventListeners.delete(listener); },
  onConversationShown: (listener: (snapshot: unknown) => void) => { shownListeners.add(listener); return () => shownListeners.delete(listener); },
  onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, sendPrompt: async () => {},
  listProjectSessions: async () => ({ projectPath, ...catalogs[projectPath] }),
  openSession: async (project: string, sessionId: string, selection?: SessionOpenSelection) => {
    if (!rows.some(row => row.uuid === sessionId && row.path === selection?.transcriptPath && row.mode === 'code')) throw new Error('Exact source and mode required');
    opened.push({ projectPath: project, sessionId, selection });
    if (deferOpening) await new Promise<void>(resolve => pendingOpens.set(sessionId, resolve));
    active = { projectPath, sessionId, mode: 'code' }; revision++;
    return snapshot();
  },
  scheduled: { scopes: async () => [], context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
};

Object.assign(window, { sessionOptimizationFixture: {
  state: () => ({ ready: Boolean(bridge?.connected), renders, catalogChanges, identityReads, opened, duplicateId, chosenPath,
    title: bridge?.bootstrap?.projectCatalogs[projectPath]?.sessions[0]?.title,
    activeSessionId: bridge?.activeSession?.sessionId, sessionLoading: bridge?.sessionLoading, pending: [...pendingOpens.keys()] }),
  openDeferred: (index: number) => {
    deferOpening = true;
    void bridge.openSession(projectPath, id(index), { scope: 'current', transcriptPath: rows[index]!.path });
  },
  completeOpen: (index: number) => { pendingOpens.get(id(index))?.(); pendingOpens.delete(id(index)); },
  submit: () => bridge.sendPrompt('Pending catalog title'),
  stream: async (count: number) => {
    for (let index = 0; index < count; index++) {
      for (const listener of eventListeners) listener({ sessionId: active.sessionId, sequence: index, event: { type: 'text_delta', text: 'x' } });
      await new Promise(requestAnimationFrame);
    }
  },
  save: () => {
    catalogs = { [projectPath]: { sessions: [{ ...rows[0]!, title: 'Authoritative saved title', message_count: 7 }, ...rows.slice(1)] } };
    revision++;
    for (const listener of shownListeners) listener(snapshot());
  },
} });

function Fixture() {
  bridge = useBridge();
  const [open, setOpen] = useState(false);
  const opener = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    renders++;
    if (bridge.bootstrap?.projectCatalogs && bridge.bootstrap.projectCatalogs !== previousCatalogs) {
      previousCatalogs = bridge.bootstrap.projectCatalogs;
      catalogChanges++;
    }
  });
  return <Theme.Provider value={tokens(false)}>
    <button ref={opener} id="open-picker" onClick={() => setOpen(true)}>Open picker</button>
    {open && <SessionPicker projectPath={projectPath} mode="code" listProjectSessions={bridge.listProjectSessions}
      openSession={bridge.openSession} onOpened={() => {}} onClose={() => { setOpen(false); restoreSessionPickerFocus(opener.current, null); }} />}
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
