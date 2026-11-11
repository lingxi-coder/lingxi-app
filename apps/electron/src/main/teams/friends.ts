import { EventEmitter } from 'node:events';
import { firestoreBlock, firestoreFriendCard, firestoreFriendName, firestoreTeamOffer } from '../../../../../packages/teams/firestore';
import { friendsTransport, type BlockEntry, type FriendCard, type FriendProfile, type FriendsCommand, type FriendsResult, type TeamOffer } from '../../../../../packages/teams/friends';
import type { Account, TeamsService } from './service.js';

const ACTIONS = new Set(['profile', 'lookup', 'request', 'redeem', 'accept', 'decline', 'cancel', 'remove', 'block', 'unblock', 'rotateCode', 'revokeCode',
  'offerTeam', 'acceptOffer', 'declineOffer']);
/** The three collections the renderer may watch: `users/{uid}/friends`, `users/{uid}/blocks` and `users/{uid}/teamOffers`. */
export type FriendsCollection = 'friends' | 'blocks' | 'teamOffers';

/**
 * Friends in the main process: the HTTP actions and the three owner-only listeners of D2 and D28, bound to the signed-in account. Every call
 * names the account it is for and fails with `INVALID_SESSION` once another account is signed in. The listeners run in the
 * Teams service's Firestore session; the renderer receives plain cards, blocks and offers, never the SDK, a credential or a document.
 */
export class FriendsService {
  private readonly events = new EventEmitter();
  private uid?: string;
  private authenticationEpoch: number;
  private epoch = 0;
  private readonly leases = new Map<string, { uid: string; collection: FriendsCollection }>();
  private readonly stops = new Map<string, () => void>();
  private readonly stopAccount: () => void;

  constructor(private readonly account: Account, private readonly teams: Pick<TeamsService, 'firestore'>, private readonly base = 'https://lingxios.net') {
    this.uid = account.snapshot().user?.uid;
    this.authenticationEpoch = account.sessionEpoch?.() ?? 0;
    this.stopAccount = account.onChanged(() => this.syncAccount());
  }

  onRows(callback: (event: { leaseId: string; rows: FriendCard[] | BlockEntry[] | TeamOffer[] }) => void): () => void { this.events.on('rows', callback); return () => { this.events.off('rows', callback); }; }
  onFailed(callback: (event: { leaseId: string; code: string }) => void): () => void { this.events.on('failed', callback); return () => { this.events.off('failed', callback); }; }

  profile(expectedUid: string): Promise<FriendProfile> { return this.request(expectedUid, '/profile'); }

  async execute<C extends FriendsCommand>(expectedUid: string, command: C): Promise<FriendsResult<C>> {
    if (!command || typeof command !== 'object' || !ACTIONS.has(command.action)) throw new Error('INVALID_INPUT');
    return this.request(expectedUid, '', command);
  }

  /** D10: a friend's `profiles/{uid}`, read once; `null` when it cannot be read. */
  async readProfile(expectedUid: string, friendUid: string): Promise<{ displayName?: string; handle?: string } | null> {
    if (!/^[A-Za-z0-9_-]{1,128}$/u.test(friendUid)) return null;
    const epoch = this.begin(expectedUid);
    const { db, sdk } = await this.teams.firestore(expectedUid);
    const profile = firestoreFriendName((await sdk.getDoc(sdk.doc(db, 'profiles', friendUid))).data());
    this.assert(expectedUid, epoch);
    return profile;
  }

  /**
   * Start sending the rows of one collection under `leaseId`. A watch that fails is ended and reported once; the renderer
   * starts a new one. Resolves when the listener runs, or when the lease was released or the account changed meanwhile.
   */
  async watch(leaseId: string, expectedUid: string, collection: FriendsCollection): Promise<void> {
    const epoch = this.begin(expectedUid);
    const lease = { uid: expectedUid, collection };
    this.leases.set(leaseId, lease);
    try {
      const { db, sdk } = await this.teams.firestore(expectedUid);
      this.syncAccount();
      if (epoch !== this.epoch || this.leases.get(leaseId) !== lease) return;
      const current = () => epoch === this.epoch && this.leases.get(leaseId) === lease;
      const rows = (docs: { id: string; data(): Record<string, unknown> }[]) => {
        if (collection === 'friends') return docs.flatMap(row => firestoreFriendCard(row.id, row.data()) ?? []);
        if (collection === 'blocks') return docs.map(row => firestoreBlock(row.id, row.data()));
        return docs.flatMap(row => firestoreTeamOffer(row.id, row.data()) ?? []);
      };
      this.stops.set(leaseId, sdk.onSnapshot(sdk.collection(db, 'users', expectedUid, collection), snapshot => {
        if (current()) this.events.emit('rows', { leaseId, rows: rows(snapshot.docs) });
      }, () => {
        if (!current()) return;
        this.release(leaseId);
        this.events.emit('failed', { leaseId, code: 'SERVICE_UNAVAILABLE' });
      }));
    } catch (error) {
      if (this.leases.get(leaseId) === lease) this.leases.delete(leaseId);
      throw error;
    }
  }

  /** Stop every watch whose lease starts with `prefix` (a window that went away, or one lease). */
  releaseWatches(prefix: string): void {
    for (const leaseId of [...this.leases.keys()]) if (leaseId.startsWith(prefix)) this.release(leaseId);
  }

  close(): void {
    this.stopAccount();
    this.reset();
    this.events.removeAllListeners();
  }

  // ── Account ──

  private syncAccount(): void {
    const uid = this.account.snapshot().user?.uid, authenticationEpoch = this.account.sessionEpoch?.() ?? 0;
    if (uid === this.uid && authenticationEpoch === this.authenticationEpoch) return;
    this.uid = uid; this.authenticationEpoch = authenticationEpoch;
    this.reset();
  }

  /** D19: another account (or none) ends every watch, and each renderer is told so it can drop what it holds. */
  private reset(): void {
    this.epoch++;
    const ended = [...this.leases.keys()];
    for (const leaseId of ended) this.release(leaseId);
    for (const leaseId of ended) this.events.emit('failed', { leaseId, code: 'INVALID_SESSION' });
  }

  private release(leaseId: string): void {
    this.stops.get(leaseId)?.();
    this.stops.delete(leaseId); this.leases.delete(leaseId);
  }

  /** The epoch to hold a call to, once the account is the one it was made for. */
  private begin(expectedUid: string): number {
    this.syncAccount();
    if (this.uid !== expectedUid) throw new Error('INVALID_SESSION');
    return this.epoch;
  }

  private assert(expectedUid: string, epoch: number): void {
    this.syncAccount();
    if (epoch !== this.epoch || this.uid !== expectedUid) throw new Error('INVALID_SESSION');
  }

  /** A call whose token and answer both belong to the account it was made for. */
  private async request<T>(expectedUid: string, path: string, body?: unknown): Promise<T> {
    const epoch = this.begin(expectedUid);
    const transport = friendsTransport(this.base, async () => {
      this.assert(expectedUid, epoch);
      const token = await this.account.getIdToken();
      this.assert(expectedUid, epoch);
      return token;
    });
    const result = await transport<T>(path, body);
    this.assert(expectedUid, epoch);
    return result;
  }
}
