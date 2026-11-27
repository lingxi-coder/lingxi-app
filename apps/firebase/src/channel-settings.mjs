import { randomBytes } from 'node:crypto';
import { FieldValue } from 'firebase-admin/firestore';
import { z } from 'zod';

const segment = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const principal = z.string().min(6).max(139).regex(/^(user:|shared_bot:).+$/u);
const posting = z.object({
  mode: z.enum(['chat', 'posts']),
  senders: z.enum(['all', 'selected']),
  selected: z.array(principal).max(200),
  replies: z.enum(['all', 'authors', 'none'])
}).strict().refine(p => p.senders !== 'selected' || p.selected.length > 0);
const actions = {
  list: {}, get: { conversationId: segment },
  profile: { conversationId: segment, title: z.string().trim().min(1).max(120), description: z.string().trim().max(1000) },
  posting: { conversationId: segment, posting },
  admin: { conversationId: segment, uid, enabled: z.boolean() },
  remove: { conversationId: segment, uid },
  detachBot: { conversationId: segment, botId: segment },
  createInvite: { conversationId: segment },
  revokeInvite: { conversationId: segment, code: z.string().length(32) },
  redeem: { conversationId: segment, code: z.string().regex(/^[A-Za-z0-9_-]{32}$/u) }
};
export const channelCommand = z.object({
  action: z.literal('channelSettings'),
  teamId: segment,
  generation: z.number().int().positive().max(Number.MAX_SAFE_INTEGER),
  request: z.discriminatedUnion('op', Object.entries(actions).map(([op, fields]) => z.object({ op: z.literal(op), ...fields }).strict()))
}).strict();
export class ChannelError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const fail = (status, code) => { throw new ChannelError(status, code); };
const manager = member => ['owner', 'admin'].includes(member?.role);

export function mayPost(channel, actor, reply = false) {
  if (channel.kind !== 'channel') return true;
  if (!channel.participants.includes(actor)) return false;
  const rule = channel.posting;
  if (!rule) return true;
  const canStart = rule.senders === 'all' || rule.selected.includes(actor);
  return reply ? rule.replies === 'all' || (rule.replies === 'authors' && canStart) : canStart;
}

/** History is immutable across mode changes. Only an actual post can be a thread root. */
export function channelMessageFields(channel, actor, replied, replyTo, expectedMode, failWith = fail) {
  if (channel.kind !== 'channel') return {};
  const mode = channel.posting?.mode ?? 'chat';
  if (expectedMode !== undefined && mode !== expectedMode) failWith(409, 'CHANNEL_MODE_CHANGED');
  const postId = replied?.postId ?? (replied?.channelPost ? replyTo : undefined);
  if (postId && mode !== 'posts') failWith(409, 'CHANNEL_MODE_CHANGED');
  if (!mayPost(channel, actor, Boolean(postId))) failWith(403, 'CHANNEL_POSTING_DENIED');
  if (postId) return { postId };
  if (mode === 'posts') {
    if (replyTo) failWith(409, 'CHANNEL_POST_REQUIRED');
    return { channelPost: true, postStats: { replies: 0, likes: 0, views: 0, revision: 0 } };
  }
  return {};
}

/** The message and its thread count commit together, after the sender's idempotency check. */
export function countPostReply(tx, conversation, postId, amount = 1) {
  if (postId) tx.update(conversation.collection('messages').doc(postId), {
    'postStats.replies': FieldValue.increment(amount),
    'postStats.revision': FieldValue.increment(1),
  });
}

/** The viewer has already passed the current team generation and conversation read checks. */
export async function interactWithPost(tx, conversation, channel, userId, postId, liked) {
  if (channel.kind !== 'channel') fail(404, 'CHANNEL_POST_REQUIRED');
  if (liked !== undefined && !channel.participants.includes('user:' + userId)) fail(403, 'CONVERSATION_REQUIRED');
  const ref = conversation.collection('messages').doc(postId);
  const viewer = ref.collection('postViewers').doc(userId);
  const [post, activity] = await tx.getAll(ref, viewer);
  const root = post.data();
  if (!root?.channelPost || root.recalled) fail(404, 'CHANNEL_POST_REQUIRED');

  const stats = { replies: 0, likes: 0, views: 0, revision: 0, ...root.postStats };
  const legacy = root.postStats?.views === undefined;
  if (legacy) {
    const replies = conversation.collection('messages').where('postId', '==', postId);
    const [total, recalled] = await Promise.all([tx.get(replies.count()), tx.get(replies.where('recalled', '==', true).count())]);
    stats.replies = total.data().count - recalled.data().count;
  }
  const previousLike = activity.data()?.liked === true;
  const nextLike = liked ?? previousLike;
  if (!activity.exists || nextLike !== previousLike || legacy) {
    if (!activity.exists) stats.views++;
    stats.likes += Number(nextLike) - Number(previousLike);
    stats.revision++;
    tx.set(viewer, { liked: nextLike, revision: stats.revision });
    tx.update(ref, { postStats: stats });
  }
  return { stats, liked: nextLike };
}

export const channelFields = value => value.kind === 'channel' ? {
  ...(value.description !== undefined ? { description: value.description } : {}),
  ...(value.posting ? { posting: value.posting } : {}),
  ...(value.channelAdmins ? { channelAdmins: value.channelAdmins } : {})
} : {};

export async function executeChannelCommand(db, user, command) {
  const { teamId, generation, request } = command;
  const team = db.doc(`teams/${teamId}`), members = team.collection('members'), channels = team.collection('conversations');
  const actor = `user:${user.uid}`;
  return db.runTransaction(async tx => {
    const [teamDoc, selfDoc] = await tx.getAll(team, members.doc(user.uid));
    const self = selfDoc.data();
    if (teamDoc.data()?.state !== 'active' || teamDoc.data()?.kind === 'pair' || !self) fail(403, 'CHANNEL_TEAM_REQUIRED');
    if (generation !== self.generation) fail(409, 'STALE_MEMBERSHIP');
    const teamAdmin = manager(self);
    if (request.op === 'list') {
      if (!teamAdmin) fail(403, 'CHANNEL_ADMIN_REQUIRED');
      const page = await tx.get(channels.where('kind', '==', 'channel'));
      return { channels: page.docs.map(doc => ({ id: doc.id, title: doc.data().title })) };
    }
    const ref = channels.doc(request.conversationId), invite = team.collection('channelInvites').doc(ref.id);
    const c = (await tx.get(ref)).data();
    const participants = c?.participants ?? [];
    const reads = c?.memberUids?.includes(user.uid);
    const appoint = request.op === 'admin' || (request.op === 'get' && teamAdmin);
    if (c?.kind !== 'channel' || (!reads && !appoint && request.op !== 'redeem')) fail(404, 'CHANNEL_UNAVAILABLE');
    const canManage = Boolean(reads && (teamAdmin || c.channelAdmins?.[actor] === generation && participants.includes(actor)));
    const humanMembers = participants.filter(id => id.startsWith('user:')).map(id => id.slice(5));
    if (request.op === 'get') {
      if (!canManage && !teamAdmin) fail(403, 'CHANNEL_ADMIN_REQUIRED');
      const membership = humanMembers.length ? await tx.getAll(...humanMembers.map(id => members.doc(id))) : [];
      const admins = membership.filter(doc => doc.exists && c.channelAdmins?.[`user:${doc.id}`] === doc.data().generation).map(doc => doc.id);
      const code = canManage ? (await tx.get(invite)).data()?.code ?? null : null;
      return { members: membership.filter(doc => doc.exists).map(doc => doc.id), admins, code };
    }
    if (request.op === 'redeem') {
      const current = (await tx.get(invite)).data();
      if (!current || current.code !== request.code) fail(404, 'CHANNEL_INVITE_INVALID');
      if (!participants.includes(actor)) {
        if (participants.length >= 200) fail(409, 'CHANNEL_MEMBER_LIMIT');
        tx.update(ref, { participants: [...participants, actor].sort(), memberUids: [...new Set([...c.memberUids, user.uid])].sort() });
      }
      return { conversationId: ref.id };
    }
    if (request.op === 'admin') {
      if (!teamAdmin) fail(403, 'CHANNEL_ADMIN_REQUIRED');
      const target = `user:${request.uid}`, admins = { ...c.channelAdmins };
      if (request.enabled) {
        const member = (await tx.get(members.doc(request.uid))).data();
        if (!member || !participants.includes(target)) fail(409, 'CHANNEL_MEMBER_REQUIRED');
        admins[target] = member.generation;
      } else delete admins[target];
      tx.update(ref, { channelAdmins: admins });
      return { ok: true };
    }
    const detach = request.op === 'detachBot';
    if (!canManage && !detach) fail(403, 'CHANNEL_ADMIN_REQUIRED');
    if (request.op === 'profile') {
      tx.update(ref, { title: request.title, description: request.description });
    } else if (request.op === 'posting') {
      const selected = request.posting.senders === 'selected' ? [...new Set(request.posting.selected)].sort() : [];
      if (selected.some(id => !participants.includes(id))) fail(409, 'CHANNEL_MEMBER_REQUIRED');
      const people = selected.filter(id => id.startsWith('user:'));
      if (people.length && (await tx.getAll(...people.map(id => members.doc(id.slice(5))))).some(doc => !doc.exists)) fail(409, 'CHANNEL_MEMBER_REQUIRED');
      tx.update(ref, { posting: { ...request.posting, selected } });
    } else if (request.op === 'createInvite') {
      const current = (await tx.get(invite)).data()?.code;
      const code = current ?? randomBytes(24).toString('base64url');
      if (!current) tx.set(invite, { code });
      return { code };
    } else if (request.op === 'revokeInvite') {
      const current = (await tx.get(invite)).data()?.code;
      if (current && current !== request.code) fail(409, 'CHANNEL_INVITE_CHANGED');
      if (current) tx.delete(invite);
      return { code: null };
    } else if (request.op === 'remove' || detach) {
      const target = detach ? `shared_bot:${request.botId}` : `user:${request.uid}`;
      if (!participants.includes(target)) return { ok: true };
      const bots = participants.filter(id => id.startsWith('shared_bot:')).map(id => id.slice(11));
      const botDocs = bots.length ? await tx.getAll(...bots.map(id => team.collection('sharedBots').doc(id))) : [];
      const retainedBots = botDocs.filter(doc => doc.id !== request.botId);
      if (detach) {
        const maintainer = botDocs.find(doc => doc.id === request.botId)?.data()?.identity?.maintainer;
        if (!reads || (!teamAdmin && !(maintainer?.uid === user.uid && maintainer?.generation === generation))) fail(403, 'CHANNEL_ADMIN_REQUIRED');
      } else {
        const member = (await tx.get(members.doc(request.uid))).data();
        if (request.uid === user.uid || manager(member) || (member && c.channelAdmins?.[target] === member.generation)) fail(403, 'CHANNEL_MEMBER_PROTECTED');
        const dependencies = retainedBots.filter(doc => doc.data()?.identity?.maintainer?.uid === request.uid)
          .map(doc => ({ id: doc.id, name: doc.data()?.name ?? doc.id }));
        if (dependencies.length) return { blockedBy: dependencies };
      }
      const remaining = participants.filter(id => id !== target);
      const readers = new Set(remaining.filter(id => id.startsWith('user:')).map(id => id.slice(5)));
      for (const doc of retainedBots) {
        const owner = doc.data()?.identity?.maintainer?.uid;
        if (owner && c.memberUids.includes(owner)) readers.add(owner);
      }
      const admins = { ...c.channelAdmins };
      delete admins[target];
      const changes = { participants: remaining, memberUids: [...readers].sort(), channelAdmins: admins };
      if (c.posting?.selected.includes(target)) changes.posting = { ...c.posting, selected: c.posting.selected.filter(id => id !== target) };
      if (detach && c.autoReply?.targets.includes(request.botId)) changes.autoReply = {
        ...c.autoReply, revision: c.autoReply.revision + 1, targets: c.autoReply.targets.filter(id => id !== request.botId)
      };
      tx.update(ref, changes);
    }
    return { ok: true };
  });
}
