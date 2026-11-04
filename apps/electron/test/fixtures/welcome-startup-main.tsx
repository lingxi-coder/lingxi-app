import { fixtureAccountSnapshot } from './provider-accounts-mock';
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
let bootstrapSettled = false, recoveryAtMs = 0, recoveryReads = 0, snapshotRevision = 0, currentSessionId = 'initial-session';
const accountSnapshot = () => {
  const value = fixtureAccountSnapshot(['openai-chatgpt']);
  if (Date.now() < recoveryAtMs) {
    value.health = value.health.map(row => ({ ...row, status: 'cooling_down', cooldownUntilMs: recoveryAtMs }));
    value.modelAvailability = value.modelAvailability?.map(row => ({ ...row, eligibleAccountIds: [] }));
  }
  return value;
};
const snapshot = (sessionId: string, revision: number) => ({
  revision,
  settings: { projects: [projectPath], activeProject: projectPath, activeSession: { projectPath, sessionId }, pinnedSessions: [], archivedSessions: [], theme: 'light' },
  activeSession: { projectPath, sessionId },
  workspace: { path: projectPath, trusted: true },
  connection: { status: 'connected' },
  runtimes: [{ projectPath, sessionId, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions: [] } },
  providerAccountSnapshot: accountSnapshot(),
  diagnostics: [],
  versions: { app: 'fixture', electron: 'fixture' },
});
const fixture = {
  newSessionPending: false,
  finishBootstrap: () => { recoveryAtMs = Date.now() + 1200; bootstrapSettled = true; resolveBootstrap(snapshot(currentSessionId, ++snapshotRevision)); },
  recoveryState: () => ({ recoveryAtMs, recoveryReads, currentSessionId }),
  finishNewSession: () => { currentSessionId = 'new-session'; resolveNewSession(snapshot(currentSessionId, ++snapshotRevision)); },
  failNewSession: () => rejectNewSession(new Error('fixture engine startup failed')),
  sent,
};
(window as any).welcomeStartupFixture = fixture;
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: () => { if (!bootstrapSettled) return bootstrapPromise; recoveryReads++; return Promise.resolve(snapshot(currentSessionId, ++snapshotRevision)); },
  newSession: () => { fixture.newSessionPending = true; return new Promise((resolve, reject) => { resolveNewSession = resolve; rejectNewSession = reject; }); },
  command: async () => {},
  listProjectSessions: async () => ({ projectPath, sessions: [] }),
  sendPrompt: async (...args: unknown[]) => { sent.push(args); },
};
createRoot(document.getElementById('root')!).render(<App />);
