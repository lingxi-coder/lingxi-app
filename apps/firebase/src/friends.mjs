import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { FieldValue, Filter } from 'firebase-admin/firestore';
import { z } from 'zod';
import { FRIEND_COPY, sendTeamPush } from './push.mjs';
import { TEAM_MEMBER_LIMIT, TeamsError, admitMember, createPairTeam, displayName, endTeam, manager } from './teams.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
// '|' joins the uids hashed into a pair key and a pair team id, so it cannot occur inside one.
const uid = z.string().min(1).max(128).regex(/^[^/|\u0000-\u001f\u007f]+$/u);
const code = z.string().regex(/^[A-Za-z0-9_-]{32}$/u);
const handle = z.string().regex(/^[A-Za-z0-9_]{3,24}$/u);
const email = z.string().trim().toLowerCase().max(254).pipe(z.email());
const name = z.string().trim().min(1).max(80);
const note = z.string().trim().min(1).max(200);
const other = { otherUid: uid };
const expected = { expectedVersion: id.optional() };
const command = z.discriminatedUnion('action', [
  z.object({ action: z.literal('profile'), displayName: name.optional(), handle: handle.optional(),
    findByHandle: z.boolean().optional(), findByEmail: z.boolean().optional(), findByHandlePart: z.boolean().optional() }).strict(),
  z.object({ action: z.literal('lookup'), handle: handle.optional(), email: email.optional(), code: code.optional(),
    teamId: id.optional(), uid: uid.optional() }).strict()
    .refine(v => [v.handle, v.email, v.code, v.teamId].filter(value => value !== undefined).length === 1 && (v.teamId === undefined) === (v.uid === undefined)),
  z.object({ action: z.literal('searchHandles'), query: handle }).strict(),
  // The proof is the one `lookup` takes: a handle, an email, or a team both belong to (teamId with uid). A bare uid is not one.
  z.object({ action: z.literal('request'), requestId: id, note: note.optional(), via: z.enum(['handle', 'email', 'team']),
    handle: handle.optional(), email: email.optional(), teamId: id.optional(), uid: uid.optional(), expectedUid: uid.optional() }).strict()
    .refine(v => (v.via === 'handle') === (v.handle !== undefined) && (v.via === 'email') === (v.email !== undefined)
      && (v.via === 'team') === (v.teamId !== undefined) && (v.teamId === undefined) === (v.uid === undefined)
      && (v.expectedUid === undefined || v.via === 'handle')),
  z.object({ action: z.literal('accept'), ...other, ...expected }).strict(),
  z.object({ action: z.literal('decline'), ...other, ...expected }).strict(),
  z.object({ action: z.literal('cancel'), ...other, ...expected }).strict(),
  z.object({ action: z.literal('redeem'), code, requestId: id }).strict(),
  z.object({ action: z.literal('remove'), ...other, ...expected }).strict(),
  z.object({ action: z.literal('block'), ...other }).strict(),
  z.object({ action: z.literal('unblock'), ...other }).strict(),
  z.object({ action: z.literal('rotateCode') }).strict(),
  z.object({ action: z.literal('revokeCode') }).strict(),
  z.object({ action: z.literal('offerTeam'), teamId: id, friendUid: uid, requestId: id }).strict(),
  z.object({ action: z.literal('acceptOffer'), offerId: id, requestId: id, ...expected }).strict(),
  z.object({ action: z.literal('declineOffer'), offerId: id, ...expected }).strict(),
]);
const hash = value => createHash('sha256').update(value).digest('hex');
const sorted = (a, b) => (a < b ? [a, b] : [b, a]);
export const FRIEND_LIMIT = 500;
export const OUTGOING_LIMIT = 50;
export const INCOMING_LIMIT = 500;
const LEGACY_VERSION = 'legacy-v1';
const versionMatches = (expectedVersion, version) => expectedVersion === undefined || expectedVersion === (version ?? LEGACY_VERSION);
export const LOOKUP_LIMIT = 30;
export const LOOKUP_WINDOW_MS = 10 * 60 * 1000;
const HANDLE_SEARCH_LIMIT = 60;
const HANDLE_SEARCH_WINDOW_MS = 60_000;
const HANDLE_CANDIDATE_LIMIT = 25;
const HANDLE_SUBSTRING_RESERVE = 8;
const HANDLE_RESULT_LIMIT = 10;
const HANDLE_BATCH_SIZE = 12;

// Handles are capped at 24 ASCII characters. At most 253 distinct substrings
// make exact substring matching a single bounded Firestore index lookup.
function handleParts(value) {
  const parts = new Set();
  for (let start = 0; start <= value.length - 3; start++) {
    for (let end = start + 3; end <= value.length; end++) parts.add(value.slice(start, end));
  }
  return [...parts];
}

/** A team offer lives 7 days (the longest an invite link may), stays listed as expired for 7 more, and a friend holds at most 20 live ones. */
export const OFFER_TTL_MS = 7 * 24 * 60 * 60 * 1000;
export const OFFER_GRACE_MS = 7 * 24 * 60 * 60 * 1000;
export const OFFER_LIMIT = 20;
export const pairKey = (a, b) => hash(sorted(a, b).join('|'));
/** A new id for every friendship of the pair (`epoch` counts them), so a deleted pair team is never reused. */
export const pairTeamId = (a, b, epoch) => `f_${hash([...sorted(a, b), epoch].join('|')).slice(0, 40)}`;
export const friendLink = value => `https://lingxios.net/open/friend/add?code=${value}`;
export class FriendsError extends TeamsError {}
const fail = (status, code) => { throw new FriendsError(status, code); };
// Missing, hidden, self and blocked (either way) are indistinguishable to the caller.
const hidden = () => fail(404, 'NOT_FOUND');
const millis = value => value?.toMillis?.() ?? Number.NaN;
/** Nothing besides the cards to delete: a pending request has no pair team and no offers. */
const NO_TIE = Object.freeze({ team: null, offers: Object.freeze([]) });
const cardOf = (profile, fallback = 'Member') => ({ displayName: profile?.displayName ?? fallback, ...(profile?.handle ? { handle: profile.handle } : {}) });
/** Whether a user (`p` is their profile, if they have one) may be found by `via`. The one rule behind `lookup` and `request`.
 * `shared`: the caller and the user are current members of one active team. */
function findable(via, p, { handleLower, shared }) {
  if (via === 'handle') return !!p?.handle && p.handleLower === handleLower && p.findByHandle !== false;
  // Nobody who never opened Friends has switched anything off, so a missing profile is discoverable by email.
  if (via === 'email') return !p || p.findByEmail !== false;
  return via === 'code' ? !!p : shared;
}
function discoveryCard(owner, via, [profile, mine, theirs, relation], { handleLower, shared, name, part }) {
  const p = profile.data();
  if (mine.exists || theirs.exists || (part && (p?.findByHandlePart !== true || !p.handleLower?.includes(part)))) return null;
  const card = relation.data();
  return findable(via, p, { handleLower, shared })
    ? { uid: owner, ...cardOf(p, name), relation: card?.state ?? 'none',
        ...(card?.state === 'incoming' ? { version: card.version ?? LEGACY_VERSION } : {}) } : null;
}
const view = (owner, p, value) => ({ uid: owner, displayName: p.displayName, ...(p.handle ? { handle: p.handle } : {}),
  findByHandle: p.findByHandle, findByEmail: p.findByEmail, findByHandlePart: p.findByHandlePart === true, friendCode: value ? { code: value, link: friendLink(value) } : null });

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
    if (input.action === 'searchHandles') return this.searchHandles(user, input.query);
    if (input.action === 'request') return this.request(user, input);
    if (input.action === 'redeem') return this.redeem(user, input);
    if (input.action === 'rotateCode') return this.rotateCode(user);
    if (input.action === 'revokeCode') return this.revokeCode(user);
    if (input.action === 'offerTeam') return this.offerTeam(user, input);
    if (input.action === 'acceptOffer') return this.acceptOffer(user, input);
    if (input.action === 'declineOffer') return this.declineOffer(user, input);
    if (input.otherUid === user.uid) fail(400, 'INVALID_INPUT');
    if (input.action === 'unblock') { await this.blockRef(user.uid, input.otherUid).delete(); return { ok: true }; }
    return this[input.action](user, input.otherUid, input.expectedVersion);
  }

  /** Sliding per-account limits. Exact lookup and live handle search have separate budgets from requests and redeems. */
  async take(owner, bucket) {
    const window = bucket === 'handleSearch' ? HANDLE_SEARCH_WINDOW_MS : LOOKUP_WINDOW_MS;
    const limit = bucket === 'handleSearch' ? HANDLE_SEARCH_LIMIT : LOOKUP_LIMIT;
    return this.run(async tx => {
      const ref = this.db.doc(`friendLimits/${hash(`${bucket}:${owner}`)}`), now = this.clock();
      const recent = ((await tx.get(ref)).data()?.sentAt ?? []).filter(at => at > now - window);
      if (recent.length >= limit) {
        const error = new FriendsError(429, 'RATE_LIMITED');
        if (bucket === 'handleSearch') error.retryAfterMs = Math.max(1, Math.min(...recent) + window - now);
        throw error;
      }
      tx.set(ref, { sentAt: [...recent, now], expiresAt: new Date(now + window) });
    });
  }
  async prune() {
    // A closed window is worthless, and so is an offer that has been expired for OFFER_GRACE_MS.
    await this.sweep(this.db.collection('friendLimits').where('expiresAt', '<=', new Date(this.clock())));
    await this.sweep(this.db.collectionGroup('teamOffers').where('expiresAt', '<=', new Date(this.clock() - OFFER_GRACE_MS)));
  }
  /** Deletes expired snapshots so concurrent renewals keep their new expiry. */
  async sweep(query) {
    const writer = this.db.bulkWriter();
    try {
      for (let page = 0; page < 20; page++) {
        const snapshot = await query.limit(100).get();
        if (snapshot.empty) break;
        const writes = snapshot.docs.map(async doc => {
          try { await writer.delete(doc.ref, { lastUpdateTime: doc.updateTime }); }
          catch (error) {
            if (error.code !== 5 && error.code !== 9) throw error;
          }
        });
        await Promise.all([writer.flush(), ...writes]);
      }
    } finally {
      await writer.close();
    }
  }

  /** The caller's profile, created with the first friend code on first use. `patch` changes display name, handle
   * (case-folded lock in `handles/`, swapped in the same transaction) and discoverability settings. */
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
      if (patch.findByHandlePart !== undefined || (next.findByHandle === false && next.findByHandlePart === true)) {
        next.findByHandlePart = patch.findByHandlePart === true && next.findByHandle !== false && !!next.handleLower;
      }
      if (next.findByHandlePart !== current?.findByHandlePart || (next.findByHandlePart && next.handleLower !== current?.handleLower)) {
        const index = this.db.doc(`friendHandleSearch/${user.uid}`);
        if (next.findByHandlePart) tx.set(index, { handleLower: next.handleLower, parts: handleParts(next.handleLower) });
        else if (current?.findByHandlePart) tx.delete(index);
      }
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
  cardRefs(viewer, owner) {
    return [this.profileRef(owner), this.blockRef(viewer, owner), this.blockRef(owner, viewer), this.friendRef(viewer, owner)];
  }

  /** `owner`'s card as `viewer` may see it, or null. `via` is the discovery route and decides which flag applies;
   * `name` is the Auth display name that stands in when the owner has no profile (email route). */
  async card(viewer, owner, via, { handleLower, teamId, name } = {}) {
    if (!owner || owner === viewer) return null;
    const team = teamId ? this.db.doc(`teams/${teamId}`) : null;
    const docs = await this.db.getAll(...this.cardRefs(viewer, owner),
      ...(team ? [team, team.collection('members').doc(viewer), team.collection('members').doc(owner)] : []));
    const [, , , , teamDoc, self, member] = docs;
    const shared = teamDoc?.data()?.state === 'active' && !!self?.exists && !!member?.exists;
    return discoveryCard(owner, via, docs, { handleLower, shared, name: member?.data()?.displayName ?? name });
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

  async searchHandles(user, query) {
    await this.take(user.uid, 'handleSearch');
    const lower = query.toLowerCase();
    const upper = `${lower}\uf8ff`;
    const index = this.db.collection('friendHandleSearch');
    const ordered = index.orderBy('handleLower').select('handleLower');
    const substring = ordered.where('parts', 'array-contains', lower).where(Filter.or(
      Filter.where('handleLower', '<', lower),
      Filter.where('handleLower', '>=', upper),
    ));
    const prefix = ordered.startAt(lower).endBefore(upper);
    const [exact, prefixes] = await Promise.allSettled([
      this.handleRef(lower).get(),
      prefix.limit(HANDLE_BATCH_SIZE - 1).get(),
    ]);
    if (exact.status === 'rejected') throw exact.reason;
    const exactUid = exact.value.data()?.uid;
    const seen = new Set([user.uid, exactUid]);
    const results = [];
    let remaining = HANDLE_CANDIDATE_LIMIT;
    const collect = async candidates => {
      if (!candidates.length) return;
      const refs = candidates.map(candidate => this.cardRefs(user.uid, candidate.uid));
      const docs = await this.db.getAll(...refs.flat(), {
        fieldMask: ['displayName', 'handle', 'handleLower', 'findByHandle', 'findByHandlePart', 'state'],
      });
      let offset = 0;
      for (const [i, candidate] of candidates.entries()) {
        const end = offset + refs[i].length;
        const result = discoveryCard(candidate.uid, 'handle', docs.slice(offset, end), {
          handleLower: candidate.handleLower,
          part: candidate.exact ? undefined : lower,
        });
        offset = end;
        if (result) results.push(result);
        if (results.length === HANDLE_RESULT_LIMIT) break;
      }
    };

    const scan = async (query, page, exactCandidate) => {
      let size = page ? HANDLE_BATCH_SIZE - 1 : Math.min(HANDLE_BATCH_SIZE, remaining, HANDLE_RESULT_LIMIT - results.length + 2);
      while (remaining > 0 && results.length < HANDLE_RESULT_LIMIT) {
        const { docs } = page ?? await query.limit(size).get();
        const candidates = exactCandidate ? [exactCandidate] : [];
        exactCandidate = undefined;
        for (const doc of docs) {
          if (seen.has(doc.id)) continue;
          seen.add(doc.id);
          candidates.push({ uid: doc.id, handleLower: doc.data().handleLower });
        }
        remaining -= docs.length;
        await collect(candidates);
        if (docs.length < size || remaining <= 0 || results.length >= HANDLE_RESULT_LIMIT) break;
        query = query.startAfter(docs.at(-1));
        page = undefined;
        size = Math.min(HANDLE_BATCH_SIZE, remaining, HANDLE_RESULT_LIMIT - results.length);
      }
    };

    const exactCandidate = exactUid && exactUid !== user.uid ? { uid: exactUid, handleLower: lower, exact: true } : undefined;
    if (prefixes.status === 'rejected') {
      await collect(exactCandidate ? [exactCandidate] : []);
      if (!results.length) throw prefixes.reason;
      return { results };
    }
    await scan(prefix, prefixes.value, exactCandidate);
    // A dense prefix window must leave room to look for visible substring matches.
    remaining = Math.max(remaining, HANDLE_SUBSTRING_RESERVE);
    try { await scan(substring); }
    catch (error) {
      // Preserve already verified matches when the secondary index is unavailable.
      if (!results.length || ![4, 8, 9, 13, 14].includes(error.code)) throw error;
    }
    return { results };
  }

  /** Everything the transitions between `user` and `peer` read first, in one batch (a transaction reads before it writes). */
  async load(tx, user, peer) {
    const [pair, blocking, blockedBy, mine, theirs, stored] = await tx.getAll(this.pairRef(user.uid, peer),
      this.blockRef(user.uid, peer), this.blockRef(peer, user.uid), this.profileRef(user.uid), this.profileRef(peer), this.friendRef(user.uid, peer));
    // `theirs` is the peer's current card: their profile, else the name already on the card in the caller's list.
    return { friendship: pair.data(), blocking: blocking.exists, blocked: blocking.exists || blockedBy.exists,
      mine: cardOf(mine.data(), displayName(user)), stored: stored.data(), profile: theirs.data(), theirs: cardOf(theirs.data(), stored.data()?.displayName) };
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
      epoch, teamId, source, ...(pending ? { acceptedVersion: friendship.version ?? LEGACY_VERSION } : {}), createdAt: pending ? friendship.createdAt : now, updatedAt: now });
    tx.set(this.friendRef(actor, peer), { state: 'friends', ...theirs, teamId, generation: 1, since: now, updatedAt: now });
    tx.set(this.friendRef(peer, actor), { state: 'friends', ...mine, teamId, generation: 1, since: now, updatedAt: now });
    return teamId;
  }
  /** What a friendship owns besides its two cards, read before `sever` writes: its pair team and the team offers either
   * friend sent the other. A pending request owns neither. */
  async tie(tx, a, b, friendship) {
    if (friendship?.state !== 'friends') return NO_TIE;
    const team = await tx.get(this.db.doc(`teams/${friendship.teamId}`));
    const sent = await tx.get(this.db.collection(`users/${b}/teamOffers`).where('fromUid', '==', a));
    const received = await tx.get(this.db.collection(`users/${a}/teamOffers`).where('fromUid', '==', b));
    return { team, offers: [...sent.docs, ...received.docs] };
  }
  /** Ends a friendship or pending request; the pair team and the offers (`tie`, read first) end with it. */
  sever(tx, a, b, { team, offers }, now) {
    tx.update(this.pairRef(a, b), { state: 'ended', endedAt: now, updatedAt: now, teamId: FieldValue.delete(), note: FieldValue.delete() });
    tx.delete(this.friendRef(a, b));
    tx.delete(this.friendRef(b, a));
    if (team?.data()?.state === 'active') endTeam(tx, team.ref, [a, b].map(member => team.ref.collection('members').doc(member)), now);
    for (const offer of offers) tx.delete(offer.ref);
  }
  async tell(event) {
    // A push is a best-effort wake-up. The lists come from Firestore, so a failed one never fails the command.
    try { await this.notify?.(event); } catch { /* ignored */ }
  }

  /** A request names its owner with the proof `lookup` takes (a handle, an email, or a team both belong to) and resolves it the
   * same way, so it is refused with the same uniform NOT_FOUND whenever `lookup` would show nothing. A request to a user who
   * already asked the caller is that user's accept. A repeated requestId never creates state twice. */
  async request(user, input) {
    const me = user.uid, now = new Date(this.clock()), lower = input.handle?.toLowerCase(), version = randomUUID();
    await this.take(me, 'request');
    // Auth is not transactional: an email proof resolves once, here, and the transaction re-reads everything else.
    const account = input.via === 'email' ? await this.byEmail(input.email) : undefined;
    const digest = hash(JSON.stringify(['request', input.via, lower ?? null, input.email ?? null, input.teamId ?? null, input.uid ?? null, input.note ?? null, ...(input.expectedUid ? [input.expectedUid] : [])]));
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
      if (!peer || peer === me || (input.expectedUid !== undefined && peer !== input.expectedUid)) hidden();
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
        return { result: { uid: peer, state: 'friends', teamId }, event: { kind: 'accepted', to: peer, from: me, teamId, name: c.mine.displayName, key: `${pairKey(me, peer)}:${teamId}` } };
      }
      if ((await this.count(tx, me, 'outgoing')) >= OUTGOING_LIMIT) fail(409, 'REQUEST_LIMIT');
      if ((await this.count(tx, peer, 'incoming')) >= INCOMING_LIMIT) fail(409, 'RECIPIENT_REQUEST_LIMIT');
      const text = input.note ? { note: input.note } : {};
      tx.set(this.pairRef(me, peer), { uids: sorted(me, peer), state: 'pending', requester: me, version, ...text, epoch: f?.epoch ?? 0,
        source: input.via, createdAt: now, updatedAt: now });
      tx.set(this.friendRef(me, peer), { state: 'outgoing', version, ...theirs, ...text, since: now, updatedAt: now });
      tx.set(this.friendRef(peer, me), { state: 'incoming', version, ...c.mine, ...text, since: now, updatedAt: now });
      tx.create(receiptRef, saved);
      return { result: { uid: peer, state: 'outgoing' }, event: { kind: 'request', to: peer, from: me, version, name: c.mine.displayName, key: `${pairKey(me, peer)}:${input.requestId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }
  async accept(user, peer, expectedVersion) {
    const me = user.uid, now = new Date(this.clock());
    const { result, event } = await this.run(async tx => {
      const c = await this.load(tx, user, peer), f = c.friendship;
      if (f?.state === 'friends' && (expectedVersion === undefined || f.acceptedVersion === expectedVersion)) {
        return { result: { state: 'friends', teamId: f.teamId } };
      }
      if (c.blocked || f?.state !== 'pending' || f.requester !== peer) fail(404, 'REQUEST_UNAVAILABLE');
      if (!versionMatches(expectedVersion, f.version)) fail(409, 'STALE_REQUEST');
      await this.room(tx, me, peer);
      const teamId = this.befriend(tx, { actor: me, peer, mine: c.mine, theirs: c.theirs, friendship: f, source: f.source, now });
      return { result: { state: 'friends', teamId }, event: { kind: 'accepted', to: peer, from: me, teamId, name: c.mine.displayName, key: `${pairKey(me, peer)}:${teamId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }
  // Declining or cancelling a request that is no longer there is a harmless repeat.
  async withdraw(user, peer, requester, expectedVersion) {
    const now = new Date(this.clock());
    await this.run(async tx => {
      const f = (await this.load(tx, user, peer)).friendship;
      if (f?.state !== 'pending' || f.requester !== requester) return;
      if (!versionMatches(expectedVersion, f.version)) fail(409, 'STALE_REQUEST');
      this.sever(tx, user.uid, peer, NO_TIE, now);
    });
    return { ok: true };
  }
  decline(user, peer, version) { return this.withdraw(user, peer, peer, version); }
  cancel(user, peer, version) { return this.withdraw(user, peer, user.uid, version); }
  async remove(user, peer, expectedVersion) {
    const now = new Date(this.clock());
    await this.run(async tx => {
      const f = (await tx.get(this.pairRef(user.uid, peer))).data();
      if (f?.state !== 'friends') return;
      if (!versionMatches(expectedVersion, f.teamId)) fail(409, 'STALE_REQUEST');
      this.sever(tx, user.uid, peer, await this.tie(tx, user.uid, peer, f), now);
    });
    return { ok: true };
  }
  /** Ends the friendship and any pending request in both directions; only `unblock` lifts it, and it restores nothing.
   * Keep only a name the caller already had access to. A repeated block keeps the first. */
  async block(user, peer) {
    const now = new Date(this.clock());
    await this.run(async tx => {
      const c = await this.load(tx, user, peer), tie = await this.tie(tx, user.uid, peer, c.friendship);
      if (!c.blocking) {
        const card = c.friendship?.state === 'friends' && !c.blocked ? c.theirs : cardOf(c.stored);
        tx.set(this.blockRef(user.uid, peer), { at: now, ...card });
      }
      if (['friends', 'pending'].includes(c.friendship?.state)) this.sever(tx, user.uid, peer, tie, now);
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
      return { result: { state: 'friends', uid: owner, teamId }, event: { kind: 'linked', to: owner, from: me, teamId, name: c.mine.displayName, key: `${pairKey(me, owner)}:${teamId}` } };
    });
    if (event) await this.tell(event);
    return result;
  }

  async liveOffers(tx, owner, now) {
    return (await tx.get(this.db.collection(`users/${owner}/teamOffers`).where('expiresAt', '>', now).count())).data().count;
  }
  /** A manager offers a team to a current friend: one document in the friend's own `teamOffers`, named by the team, so a repeat
   * refreshes it. It grants nothing and holds no secret; `acceptOffer` checks everything again. Offering needs the caller to
   * be an owner or admin of an ordinary active team that has room, and a friend (never blocked) who is not a member yet.
   * Offers push to the friend only when they are new or had expired, so a repeat cannot be used to nag. */
  async offerTeam(user, input) {
    const me = user.uid, peer = input.friendUid, now = new Date(this.clock()), newVersion = randomUUID();
    if (peer === me) fail(400, 'INVALID_INPUT');
    await this.take(me, 'request');
    const digest = hash(JSON.stringify(['offerTeam', input.teamId, peer]));
    const { result, event } = await this.run(async tx => {
      const receiptRef = this.db.doc(`users/${me}/friendRequests/${input.requestId}`), offerRef = this.db.doc(`users/${peer}/teamOffers/${input.teamId}`);
      const team = this.db.doc(`teams/${input.teamId}`);
      const [receipt, teamDoc, self, member, offer] = await tx.getAll(receiptRef, team, team.collection('members').doc(me), team.collection('members').doc(peer), offerRef);
      if (receipt.exists) {
        // A replay answers for the offer the first call made, whatever has become of it since.
        if (receipt.data().digest !== digest) fail(409, 'REQUEST_ID_CONFLICT');
        return { result: { offerId: input.teamId, expiresAt: receipt.data().expiresAt.toDate().toISOString() } };
      }
      const c = await this.load(tx, user, peer);
      if (teamDoc.data()?.state !== 'active' || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (teamDoc.data().kind === 'pair') fail(400, 'PAIR_TEAM');
      if (!manager(self.data())) fail(403, 'ROLE_REQUIRED');
      if (c.friendship?.state !== 'friends' || c.blocked) fail(403, 'FRIEND_REQUIRED');
      if (member.exists) fail(409, 'ALREADY_MEMBER');
      if (teamDoc.data().memberCount >= TEAM_MEMBER_LIMIT) fail(409, 'MEMBER_LIMIT');
      const live = offer.exists && millis(offer.data().expiresAt) > now.getTime();
      if (!live && (await this.liveOffers(tx, peer, now)) >= OFFER_LIMIT) fail(409, 'OFFER_LIMIT');
      const expiresAt = new Date(now.getTime() + OFFER_TTL_MS);
      const version = live && offer.data().fromUid === me ? offer.data().version ?? LEGACY_VERSION : newVersion;
      tx.set(offerRef, { teamId: input.teamId, teamName: teamDoc.data().name, fromUid: me, fromName: c.mine.displayName, role: 'member', expiresAt, version });
      tx.create(receiptRef, { digest, otherUid: peer, offerId: input.teamId, expiresAt, at: now });
      return { result: { offerId: input.teamId, expiresAt: expiresAt.toISOString() },
        ...(live ? {} : { event: { kind: 'offer', to: peer, from: me, name: c.mine.displayName, version, offerId: input.teamId, key: `${pairKey(me, peer)}:${input.requestId}` } }) };
    });
    if (event) await this.tell(event);
    return result;
  }
  /** The friend joins through the same core as an invite link (`admitMember`: new generation, team index, both limits). An offer
   * that cannot be honoured any more (expired, the team deleted, the sender no longer a manager or friend) is deleted and
   * answers OFFER_UNAVAILABLE; a refusal the friend can fix (MEMBER_LIMIT, TEAM_LIMIT) keeps the offer. */
  async acceptOffer(user, input) {
    const me = user.uid, now = new Date(this.clock());
    const outcome = await this.run(async tx => {
      const offerRef = this.db.doc(`users/${me}/teamOffers/${input.offerId}`), receiptRef = this.db.doc(`users/${me}/teamJoinRequests/${input.requestId}`);
      const [offer, receipt] = await tx.getAll(offerRef, receiptRef);
      if (receipt.exists) {
        const old = receipt.data();
        if (old.offerId !== input.offerId || !versionMatches(input.expectedVersion, old.version)) fail(409, 'REQUEST_ID_CONFLICT');
        const member = await tx.get(this.db.doc(`teams/${old.teamId}/members/${me}`));
        if (!member.exists || member.data().generation !== old.generation) fail(409, 'STALE_MEMBERSHIP');
        return { teamId: old.teamId, generation: old.generation, alreadyMember: true };
      }
      if (!offer.exists) fail(404, 'OFFER_UNAVAILABLE');
      const o = offer.data(), team = this.db.doc(`teams/${o.teamId}`);
      if (!versionMatches(input.expectedVersion, o.version)) fail(404, 'OFFER_UNAVAILABLE');
      const [teamDoc, sender, epoch, friendship] = await tx.getAll(team, team.collection('members').doc(o.fromUid), team.collection('membershipEpochs').doc(me), this.pairRef(me, o.fromUid));
      const honoured = millis(o.expiresAt) > now.getTime() && teamDoc.data()?.state === 'active' && teamDoc.data().kind !== 'pair'
        && manager(sender.data()) && friendship.data()?.state === 'friends';
      if (!honoured) { tx.delete(offerRef); return { gone: true }; }
      const generation = await admitMember(tx, this.db, { team: teamDoc, epoch, user, role: o.role, now });
      tx.delete(offerRef);
      tx.create(receiptRef, { teamId: o.teamId, offerId: input.offerId, generation, version: o.version ?? LEGACY_VERSION });
      return { teamId: o.teamId, generation, alreadyMember: false };
    });
    // The deletion above must commit, so the refusal is raised after the transaction instead of inside it.
    if (outcome.gone) fail(404, 'OFFER_UNAVAILABLE');
    return outcome;
  }
  /** Declining, or dismissing an expired offer, deletes it; one that is no longer there is a harmless repeat. */
  async declineOffer(user, input) {
    const ref = this.db.doc(`users/${user.uid}/teamOffers/${input.offerId}`);
    await this.run(async tx => {
      const doc = await tx.get(ref);
      if (!doc.exists) return;
      if (!versionMatches(input.expectedVersion, doc.data().version)) fail(404, 'OFFER_UNAVAILABLE');
      tx.delete(ref);
    });
    return { ok: true };
  }
}

/** Best-effort push for the people a request, a new friendship or a team offer concerns. The route opens the requests list.
 * It is sent only while the card (or offer) it announces is still there, so one that was withdrawn meanwhile stays silent. */
export function friendNotifier({ db, pushStore, messaging, clock = Date.now }) {
  const announced = async ({ kind, to, from, offerId, version, teamId }) => {
    if (kind === 'offer') {
      const offer = (await db.doc(`users/${to}/teamOffers/${offerId}`).get()).data();
      return !!offer && versionMatches(version, offer.version);
    }
    const card = (await db.doc(`users/${to}/friends/${from}`).get()).data();
    return kind === 'request'
      ? card?.state === 'incoming' && versionMatches(version, card.version)
      : card?.state === 'friends' && versionMatches(teamId, card.teamId);
  };
  return event => sendTeamPush(pushStore, messaging, {
    uid: event.to, title: event.name.trim().slice(0, 80) || 'LingXi', notificationId: hash(JSON.stringify(['friend', event.kind, event.key, event.to])),
    collapseKey: `friend:${hash(JSON.stringify([event.key])).slice(0, 32)}`, route: 'friend/requests', kind: `friend_${event.kind}`, copy: FRIEND_COPY[event.kind],
  }, () => announced(event), clock);
}
