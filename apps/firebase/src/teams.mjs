import { FieldValue } from 'firebase-admin/firestore';
import { createHash, randomBytes } from 'node:crypto';
import { z } from 'zod';
import { TeamConversationError, conversationRecord } from './team-conversations.mjs';
import { TeamMessagesStore, TeamMessageError } from './team-messages.mjs';
import { SharedBotError } from './shared-bots.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const generation = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const teamId = { teamId: id };
const command = z.discriminatedUnion('action', [
  z.object({ action: z.literal('create'), ...teamId, name: z.string().trim().min(1).max(80) }).strict(),
  z.object({ action: z.literal('delete'), ...teamId }).strict(),
  z.object({ action: z.literal('invite'), ...teamId, requestId: id,
    role: z.enum(['admin', 'member']), maxUses: z.number().int().min(1).max(50),
    ttlSeconds: z.number().int().min(3600).max(604800) }).strict(),
  z.object({ action: z.literal('revokeInvite'), ...teamId, inviteId: z.string().regex(/^[a-f0-9]{64}$/u) }).strict(),
  z.object({ action: z.literal('join'), code: z.string().regex(/^[A-Za-z0-9_-]{32}$/u), requestId: id }).strict(),
  z.object({ action: z.literal('leave'), ...teamId, generation }).strict(),
  z.object({ action: z.literal('remove'), ...teamId, uid, generation }).strict(),
  z.object({ action: z.literal('role'), ...teamId, uid, generation, role: z.enum(['admin', 'member']) }).strict(),

]);
const hash = value => createHash('sha256').update(value).digest('hex');
const millis = value => value?.toMillis?.() ?? Number.NaN;
export const TEAM_MEMBER_LIMIT = 200;
export const USER_TEAM_LIMIT = 20;
export class TeamsError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const fail = (status, code) => { throw new TeamsError(status, code); };
const active = team => team?.state === 'active';
const pair = team => team?.kind === 'pair';
/** Owners and admins manage a team: they invite, and a friend's team offer (docs/FRIENDS.md) needs one. */
export const manager = member => ['owner', 'admin'].includes(member?.role);
/** A friendship is a hidden two-person `kind: 'pair'` team (docs/FRIENDS.md). Its id space is reserved, so no client can occupy one. */
export const PAIR_PREFIX = 'f_';
export const PAIR_CONVERSATION_ID = 'friend_chat';
const PAIR_NAME = 'Friends';
export function displayName(user) {
  return typeof user.name === 'string' && user.name.trim() ? user.name.trim().slice(0, 80) : 'Member';
}
const profile = user => ({ displayName: displayName(user) });
function summary(snapshot, member) {
  const team = snapshot.data();
  return { id: snapshot.id, name: team.name, ...(pair(team) ? { kind: 'pair' } : {}), memberCount: team.memberCount, role: member.role, generation: member.generation };
}

/** Writes a new pair team inside the caller's transaction: exactly two members (generation 1, role member) and the one
 * `dm` conversation, built by the same code as createTeamConversation. It is never added to `users/{uid}/teams`, so it
 * neither lists nor counts toward USER_TEAM_LIMIT. `members` is [{ uid, displayName }, { uid, displayName }]. */
export function createPairTeam(tx, db, { teamId, createdBy, members, now }) {
  const ref = db.doc(`teams/${teamId}`);
  tx.create(ref, { name: PAIR_NAME, kind: 'pair', createdBy, state: 'active', memberCount: 2, createdAt: now, updatedAt: now });
  for (const member of members) {
    tx.create(ref.collection('membershipEpochs').doc(member.uid), { generation: 1 });
    tx.create(ref.collection('members').doc(member.uid), { displayName: member.displayName, role: 'member', joinedAt: now, lastActiveAt: now, generation: 1 });
  }
  const { record } = conversationRecord({ kind: 'dm', title: PAIR_NAME, participants: members.map(member => `user:${member.uid}`), callerUid: createdBy });
  tx.create(ref.collection('conversations').doc(PAIR_CONVERSATION_ID), record);
}

/** Team deletion in the caller's transaction (after its reads): `members` are the membership document refs to remove. */
export function endTeam(tx, ref, members, now) {
  // Keep the team/epochs as a tombstone. A concurrent join cannot resurrect it.
  tx.update(ref, { state: 'deleted', memberCount: 0, deletedAt: now, updatedAt: now });
  for (const member of members) tx.delete(member);
}

/** Admits `user` to an ordinary team in the caller's transaction. The caller has done its own authorization (an invite
 * link, or a friend's team offer) and all its reads; `team` and `epoch` are the snapshots of the team and of the user's
 * membership epoch. Every way into a team ends here, so the member limit, the team limit, the new generation and the
 * writes (epoch, member, the user's team index, the member count, the membership signal, the user's offer of this team)
 * cannot differ between them. Returns the new generation. */
export async function admitMember(tx, db, { team, epoch, user, role, now }) {
  if (team.data().memberCount >= TEAM_MEMBER_LIMIT) fail(409, 'MEMBER_LIMIT');
  const memberships = await tx.get(db.collection(`users/${user.uid}/teams`).limit(USER_TEAM_LIMIT));
  if (memberships.size >= USER_TEAM_LIMIT) fail(409, 'TEAM_LIMIT');
  const generation = (epoch.data()?.generation ?? 0) + 1;
  if (!Number.isSafeInteger(generation)) fail(409, 'GENERATION_LIMIT');
  tx.set(epoch.ref, { generation });
  tx.create(team.ref.collection('members').doc(user.uid), { ...profile(user), role, generation, joinedAt: now, lastActiveAt: now });
  tx.create(db.doc(`users/${user.uid}/teams/${team.id}`), { role, generation, joinedAt: now });
  tx.update(team.ref, { memberCount: team.data().memberCount + 1, updatedAt: now });
  tx.set(team.ref.collection('signals').doc('membership'), { updatedAt: FieldValue.serverTimestamp() });
  // However they got in, a friend's open offer of this team is answered (a delete of an absent document is a no-op).
  tx.delete(db.doc(`users/${user.uid}/teamOffers/${team.id}`));
  return generation;
}

/** All membership changes read the team document, so join/delete/remove serialize. */
export class TeamsStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  team(id) { return this.db.doc(`teams/${id}`); }
  index(user, team) { return this.db.doc(`users/${user}/teams/${team}`); }
  async list(user) {
    return this.db.runTransaction(async tx => {
      const index = await tx.get(this.db.collection(`users/${user}/teams`).limit(USER_TEAM_LIMIT));
      if (index.empty) return { teams: [] };
      const snapshots = await tx.getAll(...index.docs.flatMap(doc => [this.team(doc.id), this.team(doc.id).collection('members').doc(user)]));
      const teams = [];
      for (let i = 0; i < snapshots.length; i += 2) {
        if (active(snapshots[i].data()) && !pair(snapshots[i].data()) && snapshots[i + 1].exists) teams.push(summary(snapshots[i], snapshots[i + 1].data()));
      }
      return { teams };
    });
  }
  async detail(user, tid) {
    id.parse(tid);
    return this.db.runTransaction(async tx => {
      const ref = this.team(tid);
      const [team, self] = await tx.getAll(ref, ref.collection('members').doc(user));
      if (!active(team.data()) || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      const members = await tx.get(ref.collection('members').limit(TEAM_MEMBER_LIMIT));
      const invitations = manager(self.data()) ? await tx.get(ref.collection('invites').orderBy('createdAt', 'desc').limit(50)) : null;
      return { team: summary(team, self.data()), members: members.docs.map(doc => ({ uid: doc.id,
        displayName: doc.data().displayName, role: doc.data().role, generation: doc.data().generation })),
      invites: invitations?.docs.map(doc => { const v = doc.data(); return { id: doc.id, role: v.role,
        expiresAt: v.expiresAt.toDate().toISOString(), uses: v.uses, maxUses: v.maxUses, revoked: v.revoked }; }) ?? [] };
    });
  }
  async execute(user, raw) {
    uid.parse(user.uid);
    const input = command.parse(raw), now = new Date(this.clock());
    if (input.action === 'create') {
      if (input.teamId.startsWith(PAIR_PREFIX)) fail(400, 'PAIR_TEAM');
      return this.db.runTransaction(async tx => {
        const ref = this.team(input.teamId), previous = await tx.get(ref);
        if (previous.exists) {
          if (!active(previous.data()) || previous.data().createdBy !== user.uid || previous.data().name !== input.name) fail(409, 'TEAM_ID_CONFLICT');
          return { teamId: ref.id };
        }
        const memberships = await tx.get(this.db.collection(`users/${user.uid}/teams`).limit(USER_TEAM_LIMIT));
        if (memberships.size >= USER_TEAM_LIMIT) fail(409, 'TEAM_LIMIT');
        tx.create(ref, { name: input.name, createdBy: user.uid, state: 'active', memberCount: 1, createdAt: now, updatedAt: now });
        tx.create(ref.collection('membershipEpochs').doc(user.uid), { generation: 1 });
        tx.create(ref.collection('members').doc(user.uid), { ...profile(user), role: 'owner', joinedAt: now, lastActiveAt: now, generation: 1 });
        tx.create(this.index(user.uid, ref.id), { role: 'owner', generation: 1, joinedAt: now });
        return { teamId: ref.id };
      });
    }
    if (input.action === 'join') {
      return this.db.runTransaction(async tx => {
        const invitationId = hash(input.code), lookup = await tx.get(this.db.doc(`teamInvites/${invitationId}`));
        if (!lookup.exists) fail(404, 'INVITE_UNAVAILABLE');
        const tid = lookup.data().teamId, ref = this.team(tid), memberRef = ref.collection('members').doc(user.uid);
        const invitationRef = ref.collection('invites').doc(invitationId), epochRef = ref.collection('membershipEpochs').doc(user.uid);
        const receiptRef = this.db.doc(`users/${user.uid}/teamJoinRequests/${input.requestId}`);
        const [team, member, invitation, epoch, receipt] = await tx.getAll(ref, memberRef, invitationRef, epochRef, receiptRef);
        const v = invitation.data();
        if (!active(team.data())) fail(404, 'INVITE_UNAVAILABLE');
        if (pair(team.data())) fail(400, 'PAIR_TEAM');
        if (receipt.exists) {
          const old = receipt.data();
          if (old.teamId !== tid || old.inviteId !== invitationId) fail(409, 'REQUEST_ID_CONFLICT');
          if (!member.exists || member.data().generation !== old.generation) fail(409, 'STALE_MEMBERSHIP');
          return { teamId: tid, generation: old.generation, alreadyMember: true };
        }
        // A response lost after a successful join is safe to retry even if it consumed the final use.
        if (member.exists) {
          tx.create(receiptRef, { teamId: tid, inviteId: invitationId, generation: member.data().generation });
          return { teamId: tid, generation: member.data().generation, alreadyMember: true };
        }
        if (!v || v.revoked || millis(v.expiresAt) <= this.clock() || v.uses >= v.maxUses) fail(409, 'INVITE_UNAVAILABLE');
        const next = await admitMember(tx, this.db, { team, epoch, user, role: v.role, now });
        tx.create(receiptRef, { teamId: tid, inviteId: invitationId, generation: next });
        tx.update(invitationRef, { uses: v.uses + 1 });
        return { teamId: tid, generation: next, alreadyMember: false };
      });
    }
    // Generate the secret once outside the retriable transaction. Never expose it through rules.
    const code = input.action === 'invite' ? randomBytes(24).toString('base64url') : null;
    return this.db.runTransaction(async tx => {
      const ref = this.team(input.teamId), selfRef = ref.collection('members').doc(user.uid);
      const [team, self] = await tx.getAll(ref, selfRef);
      if (!active(team.data()) || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (pair(team.data())) fail(400, 'PAIR_TEAM');
      const role = self.data().role;
      if (input.action === 'invite') {
        if (!manager(self.data()) || (input.role === 'admin' && role !== 'owner')) fail(403, 'ROLE_REQUIRED');
        const operation = ref.collection('inviteRequests').doc(hash(`${user.uid}:${input.requestId}`));
        const old = await tx.get(operation);
        const fingerprint = hash(JSON.stringify({ role: input.role, maxUses: input.maxUses, ttlSeconds: input.ttlSeconds }));
        if (old.exists) {
          if (old.data().fingerprint !== fingerprint) fail(409, 'REQUEST_ID_CONFLICT');
          return { code: old.data().code, teamId: ref.id };
        }
        const invitationId = hash(code);
        tx.create(ref.collection('invites').doc(invitationId), { role: input.role, createdBy: user.uid, createdAt: now,
          expiresAt: new Date(now.getTime() + input.ttlSeconds * 1000), maxUses: input.maxUses, uses: 0, revoked: false });
        tx.create(this.db.doc(`teamInvites/${invitationId}`), { teamId: ref.id });
        tx.create(operation, { fingerprint, code, expiresAt: new Date(now.getTime() + input.ttlSeconds * 1000) });
        return { code, teamId: ref.id };
      }
      if (input.action === 'revokeInvite') {
        const invitation = ref.collection('invites').doc(input.inviteId), previous = await tx.get(invitation);
        if (!manager(self.data()) || (previous.data()?.role === 'admin' && role !== 'owner')) fail(403, 'ROLE_REQUIRED');
        if (!previous.exists) fail(404, 'INVITE_UNAVAILABLE');
        tx.update(invitation, { revoked: true });
        return { ok: true };
      }
      if (input.action === 'delete') {
        if (role !== 'owner') fail(403, 'OWNER_REQUIRED');
        const members = await tx.get(ref.collection('members').limit(TEAM_MEMBER_LIMIT));
        endTeam(tx, ref, members.docs.map(member => member.ref), now);
        for (const member of members.docs) tx.delete(this.index(member.id, ref.id));
        return { ok: true };
      }
      const targetUid = input.action === 'leave' ? user.uid : input.uid;
      const targetRef = ref.collection('members').doc(targetUid), target = targetUid === user.uid ? self : await tx.get(targetRef);
      if (input.action !== 'leave' && (!manager(self.data()) || targetUid === user.uid
        || (role !== 'owner' && target.data()?.role !== 'member'))) fail(403, 'ROLE_REQUIRED');
      if (!target.exists) fail(409, 'STALE_MEMBERSHIP');
      if (target.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      if (target.data().role === 'owner') fail(409, 'OWNER_CANNOT_LEAVE');
      if (input.action === 'role') {
        if (role !== 'owner') fail(403, 'OWNER_REQUIRED');
        tx.update(targetRef, { role: input.role });
        tx.update(this.index(targetUid, ref.id), { role: input.role });
      } else {
        tx.delete(targetRef);
        tx.delete(this.index(targetUid, ref.id));
        tx.update(ref, { memberCount: team.data().memberCount - 1, updatedAt: now });
      }
      tx.set(ref.collection('signals').doc('membership'), { updatedAt: FieldValue.serverTimestamp() });
      return { ok: true };
    });
  }
}

/** HTTP authorization is deliberately independent from owner-only Remote Control. */
export function createTeamsHandler({ store, friends, verifyToken, createCustomToken, origins = [] }) {
  return async request => {
    const headers = new Headers({ 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff', Vary: 'Origin' });
    const reply = (body, status = 200) => new Response(JSON.stringify(body), { status, headers });
    try {
      const origin = request.headers.get('Origin');
      if (origin && !origins.includes(origin)) fail(403, 'ORIGIN_NOT_ALLOWED');
      if (origin) headers.set('Access-Control-Allow-Origin', origin);
      if (request.method === 'OPTIONS') {
        headers.set('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
        headers.set('Access-Control-Allow-Headers', 'Authorization, Content-Type');
        return new Response(null, { status: 204, headers });
      }
      const path = new URL(request.url).pathname;
      const bearer = /^Bearer (\S+)$/u.exec(request.headers.get('Authorization') ?? '');
      if (!bearer) fail(401, 'SIGN_IN_REQUIRED');
      let user;
      try { user = await verifyToken(bearer[1]); } catch { fail(401, 'INVALID_SESSION'); }
      if (!uid.safeParse(user?.uid).success) fail(401, 'INVALID_SESSION');
      if (path === '/v1/teams' && request.method === 'GET') return reply(await store.list(user.uid));
      if (friends && path === '/v1/friends/profile' && request.method === 'GET') return reply(await friends.profile(user));
      const detail = /^\/v1\/teams\/([A-Za-z0-9_-]{8,64})$/u.exec(path);
      if (detail && detail[1] !== 'session' && request.method === 'GET') return reply(await store.detail(user.uid, detail[1]));
      if (request.method !== 'POST' || !['/v1/teams', '/v1/teams/session', '/v1/teams/messages', ...(friends ? ['/v1/friends'] : [])].includes(path)) fail(404, 'NOT_FOUND');
      const raw = await request.text();
      if (Buffer.byteLength(raw) > (path === '/v1/teams/messages' ? 1048576 : 16384)) fail(413, 'REQUEST_TOO_LARGE');
      let body;
      try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
      if (path === '/v1/teams/session') {
        z.object({}).strict().parse(body);
        return reply({ uid: user.uid, customToken: await createCustomToken(user.uid) });
      }
      if (path === '/v1/teams/messages') return reply(await new TeamMessagesStore(store.db, store.clock).execute(user, body));
      if (path === '/v1/friends') return reply(await friends.execute(user, body));
      return reply(await store.execute(user, body));
    } catch (error) {
      if (error instanceof z.ZodError) return reply({ error: { code: 'INVALID_INPUT' } }, 400);
      if (error instanceof TeamMessageError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof SharedBotError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof TeamsError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof TeamConversationError) return reply({ error: { code: error.code } }, error.status);
      return reply({ error: { code: 'SERVICE_UNAVAILABLE' } }, 503);
    }
  };
}
