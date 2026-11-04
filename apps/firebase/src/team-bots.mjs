import { createHash } from 'node:crypto';
import { z } from 'zod';
import { FieldValue } from 'firebase-admin/firestore';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().regex(/^[A-Za-z0-9_-]{1,128}$/u);
const localId = z.string().regex(/^[A-Za-z0-9_-]{22}$/u);
const key = z.string().regex(/^bot:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{22}:[A-Za-z0-9_-]{22}$/u);
const positive = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
const cursor = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const text = z.string().min(1).refine(value => Buffer.byteLength(value, 'utf8') <= 8192);
const scope = { teamId: id, generation: positive };
const target = { ...scope, botKey: key };
const thread = { ...target, conversationId: id };
const run = { ...thread, triggerId: id, attemptId: id };
const terminal = new Set(['completed', 'failed', 'missed', 'cancelled', 'recovered', 'revoked']);
export const botActions = new Set(['bots', 'bindBot', 'unbindBot', 'heartbeatBot', 'botConversations', 'botStates', 'botQueue', 'claimBot', 'startBot', 'finishBot', 'botQuestion', 'botAnswer', 'botPoll', 'recoverBot', 'botState', 'botWait']);
export const botCommand = z.discriminatedUnion('action', [
  z.object({ action: z.literal('bots'), ...scope }).strict(),
  z.object({ action: z.literal('botConversations'), ...target, after: id.optional() }).strict(),
  z.object({ action: z.literal('botStates'), ...scope, conversationId: id, triggerIds: z.array(id).min(1).max(30).refine(values => new Set(values).size === values.length) }).strict(),
  z.object({ action: z.literal('bindBot'), ...scope, hostId: localId, botId: localId, expectedRevision: cursor,
    name: z.string().trim().min(1).max(80), description: z.string().max(500), use: z.enum(['all', 'listed']), uids: z.array(uid).max(200) }).strict(),
  z.object({ action: z.literal('unbindBot'), ...target, expectedRevision: positive }).strict(),
  z.object({ action: z.literal('heartbeatBot'), ...target, revision: positive }).strict(),
  z.object({ action: z.literal('botQueue'), ...thread, after: cursor }).strict(),
  z.object({ action: z.literal('claimBot'), ...run }).strict(),
  z.object({ action: z.literal('startBot'), ...run }).strict(),
  z.object({ action: z.literal('finishBot'), ...run, text, outcome: z.enum(['completed', 'failed', 'missed', 'cancelled']) }).strict(),
  z.object({ action: z.literal('botQuestion'), ...run, questionId: id, text,
    questions: z.array(z.string().min(1).max(2000)).min(1).max(4).refine(values => new Set(values).size === values.length) }).strict(),
  z.object({ action: z.literal('botAnswer'), ...scope, conversationId: id, triggerId: id, questionId: id, answerId: id,
    answers: z.record(z.string().min(1).max(2000), z.string().min(1).max(2000)) }).strict(),
  z.object({ action: z.literal('botPoll'), ...run }).strict(),
  z.object({ action: z.literal('botState'), ...scope, conversationId: id, triggerId: id, questionId: id.optional() }).strict(),
  z.object({ action: z.literal('botWait'), ...run, waitId: id, waiting: z.boolean() }).strict(),
  // Recovery settles an ambiguous invocation. It NEVER executes its tools again.
  z.object({ action: z.literal('recoverBot'), ...thread, triggerId: id, text }).strict(),
]);
export class TeamBotError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const fail = (status, code) => { throw new TeamBotError(status, code); };
export const botDigest = value => createHash('sha256').update(value, 'utf8').digest('hex');
export const triggerIdentity = (messageId, botKey) => botDigest(JSON.stringify([messageId, botKey]));
// Reserved for trusted Bot outputs. HTTP human writes and all SDK writes reject
// these IDs, so a participant cannot preoccupy a deterministic reply/change ID.
export const botOutputIdentity = (triggerId, suffix) => `bot_${botDigest(`${triggerId}:${suffix}`).slice(0, 60)}`;
export function botPermits(card, humanUid) {
  return card?.enabled !== false && (card?.ownerUid === humanUid || card?.use === 'all' || (card?.use === 'listed' && card.uids?.includes(humanUid)));
}
export function validateBot(card, botKey, owner, humanUid) {
  if (!card || !owner || card.ownerGeneration !== owner.generation || card.enabled === false
    || botKey !== `bot:${card.ownerUid}:${card.hostId}:${card.botId}` || !botPermits(card, humanUid)) fail(403, 'BOT_USE_REQUIRED');
}

/** All reads precede any writes in the caller's transaction. Only the trusted
 * human create path calls this; SDK mentions and Bot replies cannot enqueue. */
export async function prepareBotTriggers(tx, teamRef, ref, conversation, user, generation, input) {
  const targets = conversation.kind === 'bot_dm' ? [conversation.botKey] : (input.targets ?? []);
  if (new Set(targets).size !== targets.length || targets.length > 8 || (conversation.kind === 'bot_dm' && input.targets?.some(value => value !== conversation.botKey))) fail(400, 'INVALID_BOT_TARGET');
  const cards = targets.length ? await tx.getAll(...targets.map(value => teamRef.collection('bots').doc(value))) : [];
  const owners = cards.length ? await tx.getAll(...cards.map(doc => teamRef.collection('members').doc(doc.data()?.ownerUid ?? '_missing'))) : [];
  for (const [index, botKey] of targets.entries()) {
    if (!key.safeParse(botKey).success || !conversation.participants.includes(botKey)) fail(403, 'BOT_PARTICIPANT_REQUIRED');
    validateBot(cards[index].data(), botKey, owners[index].data(), user.uid);
  }
  const evidence = targets.length ? { targets: [...targets].sort(), triggerIds: targets.map(value => triggerIdentity(input.messageId, value)).sort() } : null;
  return { evidence, write(seq, now) {
    for (const [index, botKey] of targets.entries()) {
      const card = cards[index].data(), triggerId = triggerIdentity(input.messageId, botKey);
      tx.create(ref.collection('triggers').doc(triggerId), { messageId: input.messageId, changeId: input.changeId, textDigest: botDigest(input.text),
        actorUid: user.uid, generation, botKey, botRevision: card.revision, ownerUid: card.ownerUid,
        ownerGeneration: card.ownerGeneration, hostId: card.hostId, seq, at: now });
    }
  } };
}

function appendBotMessage(tx, ref, conversation, trigger, triggerId, suffix, body, now, extra = {}) {
  const messageId = botOutputIdentity(triggerId, suffix), changeId = botOutputIdentity(triggerId, `${suffix}:change`);
  const seq = conversation.lastSeq + 1, changeSeq = conversation.lastChangeSeq + 1;
  if (!Number.isSafeInteger(seq) || !Number.isSafeInteger(changeSeq)) fail(409, 'CURSOR_LIMIT');
  tx.create(ref.collection('messages').doc(messageId), { seq, author: { kind: 'bot', uid: trigger.ownerUid, botKey: trigger.botKey },
    kind: extra.question ? 'question' : suffix === 'reply' ? 'text' : 'status', text: body, replyTo: trigger.messageId,
    triggerId, origin: { uid: trigger.actorUid, generation: trigger.generation }, ...extra,
    createdAt: now, changeSeq, changeId });
  tx.create(ref.collection('changes').doc(changeId), { seq: changeSeq, messageId, operation: 'create', actorUid: trigger.ownerUid,
    generation: trigger.ownerGeneration, at: now, textDigest: botDigest(body) });
  tx.update(ref, { lastSeq: seq, lastMessage: { id: messageId, seq, at: now }, lastChangeSeq: changeSeq, lastChange: { id: changeId, seq: changeSeq } });
  return messageId;
}

/** Server receipts, not a cursor, arbitrate execution. Claim is safely retryable;
 * once start commits, an expired/interrupted invocation needs owner recovery. */
export class TeamBotsStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  execute(user, raw) {
    const input = botCommand.parse(raw);
    return this.db.runTransaction(async tx => {
      const team = this.db.doc(`teams/${input.teamId}`), now = new Date(this.clock());
      const [teamDoc, member] = await tx.getAll(team, team.collection('members').doc(user.uid));
      if (teamDoc.data()?.state !== 'active' || !member.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (member.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      if (input.action === 'bots') {
        const cards = await tx.get(team.collection('bots').limit(200));
        const owners = cards.empty ? [] : await tx.getAll(...cards.docs.map(doc => team.collection('members').doc(doc.data().ownerUid)));
        return { bots: cards.docs.flatMap((doc, index) => {
          const b = doc.data();
          const enabled = b.enabled !== false && owners[index].data()?.generation === b.ownerGeneration;
          // The owner needs tombstone revisions to rebind after disable/rejoin.
          // Other members never receive inactive cards or their old allowlists.
          if (!enabled && b.ownerUid !== user.uid) return [];
          return [{ key: doc.id, ownerUid: b.ownerUid, hostId: b.hostId, botId: b.botId, name: b.name, description: b.description,
            revision: b.revision, enabled, use: b.use, uids: b.uids, permitted: enabled && botPermits(b, user.uid),
            onlineUntil: (b.lastSeen?.toMillis?.() ?? 0) + 60000, online: enabled && now.getTime() - (b.lastSeen?.toMillis?.() ?? 0) < 60000 }];
        }) };
      }
      if (input.action === 'bindBot') {
        if (!uid.safeParse(user.uid).success) fail(400, 'BOT_ACCOUNT_UNSUPPORTED');
        const botKey = `bot:${user.uid}:${input.hostId}:${input.botId}`, ref = team.collection('bots').doc(botKey), old = (await tx.get(ref)).data();
        if ((old?.revision ?? 0) !== input.expectedRevision) fail(409, 'BOT_REVISION_CONFLICT');
        if (new Set(input.uids).size !== input.uids.length) fail(400, 'INVALID_INPUT');
        const allowed = input.uids.length ? await tx.getAll(...input.uids.map(value => team.collection('members').doc(value))) : [];
        if (allowed.some(value => !value.exists)) fail(403, 'PARTICIPANT_REQUIRED');
        if (!old && (await tx.get(team.collection('bots').limit(200))).size >= 200) fail(409, 'BOT_LIMIT');
        const revision = (old?.revision ?? 0) + 1;
        tx.set(ref, { ownerUid: user.uid, ownerGeneration: input.generation, hostId: input.hostId, botId: input.botId,
          name: input.name, description: input.description, use: input.use, uids: input.uids.sort(), revision, enabled: true, lastSeen: now });
        tx.set(team.collection('signals').doc(botDigest(botKey)), { updatedAt: FieldValue.serverTimestamp() });
        return { botKey, revision };
      }
      if (input.action === 'botAnswer') return this.answer(tx, team, user, input, now);
      if (input.action === 'botState') return this.status(tx, team, user, input, now);
      if (input.action === 'botStates') {
        // Cache only reads in this authenticated transaction, never authorization
        // across requests. Every trigger still traverses the full status checks.
        const reads = new Map([[team.path, Promise.resolve(teamDoc)], [team.collection('members').doc(user.uid).path, Promise.resolve(member)]]);
        const reader = { getAll(...refs) {
          const missing = [...new Map(refs.filter(ref => !reads.has(ref.path)).map(ref => [ref.path, ref])).values()];
          if (missing.length) {
            const values = tx.getAll(...missing);
            missing.forEach((ref, index) => reads.set(ref.path, values.then(docs => docs[index])));
          }
          return Promise.all(refs.map(ref => reads.get(ref.path)));
        }, get(ref) { return this.getAll(ref).then(values => values[0]); } };
        const states = await Promise.all(input.triggerIds.map(async triggerId => {
          try { return [triggerId, await this.status(reader, team, user, { ...input, triggerId }, now)]; }
          catch (error) { if (error.code === 'TRIGGER_REQUIRED') return [triggerId, null]; throw error; }
        }));
        return { states: Object.fromEntries(states) };
      }
      const cardRef = team.collection('bots').doc(input.botKey), card = (await tx.get(cardRef)).data();
      if (!card || card.ownerUid !== user.uid || card.ownerGeneration !== input.generation) fail(403, 'BOT_OWNER_REQUIRED');
      if (input.action === 'botConversations') {
        let query = team.collection('conversations').where('participants', 'array-contains', input.botKey).orderBy('__name__').limit(100);
        if (input.after) query = query.startAfter(input.after);
        const page = await tx.get(query);
        if (page.docs.some(doc => !doc.data().memberUids.includes(user.uid))) fail(403, 'CONVERSATION_REQUIRED');
        return { conversations: page.docs.map(doc => ({ id: doc.id, participants: doc.data().participants, lastChangeSeq: doc.data().lastChangeSeq })),
          next: page.size === 100 ? page.docs.at(-1).id : null };
      }
      if (input.action === 'unbindBot') {
        if (input.expectedRevision !== card.revision) fail(409, 'BOT_REVISION_CONFLICT');
        tx.update(cardRef, { enabled: false, revision: card.revision + 1 });
        tx.set(team.collection('signals').doc(botDigest(input.botKey)), { updatedAt: FieldValue.serverTimestamp() }); return { ok: true };
      }
      if (input.action === 'heartbeatBot') {
        if (!card.enabled || input.revision !== card.revision) fail(409, 'BOT_REVISION_CONFLICT');
        tx.update(cardRef, { lastSeen: now });
        tx.set(team.collection('signals').doc(botDigest(input.botKey)), { updatedAt: FieldValue.serverTimestamp() }); return { ok: true };
      }
      const ref = team.collection('conversations').doc(input.conversationId), c = (await tx.get(ref)).data();
      if (!c?.memberUids.includes(user.uid) || !c.participants.includes(input.botKey)) fail(403, 'CONVERSATION_REQUIRED');
      const cursorRef = ref.collection('botCursors').doc(botDigest(input.botKey));
      const checkpoint = (await tx.get(cursorRef)).data(), currentCursor = checkpoint?.seq ?? 0;
      if (input.action === 'botQueue') {
        if (input.after > currentCursor) fail(409, 'INVALID_BOT_CURSOR');
        const page = await tx.get(ref.collection('triggers').where('botKey', '==', input.botKey).where('seq', '>', currentCursor).orderBy('seq').limit(30));
        return { cursor: currentCursor, triggers: page.docs.map(doc => ({ id: doc.id, seq: doc.data().seq })), hasMore: page.size === 30 };
      }
      const triggerRef = ref.collection('triggers').doc(input.triggerId), runRef = ref.collection('botRuns').doc(input.triggerId);
      const [triggerDoc, runDoc] = await tx.getAll(triggerRef, runRef), trigger = triggerDoc.data(), state = runDoc.data();
      if (!trigger || trigger.botKey !== input.botKey || trigger.ownerUid !== user.uid || trigger.hostId !== card.hostId) fail(403, 'TRIGGER_REQUIRED');
      if (state && terminal.has(state.state)) {
        if (input.action === 'finishBot' && state.resultDigest && state.resultDigest !== botDigest(JSON.stringify([input.outcome, input.text]))) fail(409, 'BOT_RESULT_CONFLICT');
        if (input.action === 'recoverBot' && state.recoveryDigest && state.recoveryDigest !== botDigest(input.text)) fail(409, 'BOT_RESULT_CONFLICT');
        return { state: state.state, replyId: state.replyId ?? null };
      }
      const first = await tx.get(ref.collection('triggers').where('botKey', '==', input.botKey).where('seq', '>', currentCursor).orderBy('seq').limit(1));
      if (first.docs[0]?.id !== input.triggerId || (checkpoint?.activeTriggerId && checkpoint.activeTriggerId !== input.triggerId)) fail(409, 'BOT_RUN_BLOCKED');
      const settleCursor = () => tx.set(cursorRef, { seq: trigger.seq, activeTriggerId: null });
      const original = (await tx.get(ref.collection('messages').doc(trigger.messageId))).data();
      const [human, originalChange] = await tx.getAll(team.collection('members').doc(trigger.actorUid), ref.collection('changes').doc(trigger.changeId));
      const valid = card.enabled && card.revision === trigger.botRevision && human.data()?.generation === trigger.generation
        && c.memberUids.includes(trigger.actorUid) && c.participants.includes(`user:${trigger.actorUid}`)
        && botPermits(card, trigger.actorUid) && !original?.recalled && original?.author.kind === 'user'
        && original.author.uid === trigger.actorUid && originalChange.data()?.generation === trigger.generation
        && originalChange.data()?.textDigest === trigger.textDigest && original.changeId === trigger.changeId
        && originalChange.data()?.executionIntent?.targets.includes(trigger.botKey)
        && originalChange.data().executionIntent.triggerIds.includes(input.triggerId);
      if (!valid) {
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.set(runRef, { ...state, state: 'revoked', finishedAt: now });
        settleCursor(); return { state: 'revoked' };
      }
      if (input.action === 'recoverBot') {
        // A host can persist its conservative start intent before an account /
        // transport failure prevents dispatch. That expired claim must also be
        // manually settleable, without replaying it. This transaction fences a
        // racing reclaim/start: a renewed live lease cannot be recovered.
        if (!state || !['claimed', 'running', 'recovery'].includes(state.state)
          || (['claimed', 'running'].includes(state.state) && state.leaseUntil > now.getTime())) fail(409, 'RECOVERY_NOT_REQUIRED');
        const replyId = appendBotMessage(tx, ref, c, trigger, input.triggerId, 'reply', input.text, now);
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.set(runRef, { ...state, state: 'recovered', replyId, recoveryDigest: botDigest(input.text), finishedAt: now });
        settleCursor(); return { state: 'recovered', replyId };
      }
      if (state?.state === 'running' && state.leaseUntil <= now.getTime()) {
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.update(runRef, { state: 'recovery' }); return { state: 'recovery' };
      }
      if (state?.state === 'recovery') return { state: 'recovery' };
      if (input.action === 'claimBot') {
        if (state?.state === 'running' || (state?.state === 'claimed' && state.leaseUntil > now.getTime() && state.attemptId !== input.attemptId)) return { state: 'busy' };
        if (now.getTime() - trigger.at.toMillis() > 86400000) {
          const replyId = appendBotMessage(tx, ref, c, trigger, input.triggerId, 'reply', 'This request expired while the Bot was offline. Please send a new request.', now);
          tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
          tx.set(runRef, { state: 'missed', replyId, finishedAt: now }); settleCursor(); return { state: 'missed', replyId };
        }
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.set(runRef, { state: 'claimed', attemptId: input.attemptId, leaseUntil: now.getTime() + 30000 });
        tx.set(cursorRef, { seq: currentCursor, activeTriggerId: input.triggerId });
        return { state: 'claimed', origin: { uid: trigger.actorUid, generation: trigger.generation }, text: original.text, messageId: trigger.messageId, seq: trigger.seq };
      }
      if (!state || state.attemptId !== input.attemptId || state.leaseUntil <= now.getTime()) fail(409, 'BOT_LEASE_REQUIRED');
      if (input.action === 'startBot') {
        // Only the transaction that changes claimed→running may hand out an
        // execution permit. A lost start response needs recovery, not replay.
        if (state.state === 'running') return { state: 'running', execute: false, leaseUntil: state.leaseUntil };
        if (state.state !== 'claimed') fail(409, 'BOT_LEASE_REQUIRED');
        const leaseUntil = now.getTime() + 300000;
        const statusId = appendBotMessage(tx, ref, c, trigger, input.triggerId, 'started', 'Processing on the Bot owner’s computer.', now);
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.update(runRef, { state: 'running', leaseUntil, statusId, startedAt: now });
        // The host consumes this authenticated snapshot, never an earlier claim
        // body or renderer-supplied principal. Only the one-shot transition
        // includes it; retries cannot reconstruct an execution capability.
        return { state: 'running', execute: true, leaseUntil, execution: {
          version: 1, teamId: input.teamId, conversationId: input.conversationId, botKey: input.botKey,
          triggerId: input.triggerId, attemptId: input.attemptId, ownerUid: user.uid, ownerGeneration: input.generation,
          hostId: card.hostId, botId: card.botId, botRevision: card.revision,
          origin: { uid: trigger.actorUid, generation: trigger.generation },
          messageId: trigger.messageId, seq: trigger.seq, text: original.text,
        } };
      }
      if (state.state !== 'running') fail(409, 'BOT_LEASE_REQUIRED');
      if (input.action === 'botWait') {
        const waitRef = ref.collection('botWaits').doc(input.waitId), wait = (await tx.get(waitRef)).data();
        if (wait && (wait.triggerId !== input.triggerId || wait.attemptId !== input.attemptId)) fail(409, 'QUESTION_CONFLICT');
        if (wait?.[input.waiting ? 'requested' : 'resolved']) return { ok: true };
        if (!wait && (state.approvalCount ?? 0) >= 32) fail(409, 'BOT_APPROVAL_LIMIT');
        if (!input.waiting && !wait?.requested) fail(409, 'QUESTION_CONFLICT');
        appendBotMessage(tx, ref, c, trigger, input.triggerId, `approval:${input.waitId}:${input.waiting ? 'requested' : 'resolved'}`,
          input.waiting ? 'Waiting for the Bot owner’s approval.' : 'The Bot owner’s approval request has been resolved.', now);
        tx.set(waitRef, { ...wait, triggerId: input.triggerId, attemptId: input.attemptId, [input.waiting ? 'requested' : 'resolved']: true });
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        if (!wait) tx.update(runRef, { approvalCount: (state.approvalCount ?? 0) + 1 });
        return { ok: true };
      }
      if (input.action === 'finishBot') {
        const replyId = appendBotMessage(tx, ref, c, trigger, input.triggerId, 'reply', input.text, now);
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.update(runRef, { state: input.outcome, replyId, resultDigest: botDigest(JSON.stringify([input.outcome, input.text])), finishedAt: now });
        settleCursor(); return { state: input.outcome, replyId };
      }
      if (input.action === 'botQuestion') {
        const questionRef = ref.collection('botQuestions').doc(input.questionId), existing = (await tx.get(questionRef)).data();
        const value = { triggerId: input.triggerId, attemptId: input.attemptId, actorUid: trigger.actorUid, generation: trigger.generation, questions: input.questions, text: input.text };
        const requestDigest = botDigest(JSON.stringify(value));
        if (existing) { if (existing.requestDigest !== requestDigest) fail(409, 'QUESTION_CONFLICT'); return { ok: true }; }
        const question = { id: input.questionId, questions: input.questions };
        appendBotMessage(tx, ref, c, trigger, input.triggerId, `question:${input.questionId}`, input.text, now, { question });
        tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
        tx.create(questionRef, { ...value, requestDigest }); tx.update(runRef, { questionId: input.questionId }); return { ok: true };
      }
      if (input.action === 'botPoll') {
        const question = state.questionId ? (await tx.get(ref.collection('botQuestions').doc(state.questionId))).data() : undefined;
        return { state: 'running', leaseUntil: state.leaseUntil, questionId: state.questionId ?? null, answers: question?.answers ?? null };
      }
      fail(400, 'INVALID_INPUT');
    });
  }
  async status(tx, team, user, input, now) {
    const ref = team.collection('conversations').doc(input.conversationId);
    const [conversation, triggerDoc, runDoc] = await tx.getAll(ref, ref.collection('triggers').doc(input.triggerId), ref.collection('botRuns').doc(input.triggerId));
    const c = conversation.data(), trigger = triggerDoc.data(), run = runDoc.data();
    if (!c?.memberUids.includes(user.uid)) fail(403, 'CONVERSATION_REQUIRED');
    if (!trigger || !c.participants.includes(trigger.botKey)) fail(404, 'TRIGGER_REQUIRED');
    const [cardDoc, owner, human, message, receipt] = await tx.getAll(team.collection('bots').doc(trigger.botKey),
      team.collection('members').doc(trigger.ownerUid), team.collection('members').doc(trigger.actorUid),
      ref.collection('messages').doc(trigger.messageId), ref.collection('changes').doc(trigger.changeId));
    const card = cardDoc.data(), original = message.data(), change = receipt.data();
    const valid = card?.enabled && card.ownerUid === trigger.ownerUid && card.ownerGeneration === owner.data()?.generation
      && card.ownerGeneration === trigger.ownerGeneration && card.revision === trigger.botRevision
      && trigger.botKey === `bot:${card.ownerUid}:${card.hostId}:${card.botId}` && card.hostId === trigger.hostId
      && human.data()?.generation === trigger.generation && c.memberUids.includes(trigger.actorUid)
      && c.participants.includes(`user:${trigger.actorUid}`) && botPermits(card, trigger.actorUid)
      && !original?.recalled && original?.author?.kind === 'user' && original.author.uid === trigger.actorUid
      && original.changeId === trigger.changeId && change?.generation === trigger.generation
      && change.textDigest === trigger.textDigest && change.executionIntent?.targets.includes(trigger.botKey)
      && change.executionIntent.triggerIds.includes(input.triggerId);
    const state = !valid ? 'revoked' : run?.state === 'running' && run.leaseUntil <= now.getTime() ? 'recovery' : run?.state ?? 'queued';
    const questionId = input.questionId ?? run?.questionId;
    const question = valid && questionId ? (await tx.get(ref.collection('botQuestions').doc(questionId))).data() : null;
    if (valid && input.questionId && (!question || question.triggerId !== input.triggerId || question.attemptId !== run?.attemptId)) fail(404, 'QUESTION_REQUIRED');
    // Public observation contains no tool names, arguments, traces or answers.
    return { state, ownerUid: trigger.ownerUid, botKey: trigger.botKey,
      origin: { uid: trigger.actorUid, generation: trigger.generation },
      ...(run?.leaseUntil ? { leaseUntil: run.leaseUntil } : {}),
      ...(valid && questionId ? { questionId } : {}), answered: Boolean(question?.answerId) };
  }
  async answer(tx, team, user, input, now) {
    const ref = team.collection('conversations').doc(input.conversationId);
    const [conversation, triggerDoc, questionDoc, runDoc] = await tx.getAll(ref, ref.collection('triggers').doc(input.triggerId), ref.collection('botQuestions').doc(input.questionId), ref.collection('botRuns').doc(input.triggerId));
    const trigger = triggerDoc.data(), question = questionDoc.data(), run = runDoc.data();
    if (!conversation.data()?.memberUids.includes(user.uid) || !trigger || trigger.actorUid !== user.uid || trigger.generation !== input.generation
      || question?.triggerId !== input.triggerId || question.attemptId !== run?.attemptId) fail(403, 'QUESTION_RECIPIENT_REQUIRED');
    const digest = botDigest(JSON.stringify(Object.entries(input.answers).sort()));
    if (question.answerId) { if (question.answerId !== input.answerId || question.answerDigest !== digest) fail(409, 'ANSWER_CONFLICT'); return { ok: true }; }
    if (run.state !== 'running' || run.questionId !== input.questionId || run.leaseUntil <= now.getTime()) fail(403, 'QUESTION_RECIPIENT_REQUIRED');
    const [card, owner, original] = await tx.getAll(team.collection('bots').doc(trigger.botKey), team.collection('members').doc(trigger.ownerUid), ref.collection('messages').doc(trigger.messageId));
    validateBot(card.data(), trigger.botKey, owner.data(), user.uid);
    if (card.data().revision !== trigger.botRevision || original.data()?.recalled || original.data()?.changeId !== trigger.changeId) fail(403, 'QUESTION_RECIPIENT_REQUIRED');
    if (Object.keys(input.answers).length !== question.questions.length || question.questions.some(value => !(value in input.answers))) fail(400, 'INVALID_ANSWER');
    tx.update(ref, { botUpdatedAt: FieldValue.serverTimestamp() });
    tx.update(questionDoc.ref, { answers: input.answers, answerId: input.answerId, answerDigest: digest, answeredAt: now }); return { ok: true };
  }
}
