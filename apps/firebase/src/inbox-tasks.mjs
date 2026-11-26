import { SharedBotsStore } from './shared-bots.mjs';
import { capped, digest, emptyRow, makeKey, millis } from './inbox-model.mjs';
import { access, addNotice, captureTiming, owner, writeRow } from './inbox-store.mjs';

export async function taskAllowed(tx, db, acl, task, uid) {
  if (!task?.maintainer?.uid || task.requester?.uid !== uid || task.requester.generation !== acl.generation)
    return false;
  if (task.optional && acl.chat.kind !== 'bot_dm' && !task.replyId) return false;
  const [bot, maintainer] = await tx.getAll(
    acl.root.collection('sharedBots').doc(task.sharedBotId),
    acl.root.collection('members').doc(task.maintainer.uid)
  );
  const ledger = new SharedBotsStore(db);
  if (
    !bot.exists ||
    maintainer.data()?.generation !== task.maintainer.generation ||
    !ledger.requesterAuthorized(bot.data(), task)
  )
    return false;
  try {
    await ledger.validateTask(tx, acl.root, task);
    return true;
  } catch (e) {
    if (e.status === 403) return false;
    throw e;
  }
}
const failures = new Set(['failed', 'authorization_required', 'needs_reconciliation', 'interrupted']);
export async function projectTask(store, { uid, ref, taskId }, enqueue) {
  const { db } = store,
    now = store.clock(),
    account = owner(db, uid),
    doc = account.collection('inbox').doc(makeKey(ref));
  const expires = await db.runTransaction(async tx => {
    const [acl, beforeDoc] = await Promise.all([access(tx, db, uid, ref), tx.get(doc)]);
    if (!acl || acl.generation !== ref.generation) return null;
    const task = (await tx.get(acl.root.collection('tasks').doc(taskId))).data();
    const marks = doc.collection('epochs').doc(String(acl.generation)).collection('inboxTaskMarks');
    const marker = marks.doc(taskId),
      previous = (await tx.get(marker)).data();
    const live = await taskAllowed(tx, db, acl, task, uid);
    const wait =
      // A task that waits for another Bot's reply waits for no person: nothing is asked of anyone.
      live && task.state === 'waiting' && task.deadline > now && task.currentWaitKind !== 'delegation'
        ? (await tx.get(acl.root.collection('waits').doc(task.currentWaitId))).data()
        : null;
    let kind;
    if (wait?.state === 'pending') kind = wait.kind === 'tool_approval' ? 'approval' : 'input';
    else if (live && failures.has(task.state)) kind = 'failure';
    else if (live && task.state === 'completed' && task.replyId) kind = 'completion';
    const revision = kind === 'approval' || kind === 'input' ? task.currentWaitId : (kind ?? 'closed');
    if (previous?.revision === revision || (!kind && !previous))
      return wait?.state === 'pending' ? task.deadline : null;
    const before = beforeDoc.data(),
      row = {
        ...(before?.ref.generation === acl.generation ? before : emptyRow({ ...ref, generation: acl.generation }))
      };
    const [pending, failed] = await Promise.all([
      tx.get(marks.where('pendingUntil', '>', now).limit(101)),
      tx.get(marks.where('kind', '==', 'failure').limit(2))
    ]);
    const other = pending.docs.filter(d => d.id !== taskId),
      approval = kind === 'approval' || other.some(d => d.data().kind === 'approval');
    row.pendingActionCount = capped(other.length + Number(wait?.state === 'pending'));
    if (['approval', 'input'].includes(kind) || ['approval', 'input'].includes(previous?.kind)) {
      row.pendingActionRevision = (row.pendingActionRevision ?? 0) + 1;
      row.pendingActionAt = millis(wait?.createdAt ?? now);
    }
    const hasFailure = kind === 'failure' || failed.docs.some(d => d.id !== taskId);
    row.state = row.pendingActionCount
      ? approval
        ? 'waiting_for_approval'
        : 'waiting_for_input'
      : hasFailure
        ? 'failed'
        : 'idle';
    delete row.attentionKind;
    if (hasFailure) row.attentionKind = 'failure';
    const mid = wait?.messageId ?? task?.replyId;
    const m = mid ? (await tx.get(acl.thread.collection('messages').doc(mid))).data() : null;
    const cursor = m?.seq ?? task?.messageSeq ?? row.sourceCursor;
    if (kind && (wait || cursor > row.readThrough)) {
      const at = wait?.createdAt ?? task.deliveredAt ?? task.finishedAt ?? task.createdAt;
      addNotice(
        tx,
        account,
        row,
        {
          kind,
          timing: await captureTiming(tx, account, now),
          cursor,
          taskId,
          ...(mid ? { messageId: mid } : {}),
          waitId: task.currentWaitId ?? null,
          id: 'task:' + taskId + ':' + revision,
          at: millis(at)
        },
        now
      );
    }
    if (kind) tx.set(marker, { revision, kind, pendingUntil: wait ? task.deadline : 0 });
    else tx.delete(marker);
    writeRow(tx, account, doc, before, row, now);
    return wait ? task.deadline : null;
  });
  if (expires)
    await enqueue(
      { type: 'task', uid, ref, taskId },
      digest('expiry', uid, ref, taskId, expires),
      Math.max(1, Math.ceil((expires - now) / 1000))
    );
}
