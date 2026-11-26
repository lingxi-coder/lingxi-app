import { runReadBatch } from './inbox-batch.mjs';
import { projectTask } from './inbox-tasks.mjs';
import { owner, pooled } from './inbox-store.mjs';
import { digest } from './inbox-model.mjs';

export class InboxJobs {
  constructor(store, delivery, enqueue, held) {
    this.store = store;
    this.delivery = delivery;
    this.enqueue = enqueue;
    this.held = held;
  }
  async wake(ref) {
    const now = this.store.clock(),
      bucket = Math.floor(now / 1000);
    return this.enqueue({ type: 'fold', ref }, digest('fold', ref, bucket), 1);
  }
  async conversation(ref, before, after) {
    const removed = (before?.memberUids ?? []).filter(uid => !after?.memberUids?.includes(uid));
    await pooled(removed, uid => this.store.fold(uid, ref));
    await this.wake(ref);
  }
  async bootstrap(uid) {
    const doc = owner(this.store.db, uid).collection('inboxJobs').doc('bootstrap');
    try {
      await doc.create({ state: 'queued' });
    } catch (e) {
      if (e.code !== 6 && e.code !== 'already-exists') throw e;
    }
  }
  async run(p) {
    if (p.type === 'readBatch') return runReadBatch(this.store, p.uid, p.id, this.enqueue);
    if (p.type === 'reconcile') return this.store.reconcileOwner(p.uid, p.kind, p.sourceId, p.after);
    if (p.type === 'scanMembers' || p.type === 'scanConversations') {
      const root = this.store.db.collection(p.kind === 'service' ? 'services' : 'teams').doc(p.sourceId);
      const members = p.type === 'scanMembers';
      let query = members
        ? root.collection('members')
        : root.collection('conversations').where('memberUids', 'array-contains', p.uid);
      query = query.orderBy('__name__');
      if (p.after) query = query.startAfter(p.after);
      const page = await query.limit(32).select().get();
      await pooled(page.docs, d =>
        members
          ? this.enqueue({ type: 'reconcile', kind: p.kind, sourceId: p.sourceId, uid: d.id })
          : this.wake({ kind: p.kind, sourceId: p.sourceId, conversationId: d.id })
      );
      if (page.size === 32) await this.enqueue({ ...p, after: page.docs.at(-1).id });
      return;
    }
    if (p.type === 'task') return projectTask(this.store, p, this.enqueue);
    // The time of a reply held for its Bot's maintainer is up (`held-reply-jobs.mjs`).
    if (p.type === 'heldReply') return this.held.expire(p);
    if (p.type === 'deliver') return this.delivery.deliver(p);
    if (p.type === 'fold') {
      if (await this.store.fanout(p.ref)) await this.enqueue(p);
      return;
    }
    if (p.type === 'bootstrap') {
      const db = this.store.db,
        job = owner(db, p.uid).collection('inboxJobs').doc('bootstrap');
      const check = (await job.get()).data();
      if (!check || check.state === 'done') return;
      const tasks = check.phase === 'tasks';
      let q = tasks
        ? db.collectionGroup('tasks').where('requester.uid', '==', p.uid)
        : db.collectionGroup('conversations').where('memberUids', 'array-contains', p.uid);
      q = q.orderBy('__name__');
      if (check.after) q = q.startAfter(db.doc(check.after));
      const page = await q.limit(16).get();
      await pooled(page.docs, async d => {
        const [scope, sourceId, , id] = d.ref.path.split('/');
        if (!['teams', 'services'].includes(scope)) return;
        const kind = scope === 'teams' ? 'team' : 'service';
        if (tasks) {
          const task = d.data();
          if (
            task.state === 'waiting' ||
            ['failed', 'authorization_required', 'needs_reconciliation', 'interrupted'].includes(task.state)
          )
            await projectTask(
              this.store,
              {
                uid: p.uid,
                ref: { kind, sourceId, conversationId: task.conversationId, generation: task.requester.generation },
                taskId: id
              },
              this.enqueue
            );
        } else {
          const ref = { kind, sourceId, conversationId: id };
          if (await this.store.fold(p.uid, ref)) await this.wake(ref);
        }
      });
      const more = page.size === 16;
      await db.runTransaction(async tx => {
        const current = (await tx.get(job)).data();
        if (current.state === 'done' || current.after !== check.after || current.phase !== check.phase) return;
        tx.set(
          job,
          more
            ? { state: 'queued', phase: tasks ? 'tasks' : 'conversations', after: page.docs.at(-1).ref.path }
            : tasks
              ? { state: 'done' }
              : { state: 'queued', phase: 'tasks' }
        );
      });
      if (more || !tasks) await this.enqueue(p);
    }
  }
}
