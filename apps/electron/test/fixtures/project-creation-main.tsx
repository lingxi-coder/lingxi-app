import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import '../../src/renderer/global.css';

const projectPath = '/fixture/Existing project';
const projects = [projectPath];
const ref = { projectPath, sessionId: '11111111-2222-4333-8444-555555555555' };
let revision = 0;
const fixture = {
  picks: 0, creates: [] as string[], projects,
  nextFolder: null as { id: string; path: string } | null,
  pickerError: '', createError: '', holdCreate: false, finishCreate: undefined as (() => void) | undefined,
  bootstrapErrorOnce: false, rendererErrors: [] as string[],
};
window.addEventListener('error', event => fixture.rendererErrors.push(event.message));
window.addEventListener('unhandledrejection', event => fixture.rendererErrors.push(String(event.reason)));
(window as any).projectCreationFixture = fixture;
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: async () => {
    if (fixture.bootstrapErrorOnce) { fixture.bootstrapErrorOnce = false; throw new Error('Fixture metadata refresh failed'); }
    return {
    revision: ++revision,
    settings: { version: 1, projects: [...projects], activeProject: projectPath, activeSession: ref,
      pinnedSessions: [], archivedSessions: [], theme: location.search.includes('dark') ? 'dark' : 'light' },
    activeSession: ref, workspace: { path: projectPath, trusted: false }, diagnostics: [],
    connection: { status: 'connected' },
    runtimes: [{ ...ref, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
    projectCatalogs: Object.fromEntries(projects.map(path => [path, { sessions: [] }])),
    versions: { app: 'fixture', electron: 'fixture' },
    };
  },
  providerCredentials: async () => [],
  pickProjectFolder: async () => {
    fixture.picks++;
    if (fixture.pickerError) throw new Error(fixture.pickerError);
    return fixture.nextFolder;
  },
  createProject: async (id: string) => {
    fixture.creates.push(id);
    if (fixture.createError) throw new Error(fixture.createError);
    if (fixture.holdCreate) await new Promise<void>(resolve => { fixture.finishCreate = resolve; });
    projects.push(fixture.nextFolder!.path);
    return { path: fixture.nextFolder!.path, trusted: false };
  },
  command: async () => {},
  listProjectSessions: async (path: string) => ({ projectPath: path, sessions: [] }),
};
createRoot(document.getElementById('root')!).render(<App />);
