import { inboxNotificationEnabled } from './inbox-rollout.mjs';
import { FieldValue } from 'firebase-admin/firestore';
import { DAY, deliveryTime, shard, alertEnabled, digest, millis, splitKey } from './inbox-model.mjs';
import { taskAllowed } from './inbox-tasks.mjs';
import { buildPushMessage } from './push.mjs';
import { access, captureTiming, owner, pooled } from './inbox-store.mjs';

export function eligible(event, row, now) {
  if (
    !row ||
    row.ref.generation !== event.generation ||
    row.archived ||
    !alertEnabled(row.notificationMode, event.kind) ||
    millis(event.deliverUntil) <= now ||
    millis(event.expiresAt) <= now
  )
    return false;
  if (event.kind === 'input' || event.kind === 'approval') {
    const observed =
      event.pendingActionRevision > 0 &&
      (row.pendingReadRevision ?? 0) >= event.pendingActionRevision &&
      (row.pendingReadThrough ?? -1) >= event.through;
    if (observed || row.pendingActionCount === 0) return false;
    // A Team wait has its own authoritative task/wait ID checked below. The
    // Assistant descriptor carries the source action-set revision instead.
    return (
      Boolean(event.taskId) ||
      (row.pendingActionRevision === event.pendingActionRevision &&
        row.state === (event.kind === 'input' ? 'waiting_for_input' : 'waiting_for_approval'))
    );
  }
  if (
    event.failureRevision !== undefined &&
    (row.state !== 'failed' || row.failureRevision !== event.failureRevision)
  )
    return false;
  if (event.failureRevision !== undefined && row.failureReadRevision === event.failureRevision) return false;
  const actualRead =
    row.readThrough >= event.through && (row.actualReadRevision ?? 0) > (event.createdReadRevision ?? 0);
  if (actualRead) return false;
  if (
    event.kind === 'completion' &&
    row.completionId &&
    row.completionId !== event.id &&
    row.completionAt >= event.lastActivityAt &&
    row.completionAt <= event.lastActivityAt + 5000
  )
    return false;
  return true;
}
const safeTarget = id => typeof id === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(id);
export async function authorizedTargets(tx, db, uid, acl, event) {
  const target = {};
  if (!acl.thread) return target;
  if (safeTarget(event.taskId)) {
    const task = (await tx.get(acl.root.collection('tasks').doc(event.taskId))).data();
    if (
      !(await taskAllowed(tx, db, acl, task, uid)) ||
      (['input', 'approval'].includes(event.kind) &&
        (task.state !== 'waiting' || task.currentWaitId !== event.waitId))
    )
      return target;
    target.taskId = event.taskId;
  }
  if (safeTarget(event.messageId)) {
    const m = (await tx.get(acl.thread.collection('messages').doc(event.messageId))).data();
    if (m && !m.recalled) target.messageId = event.messageId;
  }
  return target;
}
export async function inspectNotice(tx, db, uid, event, now) {
  if (!event || !inboxNotificationEnabled(uid)) return false;
  const ref = splitKey(event.key),
    account = owner(db, uid);
  const [acl, doc] = await Promise.all([
    access(tx, db, uid, ref),
    tx.get(account.collection('inbox').doc(event.key))
  ]);
  if (
    acl?.generation !== event.generation ||
    (ref.kind === 'assistant' &&
      (acl.conversationConsentRevision ?? 0) !== (event.conversationConsentRevision ?? 0)) ||
    !eligible(event, doc.data(), now)
  )
    return false;
  if (!acl.thread) return true;
  const read = (await tx.get(acl.thread.collection('reads').doc(uid))).data();
  if (!event.taskId && read?.generation === event.generation && read.lastReadSeq >= event.through) return false;
  if (event.messageId) {
    const message = (await tx.get(acl.thread.collection('messages').doc(event.messageId))).data();
    if (!message || message.recalled || message.author?.uid === uid) return false;
    if (message.author?.kind === 'user' && !event.taskId) {
      const change = event.sourceEventId;
      const [sender, receipt] = await tx.getAll(
        acl.root.collection('members').doc(message.author.uid),
        acl.thread.collection('changes').doc(event.createChangeId ?? message.changeId)
      );
      if (
        !sender.exists ||
        receipt.data()?.messageId !== change ||
        receipt.data().operation !== 'create' ||
        sender.data().generation !== receipt.data().generation ||
        !acl.chat.memberUids.includes(message.author.uid)
      )
        return false;
    }
  }
  if (event.taskId) {
    const task = (await tx.get(acl.root.collection('tasks').doc(event.taskId))).data();
    if (
      !(await taskAllowed(tx, db, acl, task, uid)) ||
      (['input', 'approval'].includes(event.kind) &&
        (task.state !== 'waiting' || task.currentWaitId !== event.waitId || task.deadline <= now))
    )
      return false;
  }
  return true;
}
export class InboxDelivery {
  constructor({ db, pushStore, messaging, enqueue, clock = Date.now }) {
    Object.assign(this, { db, pushStore, messaging, enqueue, clock });
  }
  async onNotice(uid, id) {
    const account = owner(this.db, uid),
      history = account.collection('inboxNotices');
    const now = this.clock(),
      notice = history.doc(id);
    // Freeze the fallback for this event. A device going idle or receiving a
    // push before the deadline does not shorten another device's waiting time.
    const event = await this.db.runTransaction(async tx => {
      const initial = (await tx.get(notice)).data();
      if (!initial || initial.timing) return initial;
      const timing = await captureTiming(tx, account, now);
      tx.update(notice, { timing });
      tx.set(
        account.collection('inboxSignals').doc(shard(initial.key)),
        { revision: FieldValue.increment(1) },
        { merge: true }
      );
      return { ...initial, timing };
    });
    if (!event) return;
    const permitted =
      millis(event.deliverUntil) > now &&
      (await this.db.runTransaction(tx => inspectNotice(tx, this.db, uid, event, now), { readOnly: true }));
    if (permitted) {
      const devices = await this.pushStore.active(uid);
      await pooled(devices, async device => {
        const due = deliveryTime(event, device.id);
        await this.enqueue(
          { type: 'deliver', uid, id, deviceId: device.id, revision: device.revision },
          digest(id, device.id),
          Math.max(0, Math.ceil((due - now) / 1000))
        );
      });
    }
    // A fixed ring bounds the retained history using O(1) reads and writes per
    // admitted notice. Transactional admission tolerates duplicate trigger delivery.
    await this.db.runTransaction(async tx => {
      const ring = account.collection('inboxRetention'),
        head = ring.doc('head');
      const [notice, counter] = await tx.getAll(history.doc(id), head);
      if (!notice.exists || notice.data().retained) return;
      const next = counter.data()?.next ?? 0,
        slot = ring.doc(String(next % 1000));
      const old = (await tx.get(slot)).data();
      if (old?.id && old.id !== id) tx.delete(history.doc(old.id));
      tx.set(slot, { id });
      tx.set(head, { next: next + 1 });
      tx.update(notice.ref, { retained: true });
    });
  }
  async deliver({ uid, id, deviceId, revision }) {
    const account = owner(this.db, uid),
      notice = account.collection('inboxNotices').doc(id);
    const attempt = account.collection('inboxDeliveryKeys').doc(digest(id, deviceId));
    const now = this.clock();
    const event = await this.db.runTransaction(async tx => {
      const [e, record, presence] = await tx.getAll(
        notice,
        attempt,
        account.collection('inboxPresence').doc(deviceId)
      );
      if (
        (record.exists && record.data().state !== 'retry') ||
        !(await inspectNotice(tx, this.db, uid, e.data(), now))
      )
        return null;
      const p = presence.data(),
        v = e.data();
      if (deliveryTime(v, deviceId) > now) return { deferredUntil: deliveryTime(v, deviceId) };
      if (p?.foreground && p.key === v.key && p.generation === v.generation && millis(p.expiresAt) > now) return null;
      tx.set(attempt, { state: 'claimed', at: now, expiresAt: new Date(now + 7 * DAY) });
      return v;
    });
    if (!event) return;
    if (event.deferredUntil) {
      await this.enqueue(
        { type: 'deliver', uid, id, deviceId, revision },
        digest('deferred', id, deviceId, event.deferredUntil),
        Math.max(1, Math.ceil((event.deferredUntil - now) / 1000))
      );
      return;
    }
    const device = (await this.pushStore.device(deviceId).get()).data();
    if (
      device?.uid !== uid ||
      device.revision !== revision ||
      !device.notificationsEnabled ||
      millis(device.expiresAt) <= this.clock()
    )
      return;
    const deliveryBadge = await this.db.runTransaction(
      async tx => {
        const live = (await tx.get(account.collection('inboxPresence').doc(deviceId))).data();
        if (
          live?.foreground &&
          live.key === event.key &&
          live.generation === event.generation &&
          millis(live.expiresAt) > this.clock()
        )
          return null;
        if (!(await inspectNotice(tx, this.db, uid, event, this.clock()))) return null;
        const shards = await tx.get(account.collection('inboxBadges').limit(16));
        return Math.min(
          100,
          shards.docs.reduce((sum, d) => sum + Math.max(0, d.data().count ?? 0), 0)
        );
      },
      { readOnly: true }
    );
    if (deliveryBadge === null) return;
    const key = event.key;
    try {
      const message = buildPushMessage(
        device,
        {
          title: 'LingXi',
          body: 'Conversation update',
          route: 'inbox/' + encodeURIComponent(key),
          ttlSeconds: Math.max(1, Math.min(3600, Math.floor((millis(event.deliverUntil) - this.clock()) / 1000))),
          collapseKey: 'inbox:' + digest(key).slice(0, 32),
          data: {
            kind: 'inbox',
            key,
            through: String(event.through),
            generation: String(event.generation),
            eventKind: event.kind,
            ...(safeTarget(event.messageId) ? { messageId: event.messageId } : {}),
            ...(safeTarget(event.taskId) ? { taskId: event.taskId } : {}),
            ...(event.failureRevision !== undefined ? { failureRevision: String(event.failureRevision) } : {}),
            ...(event.pendingActionRevision !== undefined
              ? { pendingActionRevision: String(event.pendingActionRevision) }
              : {}),
            notificationId: id
          }
        },
        this.clock()
      );
      message.apns.payload.aps.badge = deliveryBadge;
      await this.messaging.send(message);
      await attempt.update({ state: 'sent' });
    } catch (error) {
      if (
        ['messaging/server-unavailable', 'messaging/quota-exceeded', 'messaging/message-rate-exceeded'].includes(
          error.code
        )
      ) {
        await attempt.update({ state: 'retry' });
        throw error;
      }
      if (
        ['messaging/registration-token-not-registered', 'messaging/invalid-registration-token'].includes(error.code)
      )
        await this.pushStore.invalidate({ id: deviceId, ...device });
      await attempt.update({ state: 'failed' });
    }
  }
}
