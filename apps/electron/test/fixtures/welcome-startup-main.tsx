import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import '../../src/renderer/global.css';

// Exercise the real App/useBridge lifecycle while the main-process requests
// remain unresolved, without requiring credentials or a running engine.
const projectPath = '/fixture/LingXi-Next';
let resolveBootstrap: (value: unknown) => void;
let resolveNewSession: (value: unknown) => void;
let rejectNewSession: (reason: Error) => void;
const bootstrapPromise = new Promise((resolve) => { resolveBootstrap = resolve; });
const sent: unknown[] = [];
const snapshot = (sessionId: string, revision: number) => ({
  revision,
  settings: { projects: [projectPath], activeProject: projectPath, activeSession: { projectPath, sessionId }, pinnedSessions: [], archivedSessions: [], theme: 'light' },
  activeSession: { projectPath, sessionId },
  workspace: { path: projectPath, trusted: true },
  connection: { status: 'connected' },
  runtimes: [{ projectPath, sessionId, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions: [] } },
  providerCredentials: [{ providerId: 'openai', configured: true }],
  diagnostics: [],
  versions: { app: 'fixture', electron: 'fixture' },
});
const fixture = {
  newSessionPending: false,
  finishBootstrap: () => resolveBootstrap(snapshot('initial-session', 1)),
  finishNewSession: () => resolveNewSession(snapshot('new-session', 2)),
  failNewSession: () => rejectNewSession(new Error('fixture engine startup failed')),
  sent,
};
(window as any).welcomeStartupFixture = fixture;
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: () => bootstrapPromise,
  newSession: () => { fixture.newSessionPending = true; return new Promise((resolve, reject) => { resolveNewSession = resolve; rejectNewSession = reject; }); },
  command: async () => {},
  listProjectSessions: async () => ({ projectPath, sessions: [] }),
  sendPrompt: async (...args: unknown[]) => { sent.push(args); },
};
createRoot(document.getElementById('root')!).render(<App />);
