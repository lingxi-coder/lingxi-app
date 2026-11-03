import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { createRoot } from 'react-dom/client';
import { StrictMode, useState } from 'react';
import type { LocalAppEditRequest } from '../../src/shared/localApps';
import { App } from '../../src/renderer/App';
import { ToolCall } from '../../src/renderer/components/ToolCall';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { ToolRunItem } from '../../src/renderer/model/runItem';
import '../../src/renderer/global.css';

// Test labels are deterministic across developer system languages.
localStorage.setItem(LANGUAGE_STORAGE_KEY, 'en');

const projectPath = '/fixture/Existing project';
const projects = [projectPath];
let ref = { projectPath, sessionId: '11111111-2222-4333-8444-555555555555' };
let selectedFolder: { id: string; path: string; createDirectory?: true } | null = null;
let revision = 0;
const fixture = {
  mode: (location.search.includes('mode=chat') ? 'chat' : 'code') as 'chat' | 'code', switches: [] as string[],
  pendingPublications: location.search.includes('mode=chat') ? [{ requestId: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff', expectedRevision: 0 }] : [],
  publicationRecoveries: [] as string[],
  pendingSessions: location.search.includes('mode=chat') || location.search.includes('recover=1')
    ? [{ requestId: 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff', name: 'Parked app' }] : [],
  sessionRecoveries: [] as string[], newSessionPurposes: [] as unknown[],
  picks: 0, creates: [] as string[], projects,
  nextFolder: null as { id: string; path: string } | null,
  pickerError: '', createError: '', newSessionError: '', holdCreate: false, finishCreate: undefined as (() => void) | undefined,
  bootstrapErrorOnce: false, rendererErrors: [] as string[],
  defaults: 0, defaultError: '', appsRoot: '/fixture/.current-product/.apps', pickerPurposes: [] as string[], newSessions: [] as string[],
  holdDefaults: false, defaultWaiters: [] as (() => void)[],
  requestEdit: undefined as ((request: LocalAppEditRequest) => void) | undefined,
};
window.addEventListener('error', event => fixture.rendererErrors.push(event.message));
window.addEventListener('unhandledrejection', event => fixture.rendererErrors.push(String(event.reason)));
(window as any).projectCreationFixture = fixture;
function snapshot() {
  return {
    revision: ++revision,
    settings: { version: 1, projects: [...projects], activeProject: ref.projectPath, activeSession: ref,
      pinnedSessions: [], archivedSessions: [], theme: location.search.includes('dark') ? 'dark' : 'light' },
    activeSession: ref, workspace: { path: ref.projectPath, trusted: false }, diagnostics: [],
    conversationSpaces: fixtureConversationSpaces({ mode: fixture.mode, projects, selectedProject: ref.projectPath, activeSession: ref }),
    connection: { status: 'connected' },
    runtimes: [{ ...ref, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
    projectCatalogs: Object.fromEntries(projects.map(path => [path, { sessions: [] }])),
    versions: { app: 'fixture', electron: 'fixture' },
  };
}
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: () => () => {}, onComputerAccess: () => () => {},
  onLocalAppEdit: (listener: (request: LocalAppEditRequest) => void) => {
    fixture.requestEdit = listener;
    return () => { if (fixture.requestEdit === listener) fixture.requestEdit = undefined; };
  },
  bootstrap: async () => {
    if (fixture.bootstrapErrorOnce) { fixture.bootstrapErrorOnce = false; throw new Error('Fixture metadata refresh failed'); }
    return snapshot();
  },
  pickProjectFolder: async (purpose: string) => {
    fixture.pickerPurposes.push(purpose);
    fixture.picks++;
    if (fixture.pickerError) throw new Error(fixture.pickerError);
    if (fixture.nextFolder) selectedFolder = fixture.nextFolder;
    return fixture.nextFolder;
  },
  defaultAppProjectFolder: async () => {
    fixture.defaults++;
    if (fixture.holdDefaults) await new Promise<void>(resolve => fixture.defaultWaiters.push(resolve));
    if (fixture.defaultError) throw new Error(fixture.defaultError);
    selectedFolder = { id: `default-${fixture.defaults}`, path: fixture.appsRoot, createDirectory: true };
    return selectedFolder;
  },
  createProject: async (id: string, directoryName?: string) => {
    fixture.creates.push(id);
    if (fixture.createError) throw new Error(fixture.createError);
    if (fixture.holdCreate) await new Promise<void>(resolve => { fixture.finishCreate = resolve; });
    const path = selectedFolder!.createDirectory ? `${selectedFolder!.path}/${directoryName}` : selectedFolder!.path;
    if (!projects.includes(path)) projects.push(path);
    return { path, trusted: false };
  },
  newSession: async (requestedPath: string | null, _model: unknown, purpose: any) => {
    const path = purpose?.kind === 'recover-local-app' ? projectPath : requestedPath;
    if (!path) throw new Error('Fixture session needs a project');
    fixture.newSessionPurposes.push(purpose);
    fixture.newSessions.push(path);
    if (fixture.newSessionError) throw new Error(fixture.newSessionError);
    ref = { projectPath: path, sessionId: `22222222-2222-4333-8444-${String(fixture.newSessions.length).padStart(12, '0')}` };
    return snapshot();
  },
  switchConversationSpace: async (mode: 'chat' | 'code') => { fixture.switches.push(mode); fixture.mode = mode; return snapshot(); },
  localApps: {
    list: async () => [], onChanged: () => () => {},
    pendingPublications: async () => fixture.pendingPublications,
    resumePublication: async (requestId: string) => { fixture.publicationRecoveries.push(requestId); throw new Error('No pending publication'); },
    pendingSourceCopies: async () => [], resumeSourceCopy: async () => { throw new Error('No pending source'); },
    pendingAuthoringSessions: async () => fixture.pendingSessions,
    resumeAuthoringSession: async (requestId: string) => { fixture.sessionRecoveries.push(requestId);
      fixture.requestEdit?.({ id: '', name: 'Parked app', authoringSessionIntentId: requestId }); },
    remoteSettings: async () => ({}), remoteConfiguration: async () => ({ configured: false }), setRemoteAccess: async () => {},
    prepareAuthoring: async (session: typeof ref) => ({ workspaceId: 'fixture-workspace', sourceDirectory: session.projectPath,
      projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId }),
    bindAuthoring: async (appId: string, session: typeof ref) => ({ appId, workspaceId: 'fixture-workspace', sourceDirectory: session.projectPath,
      projectRef: { kind: 'desktopPath', path: session.projectPath }, authoringSessionId: session.sessionId }),
  },
  command: async () => {},
  listProjectSessions: async (path: string) => ({ projectPath: path, sessions: [] }),
};
createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>);

// Exercise the real disclosure and styles in a narrow Electron layout.
(window as any).mountCodeRequiredTool = () => {
  const container = document.createElement('div');
  Object.assign(container.style, { position: 'fixed', top: '10px', left: '10px', width: '220px', zIndex: '100000' });
  container.id = 'code-required-tool-fixture'; document.body.appendChild(container);
  const root = createRoot(container);
  const item: ToolRunItem = { type: 'tool', id: 'code-required', tool: 'LocalApp', status: 'error',
    view: { verb: 'other', label: 'LocalApp', title: 'LocalApp' },
    result: { headline: 'local_app_development_requires_code: Code required', body: 'local_app_development_requires_code: Code required' } };
  function Fixture() {
    const [open, setOpen] = useState(false);
    return <Theme.Provider value={tokens(false)}><ToolCall item={item} open={open} onSetOpen={(_id, value) => setOpen(value)} /></Theme.Provider>;
  }
  root.render(<Fixture />);
  return () => { root.unmount(); container.remove(); };
};
