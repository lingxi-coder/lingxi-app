import { SharedBotError, ownerOf } from './shared-bots.mjs';

/**
 * A Bot's reply that its maintainer wants to read before anyone else does (Bot v3, B6). The Bot runs as it always does; when its result
 * is ready, its host sends `holdReply` instead of `completeTask`, and the service keeps the reply until the maintainer decides.
 *
 * `teams/{team}/heldReplies/{taskId}` is the draft: the reply exactly as the host sent it. It is a document of its own, which only the
 * service reads, so that nothing a member can read, a task or a projection of it, ever carries a word of it. Its task waits in the state
 * `awaiting_approval` and has no more than a deadline: the draft exists exactly while the task is in that state, and every way the task
 * leaves it (approved, rejected, timed out, superseded, canceled) deletes the draft in the same transaction.
 *
 * A held task leaves its state once, whoever is first. Each command reads the task and its draft and writes both, so of an approval, a
 * rejection, the time running out and a newer message of the Chat, the later ones find the task decided and are told how (`closed`).
 */

/** How long a reply waits for its maintainer after it is held. */
export const approvalMs = 10 * 60 * 1000;
/** The most held replies one read of the list carries. A reply lives ten minutes, so this is a bound, not a page. */
const listLimit = 100;

const deny = (status, code) => { throw new SharedBotError(status, code); };

/** What a late approval or rejection is told: how the task left the state it was waiting in. */
const closed = task => task.approval?.decision === 'approved' ? 'REPLY_APPROVED'
  : task.declineReason === 'approval_rejected' ? 'REPLY_REJECTED'
  : task.declineReason === 'approval_timeout' ? 'REPLY_EXPIRED'
  : task.declineReason === 'superseded' ? 'REPLY_SUPERSEDED' : 'REPLY_NOT_HELD';

/**
 * What became of a reply the maintainer was asked to approve, for the auto-reply log: nothing for a task that was never held. The state is the one
 * the task is shown in, which for a reply whose time ran out before anything ended it is already `declined`.
 */
export function approvalOutcome(task, state) {
  if (task.state === 'awaiting_approval') return state === 'awaiting_approval' ? 'held' : 'timeout';
  if (task.approval?.decision === 'approved') return 'approved';
  if (!task.heldAt) return undefined;
  return { approval_rejected: 'rejected', approval_timeout: 'timeout', superseded: 'superseded' }[task.declineReason];
}

/** The drafts a new message of a Chat ends: those of replies nobody asked for. Reads only, so it runs before the message is written. */
export async function supersedable(tx, team, c, conversationId) {
  // Only a group or channel with a Bot in it can hold a reply nobody asked for.
  if (!['group', 'channel'].includes(c.kind) || !c.participants.some(id => id.startsWith('shared_bot:'))) return [];
  return (await tx.get(team.collection('heldReplies').where('conversationId', '==', conversationId))).docs.map(doc => doc.data()).filter(draft => draft.unasked);
}
/**
 * The task of a held reply ends here, and its draft goes with it. Nothing is posted. `held` is the task or its draft, which carries what of the task this needs:
 * a run that left an operation of unknown outcome still has to be checked by a person, whatever became of its reply, as `completeTask` would leave it.
 */
export function endHeld(tx, team, { taskId, sharedBotId, unverifiedOperations, visibility }, now, reason, rejectedBy) {
  const reconcile = unverifiedOperations > 0;
  tx.update(team.collection('tasks').doc(taskId), { state: reconcile ? 'needs_reconciliation' : 'declined', declineReason: reason, finishedAt: now,
    ...(reconcile ? { reconciledState: 'declined', ...(visibility === 'hidden' ? { visibility: 'shown' } : {}) } : {}),
    ...(rejectedBy ? { approval: { decision: 'rejected', by: rejectedBy, at: now } } : {}) });
  tx.delete(team.collection('heldReplies').doc(taskId));
  tx.set(team.collection('signals').doc(sharedBotId), { updatedAt: new Date(now) });
}
export const endSuperseded = (tx, team, drafts, now) => { for (const draft of drafts) endHeld(tx, team, draft, now, 'superseded'); };

/** The task and draft of a reply that still waits, for its Bot's maintainer; anyone else is refused, and so is a decision on a reply that is no longer there to be decided. */
async function load(tx, team, identity, taskId, now) {
  const [taskDoc, draftDoc] = await tx.getAll(team.collection('tasks').doc(taskId), team.collection('heldReplies').doc(taskId));
  const task = taskDoc.data(), draft = draftDoc.data();
  if (!task) deny(404, 'TASK_REQUIRED');
  const bot = (await tx.get(team.collection('sharedBots').doc(task.sharedBotId))).data();
  if (!bot || !ownerOf(bot, identity)) deny(403, 'MAINTAINER_REQUIRED');
  if (task.state !== 'awaiting_approval' || !draft) deny(409, closed(task));
  // The time is up even if the job that ends the task has not run yet.
  if (draft.expiresAt <= now) deny(409, 'REPLY_EXPIRED');
  return { task, bot, draft };
}

/** The replies of one of their Bots that wait for the maintainer, newest first, optionally in one Chat: the draft itself, which no one else is shown. */
export async function listHeldReplies(_store, { tx, team, identity, command: { payload: p }, now }) {
  const bot = (await tx.get(team.collection('sharedBots').doc(p.sharedBotId))).data();
  if (!bot || !ownerOf(bot, identity)) deny(403, 'MAINTAINER_REQUIRED');
  // One field each, so that no composite index is needed: a Chat holds a handful of drafts, which are told apart by Bot here.
  const query = team.collection('heldReplies').where(p.conversationId ? 'conversationId' : 'sharedBotId', '==', p.conversationId ?? p.sharedBotId);
  const drafts = (await tx.get(query.limit(listLimit))).docs.map(doc => doc.data()).filter(draft => draft.sharedBotId === p.sharedBotId && draft.expiresAt > now);
  const chats = drafts.length ? await tx.getAll(...drafts.map(draft => team.collection('conversations').doc(draft.conversationId))) : [];
  // A Chat the maintainer does not read is not theirs to be shown a draft of.
  const replies = drafts.flatMap((draft, i) => {
    const chat = chats[i].data();
    return chat?.memberUids?.includes(identity.uid) ? [{ taskId: draft.taskId, conversationId: draft.conversationId, messageId: draft.messageId, requester: draft.requester,
      unasked: draft.unasked, heldAt: draft.heldAt, expiresAt: draft.expiresAt, text: draft.result.text, ...(draft.result.document ? { document: draft.result.document } : {}),
      ...(chat.title ? { conversationTitle: chat.title } : {}) }] : [];
  });
  return { replies: replies.sort((a, b) => b.heldAt - a.heldAt || a.taskId.localeCompare(b.taskId)) };
}

/**
 * The maintainer lets the reply go, as it is or with their own text; the document the run made stays. It is posted by the code `completeTask` posts a
 * result with, as if the host had just finished, so it works while the host is off. A reply that may no longer be posted (the message was edited, the Bot
 * was republished, its requester left) ends as `completeTask` would end it, or as `declined` when the task itself is no longer valid.
 */
export async function approveReply(store, { tx, team, identity, command: { payload: p }, now }) {
  const { task, bot, draft } = await load(tx, team, identity, p.taskId, now);
  const edited = p.text !== undefined && p.text !== draft.result.text;
  const result = { ...draft.result, taskId: task.taskId, ...(edited ? { text: p.text } : {}) };
  let gate = null;
  if (store.requesterAuthorized(bot, task)) {
    try { gate = await store.outputGate(tx, team, task, bot, result); }
    catch (error) { if (!(error instanceof SharedBotError && error.code === 'TASK_REVOKED')) throw error; }
  }
  if (!gate) {
    endHeld(tx, team, task, now, 'revoked');
    return { taskId: task.taskId, state: 'declined', declineReason: 'revoked', replyId: null, artifactId: null };
  }
  const fate = await store.fate(tx, team, task, gate, result);
  const response = await store.post(tx, team, task, bot, identity, now, gate, fate, result,
    { resultDigest: task.resultDigest, approval: { decision: 'approved', by: identity, at: now, ...(edited ? { edited: true } : {}) } });
  tx.delete(team.collection('heldReplies').doc(task.taskId));
  tx.set(team.collection('signals').doc(task.sharedBotId), { updatedAt: new Date(now) });
  return response;
}

/** The maintainer does not let the reply go. Nothing is posted and nobody but them has read it. */
export async function rejectReply(_store, { tx, team, identity, command: { payload: p }, now }) {
  const { task } = await load(tx, team, identity, p.taskId, now);
  endHeld(tx, team, task, now, 'approval_rejected', identity);
  return { taskId: task.taskId, state: 'declined', declineReason: 'approval_rejected' };
}

/** The job that runs when a reply's time is up: it ends the task if no one has decided it. The queue calls it at the deadline, so it never finds the reply early but for a clock apart. */
export async function expireHeldReply(db, clock, { teamId, taskId }) {
  const team = db.doc(`teams/${teamId}`);
  return db.runTransaction(async tx => {
    const now = clock();
    const [taskDoc, draftDoc] = await tx.getAll(team.collection('tasks').doc(taskId), team.collection('heldReplies').doc(taskId));
    const task = taskDoc.data(), draft = draftDoc.data();
    if (task?.state !== 'awaiting_approval' || !draft) return { ended: false };
    // Cloud Tasks tries again a little later.
    if (draft.expiresAt > now) throw new Error('HELD_REPLY_NOT_DUE');
    endHeld(tx, team, task, now, 'approval_timeout');
    return { ended: true };
  }, { maxAttempts: 20 });
}
