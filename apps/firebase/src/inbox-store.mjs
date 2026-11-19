import { startReadBatch, readBatchStatus } from './inbox-batch.mjs';
import { FieldValue } from 'firebase-admin/firestore';
import { isDeepStrictEqual } from 'node:util';
import { serviceScope } from './hosted-bot-service.mjs';
import { authorizedTargets, inspectNotice } from './inbox-delivery.mjs';
import { messagePreview } from './team-message-preview.mjs';
import {
  DAY,
  attention,
  unread,
  badge,
  shard,
  deny,
  digest,
  emptyRow,
  makeKey,
  splitKey,
  millis,
  capped,
  applyRead,
  messageEvent,
  command,
  deliveryTime,
  searchTitle,
  prefixEnd
} from './inbox-model.mjs';

export const owner = (db, uid) => db.collection('users').doc(uid);
export const source = (db, uid, ref) =>
  ref.kind === 'assistant'
    ? owner(db, uid).collection('inboxSources').doc(ref.sourceId)
    : db.collection(ref.kind === 'team' ? 'teams' : 'services').doc(ref.sourceId);
export async function access(tx, db, uid, ref) {
  const root = source(db, uid, ref);
  if (ref.kind === 'assistant') {
    const [bindingDoc, checkpoint] = await tx.getAll(root, root.collection('checkpoints').doc(ref.conversationId));
    const binding = bindingDoc.data(),
      state = checkpoint.data();
    return binding?.enabled && !state?.removed
      ? { generation: binding.generation, conversationConsentRevision: state?.conversationConsentRevision ?? 0, root }
      : null;
  }
  const thread = root.collection('conversations').doc(ref.conversationId);
  const [s, m, c] = await tx.getAll(root, root.collection('members').doc(uid), thread);
  return chatAccess(root, thread, uid, ref.kind, s, m, c);
}
function chatAccess(root, thread, uid, kind, s, m, c) {
  const scope = s.data(),
    member = m.data(),
    chat = c.data();
  if (scope?.state !== 'active' || !member || !chat?.memberUids?.includes(uid)) return null;
  if (kind === 'service') {
    try {
      serviceScope.check(scope, member, uid);
    } catch {
      return null;
    }
  }
  return { generation: member.generation, root, thread, scope, member, chat };
}
const match = (access, generation) => {
  if (!access) deny(403, 'SOURCE_ACCESS_REQUIRED');
  if (generation !== undefined && access.generation !== generation) deny(409, 'STALE_MEMBERSHIP');
};
// Opt-out deletes the cloud summary, but the owner's scalars must not return
// to an earlier CAS version when this same canonical key becomes visible again.
// Persist only these personal values, never text, host routes or action metadata.
function personalAfterPrivacyChange(row, floor = 0) {
  const manualUnreadRevision = Math.max(row?.manualUnreadRevision ?? 0, floor) + 1;
  return {
    pinOrder: row?.pinOrder ?? null,
    notificationMode: row?.notificationMode ?? 'mentions',
    archived: row?.archived ?? false,
    manualUnread: row?.manualUnread ?? false,
    manualUnreadRevision,
    readRevisionFloor: manualUnreadRevision,
    preferencesRevision: Math.max(row?.preferencesRevision ?? 0, floor) + 1,
    stateRevision: Math.max(row?.stateRevision ?? 0, floor) + 1,
    actualReadRevision: row?.actualReadRevision ?? 0,
    lastActualReadAt: row?.lastActualReadAt ?? 0,
    pendingReadRevision: row?.pendingReadRevision ?? 0,
    pendingReadThrough: row?.pendingReadThrough ?? 0,
    failureReadRevision: row?.failureReadRevision ?? 0
  };
}
function publicRow(row) {
  const {
    isUnread,
    needsAttention,
    pinRank,
    titleSort,
    baseline,
    lastActualReadAt,
    actualReadRevision,
    readRevisionFloor,
    pendingReadRevision,
    pendingReadThrough,
    failureReadRevision,
    completionAt,
    completionId,
    ...result
  } = row;
  return result;
}
export function writeRow(tx, account, doc, before, next, now) {
  if (next)
    next = {
      ...next,
      titleSort: searchTitle(next.title),
      pinRank: next.pinOrder ?? Number.MAX_SAFE_INTEGER,
      isUnread: unread(next),
      needsAttention: attention(next)
    };
  if (isDeepStrictEqual(before, next)) return null;
  const index = shard(doc.id),
    change = Number(badge(next)) - Number(badge(before));
  if (change)
    tx.set(account.collection('inboxBadges').doc(index), { count: FieldValue.increment(change) }, { merge: true });
  tx.set(account.collection('inboxSignals').doc(index), { revision: FieldValue.increment(1) }, { merge: true });
  if (next) tx.set(doc, next);
  else if (before) {
    tx.delete(doc);
    if (before.ref.kind !== 'assistant')
      tx.set(doc.collection('metadata').doc('checkpoint'), {
        generation: before.ref.generation,
        sourceRevision: before.sourceRevision,
        sourceCursor: before.sourceCursor,
        readThrough: before.readThrough
      });
    tx.set(account.collection('inboxTombstones').doc(doc.id), {
      generation: before.ref.generation,
      expiresAt: new Date(now + 30 * DAY)
    });
  }
  return { rowWrites: 1, signalWrites: 1, badgeWrites: Number(change !== 0) };
}
// This index is shared by every reader of the source. Only the cold/read/recall
// path needs queries; the ordinary create path uses the ordered page itself.
async function counts(tx, thread, uid, joinedAt, through, upper) {
  if (through >= upper) return { unreadCount: 0, mentionCount: 0 };
  const base = thread
    .collection('inboxCountEvents')
    .where('seq', '>', through)
    .where('seq', '<=', upper)
    .where('at', '>=', millis(joinedAt));
  const all = base
    .where('actor', '!=', 'user:' + uid)
    .orderBy('actor')
    .orderBy('seq')
    .orderBy('at');
  const mentions = base.where('mentions', 'array-contains', uid).orderBy('seq').orderBy('at');
  const pages = await Promise.all([all, mentions].map(q => tx.get(q.limit(100).select())));
  return { unreadCount: pages[0].size, mentionCount: pages[1].size };
}
async function friendCard(tx, account, row) {
  if (row.category !== 'friend' || !row.peerUid) return null;
  const snapshot = await tx.get(account.collection('friends').doc(row.peerUid));
  const data = snapshot.data();
  return data?.state === 'friends' && data.teamId === row.ref.sourceId && data.generation === row.ref.generation
    ? snapshot
    : null;
}
function writeFriendCount(tx, card, row) {
  if (card && card.data().unreadCount !== row.unreadCount) tx.update(card.ref, { unreadCount: row.unreadCount });
}
async function sourcePage(tx, thread, after, head) {
  if (after > head) deny(503, 'SOURCE_WATERMARK_REGRESSED');
  if (after === head) return { entries: [], byId: new Map(), head, after };
  const changes = await tx.get(
    thread.collection('changes').where('seq', '>', after).where('seq', '<=', head).orderBy('seq').limit(40)
  );
  const entries = changes.docs.map(d => ({ ...d.data(), id: d.id }));
  if (entries.some((v, i) => v.seq !== after + i + 1) || (!entries.length && after < head)) deny(503, 'CHANGE_GAP');
  const ids = [...new Set(entries.map(e => e.messageId))];
  const messages = ids.length ? await tx.getAll(...ids.map(id => thread.collection('messages').doc(id))) : [];
  const byId = new Map(messages.map(d => [d.id, d.data()]));
  const replies = [
    ...new Set(
      messages
        .map(d => d.data()?.replyTo)
        .filter(v => typeof v === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(v) && !byId.has(v))
    )
  ];
  if (replies.length)
    for (const d of await tx.getAll(...replies.map(id => thread.collection('messages').doc(id))))
      byId.set(d.id, d.data());
  return { entries, byId, head, after, sourceReads: entries.length + messages.length + replies.length };
}
async function advanceSharedIndex(db, thread, stats) {
  let writes = 0,
    reads = 0;
  const checkpoint = thread.collection('inboxMetadata').doc('countIndex');
  const result = await db.runTransaction(async tx => {
    writes = 0;
    reads = 1;
    const [chat, saved] = await tx.getAll(thread, checkpoint);
    if (!chat.exists) return 0;
    const head = chat.data().lastChangeSeq,
      after = saved.data()?.revision ?? 0;
    if (after === head) return head;
    const page = await sourcePage(tx, thread, after, head);
    reads += page.sourceReads ?? 0;
    for (const e of page.entries) {
      if (e.operation === 'edit') continue;
      const m = page.byId.get(e.messageId);
      if (!m) deny(503, 'MESSAGE_GAP');
      const doc = thread.collection('inboxCountEvents').doc(e.messageId);
      if (m.recalled || e.operation === 'recall') {
        tx.delete(doc);
        writes++;
        continue;
      }
      const human =
        m.author?.kind === 'user' && ['text', 'status', 'question', 'task_ref', 'app_ref'].includes(m.kind);
      const result =
        m.author?.kind === 'shared_bot' && typeof m.deliveryId === 'string' && typeof m.taskId === 'string';
      if (!human && !result) continue;
      const mentions =
        human && Array.isArray(m.mentions)
          ? m.mentions
              .flatMap(v => {
                if (typeof v !== 'string' || !v || v.length > 133) return [];
                return v.startsWith('user:') ? [v, v.slice(5)] : [v];
              })
              .filter(v => v && v !== m.author.uid)
          : [];
      writes++;
      tx.set(doc, {
        seq: m.seq,
        at: millis(m.createdAt),
        actor: human ? 'user:' + m.author.uid : 'assistant',
        mentions: [...new Set(mentions)]
      });
    }
    const revision = page.entries.at(-1).seq;
    tx.set(checkpoint, { revision });
    return revision;
  });
  if (stats) {
    stats.normalizedEventWrites += writes;
    stats.logicalCanonicalReads += reads;
  }
  return result;
}
export async function mirrorHttpRead(tx, db, uid, ref, through, now, joinedAt) {
  const account = owner(db, uid),
    doc = account.collection('inbox').doc(makeKey(ref));
  const before = (await tx.get(doc)).data();
  const current = before?.ref.generation === ref.generation ? before : emptyRow(ref);
  if (before?.ref.generation === ref.generation && through <= current.readThrough) return;
  const row = applyRead(current, through, undefined);
  row.sourceCursor = Math.max(row.sourceCursor, through);
  Object.assign(
    row,
    await counts(
      tx,
      source(db, uid, ref).collection('conversations').doc(ref.conversationId),
      uid,
      joinedAt,
      row.readThrough,
      current.sourceCursor
    ),
    {
      lastActualReadAt: now,
      actualReadRevision: (current.actualReadRevision ?? 0) + 1
    }
  );
  writeFriendCount(tx, await friendCard(tx, account, row), row);
  writeRow(tx, account, doc, before, row, now);
}
export async function captureTiming(tx, account, now) {
  const present = await tx.get(
    account
      .collection('inboxPresence')
      .where('foreground', '==', true)
      .where('expiresAt', '>', new Date(now))
      .orderBy('expiresAt', 'desc')
      .limit(2)
  );
  return { primary: present.docs[0]?.id ?? null, multiple: present.size > 1 };
}
export function addNotice(tx, account, row, event, now) {
  if (event.kind === 'message' && row.notificationMode !== 'all') return;
  if (event.at < now - 30 * DAY || event.at > now + 60000) return;
  const id = digest(account.id, row.key, row.ref.generation, row.conversationConsentRevision ?? 0, event.id);
  if (event.kind === 'completion' && event.at >= (row.completionAt ?? 0)) {
    row.completionAt = event.at;
    row.completionId = id;
  }
  tx.set(account.collection('inboxNotices').doc(id), {
    id,
    key: row.key,
    generation: row.ref.generation,
    through: event.cursor,
    sourceRevision: row.sourceRevision,
    kind: event.kind,
    sourceEventId: event.id,
    conversationConsentRevision: row.conversationConsentRevision ?? 0,
    timing: event.timing,
    ...(event.failureRevision !== undefined ? { failureRevision: event.failureRevision } : {}),
    lastActivityAt: event.at,
    createdReadRevision: row.actualReadRevision ?? 0,
    ...(['input', 'approval'].includes(event.kind) ? { pendingActionRevision: row.pendingActionRevision } : {}),
    expiresAt: new Date(event.at + 30 * DAY),
    deliverUntil: new Date(event.at + 3600000),
    ...(event.messageId
      ? { messageId: event.messageId, ...(event.createChangeId ? { createChangeId: event.createChangeId } : {}) }
      : {}),
    ...(event.taskId ? { taskId: event.taskId, waitId: event.waitId ?? null } : {})
  });
}
export class InboxStore {
  constructor(db, clock = Date.now, enqueue = null, observe = null) {
    this.observe = observe;
    this.enqueue = enqueue;
    this.db = db;
    this.clock = clock;
  }
  async execute(uid, raw) {
    const p = command.parse(raw),
      account = owner(this.db, uid),
      now = this.clock();
    if (p.action === 'notice')
      return this.db.runTransaction(
        async tx => {
          const d = await tx.get(account.collection('inboxNotices').doc(p.id));
          const item = await this.noticeItem(tx, uid, d, p.deviceId);
          if (!item) deny(404, 'NOTICE_MISSING');
          return { item };
        },
        { readOnly: true }
      );
    if (p.action === 'markAllReadSnapshot') return startReadBatch(this, uid, p);
    if (p.action === 'readBatchStatus') return readBatchStatus(this, uid, p.jobId);
    if (p.action === 'list' || p.action === 'notices') return this.list(uid, p);
    if (p.action === 'assistantSource') return this.bind(uid, p);
    if (p.action === 'assistantPublish' || p.action === 'assistantRemove') return this.publish(uid, p);
    if (p.action === 'markAllRead') {
      const results = [];
      for (const entry of p.entries) {
        try {
          await this.mutate(uid, { ...entry, action: 'markRead' });
          results.push({ key: entry.key, ok: true });
        } catch (e) {
          if (![403, 409].includes(e.status)) throw e;
          results.push({ key: entry.key, ok: false, code: e.code });
        }
      }
      return { results };
    }
    if (p.action === 'accountPreferences') {
      const doc = account.collection('inboxSettings').doc('personal');
      if (p.dmReceipts !== undefined) await doc.set({ dmReceipts: p.dmReceipts }, { merge: true });
      return { dmReceipts: (await doc.get()).data()?.dmReceipts === true };
    }
    if (p.action === 'presence') {
      return this.db.runTransaction(async tx => {
        if (p.key) {
          if (!p.generation) deny(400, 'GENERATION_REQUIRED');
          match(await access(tx, this.db, uid, splitKey(p.key)), p.generation);
        }
        tx.set(account.collection('inboxPresence').doc(p.deviceId), {
          key: p.key,
          generation: p.generation ?? 0,
          foreground: p.foreground,
          expiresAt: new Date(now + 45000)
        });
        return { ok: true };
      });
    }
    return this.mutate(uid, p);
  }
  async noticeItem(tx, uid, d, deviceId) {
    const v = d.data();
    if (!v || millis(v.expiresAt) <= this.clock()) return null;
    const ref = splitKey(v.key),
      acl = await access(tx, this.db, uid, ref);
    if (
      acl?.generation !== v.generation ||
      (ref.kind === 'assistant' && (acl.conversationConsentRevision ?? 0) !== (v.conversationConsentRevision ?? 0))
    )
      return null;
    return {
      id: d.id,
      key: v.key,
      generation: v.generation,
      through: v.through,
      kind: v.kind,
      ...(v.failureRevision !== undefined ? { failureRevision: v.failureRevision } : {}),
      ...(v.pendingActionRevision !== undefined ? { pendingActionRevision: v.pendingActionRevision } : {}),
      ...(await authorizedTargets(tx, this.db, uid, acl, v)),
      lastActivityAt: v.lastActivityAt,
      deliverAfter: deliveryTime(v, deviceId),
      deliverable: await inspectNotice(tx, this.db, uid, v, this.clock())
    };
  }
  async list(uid, p) {
    const notices = p.action === 'notices',
      account = owner(this.db, uid);
    return this.db.runTransaction(
      async tx => {
        let q = account.collection(notices ? 'inboxNotices' : 'inbox');
        const prefix = searchTitle(p.searchPrefix ?? '');
        if (!notices) {
          if (p.category) q = q.where('category', '==', p.category);
          if (p.filter === 'unread') q = q.where('isUnread', '==', true);
          if (p.filter === 'attention') q = q.where('needsAttention', '==', true);
          if (p.archived === true || !['unread', 'attention'].includes(p.filter))
            q = q.where('archived', '==', p.archived ?? false);
          if (prefix) {
            q = q.where('titleSort', '>=', prefix).orderBy('titleSort', 'asc');
            const end = prefixEnd(prefix);
            if (end) q = q.where('titleSort', '<', end);
          } else q = q.orderBy('pinRank', 'asc');
        }
        if (!prefix) q = q.orderBy('lastActivityAt', 'desc');
        q = q.orderBy('__name__', 'asc');
        if (p.after) {
          if (prefix) {
            const anchor =
              p.after.titleSort ?? (await tx.get(account.collection('inbox').doc(p.after.key))).data()?.titleSort;
            if (typeof anchor !== 'string') deny(409, 'SEARCH_CURSOR_EXPIRED');
            q = q.startAfter(anchor, p.after.key);
          } else if (notices) q = q.startAfter(p.after.lastActivityAt, p.after.key);
          else {
            const pin =
              p.after.pinOrder === undefined
                ? (await tx.get(account.collection('inbox').doc(p.after.key))).data()?.pinOrder
                : p.after.pinOrder;
            q = q.startAfter(pin ?? Number.MAX_SAFE_INTEGER, p.after.lastActivityAt, p.after.key);
          }
        }
        const docs = (await tx.get(q.limit(51))).docs,
          slice = docs.slice(0, 50);
        const items = await Promise.all(
          slice.map(async d => {
            if (notices) return this.noticeItem(tx, uid, d, p.deviceId);
            const v = d.data(),
              acl = await access(tx, this.db, uid, v.ref);
            return acl?.generation === v.ref.generation ? publicRow(v) : null;
          })
        );
        return {
          items: items.filter(Boolean),
          next:
            docs.length > 50
              ? {
                  lastActivityAt: slice.at(-1).data().lastActivityAt,
                  key: slice.at(-1).id,
                  ...(prefix
                    ? { titleSort: slice.at(-1).data().titleSort }
                    : notices
                      ? {}
                      : { pinOrder: slice.at(-1).data().pinOrder })
                }
              : null
        };
      },
      { readOnly: true }
    );
  }
  async mutate(uid, p) {
    const ref = splitKey(p.key),
      account = owner(this.db, uid),
      doc = account.collection('inbox').doc(p.key),
      now = this.clock();
    return this.db.runTransaction(async tx => {
      const acl = await access(tx, this.db, uid, ref);
      match(acl, p.generation);
      const before = (await tx.get(doc)).data();
      const current = before?.ref.generation === acl.generation ? before : null;
      if (!current && (p.action === 'get' || ref.kind === 'assistant')) deny(404, 'INBOX_ITEM_MISSING');
      let row = { ...(current ?? emptyRow({ ...ref, generation: acl.generation })) };
      if (p.action === 'get') return { item: publicRow(row) };
      if (p.action === 'peerReceipt') return this.receipt(tx, uid, acl);
      if (p.action === 'preferences') {
        if (p.expectedPreferencesRevision !== (row.preferencesRevision ?? 0))
          deny(409, 'PREFERENCE_REVISION_CONFLICT');
        row = {
          ...row,
          ...p.patch,
          preferencesRevision: (row.preferencesRevision ?? 0) + 1,
          stateRevision: row.stateRevision + 1
        };
      } else if (p.action === 'markUnread') {
        if (p.expectedManualUnreadRevision === undefined) {
          // A previously recorded pre-release mutation can be acknowledged.
          const old = account
            .collection('inboxOperations')
            .doc(digest(p.action, p.key, acl.generation, p.operationId));
          if ((await tx.get(old)).exists) return { item: publicRow(row) };
          deny(409, 'MANUAL_REVISION_REQUIRED');
        }
        if (p.expectedManualUnreadRevision < row.manualUnreadRevision) return { item: publicRow(row) };
        if (p.expectedManualUnreadRevision !== row.manualUnreadRevision) deny(409, 'MANUAL_REVISION_CONFLICT');
        row = {
          ...row,
          manualUnread: true,
          manualUnreadRevision: row.manualUnreadRevision + 1,
          stateRevision: row.stateRevision + 1
        };
      } else if (p.action === 'markRead') {
        if (p.manualUnreadRevision < (row.readRevisionFloor ?? 0)) return { item: publicRow(row) };
        if (p.through > (acl.chat?.lastSeq ?? row.sourceCursor)) deny(409, 'INVALID_READ_CURSOR');
        let through = p.through,
          readRef,
          previousRead;
        if (acl.thread) {
          readRef = acl.thread.collection('reads').doc(uid);
          previousRead = (await tx.get(readRef)).data();
          if (previousRead?.generation === acl.generation) through = Math.max(through, previousRead.lastReadSeq);
        }
        // An offline request proves observation only up to the cursor it carried.
        // A larger canonical cursor from another device cannot extend that proof.
        const advancing = p.through > row.readThrough;
        const acknowledgesPending =
          p.observedPendingActionRevision > 0 &&
          p.observedPendingActionRevision === row.pendingActionRevision &&
          row.pendingActionCount > 0 &&
          p.through >= Math.max(row.sourceCursor, row.readThrough);
        if (acknowledgesPending) {
          row.pendingReadRevision = p.observedPendingActionRevision;
          row.pendingReadThrough = p.through;
        }
        if (
          p.observedFailureRevision > 0 &&
          p.observedFailureRevision === row.failureRevision &&
          row.state === 'failed' &&
          p.through >= Math.max(row.sourceCursor, row.readThrough)
        )
          row.failureReadRevision = p.observedFailureRevision;
        const refreshCount = through > row.readThrough;
        row = applyRead(row, through, p.manualUnreadRevision);
        row.sourceCursor = Math.max(through, row.sourceCursor);
        if (refreshCount)
          Object.assign(
            row,
            acl.thread
              ? await counts(tx, acl.thread, uid, acl.member.joinedAt, row.readThrough, row.sourceCursor)
              : { unreadCount: capped(row.sourceCursor - Math.max(row.baseline, row.readThrough)) }
          );
        if (advancing) {
          row.lastActualReadAt = now;
          row.actualReadRevision = (row.actualReadRevision ?? 0) + 1;
        }
        writeFriendCount(tx, await friendCard(tx, account, row), row);
        if (readRef && (previousRead?.generation !== acl.generation || previousRead.lastReadSeq !== row.readThrough))
          tx.set(readRef, { lastReadSeq: row.readThrough, generation: acl.generation });
      }
      writeRow(tx, account, doc, before, row, now);
      return { item: publicRow(row) };
    });
  }
  async receipt(tx, uid, acl) {
    if (
      !acl.chat ||
      acl.chat.kind !== 'dm' ||
      acl.chat.memberUids.length !== 2 ||
      acl.chat.participants?.some(p => !p.startsWith('user:'))
    )
      return { readThrough: null };
    const peer = acl.chat.memberUids.find(u => u !== uid);
    if (!peer) return { readThrough: null };
    const [a, b, m, r] = await tx.getAll(
      ...[uid, peer].map(u => owner(this.db, u).collection('inboxSettings').doc('personal')),
      acl.root.collection('members').doc(peer),
      acl.thread.collection('reads').doc(peer)
    );
    const allowed =
      a.data()?.dmReceipts === true &&
      b.data()?.dmReceipts === true &&
      m.exists &&
      r.data()?.generation === m.data().generation;
    return { readThrough: allowed ? r.data().lastReadSeq : null };
  }
  async bind(uid, p) {
    const account = owner(this.db, uid),
      doc = account.collection('inboxSources').doc(p.sourceId),
      hash = digest(uid, p.sourceId, p.sourceSecret);
    const result = await this.db.runTransaction(async tx => {
      const prev = (await tx.get(doc)).data();
      if (prev && prev.secretDigest !== hash) deny(403, 'SOURCE_ACCESS_REQUIRED');
      if (prev?.consentRevision) {
        if (p.consentRevision === undefined && prev.enabled !== p.enabled) deny(409, 'CONSENT_REVISION_REQUIRED');
        if (
          p.consentRevision < prev.consentRevision ||
          (p.consentRevision === undefined && prev.enabled === p.enabled)
        ) {
          return { enabled: prev.enabled, generation: prev.generation, consentRevision: prev.consentRevision };
        }
        if (p.consentRevision === prev.consentRevision && prev.enabled !== p.enabled)
          deny(409, 'CONSENT_REVISION_CONFLICT');
      }
      if (!prev && !p.enabled && !p.consentRevision) deny(404, 'SOURCE_NOT_FOUND');
      const generation = (prev?.generation ?? 0) + Number(prev?.enabled !== p.enabled);
      const binding = {
        enabled: p.enabled,
        generation,
        ...(p.consentRevision ? { consentRevision: p.consentRevision } : {})
      };
      tx.set(doc, { secretDigest: hash, ...binding });
      return binding;
    });
    if (!result.enabled) await this.reconcileOwner(uid, 'assistant', p.sourceId);
    return result;
  }
  async publish(uid, p) {
    const account = owner(this.db, uid),
      ref = { kind: 'assistant', sourceId: p.sourceId, conversationId: p.conversationId, generation: p.generation };
    const doc = account.collection('inbox').doc(makeKey(ref)),
      now = this.clock();
    return this.db.runTransaction(async tx => {
      const root = source(this.db, uid, ref),
        watermark = root.collection('checkpoints').doc(p.conversationId);
      const [binding, snapshot, checkpoint] = await tx.getAll(root, doc, watermark);
      const b = binding.data(),
        before = snapshot.data();
      if (!b?.enabled || b.secretDigest !== digest(uid, p.sourceId, p.sourceSecret))
        deny(403, 'SOURCE_ACCESS_REQUIRED');
      if (p.generation !== b.generation) deny(409, 'STALE_MEMBERSHIP');
      const current = before?.ref.generation === p.generation ? before : null;
      const saved = checkpoint.data();
      const oldConsent = saved?.conversationConsentRevision ?? 0;
      const newConsent = p.conversationConsentRevision ?? 0;
      if (oldConsent && newConsent < oldConsent) deny(409, 'STALE_CONVERSATION_CONSENT');
      const restoring = saved?.removed && saved.generation === p.generation;
      // Older experimental tombstones have only a read cursor. Give those a
      // fresh server-time version range instead of reusing zero-based revisions.
      const restoredPersonal = restoring
        ? (saved.personal ?? personalAfterPrivacyChange(null, millis(checkpoint.updateTime)))
        : null;
      if (p.action === 'assistantRemove') {
        if (p.sourceRevision < (saved?.sourceRevision ?? 0) && newConsent <= oldConsent)
          deny(409, 'STALE_SOURCE_REVISION');
        tx.set(watermark, {
          generation: p.generation,
          sourceRevision: Math.max(p.sourceRevision, saved?.sourceRevision ?? 0),
          sourceCursor: saved?.sourceCursor ?? 0,
          removed: true,
          personal: restoredPersonal ?? personalAfterPrivacyChange(current),
          conversationConsentRevision: newConsent,
          readThrough: current?.readThrough ?? saved?.readThrough ?? 0
        });
        if (before) writeRow(tx, account, doc, before, null, now);
        return { removed: true, sourceRevision: Math.max(p.sourceRevision, saved?.sourceRevision ?? 0) };
      }
      if (saved?.removed && (oldConsent || saved.generation === p.generation) && newConsent <= oldConsent)
        deny(410, 'SOURCE_REMOVED');
      const advancedConsent = current && newConsent > oldConsent;
      // An enable can arrive before the intervening offline disable. Commit its
      // newer consent and fences even when the semantic snapshot is unchanged.
      if (advancedConsent && (p.sourceRevision <= current.sourceRevision || p.sourceCursor < current.sourceCursor)) {
        tx.set(watermark, { conversationConsentRevision: newConsent }, { merge: true });
        writeRow(
          tx,
          account,
          doc,
          before,
          { ...current, ...personalAfterPrivacyChange(current), conversationConsentRevision: newConsent },
          now
        );
        return { sourceRevision: current.sourceRevision, sourceCursor: current.sourceCursor };
      }
      if (saved && (p.sourceRevision < saved.sourceRevision || p.sourceCursor < saved.sourceCursor)) {
        return { sourceRevision: saved.sourceRevision, sourceCursor: saved.sourceCursor };
      }
      if (current && p.sourceRevision <= current.sourceRevision)
        return { sourceRevision: current.sourceRevision, sourceCursor: current.sourceCursor };
      if (
        p.baseline > p.sourceCursor ||
        p.sourceCursor < (current?.sourceCursor ?? 0) ||
        (current && current.baseline !== p.baseline) ||
        p.lastActivityAt > now + 60000 ||
        (p.failureAt ?? 0) > now + 60000 ||
        (p.failureRevision !== undefined && p.failureRevision < (current?.failureRevision ?? 0)) ||
        (p.pendingActionAt ?? 0) > now + 60000 ||
        (p.pendingActionRevision !== undefined && p.pendingActionRevision < (current?.pendingActionRevision ?? 0))
      )
        deny(409, 'INVALID_SOURCE_CURSOR');
      const row = {
        ...(current ?? emptyRow(ref)),
        ...(advancedConsent ? personalAfterPrivacyChange(current) : restoredPersonal),
        title: p.title ?? 'Assistant',
        preview: { reply: 'Reply', taskResult: 'Task completed', failure: 'Task failed' }[p.previewKind] ?? '',
        baseline: p.baseline,
        readThrough: Math.max(p.baseline, current?.readThrough ?? saved?.readThrough ?? 0),
        sourceRevision: p.sourceRevision,
        sourceCursor: p.sourceCursor,
        lastActivityAt: Math.max(current?.lastActivityAt ?? 0, p.lastActivityAt),
        state: p.state,
        pendingActionCount: capped(p.pendingActionCount),
        offline: p.state === 'offline',
        conversationConsentRevision: newConsent
      };
      delete row.attentionKind;
      delete row.remoteHostId;
      delete row.sourceLabel;
      if (p.remoteHostId) row.remoteHostId = p.remoteHostId;
      if (p.sourceLabel) row.sourceLabel = p.sourceLabel;
      if (p.state === 'failed') row.attentionKind = 'failure';
      row.unreadCount = capped(row.sourceCursor - row.readThrough);
      const pending = row.pendingActionCount > 0 && ['waiting_for_input', 'waiting_for_approval'].includes(p.state);
      const previousPending = (current?.pendingActionCount ?? 0) > 0;
      const actionChanged =
        pending !== previousPending ||
        ((pending || previousPending) &&
          (current?.state !== p.state || (current?.pendingActionCount ?? 0) !== row.pendingActionCount));
      const actionRevision = p.pendingActionRevision ?? (current?.pendingActionRevision ?? 0) + Number(actionChanged);
      const newAction = actionRevision !== (current?.pendingActionRevision ?? 0) || actionChanged;
      row.pendingActionRevision = actionRevision;
      row.pendingActionAt = p.pendingActionAt || (newAction ? now : (current?.pendingActionAt ?? 0));
      const changedFailure = (current?.state === 'failed') !== (p.state === 'failed');
      const failureRevision = p.failureRevision ?? (current?.failureRevision ?? 0) + Number(changedFailure);
      row.failureRevision = failureRevision;
      row.failureAt = p.failureAt || (changedFailure ? now : (current?.failureAt ?? 0));
      let kind;
      if (pending && newAction) kind = p.state === 'waiting_for_input' ? 'input' : 'approval';
      else if (p.state === 'failed' && (changedFailure || failureRevision !== current?.failureRevision))
        kind = 'failure';
      else if (row.sourceCursor > (current?.sourceCursor ?? p.baseline))
        kind = p.previewKind === 'failure' ? 'failure' : 'completion';
      if (kind && !(saved?.removed && p.sourceRevision <= saved.sourceRevision)) {
        const actionNotice = kind === 'approval' || kind === 'input',
          statusFailure = kind === 'failure' && row.state === 'failed';
        addNotice(
          tx,
          account,
          row,
          {
            kind,
            cursor: row.sourceCursor,
            timing: await captureTiming(tx, account, now),
            id: actionNotice
              ? 'action:' + actionRevision
              : statusFailure
                ? 'failure:' + failureRevision
                : 'assistant:' + p.sourceRevision,
            at: actionNotice ? row.pendingActionAt : statusFailure ? row.failureAt : p.lastActivityAt,
            ...(statusFailure ? { failureRevision } : {})
          },
          now
        );
      }
      tx.set(watermark, {
        generation: p.generation,
        sourceRevision: p.sourceRevision,
        sourceCursor: p.sourceCursor,
        removed: false,
        conversationConsentRevision: newConsent
      });
      writeRow(tx, account, doc, before, row, now);
      return { sourceRevision: row.sourceRevision, sourceCursor: row.sourceCursor };
    });
  }
  // Consume the immutable source change log in sequence. Eventarc is only the wake-up:
  // concurrent folds serialize on the recipient row, and a redelivery resumes at its watermark.
  async fold(uid, ref, prepared, stats) {
    const account = owner(this.db, uid),
      doc = account.collection('inbox').doc(makeKey(ref)),
      now = this.clock();
    const indexed =
      ref.kind === 'assistant'
        ? 0
        : (prepared?.head ??
          (await advanceSharedIndex(
            this.db,
            source(this.db, uid, ref).collection('conversations').doc(ref.conversationId)
          )));
    let cost;
    const result = await this.db.runTransaction(async tx => {
      cost = {
        logicalCanonicalReads: ref.kind === 'assistant' ? 0 : 4,
        rowWrites: 0,
        signalWrites: 0,
        badgeWrites: 0,
        recountQueries: 0
      };
      let acl, snapshot, read, checkpoint;
      if (ref.kind === 'assistant') {
        [acl, snapshot] = await Promise.all([access(tx, this.db, uid, ref), tx.get(doc)]);
      } else {
        const root = source(this.db, uid, ref),
          thread = root.collection('conversations').doc(ref.conversationId);
        const docs = await tx.getAll(
          root,
          root.collection('members').doc(uid),
          thread,
          doc,
          thread.collection('reads').doc(uid),
          doc.collection('metadata').doc('checkpoint')
        );
        acl = chatAccess(root, thread, uid, ref.kind, ...docs.slice(0, 3));
        [snapshot, read, checkpoint] = docs.slice(3);
      }
      const before = snapshot.data();
      if (!acl) {
        if (before) Object.assign(cost, writeRow(tx, account, doc, before, null, now));
        return false;
      }
      if (ref.kind === 'assistant') {
        if (before && before.ref.generation !== acl.generation) writeRow(tx, account, doc, before, null, now);
        return false;
      }
      const current = before?.ref.generation === acl.generation ? before : null;
      const row = { ...(current ?? emptyRow({ ...ref, generation: acl.generation })) },
        chat = acl.chat;
      const saved = checkpoint?.data();
      const resume =
        saved?.generation === acl.generation && (!current || current.sourceRevision < saved.sourceRevision);
      if (resume)
        Object.assign(row, {
          sourceRevision: saved.sourceRevision,
          sourceCursor: saved.sourceCursor,
          readThrough: Math.max(row.readThrough, saved.readThrough)
        });
      const page =
        prepared?.head === indexed && prepared.after === row.sourceRevision
          ? prepared
          : await sourcePage(
              tx,
              acl.thread,
              row.sourceRevision,
              Math.max(row.sourceRevision, Math.min(indexed, chat.lastChangeSeq))
            );
      if (page !== prepared) cost.logicalCanonicalReads += page.sourceReads ?? 0;
      const { entries, byId } = page;
      const seen = read.data();
      if (seen?.generation === acl.generation && seen.lastReadSeq > row.readThrough) {
        row.readThrough = seen.lastReadSeq;
        row.stateRevision++;
        row.actualReadRevision = (row.actualReadRevision ?? 0) + 1;
        row.lastActualReadAt = now;
      }
      const added = [], notices = [];
      let hasRecall = false;
      for (const e of entries) {
        const m = byId.get(e.messageId);
        if (!m) deny(503, 'MESSAGE_GAP');
        row.sourceRevision = e.seq;
        if (e.operation === 'recall') {
          hasRecall = true;
          continue;
        }
        if (e.operation !== 'create') continue;
        row.sourceCursor = Math.max(row.sourceCursor, m.seq);
        if (millis(m.createdAt) < millis(acl.member.joinedAt)) continue;
        const semanticResult =
          m.author?.kind === 'shared_bot' &&
          !m.recalled &&
          typeof m.deliveryId === 'string' &&
          typeof m.taskId === 'string';
        const event = semanticResult
          ? { cursor: m.seq, kind: 'completion', mention: false }
          : messageEvent(m, uid, chat, byId.get(m.replyTo));
        if (!event || m.seq <= row.readThrough) continue;
        added.push(event);
        if (!semanticResult)
          notices.push({
            ...event,
            id: e.messageId,
            at: millis(m.createdAt),
            messageId: e.messageId,
            createChangeId: e.id
          });
      }
      if (resume || (current && (row.readThrough !== current.readThrough || hasRecall))) {
        if (row.readThrough < row.sourceCursor) cost.recountQueries += 2;
        Object.assign(row, await counts(tx, acl.thread, uid, acl.member.joinedAt, row.readThrough, row.sourceCursor));
      } else {
        row.unreadCount = capped(row.unreadCount + added.length);
        row.mentionCount = capped(row.mentionCount + added.filter(e => e.mention).length);
      }
      row.category = acl.scope.kind === 'pair' ? 'friend' : ref.kind;
      // Clients group Bot DMs apart from people's chats; rows folded before this field fill it in on their next fold.
      if (['dm', 'group', 'channel', 'bot_dm'].includes(chat.kind)) row.conversationKind = chat.kind;
      row.title = messagePreview(chat.title) || 'Chat';
      row.sourceLabel = messagePreview(acl.scope.name) || row.title;
      let card;
      if (row.category === 'friend') {
        const peer = chat.memberUids.find(other => other !== uid);
        if (peer) {
          row.peerUid = peer;
          card = await friendCard(tx, account, row);
          row.title = messagePreview(card?.data().displayName) || row.title;
        }
      }
      const caughtUp = row.sourceRevision === chat.lastChangeSeq;
      if (caughtUp) {
        row.sourceCursor = chat.lastSeq;
        row.preview = messagePreview(chat.lastMessage?.preview);
        row.lastActivityAt = millis(chat.lastMessage?.at ?? acl.scope.createdAt);
      }
      const admitted = notices.filter(e => e.kind !== 'message' || row.notificationMode === 'all');
      const timing = admitted.length ? await captureTiming(tx, account, now) : null;
      writeFriendCount(tx, card, row);
      for (const e of admitted) addNotice(tx, account, row, { ...e, timing }, now);
      Object.assign(cost, writeRow(tx, account, doc, before, row, now));
      return !caughtUp;
    });
    if (stats) for (const k of Object.keys(cost)) stats[k] += cost[k];
    return result;
  }
  async reconcileOwner(uid, kind, sourceId, after) {
    const q = owner(this.db, uid).collection('inbox').where('ref.sourceId', '==', sourceId).orderBy('__name__');
    const page = await (after ? q.startAfter(after) : q).limit(64).get();
    await pooled(
      page.docs.filter(d => d.data().ref.kind === kind),
      async d => {
        const more = await this.fold(uid, d.data().ref);
        if (more && this.enqueue) await this.enqueue({ type: 'fold', ref: d.data().ref });
      }
    );
    if (page.size === 64) {
      const next = page.docs.at(-1).id;
      if (this.enqueue) await this.enqueue({ type: 'reconcile', uid, kind, sourceId, after: next });
      else await this.reconcileOwner(uid, kind, sourceId, next);
    }
  }
  async fanout(ref, previous = []) {
    const started = Date.now(),
      stats = {
        sourceKind: ref.kind,
        sourceHash: digest(makeKey(ref)),
        recipientCount: 0,
        logicalCanonicalReads: 1,
        normalizedEventWrites: 0,
        rowWrites: 0,
        signalWrites: 0,
        badgeWrites: 0,
        recountQueries: 0
      };
    const root = source(this.db, '_', ref),
      thread = root.collection('conversations').doc(ref.conversationId);
    const conv = (await thread.get()).data();
    if ((conv?.memberUids?.length ?? 0) > 200) deny(503, 'RECIPIENT_LIMIT');
    const uids = [...new Set([...(conv?.memberUids ?? []), ...previous])];
    stats.recipientCount = uids.length;
    if (!uids.length) return false;
    // Source pages are common to every recipient with the same watermark. Read
    // them once. Each recipient transaction rechecks both ACL and source head.
    const snapshots = await this.db.getAll(
      ...uids.map(uid => owner(this.db, uid).collection('inbox').doc(makeKey(ref))),
      { fieldMask: ['sourceRevision'] }
    );
    const after = snapshots.map(d => d.data()?.sourceRevision ?? 0),
      pages = new Map();
    const indexed = conv ? await advanceSharedIndex(this.db, thread, stats) : 0;
    if (conv)
      await pooled([...new Set(after)], async revision => {
        const page = await this.db.runTransaction(
          tx => sourcePage(tx, thread, revision, Math.max(indexed, revision)),
          {
            readOnly: true
          }
        );
        stats.logicalCanonicalReads += page.sourceReads ?? 0;
        pages.set(revision, page);
      });
    let more = false;
    await pooled(
      uids.map((uid, i) => ({ uid, page: pages.get(after[i]) })),
      async ({ uid, page }) => {
        if (await this.fold(uid, ref, page, stats)) more = true;
      }
    );
    this.observe?.({ ...stats, durationMs: Date.now() - started, more });
    return more;
  }
}
export async function pooled(items, fn) {
  let i = 0;
  const result = await Promise.allSettled(
    Array.from({ length: Math.min(8, items.length) }, async () => {
      while (i < items.length) await fn(items[i++]);
    })
  );
  const failure = result.find(r => r.status === 'rejected');
  if (failure) throw failure.reason;
}
