import { createHash, randomBytes } from 'node:crypto';
import { FieldValue } from 'firebase-admin/firestore';
import { z } from 'zod';
import { FRIEND_COPY, sendTeamPush } from './push.mjs';
import { TeamsError, createPairTeam, displayName, endTeam } from './teams.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
// '|' joins the uids hashed into a pair key and a pair team id, so it cannot occur inside one.
const uid = z.string().min(1).max(128).regex(/^[^/|\u0000-\u001f\u007f]+$/u);
const code = z.string().regex(/^[A-Za-z0-9_-]{32}$/u);
const handle = z.string().regex(/^[A-Za-z0-9_]{3,24}$/u);
const email = z.string().trim().toLowerCase().max(254).pipe(z.email());
const name = z.string().trim().min(1).max(80);
const note = z.string().trim().min(1).max(200);
const other = { otherUid: uid };
const command = z.discriminatedUnion('action', [
  z.object({ action: z.literal('profile'), displayName: name.optional(), handle: handle.optional(),
    findByHandle: z.boolean().optional(), findByEmail: z.boolean().optional() }).strict(),
  z.object({ action: z.literal('lookup'), handle: handle.optional(), email: email.optional(), code: code.optional(),
    teamId: id.optional(), uid: uid.optional() }).strict()
    .refine(v => [v.handle, v.email, v.code, v.teamId].filter(value => value !== undefined).length === 1 && (v.teamId === undefined) === (v.uid === undefined)),
  // The proof is the one `lookup` takes: a handle, an email, or a team both belong to (teamId with uid). A bare uid is not one.
  z.object({ action: z.literal('request'), requestId: id, note: note.optional(), via: z.enum(['handle', 'email', 'team']),
    handle: handle.optional(), email: email.optional(), teamId: id.optional(), uid: uid.optional() }).strict()
    .refine(v => (v.via === 'handle') === (v.handle !== undefined) && (v.via === 'email') === (v.email !== undefined)
      && (v.via === 'team') === (v.teamId !== undefined) && (v.teamId === undefined) === (v.uid === undefined)),
  z.object({ action: z.literal('accept'), ...other }).strict(),
  z.object({ action: z.literal('decline'), ...other }).strict(),
  z.object({ action: z.literal('cancel'), ...other }).strict(),
  z.object({ action: z.literal('redeem'), code, requestId: id }).strict(),
  z.object({ action: z.literal('remove'), ...other }).strict(),
  z.object({ action: z.literal('block'), ...other }).strict(),
  z.object({ action: z.literal('unblock'), ...other }).strict(),
  z.object({ action: z.literal('rotateCode') }).strict(),
  z.object({ action: z.literal('revokeCode') }).strict(),
]);
const hash = value => createHash('sha256').update(value).digest('hex');
const sorted = (a, b) => (a < b ? [a, b] : [b, a]);
export const FRIEND_LIMIT = 500;
export const OUTGOING_LIMIT = 50;
export const LOOKUP_LIMIT = 30;
export const LOOKUP_WINDOW_MS = 10 * 60 * 1000;
export const pairKey = (a, b) => hash(sorted(a, b).join('|'));
/** A new id for every friendship of the pair (`epoch` counts them), so a deleted pair team is never reused. */
export const pairTeamId = (a, b, epoch) => `f_${hash([...sorted(a, b), epoch].join('|')).slice(0, 40)}`;
export const friendLink = value => `https://lingxios.net/open/friend/add?code=${value}`;
export class FriendsError extends TeamsError {}
const fail = (status, code) => { throw new FriendsError(status, code); };
// Missing, hidden, self and blocked (either way) are indistinguishable to the caller.
const hidden = () => fail(404, 'NOT_FOUND');
const cardOf = (profile, fallback = 'Member') => ({ displayName: profile?.displayName ?? fallback, ...(profile?.handle ? { handle: profile.handle } : {}) });
/** Whether a user (`p` is their profile, if they have one) may be found by `via`. The one rule behind `lookup` and `request`.
 * `shared`: the caller and the user are current members of one active team. */
function findable(via, p, { handleLower, shared }) {
  if (via === 'handle') return !!p?.handle && p.handleLower === handleLower && p.findByHandle !== false;
  // Nobody who never opened Friends has switched anything off, so a missing profile is discoverable by email.
  if (via === 'email') return !p || p.findByEmail !== false;
  return via === 'code' ? !!p : shared;
}
const view = (owner, p, value) => ({ uid: owner, displayName: p.displayName, ...(p.handle ? { handle: p.handle } : {}),
  findByHandle: p.findByHandle, findByEmail: p.findByEmail, friendCode: value ? { code: value, link: friendLink(value) } : null });

/** Profiles, discovery, requests and friendships (docs/FRIENDS.md). A friendship is a hidden two-person pair team,
 * so every transition that changes it commits the team, both `users/{uid}/friends` cards and `friendships/{pairKey}`
 * in one transaction. The friendship document serializes everything between two users. */
export class FriendsStore {
  constructor(db, clock = Date.now, { auth, notify } = {}) { this.db = db; this.clock = clock; this.auth = auth; this.notify = notify; }
  profileRef(owner) { return this.db.doc(`profiles/${owner}`); }
  friendRef(owner, peer) { return this.db.doc(`users/${owner}/friends/${peer}`); }
  blockRef(owner, peer) { return this.db.doc(`users/${owner}/blocks/${peer}`); }
  pairRef(a, b) { return this.db.doc(`friendships/${pairKey(a, b)}`); }
  codeRef(value) { return this.db.doc(`friendCodes/${hash(value)}`); }
  handleRef(lower) { return this.db.doc(`handles/${lower}`); }
  run(work) { return this.db.runTransaction(work, { maxAttempts: 20 }); }

  async execute(user, raw) {
    const input = command.parse(raw);
    uid.parse(user.uid);
    if (input.action === 'profile') return this.profile(user, input);
    if (input.action === 'lookup') return this.lookup(user, input);
    if (input.action === 'request') return this.request(user, input);
    if (input.action === 'redeem') return this.redeem(user, input);
    if (input.action === 'rotateCode') return this.rotateCode(user);
    if (input.action === 'revokeCode') return this.revokeCode(user);
    if (input.otherUid === user.uid) fail(400, 'INVALID_INPUT');
    if (input.action === 'unblock') { await this.blockRef(user.uid, input.otherUid).delete(); return { ok: true }; }
    return this[input.action](user, input.otherUid);
  }

  /** At most LOOKUP_LIMIT per user in any LOOKUP_WINDOW_MS (a sliding window of timestamps, as PushStore.takeSelfQuota).
   * Lookups have their own bucket; requests and redeems, which push to the other user, share another. */
  async take(owner, bucket) {
    return this.run(async tx => {
      const ref = this.db.doc(`friendLimits/${hash(`${bucket}:${owner}`)}`), now = this.clock();
      const recent = ((await tx.get(ref)).data()?.sentAt ?? []).filter(at => at > now - LOOKUP_WINDOW_MS);
      if (recent.length >= LOOKUP_LIMIT) fail(429, 'RATE_LIMITED');
      tx.set(ref, { sentAt: [...recent, now], expiresAt: new Date(now + LOOKUP_WINDOW_MS) });
    });
  }
  async prune() {
    // A closed window is worthless; the bounded loop repeats the query because deletes shift pages.
    for (let page = 0; page < 20; page++) {
      const snapshot = await this.db.collection('friendLimits').where('expiresAt', '<=', new Date(this.clock())).limit(100).get();
      if (snapshot.empty) break;
      const batch = this.db.batch();
      for (const doc of snapshot.docs) batch.delete(doc.ref);
      await batch.commit();
    }
  }

  /** The caller's profile, created with the first friend code on first use. `patch` changes display name, handle
   * (case-folded lock in `handles/`, swapped in the same transaction) and the two discoverability flags. */
  async profile(user, patch = {}) {
    // The secret is generated once, outside the retried transaction.
    const fresh = randomBytes(24).toString('base64url'), now = new Date(this.clock()), lower = patch.handle?.toLowerCase();
    return this.run(async tx => {
      const ref = this.profileRef(user.uid), current = (await tx.get(ref)).data();
      const claim = lower && lower !== current?.handleLower ? await tx.get(this.handleRef(lower)) : null;
      const release = claim && current?.handleLower ? await tx.get(this.handleRef(current.handleLower)) : null;
      const stored = current?.friendCodeId ? await tx.get(this.db.doc(`friendCodes/${current.friendCodeId}`)) : null;
      if (claim?.exists && claim.data().uid !== user.uid) fail(409, 'HANDLE_TAKEN');
      const next = { ...(current ?? { displayName: displayName(user), findByHandle: true, findByEmail: true, friendCodeId: hash(fresh) }) };
      if (patch.displayName !== undefined) next.displayName = patch.displayName;
      if (patch.findByHandle !== undefined) next.findByHandle = patch.findByHandle;
      if (patch.findByEmail !== undefined) next.findByEmail = patch.findByEmail;
      if (patch.handle !== undefined) Object.assign(next, { handle: patch.handle, handleLower: lower });
      if (!current || Object.keys(next).some(key => next[key] !== current[key])) {
        if (claim) {
          tx.set(this.handleRef(lower), { uid: user.uid });
          if (release?.data()?.uid === user.uid) tx.delete(release.ref);
        }
        if (!current) tx.create(this.codeRef(fresh), { uid: user.uid, code: fresh, createdAt: now, revoked: false });
        tx.set(ref, { ...next, updatedAt: now });
      }
      return view(user.uid, next, current ? (stored?.data()?.revoked === false ? stored.data().code : null) : fresh);
    });
  }
  /** The previous code stops working in the same transaction that issues the new one. */
  async rotateCode(user) {
    const fresh = randomBytes(24).toString('base64url'), now = new Date(this.clock());
    return this.run(async tx => {
      const ref = this.profileRef(user.uid), current = (await tx.get(ref)).data();
      const old = current?.friendCodeId ? await tx.get(this.db.doc(`friendCodes/${current.friendCodeId}`)) : null;
      if (old?.exists) tx.update(old.ref, { revoked: true });
      tx.create(this.codeRef(fresh), { uid: user.uid, code: fresh, createdAt: now, revoked: false });
      tx.set(ref, { ...(current ?? { displayName: displayName(user), findByHandle: true, findByEmail: true }), friendCodeId: hash(fresh), updatedAt: now });
      return { code: fresh, link: friendLink(fresh) };
    });
  }
  async revokeCode(user) {
    const now = new Date(this.clock());
    return this.run(async tx => {
      const ref = this.profileRef(user.uid), current = (await tx.get(ref)).data();
      if (!current?.friendCodeId) return { ok: true };
      const old = await tx.get(this.db.doc(`friendCodes/${current.friendCodeId}`));
      if (old.exists) tx.update(old.ref, { revoked: true });
      tx.update(ref, { friendCodeId: FieldValue.delete(), updatedAt: now });
      return { ok: true };
    });
  }

  /** `{ uid, name }` of the account that owns `address`, else undefined. `name` (the Auth display name) names a user who has no profile. */
  async byEmail(address) {
    if (!this.auth) fail(503, 'SERVICE_UNAVAILABLE');
    try {
      const account = await this.auth.getUserByEmail(address);
      // Anyone can register an address they do not own, so only a verified one identifies a person.
      if (account.emailVerified !== true || !uid.safeParse(account.uid).success) return undefined;
      const name = typeof account.displayName === 'string' ? account.displayName.trim().slice(0, 80) : '';
      return { uid: account.uid, ...(name ? { name } : {}) };
    } catch (error) {
      if (['auth/user-not-found', 'auth/invalid-email'].includes(error?.code)) return undefined;
      throw error;
    }
  }
  /** `owner`'s card as `viewer` may see it, or null. `via` is the discovery route and decides which flag applies;
   * `name` is the Auth display name that stands in when the owner has no profile (email route). */
  async card(viewer, owner, via, { handleLower, teamId, name } = {}) {
    if (!owner || owner === viewer) return null;
    const team = teamId ? this.db.doc(`teams/${teamId}`) : null;
    const [profile, mine, theirs, relation, teamDoc, self, member] = await this.db.getAll(this.profileRef(owner),
      this.blockRef(viewer, owner), this.blockRef(owner, viewer), this.friendRef(viewer, owner),
      ...(team ? [team, team.collection('members').doc(viewer), team.collection('members').doc(owner)] : []));
    const p = profile.data();
    if (mine.exists || theirs.exists) return null;
    const shared = teamDoc?.data()?.state === 'active' && !!self?.exists && !!member?.exists;
    return findable(via, p, { handleLower, shared })
      ? { uid: owner, ...cardOf(p, member?.data()?.displayName ?? name), relation: relation.data()?.state ?? 'none' } : null;
  }
  async lookup(user, input) {
    await this.take(user.uid, 'lookup');
    let owner, via = 'team', handleLower, name;
    if (input.handle) { via = 'handle'; handleLower = input.handle.toLowerCase(); owner = (await this.handleRef(handleLower).get()).data()?.uid; }
    else if (input.email) { via = 'email'; const account = await this.byEmail(input.email); owner = account?.uid; name = account?.name; }
    else if (input.code) { via = 'code'; const entry = (await this.codeRef(input.code).get()).data(); owner = entry && !entry.revoked ? entry.uid : undefined; }
    else owner = input.uid;
    return (await this.card(user.uid, owner, via, { handleLower, teamId: input.teamId, name })) ?? hidden();
  }

  /** Everything the transitions between `user` and `peer` read first, in one batch (a transaction reads before it writes). */
  async load(tx, user, peer) {
    const [pair, blocking, blockedBy, mine, theirs, stored] = await tx.getAll(this.pairRef(user.uid, peer),
      this.blockRef(user.uid, peer), this.blockRef(peer, user.uid), this.profileRef(user.uid), this.profileRef(peer), this.friendRef(user.uid, peer));
    // `theirs` is the peer's current card: their profile, else the name already on the card in the caller's list.
    return { friendship: pair.data(), blocking: blocking.exists, blocked: blocking.exists || blockedBy.exists,
      mine: cardOf(mine.data(), displayName(user)), profile: theirs.data(), theirs: cardOf(theirs.data(), stored.data()?.displayName) };
  }
  async count(tx, owner, state) {
    return (await tx.get(this.db.collection(`users/${owner}/friends`).where('state', '==', state).count())).data().count;
  }
  async room(tx, a, b) {
    if ((await this.count(tx, a, 'friends')) >= FRIEND_LIMIT || (await this.count(tx, b, 'friends')) >= FRIEND_LIMIT) fail(409, 'FRIEND_LIMIT');
  }
  /** Pair team, both cards and the friendship state change together; `actor` is the one whose action completes it. */
  befriend(tx, { actor, peer, mine, theirs, friendship, source, now }) {
    const epoch = (friendship?.epoch ?? 0) + 1, teamId = pairTeamId(actor, peer, epoch), pending = friendship?.state === 'pending';
    createPairTeam(tx, this.db, { teamId, createdBy: actor, now,
      members: [{ uid: actor, displayName: mine.displayName }, { uid: peer, displayName: theirs.displayName }] });
    tx.set(this.pairRef(actor, peer), { uids: sorted(actor, peer), state: 'friends', requester: pending ? friendship.requester : actor,
      epoch, teamId, source, createdAt: pending ? friendship.createdAt : now, updatedAt: now });
    tx.set(this.friendRef(actor, peer), { state: 'friends', ...theirs, teamId, generation: 1, since: now, updatedAt: now });
    tx.set(this.friendRef(peer, actor), { state: 'friends', ...mine, teamId, generation: 1, since: now, updatedAt: now });
    return teamId;
  }
  /** Ends a friendship or pending request; a friendship's pair team (`team`, read first) is deleted with it. */
  sever(tx, a, b, team, now) {
    tx.update(this.pairRef(a, b), { state: 'ended', endedAt: now, updatedAt: now, teamId: FieldValue.delete(), note: FieldValue.delete() });
    tx.delete(this.friendRef(a, b));
    tx.delete(this.friendRef(b, a));
    if (team?.data()?.state === 'active') endTeam(tx, team.ref, [a, b].map(member => team.ref.collection('members').doc(member)), now);
  }
  teamOf(tx, friendship) { return friendship?.state === 'friends' ? tx.get(this.db.doc(`teams/${friendship.teamId}`)) : null; }
  async tell(event) {
    // A push is a best-effort wake-up. The lists come from Firestore, so a failed one never fails the command.
    try { await this.notify?.(event); } catch { /* ignored */ }
  }

  /** A request names its owner with the proof `lookup` takes (a handle, an email, or a team both belong to) and resolves it the
   * same way, so it is refused with the same uniform NOT_FOUND whenever `lookup` would show nothing. A request to a user who
   * already asked the caller is that user's accept. A repeated requestId never creates state twice. */
  async request(user, input) {
    const me = user.uid, now = new Date(this.clock()), lower = input.handle?.toLowerCase();
    await this.take(me, 'request');
    // Auth is not transactional: an email proof resolves once, here, and the transaction re-reads everything else.
    const account = input.via === 'email' ? await this.byEmail(input.email) : undefined;
    const digest = hash(JSON.stringify(['request', input.via, lower ?? null, input.email ?? null, input.teamId ?? null, input.uid ?? null, input.note ?? null]));
    const { result, event } = await this.run(async tx => {
      const receiptRef = this.db.doc(`users/${me}/friendRequests/${input.requestId}`), receipt = await tx.get(receiptRef);
      if (receipt.exists) {
        // A replay is not a discovery: it answers for the user the first call resolved.
        const old = receipt.data();
        if (old.digest !== digest) fail(409, 'REQUEST_ID_CONFLICT');
        const f = (await tx.get(this.pairRef(me, old.otherUid))).data();
        if (f?.state === 'friends') return { result: { uid: old.otherUid, state: 'friends', teamId: f.teamId } };
        if (f?.state === 'pending' && f.requester === me) return { result: { uid: old.otherUid, state: 'outgoing' } };
        fail(409, 'STALE_REQUEST');
      }
      const peer = input.via === 'handle' ? (await tx.get(this.handleRef(lower))).data()?.uid : input.via === 'email' ? account?.uid : input.uid;
      if (!peer || peer === me) hidden();
      const team = input.via === 'team' ? this.db.doc(`teams/${input.teamId}`) : null;
      const [teamDoc, self, member] = team ? await tx.getAll(team, team.collection('members').doc(me), team.collection('members').doc(peer)) : [];
      const c = await this.load(tx, user, peer), f = c.friendship, p = c.profile;
      const shared = teamDoc?.data()?.state === 'active' && !!self.exists && !!member.exists;
      if (c.blocked || !findable(input.via, p, { handleLower: lower, shared })) hidden();
      const saved = { digest, otherUid: peer, at: now };
      if (f?.state === 'friends') { tx.create(receiptRef, saved); return { result: { uid: peer, state: 'friends', teamId: f.teamId } }; }
      if (f?.state === 'pending' && f.requester === me) { tx.create(receiptRef, saved); return { result: { uid: peer, state: 'outgoing' } }; }
      const theirs = cardOf(p, member?.data()?.displayName ?? account?.name ?? c.theirs.displayName);
      if (f?.state === 'pending') {
        await this.room(tx, me, peer);
        const teamId = this.befriend(tx, { actor: me, peer, mine: c.mine, theirs, friendship: f, source: f.source, now });
        tx.create(receiptRef, saved);
        return { result: { uid: peer, state: 'friends', teamId }, event: { kind: 'accepted', to: peer, from: me, name: c.mine.displayName, key: `${pairKey(me, peer)}:${teamId}` } };
      }
      if ((await this.count(tx, me, 'outgoing')) >= OUTGOING_LIMIT) fail(409, 'REQUEST_LIMIT');
      const text = input.note ? { note: input.note } : {};
      tx.set(this.pairRef(me, peer), { uids: sorted(me, peer), state: 'pending', requester: me, ...text, epoch: f?.epoch ?? 0,
        source: input.via, createdAt: now, updatedAt: now });
      tx.set(this.friendRef(me, peer), { state: 'outgoing', ...theirs, ...text, since: now, updatedAt: now });
      tx.set(this.friendRef(peer, me), { state: 'incoming', ...c.mine, ...text, since: now, updatedAt: now });
      tx.create(receiptRef, saved);
      return { result: { uid: peer, state: 'outgoing' }, event: { kind: 'request', to: peer, from: me, name: c.mine.displayName, key: `${pairKey(me, peer)}:${input.requestId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }
  async accept(user, peer) {
    const me = user.uid, now = new Date(this.clock());
    const { result, event } = await this.run(async tx => {
      const c = await this.load(tx, user, peer), f = c.friendship;
      if (f?.state === 'friends') return { result: { state: 'friends', teamId: f.teamId } };
      if (c.blocked || f?.state !== 'pending' || f.requester !== peer) fail(404, 'REQUEST_UNAVAILABLE');
      await this.room(tx, me, peer);
      const teamId = this.befriend(tx, { actor: me, peer, mine: c.mine, theirs: c.theirs, friendship: f, source: f.source, now });
      return { result: { state: 'friends', teamId }, event: { kind: 'accepted', to: peer, from: me, name: c.mine.displayName, key: `${pairKey(me, peer)}:${teamId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }
  // Declining or cancelling a request that is no longer there is a harmless repeat.
  async withdraw(user, peer, requester, now = new Date(this.clock())) {
    await this.run(async tx => {
      const f = (await this.load(tx, user, peer)).friendship;
      if (f?.state === 'pending' && f.requester === requester) this.sever(tx, user.uid, peer, null, now);
    });
    return { ok: true };
  }
  decline(user, peer) { return this.withdraw(user, peer, peer); }
  cancel(user, peer) { return this.withdraw(user, peer, user.uid); }
  async remove(user, peer) {
    const now = new Date(this.clock());
    await this.run(async tx => {
      const f = (await this.load(tx, user, peer)).friendship, team = await this.teamOf(tx, f);
      if (f?.state === 'friends') this.sever(tx, user.uid, peer, team, now);
    });
    return { ok: true };
  }
  /** Ends the friendship and any pending request in both directions; only `unblock` lifts it, and it restores nothing. */
  async block(user, peer) {
    const now = new Date(this.clock());
    await this.run(async tx => {
      const c = await this.load(tx, user, peer), team = await this.teamOf(tx, c.friendship);
      if (!c.blocking) tx.set(this.blockRef(user.uid, peer), { at: now });
      if (['friends', 'pending'].includes(c.friendship?.state)) this.sever(tx, user.uid, peer, team, now);
    });
    return { ok: true };
  }

  /** The code owner's consent is the link: the friendship is instant, and replaces any pending request between the two. */
  async redeem(user, input) {
    const me = user.uid, now = new Date(this.clock());
    await this.take(me, 'request');
    const digest = hash(JSON.stringify(['redeem', hash(input.code)]));
    const { result, event } = await this.run(async tx => {
      const receiptRef = this.db.doc(`users/${me}/friendRequests/${input.requestId}`);
      const [entry, receipt] = await tx.getAll(this.codeRef(input.code), receiptRef);
      if (receipt.exists) {
        if (receipt.data().digest !== digest) fail(409, 'REQUEST_ID_CONFLICT');
        const f = (await tx.get(this.pairRef(me, receipt.data().otherUid))).data();
        if (f?.state !== 'friends') fail(409, 'STALE_REQUEST');
        return { result: { state: 'friends', uid: receipt.data().otherUid, teamId: f.teamId } };
      }
      const owner = entry.data()?.uid;
      if (!owner || entry.data().revoked || owner === me) hidden();
      const c = await this.load(tx, user, owner), f = c.friendship;
      if (c.blocked || !c.profile) hidden();
      const saved = { digest, otherUid: owner, at: now };
      if (f?.state === 'friends') { tx.create(receiptRef, saved); return { result: { state: 'friends', uid: owner, teamId: f.teamId } }; }
      await this.room(tx, me, owner);
      const teamId = this.befriend(tx, { actor: me, peer: owner, mine: c.mine, theirs: c.theirs, friendship: f, source: 'code', now });
      tx.create(receiptRef, saved);
      return { result: { state: 'friends', uid: owner, teamId }, event: { kind: 'linked', to: owner, from: me, name: c.mine.displayName, key: `${pairKey(me, owner)}:${teamId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }
}

/** Best-effort push for the people a request or new friendship concerns. The route opens the requests list. It is
 * sent only while the card it announces is still there, so a request that was cancelled meanwhile stays silent. */
export function friendNotifier({ db, pushStore, messaging, clock = Date.now }) {
  return ({ kind, to, from, name, key }) => sendTeamPush(pushStore, messaging, {
    uid: to, title: name.trim().slice(0, 80) || 'LingXi', notificationId: hash(JSON.stringify(['friend', kind, key, to])),
    collapseKey: `friend:${hash(JSON.stringify([key])).slice(0, 32)}`, route: 'friend/requests', kind: `friend_${kind}`, copy: FRIEND_COPY[kind],
  }, async () => (await db.doc(`users/${to}/friends/${from}`).get()).data()?.state === (kind === 'request' ? 'incoming' : 'friends'), clock);
}
