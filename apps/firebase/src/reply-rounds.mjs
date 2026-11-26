import { createHash } from 'node:crypto';
import { FieldValue } from 'firebase-admin/firestore';
import { SharedBotError } from './shared-bots.mjs';
import { bidTiers } from './shared-bot-commands.mjs';

/**
 * Several Bots on several Desktops answering one Chat message nobody asked them about (Bot v3, B7). No Desktop can be the referee, so the service is:
 * the message that admits optional tasks for a group or channel also opens a round, `teams/{team}/replyRounds/{messageId}`, with those tasks as its
 * candidates. Each Desktop judges its own Bot (B5) and bids (`bidReply`); the round is settled by the service's clock alone, when every candidate has
 * bid or fifteen seconds have passed. The best bids win, as many as the Chat allows; every other candidate ends `declined` and, since nothing is charged
 * before a run starts, costs nothing.
 *
 * The round is a document of its own that only the service reads (no rule matches the path), so a bid is seen by nobody but the service. What a
 * maintainer may know of their own Bot's bid reaches them through `listTriage` (`round`), and the host learns where its task stands from `listHostTasks`.
 *
 * `open` collects bids. `resolved` has chosen: its winners have `startWindowMs` to start, and once the window has passed one promotion puts the
 * next-ranked bidders in the place of winners that did not start (the round never promotes twice, so a failure cannot chain). `voided` ends it before
 * anyone is chosen: a newer message of the Chat, or the recall of its own message.
 */

/**
 * How long a round collects bids, and how long its winners then have to start. The bids window covers a host's wait for a burst to settle (2.5 s) and its
 * Decision call (`TRIAGE_TIMEOUT_MS`, 10 s), so that a slow Decision service still bids in time; a round whose Bots have all bid settles at once.
 */
export const roundWindowMs = 15000, startWindowMs = 20000;

/** The better the tier, the higher: a function, because the list is read when a round settles, not when this module loads (the two modules import each other through `shared-bots.mjs`). */
const rank = tier => bidTiers.length - bidTiers.indexOf(tier);
/** Equal bids are ordered by this, so every settlement of the same round gives the same answer. */
const order = taskId => createHash('sha256').update(taskId).digest('hex');
const deny = (status, code) => { throw new SharedBotError(status, code); };
const roundRef = (team, roundId) => team.collection('replyRounds').doc(roundId);
const taskRef = (team, taskId) => team.collection('tasks').doc(taskId);
/** The Bot's host is woken to look at its tasks again. */
const wake = (tx, team, sharedBotId, now) => tx.set(team.collection('signals').doc(sharedBotId), { updatedAt: new Date(now) });
/** A task that can still take a seat of its round: waiting, never started, not out of time. */
const standing = (task, round, now) => task?.roundId === round.roundId && task.state === 'queued' && task.epoch === 0 && !task.round && task.deadline > now;
const tasksOf = async (tx, team, rounds) => {
  const refs = rounds.flatMap(round => round.candidates.map(({ taskId }) => taskRef(team, taskId)));
  return new Map((refs.length ? await tx.getAll(...refs) : []).map(doc => [doc.id, doc.data()]));
};

/** The round a message opens: one candidate for each Bot whose task it admitted. `maxReplies` is fixed here, so a later change of the Chat's setting does not move an open round. */
export function openRound(tx, team, { roundId, conversationId, candidates, maxReplies, now }) {
  tx.create(roundRef(team, roundId), { schemaVersion: 1, roundId, conversationId, state: 'open', createdAt: now, deadlineAt: now + roundWindowMs, maxReplies,
    candidates: candidates.map(({ taskId, sharedBotId, addressed }) => ({ taskId, sharedBotId, ...(addressed ? { addressed: true } : {}) })), bids: {} });
}

/** Who bid what, who is out, and the order of the rest: best tier, then the Decision probability, then a fixed order of the task ids. */
function ranking(round, tasks, now) {
  const present = round.candidates.filter(({ taskId }) => standing(tasks.get(taskId), round, now)).map(({ taskId, sharedBotId, addressed }) => {
    const bid = round.bids[taskId];
    // An addressed Bot has the best tier whatever its host bid, but it must still have bid: a Desktop that never answered did not judge.
    return { taskId, sharedBotId, ...(bid ? { tier: addressed ? 'addressed' : bid.tier, score: bid.score ?? 0 } : {}) };
  });
  const ranked = present.filter(bid => bid.tier && bid.tier !== 'decline')
    .sort((a, b) => rank(b.tier) - rank(a.tier) || b.score - a.score || order(a.taskId).localeCompare(order(b.taskId)));
  return { present, ranked };
}
/** What each candidate's task is told. Returns the outcome of every task that was still in the round. */
function resolveRound(tx, team, round, tasks, now) {
  const { present, ranked } = ranking(round, tasks, now);
  const winners = ranked.slice(0, round.maxReplies), outcomes = new Map();
  for (const bid of present) {
    const won = winners.includes(bid), outcome = won ? 'won' : ranked.includes(bid) ? 'lost' : 'abstained';
    outcomes.set(bid.taskId, outcome);
    const seen = { outcome, ...(bid.tier ? { tier: bid.tier } : {}) };
    // A host that judged "no" said so itself; the rest were passed over.
    tx.update(taskRef(team, bid.taskId), won ? { round: seen }
      : { state: 'declined', declineReason: bid.tier === 'decline' ? 'triage' : 'coordination', finishedAt: now, round: seen });
    wake(tx, team, bid.sharedBotId, now);
  }
  tx.update(roundRef(team, round.roundId), { state: 'resolved', resolvedAt: now, startBy: now + startWindowMs,
    winners: winners.map(bid => bid.taskId), standby: ranked.slice(round.maxReplies).map(bid => bid.taskId) });
  return outcomes;
}

/**
 * A Bot's host says how much it wants to answer, after judging its own task (B5), and then waits: it starts only when the round says its task won.
 * The first bid of a task stays whatever is sent again (a restarted Desktop judges and bids again); a bid that comes after the deadline is not counted.
 * Every candidate having bid settles the round in this transaction. Answer: `{ taskId, round }`, where `round` is `open` or what became of the task.
 */
export async function bidReply(store, { tx, team, identity, command: { payload: p }, now }) {
  const { bot } = await store.checkHost(tx, team, identity, p);
  const task = (await tx.get(taskRef(team, p.taskId))).data();
  if (!task || task.deploymentId !== p.deploymentId) deny(404, 'TASK_REQUIRED');
  if (!task.optional) deny(409, 'TASK_NOT_OPTIONAL');
  if (!task.roundId) deny(409, 'ROUND_REQUIRED');
  if (p.tier === 'addressed' && !task.addressed) deny(409, 'BID_TIER_INVALID');
  // Decided already, by this bid or by the round: the same answer however often it is asked.
  if (task.round) return { taskId: p.taskId, round: task.round.outcome };
  if (task.state !== 'queued' || task.epoch !== 0) deny(409, 'TASK_ALREADY_STARTED');
  if (task.deadline <= now) deny(409, 'TASK_EXPIRED');
  if (!store.requesterAuthorized(bot, task)) deny(403, 'TASK_REVOKED');
  const roundDoc = roundRef(team, task.roundId), found = (await tx.get(roundDoc)).data();
  if (!found) deny(404, 'ROUND_REQUIRED');
  if (found.state !== 'open') deny(409, 'ROUND_CLOSED');
  const tasks = await tasksOf(tx, team, [found]);
  let round = found;
  if (now < round.deadlineAt && !round.bids[p.taskId]) {
    const bid = { tier: p.tier, ...(p.score !== undefined ? { score: Math.round(p.score * 1000) / 1000 } : {}), source: p.source, at: now };
    round = { ...round, bids: { ...round.bids, [p.taskId]: bid } };
    tx.update(roundDoc, { bids: round.bids });
  }
  const waiting = round.candidates.filter(({ taskId }) => standing(tasks.get(taskId), round, now));
  if (now < round.deadlineAt && !waiting.every(({ taskId }) => round.bids[taskId])) return { taskId: p.taskId, round: 'open' };
  return { taskId: p.taskId, round: resolveRound(tx, team, round, tasks, now).get(p.taskId) };
}

/** The open rounds of a Chat with their tasks: what a newer message of the Chat ends. Reads only, so it runs before the message is written. */
export async function openRounds(tx, team, c, conversationId) {
  // Only a group or channel with a Bot in it can have one.
  if (!['group', 'channel'].includes(c.kind) || !c.participants.some(id => id.startsWith('shared_bot:'))) return [];
  const rounds = (await tx.get(team.collection('replyRounds').where('conversationId', '==', conversationId).where('state', '==', 'open'))).docs.map(doc => doc.data());
  const tasks = await tasksOf(tx, team, rounds);
  return rounds.map(round => ({ round, tasks }));
}
/** The Bots of a round that had not started when it was voided are told so; what was bid stays on the round. */
function voidRound(tx, team, { round, tasks }, now) {
  tx.update(roundRef(team, round.roundId), { state: 'voided', voidedAt: now });
  endWaiting(tx, team, round, tasks, now);
}
/** Candidates that still wait for a start end `declined` as `superseded`. This is the one place a round's task is ended for something newer than itself. */
function endWaiting(tx, team, round, tasks, now) {
  for (const { taskId, sharedBotId, addressed } of round.candidates) {
    const task = tasks.get(taskId);
    if (task?.roundId !== round.roundId || task.state !== 'queued' || task.epoch !== 0) continue;
    const tier = addressed ? 'addressed' : round.bids[taskId]?.tier ?? task.round?.tier;
    tx.update(taskRef(team, taskId), { state: 'declined', declineReason: 'superseded', finishedAt: now, round: { outcome: 'superseded', ...(tier ? { tier } : {}) } });
    wake(tx, team, sharedBotId, now);
  }
}
export const voidRounds = (tx, team, rounds, now) => { for (const open of rounds) voidRound(tx, team, open, now); };

/**
 * A message is recalled: its round ends, its optional tasks that have not started end, and so do the tasks that asked a Bot (`taskIds`) and still wait.
 * Reads only; `write` runs once the reads of the transaction are done.
 */
export async function recalled(tx, team, messageId, message) {
  const asked = message.taskIds ?? [];
  const [roundDoc, ...askedDocs] = await tx.getAll(roundRef(team, messageId), ...asked.map(id => taskRef(team, id)));
  const round = roundDoc.data(), tasks = round ? await tasksOf(tx, team, [round]) : new Map();
  return { write(now) {
    // A settled round is voided too, so that its winners' timer finds nothing to promote for a message that is gone.
    if (round && round.state !== 'voided') tx.update(roundRef(team, messageId), { state: 'voided', voidedAt: now });
    if (round) endWaiting(tx, team, round, tasks, now);
    for (const doc of askedDocs) {
      const task = doc.data();
      if (task?.state !== 'queued' || task.epoch !== 0) continue;
      tx.update(doc.ref, { state: 'cancelled', finishedAt: now });
      wake(tx, team, task.sharedBotId, now);
    }
  } };
}

/**
 * The job the queue runs when a round's time is up (`phase: 'bids'`: it settles a round nobody finished) or when its winners' time to start is up
 * (`phase: 'start'`: one promotion). Both are safe to run again: a round settles once and promotes once, and a job that comes early is tried again later.
 */
export async function settleReplyRound(db, clock, { teamId, roundId, phase }) {
  const team = db.doc(`teams/${teamId}`);
  return db.runTransaction(async tx => {
    const now = clock(), round = (await tx.get(roundRef(team, roundId))).data();
    if (!round) return { settled: false };
    if (phase === 'bids') {
      if (round.state !== 'open') return { settled: false };
      if (round.deadlineAt > now) throw new Error('REPLY_ROUND_NOT_DUE');
      resolveRound(tx, team, round, await tasksOf(tx, team, [round]), now);
      return { settled: true };
    }
    if (round.state !== 'resolved' || round.promoted) return { settled: false };
    if (round.startBy > now) throw new Error('REPLY_ROUND_NOT_DUE');
    return { settled: true, promoted: await promote(tx, team, round, now) };
  }, { maxAttempts: 20 });
}
/**
 * A winner that has not started when its time is up gives its seat to the next bidder, once: those that did start, and those that went on to ask their
 * maintainer (B6), keep the round, and a promoted Bot is never checked again. A bidder whose Desktop is no longer there is passed over.
 */
async function promote(tx, team, round, now) {
  const tasks = await tasksOf(tx, team, [{ candidates: [...round.winners, ...round.standby].map(taskId => ({ taskId })) }]);
  // Only a winner still waiting to start lapsed: one cancelled, expired or ended for a newer message gives its seat to nobody.
  const lapsed = round.winners.filter(taskId => { const task = tasks.get(taskId); return task?.state === 'queued' && task.epoch === 0 && !task.startedAt; });
  const spare = round.standby.filter(taskId => {
    const task = tasks.get(taskId);
    return task?.roundId === round.roundId && task.state === 'declined' && task.declineReason === 'coordination' && task.deadline > now;
  });
  const hosts = spare.length ? await tx.getAll(...spare.map(taskId => team.collection('deployments').doc(tasks.get(taskId).deploymentId))) : [];
  const chosen = lapsed.length ? spare.filter((_, i) => hosts[i].data()?.enabled && hosts[i].data().ready && hosts[i].data().onlineUntil > now).slice(0, lapsed.length) : [];
  for (const taskId of lapsed) {
    const task = tasks.get(taskId);
    tx.update(taskRef(team, taskId), { state: 'declined', declineReason: 'coordination', finishedAt: now, round: { ...task.round, outcome: 'lost' } });
    wake(tx, team, task.sharedBotId, now);
  }
  for (const taskId of chosen) {
    const task = tasks.get(taskId);
    tx.update(taskRef(team, taskId), { state: 'queued', declineReason: FieldValue.delete(), finishedAt: FieldValue.delete(), round: { ...task.round, outcome: 'promoted' } });
    wake(tx, team, task.sharedBotId, now);
  }
  tx.update(roundRef(team, round.roundId), { promoted: chosen, promotedAt: now });
  return chosen;
}
/** Where a round task stands for its host: `open` until the round has said, then `won` or `promoted`. A task outside any round has none. */
export const roundState = task => task.roundId ? task.round?.outcome ?? 'open' : undefined;
