import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import { SESSION_PICKER_EVENT } from '../../src/shared/sessionWorkspace';
import '../../src/renderer/global.css';

// Real App, hook, composer and dialog; the host is entirely synthetic.
const refs = [
  { projectPath: '/fixture/project-A', sessionId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' },
  { projectPath: '/fixture/project-B', sessionId: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb' },
];
const rows = refs.map((ref, index) => ({
  uuid: ref.sessionId, title: `Chat ${index ? 'B' : 'A'}`, resume_title: `Chat ${index ? 'B' : 'A'}`,
  path: `/fixture/transcripts/${ref.sessionId}.jsonl`, mode: 'code', message_count: 4,
  modified_rfc3339: '2026-10-21T00:00:00Z',
}));
let active = refs[0]!;
let revision = 1;
let deferNextCommand = false;
let finishCommand: (() => void) | undefined;
let completedCommands = 0;
let deferNextResume = false;
let pendingResume: { sessionId: string; approved: boolean; finish(): void } | undefined;
const permissionListeners = new Set<(envelope: unknown) => void>();
const applicationMenuListeners = new Set<(action: string) => void>();
const approvalCalls: Array<{ sessionId: string; requestId: number; response: unknown }> = [];
const promptCalls: Array<{ sessionId: string; text: string }> = [];
const navigationCalls: string[] = [];
const admissions: Array<{ sessionId: string; text: string }> = [];
const pickerEvents: Array<{ projectPath: string; query: string }> = [];
const listCalls: Array<{ projectPath: string; scope: string }> = [];
const openCalls: Array<{ projectPath: string; sessionId: string }> = [];

function snapshot() {
  return {
    revision, activeSession: active,
    settings: { version: 1, projects: refs.map(ref => ref.projectPath), activeProject: active.projectPath,
      activeSession: active, pinnedSessions: [], archivedSessions: [], theme: 'light',
      sidebar: { organization: 'list', chatSort: 'updated', manualSessionOrder: {} } },
    workspace: { path: active.projectPath, trusted: true }, connection: { status: 'connected' },
    runtimes: refs.map(ref => ({ ...ref, connection: { status: 'connected' }, turnActive: false,
      pendingInteractions: 0, pendingAskUserQuestions: 0 })),
    projectCatalogs: Object.fromEntries(refs.map((ref, index) => [ref.projectPath, { sessions: [rows[index]] }])),
    providerCredentials: [{ providerId: 'openai', configured: true }], diagnostics: [],
  };
}

window.addEventListener(SESSION_PICKER_EVENT, (event) => {
  const query = (event as CustomEvent<{ query: string }>).detail.query;
  pickerEvents.push({ projectPath: active.projectPath, query });
});
Object.assign(window, { sessionPickerFixture: {
  state: () => ({ active, pending: Boolean(finishCommand), completedCommands, admissions, pickerEvents, listCalls, openCalls,
    resumePending: Boolean(pendingResume), resumeTarget: pendingResume?.sessionId, resumeApproved: pendingResume?.approved ?? false,
    approvalCalls, promptCalls, navigationCalls }),
  deferNextLocalCommand: () => { deferNextCommand = true; },
  finishLocalCommand: () => finishCommand?.(),
  deferNextResumeApproval: () => { deferNextResume = true; },
  finishResume: () => { if (!pendingResume?.approved) throw new Error('Resume approval is still pending.'); pendingResume.finish(); },
  applicationMenu: (action: string) => { for (const listener of applicationMenuListeners) listener(action); },
} });
(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onApplicationMenu: (listener: (action: string) => void) => {
    applicationMenuListeners.add(listener); return () => applicationMenuListeners.delete(listener);
  },
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onPermission: (listener: (envelope: unknown) => void) => {
    permissionListeners.add(listener); return () => permissionListeners.delete(listener);
  },
  onComputerAccess: () => () => {},
  bootstrap: async () => snapshot(), settings: async () => snapshot().settings,
  command: async (sessionId: string, command: { type: string; text?: string }) => {
    if (command.type !== 'record_prompt_suggestion_submission' || !command.text?.startsWith('/resume')) return;
    admissions.push({ sessionId, text: command.text });
    if (deferNextCommand) {
      deferNextCommand = false;
      await new Promise<void>(resolve => { finishCommand = () => { finishCommand = undefined; resolve(); }; });
    }
    completedCommands++;
  },
  openSession: async (projectPath: string, sessionId: string, selection?: { transcriptPath?: string }) => {
    const target = refs.find(ref => ref.sessionId === sessionId && (selection?.transcriptPath || ref.projectPath === projectPath));
    if (!target) throw new Error('Unknown fixture chat.');
    openCalls.push({ projectPath, sessionId });
    if (deferNextResume) {
      deferNextResume = false;
      await new Promise<void>(resolve => {
        pendingResume = { sessionId, approved: false, finish: resolve };
        queueMicrotask(() => {
          for (const listener of permissionListeners) listener({ sessionId, event: { request_id: 77,
            kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{"command":"echo resumed target"}', default_allow: false },
            backgroundOwned: false } });
        });
      });
      pendingResume = undefined;
    }
    active = target; revision++;
    return snapshot();
  },
  approve: async (sessionId: string, requestId: number, response: unknown) => {
    if (!pendingResume || pendingResume.sessionId !== sessionId || requestId !== 77) throw new Error('Wrong resume approval owner.');
    approvalCalls.push({ sessionId, requestId, response }); pendingResume.approved = true;
  },
  sendPrompt: async (sessionId: string, text: string) => { promptCalls.push({ sessionId, text }); },
  pickWorkspace: async () => { navigationCalls.push('add-project'); throw new Error('Unexpected project picker during resume.'); },
  setWorkspace: async () => { navigationCalls.push('activate-project'); throw new Error('Unexpected project switch during resume.'); },
  removeProject: async () => { navigationCalls.push('remove-project'); throw new Error('Unexpected project removal during resume.'); },
  newSession: async () => { navigationCalls.push('new-session'); throw new Error('Unexpected new session during resume.'); },
  listProjectSessions: async (projectPath: string, scope = 'current') => {
    listCalls.push({ projectPath, scope });
    return { projectPath, sessions: scope === 'current'
      ? [rows[refs.findIndex(ref => ref.projectPath === projectPath)]].filter(Boolean) : rows };
  },
  preflightSessionArchive: async () => [],
  scheduled: { scopes: async () => [{ id: 'global', label: 'No project' }],
    context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
};
createRoot(document.getElementById('root')!).render(<App />);
