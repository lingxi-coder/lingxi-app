import { LingxiDataError } from './lingxi-data.mjs';
import { retireTeamVisualizations } from './team-visualization-storage.mjs';
import { FieldValue } from 'firebase-admin/firestore';
import { createHash, randomBytes } from 'node:crypto';
import { z } from 'zod';
import { TeamConversationError, conversationRecord } from './team-conversations.mjs';
import { ChannelError } from './channel-settings.mjs';
import { TeamMessagesStore, TeamMessageError } from './team-messages.mjs';
import { SharedBotError, SharedBotsStore } from './shared-bots.mjs';
import { HostedBotServiceError, HostedBotServices, SERVICE_PREFIX } from './hosted-bot-service.mjs';

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
  z.object({ action: z.literal('removalBlockers'), ...teamId, uid, generation }).strict(),
  z.object({ action: z.literal('pauseRemovalBot'), ...teamId, uid, generation, actorGeneration: generation,
    botId: id, revision: generation }).strict(),
  z.object({ action: z.literal('role'), ...teamId, uid, generation, role: z.enum(['admin', 'member']) }).strict(),
  z.object({ action: z.literal('handOver'), ...teamId, uid, generation }).strict(),

]);
const TEAM_PROFILE_PATH = /^\/v1\/teams\/([A-Za-z0-9_-]{8,64})\/profile$/u;
const TEAM_AVATAR_BYTES = 48 * 1024;
const TEAM_AVATAR_EDGE = 96;
const profileCommand = z.object({
  action: z.literal('updateProfile'), ...teamId, generation,
  version: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  changes: z.object({
    name: z.string().trim().min(1).max(80).optional(),
    description: z.string().trim().max(1000).optional(),
    avatar: z.string().min(1).max(4 * Math.ceil(TEAM_AVATAR_BYTES / 3)).regex(/^[A-Za-z0-9+/]+={0,2}$/u).nullable().optional(),
  }).strict().refine(changes => Object.keys(changes).length > 0),
}).strict();
const PNG_HEADER = Buffer.from('89504e470d0a1a0a0000000d49484452', 'hex');
const PNG_END = Buffer.from('0000000049454e44ae426082', 'hex');
function validTeamAvatar(avatar) {
  const png = Buffer.from(avatar, 'base64');
  return png.length >= 45 && png.length <= TEAM_AVATAR_BYTES && png.toString('base64') === avatar
    && png.subarray(0, PNG_HEADER.length).equals(PNG_HEADER) && png.subarray(-PNG_END.length).equals(PNG_END)
    && png.readUInt32BE(16) === TEAM_AVATAR_EDGE && png.readUInt32BE(20) === TEAM_AVATAR_EDGE;
}

const teamProfileRef = team => team.collection('profile').doc('current');
function teamProfile(value) {
  return { version: value?.version ?? 0, description: value?.description ?? '', avatar: value?.avatar ?? null };
}
async function publishedBotsBy(tx, team, memberId, membershipGeneration) {
  const bots = await tx.get(team.collection('sharedBots').where('identity.maintainer.uid', '==', memberId));
  return bots.docs.flatMap(doc => {
    const { identity, name } = doc.data();
    return identity.state === 'published' && identity.maintainer.generation === membershipGeneration
      ? [{ id: doc.id, name, revision: identity.revision }] : [];
  });
}
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
      const [members, invitations, settings] = await Promise.all([
        tx.get(ref.collection('members').limit(TEAM_MEMBER_LIMIT)),
        manager(self.data()) ? tx.get(ref.collection('invites').orderBy('createdAt', 'desc').limit(50)) : null,
        pair(team.data()) ? null : tx.get(teamProfileRef(ref)),
      ]);
      return { team: summary(team, self.data()), members: members.docs.map(doc => ({ uid: doc.id,
        displayName: doc.data().displayName, role: doc.data().role, generation: doc.data().generation })),
      invites: invitations?.docs.map(doc => { const v = doc.data(); return { id: doc.id, role: v.role,
        expiresAt: v.expiresAt.toDate().toISOString(), uses: v.uses, maxUses: v.maxUses, revoked: v.revoked }; }) ?? [],
      ...(settings ? { profile: teamProfile(settings.data()) } : {}) };
    });
  }
  async updateProfile(user, raw) {
    const input = profileCommand.parse(raw);
    const { changes } = input;
    if (changes.avatar && !validTeamAvatar(changes.avatar)) fail(400, 'TEAM_AVATAR_INVALID');
    return this.db.runTransaction(async tx => {
      const ref = this.team(input.teamId), settings = teamProfileRef(ref);
      const [team, self, profileDoc] = await tx.getAll(ref, ref.collection('members').doc(user), settings);
      const previous = team.data();
      if (!active(previous) || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (pair(previous)) fail(400, 'PAIR_TEAM');
      if (self.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      if (!manager(self.data())) fail(403, 'ROLE_REQUIRED');

      const current = teamProfile(profileDoc.data());
      const name = changes.name ?? previous.name;
      const description = changes.description ?? current.description;
      const avatar = changes.avatar === undefined ? current.avatar : changes.avatar;
      // A transport retry of an already saved change is safe even if the version has advanced.
      if (name === previous.name && description === current.description && avatar === current.avatar) return { ok: true };
      if (input.version !== current.version || current.version >= Number.MAX_SAFE_INTEGER) fail(409, 'TEAM_PROFILE_CHANGED');
      const members = await tx.get(ref.collection('members').limit(TEAM_MEMBER_LIMIT));
      const version = current.version + 1;
      tx.set(settings, { version, description, avatar });
      tx.update(ref, { name, createdName: previous.createdName ?? previous.name, updatedAt: new Date(this.clock()) });
      // Every client already watches its team index. Refresh its names and selected detail without duplicating the thumbnail.
      for (const member of members.docs) tx.set(this.index(member.id, ref.id), { profileVersion: version }, { merge: true });
      return { ok: true };
    });
  }
  async execute(user, raw) {
    uid.parse(user.uid);
    const input = command.parse(raw), now = new Date(this.clock());
    if (input.action === 'create') {
      if (input.teamId.startsWith(PAIR_PREFIX)) fail(400, 'PAIR_TEAM');
      if (input.teamId.startsWith(SERVICE_PREFIX)) fail(400, 'RESERVED_TEAM_ID');
      return this.db.runTransaction(async tx => {
        const ref = this.team(input.teamId), previous = await tx.get(ref);
        if (previous.exists) {
          const original = previous.data();
          if (!active(original) || original.createdBy !== user.uid || (original.createdName ?? original.name) !== input.name) fail(409, 'TEAM_ID_CONFLICT');
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
        tx.delete(teamProfileRef(ref));
        retireTeamVisualizations(tx, ref, now.getTime());
        for (const member of members.docs) tx.delete(this.index(member.id, ref.id));
        return { ok: true };
      }
      const targetUid = input.action === 'leave' ? user.uid : input.uid;
      const targetRef = ref.collection('members').doc(targetUid), target = targetUid === user.uid ? self : await tx.get(targetRef);
      if (input.action === 'handOver') {
        // The owner gives the team to a current member and stays as an admin. Owning the team is a role of its own: every Bot stays
        // with its maintainer, every Chat with its readers, and nobody's model account or shared files change hands.
        if (targetUid === user.uid) fail(403, 'ROLE_REQUIRED');
        if (!target.exists || target.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
        // Asked again after a lost answer: that member already owns the team.
        if (target.data().role === 'owner' && manager(self.data())) return { ok: true };
        if (role !== 'owner') fail(403, 'OWNER_REQUIRED');
        tx.update(targetRef, { role: 'owner' }); tx.update(this.index(targetUid, ref.id), { role: 'owner' });
        tx.update(selfRef, { role: 'admin' }); tx.update(this.index(user.uid, ref.id), { role: 'admin' });
        tx.update(ref, { updatedAt: now });
        tx.set(ref.collection('signals').doc('membership'), { updatedAt: FieldValue.serverTimestamp() });
        return { ok: true };
      }
      if (input.action !== 'leave' && (!manager(self.data()) || targetUid === user.uid
        || (role !== 'owner' && target.data()?.role !== 'member'))) fail(403, 'ROLE_REQUIRED');
      if (!target.exists) fail(409, 'STALE_MEMBERSHIP');
      if (target.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      if (target.data().role === 'owner') fail(409, 'OWNER_CANNOT_LEAVE');
      if (input.action === 'removalBlockers' || input.action === 'remove') {
        const blockingBots = await publishedBotsBy(tx, ref, targetUid, input.generation);
        if (input.action === 'removalBlockers') return { blockingBots };
        if (blockingBots.length) fail(409, 'TEAM_MEMBER_HAS_BOTS');
      }
      if (input.action === 'pauseRemovalBot') {
        if (self.data().generation !== input.actorGeneration) fail(409, 'STALE_MEMBERSHIP');
        const botRef = ref.collection('sharedBots').doc(input.botId), bot = (await tx.get(botRef)).data();
        const identity = bot?.identity;
        if (identity?.maintainer.uid !== targetUid || identity.maintainer.generation !== input.generation) fail(409, 'BOT_REVISION_CONFLICT');
        if (identity.state === 'paused' && identity.revision === input.revision + 1) return { ok: true };
        if (identity.state !== 'published' || identity.revision !== input.revision) fail(409, 'BOT_REVISION_CONFLICT');
        await new SharedBotsStore(this.db, this.clock).pausePublication(tx, ref, botRef, bot, now.getTime());
        return { ok: true };
      }
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
const datasetPath = '/v1/teams/datasets';
const capturePath = '/v1/teams/dataset-captures';
const pagesPath = '/v1/teams/dataset-pages';

export function datasetBodyLimit(path) {
  if (path === capturePath || path === pagesPath || path === '/v1/teams/visualizations') return 3 * 1024 * 1024;
  if (path === datasetPath || path === '/v2/dataset-runs' || path.startsWith('/v1/dataset-browser/')) return 131072;
  return undefined;
}

export function teamProfileBodyLimit(path) {
  return TEAM_PROFILE_PATH.test(path) ? 80 * 1024 : undefined;
}

async function boundedDatasetBody(request, limit) {
  if (Number(request.headers.get('Content-Length')) > limit) fail(413, 'REQUEST_TOO_LARGE');
  const reader = request.body?.getReader();
  let raw = '';
  if (!reader) return raw;
  const decoder = new TextDecoder();
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return raw + decoder.decode();
      size += value.byteLength;
      if (size > limit) {
        void reader.cancel().catch(() => undefined);
        fail(413, 'REQUEST_TOO_LARGE');
      }
      raw += decoder.decode(value, { stream: true });
    }
  } finally { reader.releaseLock(); }
}

export function createTeamsHandler({ store, friends, inbox, data, datasets, datasetCommand, datasetRuns, datasetPages, datasetRender, visualizations, verifyToken, createCustomToken, origins = [] }) {
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
      if (datasetRuns && path === '/v2/dataset-runs' && request.method === 'POST') {
        const raw = await request.text();
        if (Buffer.byteLength(raw) > 131072) fail(413, 'REQUEST_TOO_LARGE');
        let body;
        try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
        return reply(await datasetRuns(store.db, request.headers.get('Authorization') ?? '', body));
      }
      const bearer = /^Bearer (\S+)$/u.exec(request.headers.get('Authorization') ?? '');
      if (!bearer) fail(401, 'SIGN_IN_REQUIRED');
      async function identity() {
        let user;
        try { user = await verifyToken(bearer[1]); } catch { fail(401, 'INVALID_SESSION'); }
        if (!uid.safeParse(user?.uid).success) fail(401, 'INVALID_SESSION');
        return user;
      }
      if (path === '/v1/teams/visualizations' && request.method === 'POST') {
        if (!visualizations) fail(404, 'VISUALIZATION_UNAVAILABLE');
        const user = await identity();
        const raw = await boundedDatasetBody(request, 3 * 1024 * 1024);
        let body;
        try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
        return reply(await visualizations.execute(user, bearer[1], body));
      }
      // Core authenticates forwarded commands. Product-side operations still
      // require the full verified identity before touching product resources.
      if (datasets && !inbox && (path === datasetPath || path === capturePath) && request.method === 'POST') {
        const raw = await boundedDatasetBody(request, datasetBodyLimit(path));
        let body;
        try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
        if (path === capturePath && body?.command?.action !== 'publishCapture') fail(400, 'INVALID_DATA_REQUEST');
        if (body?.action === 'page' && datasetPages) return reply(await datasetPages(await identity(), bearer[1], body.request));
        if (body?.action === 'render' && datasetRender) {
          return reply(await datasetRender(await identity(), bearer[1], body.request));
        }
        if (datasetCommand && body?.action !== 'share') return reply(await datasetCommand(bearer[1], body));
        return reply(await datasets(store.db, await identity(), bearer[1], body));
      }
      const user = await identity();
      if (inbox) {
        if (!['/','/v1/inbox','/inboxApi'].includes(path) || request.method !== 'POST') fail(404, 'NOT_FOUND');
        const raw = await request.text();
        if (Buffer.byteLength(raw) > 32768) fail(413,'REQUEST_TOO_LARGE');
        let body;
        try { body = JSON.parse(raw); } catch { fail(400,'INVALID_JSON'); }
        try { return reply(await inbox.execute(user.uid,body)); }
        catch (error) {
          if (Number.isInteger(error.status) && error.status >= 400 && error.status <= 599 && typeof error.code === 'string') return reply({error:{code:error.code}},error.status);
          throw error;
        }
      }
      if (path === '/v1/teams' && request.method === 'GET') return reply(await store.list(user.uid));
      if (friends && path === '/v1/friends/profile' && request.method === 'GET') return reply(await friends.profile(user));
      const detail = /^\/v1\/teams\/([A-Za-z0-9_-]{8,64})$/u.exec(path);
      if (detail && detail[1] !== 'session' && request.method === 'GET') return reply(await store.detail(user.uid, detail[1]));
      const profile = TEAM_PROFILE_PATH.exec(path);
      if (profile && request.method === 'POST') {
        const raw = await boundedDatasetBody(request, teamProfileBodyLimit(path));
        let body;
        try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
        if (body?.teamId !== profile[1]) fail(400, 'INVALID_INPUT');
        return reply(await store.updateProfile(user.uid, body));
      }
      if (request.method !== 'POST' || !['/v1/teams', '/v1/teams/session', '/v1/teams/messages', '/v1/services', ...(data ? ['/v1/teams/data'] : []), ...(datasetPages ? ['/v1/teams/dataset-pages'] : []), ...(friends ? ['/v1/friends'] : [])].includes(path)) fail(404, 'NOT_FOUND');
      const raw = path === pagesPath ? await boundedDatasetBody(request, datasetBodyLimit(path)) : await request.text();
      const size = Buffer.byteLength(raw);
      const limit = { '/v1/teams/dataset-pages': 3 * 1024 * 1024, '/v1/teams/data': 3 * 1024 * 1024, '/v1/teams/messages': 1048576 }[path] ?? 16384;
      if (size > limit) fail(413, 'REQUEST_TOO_LARGE');
      let body;
      try { body = JSON.parse(raw); } catch { fail(400, 'INVALID_JSON'); }
      if (path === '/v1/teams/session') {
        z.object({}).strict().parse(body);
        return reply({ uid: user.uid, customToken: await createCustomToken(user.uid) });
      }
      if (path === '/v1/teams/dataset-pages') return reply(await datasetPages(user, bearer[1], body));
      if (path === '/v1/teams/data') {
        if (size > 131072 && body?.command?.action !== 'publishReport') fail(413, 'REQUEST_TOO_LARGE');
        return reply(await data(user.uid, bearer[1], body));
      }
      if (path === '/v1/teams/messages') return reply(await new TeamMessagesStore(store.db, store.clock, data && (request => data(user.uid, bearer[1], request))).execute(user, body));
      // Hosted Bot services: the commands about a service itself. Its sessions use /v1/teams/messages with `serviceId`.
      if (path === '/v1/services') return reply(await new HostedBotServices(store.db, store.clock).execute(user, body));
      if (path === '/v1/friends') return reply(await friends.execute(user, body));
      return reply(await store.execute(user, body));
    } catch (error) {
      if (error instanceof z.ZodError) return reply({ error: { code: 'INVALID_INPUT' } }, 400);
      if (error instanceof ChannelError || error instanceof TeamMessageError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof SharedBotError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof HostedBotServiceError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof LingxiDataError) return reply({ error: { code: error.code } }, error.status);
      if (error instanceof TeamsError) return reply({ error: {
        code: error.code, ...(error.retryAfterMs !== undefined ? { retryAfterMs: error.retryAfterMs } : {}),
      } }, error.status);
      if (error instanceof TeamConversationError) return reply({ error: { code: error.code } }, error.status);
      // The reply says nothing of the cause. The log names its class and gRPC status (a Firestore PERMISSION_DENIED is 7) and the path, never its message:
      // that can carry document paths and request data.
      console.error(JSON.stringify({ severity: 'ERROR', message: 'teams.unhandled', path: new URL(request.url).pathname,
        error: error?.constructor?.name ?? typeof error, ...(Number.isInteger(error?.code) ? { status: error.code } : {}) }));
      return reply({ error: { code: 'SERVICE_UNAVAILABLE' } }, 503);
    }
  };
}
