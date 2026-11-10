import { TeamsApiError } from './index.js';
import type { TeamConversation } from './messages.js';

// ─── Wire and card types (docs/FRIENDS-CLIENTS.md, appendix A) ──────────────

export type Relation = 'none' | 'friends' | 'incoming' | 'outgoing';
export type CardState = 'friends' | 'incoming' | 'outgoing';
export type Pointer = NonNullable<TeamConversation['lastMessage']>;

/** `users/{uid}/friends/{other}` as plain values: `since` in epoch milliseconds, `lastMessage` the whole Teams conversation pointer. */
export interface FriendCard {
  uid: string; state: CardState; displayName: string; handle?: string; note?: string; since: number;
  /** Both only when `friends`; the generation is read from the card, never assumed. */
  teamId?: string; generation?: number;
  /** The chat summary, only when `friends`. Each part is absent until the first message or read mark. */
  lastSeq?: number; lastReadSeq?: number; lastChangeSeq?: number; lastMessage?: Pointer;
}
/** `users/{uid}/blocks/{other}`; `at` in epoch milliseconds. The name is the one the person had when blocked. */
export interface BlockEntry { uid: string; at: number; displayName: string; handle?: string }
export interface FriendProfile { uid: string; displayName: string; handle?: string; findByHandle: boolean; findByEmail: boolean; friendCode: { code: string; link: string } | null }
export interface LookupResult { uid: string; displayName: string; handle?: string; relation: Relation }

export type FriendsCommand =
  | { action: 'profile'; displayName?: string; handle?: string; findByHandle?: boolean; findByEmail?: boolean }
  | { action: 'lookup'; handle: string } | { action: 'lookup'; email: string } | { action: 'lookup'; code: string } | { action: 'lookup'; teamId: string; uid: string }
  | { action: 'request'; via: 'handle'; handle: string; requestId: string; note?: string }
  | { action: 'request'; via: 'email'; email: string; requestId: string; note?: string }
  | { action: 'request'; via: 'team'; teamId: string; uid: string; requestId: string; note?: string }
  | { action: 'redeem'; code: string; requestId: string }
  | { action: 'accept' | 'decline' | 'cancel' | 'remove' | 'block' | 'unblock'; otherUid: string }
  | { action: 'rotateCode' | 'revokeCode' };
interface Ok { ok: true }
export interface FriendsResults {
  profile: FriendProfile; lookup: LookupResult;
  request: { uid: string; state: 'outgoing' } | { uid: string; state: 'friends'; teamId: string };
  redeem: { state: 'friends'; uid: string; teamId: string };
  accept: { state: 'friends'; teamId: string };
  decline: Ok; cancel: Ok; remove: Ok; block: Ok; unblock: Ok;
  rotateCode: { code: string; link: string }; revokeCode: Ok;
}
export type FriendsResult<C extends FriendsCommand> = FriendsResults[C['action']];

/**
 * What the Friends views need from an account: HTTP actions and the two owner-only listeners. Web talks to
 * Firebase itself; Desktop's main process owns both and the renderer reaches them over IPC.
 */
export interface FriendsApi {
  /** `GET /v1/friends/profile`: creates the profile and the first friend code on first use. */
  profile(): Promise<FriendProfile>;
  execute<C extends FriendsCommand>(command: C): Promise<FriendsResult<C>>;
  /** The whole `users/{uid}/friends` collection on every change. `failed` (an error code) ends the watch. Returns the stop function. */
  watchCards(next: (cards: FriendCard[]) => void, failed: (code: string) => void): () => void;
  watchBlocks(next: (blocks: BlockEntry[]) => void, failed: (code: string) => void): () => void;
  /** One-shot read of a friend's `profiles/{uid}` (D10). Resolves `null` when it cannot be read. */
  readProfile(uid: string): Promise<{ displayName?: string; handle?: string } | null>;
}
/** The same API as `FriendsApi` for a renderer whose account is bound by the caller (Desktop IPC). */
export interface DesktopFriendsApi {
  profile(uid: string): Promise<FriendProfile>;
  execute<C extends FriendsCommand>(uid: string, command: C): Promise<FriendsResult<C>>;
  watchCards(uid: string, next: (cards: FriendCard[]) => void, failed: (code: string) => void): () => void;
  watchBlocks(uid: string, next: (blocks: BlockEntry[]) => void, failed: (code: string) => void): () => void;
  readProfile(uid: string, friendUid: string): Promise<{ displayName?: string; handle?: string } | null>;
}

/** `teamsTransport` for `/v1/friends`: the profile is `friends('/profile')`, every action is `friends('', command)`. */
export function friendsTransport(base: string, token: () => Promise<string>, fetcher: typeof fetch = fetch) {
  return async <T>(path = '', body?: unknown): Promise<T> => {
    const credential = await token();
    const response = await fetcher(`${base}/v1/friends${path}`, { method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: `Bearer ${credential}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(20000) });
    const value = await response.json();
    if (!response.ok) throw new TeamsApiError(value?.error?.code ?? 'SERVICE_UNAVAILABLE', response.status);
    return value as T;
  };
}

// ─── Limits and ids ─────────────────────────────────────────────────────────

/** The only conversation of a pair team (D9). */
export const FRIEND_CHAT = 'friend_chat';
export const NAME_MAX = 80, NOTE_MAX = 200, SECTION_ROWS = 20, BADGE_CAP = 99;
const HANDLE = /^[A-Za-z0-9_]{3,24}$/u;
const CODE = /^[A-Za-z0-9_-]{32}$/u;
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/u;

export const handleValid = (handle: string): boolean => HANDLE.test(handle);
/** Lengths are UTF-16 code units, as the server counts them. */
export const nameValid = (name: string): boolean => name.trim().length >= 1 && name.trim().length <= NAME_MAX;
export const noteValid = (note: string): boolean => note.length <= NOTE_MAX;
/** Minted when the person taps the confirming button (D18). */
export const newRequestId = (): string => crypto.randomUUID();

// ─── Order and filter (D5) ──────────────────────────────────────────────────

/** NFKC, lowercase, NFD, no combining marks, NFC. */
export const foldName = (text: string): string => text.normalize('NFKC').toLowerCase().normalize('NFD').replace(/[̀-ͯ]/gu, '').normalize('NFC');
/** Names compare ignoring case and accents, so "Sam" and "sam" tie and the uid decides. */
export const friendCollator = (locale: string): Intl.Collator => new Intl.Collator(locale, { sensitivity: 'base' });

/** `lastMessage.at` in whole epoch milliseconds; a pointer without a readable time counts as none. */
const activity = (card: Pick<FriendCard, 'lastMessage'>): number | undefined => {
  const ms = card.lastMessage?.at ? Date.parse(card.lastMessage.at) : Number.NaN;
  return Number.isNaN(ms) ? undefined : ms;
};
// Uids are ASCII ([A-Za-z0-9_-]), where UTF-16 order is code point order (and "n10" sorts before "n9").
const byUid = (a: string, b: string): number => a < b ? -1 : a > b ? 1 : 0;

type Named = Pick<FriendCard, 'uid' | 'displayName' | 'lastMessage'>;
/** Friends: those with a message first, the newer first; then by name; then by uid. */
export const compareFriends = (collator: Intl.Collator) => (a: Named, b: Named): number => {
  const x = activity(a), y = activity(b);
  if (x !== undefined && y !== undefined && x !== y) return y - x;
  if ((x === undefined) !== (y === undefined)) return x === undefined ? 1 : -1;
  return collator.compare(a.displayName, b.displayName) || byUid(a.uid, b.uid);
};
const rank: Record<CardState, number> = { incoming: 0, outgoing: 1, friends: 2 };
/** Requests: incoming before outgoing, then the newest first, then by uid. */
export const compareRequests = (a: Pick<FriendCard, 'uid' | 'state' | 'since'>, b: Pick<FriendCard, 'uid' | 'state' | 'since'>): number =>
  rank[a.state] - rank[b.state] || b.since - a.since || byUid(a.uid, b.uid);
/** Blocked people (D12): the newest block first. */
export const compareBlocks = (a: Pick<BlockEntry, 'uid' | 'at'>, b: Pick<BlockEntry, 'uid' | 'at'>): number => b.at - a.at || byUid(a.uid, b.uid);
/** The whole collection: incoming requests, outgoing requests, then friends. */
export const compareCards = (collator: Intl.Collator) => {
  const friends = compareFriends(collator);
  return (a: FriendCard, b: FriendCard): number => a.state === 'friends' && b.state === 'friends' ? friends(a, b) : compareRequests(a, b);
};

export interface FriendSections { received: FriendCard[]; sent: FriendCard[]; friends: FriendCard[] }
/** The Requests and Friends sections of the root (D3, D4) as slices of the sorted collection. */
export function sectionCards(cards: readonly FriendCard[], collator: Intl.Collator): FriendSections {
  const sorted = [...cards].sort(compareCards(collator));
  return { received: sorted.filter(card => card.state === 'incoming'), sent: sorted.filter(card => card.state === 'outgoing'), friends: sorted.filter(card => card.state === 'friends') };
}

/** Contains-match on the folded name or handle; a leading `@` is dropped; empty matches everyone. */
export function matchesFilter(card: Pick<FriendCard, 'displayName' | 'handle'>, query: string): boolean {
  let needle = foldName(query.trim());
  if (needle.startsWith('@')) needle = needle.slice(1).trim();
  return !needle || foldName(card.displayName).includes(needle) || foldName(card.handle ?? '').includes(needle);
}

// ─── Unread and badge (D13, D14) ────────────────────────────────────────────

/** `max(0, lastSeq - lastReadSeq)`; a missing counter is 0 and `lastChangeSeq` never counts. */
export const unreadOf = (card: Pick<FriendCard, 'lastSeq' | 'lastReadSeq'>): number => Math.max(0, (card.lastSeq ?? 0) - (card.lastReadSeq ?? 0));
/** Incoming requests plus the unread of the friends cards. `unread` lets a host count an open chat as read. */
export const badgeCount = (cards: readonly Pick<FriendCard, 'state' | 'lastSeq' | 'lastReadSeq'>[], unread: (card: Pick<FriendCard, 'lastSeq' | 'lastReadSeq'>) => number = unreadOf): number =>
  cards.reduce((sum, card) => sum + (card.state === 'incoming' ? 1 : card.state === 'friends' ? unread(card) : 0), 0);
/** Nothing at 0, the number up to 99, `99+` from 100. */
export const badgeText = (count: number): string | null => count <= 0 ? null : count > BADGE_CAP ? `${BADGE_CAP}+` : String(count);

// ─── One field, three kinds (D6, D7) ────────────────────────────────────────

export type Proof = { kind: 'handle'; handle: string } | { kind: 'email'; email: string } | { kind: 'code'; code: string } | { kind: 'team'; teamId: string; uid: string };
export type Query = Exclude<Proof, { kind: 'team' }> | { kind: 'invalid' };

/** The friend code of a bare code or of a friend link, or null. A link has exactly the one `code` parameter. */
export function friendCodeOf(input: string): string | null {
  const value = input.trim();
  if (CODE.test(value)) return value;
  try {
    const url = new URL(value);
    if (url.username || url.password || url.port || url.hash) return null;
    if (!(url.protocol === 'lingxi:' && url.hostname === 'friend' && url.pathname === '/add')
      && !(url.protocol === 'https:' && url.hostname === 'lingxios.net' && ['/open/friend/add', '/friends/add'].includes(url.pathname))) return null;
    const entries = [...url.searchParams];
    return entries.length === 1 && entries[0]![0] === 'code' && CODE.test(entries[0]![1]) ? entries[0]![1] : null;
  } catch { return null; }
}

/** Classify the trimmed text, first match wins: code, email, handle, else invalid. */
export function classifyQuery(input: string): Query {
  const text = input.trim();
  const code = friendCodeOf(text);
  if (code) return { kind: 'code', code };
  if (text.indexOf('@') > 0) {
    const email = text.toLowerCase();
    return email.length <= 254 && EMAIL.test(email) ? { kind: 'email', email } : { kind: 'invalid' };
  }
  const handle = text.startsWith('@') ? text.slice(1) : text;
  return HANDLE.test(handle) ? { kind: 'handle', handle } : { kind: 'invalid' };
}

type LookupBody = Extract<FriendsCommand, { action: 'lookup' }>;
type ConfirmBody = Extract<FriendsCommand, { action: 'request' | 'redeem' }>;
/** The first call: one proof, whose result only fills the card. */
export function lookupBody(proof: Proof): LookupBody {
  switch (proof.kind) {
    case 'handle': return { action: 'lookup', handle: proof.handle };
    case 'email': return { action: 'lookup', email: proof.email };
    case 'code': return { action: 'lookup', code: proof.code };
    case 'team': return { action: 'lookup', teamId: proof.teamId, uid: proof.uid };
  }
}
/** The call after the person confirms. A code always redeems and takes no note. */
export function confirmBody(proof: Proof, requestId: string, note = ''): ConfirmBody {
  const extra = note.trim() ? { note: note.trim() } : {};
  switch (proof.kind) {
    case 'handle': return { action: 'request', via: 'handle', handle: proof.handle, requestId, ...extra };
    case 'email': return { action: 'request', via: 'email', email: proof.email, requestId, ...extra };
    case 'team': return { action: 'request', via: 'team', teamId: proof.teamId, uid: proof.uid, requestId, ...extra };
    case 'code': return { action: 'redeem', code: proof.code, requestId };
  }
}
/** What the result card offers for a relation (D6). */
export const relationAction = (relation: Relation): 'add' | 'accept' | 'sent' | 'friends' => ({ none: 'add', incoming: 'accept', outgoing: 'sent', friends: 'friends' } as const)[relation];

// ─── Errors (D18) ───────────────────────────────────────────────────────────

const ERROR_KEYS = new Map([
  ['NOT_FOUND', 'friends_error_not_found'], ['RATE_LIMITED', 'friends_error_rate_limited'], ['FRIEND_LIMIT', 'friends_error_friend_limit'],
  ['REQUEST_LIMIT', 'friends_error_request_limit'], ['HANDLE_TAKEN', 'friends_error_handle_taken'], ['REQUEST_UNAVAILABLE', 'friends_error_request_unavailable'],
  ['STALE_REQUEST', 'friends_error_stale_request'], ['SIGN_IN_REQUIRED', 'friends_error_session'], ['INVALID_SESSION', 'friends_error_session'],
  ['SERVICE_UNAVAILABLE', 'friends_error_network'], ['INVALID_INPUT', 'friends_error_invalid_input'], ['INVALID_JSON', 'friends_error_invalid_input'],
  ['MEMBERSHIP_REQUIRED', 'friends_chat_unavailable'], ['STALE_MEMBERSHIP', 'friends_chat_unavailable'], ['CONVERSATION_REQUIRED', 'friends_chat_unavailable'],
]);
export const friendsErrorKey = (code: string): string => ERROR_KEYS.get(code) ?? 'friends_error_generic';
/** The chat's ended-friendship codes (D9). */
export const chatEnded = (code: string): boolean => friendsErrorKey(code) === 'friends_chat_unavailable';
/** A request is retried with the same id only when its outcome is unknown: no answer, a timeout, or SERVICE_UNAVAILABLE. */
export const definitive = (code: string): boolean => code !== 'SERVICE_UNAVAILABLE';

// ─── Links (D16) ────────────────────────────────────────────────────────────

export type FriendLink = { kind: 'add'; code: string } | { kind: 'requests' } | { kind: 'chat'; uid: string };
/** The Friends destination of an already parsed canonical link, or null for any other route. */
export function friendLink(target: { route: string; params: Record<string, string> }): FriendLink | null {
  if (target.route === 'friend/add') return { kind: 'add', code: target.params.code! };
  if (target.route === 'friend/requests') return { kind: 'requests' };
  if (target.route === 'friend/chat') return { kind: 'chat', uid: target.params.uid! };
  return null;
}
