import { createHash } from 'node:crypto';
import { z } from 'zod';
import { createTeamConversation } from './team-conversations.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const scope = { teamId: id, generation: cursor.refine(value => value > 0) };
const thread = { ...scope, conversationId: id };
const text = z.string().min(1).refine(value => Buffer.byteLength(value, 'utf8') <= 8192);
export const messageCommand = z.discriminatedUnion('action', [
  z.object({ action: z.literal('list'), ...scope, after: id.optional() }).strict(),
  z.object({ action: z.literal('create'), ...thread, kind: z.enum(['dm', 'group', 'channel']),
    title: z.string().trim().min(1).max(120), participantUids: z.array(uid).min(2).max(200) }).strict(),
  z.object({ action: z.literal('sync'), ...thread, after: cursor }).strict(),
  z.object({ action: z.literal('mutate'), ...thread, messageId: id, changeId: id,
    operation: z.enum(['create', 'edit', 'recall']), text: z.string(), replyTo: id.optional() }).strict()
    .refine(value => value.operation === 'recall' ? value.text === '' : text.safeParse(value.text).success)
    .refine(value => value.operation === 'create' || value.replyTo === undefined),
  z.object({ action: z.literal('read'), ...thread, lastReadSeq: cursor }).strict(),
]);
export class TeamMessageError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}
const fail = (status, code) => { throw new TeamMessageError(status, code); };
const digest = text => createHash('sha256').update(text, 'utf8').digest('hex');
const time = value => value?.toDate?.().toISOString() ?? null;
const message = snapshot => {
  const value = snapshot.data();
  return { id: snapshot.id, ...value, createdAt: time(value.createdAt), editedAt: time(value.editedAt) };
};
function conversation(snapshot, read, generation) {
  const value = snapshot.data();
  return { id: snapshot.id, kind: value.kind, title: value.title, participants: value.participants,
    lastSeq: value.lastSeq, lastChangeSeq: value.lastChangeSeq,
    lastReadSeq: read?.generation === generation ? read.lastReadSeq : 0 };
}

/** One canonical transaction protocol for all four clients. SDK writes remain
 * rules-checked; HTTP also binds queued mutations to the membership generation. */
export class TeamMessagesStore {
  constructor(db, clock = Date.now) { this.db = db; this.clock = clock; }
  async execute(user, raw) {
    const input = messageCommand.parse(raw);
    if (input.action === 'create') {
      if (!input.participantUids.includes(user.uid)) fail(403, 'CONVERSATION_REQUIRED');
      const value = await createTeamConversation(this.db, { teamId: input.teamId, conversationId: input.conversationId,
        callerUid: user.uid, callerGeneration: input.generation, kind: input.kind, title: input.title,
        participants: input.participantUids.map(uid => `user:${uid}`) });
      return { conversationId: input.conversationId, lastSeq: value.lastSeq };
    }
    return this.db.runTransaction(async tx => {
      const teamRef = this.db.doc(`teams/${input.teamId}`);
      const [team, self] = await tx.getAll(teamRef, teamRef.collection('members').doc(user.uid));
      if (team.data()?.state !== 'active' || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (self.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      const threads = teamRef.collection('conversations');
      if (input.action === 'list') {
        let query = threads.where('memberUids', 'array-contains', user.uid).orderBy('__name__').limit(100);
        if (input.after) query = query.startAfter(input.after);
        const page = await tx.get(query);
        const reads = page.empty ? [] : await tx.getAll(...page.docs.map(doc => doc.ref.collection('reads').doc(user.uid)));
        return { conversations: page.docs.map((doc, index) => conversation(doc, reads[index].data(), input.generation)),
          next: page.size === 100 ? page.docs.at(-1).id : null };
      }
      const ref = threads.doc(input.conversationId), current = await tx.get(ref), c = current.data();
      if (!c?.memberUids.includes(user.uid)) fail(403, 'CONVERSATION_REQUIRED');
      const readRef = ref.collection('reads').doc(user.uid);
      if (input.action === 'read') {
        const read = (await tx.get(readRef)).data();
        if (input.lastReadSeq > c.lastSeq) fail(409, 'INVALID_READ_CURSOR');
        const lastReadSeq = Math.max(read?.generation === input.generation ? read.lastReadSeq : 0, input.lastReadSeq);
        tx.set(readRef, { lastReadSeq, generation: input.generation });
        return { lastReadSeq };
      }
      if (input.action === 'sync') {
        if (input.after > c.lastChangeSeq) fail(409, 'INVALID_CHANGE_CURSOR');
        const page = await tx.get(ref.collection('changes').where('seq', '>', input.after).orderBy('seq').limit(100));
        const ids = [...new Set(page.docs.map(doc => doc.data().messageId))];
        const snapshots = ids.length ? await tx.getAll(...ids.map(id => ref.collection('messages').doc(id))) : [];
        if (page.docs.some((doc, index) => doc.data().seq !== input.after + index + 1) || snapshots.some(doc => !doc.exists)
          || (page.empty && input.after !== c.lastChangeSeq)) fail(503, 'CHANGE_GAP');
        const read = (await tx.get(readRef)).data();
        return { conversation: conversation(current, read, input.generation),
          changes: page.docs.map(doc => ({ id: doc.id, ...doc.data(), at: time(doc.data().at) })),
          messages: snapshots.map(message), hasMore: (page.docs.at(-1)?.data().seq ?? input.after) < c.lastChangeSeq };
      }
      // T2 is human messaging. Bot execution/authenticated triggers are added in T3.
      if (!c.participants.includes(`user:${user.uid}`) || c.kind === 'bot_dm') fail(403, 'CONVERSATION_REQUIRED');
      const msgRef = ref.collection('messages').doc(input.messageId), changeRef = ref.collection('changes').doc(input.changeId);
      const [existing, receipt] = await tx.getAll(msgRef, changeRef);
      const m = existing.data(), old = receipt.data(), textDigest = digest(input.text);
      if (old) {
        if (old.messageId !== input.messageId || old.operation !== input.operation || old.actorUid !== user.uid
          || old.generation !== input.generation || old.textDigest !== textDigest
          || (input.operation === 'create' && (m?.author.kind !== 'user' || m.author.uid !== user.uid || m.kind !== 'text'
            || (m.replyTo ?? null) !== (input.replyTo ?? null) || (m.mentions?.length ?? 0) !== 0))) fail(409, 'MESSAGE_ID_CONFLICT');
        return { messageId: input.messageId, changeId: input.changeId, changeSeq: old.seq };
      }
      if (input.operation === 'create' && existing.exists) fail(409, 'MESSAGE_ID_CONFLICT');
      if (input.operation !== 'create' && (!m || m.author.kind !== 'user' || m.author.uid !== user.uid)) fail(403, 'MESSAGE_AUTHOR_REQUIRED');
      if (m?.recalled) fail(409, 'MESSAGE_RECALLED');
      if (input.replyTo && !(await tx.get(ref.collection('messages').doc(input.replyTo))).exists) fail(400, 'REPLY_REQUIRED');
      const changeSeq = c.lastChangeSeq + 1, seq = input.operation === 'create' ? c.lastSeq + 1 : m.seq;
      if (!Number.isSafeInteger(changeSeq) || !Number.isSafeInteger(seq)) fail(409, 'CURSOR_LIMIT');
      const now = new Date(this.clock());
      const change = { seq: changeSeq, messageId: input.messageId, operation: input.operation,
        actorUid: user.uid, generation: input.generation, textDigest, at: now };
      const update = { lastChangeSeq: changeSeq, lastChange: { id: input.changeId, seq: changeSeq } };
      if (input.operation === 'create') {
        Object.assign(update, { lastSeq: seq, lastMessage: { id: input.messageId, seq, at: now } });
        tx.create(msgRef, { seq, author: { kind: 'user', uid: user.uid }, kind: 'text', text: input.text,
          mentions: [], ...(input.replyTo ? { replyTo: input.replyTo } : {}), createdAt: now, changeSeq, changeId: input.changeId });
      } else {
        tx.update(msgRef, { text: input.text, editedAt: now, changeSeq, changeId: input.changeId,
          ...(input.operation === 'recall' ? { recalled: true } : {}) });
      }
      tx.update(ref, update); tx.create(changeRef, change);
      return { messageId: input.messageId, changeId: input.changeId, changeSeq };
    });
  }
}
