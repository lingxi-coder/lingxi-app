import { digest } from './inbox-model.mjs';
import { settleReplyRound } from './reply-rounds.mjs';

/** A write of a task that has just become a candidate of a round: it was created with one. */
export const becameCandidate = (before, after) => !before && !!after?.roundId;
/** A write of a task that has just won its round (a promoted one is not timed again: the round promotes once). */
export const becameWinner = (before, after) => after?.round?.outcome === 'won' && before?.round?.outcome !== 'won';

/**
 * The timers of a reply round (`reply-rounds.mjs`), which the transactions that open and settle it cannot set themselves: the trigger of a task calls
 * `candidate` when the round's first task is created, and `winner` when a task has won, and the queue calls `settle` when the time is up. Every job has an id
 * from the round and its phase, so the tasks of one round, a replayed trigger and a retry enqueue it once; a job that finds nothing left to do ends quietly.
 */
export class ReplyRoundJobs {
  constructor({ db, enqueue, clock = Date.now }) { this.db = db; this.enqueue = enqueue; this.clock = clock; }
  /** The round's bids are due at its deadline: a second after, so the queue never hands the job over early. */
  async candidate(teamId, taskId) {
    const roundId = await this.roundOf(teamId, taskId), round = roundId && (await this.db.doc(`teams/${teamId}/replyRounds/${roundId}`).get()).data();
    if (round?.state === 'open') await this.later({ teamId, roundId, phase: 'bids' }, round.deadlineAt);
  }
  /** A winner has `startBy` to start; the job promotes once if it has not. */
  async winner(teamId, taskId) {
    const roundId = await this.roundOf(teamId, taskId), round = roundId && (await this.db.doc(`teams/${teamId}/replyRounds/${roundId}`).get()).data();
    if (round?.state === 'resolved' && !round.promoted) await this.later({ teamId, roundId, phase: 'start' }, round.startBy);
  }
  async roundOf(teamId, taskId) { return (await this.db.doc(`teams/${teamId}/tasks/${taskId}`).get()).data()?.roundId; }
  later(job, at) { return this.enqueue({ type: 'replyRound', ...job }, digest('replyRound', job.teamId, job.roundId, job.phase), Math.max(1, Math.ceil((at - this.clock()) / 1000) + 1)); }
  settle(job) { return settleReplyRound(this.db, this.clock, job); }
}
