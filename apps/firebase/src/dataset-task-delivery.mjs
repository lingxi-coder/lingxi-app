import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { isResourceRef } from './dataset-resource.mjs';
import { messagePreview } from './team-message-preview.mjs';

const hash = value => createHash('sha256').update(value).digest('base64url');

// The Core creates this outbox entry from an approved run. Only the product writer
// allocates Chat sequence numbers. Retrying either side keeps the same message ID.
export async function deliverDatasetTaskOutput(db, deliveryId, clock = Date.now) {
  const ref = db.collection('datasetV2Deliveries').doc(deliveryId);
  return db.runTransaction(async tx => {
    const delivery = (await tx.get(ref)).data();
    if (!delivery || delivery.state !== 'pending') return delivery?.state ?? 'missing';
    const { dataset, task, reference, destination, messageId, issuer } = delivery;
    const datasetRef = db.collection('datasetV2').doc(dataset);
    const team = db.collection('teams').doc(destination.team);
    const thread = team.collection('conversations').doc(destination.conversation);
    const message = thread.collection('messages').doc(messageId);
    const change = thread.collection('changes').doc(messageId);
    const [meta, grant, permit, taskHead, teamDoc, chat, old, oldChange] =
      (await tx.getAll(datasetRef, datasetRef.collection('grants').doc(deliveryId), db.collection('datasetV2Permits').doc(task.permit),
        db.collection('datasetV2Tasks').doc(task.taskKey), team, thread, message, change)).map(d => d.data());
    function reject() {
      tx.update(ref, { state: 'approvalChanged' });
      return 'approvalChanged';
    }
    if (meta?.deleted !== false || !grant || grant.status === 'revoked' || !isDeepStrictEqual(grant.task, task) ||
        grant.issuer !== issuer || grant.message !== messageId || grant.textDigest !== hash(delivery.text) ||
        !isResourceRef(reference) || !isDeepStrictEqual(grant.reference, reference) || !isDeepStrictEqual(grant.destination, destination) ||
        permit?.revoked !== false || permit.uid !== issuer || permit.dataset !== dataset || permit.digest !== delivery.planDigest ||
        permit.plan.expires <= Math.floor(clock() / 1000) || taskHead?.deleted !== false || taskHead.permit !== task.permit ||
        teamDoc?.state !== 'active' || !chat?.participants?.includes('user:' + issuer)) return reject();
    const recipients = Object.keys(grant.audience);
    if (chat.memberUids?.length !== recipients.length || recipients.some(uid => !chat.memberUids.includes(uid))) return reject();
    const members = await tx.getAll(...recipients.map(uid => team.collection('members').doc(uid)));
    if (members.some(m => m.data()?.generation !== grant.audience[m.id])) return reject();
    if (meta.owner.kind === 'team') {
      const ownerTeam = db.collection('teams').doc(meta.owner.id);
      const [t, member] = await tx.getAll(ownerTeam, ownerTeam.collection('members').doc(issuer));
      if (t.data()?.state !== 'active' || !['owner', 'admin'].includes(member.data()?.role)) return reject();
    } else if (meta.owner.kind !== 'user' || meta.owner.id !== issuer) return reject();
    if (old || oldChange) {
      if (!old || !oldChange || old.recalled || old.resourceGrant !== deliveryId || oldChange.messageId !== messageId) return reject();
    } else {
      const seq = chat.lastSeq + 1, changeSeq = chat.lastChangeSeq + 1;
      if (!Number.isSafeInteger(seq) || !Number.isSafeInteger(changeSeq)) throw new Error('INVALID_CHAT_SEQUENCE');
      const now = new Date(clock());
      const author = { kind: 'task', taskId: task.task, logicalRun: task.logicalRun, displayName: task.task };
      tx.create(message, { seq, changeSeq, changeId: messageId, author, kind: 'text', text: delivery.text, createdAt: now,
        resourceRef: reference, resourceGrant: deliveryId });
      tx.create(change, { seq: changeSeq, operation: 'create', messageId, actorUid: issuer, actorKind: 'task',
        generation: grant.audience[issuer], textDigest: createHash('sha256').update(delivery.text).digest('hex'), at: now });
      tx.update(thread, { lastSeq: seq, lastChangeSeq: changeSeq, lastChange: { id: messageId, seq: changeSeq },
        lastMessage: { id: messageId, seq, author, at: now, preview: messagePreview(delivery.text) } });
    }
    tx.update(ref, { state: 'delivered', deliveredAt: new Date(clock()) });
    return 'delivered';
  });
}
