import { useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { useBridge } from '../../src/renderer/bridge/useBridge';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const ref = { projectPath: '/fixture', sessionId: 'new-session' };
let saved = false;
let finishSend: (() => void) | undefined;
const oldRows = Array.from({ length: 6 }, (_, i) => ({ uuid: `old-${i}`, title: `Older chat ${i}`, message_count: 2, modified_rfc3339: '2026-09-10T00:00:00Z', mode: 'code', path: '' }));
const list = () => ({ projectPath: ref.projectPath, sessions: saved ? [...oldRows, { ...oldRows[0], uuid: ref.sessionId, title: 'Saved title', message_count: 2 }] : oldRows });
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, listProjectSessions: async () => list(),
  sendPrompt: () => new Promise<void>(resolve => { finishSend = resolve; }),
  bootstrap: async () => ({
    settings: { projects: ['/fixture'], activeProject: '/fixture', activeSession: ref, pinnedSessions: [], archivedSessions: [] },
    activeSession: ref, workspace: { path: '/fixture', trusted: false }, diagnostics: [],
    runtimes: [{ ...ref, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
    projectCatalogs: { '/fixture': { sessions: oldRows } },
  }),
};
function Fixture() {
  const bridge = useBridge();
  useEffect(() => {
    (window as any).firstSessionFixture = {
      ready: !bridge.loading,
      refresh: () => bridge.listProjectSessions('/fixture'),
      persist: () => { saved = true; finishSend?.(); return bridge.listProjectSessions('/fixture'); },
    };
  }, [bridge]);
  return <Theme.Provider value={tokens(false)}><div style={{ display: 'flex', height: '100vh' }}>
    <BetaSidebar bridge={bridge} onOpenSettings={() => {}} />
    <main style={{ padding: 32 }}><button id="send" onClick={() => void bridge.sendPrompt('First message appears immediately')}>Send first message</button><input id="composer" aria-label="Composer" /></main>
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
