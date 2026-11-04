import { DESKTOP_BRIDGE_GLOBAL } from '../../src/shared/desktopBridge';
import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import '../../src/renderer/global.css';

// The whole window with two Bots on a stand-in host, to drive the Bots mode of the sidebar in a real renderer.
//   ?saved=bots   the settings file says the window was last in Bots mode
//   ?bots=off     Bots cannot be used on this computer
const params = new URLSearchParams(location.search);
const botsOn = params.get('bots') !== 'off';
const projectPath = '/fixture/Project';
const botPath = (botId: string) => `/fixture/bots/${botId}/workspace`;
const MINUTE = 60_000;
const now = Date.now();

const chat = (uuid: string, title: string, minutesAgo: number) => ({
  uuid, title, modified_rfc3339: new Date(now - minutesAgo * MINUTE).toISOString(), message_count: 3, mode: 'code', path: `/fixture/transcripts/${uuid}.jsonl`,
});
const bots = [
  { botId: 'alpha', name: 'Alpha', description: 'Writes things down', createdAt: now - 900 * MINUTE, updatedAt: now, revision: 1, workspaces: [], lastActivityAt: now - 30 * MINUTE },
  { botId: 'beta', name: 'Beta', description: 'Reviews changes', createdAt: now - 800 * MINUTE, updatedAt: now, revision: 1, workspaces: [], lastActivityAt: now - 3 * MINUTE },
];
const chats: Record<string, ReturnType<typeof chat>[]> = {
  alpha: [chat('a-1', 'Plan the launch', 30), chat('a-2', 'Weekly notes', 60), chat('a-3', 'Draft the post', 90), chat('a-4', 'Fix the intro', 120), chat('a-5', 'Release checklist', 150), chat('a-6', 'Old ideas', 600), chat('a-7', 'Oldest thing', 700)],
  beta: [chat('b-1', 'Review the diff', 3)],
};

const calls: unknown[][] = [];
(window as any).fixtureCalls = calls;
const settings: any = {
  version: 1, projects: [projectPath], activeProject: projectPath, activeSession: { projectPath, sessionId: 'project-session' },
  pinnedSessions: [], archivedSessions: [], theme: 'dark',
  ...(params.get('saved') === 'bots' ? { sidebar: { organization: 'project', chatSort: 'updated', manualSessionOrder: {}, mode: 'bots' } } : {}),
};
let activeSession: { projectPath: string; sessionId: string; mode: 'code'; projectId: string | null } = { projectPath, sessionId: 'project-session', mode: 'code', projectId: projectPath };
const botRuntimes: Array<{ sessionId: string; projectPath: string; botId: string }> = [];
let revision = 1;
let fresh = 0;

const runtime = (session: { projectPath: string; sessionId: string }, botId?: string) => ({
  ...session, ...(botId ? { botId } : {}), connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0,
});

(window as any)[DESKTOP_BRIDGE_GLOBAL] = {
  platform: 'darwin', isElectron: true,
  onApplicationMenu: (listener: (action: string) => void) => {
    (window as any).applicationMenuAction = listener;
    return () => { delete (window as any).applicationMenuAction; };
  },
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, listProjectSessions: async () => ({ projectPath, sessions: [] }),
  bootstrap: async () => ({
    revision: ++revision, settings: structuredClone(settings), activeSession, workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
    conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: [projectPath], selectedProject: projectPath, activeSession: activeSession.projectId === null ? undefined : activeSession }),
    runtimes: [runtime({ projectPath, sessionId: 'project-session' }), ...botRuntimes.map((entry) => runtime(entry, entry.botId))],
    projectCatalogs: { [projectPath]: { sessions: [] } }, diagnostics: [], providerAccountSnapshot: fixtureAccountSnapshot(['openai']),
  }),
  updateSettings: async (patch: any) => {
    calls.push(['updateSettings', patch]);
    // Like the host: the sidebar keys left out of an update keep what they were, and a first update starts from the defaults.
    if (patch.sidebar) settings.sidebar = { organization: 'project', chatSort: 'updated', manualSessionOrder: {}, mode: 'home', ...settings.sidebar, ...patch.sidebar };
    return structuredClone(settings);
  },
  scheduled: { scopes: async () => [], context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
  bots: {
    availability: async () => botsOn ? { available: true } : { available: false, reason: 'Fixture: Bots are off.' },
    list: async () => botsOn ? structuredClone(bots) : [],
    get: async (botId: string) => structuredClone(bots.find((bot) => bot.botId === botId)),
    delete: async (botId: string) => { calls.push(['delete', botId]); bots.splice(bots.findIndex((bot) => bot.botId === botId), 1); },
    listSessions: async (botId: string) => { calls.push(['listSessions', botId]); return { sessions: structuredClone(chats[botId] ?? []) }; },
    openSession: async (botId: string, sessionId?: string, transcriptPath?: string) => {
      calls.push(['openSession', botId, sessionId ?? null, ...(transcriptPath === undefined ? [] : [transcriptPath])]);
      const id = sessionId ?? `fresh-${++fresh}`;
      activeSession = { projectPath: botPath(botId), sessionId: id, mode: 'code', projectId: null };
      settings.activeSession = activeSession;
      if (!botRuntimes.some((entry) => entry.sessionId === id)) botRuntimes.push({ sessionId: id, projectPath: botPath(botId), botId });
      return { botId, projectPath: botPath(botId), sessionId: id };
    },
    onSessionShown: () => () => {}, onPromptSent: () => () => {},
    persona: { get: async () => '', set: async () => {} },
    memory: { list: async () => [], read: async () => '', write: async () => {}, delete: async () => {} },
    skills: { list: async () => [], delete: async () => {}, importFromUser: async () => ({ name: '', active: { received: 0, sent: 0 } }), listImportable: async () => [] },
    tasks: { list: async () => ({ tasks: [], truncated: false, active: { received: 0, sent: 0 } }), cancel: async () => {}, onChanged: () => () => {} },
    loginItem: { status: async () => ({ available: false, enabled: false, needsApproval: false }), set: async () => ({ available: false, enabled: false, needsApproval: false }), openSettings: async () => {} },
  },
};
createRoot(document.getElementById('root')!).render(<App />);
