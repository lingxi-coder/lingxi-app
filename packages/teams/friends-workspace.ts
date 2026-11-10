import type { TeamMember, TeamsApi } from './index.js';
import {
  badgeCount, compareBlocks, confirmBody, definitive, FRIEND_CHAT, lookupBody, newRequestId,
  type BlockEntry, type FriendCard, type FriendProfile, type FriendsApi, type FriendsCommand, type FriendsResult, type LookupResult, type Proof,
} from './friends.js';
import { TeamsWorkspace, type TeamsWorkspaceOptions } from './workspace.js';

/** The pages of the Friends main area (D1). The chat is a page of its own, never mixed with the others. */
export type FriendsPage = 'home' | 'chat' | 'add' | 'profile' | 'blocked';

/** An open friend chat (D9). */
export interface FriendChat {
  uid: string;
  /** The friend's name for the header: refreshed by D10 if it was, else the card's; the last known one after the friendship ended. */
  name: string;
  handle?: string;
  /** The pair-pinned workspace. Absent while the friend's card has not arrived and after the friendship ended. */
  workspace: TeamsWorkspace | undefined;
  /** The friendship is over (the card is gone), or the person was never a friend: the view says `friends_chat_unavailable`. */
  ended: boolean;
  /** The friendship was just made (D7): the view says `friends_now_friends(name)` once, and the friend's card is awaited. */
  joined: boolean;
}

export interface FriendsSnapshot {
  readonly uid: string;
  readonly page: FriendsPage;
  /** Waiting for the first cards snapshot. */
  readonly loading: boolean;
  /** Error code of the cards listener; empty when it runs. */
  readonly loadError: string;
  /**
   * The whole `users/{uid}/friends` collection as the views show it: unsorted (the views sort with their locale's collator),
   * with names refreshed by D10 and the open chat counted as read (D13).
   */
  readonly cards: FriendCard[];
  /** `users/{uid}/blocks`, newest first. */
  readonly blocks: BlockEntry[];
  readonly blocksLoading: boolean;
  readonly blocksError: string;
  /** D14: incoming requests plus the unread of the friends. Render it with `badgeText`. */
  readonly badge: number;
  /** In memory only (D2). Absent until `GET /v1/friends/profile` answered. */
  readonly profile: FriendProfile | undefined;
  readonly profileLoading: boolean;
  readonly profileError: string;
  readonly chat: FriendChat | undefined;
  /** Increments when the Requests section should be scrolled into view (a `friend/requests` link). */
  readonly requestsFocus: number;
}

export interface FriendsWorkspaceOptions {
  /** Options for each friend chat's pair-pinned {@link TeamsWorkspace} (command storage, environment). */
  chat?: Omit<TeamsWorkspaceOptions, 'pair'>;
}

export type ProfileChanges = Omit<Extract<FriendsCommand, { action: 'profile' }>, 'action'>;
/** What a confirmed Add friend did: a request is waiting, or the two are friends and the chat is opening. */
export type Confirmed = { error: string } | { done: 'sent' | 'friends'; name: string };

/** An error code, or `SERVICE_UNAVAILABLE` for a failure that has none (no answer, a timeout). */
const codeOf = (error: unknown): string => error instanceof Error && /^[A-Z][A-Z_]{2,59}$/u.test(error.message) ? error.message : 'SERVICE_UNAVAILABLE';

interface Chat extends FriendChat { teamId?: string; generation?: number; names?: string }

/**
 * Everything the Friends views need from one signed-in account: the two owner-only listeners of D2 (and nothing per friend),
 * the profile, the page being shown, the open chat's pair-pinned {@link TeamsWorkspace}, and the actions of D3 to D15.
 * No React: `useSyncExternalStore(workspace.subscribe, workspace.getSnapshot)`. Actions resolve an error code, empty on
 * success, so a view can put the message under the control that failed. The workspace is closed on sign-out or a change
 * of account, which is all that D19 asks for: nothing here is kept on disk.
 */
export class FriendsWorkspace {
  readonly subscribe = (listener: () => void): (() => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  readonly getSnapshot = (): FriendsSnapshot => this.current;

  private current: FriendsSnapshot;
  private readonly listeners = new Set<() => void>();
  private closed = false;
  private watching = 0;
  private stops: (() => void)[] = [];
  private raw: FriendCard[] = [];
  private cards: FriendCard[] = [];
  private loaded = false;
  private loadError = '';
  private blocks: BlockEntry[] = [];
  private blocksLoaded = false;
  private blocksError = '';
  private profile: FriendProfile | undefined;
  private profileLoading = false;
  private profileError = '';
  private profileRun = 0;
  private page: FriendsPage = 'home';
  private requestsFocus = 0;
  private chat: Chat | undefined;
  // D10: the profile's values for a friend, for the rest of the session. Cards are never written.
  private readonly names = new Map<string, { displayName?: string; handle?: string }>();
  // D18: the id of the confirmation in flight, kept only to retry the same one after an unknown outcome.
  private attempt: { key: string; id: string } | undefined;

  constructor(readonly api: FriendsApi, readonly teams: TeamsApi, readonly uid: string, private readonly options: FriendsWorkspaceOptions = {}) {
    this.current = this.build();
    this.watch();
    void this.refreshProfile();
  }

  // ── Data ──

  /** `GET /v1/friends/profile`: on the first open and on every open of Add friend or Profile settings (D2). A failure never blocks the list. */
  readonly refreshProfile = async (): Promise<void> => {
    if (this.closed) return;
    const run = ++this.profileRun;
    this.profileLoading = true; this.profileError = ''; this.publish();
    try {
      const profile = await this.api.profile();
      if (this.closed || run !== this.profileRun) return;
      this.profile = profile;
    } catch (error) {
      if (this.closed || run !== this.profileRun) return;
      this.profileError = codeOf(error);
    }
    this.profileLoading = false; this.syncChat(); this.publish();
  };

  /** Restart the listeners after a failure and read the profile again. */
  readonly retry = (): void => {
    if (this.closed) return;
    this.watch(); this.publish();
    if (!this.profile) void this.refreshProfile();
  };

  // ── Pages ──

  /** Show a page of the main area. Leaving the chat closes it. */
  readonly show = (page: Exclude<FriendsPage, 'chat'>): void => {
    if (this.closed) return;
    this.dropChat(); this.page = page; this.settle();
  };

  /** Show the root and ask its Requests section to scroll into view. */
  readonly showRequests = (): void => { this.requestsFocus++; this.show('home'); };

  /**
   * Open a friend's chat. With `joined` (the friendship was just made) the friend's card is awaited; otherwise a
   * person who is not a friend, once the first cards have arrived, gets the unavailable page.
   */
  readonly openChat = (uid: string, joined = false): void => {
    if (this.closed) return;
    if (this.chat?.uid !== uid) { this.dropChat(); this.chat = { uid, name: '', workspace: undefined, ended: false, joined }; }
    this.page = 'chat'; this.settle();
  };

  // ── Actions ──

  /** D6/D7/D15: look one proof up. The result only fills the card. */
  readonly lookup = async (proof: Proof): Promise<{ result: LookupResult } | { error: string }> => {
    const outcome = await this.call(lookupBody(proof));
    return 'error' in outcome ? outcome : { result: outcome.value };
  };

  /**
   * The confirming tap of D7. A request or redeem carries a `requestId` minted here and reused only to retry this very
   * confirmation after an unknown outcome. A found `incoming` request is accepted instead (never for a code).
   * A made friendship opens the chat once the card arrives.
   */
  readonly confirm = async (proof: Proof, found: LookupResult, note: string): Promise<Confirmed> => {
    const accept = found.relation === 'incoming' && proof.kind !== 'code';
    const key = JSON.stringify([proof, found.uid, accept ? '' : note.trim()]);
    const id = this.attempt?.key === key ? this.attempt.id : newRequestId();
    this.attempt = accept ? undefined : { key, id };
    const outcome = accept ? await this.call({ action: 'accept', otherUid: found.uid }) : await this.call(confirmBody(proof, id, note));
    if ('error' in outcome) {
      if (definitive(outcome.error)) this.attempt = undefined;
      return outcome;
    }
    this.attempt = undefined;
    if ((outcome.value as { state: string }).state !== 'friends') return { done: 'sent', name: found.displayName };
    this.openChat(found.uid, true);
    return { done: 'friends', name: found.displayName };
  };

  /** D4: answer a request from its row. The listener removes or changes the row. */
  readonly respond = async (uid: string, action: 'accept' | 'decline' | 'cancel'): Promise<string> => {
    const outcome = await this.call({ action, otherUid: uid });
    return 'error' in outcome ? outcome.error : '';
  };

  /** D10: end the friendship, or end it and block the person. On success the list is shown. */
  readonly leave = async (uid: string, action: 'remove' | 'block'): Promise<string> => {
    const outcome = await this.call({ action, otherUid: uid });
    if ('error' in outcome) return outcome.error;
    this.show('home');
    return '';
  };

  readonly unblock = async (uid: string): Promise<string> => {
    const outcome = await this.call({ action: 'unblock', otherUid: uid });
    return 'error' in outcome ? outcome.error : '';
  };

  /** D6: a new friend link replaces the old one (or creates the first). */
  readonly rotateCode = async (): Promise<string> => {
    const outcome = await this.call({ action: 'rotateCode' });
    if ('error' in outcome) return outcome.error;
    if (this.profile) this.profile = { ...this.profile, friendCode: outcome.value };
    this.publish();
    return '';
  };

  /** D6: turn the friend link off. */
  readonly revokeCode = async (): Promise<string> => {
    const outcome = await this.call({ action: 'revokeCode' });
    if ('error' in outcome) return outcome.error;
    if (this.profile) this.profile = { ...this.profile, friendCode: null };
    this.publish();
    return '';
  };

  /** D11: one `profile` action with only the changed fields. */
  readonly saveProfile = async (changes: ProfileChanges): Promise<string> => {
    const outcome = await this.call({ action: 'profile', ...changes });
    if ('error' in outcome) return outcome.error;
    this.profile = outcome.value; this.profileError = '';
    this.syncChat(); this.publish();
    return '';
  };

  /** D10: read the friend's profile once and, if its name or username differ from the card, use them for the rest of the session. */
  readonly refreshFriend = async (uid: string): Promise<void> => {
    const read = await this.api.readProfile(uid).catch(() => null);
    const card = this.raw.find(value => value.uid === uid);
    if (this.closed || !read || !card) return;
    const next = { ...(read.displayName && read.displayName !== card.displayName ? { displayName: read.displayName } : {}), ...(read.handle && read.handle !== card.handle ? { handle: read.handle } : {}) };
    if (!Object.keys(next).length) return;
    this.names.set(uid, { ...this.names.get(uid), ...next });
    this.settle();
  };

  /** Stops both listeners and the open chat. The workspace is unusable afterwards. */
  close(): void {
    if (this.closed) return;
    this.closed = true; this.watching++; this.profileRun++;
    for (const stop of this.stops.splice(0)) stop();
    this.dropChat();
    this.listeners.clear();
  }

  // ── Internals ──

  private async call<C extends FriendsCommand>(command: C): Promise<{ value: FriendsResult<C> } | { error: string }> {
    if (this.closed) return { error: 'INVALID_SESSION' };
    try {
      const value = await this.api.execute(command);
      return this.closed ? { error: 'INVALID_SESSION' } : { value };
    } catch (error) { return { error: codeOf(error) }; }
  }

  private watch(): void {
    for (const stop of this.stops.splice(0)) stop();
    const run = ++this.watching;
    this.loaded = false; this.loadError = ''; this.blocksLoaded = false; this.blocksError = '';
    const current = () => !this.closed && run === this.watching;
    this.stops.push(
      this.api.watchCards(cards => { if (!current()) return; this.raw = cards; this.loaded = true; this.loadError = ''; this.settle(); },
        code => { if (current()) { this.loaded = true; this.loadError = code; this.publish(); } }),
      this.api.watchBlocks(blocks => { if (!current()) return; this.blocks = [...blocks].sort(compareBlocks); this.blocksLoaded = true; this.blocksError = ''; this.publish(); },
        code => { if (current()) { this.blocksLoaded = true; this.blocksError = code; this.publish(); } }),
    );
  }

  /** The cards, the names and the open chat changed: rebuild what the views show and keep the chat in step. */
  private settle(): void {
    this.cards = this.raw.map(card => {
      const named = this.names.get(card.uid);
      const open = this.chat?.uid === card.uid;
      return named || open ? { ...card, ...named, ...(open ? { lastReadSeq: card.lastSeq } : {}) } : card;
    });
    this.syncChat();
    this.publish();
  }

  private dropChat(): void {
    this.chat?.workspace?.close();
    this.chat = undefined;
  }

  /** Open, refresh or end the chat's pair workspace to match the friend's card (D9). */
  private syncChat(): void {
    const chat = this.chat;
    if (!chat || chat.ended) return;
    const card = this.cards.find(value => value.uid === chat.uid);
    if (card) { chat.name = card.displayName; chat.handle = card.handle; }
    if (card?.state !== 'friends' || card.teamId === undefined || card.generation === undefined) {
      // Before the first snapshot, or while a just-made friendship's card is on its way, there is nothing to decide.
      if (!this.loaded || (chat.joined && !chat.workspace)) return;
      chat.workspace?.close();
      chat.workspace = undefined; chat.ended = true;
      return;
    }
    const names = JSON.stringify([this.profile?.displayName, card.displayName]);
    const members = (): TeamMember[] => [
      { uid: this.uid, displayName: this.profile?.displayName ?? '', role: 'member', generation: card.generation! },
      { uid: card.uid, displayName: card.displayName, role: 'member', generation: card.generation! },
    ];
    if (chat.workspace && chat.teamId === card.teamId && chat.generation === card.generation) {
      if (names !== chat.names) { chat.names = names; chat.workspace.setPairMembers(members()); }
      return;
    }
    chat.workspace?.close();
    chat.teamId = card.teamId; chat.generation = card.generation; chat.names = names;
    chat.workspace = new TeamsWorkspace(this.teams, this.uid, { ...this.options.chat,
      pair: { team: { id: card.teamId, name: card.displayName, memberCount: 2, role: 'member', generation: card.generation }, members: members(), conversationId: FRIEND_CHAT } });
  }

  private publish(): void {
    if (this.closed) return;
    this.current = this.build();
    for (const listener of [...this.listeners]) listener();
  }

  private build(): FriendsSnapshot {
    const chat = this.chat;
    return {
      uid: this.uid, page: this.page, loading: !this.loaded, loadError: this.loadError, cards: this.cards, blocks: this.blocks,
      blocksLoading: !this.blocksLoaded, blocksError: this.blocksError, badge: badgeCount(this.cards),
      profile: this.profile, profileLoading: this.profileLoading, profileError: this.profileError,
      chat: chat && { uid: chat.uid, name: chat.name, handle: chat.handle, workspace: chat.workspace, ended: chat.ended, joined: chat.joined },
      requestsFocus: this.requestsFocus,
    };
  }
}
