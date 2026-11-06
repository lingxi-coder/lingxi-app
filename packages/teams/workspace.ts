import { executionCommand, sharedBotUsable, type SharedBotCard } from './execution.js';
import { indexedDbCommandStorage } from './command-storage.js';
import { inviteLink, type TeamCommand, type TeamDetail, type TeamMember, type TeamSummary, type TeamsApi } from './index.js';
import { TeamMessaging, pendingPayload, type CommandStorage, type MessageScope, type PendingCommands, type TeamConversation, type TeamMessage } from './messages.js';

// ─── Pure helpers ───────────────────────────────────────────────────────────

/** A person or Bot, in the shape avatars take. */
export interface Identity { kind: 'user' | 'bot'; id: string; name: string }

/** `max(0, lastSeq - lastReadSeq)`; edits and recalls never advance `lastSeq`. */
export const unreadCount = (conversation: Pick<TeamConversation, 'lastSeq' | 'lastReadSeq'>): number => Math.max(0, conversation.lastSeq - conversation.lastReadSeq);
const totalUnreadOf = (conversations: readonly TeamConversation[]): number => conversations.reduce((sum, conversation) => sum + unreadCount(conversation), 0);

const activity = (conversation: TeamConversation): number | undefined => {
  const ms = conversation.lastMessage?.at ? Date.parse(conversation.lastMessage.at) : Number.NaN;
  return Number.isNaN(ms) ? undefined : ms;
};
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
/** Newest `lastMessage.at` first. Rows without a time follow, by `lastSeq` (newest first), then title. */
export function compareConversations(a: TeamConversation, b: TeamConversation): number {
  const x = activity(a), y = activity(b);
  if (x !== undefined && y !== undefined && x !== y) return y - x;
  if ((x === undefined) !== (y === undefined)) return x === undefined ? 1 : -1;
  return b.lastSeq - a.lastSeq || collator.compare(a.title, b.title) || a.id.localeCompare(b.id, 'en');
}

export interface ConversationGroups { channels: TeamConversation[]; direct: TeamConversation[]; botDirect: TeamConversation[] }
/**
 * Sidebar sections: channels, people (dm and group), and Bot DMs. A Bot's owner can read other
 * members' DMs with that Bot; those follow the viewer's own Bot DMs.
 */
export function groupConversations(conversations: readonly TeamConversation[], uid: string): ConversationGroups {
  const channels: TeamConversation[] = [], direct: TeamConversation[] = [], botDirect: TeamConversation[] = [];
  for (const conversation of conversations) {
    if (conversation.kind === 'channel') channels.push(conversation);
    else if (conversation.kind === 'bot_dm') botDirect.push(conversation);
    else direct.push(conversation);
  }
  const mine = (conversation: TeamConversation) => !conversation.participants.length || conversation.participants.includes(`user:${uid}`);
  channels.sort(compareConversations); direct.sort(compareConversations);
  botDirect.sort((a, b) => Number(mine(b)) - Number(mine(a)) || compareConversations(a, b));
  return { channels, direct, botDirect };
}

const userIds = (conversation: TeamConversation): string[] => conversation.participants.filter(value => value.startsWith('user:')).map(value => value.slice(5));
const sharedBotIdOf = (conversation: TeamConversation): string | undefined => conversation.participants.find(value => value.startsWith('shared_bot:'))?.slice(11);
const cardFor = (key: string | undefined, bots: readonly SharedBotCard[]) => key ? bots.find(bot => bot.sharedBotId === key) : undefined;

export interface LabelOptions { bots?: readonly SharedBotCard[]; zh?: boolean }
/** A person's display name, falling back to "You" / "Member" for people the directory does not list. */
function memberName(id: string, members: readonly TeamMember[], uid: string, zh: boolean): string {
  return members.find(member => member.uid === id)?.displayName || (id === uid ? (zh ? '你' : 'You') : (zh ? '成员' : 'Member'));
}
/** Titles made of the participants' own names are generated defaults, not names a person chose. */
function isGeneratedTitle(title: string, names: string[]): boolean {
  const parts = title.split(/[,、]/u).map(part => part.trim()).filter(Boolean);
  if (!parts.length || parts.length !== names.length) return false;
  const rest = [...names];
  for (const part of parts) {
    const index = rest.indexOf(part);
    if (index < 0) return false;
    rest.splice(index, 1);
  }
  return true;
}
/**
 * What to call a conversation. A channel is its title. A dm or group without a chosen title (empty,
 * or just the participants' names) is named after the other participants. A Bot DM is the Bot's name,
 * followed by the person for the Bot's owner reading someone else's DM.
 */
export function conversationLabel(conversation: TeamConversation, members: readonly TeamMember[], uid: string, options: LabelOptions = {}): string {
  const zh = options.zh ?? false, title = conversation.title.trim();
  const people = userIds(conversation);
  if (conversation.kind === 'channel') return title || (zh ? '频道' : 'Channel');
  if (conversation.kind === 'bot_dm') {
    const bot = cardFor(sharedBotIdOf(conversation), options.bots ?? [])?.name || title || 'Bot';
    const others = people.filter(id => id !== uid);
    return people.includes(uid) || !others.length ? bot : `${bot} · ${others.map(id => memberName(id, members, uid, zh)).join(zh ? '、' : ', ')}`;
  }
  const names = people.map(id => memberName(id, members, uid, zh));
  if (title && !isGeneratedTitle(title, names)) return title;
  const others = people.filter(id => id !== uid).map(id => memberName(id, members, uid, zh));
  return others.join(zh ? '、' : ', ') || title || (conversation.kind === 'dm' ? (zh ? '私信' : 'Direct message') : (zh ? '群聊' : 'Group'));
}

/** Everyone in a conversation as avatar identities: other people, then Bots, then the viewer. */
export function conversationParticipants(conversation: TeamConversation, members: readonly TeamMember[], bots: readonly SharedBotCard[], uid: string, zh = false): (Identity & { self: boolean })[] {
  const people = userIds(conversation).map(id => ({ kind: 'user' as const, id, name: memberName(id, members, uid, zh), self: id === uid }));
  const robots = conversation.participants.filter(value => value.startsWith('shared_bot:')).map(principal => {
    const key = principal.slice(11);
    const card = cardFor(key, bots);
    return { kind: 'bot' as const, id: card?.sharedBotId ?? key, name: card?.name || 'Bot', self: false };
  });
  return [...people.filter(person => !person.self), ...robots, ...people.filter(person => person.self)];
}

/** The viewer's own Bot DM with a Bot, if there is one. (A Bot's owner also sees other people's.) */
export function findBotDm(conversations: readonly TeamConversation[], uid: string, sharedBotId: string): TeamConversation | undefined {
  return conversations.find(conversation => conversation.kind === 'bot_dm' && conversation.participants.includes(`shared_bot:${sharedBotId}`) && conversation.participants.includes(`user:${uid}`));
}

/** The viewer's existing two-person DM with someone. */
export function findDm(conversations: readonly TeamConversation[], uid: string, peerUid: string): TeamConversation | undefined {
  return conversations.find(conversation => conversation.kind === 'dm' && conversation.participants.length === 2
    && conversation.participants.includes(`user:${uid}`) && conversation.participants.includes(`user:${peerUid}`));
}

/**
 * The stored title for a dm or group made without one: every participant's name, the creator first.
 * `conversationLabel` recognizes it and shows only the other people. Cut at `limit` characters.
 */
export function defaultConversationTitle(uids: readonly string[], members: readonly TeamMember[], uid: string, limit = 120): string {
  const names = [uid, ...uids.filter(id => id !== uid)].map(id => memberName(id, members, uid, false));
  let title = '';
  for (const name of names) {
    const next = title ? `${title}, ${name}` : name;
    if (Array.from(next).length > limit) return title || Array.from(name).slice(0, limit).join('');
    title = next;
  }
  return title;
}

/**
 * One line of a conversation's newest message. `lastMessage` absent: no messages yet. Recalled and non-text
 * messages get a label. "You: " prefixes the viewer's own message; `author` names the sender in groups and channels.
 */
export function conversationPreview(conversation: TeamConversation, uid: string, zh: boolean, author?: (message: NonNullable<TeamConversation['lastMessage']>) => string): string {
  const last = conversation.lastMessage;
  if (!last) return zh ? '暂无消息' : 'No messages yet';
  const mine = last.author.kind === 'user' && last.author.uid === uid;
  const prefix = mine ? (zh ? '你：' : 'You: ') : (conversation.kind === 'group' || conversation.kind === 'channel') && author ? `${author(last)}${zh ? '：' : ': '}` : '';
  if (last.recalled) return `${prefix}${zh ? '消息已撤回' : 'Message recalled'}`;
  return `${prefix}${last.preview || (zh ? '[非文本消息]' : '[Non-text message]')}`;
}

// ─── Workspace ──────────────────────────────────────────────────────────────

export interface TeamsSnapshot {
  readonly uid: string;
  /** Increments whenever the visible account/membership view is invalidated. Views drop open confirmations when it changes. */
  readonly authority: number;
  /** The team list has not been fetched yet (or is being fetched again after an invalidation). */
  readonly loading: boolean;
  /** A team command (create, join, invite, role, remove, leave, delete) is in flight. */
  readonly busy: boolean;
  /** A messaging action (create conversation, send, recall, mark read, Bot directory) is in flight. */
  readonly messageBusy: boolean;
  /** Team-level error code (a failed command or refetch); translate with `teamsErrorText`. Empty when none. */
  readonly error: string;
  /** Code of the last failed refetch of the team list or detail; empty after a successful one. */
  readonly loadError: string;
  /** Code of the last failed messaging action; translate with `messageErrorText`. */
  readonly actionError: string;
  readonly teams: TeamSummary[];
  readonly selectedTeamId: string;
  /** The selected team. Survives an invalidation while its conversations stay open. */
  readonly team: TeamSummary | undefined;
  /** Members and invites of the selected team. Hidden (null) from an invalidation until refetched. */
  readonly detail: TeamDetail | null;
  readonly detailLoading: boolean;
  /** People of the selected team for display names; kept across an invalidation, unlike `detail`. */
  readonly directory: TeamMember[];
  /** Invitation link created by the last successful invite command. */
  readonly inviteLink: string;
  /** The live controller, for embedded Bot cards that read its pending-command state. */
  readonly messaging: TeamMessaging | undefined;
  readonly conversations: TeamConversation[];
  readonly selectedConversationId: string;
  readonly conversation: TeamConversation | undefined;
  readonly messages: TeamMessage[];
  /** Pending (unconfirmed) mutations of the selected conversation. */
  readonly pending: PendingCommands['outbox'];
  /** A conversation creation that has not been confirmed yet. */
  readonly creation: PendingCommands['creation'];
  /** Showing this device's saved data; sending waits for a connection. */
  readonly offline: boolean;
  /** Waiting for the initial server snapshot; cached data alone is not an offline verdict. */
  readonly messagingConnecting: boolean;
  /** `TeamMessaging.error`: the controller's own failure code. */
  readonly messagingError: string;
  readonly bots: SharedBotCard[];
  /** The Bot list has been fetched at least once for this team. */
  readonly botsLoaded: boolean;
  /** The Bot list could not be fetched, so Bot availability is unknown. */
  readonly botsUnverified: boolean;
  /** Unread messages per team. The selected team counts its open directory, others a directory-only listener. */
  readonly unreadByTeam: Readonly<Record<string, number>>;
  readonly totalUnread: number;
}

export interface TeamsWorkspaceOptions {
  /** Pending-command storage for each team's controller. IndexedDB by default. */
  storage?: () => CommandStorage;
  /** Where `online` and `focus` come from. `window` by default; none when there is no window. */
  environment?: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>;
  /** Team to open first when it is in the list. */
  initialTeamId?: string;
  /** Delay between a change notification and the refetch it causes. */
  refreshDelay?: number;
  /** Teams (other than the selected one) that get a directory-only unread listener. */
  maxWatchedTeams?: number;
}

const EMPTY_MEMBERS: TeamMember[] = [], EMPTY_BOTS: SharedBotCard[] = [], EMPTY_CONVERSATIONS: TeamConversation[] = [], EMPTY_MESSAGES: TeamMessage[] = [];
const EMPTY_OUTBOX: PendingCommands['outbox'] = [];

const codeOf = (error: unknown, fallback = 'SERVICE_UNAVAILABLE'): string => error instanceof Error ? error.message : fallback;
/** A server verdict (an explicit error code) rather than a connectivity failure. */
const isVerdict = (error: unknown): boolean => error instanceof Error && /^[A-Z][A-Z_]{2,59}$/u.test(error.message) && error.message !== 'SERVICE_UNAVAILABLE';

/** Loads a team's Bot cards and reloads them when presence expires. */
class BotCards {
  private closed = false;
  private running = false;
  private again = false;
  private deadline: ReturnType<typeof setTimeout> | undefined;
  constructor(private readonly api: TeamsApi, private readonly scope: MessageScope, private readonly apply: (bots: SharedBotCard[] | undefined) => void) {}
  readonly refresh = async (): Promise<void> => {
    if (this.closed) return;
    if (this.running) { this.again = true; return; }
    this.running = true;
    try {
      const result = await this.api.messages<{ bots: SharedBotCard[] }>(executionCommand(this.scope, 'listSharedBots', {}));
      if (!this.closed) {
        this.apply(result.bots); clearTimeout(this.deadline);
        const expirations = result.bots.filter(bot => bot.ready && bot.onlineUntil && bot.onlineUntil > Date.now()).map(bot => bot.onlineUntil!);
        if (expirations.length) this.deadline = setTimeout(() => void this.refresh(), Math.max(1, Math.min(...expirations) - Date.now()));
      }
    } catch { if (!this.closed) this.apply(undefined); }
    finally { this.running = false; if (this.again && !this.closed) { this.again = false; void this.refresh(); } }
  };
  close(): void { this.closed = true; clearTimeout(this.deadline); }
}

/**
 * Everything a Slack-style Teams UI needs from one signed-in account: the team list, the selected
 * team's detail, that team's {@link TeamMessaging} controller, its Bot cards, and unread counts for
 * the other teams. No React: `useSyncExternalStore(workspace.subscribe, workspace.getSnapshot)`.
 *
 * Responses are applied only if no newer request or invalidation superseded them (epoch and authority
 * guards). There is one controller per team and membership generation, refreshed on `online` and `focus`.
 * Errors are kept as codes; views translate them.
 */
export class TeamsWorkspace {
  readonly subscribe = (listener: () => void): (() => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  readonly getSnapshot = (): TeamsSnapshot => this.current;

  private current: TeamsSnapshot;
  private readonly listeners = new Set<() => void>();
  private readonly storage: () => CommandStorage;
  private readonly environment: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> | undefined;
  private readonly refreshDelay: number;
  private readonly maxWatchedTeams: number;
  private initialTeamId: string;
  private readonly stops: (() => void)[] = [];
  private closed = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  // A response is applied only if no newer request or invalidation superseded it.
  private epoch = 0;
  private authority = 0;
  private selection = '';
  private listed = false;
  private refreshing = 0;
  private busy = false;
  private submitting = false;
  private messageBusy = false;
  private error = '';
  private loadError = '';
  private actionError = '';
  private teams: TeamSummary[] = [];
  private detail: TeamDetail | null = null;
  private readonly directories = new Map<string, TeamMember[]>();
  private link = '';
  // The selected team's controller and Bot cards.
  private messaging: TeamMessaging | undefined;
  private messagingTeam: TeamSummary | undefined;
  private releaseMessaging: (() => void) | undefined;
  private botCards: BotCards | undefined;
  private bots: SharedBotCard[] = EMPTY_BOTS;
  private botsLoaded = false;
  private botsUnverified = false;
  private readonly conversationByTeam = new Map<string, string>();
  private readonly drafts = new Map<string, string>();
  // Unread of the other teams: one directory-only listener each (`watchMessages` with no conversation).
  private readonly watchers = new Map<string, { generation: number; stop(): void }>();
  private readonly unread = new Map<string, number>();
  private outboxFor: { outbox: PendingCommands['outbox']; id: string; value: PendingCommands['outbox'] } | undefined;

  constructor(readonly api: TeamsApi, readonly uid: string, options: TeamsWorkspaceOptions = {}) {
    this.storage = options.storage ?? indexedDbCommandStorage;
    this.environment = options.environment ?? (typeof window === 'undefined' ? undefined : window);
    this.refreshDelay = options.refreshDelay ?? 80;
    this.maxWatchedTeams = options.maxWatchedTeams ?? 20;
    this.initialTeamId = options.initialTeamId ?? '';
    this.current = this.build();
    this.stops.push(api.onChanged(event => this.changed(event)));
    // Held for the workspace's whole life: the Web adapter stops every message listener when its last callback leaves.
    this.stops.push(api.onMessagesChanged(signal => { if (!signal || signal.bots) void this.botCards?.refresh(); }));
    void this.refresh();
  }

  // ── Reading ──

  /** Drafts survive switching conversations (not reloads). */
  draft = (conversationId: string): string => this.drafts.get(this.draftKey(conversationId)) ?? '';
  setDraft = (conversationId: string, text: string): void => {
    const key = this.draftKey(conversationId);
    if (text) this.drafts.set(key, text); else this.drafts.delete(key);
  };
  private draftKey(conversationId: string): string { return `${this.selection}:${conversationId}`; }

  // ── Team list and selection ──

  /** Refetch the team list and the selected team's detail. The newest request wins. */
  readonly refresh = async (): Promise<void> => {
    if (this.closed) return;
    const request = ++this.epoch, teamId = this.selection;
    this.refreshing++;
    try {
      const [list, current] = await Promise.allSettled([this.api.list(), teamId ? this.api.detail(teamId) : Promise.resolve(null)]);
      if (request !== this.epoch || this.closed) return;
      if (list.status === 'rejected') throw list.reason;
      this.teams = list.value.teams; this.listed = true;
      if (current.status === 'rejected') this.error = this.loadError = codeOf(current.reason);
      else { this.detail = current.value; this.error = this.loadError = ''; if (current.value) this.directories.set(current.value.team.id, current.value.members); }
      // A selection follows the list: a team that is gone (left, removed, deleted) is dropped, one that merely failed to load is kept.
      if (this.syncSelection()) queueMicrotask(() => void this.refresh());
    } catch (error) {
      if (request !== this.epoch || this.closed) return;
      this.listed = true; this.error = this.loadError = codeOf(error);
      // Only a verdict hides what is shown; a connectivity failure keeps the last list and the open conversations.
      if (isVerdict(error)) { this.detail = null; this.teams = []; this.link = ''; }
    } finally {
      this.refreshing--;
      this.publish();
    }
  };

  selectTeam = (id: string): void => {
    if (this.closed || id === this.selection) return;
    const summary = this.teams.find(team => team.id === id);
    if (!summary) return;
    this.selection = id; this.detail = null; this.link = ''; this.error = '';
    this.openMessaging(summary);
    this.reconcileWatchers();
    void this.refresh();
    this.publish();
  };

  /** Run a team command: one at a time; ignored after an invalidation; refetches afterwards. Resolves `true` when applied. */
  execute = async (command: TeamCommand): Promise<boolean> => {
    if (this.busy || this.closed) return false;
    const authorized = this.authority;
    this.busy = true; this.error = ''; this.publish();
    try {
      const result = await this.api.execute(command);
      if (this.closed) return false;
      if (command.action === 'delete' || command.action === 'leave') { this.dropSelection(); this.closeMessaging(); }
      if (authorized !== this.authority) { await this.refresh(); return false; }
      if (result.code) this.link = inviteLink(result.code);
      if (result.teamId && (command.action === 'create' || command.action === 'join')) { this.selection = result.teamId; this.detail = null; }
      await this.refresh();
      return true;
    } catch (error) {
      this.error = codeOf(error);
      return false;
    } finally {
      this.busy = false;
      this.publish();
    }
  };

  // ── Conversations and messaging ──

  selectConversation = (id: string): void => { this.messaging?.select(id); };
  /** Reconnect the selected team's listeners and retry pending commands. */
  reconnect = (): void => { this.messaging?.reconnect(); };
  refreshBots = (): Promise<void> => this.botCards?.refresh() ?? Promise.resolve();

  /**
   * Run a messaging action with the panel's lock: one at a time, failures kept as `actionError`.
   * Resolves `true` when it ran to completion. A second call while one runs resolves `false` and does nothing.
   */
  run = async (action: () => Promise<unknown>): Promise<boolean> => {
    if (this.submitting || this.closed) return false;
    this.submitting = true; this.messageBusy = true; this.actionError = ''; this.publish();
    try { await action(); return true; }
    catch (error) { this.actionError = codeOf(error, 'UNKNOWN'); return false; }
    finally { this.submitting = false; this.messageBusy = false; this.publish(); }
  };
  /** `run` for components that take an action and report nothing back. */
  attempt = async (action: () => Promise<unknown>): Promise<void> => { await this.run(action); };

  /** Create a dm, group or channel with people and Bots. Selects it. Bots must be usable now. */
  createConversation = (kind: 'dm' | 'group' | 'channel', title: string, parties: string[], botParties: string[] = []): Promise<boolean> => this.run(async () => {
    const model = this.messaging;
    if (!model) return;
    if (botParties.some(key => !this.bots.some(bot => bot.sharedBotId === key && sharedBotUsable(bot)))) throw new Error('BOT_USE_REQUIRED');
    await model.create(kind, title.trim(), [this.uid, ...parties], botParties);
  });
  /** Open the viewer's Bot DM, creating it when `fresh` or when there is none. Throws when the Bot is not usable; wrap it in `attempt`. */
  startBotDm = async (bot: SharedBotCard, fresh = false): Promise<void> => {
    const model = this.messaging;
    if (!model) return;
    if (!sharedBotUsable(bot)) throw new Error('BOT_USE_REQUIRED');
    const existing = fresh ? undefined : findBotDm(model.state.conversations, this.uid, bot.sharedBotId);
    if (existing) model.select(existing.id);
    else await model.create('bot_dm', bot.name, [this.uid], [bot.sharedBotId]);
  };
  openBotDm = (bot: SharedBotCard, fresh = false): Promise<boolean> => this.run(() => this.startBotDm(bot, fresh));
  /** Open the viewer's DM with a person, creating it when there is none. */
  openDm = (peerUid: string): Promise<boolean> => this.run(async () => {
    const model = this.messaging;
    if (!model) return;
    const existing = findDm(model.state.conversations, this.uid, peerUid);
    if (existing) model.select(existing.id);
    else await model.create('dm', defaultConversationTitle([peerUid], this.detail?.members ?? this.directories.get(this.selection) ?? EMPTY_MEMBERS, this.uid), [this.uid, peerUid]);
  });

  /**
   * Mark a conversation read (the open one by default). The open conversation goes through
   * `TeamMessaging.markRead`; any other is read up to its `lastSeq` with the same `read` command.
   * Nothing happens offline.
   */
  markRead = (conversationId: string = this.messaging?.selected ?? ''): Promise<boolean> => this.run(async () => {
    const model = this.messaging;
    if (!model || !conversationId) return;
    if (conversationId === model.selected) { await model.markRead(); return; }
    const row = model.state.conversations.find(value => value.id === conversationId);
    if (!row || model.offline || unreadCount(row) === 0) return;
    await this.api.messages({ action: 'read', ...model.scope, conversationId, lastReadSeq: row.lastSeq });
  });

  // ── Lifecycle ──

  /** Releases every listener, timer and controller. The workspace is unusable afterwards. */
  close(): void {
    if (this.closed) return;
    this.closed = true; this.epoch++; this.authority++;
    clearTimeout(this.timer);
    this.closeMessaging();
    for (const watcher of this.watchers.values()) watcher.stop();
    this.watchers.clear(); this.unread.clear();
    for (const stop of this.stops.splice(0)) stop();
    this.listeners.clear();
  }

  // ── Internals ──

  private changed(event: { invalidated?: boolean }): void {
    if (this.closed) return;
    if (event.invalidated) {
      // Hide the selection's detail now, before refetching. Open conversations keep their own access checks.
      this.epoch++; this.authority++;
      this.detail = null; this.teams = []; this.link = ''; this.listed = false;
      for (const watcher of this.watchers.values()) watcher.stop();
      this.watchers.clear(); this.unread.clear();
      this.publish();
    }
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.refresh(), this.refreshDelay);
  }

  private dropSelection(): void { this.selection = ''; this.detail = null; this.link = ''; }

  /** Make the controller follow the selected team. Returns whether a new team was chosen and still needs its detail. */
  private syncSelection(): boolean {
    let chose = false;
    if (this.selection && !this.teams.some(team => team.id === this.selection)) { this.directories.delete(this.selection); this.dropSelection(); }
    if (!this.selection && this.teams.length) {
      this.selection = this.teams.find(team => team.id === this.initialTeamId)?.id ?? this.teams[0]!.id;
      chose = true;
    }
    this.initialTeamId = '';
    const summary = this.teams.find(team => team.id === this.selection);
    if (!summary) this.closeMessaging();
    else if (this.messagingTeam?.id !== summary.id || this.messagingTeam.generation !== summary.generation) this.openMessaging(summary);
    else this.messagingTeam = summary;
    this.reconcileWatchers();
    return chose;
  }

  private openMessaging(team: TeamSummary): void {
    this.closeMessaging();
    const scope = { teamId: team.id, generation: team.generation };
    const messaging = new TeamMessaging(this.api, this.storage(), this.uid, scope);
    const stop = messaging.onChanged(() => { if (this.messaging === messaging) this.publish(); });
    const online = () => { void messaging.refresh(); void this.refresh(); };
    const focus = () => { void messaging.refresh(); if (this.error) void this.refresh(); };
    this.environment?.addEventListener('online', online); this.environment?.addEventListener('focus', focus);
    this.releaseMessaging = () => { stop(); messaging.close(); this.environment?.removeEventListener('online', online); this.environment?.removeEventListener('focus', focus); };
    const botCards = new BotCards(this.api, scope, bots => {
      if (this.botCards !== botCards) return;
      this.bots = bots ?? EMPTY_BOTS; this.botsUnverified = !bots; this.botsLoaded = this.botsLoaded || !!bots; this.publish();
    });
    this.messaging = messaging; this.messagingTeam = team; this.botCards = botCards;
    this.bots = EMPTY_BOTS; this.botsLoaded = false; this.botsUnverified = false; this.actionError = '';
    void messaging.start().catch(() => { if (this.messaging === messaging) { this.actionError = 'STORAGE_FAILED'; this.publish(); } });
    const remembered = this.conversationByTeam.get(team.id);
    if (remembered) messaging.select(remembered);
    void botCards.refresh();
  }

  private closeMessaging(): void {
    const model = this.messaging, team = this.messagingTeam;
    if (model && team) {
      this.unread.set(team.id, totalUnreadOf(model.state.conversations));
      if (model.selected) this.conversationByTeam.set(team.id, model.selected);
    }
    this.releaseMessaging?.();
    this.botCards?.close();
    this.releaseMessaging = undefined; this.botCards = undefined; this.messaging = undefined; this.messagingTeam = undefined;
    this.bots = EMPTY_BOTS; this.botsLoaded = false; this.botsUnverified = false;
  }

  /** Keep one directory-only listener per other team, up to the limit; the selected team uses its controller. */
  private reconcileWatchers(): void {
    if (this.closed) return;
    const wanted = new Map(this.teams.filter(team => team.id !== this.selection).slice(0, this.maxWatchedTeams).map(team => [team.id, team] as const));
    for (const [id, watcher] of this.watchers) {
      if (wanted.get(id)?.generation === watcher.generation) continue;
      watcher.stop(); this.watchers.delete(id);
    }
    for (const team of wanted.values()) if (!this.watchers.has(team.id)) this.watch(team);
    for (const id of [...this.unread.keys()]) if (id !== this.selection && !wanted.has(id)) this.unread.delete(id);
  }

  private watch(team: TeamSummary): void {
    const scope = { teamId: team.id, generation: team.generation };
    const watcher = { generation: team.generation, stop: () => {} };
    let rows = new Map<string, TeamConversation>();
    this.watchers.set(team.id, watcher);
    try {
      watcher.stop = this.api.watchMessages(scope, '', snapshot => {
        if (this.closed || this.watchers.get(team.id) !== watcher) return;
        if (snapshot.uid !== this.uid || snapshot.teamId !== team.id || snapshot.generation !== team.generation) return;
        if (snapshot.kind === 'directory') rows = new Map(snapshot.conversations.map(value => [value.id, value]));
        else if (snapshot.kind === 'read') {
          const row = rows.get(snapshot.conversationId);
          if (!row) return;
          rows.set(row.id, { ...row, lastReadSeq: Math.max(row.lastReadSeq, snapshot.lastReadSeq) });
        } else if (snapshot.kind === 'error') {
          // Revoked or failed: stop counting; the next list refresh restarts the listener if the team is still there.
          watcher.stop(); this.watchers.delete(team.id); this.unread.delete(team.id); this.publish();
          return;
        } else return;
        this.unread.set(team.id, totalUnreadOf([...rows.values()])); this.publish();
      });
    } catch { this.watchers.delete(team.id); }
  }

  private publish(): void {
    if (this.closed) return;
    this.current = this.build();
    for (const listener of [...this.listeners]) listener();
  }

  private build(): TeamsSnapshot {
    const model = this.messaging, selected = model?.selected ?? '';
    const conversations = model?.state.conversations ?? EMPTY_CONVERSATIONS;
    const outbox = model?.state.outbox ?? EMPTY_OUTBOX;
    if (!this.outboxFor || this.outboxFor.outbox !== outbox || this.outboxFor.id !== selected) {
      this.outboxFor = { outbox, id: selected, value: outbox.length ? outbox.filter(value => pendingPayload(value).conversationId === selected) : EMPTY_OUTBOX };
    }
    const unreadByTeam: Record<string, number> = Object.fromEntries(this.unread);
    if (model && this.messagingTeam) unreadByTeam[this.messagingTeam.id] = totalUnreadOf(conversations);
    const team = this.teams.find(value => value.id === this.selection) ?? (this.selection && this.messagingTeam?.id === this.selection ? this.messagingTeam : undefined);
    return {
      uid: this.uid, authority: this.authority, loading: !this.listed, busy: this.busy, messageBusy: this.messageBusy,
      error: this.error, loadError: this.loadError, actionError: this.actionError,
      teams: this.teams, selectedTeamId: this.selection, team, detail: this.detail,
      detailLoading: !!this.selection && this.detail === null && this.refreshing > 0,
      directory: this.detail?.members ?? this.directories.get(this.selection) ?? EMPTY_MEMBERS,
      inviteLink: this.link, messaging: model, conversations, selectedConversationId: selected,
      conversation: conversations.find(value => value.id === selected),
      messages: model?.state.threads[selected]?.messages ?? EMPTY_MESSAGES,
      pending: this.outboxFor.value, creation: model?.state.creation, offline: model?.offline ?? false,
      messagingConnecting: model?.connecting ?? !!this.selection, messagingError: model?.error ?? '',
      bots: this.bots, botsLoaded: this.botsLoaded, botsUnverified: this.botsUnverified,
      unreadByTeam, totalUnread: Object.values(unreadByTeam).reduce((sum, value) => sum + value, 0),
    };
  }
}
