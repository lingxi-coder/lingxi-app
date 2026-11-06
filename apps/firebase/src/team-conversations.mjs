import { createHash } from 'node:crypto';
import { z } from 'zod';
import { sharedBotPermits } from './team-execution-contract.mjs';

const segment = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const inputSchema = z.object({
  teamId: segment, conversationId: id, callerUid: segment, callerGeneration: z.number().int().positive(),
  kind: z.enum(['channel', 'group', 'dm']),
  title: z.string().max(120), participants: z.array(z.string().max(300)).min(1).max(500),
}).strict();

/** SharedBot conversations use the same authoritative conversations collection
 * and membership ACL as human Chats. The caller owns the transaction/receipt. */
export async function prepareSharedConversation(transaction, team, identity, input) {
  const { conversationId, kind, title, participantUids, sharedBotIds } = input;
  if (!participantUids.includes(identity.uid)
    || (kind === 'bot_dm' && (participantUids.length !== 1 || sharedBotIds.length !== 1))) {
    throw new TeamConversationError(400, 'Invalid conversation participants');
  }
  const ref = team.collection('conversations').doc(conversationId);
  const [existing, ...botDocs] = await transaction.getAll(ref, ...sharedBotIds.map(id => team.collection('sharedBots').doc(id)));
  if (botDocs.some(doc => !doc.exists)) throw new TeamConversationError(403, 'Registered Bot required');
  const bots = botDocs.map(doc => doc.data());
  const memberUids = [...new Set([...participantUids, ...bots.map(bot => bot.identity.maintainer.uid)])].sort();
  const members = await transaction.getAll(...memberUids.map(uid => team.collection('members').doc(uid)));
  if (members.some(member => !member.exists)) throw new TeamConversationError(403, 'Current participants required', 'PARTICIPANT_REQUIRED');
  const memberById = new Map(members.map(doc => [doc.id, doc.data()]));
  for (const bot of bots) {
    if (memberById.get(bot.identity.maintainer.uid).generation !== bot.identity.maintainer.generation
      || !sharedBotPermits(bot.identity, identity)) throw new TeamConversationError(403, 'Bot use is not permitted');
  }
  const participants = [...participantUids.map(uid => `user:${uid}`), ...sharedBotIds.map(id => `shared_bot:${id}`)].sort();
  const requestDigest = createHash('sha256').update(JSON.stringify({ kind, title, participants, memberUids, createdBy: identity.uid })).digest('hex');
  if (existing.exists) {
    if (existing.data().requestDigest !== requestDigest) throw new TeamConversationError(409, 'Conversation identity conflict');
    return { ref, record: existing.data(), write() {} };
  }
  const record = { kind, title, participants, memberUids,
    ...(kind === 'bot_dm' ? { sharedBotId: sharedBotIds[0] } : {}),
    createdBy: identity.uid, requestDigest, lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null };
  return { ref, record, write() { transaction.create(ref, record); } };
}

export class TeamConversationError extends Error {
  constructor(status, message, code = 'CONVERSATION_REJECTED') { super(message); this.status = status; this.code = code; }
}

function principals(values) {
  if (new Set(values).size !== values.length) throw new TeamConversationError(400, 'Duplicate participant');
  return values.map(value => {
    if (value.startsWith('user:') && segment.safeParse(value.slice(5)).success) return { kind: 'user', uid: value.slice(5) };
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
  if ((args.kind === 'dm' && humans.length !== 2) || humans.length < 2) throw new TeamConversationError(400, 'Invalid conversation participants');
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
      ...memberUids.map(uid => team.collection('members').doc(uid)));
    if (teamSnapshot.data()?.state !== 'active' || !snapshots[memberUids.indexOf(args.callerUid)].exists) {
      throw new TeamConversationError(403, 'Current team membership required', 'MEMBERSHIP_REQUIRED');
    }
    if (snapshots.slice(0, memberUids.length).some(member => !member.exists)) throw new TeamConversationError(403, 'Current participants required', 'PARTICIPANT_REQUIRED');
    if (snapshots[memberUids.indexOf(args.callerUid)].data().generation !== args.callerGeneration) {
      throw new TeamConversationError(409, 'Stale membership', 'STALE_MEMBERSHIP');
    }
    if (existing.exists) {
      if (existing.data().requestDigest !== requestDigest) throw new TeamConversationError(409, 'Conversation identity conflict');
      return existing.data();
    }
    const record = { kind: args.kind, title: args.title, participants, memberUids,
      createdBy: args.callerUid, requestDigest, lastSeq: 0, lastMessage: null, lastChangeSeq: 0, lastChange: null };
    transaction.create(conversation, record);
    return record;
  });
}
