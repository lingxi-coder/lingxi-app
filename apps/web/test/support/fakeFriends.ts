import type { BlockEntry, FriendCard, FriendProfile, FriendsApi, FriendsCommand, FriendsResult, LookupResult, TeamOffer } from '../../../../packages/teams/friends';
import { TeamsApiError } from '../../../../packages/teams';
import { FriendsWorkspace, type FriendsSnapshot, type FriendsWorkspaceOptions } from '../../../../packages/teams/friends-workspace';
import { FakeTeams, memoryStorage, uid } from './fakeTeams';

export { until, settle, uid } from './fakeTeams';

/** `f_` and forty hex digits: the id of a pair team. */
export const pairTeamId = (other: string): string => `f_${[...other].map(letter => letter.charCodeAt(0).toString(16).padStart(2, '0')).join('').padEnd(40, '0').slice(0, 40)}`;
const NAME = (id: string) => id[0]!.toUpperCase() + id.slice(1);
/** A card as `users/{uid}/friends/{other}` holds it, with a pair team when the two are friends. */
export function card(id: string, more: Partial<FriendCard> = {}): FriendCard {
  const state = more.state ?? 'friends';
  return { uid: id, state, displayName: NAME(id), since: Date.UTC(2026, 10, 1), ...(state === 'friends' ? { teamId: pairTeamId(id), generation: 1 } : {}), ...more };
}
export const block = (id: string, at: number, more: Partial<BlockEntry> = {}): BlockEntry => ({ uid: id, at, displayName: NAME(id), ...more });
/** An offer of team `teamId` from `from`, as `users/{uid}/teamOffers/{teamId}` holds it. */
export const offer = (teamId: string, expiresAt: number, more: Partial<TeamOffer> = {}): TeamOffer => ({ teamId, teamName: `Team ${teamId}`, fromUid: 'bob', fromName: 'Bob', role: 'member', expiresAt, ...more });
export const profile = (more: Partial<FriendProfile> = {}): FriendProfile => ({
  uid, displayName: 'Alice', handle: 'alice_w', findByHandle: true, findByEmail: false,
  friendCode: { code: 'a'.repeat(32), link: `https://lingxios.net/open/friend/add?code=${'a'.repeat(32)}` }, ...more,
});
export const pointer = (seq: number, more: Partial<NonNullable<FriendCard['lastMessage']>> = {}): NonNullable<FriendCard['lastMessage']> => ({
  id: `message-${seq}`, seq, at: new Date(Date.UTC(2026, 10, 2, 10, seq)).toISOString(), author: { kind: 'user', uid: 'bob' }, preview: `Body ${seq}`, ...more,
});

interface Watch<Row> { next(rows: Row[]): void; failed(code: string): void; open: boolean }

/** A scriptable `FriendsApi`: the three listeners are tracked so a test can prove they were released, and every command is recorded. */
export class FakeFriends {
  cards: FriendCard[] = [];
  blocks: BlockEntry[] = [];
  offers: TeamOffer[] = [];
  profile = profile();
  /** Per action, what the server answers; an `Error` is thrown, a function is called with the command. */
  replies = new Map<string, unknown>();
  commands: FriendsCommand[] = [];
  profileCalls = 0;
  profileError: string | undefined;
  readProfiles: string[] = [];
  names = new Map<string, { displayName?: string; handle?: string } | null>();
  readonly cardWatches: Watch<FriendCard>[] = [];
  readonly blockWatches: Watch<BlockEntry>[] = [];
  readonly offerWatches: Watch<TeamOffer>[] = [];
  /** Hold the listeners back: a test delivers the first snapshot itself. */
  quiet = false;

  readonly api: FriendsApi = {
    profile: async () => {
      this.profileCalls++;
      if (this.profileError) throw new TeamsApiError(this.profileError, 503);
      return structuredClone(this.profile);
    },
    execute: async <C extends FriendsCommand>(command: C): Promise<FriendsResult<C>> => {
      this.commands.push(structuredClone(command));
      const reply = this.replies.get(command.action);
      const value = typeof reply === 'function' ? reply(command) : reply;
      if (value instanceof Error) throw value;
      return (value ?? this.defaultReply(command)) as FriendsResult<C>;
    },
    watchCards: (next, failed) => this.watch(this.cardWatches, next, failed, () => this.cards),
    watchBlocks: (next, failed) => this.watch(this.blockWatches, next, failed, () => this.blocks),
    watchOffers: (next, failed) => this.watch(this.offerWatches, next, failed, () => this.offers),
    readProfile: async friend => { this.readProfiles.push(friend); return this.names.get(friend) ?? null; },
  };

  private watch<Row>(list: Watch<Row>[], next: (rows: Row[]) => void, failed: (code: string) => void, rows: () => Row[]): () => void {
    const watch: Watch<Row> = { next, failed, open: true };
    list.push(watch);
    if (!this.quiet) queueMicrotask(() => { if (watch.open) next(structuredClone(rows())); });
    return () => { watch.open = false; };
  }
  private defaultReply(command: FriendsCommand): unknown {
    switch (command.action) {
      case 'request': return { uid: 'via' in command && command.via === 'team' ? command.uid : 'other', state: 'outgoing' };
      case 'redeem': return { state: 'friends', uid: 'other', teamId: pairTeamId('other') };
      case 'accept': return { state: 'friends', teamId: pairTeamId(command.otherUid) };
      case 'rotateCode': return { code: 'b'.repeat(32), link: `https://lingxios.net/open/friend/add?code=${'b'.repeat(32)}` };
      case 'offerTeam': return { offerId: command.teamId, expiresAt: new Date(Date.UTC(2026, 10, 9)).toISOString() };
      case 'acceptOffer': return { teamId: command.offerId, generation: 1, alreadyMember: false };
      case 'profile': return { ...this.profile, ...command };
      default: return { ok: true };
    }
  }
  lookup(result: Partial<LookupResult> & { uid: string }): void {
    this.replies.set('lookup', { displayName: NAME(result.uid), relation: 'none', ...result });
  }
  /** Failing with a code, the way a rejected request does. */
  fail(action: string, code: string, status = 409): void { this.replies.set(action, new TeamsApiError(code, status)); }

  /** The cards listener sends these rows now. */
  emitCards(rows: FriendCard[] = this.cards): void { this.cards = rows; for (const watch of this.cardWatches) if (watch.open) watch.next(structuredClone(rows)); }
  emitBlocks(rows: BlockEntry[] = this.blocks): void { this.blocks = rows; for (const watch of this.blockWatches) if (watch.open) watch.next(structuredClone(rows)); }
  failCards(code = 'SERVICE_UNAVAILABLE'): void { for (const watch of this.cardWatches) if (watch.open) watch.failed(code); }
  failBlocks(code = 'SERVICE_UNAVAILABLE'): void { for (const watch of this.blockWatches) if (watch.open) watch.failed(code); }
  emitOffers(rows: TeamOffer[] = this.offers): void { this.offers = rows; for (const watch of this.offerWatches) if (watch.open) watch.next(structuredClone(rows)); }
  failOffers(code = 'SERVICE_UNAVAILABLE'): void { for (const watch of this.offerWatches) if (watch.open) watch.failed(code); }
  get open(): { cards: number; blocks: number; offers: number } {
    return { cards: this.cardWatches.filter(watch => watch.open).length, blocks: this.blockWatches.filter(watch => watch.open).length, offers: this.offerWatches.filter(watch => watch.open).length };
  }
  executed(action: FriendsCommand['action']): FriendsCommand[] { return this.commands.filter(command => command.action === action); }
}

/** A Friends workspace on fakes, for an account that already has `cards`. */
export function friendsFixture(setup: (fake: FakeFriends, teams: FakeTeams) => void = () => {}, options: FriendsWorkspaceOptions = {}) {
  const fake = new FakeFriends(), teams = new FakeTeams();
  setup(fake, teams);
  const storage = memoryStorage();
  const workspace = new FriendsWorkspace(fake.api, teams.api, uid, { chat: { storage: () => storage, environment: new EventTarget(), refreshDelay: 5 }, ...options });
  return { fake, teams, workspace, storage, snapshot: (): FriendsSnapshot => workspace.getSnapshot() };
}
