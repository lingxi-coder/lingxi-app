import { confirmShareDelivery } from './dataset-message-grant.mjs';
import { createHash } from 'node:crypto';
import { z } from 'zod';
import { mirrorHttpRead } from './inbox-store.mjs';
import { createTeamConversation } from './team-conversations.mjs';
import { SharedBotsStore } from './shared-bots.mjs';
import { authorizeDataBinding, dataReference } from './lingxi-data.mjs';
import { messagePreview } from './team-message-preview.mjs';
import { recallAttachments } from './team-attachments.mjs';
import { mentionEntitiesSchema, mentionFields, mentionMemberRefs, preserveMentionEntities, validMentionEntities } from './team-mentions.mjs';
import { serviceMessageActions, serviceScope } from './hosted-bot-service.mjs';

const id = z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u);
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);
const cursor = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const scope = { teamId: id, generation: cursor.refine(value => value > 0) };
const thread = { ...scope, conversationId: id };
const text = z.string().min(1).refine(value => Buffer.byteLength(value, 'utf8') <= 8192);
export const messageCommand = z.discriminatedUnion('action', [
  z.object({ action: z.literal('list'), ...scope, after: id.optional() }).strict(),
  z.object({ action: z.literal('create'), ...thread, kind: z.enum(['dm', 'group', 'channel']),
    title: z.string().trim().min(1).max(120), participantUids: z.array(uid).min(1).max(200) }).strict(),
  z.object({ action: z.literal('sync'), ...thread, after: cursor, limit: z.number().int().min(1).max(100).optional() }).strict(),
  z.object({ action: z.literal('history'), ...thread, before: cursor.optional() }).strict(),
  z.object({ action: z.literal('mutate'), ...thread, messageId: id, changeId: id,
    operation: z.enum(['create', 'edit', 'recall']), text: z.string(), replyTo: id.optional(), dataRef: dataReference.optional(), mentionEntities: mentionEntitiesSchema.optional() }).strict()
    .refine(value => value.operation === 'recall' ? value.text === '' : text.safeParse(value.text).success)
    .refine(value => validMentionEntities(value.text, value.mentionEntities))
    .refine(value => value.operation === 'create' || (value.replyTo === undefined && value.dataRef === undefined)),
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
    ...(value.autoReply ? { autoReply: { revision: value.autoReply.revision, targets: value.autoReply.targets } } : {}),
    lastReadSeq: read?.generation === generation ? read.lastReadSeq : 0 };
}

/** One canonical transaction protocol for all four clients. SDK writes remain
 * rules-checked; HTTP also binds queued mutations to the membership generation. */
export class TeamMessagesStore {
  constructor(db, clock = Date.now, readData) { this.db = db; this.clock = clock; this.readData = readData; }
  async execute(user, raw, share) {
    if (raw?.schemaVersion !== undefined) return new SharedBotsStore(this.db, this.clock).execute(user, raw);
    // A hosted Bot service's sessions are read with the same commands, addressed by `serviceId`. They are written only
    // through the execution ledger, so nothing but reading is taken here.
    const service = raw?.serviceId !== undefined;
    if (service && !serviceMessageActions.has(raw.action)) fail(403, 'SERVICE_SCOPE_UNSUPPORTED');
    const { serviceId, ...rest } = service ? raw : {};
    const input = service ? { ...messageCommand.parse({ ...rest, teamId: id.parse(serviceId) }), teamId: undefined } : messageCommand.parse(raw);
    if (input.action === 'mutate' && (input.messageId.startsWith('bot_') || input.changeId.startsWith('bot_'))) fail(400, 'INVALID_INPUT');
    const dataScope = { kind: 'team', teamId: input.teamId, conversationId: input.conversationId, generation: input.generation };
    if (input.dataRef) {
      if (!this.readData || service) fail(400, 'DATA_REFERENCE_UNAVAILABLE');
      await this.readData({ scope: dataScope, command: { action: 'get', reference: input.dataRef } });
    }
    if (input.action === 'create') {
      if (!input.participantUids.includes(user.uid)) fail(403, 'CONVERSATION_REQUIRED');
      const value = await createTeamConversation(this.db, { teamId: input.teamId, conversationId: input.conversationId,
        callerUid: user.uid, callerGeneration: input.generation, kind: input.kind, title: input.title,
        participants: input.participantUids.map(uid => `user:${uid}`) });
      return { conversationId: input.conversationId, lastSeq: value.lastSeq };
    }
    return this.db.runTransaction(async tx => {
      const teamRef = service ? serviceScope.root(this.db, serviceId) : this.db.doc(`teams/${input.teamId}`);
      const [team, self] = await tx.getAll(teamRef, teamRef.collection('members').doc(user.uid));
      if (service) serviceScope.check(team.data(), self.data(), user.uid);
      else if (team.data()?.state !== 'active' || !self.exists) fail(403, 'MEMBERSHIP_REQUIRED');
      if (self.data().generation !== input.generation) fail(409, 'STALE_MEMBERSHIP');
      const threads = teamRef.collection('conversations');
      if (input.action === 'list') {
        let query = threads.where('memberUids', 'array-contains', user.uid).orderBy('__name__').limit(101);
        if (input.after) query = query.startAfter(input.after);
        const page = await tx.get(query), docs = page.docs.slice(0, 100);
        const reads = docs.length ? await tx.getAll(...docs.map(doc => doc.ref.collection('reads').doc(user.uid))) : [];
        return { conversations: docs.map((doc, index) => conversation(doc, reads[index].data(), input.generation)), next: page.size > 100 ? docs.at(-1).id : null };
      }
      const ref = threads.doc(input.conversationId), current = await tx.get(ref), c = current.data();
      if (!c?.memberUids.includes(user.uid)) fail(403, 'CONVERSATION_REQUIRED');
      const readRef = ref.collection('reads').doc(user.uid);
      if (input.action === 'read') {
        const read = (await tx.get(readRef)).data();
        if (input.lastReadSeq > c.lastSeq) fail(409, 'INVALID_READ_CURSOR');
        const lastReadSeq = Math.max(read?.generation === input.generation ? read.lastReadSeq : 0, input.lastReadSeq);
        await mirrorHttpRead(tx,this.db,user.uid,{kind:service?'service':'team',sourceId:service?serviceId:input.teamId,
          conversationId:input.conversationId,generation:input.generation},lastReadSeq,this.clock(),self.data().joinedAt);
        if (read?.generation !== input.generation || read.lastReadSeq !== lastReadSeq) {
          tx.set(readRef, { lastReadSeq, generation: input.generation });
        }
        return { lastReadSeq };
      }
      if (input.action === 'history') {
        let q = ref.collection('messages').orderBy('seq', 'desc');
        if (input.before !== undefined) q = q.startAfter(input.before);
        const page = await tx.get(q.limit(51)), docs = page.docs.slice(0,50);
        return { messages: docs.map(message).reverse(), next: page.size > 50 ? docs.at(-1).data().seq : null,
          lastSeq: c.lastSeq, lastChangeSeq: c.lastChangeSeq };
      }
      if (input.action === 'sync') {
        if (input.after > c.lastChangeSeq) fail(409, 'INVALID_CHANGE_CURSOR');
        const page = await tx.get(ref.collection('changes').where('seq', '>', input.after).orderBy('seq').limit(input.limit ?? 100));
        const ids = [...new Set(page.docs.map(doc => doc.data().messageId))];
        const snapshots = ids.length ? await tx.getAll(...ids.map(id => ref.collection('messages').doc(id))) : [];
        if (page.docs.some((doc, index) => doc.data().seq !== input.after + index + 1) || snapshots.some(doc => !doc.exists)
          || (page.empty && input.after !== c.lastChangeSeq)) fail(503, 'CHANGE_GAP');
        const read = (await tx.get(readRef)).data();
        return { conversation: conversation(current, read, input.generation),
          changes: page.docs.map(doc => ({ id: doc.id, ...doc.data(), at: time(doc.data().at) })),
          messages: snapshots.map(message), hasMore: (page.docs.at(-1)?.data().seq ?? input.after) < c.lastChangeSeq };
      }
      if (input.dataRef) await authorizeDataBinding(tx, this.db, user.uid, dataScope, input.dataRef.dataset, false);
      if (!c.participants.includes(`user:${user.uid}`)) fail(403, 'CONVERSATION_REQUIRED');
      if (c.kind === 'bot_dm' && input.operation === 'create') fail(400, 'SUBMIT_REQUIRED');
      if (share) await confirmShareDelivery(tx, teamRef, c, input, user.uid, share);
      const msgRef = ref.collection('messages').doc(input.messageId), changeRef = ref.collection('changes').doc(input.changeId);
      const [existing, receipt] = await tx.getAll(msgRef, changeRef);
      const m = existing.data(), old = receipt.data();
      let content = input.text;
      if (share) content = JSON.stringify([input.text, share.reference, share.grant]);
      else if (input.dataRef) content = JSON.stringify([input.text, input.dataRef]);
      const textDigest = digest(content);
      const mentionDigest = input.mentionEntities === undefined ? undefined : digest(JSON.stringify(input.mentionEntities));
      if (old) {
        if (old.messageId !== input.messageId || old.operation !== input.operation || old.actorUid !== user.uid
          || old.generation !== input.generation || old.textDigest !== textDigest || old.mentionDigest !== mentionDigest
          || (input.operation === 'create' && (m?.author.kind !== 'user' || m.author.uid !== user.uid || m.kind !== 'text'
            || (m.replyTo ?? null) !== (input.replyTo ?? null)))) fail(409, 'MESSAGE_ID_CONFLICT');
        return { messageId: input.messageId, changeId: input.changeId, changeSeq: old.seq };
      }
      if (input.operation === 'create' && existing.exists) fail(409, 'MESSAGE_ID_CONFLICT');
      if (input.operation !== 'create' && (!m || m.author.kind !== 'user' || m.author.uid !== user.uid)) fail(403, 'MESSAGE_AUTHOR_REQUIRED');
      if (m?.recalled) fail(409, 'MESSAGE_RECALLED');
      const priorTargets = new Set(input.operation === 'edit' ? m.mentionEntities?.map(({ userId }) => userId) : []);
      const newMentions = input.mentionEntities?.filter(({ userId }) => !priorTargets.has(userId));
      const memberRefs = mentionMemberRefs(teamRef, c, newMentions, user.uid);
      if (!memberRefs) fail(409, 'MENTION_TARGET_REQUIRED');
      const replyRefs = input.replyTo ? [ref.collection('messages').doc(input.replyTo)] : [];
      if (replyRefs.length || memberRefs.length) {
        const reads = await tx.getAll(...replyRefs, ...memberRefs);
        if (replyRefs.length && !reads[0].exists) fail(400, 'REPLY_REQUIRED');
        if (reads.slice(replyRefs.length).some(member => !member.exists)) fail(409, 'MENTION_TARGET_REQUIRED');
      }
      const clock = this.clock();
      const releaseFiles = input.operation === 'recall' ? await recallAttachments(tx, teamRef, m, clock) : () => {};
      // A new message also goes to the Chat's Bots that answer unasked; each one's host decides whether it does.
      const unasked = input.operation === 'create' ? await new SharedBotsStore(this.db, this.clock).optionalTasks(tx, teamRef, c,
        input, { uid: user.uid, generation: input.generation }, [], clock) : { write() {} };
      const changeSeq = c.lastChangeSeq + 1, seq = input.operation === 'create' ? c.lastSeq + 1 : m.seq;
      if (!Number.isSafeInteger(changeSeq) || !Number.isSafeInteger(seq)) fail(409, 'CURSOR_LIMIT');
      const now = new Date(clock);
      const change = { seq: changeSeq, messageId: input.messageId, operation: input.operation,
        actorUid: user.uid, generation: input.generation, textDigest, ...(mentionDigest ? { mentionDigest } : {}), at: now };
      const update = { lastChangeSeq: changeSeq, lastChange: { id: input.changeId, seq: changeSeq } };
      if (input.operation === 'create') {
        const author = { kind: 'user', uid: user.uid };
        // The conversation list reads this pointer instead of the message; the author mirrors the message doc.
        Object.assign(update, { lastSeq: seq, lastMessage: { id: input.messageId, seq, at: now, author, preview: messagePreview(input.text) } });
        tx.create(msgRef, { seq, author, kind: 'text', text: input.text,
          ...(input.mentionEntities?.length ? mentionFields(input.mentionEntities) : {}),
          ...(share ? { resourceRef: share.reference, resourceGrant: share.grant } : {}),
          ...(input.replyTo ? { replyTo: input.replyTo } : {}), ...(input.dataRef ? { dataRef: input.dataRef } : {}), createdAt: now, changeSeq, changeId: input.changeId });
        unasked.write(seq);
      } else {
        const entities = input.operation === 'recall' ? []
          : input.mentionEntities ?? preserveMentionEntities(m.text, input.text, m.mentionEntities);
        // Delivery recipients belong to the original send. Editing only changes the inline ranges.
        tx.update(msgRef, { ...(m.mentionEntities?.length || entities?.length ? { mentionEntities: entities ?? [] } : {}),
          text: input.text, editedAt: now, changeSeq, changeId: input.changeId,
          ...(input.operation === 'recall' ? { recalled: true, ...(m.dataRef ? { dataRef: null } : {}), ...(m.resourceRef ? { resourceRef: null, resourceGrant: null } : {}) } : {}) });
        // Only the newest message is mirrored. An older edit/recall leaves the pointer alone; a recall never leaves its text behind.
        if (c.lastMessage?.id === input.messageId) {
          update.lastMessage = { ...c.lastMessage, preview: input.operation === 'recall' ? '' : messagePreview(input.text),
            ...(input.operation === 'recall' ? { recalled: true } : {}) };
        }
      }
      tx.update(ref, update); tx.create(changeRef, change); releaseFiles();
      return { messageId: input.messageId, changeId: input.changeId, changeSeq };
    });
  }
}
