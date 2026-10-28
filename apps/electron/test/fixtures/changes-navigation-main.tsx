import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import type { GitApi, GitScope, GitStatus } from '../../src/shared/git';
import '../../src/renderer/global.css';

const params = new URLSearchParams(location.search);
const scenario = params.get('scenario');
const projectPath = '/fixture/Changes';
const activeSession = scenario === 'draft' || scenario === 'empty' ? undefined : { projectPath, sessionId: 'session-a' };
const eventListeners = new Set<(event: unknown) => void>();
const changedListeners = new Set<() => void>();
const scopes: GitScope[] = [];
let files = [{ path: 'example.ts', index: ' ', working: 'M', untracked: false, conflict: false, additions: 1, deletions: 1, binary: false }];
const git: GitApi = {
  onChanged(callback) { const notify = () => callback({ root: projectPath }); changedListeners.add(notify); return () => { changedListeners.delete(notify); }; },
  async request(scope, request) {
    scopes.push(scope);
    if (request.kind === 'status') return { status: {
      available: true, repository: scenario !== 'non-repository', root: projectPath, gitDir: `${projectPath}/.git`, commonDir: `${projectPath}/.git`,
      branch: 'main', head: 'abc123', ahead: 0, behind: 0, files, token: String(files.length), busy: false, merging: false, mergeOwned: false, remotes: [],
    } satisfies GitStatus };
    if (request.kind === 'branches') return { branches: [{ name: 'main', current: true, remote: false }] };
    if (request.kind === 'diff') return { diff: { files, binary: false, truncated: false, token: String(files.length), patch: files.length ? 'diff --git a/example.ts b/example.ts\n--- a/example.ts\n+++ b/example.ts\n@@ -1 +1 @@\n-before\n+after\n' : '' } };
    throw new Error(`Unexpected mutating Git request: ${request.kind}`);
  },
};
(window as any).changesFixture = {
  scopes,
  clean() { files = []; changedListeners.forEach(notify => notify()); },
  showSideChat() { eventListeners.forEach(listener => listener({ sessionId: activeSession?.sessionId, sequence: 1, event: { type: 'side_chat_snapshot', side_id: 'side-a', events_json: [] } })); },
};
(window as any).lingxi = {
  platform: 'darwin', isElectron: true, git,
  onEvent: (listener: (event: unknown) => void) => { eventListeners.add(listener); return () => { eventListeners.delete(listener); }; },
  onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, listProjectSessions: async () => ({ projectPath, sessions: [] }),
  bootstrap: async () => ({
    revision: 1,
    settings: { version: 1, projects: scenario === 'empty' ? [] : [projectPath], activeProject: scenario === 'empty' ? undefined : projectPath, activeSession, pinnedSessions: [], archivedSessions: [], theme: params.get('theme') ?? 'light' },
    activeSession, workspace: { path: scenario === 'empty' ? undefined : projectPath, trusted: scenario !== 'empty' }, connection: { status: 'connected' },
    runtimes: activeSession ? [{ ...activeSession, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }] : [],
    projectCatalogs: { [projectPath]: { sessions: [] } }, diagnostics: [], providerCredentials: [{ providerId: 'openai', configured: true }],
  }),
  scheduled: { scopes: async () => [], context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
};
createRoot(document.getElementById('root')!).render(<App />);
