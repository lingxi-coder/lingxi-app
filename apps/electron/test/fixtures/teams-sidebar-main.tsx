import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { FakeTeams, botCard, conversation, message, team } from '../../../web/test/support/fakeTeams';
import '../../src/renderer/global.css';

// The whole window with a signed-in account, a stand-in Teams service and two Bots, to drive the Teams mode of the sidebar in a real renderer.
//   ?saved=teams|bots      the settings file says the window was last in that mode
//   ?drawerWidth=420       the settings file holds this width for the Teams details panel
//   ?theme=light|dark      the appearance (dark by default)
//   ?signedOut=1           there is no account
//   ?many=1                Beta has 249 unread messages, so that the rail counts past 99
// `window.deliverLink(route, params)` hands the window a link, as the main process does when one is opened.
const params = new URLSearchParams(location.search);
const projectPath = '/fixture/Project';
const MINUTE = 60_000;
const now = Date.now();
const ago = (minutes: number) => new Date(now - minutes * MINUTE).toISOString();

// ---- Teams ----
const fake = new FakeTeams();
const ownerBot = botCard();
fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner', memberCount: 3 }), team('team-bbbb', { name: 'Beta', generation: 2, memberCount: 2 })];
fake.bots = [ownerBot];
const lastMessage = (id: string, seq: number, minutes: number, preview: string, uid: string) => ({ id, seq, at: ago(minutes), preview, author: { kind: 'user' as const, uid } });
fake.conversations.set('team-aaaa', [
  conversation('chan-general', { title: 'general', participants: ['user:alice', 'user:bob', ownerBot.key], lastSeq: 7, lastReadSeq: 3, lastMessage: lastMessage('message-7', 7, 20, 'Ship it', 'bob') }),
  conversation('chan-design', { title: 'design', participants: ['user:alice', 'user:bob'], lastSeq: 2, lastReadSeq: 2, lastMessage: lastMessage('d-2', 2, 300, 'Mockups are in the folder', 'bob') }),
  conversation('dm-bob', { kind: 'dm', title: 'Alice, Bob', participants: ['user:alice', 'user:bob'], lastSeq: 1, lastReadSeq: 1, lastMessage: lastMessage('dm-1', 1, 10, 'Lunch?', 'alice') }),
  conversation('bot-chat', { kind: 'bot_dm', title: 'Fixture Bot', participants: ['user:alice', ownerBot.key], botKey: ownerBot.key, lastSeq: 2, lastReadSeq: 2,
    lastMessage: { id: 'b-2', seq: 2, at: ago(90), preview: 'Done: the summary is ready', author: { kind: 'bot', uid: 'owner', botKey: ownerBot.key } } }),
]);
const many = params.get('many') === '1';
fake.conversations.set('team-bbbb', [conversation('chan-b', { title: 'announcements', lastSeq: many ? 250 : 4, lastReadSeq: 1, lastMessage: lastMessage('m-b', many ? 250 : 4, 700, 'Welcome to Beta', 'bob') })]);
fake.threads.set('chan-general', [
  message(1, { text: 'Morning', createdAt: ago(62) }),
  message(2, { text: 'Standup in five', createdAt: ago(61) }),
  message(3, { text: 'Notes are in the doc', createdAt: ago(60) }),
  message(4, { text: 'Thanks, reading them now', author: { kind: 'user', uid: 'alice' }, createdAt: ago(41) }),
  message(5, { text: 'Anytime', replyTo: 'message-4', createdAt: ago(40) }),
  message(6, { text: 'Summary: three items are open and one is blocked.', author: { kind: 'bot', uid: 'owner', botKey: ownerBot.key }, createdAt: ago(25) }),
  message(7, { text: 'Ship it', createdAt: ago(20) }),
]);

const calls: unknown[][] = [];
(window as any).fixtureCalls = calls;
// Like the service: a message that is sent joins its thread, and every open watch of the conversation is told.
const baseMessages = fake.messages;
fake.messages = async <T,>(command: any): Promise<T> => {
  if (command.action === 'mutate') {
    calls.push(['messages', command]);
    if (command.operation === 'create') {
      const thread = fake.threads.get(command.conversationId) ?? [];
      const seq = (thread.at(-1)?.seq ?? 0) + 1;
      thread.push(message(seq, { id: command.messageId, text: command.text, author: { kind: 'user', uid: 'alice' }, createdAt: new Date().toISOString(), changeId: command.changeId }));
      fake.threads.set(command.conversationId, thread);
      const row = fake.conversations.get(command.teamId)?.find(item => item.id === command.conversationId);
      if (row) { row.lastSeq = seq; row.lastReadSeq = seq; }
      for (const watch of fake.open(command.teamId, command.conversationId)) fake.deliver(watch);
    }
    return {} as T;
  }
  return baseMessages(command) as Promise<T>;
};
const baseExecute = fake.execute;
fake.execute = async command => { calls.push(['execute', command]); return baseExecute(command); };

const desktopTeams = {
  localBots: async () => [],
  bindLocalBot: async () => ({ botKey: ownerBot.key, revision: 1 }),
  list: async () => fake.api.list(),
  detail: async (_uid: string, teamId: string) => fake.api.detail(teamId),
  execute: async (_uid: string, command: any) => fake.api.execute(command),
  messages: async (_uid: string, command: any) => fake.api.messages(command),
  onChanged: (callback: any) => fake.api.onChanged(callback),
  onMessagesChanged: (callback: any) => fake.api.onMessagesChanged(callback),
  watchMessages: (_uid: string, scope: any, conversationId: string, next: any) => fake.api.watchMessages(scope, conversationId, next),
};

// ---- Bots ----
const botPath = (botId: string) => `/fixture/bots/${botId}/workspace`;
const chat = (uuid: string, title: string, minutesAgo: number) => ({
  uuid, title, modified_rfc3339: ago(minutesAgo), message_count: 3, mode: 'code', path: `/fixture/transcripts/${uuid}.jsonl`,
});
const bots = [
  { botId: 'alpha', name: 'Alpha', description: 'Writes things down', createdAt: now - 900 * MINUTE, updatedAt: now, revision: 1, workspaces: [], lastActivityAt: now - 30 * MINUTE },
  { botId: 'beta', name: 'Beta', description: 'Reviews changes', createdAt: now - 800 * MINUTE, updatedAt: now, revision: 1, workspaces: [], lastActivityAt: now - 3 * MINUTE },
];
const chats: Record<string, ReturnType<typeof chat>[]> = {
  alpha: [chat('a-1', 'Plan the launch', 30), chat('a-2', 'Weekly notes', 60), chat('a-3', 'Draft the post', 90)],
  beta: [chat('b-1', 'Review the diff', 3)],
};

// ---- The window ----
const saved = params.get('saved');
const drawerWidth = Number(params.get('drawerWidth'));
const settings: any = {
  version: 1, projects: [projectPath], activeProject: projectPath, activeSession: { projectPath, sessionId: 'project-session' },
  pinnedSessions: [], archivedSessions: [], theme: params.get('theme') === 'light' ? 'light' : 'dark',
  ...(saved || drawerWidth ? { sidebar: { organization: 'project', chatSort: 'updated', manualSessionOrder: {}, ...(saved ? { mode: saved } : {}), ...(drawerWidth ? { teamsDrawerWidth: drawerWidth } : {}) } } : {}),
};
let activeSession = { projectPath, sessionId: 'project-session' };
const botRuntimes: Array<{ sessionId: string; projectPath: string; botId: string }> = [];
let revision = 1;
let fresh = 0;
const runtime = (session: { projectPath: string; sessionId: string }, botId?: string) => ({
  ...session, ...(botId ? { botId } : {}), connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0,
});
const accountState = {
  user: params.get('signedOut') === '1' ? null : { uid: 'alice', email: 'alice@example.com', displayName: 'Alice', emailVerified: true },
  busy: false, persistence: 'session', error: null,
};
const pendingLinks: Array<{ id: string; target: { route: string; params: Record<string, string> } }> = [];
const linkListeners = new Set<() => void>();
let links = 0;
(window as any).deliverLink = (route: string, linkParams: Record<string, string>) => {
  pendingLinks.push({ id: `link-${++links}`, target: { route, params: linkParams } });
  for (const listener of [...linkListeners]) listener();
};

(window as any).lingxi = {
  platform: 'darwin', isElectron: true,
  onApplicationMenu: (listener: (action: string) => void) => {
    (window as any).applicationMenuAction = listener;
    return () => { delete (window as any).applicationMenuAction; };
  },
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  command: async () => {}, listProjectSessions: async () => ({ projectPath, sessions: [] }),
  bootstrap: async () => ({
    revision: ++revision, settings: structuredClone(settings), activeSession, workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
    runtimes: [runtime({ projectPath, sessionId: 'project-session' }), ...botRuntimes.map((entry) => runtime(entry, entry.botId))],
    projectCatalogs: { [projectPath]: { sessions: [] } }, diagnostics: [], providerAccountSnapshot: fixtureAccountSnapshot(['openai']),
  }),
  updateSettings: async (patch: any) => {
    calls.push(['updateSettings', patch]);
    // Like the host: the sidebar keys left out of an update keep what they were.
    if (patch.sidebar) settings.sidebar = { organization: 'project', chatSort: 'updated', manualSessionOrder: {}, ...settings.sidebar, ...patch.sidebar };
    return structuredClone(settings);
  },
  deepLinks: {
    copySessionLink: async () => {},
    next: async () => pendingLinks.shift() ?? null,
    acknowledge: async (id: string, error?: string) => { calls.push(['acknowledge', id, error ?? null]); },
    onAvailable: (callback: () => void) => { linkListeners.add(callback); return () => { linkListeners.delete(callback); }; },
  },
  account: { request: async () => structuredClone(accountState), onChanged: () => () => {} },
  teams: desktopTeams,
  scheduled: { scopes: async () => [], context: async () => ({ models: [], currentModel: '', sessions: [] }), manage: async () => [] },
  bots: {
    availability: async () => ({ available: true }),
    list: async () => structuredClone(bots),
    get: async (botId: string) => structuredClone(bots.find((bot) => bot.botId === botId)),
    delete: async () => {},
    listSessions: async (botId: string) => ({ sessions: structuredClone(chats[botId] ?? []) }),
    openSession: async (botId: string, sessionId?: string) => {
      const id = sessionId ?? `fresh-${++fresh}`;
      activeSession = { projectPath: botPath(botId), sessionId: id };
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
