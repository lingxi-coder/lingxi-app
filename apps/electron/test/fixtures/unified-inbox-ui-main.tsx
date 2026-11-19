import { createRoot } from 'react-dom/client';
import { inboxKey, isUnread, matchesInboxCategory, needsAttention, orderItems, type InboxCommand, type InboxItem, type InboxRef } from '../../../../packages/inbox/contracts';
import { card, FakeFriends, pairTeamId } from '../../../web/test/support/fakeFriends';
import { conversation, FakeTeams, message, team } from '../../../web/test/support/fakeTeams';
import { FRIEND_CHAT } from '../../../../packages/teams/friends';
import { App } from '../../src/renderer/App';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import type { DesktopInboxApi, InboxNotification } from '../../src/shared/inbox';
import { fixtureConversationSpaces } from './conversation-spaces-mock';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import '../../src/renderer/global.css';

setLanguagePreference('en');
const projectPath = '/fixture/Inbox Project';
const sourceId = 'desktop-source';
const assistantId = 'assistant-result';
const resultUuid = '12345678-1234-4234-8234-123456789abc';
const pairId = pairTeamId('bob');
const dmId = FRIEND_CHAT;
const teamId = 'team-studio';
const channelId = 'studio-general';
const now = Date.now();

function item(ref: InboxRef, title: string, preview: string, overrides: Partial<InboxItem>): InboxItem {
  return {
    schemaVersion: 1, key: inboxKey(ref), ref, category: ref.kind, title, preview, sourceCursor: 1,
    sourceRevision: 1, readThrough: 0, unreadCount: 1, mentionCount: 0, pendingActionCount: 0,
    lastActivityAt: now, state: 'idle', pinOrder: null, notificationMode: 'all', archived: false,
    manualUnread: false, manualUnreadRevision: 0, preferencesRevision: 0, stateRevision: 0, offline: false, ...overrides,
  };
}
const assistant = item({ kind: 'assistant', sourceId, conversationId: assistantId, generation: 1 },
  'Release checklist', 'The compatibility report is ready.', {
    sourceLabel: 'Inbox Project', state: 'waiting_for_approval', pendingActionCount: 1, pendingActionRevision: 2,
  });
const friend = item({ kind: 'team', sourceId: pairId, conversationId: dmId, generation: 1 },
  'Bob', 'The design is ready for your review.', { category: 'friend', peerUid: 'bob', lastActivityAt: now - 60_000 });
const channel = item({ kind: 'team', sourceId: teamId, conversationId: channelId, generation: 3 },
  'general', 'Build 42 passed the smoke checks.', { sourceLabel: 'Studio', lastActivityAt: now - 120_000, readThrough: 1, unreadCount: 0 });
const items = [assistant, friend, channel];

const teams = new FakeTeams();
teams.teams = [team(teamId, { name: 'Studio', generation: 3 })];
const dm = conversation(dmId, { kind: 'dm', lastSeq: 1 });
const chat = conversation(channelId, { title: 'general', lastSeq: 1, lastReadSeq: 1 });
teams.conversations.set(pairId, [dm]);
teams.conversations.set(teamId, [chat]);
teams.threads.set(dmId, [message(1, { text: friend.preview })]);
teams.threads.set(channelId, [message(1, { text: channel.preview })]);

const friends = new FakeFriends();
friends.cards = [card('bob')];
friends.names.set('bob', { displayName: 'Bob' });
const active = (sessionId: string, path = projectPath, projectId: string | null = projectPath) =>
  ({ projectPath: path, projectId, sessionId, mode: 'code' as const });
const target = active(assistantId);
let selected = active('startup');
let revision = 0;
const catalog = {
  uuid: assistantId, title: assistant.title, message_count: 3, mode: 'code', modified_rfc3339: new Date(now).toISOString(),
  path: '/fixture/release.jsonl',
  activity_json: JSON.stringify({ sourceId, cursor: 1, baselineCursor: 0, latestResultUuids: [resultUuid] }),
};
const settings = () => ({
  version: 1, projects: [projectPath], activeProject: projectPath, activeSession: selected,
  theme: 'light', pinnedSessions: [], archivedSessions: [],
});
const snapshot = () => ({
  revision: ++revision, settings: settings(), activeSession: selected, workspace: { path: selected.projectPath, trusted: true },
  conversationSpaces: fixtureConversationSpaces({ mode: 'code', projects: [projectPath], selectedProject: projectPath,
    activeSession: selected, sessions: [target] }),
  connection: { status: 'connected' },
  runtimes: [{ ...selected, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
  projectCatalogs: { [projectPath]: { sessions: [catalog] } },
  providerAccountSnapshot: fixtureAccountSnapshot(['openai']), diagnostics: [],
});
const watch = new Set<{ uid: string | null; dirty: () => void; badge: (count: number) => void }>();
const notifications = new Set<(notice: InboxNotification) => void>();
const eventListeners = new Set<(envelope: unknown) => void>();
const requests: { uid: string | null; command: InboxCommand; at: number }[] = [];
const visible: { sessionId: string | null; key?: string | null; generation?: number }[] = [];
const opens: string[] = [];
let sequence = 0;
let summarySync = false;
let dmReceipts = false;
let conversationSummarySync = true;

function refresh() {
  for (const listener of watch) {
    listener.badge(listener.uid ? items.filter(row => !row.archived && (isUnread(row) || needsAttention(row))).length : 0);
    listener.dirty();
  }
}
function emit(type: string, properties: object) {
  const event = { sessionId: selected.sessionId, sequence: ++sequence, event: { type, ...properties } };
  for (const callback of eventListeners) callback(event);
}
function resumeAssistant() {
  emit('session_resumed', { session_id: assistantId, mode: 'code', messages: [
    { role: 'user', blocks: [{ type: 'text', text: 'Check compatibility before the release.' }] },
    { role: 'assistant', blocks: [{ type: 'text', text: 'Earlier discussion, before the latest result.' }] },
    { role: 'assistant', transcript_uuid: resultUuid, blocks: [{ type: 'text', text: assistant.preview }] },
  ] });
}
function serve(uid: string | null, command: InboxCommand): unknown {
  requests.push({ uid, command: structuredClone(command), at: Date.now() });
  if (command.action === 'list') return { items: uid === 'alice' ? items.filter(row =>
    row.archived === Boolean(command.archived) && matchesInboxCategory(row.category, command.category) &&
    (command.filter !== 'unread' || isUnread(row)) && (command.filter !== 'attention' || needsAttention(row)) &&
    (!command.searchPrefix || row.title.toLowerCase().startsWith(command.searchPrefix))).sort(orderItems) : [] };
  if (!uid) throw new Error('SIGN_IN_REQUIRED');
  if (command.action === 'accountPreferences') {
    dmReceipts = command.dmReceipts ?? dmReceipts;
    return { dmReceipts };
  }
  if (command.action === 'notices') return { items: [] };
  if (command.action === 'peerReceipt') return { readThrough: null };
  if (!('key' in command)) throw new Error(`Unsupported fixture action: ${command.action}`);
  const row = items.find(item => item.key === command.key);
  if (!row) throw new Error('INBOX_ITEM_MISSING');
  if (command.action === 'get') return { item: row };
  if (command.action === 'preferences') {
    if (command.expectedPreferencesRevision !== row.preferencesRevision) throw new Error('PREFERENCE_REVISION_CONFLICT');
    Object.assign(row, command.patch); row.preferencesRevision = (row.preferencesRevision ?? 0) + 1;
  }
  else if (command.action === 'markRead') {
    row.readThrough = Math.max(row.readThrough, command.through);
    row.unreadCount = Math.max(0, row.sourceCursor - row.readThrough);
    if (command.manualUnreadRevision === row.manualUnreadRevision) row.manualUnread = false;
  } else if (command.action === 'markUnread') {
    if (command.expectedManualUnreadRevision !== row.manualUnreadRevision) return { item: row };
    row.manualUnread = true;
    row.manualUnreadRevision++;
  } else throw new Error(`Unsupported fixture mutation: ${command.action}`);
  row.stateRevision++;
  refresh();
  return { item: row };
}
const inbox: DesktopInboxApi = {
  request: async <T,>(uid: string | null, command: InboxCommand) => structuredClone(serve(uid, command)) as T,
  watch: (uid, dirty, badge) => {
    const listener = { uid, dirty, badge };
    watch.add(listener);
    refresh();
    return () => { watch.delete(listener); };
  },
  onNotification: callback => { notifications.add(callback); return () => { notifications.delete(callback); }; },
  setVisible: async (sessionId, key, generation) => { visible.push({ sessionId, key, generation }); },
  localRef: async key => key === assistant.key ? target : null,
  activityPage: async (key, after) => ({
    cursor: 1, baselineCursor: 0, hasMore: false,
    items: key === assistant.key && after < 1 ? [{ cursor: 1, resultUuids: [resultUuid], kind: 'reply', at: now }] : [],
  }),
  activityPreview: async (key, through) => ({ available: key === assistant.key && through === 1,
    messages: key === assistant.key && through === 1 ? [{ role: 'assistant', transcript_uuid: resultUuid,
      blocks: [{ type: 'text', text: assistant.preview }] }] : [] }),
  summarySync: async () => summarySync,
  setSummarySync: async (_uid, enabled) => { summarySync = enabled; },
  conversationSummarySync: async () => conversationSummarySync,
  setConversationSummarySync: async (_uid, _key, enabled) => { conversationSummarySync = enabled; },
};
const off = () => () => {};
Object.assign(window, {
  inboxFixture: {
    assistant: assistant.key, friend: friend.key, channel: channel.key, resultUuid, selected: () => selected.sessionId,
    opens, visible, requests, resumeAssistant,
    notifyAssistant: (activate = true) => {
      for (const callback of notifications) callback({ key: assistant.key, generation: 1, title: 'LingXi', body: 'Conversation update', kind: 'completion', through: 1, activate });
    },
    notifyFriend: () => {
      for (const callback of notifications) callback({ key: friend.key, generation: 1, title: 'LingXi', body: 'Conversation update', kind: 'dm', through: 1 });
    },
  },
  lingxi: {
    platform: 'darwin', isElectron: true, inbox,
    account: { request: async () => ({ user: { uid: 'alice', email: 'alice@example.com', displayName: 'Alice', emailVerified: true },
      busy: false, persistence: 'session', error: null }), onChanged: off },
    bootstrap: async () => snapshot(), settings: async () => settings(),
    updateSettings: async () => settings(),
    localApps: { list: async () => [], authoringSessions: async () => [{ name: 'Fixture App', projectPath: '/fixture/app-source', sessionId: 'app-author', source: { scope: 'current', transcriptPath: '/fixture/app-source/app.jsonl' } }], onChanged: off },
    openSession: async (path: string, id: string) => { opens.push(id); selected = active(id, path, id === 'draft' ? null : path); return snapshot(); },
    newSession: async () => { selected = active('draft', '/fixture/global-code', null); return snapshot(); },
    onEvent: (callback: (envelope: unknown) => void) => { eventListeners.add(callback); return () => { eventListeners.delete(callback); }; },
    onConnectionStateChanged: off, onPermission: off, onComputerAccess: off, command: async () => {},
    listProjectSessions: async () => ({ projectPath, sessions: [catalog] }),
    teams: {
      localBots: async () => [],
      list: async () => teams.api.list(),
      detail: async (_uid: string, id: string) => teams.api.detail(id),
      execute: async (_uid: string, command: Parameters<typeof teams.api.execute>[0]) => teams.api.execute(command),
      messages: async (_uid: string, command: Parameters<typeof teams.api.messages>[0]) => teams.api.messages(command),
      onChanged: teams.api.onChanged, onMessagesChanged: teams.api.onMessagesChanged,
      watchMessages: (_uid: string, ...args: Parameters<typeof teams.api.watchMessages>) => teams.api.watchMessages(...args),
    },
    friends: {
      profile: friends.api.profile,
      execute: (_uid: string, command: Parameters<typeof friends.api.execute>[0]) => friends.api.execute(command),
      watchCards: (_uid: string, ...args: Parameters<typeof friends.api.watchCards>) => friends.api.watchCards(...args),
      watchBlocks: (_uid: string, ...args: Parameters<typeof friends.api.watchBlocks>) => friends.api.watchBlocks(...args),
      watchOffers: (_uid: string, ...args: Parameters<typeof friends.api.watchOffers>) => friends.api.watchOffers(...args),
      readProfile: (_uid: string, other: string) => friends.api.readProfile(other),
    },
  },
});
createRoot(document.getElementById('root')!).render(<App />);
