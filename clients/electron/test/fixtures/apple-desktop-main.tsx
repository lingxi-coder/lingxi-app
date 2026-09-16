import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import '../../src/renderer/global.css';

// Real renderer, entirely synthetic bridge. No preload, credentials, or engine.
const projectPath = '/fixture/LingXi-Next';
const theme = new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light';
const activeSession = { projectPath, sessionId: 'draft' };
const sessions = ['Refine desktop experience', 'Review workspace changes', 'Improve session navigation'].map((title, index) => ({
  uuid: `session-${index}`, title, message_count: 4, modified_rfc3339: '2026-09-15T12:00:00Z', mode: 'code', path: '',
}));
const snapshot = {
  revision: 1,
  settings: { version: 1, projects: [projectPath, '/fixture/Design-System'], activeProject: projectPath, activeSession, pinnedSessions: [{ projectPath, sessionId: 'session-0' }], archivedSessions: [], theme },
  activeSession, workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
  runtimes: [{ ...activeSession, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions }, '/fixture/Design-System': { sessions: [] } },
  providerCredentials: [{ providerId: 'openai', configured: true }], diagnostics: [], versions: { app: '0.1.0', electron: '43.1.1' },
};
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: async () => snapshot, command: async () => {},
  preflightSessionArchive: async () => [],
  listProjectSessions: async (path: string) => ({ projectPath: path, sessions: path === projectPath ? sessions : [] }),
};
createRoot(document.getElementById('root')!).render(<App />);
