import { createHash } from 'node:crypto';
import { z } from 'zod';
import { botPermits } from './team-bots.mjs';

const segment = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const inputSchema = z.object({
  teamId: segment, conversationId: id, callerUid: segment, callerGeneration: z.number().int().positive(),
  kind: z.enum(['channel', 'group', 'dm', 'bot_dm']),
  title: z.string().max(120), participants: z.array(z.string().max(300)).min(1).max(500),
}).strict();

export class TeamConversationError extends Error {
  constructor(status, message, code = 'CONVERSATION_REJECTED') { super(message); this.status = status; this.code = code; }
}

function principals(values) {
  if (new Set(values).size !== values.length) throw new TeamConversationError(400, 'Duplicate participant');
  return values.map(value => {
    if (value.startsWith('user:') && segment.safeParse(value.slice(5)).success) return { kind: 'user', uid: value.slice(5) };
    const bot = /^bot:([A-Za-z0-9_-]{1,128}):([A-Za-z0-9_-]{22}):([A-Za-z0-9_-]{22})$/u.exec(value);
    if (bot) return { kind: 'bot', uid: bot[1], key: value };
    throw new TeamConversationError(400, 'Invalid participant');
  });
}

/** Trusted backend entry point: callerUid MUST come from a verified ID token, never the request body. */
export async function createTeamConversation(db, input) {
  const parsed = inputSchema.safeParse(input);
  if (!parsed.success) throw new TeamConversationError(400, 'Invalid conversation');
  const args = parsed.data;
  const parties = principals(args.participants);
  const humans = parties.filter(party => party.kind === 'user');
  const bots = parties.filter(party => party.kind === 'bot');
  if ((args.kind === 'dm' && (humans.length !== 2 || bots.length !== 0))
    || (args.kind === 'bot_dm' && (humans.length !== 1 || bots.length !== 1))
    || humans.length === 0 || (args.kind !== 'bot_dm' && parties.length < 2)) throw new TeamConversationError(400, 'Invalid conversation participants');
  const memberUids = [...new Set(parties.map(party => party.uid))].sort();
  if (!memberUids.includes(args.callerUid)) throw new TeamConversationError(403, 'Caller is not a participant');
  const participants = [...args.participants].sort();
  const requestDigest = createHash('sha256').update(JSON.stringify({
    kind: args.kind, title: args.title, participants, createdBy: args.callerUid,
  })).digest('hex');
  const team = db.doc(`teams/${args.teamId}`);
  const conversation = team.collection('conversations').doc(args.conversationId);
  return db.runTransaction(async transaction => {
    const [teamSnapshot, existing, ...snapshots] = await transaction.getAll(team, conversation,
      ...memberUids.map(uid => team.collection('members').doc(uid)),
      ...bots.map(bot => team.collection('bots').doc(bot.key)));
    if (teamSnapshot.data()?.state !== 'active' || !snapshots[memberUids.indexOf(args.callerUid)].exists) {
      throw new TeamConversationError(403, 'Current team membership required', 'MEMBERSHIP_REQUIRED');
    }
    if (snapshots.slice(0, memberUids.length).some(member => !member.exists)) throw new TeamConversationError(403, 'Current participants required', 'PARTICIPANT_REQUIRED');
    if (snapshots[memberUids.indexOf(args.callerUid)].data().generation !== args.callerGeneration) {
      throw new TeamConversationError(409, 'Stale membership', 'STALE_MEMBERSHIP');
    }
    const cards = snapshots.slice(memberUids.length);
    for (const [index, bot] of bots.entries()) {
      const card = cards[index].data();
      if (!card || card.enabled !== true || card.ownerGeneration !== snapshots[memberUids.indexOf(bot.uid)].data()?.generation
        || card.ownerUid !== bot.uid || bot.key !== `bot:${card.ownerUid}:${card.hostId}:${card.botId}`) {
        throw new TeamConversationError(403, 'Registered Bot required');
      }
      // A private Bot thread grants the human access; an owner cannot bypass the
      // target Bot's policy by creating it on behalf of a disallowed human.
      if (!botPermits(card, args.kind === 'bot_dm' ? humans[0].uid : args.callerUid)) {
        throw new TeamConversationError(403, 'Bot use is not permitted');
      }
    }
    if (existing.exists) {
      if (existing.data().requestDigest !== requestDigest) throw new TeamConversationError(409, 'Conversation identity conflict');
      return existing.data();
    }
    const record = { kind: args.kind, title: args.title, participants, memberUids,
      ...(args.kind === 'bot_dm' ? { botKey: bots[0].key } : {}),
      createdBy: args.callerUid, requestDigest, lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null };
    transaction.create(conversation, record);
    return record;
  });
}
