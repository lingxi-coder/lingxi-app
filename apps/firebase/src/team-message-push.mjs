import { createHash } from 'node:crypto';
import { sendTeamPush } from './push.mjs';
const hash = value => createHash('sha256').update(value).digest('hex');
const millis = value => value?.toMillis?.() ?? 0;
const segment = value => typeof value === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(value);

/** Push is a best-effort wake-up, never the message delivery log. A durable
 * per-recipient attempt is claimed before FCM; an ambiguous crash is not blindly
 * replayed. Reconnect always recovers messages through the immutable change log. */
export async function onTeamMessageCreated({ db, pushStore, messaging, teamId, conversationId, messageId, original, clock = Date.now }) {
  if (![teamId, conversationId, messageId].every(segment) || !original?.author?.uid || !segment(original.changeId)) return;
  const team = db.doc(`teams/${teamId}`), thread = team.collection('conversations').doc(conversationId);
  const c = (await thread.get()).data();
  if (!c || !Array.isArray(c.memberUids)) return;
  for (const uid of c.memberUids.filter(uid => uid !== original.author.uid)) {
    const attempt = thread.collection('pushAttempts').doc(hash(JSON.stringify([messageId, uid])));
    const value = await db.runTransaction(async tx => {
      const [t, conversation, live, receipt, sender, recipient, prior] = await tx.getAll(team, thread,
        thread.collection('messages').doc(messageId), thread.collection('changes').doc(original.changeId),
        team.collection('members').doc(original.author.uid), team.collection('members').doc(uid), attempt);
      const m = live.data(), change = receipt.data();
      if (prior.exists || t.data()?.state !== 'active' || !sender.exists || !recipient.exists || !m || m.recalled
        || !conversation.data()?.memberUids.includes(uid) || !conversation.data().memberUids.includes(original.author.uid)
        || change?.operation !== 'create' || change.messageId !== messageId || change.actorUid !== original.author.uid
        || change.generation !== sender.data().generation || m.author.uid !== original.author.uid
        || millis(original.createdAt) < millis(recipient.data().joinedAt) || clock() - millis(original.createdAt) > 3600000) return null;
      let title = sender.data().displayName;
      if (original.author.kind === 'bot') {
        const bot = (await tx.get(team.collection('bots').doc(original.author.botKey))).data();
        if (!bot || bot.ownerUid !== original.author.uid || !conversation.data().participants.includes(original.author.botKey)) return null;
        title = bot.name;
      } else if (!conversation.data().participants.includes(`user:${original.author.uid}`)) return null;
      tx.create(attempt, { state: 'attempted', generation: recipient.data().generation, at: new Date(clock()) });
      return { title: typeof title === 'string' && title.trim() ? title.trim().slice(0, 80) : 'LingXi', generation: recipient.data().generation };
    });
    if (!value) continue;
    const result = await sendTeamPush(pushStore, messaging, { uid, title: value.title, notificationId: hash(JSON.stringify([teamId, conversationId, messageId, uid])),
      collapseKey: `team:${hash(JSON.stringify([teamId, conversationId])).slice(0, 32)}` }, async () => {
      const [t, member] = await Promise.all([team.get(), team.collection('members').doc(uid).get()]);
      return t.data()?.state === 'active' && member.data()?.generation === value.generation;
    }, clock);
    await attempt.update({ state: 'finished', ...result, finishedAt: new Date(clock()) });
  }
}
