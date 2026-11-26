import { createHash } from 'node:crypto';
import { sendTeamPush } from './push.mjs';
import { digest } from './inbox-model.mjs';
import { expireHeldReply } from './held-replies.mjs';

const hash = value => createHash('sha256').update(value).digest('hex');
/** Generic wording only, like every Team push: what the reply says is read in the app, by the person who maintains the Bot. */
export const HELD_REPLY_COPY = { zh: '有一条回复等待你批准', en: 'A reply is waiting for your approval' };

/** A write of a task that moved it into `awaiting_approval`: the reply's time and the maintainer's push start on this write and no other. */
export const becameHeld = (before, after) => after?.state === 'awaiting_approval' && before?.state !== 'awaiting_approval';

/**
 * What happens around a reply held for its maintainer, outside the ledger transaction that holds it (`held-replies.mjs`): the trigger of the task calls
 * `held` once the task is `awaiting_approval`, and the queue calls `expire` when the reply's time is up. Both are safe to run again: the job has an id
 * from the reply and its deadline, and the push is claimed once per reply and person before it is sent.
 */
export class HeldReplyJobs {
  constructor({ db, pushStore, messaging, enqueue, clock = Date.now }) {
    this.db = db; this.pushStore = pushStore; this.messaging = messaging; this.enqueue = enqueue; this.clock = clock;
  }
  /** A task has just gone to `awaiting_approval`: set the end of its time, and tell the Bot's maintainer. */
  async held(teamId, taskId) {
    const team = this.db.doc(`teams/${teamId}`);
    const [taskDoc, draftDoc] = await this.db.getAll(team.collection('tasks').doc(taskId), team.collection('heldReplies').doc(taskId));
    const draft = draftDoc.data();
    // Decided before this ran: there is nothing to end and nobody to tell.
    if (taskDoc.data()?.state !== 'awaiting_approval' || !draft) return;
    // A second past the deadline, so the queue never hands it over early.
    await this.enqueue({ type: 'heldReply', teamId, taskId }, digest('heldReply', teamId, taskId, draft.expiresAt),
      Math.max(1, Math.ceil((draft.expiresAt - this.clock()) / 1000) + 1));
    await this.notify(teamId, taskId, draft);
  }
  /** The queue's job at the deadline: the reply is declined unless someone decided it. */
  expire(job) { return expireHeldReply(this.db, this.clock, job); }
  /** Push is a wake-up, never the record: a durable attempt is claimed before FCM, as for a message, and an ambiguous send is not replayed. */
  async notify(teamId, taskId, draft) {
    const { db, clock } = this, team = db.doc(`teams/${teamId}`), thread = team.collection('conversations').doc(draft.conversationId), uid = draft.maintainer.uid;
    const attempt = thread.collection('pushAttempts').doc(hash(JSON.stringify(['heldReply', taskId, uid])));
    const value = await db.runTransaction(async tx => {
      const [t, conversation, member, bot, prior] = await tx.getAll(team, thread, team.collection('members').doc(uid), team.collection('sharedBots').doc(draft.sharedBotId), attempt);
      if (prior.exists || t.data()?.state !== 'active' || member.data()?.generation !== draft.maintainer.generation
        || !conversation.data()?.memberUids.includes(uid)) return null;
      tx.create(attempt, { state: 'attempted', generation: member.data().generation, at: new Date(clock()) });
      const name = bot.data()?.name;
      return { title: typeof name === 'string' && name.trim() ? name.trim().slice(0, 80) : 'LingXi', generation: member.data().generation };
    });
    if (!value) return;
    const result = await sendTeamPush(this.pushStore, this.messaging, { uid, title: value.title, notificationId: hash(JSON.stringify([teamId, taskId, uid, 'reply_approval'])),
      collapseKey: `approval:${hash(JSON.stringify([teamId, draft.conversationId])).slice(0, 32)}`,
      route: `team/chat?conversation=${draft.conversationId}&team=${teamId}`, kind: 'reply_approval', copy: HELD_REPLY_COPY }, async () => {
      const [t, member, conversation] = await Promise.all([team.get(), team.collection('members').doc(uid).get(), thread.get()]);
      return t.data()?.state === 'active' && member.data()?.generation === value.generation && conversation.data()?.memberUids.includes(uid);
    }, clock);
    await attempt.update({ state: 'finished', ...result, finishedAt: new Date(clock()) });
  }
}
