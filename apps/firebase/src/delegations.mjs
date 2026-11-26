import { FieldValue } from 'firebase-admin/firestore';
import { SharedBotError, ownerOf, hash, same, botId } from './shared-bots.mjs';
import { sharedBotPermits } from './team-execution-contract.mjs';

/**
 * A Bot handing part of a request to another Bot of the same Chat (Bot v3, B8).
 *
 * Only a tool call of the Bot's run delegates (`TeamDelegate`, answered by `delegateTask`, or by `pauseTask` of kind `delegation` when the run waits for
 * the reply): an @ the Bot writes in its text triggers nothing. Both Bots have to agree: the delegating Bot lists the target in `delegateTo`, the target
 * accepts it (`acceptDelegation`: `team_bots`, or `listed` with `acceptFrom`), and the target is a participant of the Chat. The target runs as the person
 * who asked the first Bot, so their rights to the target are checked as for any message they send, and it uses only its own scope.
 *
 * What a delegation makes: a message of the delegating Bot in the Chat ("@Target ...", a reply to the message that started the run) and an asked task for the
 * target, caused by it (`task.delegation`), in one transaction with whatever else the command writes. `teams/{team}/delegations/{id}` records it. Only the
 * service reads that document: no rule matches the path, so the target's reply, which is kept in it while the delegating run waits, is never visible to a
 * member before the delegating Bot has used it.
 *
 * Limits: a chain is at most `delegationDepth` hops from the person's message (a data handoff is one hop), a Bot that is already on the chain is not asked
 * again, and a root task has at most `delegationsPerRoot` delegations and handoffs in all. Quota is charged to the delegating Bot, as a handoff's is.
 */

/** Whether, and from whom, a Bot accepts delegations. Leaving it out of a publication accepts none. */
export const delegationAccept = ['none', 'team_bots', 'listed'];
/** The most Bots a Bot lists to delegate to, or to accept delegations from. */
export const delegationBots = 20;
/** human -> A -> B -> C: the task of the third Bot is two hops from the person's message, and cannot delegate. */
export const delegationDepth = 2;
/** Delegations and data handoffs that one person's message may start. */
export const delegationsPerRoot = 3;
/** How long a run waits for the reply of the Bot it delegated to; after that it goes on without it, and the reply is posted in the Chat when it comes. */
export const delegationWaitMs = 10 * 60 * 1000;
/** What a delegating run may ask, in bytes. */
export const delegationRequestBytes = 2000;
/** What of the target's reply is handed to the delegating run, in bytes: a continuation carries at most 8 KiB of text, and its Desktop adds a line to the reply. */
export const handBackBytes = 7 * 1024;
/** A task that waits for a Bot's reply stays 24 hours: its Desktop may be off when the reply comes. */
const holdMs = 24 * 60 * 60 * 1000;

const deny = (status, code) => { throw new SharedBotError(status, code); };
const refused = code => ({ ok: false, code });

export const delegationId = (parentTaskId, logicalStepId) => hash(['delegation', parentTaskId, logicalStepId]);
/** Hops from the person's message: a delegated task is one more than its delegating task's, and a task a data handoff made is one. */
export const taskDepth = task => task.delegation?.depth ?? (task.dataCause?.kind === 'handoff' ? 1 : 0);
/** The Bots whose work this task is part of, first the one the person asked. */
export const taskChain = task => task.delegation ? task.delegation.chain.map(link => link.sharedBotId)
  : task.dataCause?.kind === 'handoff' ? [task.dataCause.quotaOwner, task.sharedBotId] : [task.sharedBotId];
const handed = task => Boolean(task.delegation) || task.dataCause?.kind === 'handoff';
/** Nobody asked a Bot that only judges a message it was not spoken to: it does not ask other Bots to answer it. */
const unasked = task => task.optional === true && task.addressed !== true;
export const acceptsFrom = (bot, fromId) => bot.acceptDelegation === 'team_bots' || (bot.acceptDelegation === 'listed' && (bot.acceptFrom ?? []).includes(fromId));
const cut = (text, bytes) => {
  const buffer = Buffer.from(text, 'utf8');
  if (buffer.length <= bytes) return text;
  let end = bytes;
  while (end > 0 && (buffer[end] & 0xc0) === 0x80) end -= 1;
  return buffer.subarray(0, end).toString('utf8');
};

/** How many delegations and handoffs the root task has made so far, finished or not. */
export async function usedByRoot(tx, team, rootTaskId) {
  const family = await tx.get(team.collection('tasks').where('rootTaskId', '==', rootTaskId));
  return family.docs.filter(doc => handed(doc.data())).length;
}

/** Why `target`, a published Bot, cannot take a delegation from `bot`'s task, or null. The rules of both publications and the person's rights; the Chat is `submit`'s. */
function whyNot(store, task, bot, target, deployment) {
  if (!target || target.identity.state !== 'published') return 'DELEGATION_TARGET_UNAVAILABLE';
  if (!acceptsFrom(target, bot.identity.sharedBotId)) return 'DELEGATION_NOT_ACCEPTED';
  if (!sharedBotPermits(target.identity, task.requester) || target.grants.some(grant => !store.grantPermits(grant, task.requester))) return 'BOT_USE_REQUIRED';
  if (!deployment?.enabled || !deployment.capabilities?.delegation) return 'DELEGATION_UNSUPPORTED_TARGET';
  return null;
}

/**
 * The Bots a task's run may be offered to delegate to: what `delegate` would not refuse for a reason that does not change within the run. Reads only.
 * `[]` when the Bot cannot delegate here at all (no list, a Desktop without the tool, a private-data run, a Bot nobody asked, a chain at its end).
 */
export async function delegationOffers(store, { tx, team, task, bot, deployment, conversation }) {
  const chat = conversation.data(), self = bot.identity.sharedBotId, chain = taskChain(task);
  if (!bot.delegateTo?.length || !deployment.capabilities?.delegation || task.privateDataSource || unasked(task) || taskDepth(task) >= delegationDepth) return [];
  const targets = bot.delegateTo.filter(id => id !== self && !chain.includes(id) && chat.participants.includes(`shared_bot:${id}`)
    && (!chat.executionPolicy || chat.executionPolicy.targets.includes(id)));
  if (!targets.length || await usedByRoot(tx, team, task.rootTaskId) >= delegationsPerRoot) return [];
  const bots = await tx.getAll(...targets.map(id => team.collection('sharedBots').doc(id)));
  const deployments = await tx.getAll(...bots.map(doc => team.collection('deployments').doc(doc.data()?.identity.activeDeploymentId ?? '_missing')));
  return bots.flatMap((doc, index) => whyNot(store, task, bot, doc.data(), deployments[index].data()) ? []
    : [{ sharedBotId: doc.id, name: doc.data().name, ...(doc.data().description ? { description: cut(doc.data().description, 300) } : {}) }]);
}

/**
 * The delegation `call` of `task` (a running task of `bot`): the message in the Chat, the target's task, and the record. Idempotent by the logical step.
 * `{ ok: false, code }` is a refusal the delegating Bot is told in words (nothing was written); anything else that is wrong throws as every command does.
 * The caller has read everything it writes and writes only after this returns.
 */
export async function delegate(store, { tx, team, task, bot, deployment, now, call, waitId }) {
  const ref = team.collection('delegations').doc(delegationId(task.taskId, call.logicalStepId));
  const targetRef = team.collection('sharedBots').doc(call.target);
  const [existing, conversation, targetDoc] = await tx.getAll(ref, team.collection('conversations').doc(task.conversationId), targetRef);
  const digest = hash([call.target, call.text, call.expectReply]);
  if (existing.exists) {
    // The same step again answers what it made; a different request under it is a different delegation, which a step cannot be.
    if (existing.data().digest !== digest) deny(409, 'DELEGATION_CONFLICT');
    return { ok: true, replay: true, delegationId: ref.id, taskId: existing.data().targetTaskId, messageId: existing.data().messageId };
  }
  if (task.privateDataSource) return refused('DELEGATION_UNSUPPORTED');
  if (unasked(task)) return refused('DELEGATION_UNASKED');
  if (!bot.delegateTo?.includes(call.target) || call.target === bot.identity.sharedBotId) return refused('DELEGATION_NOT_ALLOWED');
  if (!deployment.capabilities?.delegation) return refused('DELEGATION_UNSUPPORTED');
  const target = targetDoc.data(), depth = taskDepth(task) + 1, chain = taskChain(task);
  const targetDeployment = target ? (await tx.get(team.collection('deployments').doc(target.identity.activeDeploymentId))).data() : undefined;
  const why = whyNot(store, task, bot, target, targetDeployment);
  if (why) return refused(why);
  if (depth > delegationDepth) return refused('DELEGATION_DEPTH');
  if (chain.includes(call.target)) return refused('DELEGATION_LOOP');
  if (await usedByRoot(tx, team, task.rootTaskId) >= delegationsPerRoot) return refused('DELEGATION_LIMIT');
  if (!bot.delegation?.daily) return refused('DELEGATION_NOT_ALLOWED');
  const messageId = botId(['delegation', ref.id]), changeId = botId(['delegation-change', ref.id]);
  const links = [...(task.delegation?.chain ?? chain.map((id, index) => ({ sharedBotId: id, name: index === chain.length - 1 ? bot.name : id }))), { sharedBotId: call.target, name: target.name }];
  let receipt;
  try {
    receipt = await store.submit({ tx, team, identity: task.requester, self: {}, now,
      delegation: { author: { kind: 'shared_bot', sharedBotId: bot.identity.sharedBotId, displayName: bot.name },
        quotaOwner: bot.identity.sharedBotId, dailyLimit: bot.delegation.daily,
        // Kept on the target's task, and shown with it.
        task: { delegationId: ref.id, rootTaskId: task.rootTaskId, parentTaskId: task.taskId, parentBotId: bot.identity.sharedBotId, depth, chain: links, expectReply: call.expectReply } },
      command: { payload: { conversationId: task.conversationId, messageId, changeId, text: `@${target.name} ${call.text}`, targets: [call.target], replyTo: task.messageId } } });
  } catch (error) {
    // `submit` writes nothing before it has checked everything: a refusal of the Chat, the person or the quota leaves the transaction as it was.
    if (error instanceof SharedBotError && error.status >= 400 && error.status < 500) return refused(error.code);
    throw error;
  }
  const targetTaskId = receipt.tasks[0].taskId;
  tx.create(ref, { schemaVersion: 1, delegationId: ref.id, parentTaskId: task.taskId, parentBotId: bot.identity.sharedBotId, rootTaskId: task.rootTaskId,
    targetBotId: call.target, targetTaskId, conversationId: task.conversationId, messageId, expectReply: call.expectReply, digest, depth,
    state: 'open', waitId: call.expectReply ? waitId : null, createdAt: now });
  return { ok: true, delegationId: ref.id, taskId: targetTaskId, messageId };
}

/**
 * The task that waits for a delegation, with what is read to let it go on. All reads, in one round. `waiting` is whether it can still go on: it waits for
 * this delegation, and its Bot is still the publication it was admitted under. A Bot that was published again, moved to another deployment or stopped
 * refuses its earlier tasks at their next step, as for every wait, so the reply is not handed to a run that can never use it: it is posted in the Chat.
 */
export async function waiterOf(tx, team, delegation) {
  const parentRef = team.collection('tasks').doc(delegation.parentTaskId), waitRef = team.collection('waits').doc(delegation.waitId);
  const lockRef = team.collection('chatExecutionLocks').doc(hash([delegation.parentBotId, delegation.conversationId]));
  const [parent, wait, lock, bot] = (await tx.getAll(parentRef, waitRef, lockRef, team.collection('sharedBots').doc(delegation.parentBotId))).map(doc => doc.data());
  const waiting = Boolean(delegation.waitId && parent && wait) && parent.state === 'waiting' && parent.currentWaitId === delegation.waitId
    && wait.kind === 'delegation' && wait.state === 'pending'
    && bot?.identity.state === 'published' && bot.identity.revision === parent.botRevision && bot.identity.activeDeploymentId === parent.deploymentId;
  return { parentRef, parent, waitRef, wait, lockRef, lock, waiting };
}

/** The waiting task goes on: queued again, with how the delegation ended as the outcome of its wait, which `continueTask` hands to its host. */
function release(tx, team, waiter, now, outcome, delegation) {
  const { parentRef, parent, waitRef, lockRef, lock } = waiter, deadline = now + holdMs;
  tx.update(waitRef, { state: 'resolved', resolvedAt: now, delegationOutcome: outcome });
  tx.update(parentRef, { state: 'queued', deadline, resume: { waitId: parent.currentWaitId, delegation: delegation.delegationId } });
  if (!lock || lock.until <= now || lock.taskId === parent.taskId) tx.set(lockRef, { taskId: parent.taskId, until: deadline });
  tx.set(team.collection('signals').doc(parent.sharedBotId), { updatedAt: new Date(now) });
}

/**
 * What `fate` reads, for a finished task that was delegated to and whose delegating task may wait for its reply: the record and its waiter.
 * `open` is whether the reply is to be handed back, rather than posted: the delegating task is still waiting for it.
 */
export async function handBackOf(tx, team, task) {
  if (!task.delegation?.expectReply) return null;
  const ref = team.collection('delegations').doc(task.delegation.delegationId), delegation = (await tx.get(ref)).data();
  if (!delegation || delegation.state !== 'open') return null;
  const waiter = await waiterOf(tx, team, delegation);
  return waiter.waiting ? { ref, delegation, waiter } : null;
}

/**
 * The reply goes to the task that waits for it instead of the Chat. `outcome`: `done` (the text is handed over), `withheld` (the reply may not be shared with
 * this Chat: only the fact is) or `failed` (the target did not finish the request).
 */
export function handBack(tx, team, { ref, delegation, waiter }, now, outcome, text) {
  tx.update(ref, { state: outcome, doneAt: now, ...(outcome === 'done' ? { result: { text: cut(text, handBackBytes) } } : {}) });
  release(tx, team, waiter, now, outcome, delegation);
}

/** The delegating task stops waiting for the reply. Whoever is first, the reply or the end of the time or a cancellation, decides; the others find it decided. */
export async function detach(store, { tx, team, identity, command: { payload: p }, now }) {
  await store.checkHost(tx, team, identity, p);
  const taskRef = team.collection('tasks').doc(p.taskId), waitRef = team.collection('waits').doc(p.waitId);
  const [task, wait] = (await tx.getAll(taskRef, waitRef)).map(doc => doc.data());
  if (!task || task.deploymentId !== p.deploymentId || !wait || wait.taskId !== task.taskId || wait.kind !== 'delegation') deny(404, 'TASK_REQUIRED');
  // A wait that was refused when it was made has no delegation and no time to run out.
  if (!wait.delegationId) return { taskId: task.taskId, state: task.state, outcome: 'refused' };
  const ref = team.collection('delegations').doc(wait.delegationId), delegation = (await tx.get(ref)).data();
  const waiter = await waiterOf(tx, team, delegation);
  if (!waiter.waiting) return { taskId: task.taskId, state: task.state, outcome: delegation.state };
  // The time is the service's: a host asking early is told how long is left.
  if (wait.expiresAt > now) deny(409, 'DELEGATION_WAIT_ACTIVE');
  if (task.deadline <= now) deny(409, 'TASK_EXPIRED');
  tx.update(ref, { state: 'detached', doneAt: now });
  release(tx, team, waiter, now, 'detached', delegation);
  return { taskId: task.taskId, state: 'queued', outcome: 'detached' };
}

/**
 * What a task that was queued again after its wait on a delegation is told: how the delegation ended and, for a reply, the reply. A refusal at the pause
 * is kept on the resume itself, since nothing was made.
 */
export async function waitOutcome(tx, team, task) {
  const { waitId, delegation: id, refusal, target } = task.resume;
  const delegation = refusal ? null : (await tx.get(team.collection('delegations').doc(id))).data();
  const targetBotId = refusal ? target : delegation.targetBotId;
  const named = { targetBotId, targetName: (await tx.get(team.collection('sharedBots').doc(targetBotId))).data()?.name ?? null };
  if (refusal) return { waitId, delegation: { outcome: 'refused', code: refusal, ...named } };
  return { waitId, delegation: { delegationId: id, ...named, outcome: delegation.state, ...(delegation.state === 'done' ? { text: delegation.result.text } : {}) } };
}

/**
 * Cancelling a task. `canCancel`: besides the person who asked and the task's maintainer, a delegated task is cancelled by the maintainer of the Bot that
 * delegated it. `plan` reads what the cancellation touches: the tasks made under it (the whole family is at most four tasks), and, for a delegated task
 * whose delegating task waits for its reply, that task. `apply` writes it, after the caller's own writes: the unfinished tasks made under it are cancelled
 * with it, and the delegating task goes on at once with "cancelled".
 */
export async function canCancel(tx, team, task, bot, identity) {
  if (same(task.requester, identity) || ownerOf(bot, identity)) return true;
  if (!task.delegation) return false;
  return ownerOf((await tx.get(team.collection('sharedBots').doc(task.delegation.parentBotId))).data(), identity);
}
const underTask = (family, taskId) => {
  const under = new Set([taskId]);
  for (let changed = true; changed;) {
    changed = false;
    for (const doc of family) if (!under.has(doc.id) && under.has(doc.data().delegation?.parentTaskId)) { under.add(doc.id); changed = true; }
  }
  under.delete(taskId);
  return family.filter(doc => under.has(doc.id));
}
export async function plan(tx, team, task) {
  const family = (await tx.get(team.collection('tasks').where('rootTaskId', '==', task.rootTaskId))).docs;
  const children = underTask(family, task.taskId).filter(doc => ['queued', 'waiting', 'running', 'awaiting_approval'].includes(doc.data().state));
  const locks = children.length ? await tx.getAll(...children.map(doc => team.collection('chatExecutionLocks').doc(hash([doc.data().sharedBotId, doc.data().conversationId])))) : [];
  const refs = family.filter(doc => doc.data().delegation?.expectReply).map(doc => team.collection('delegations').doc(doc.data().delegation.delegationId));
  const delegations = (refs.length ? await tx.getAll(...refs) : []).filter(doc => doc.exists);
  const own = task.delegation?.expectReply ? delegations.find(doc => doc.id === task.delegation.delegationId)?.data() : undefined;
  const waiter = own?.state === 'open' ? await waiterOf(tx, team, own) : null;
  return { children, locks, delegations, own, waiter: waiter?.waiting ? waiter : null };
}
export function apply(tx, team, task, { children, locks, delegations, own, waiter }, now) {
  for (const [index, doc] of children.entries()) {
    const child = doc.data();
    if (child.state === 'running') { if (!child.cancelRequestedAt) tx.update(doc.ref, { cancelRequestedAt: now }); continue; }
    tx.update(doc.ref, { state: 'cancelled', finishedAt: now, resume: FieldValue.delete() });
    if (child.state === 'waiting') tx.update(team.collection('waits').doc(child.currentWaitId), { state: 'revoked', resolvedAt: now });
    if (child.state === 'awaiting_approval') tx.delete(team.collection('heldReplies').doc(child.taskId));
    if (locks[index].data()?.taskId === child.taskId) tx.delete(locks[index].ref);
    tx.set(team.collection('signals').doc(child.sharedBotId), { updatedAt: new Date(now) });
  }
  // What was delegated by the cancelled task, or by one of the tasks cancelled with it, is not answered any more.
  const gone = new Set([task.taskId, ...children.map(doc => doc.id)]);
  for (const doc of delegations) if (doc.data().state === 'open' && gone.has(doc.data().parentTaskId)) tx.update(doc.ref, { state: 'cancelled', doneAt: now });
  // The task that waits for this one's reply goes on at once.
  if (own && waiter) {
    tx.update(team.collection('delegations').doc(own.delegationId), { state: 'cancelled', doneAt: now });
    release(tx, team, waiter, now, 'cancelled', own);
  }
}
