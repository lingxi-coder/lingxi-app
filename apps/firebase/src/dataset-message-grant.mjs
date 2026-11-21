import { createHash } from 'node:crypto';
import { LingxiDataError } from './lingxi-data.mjs';

function fail(status, code) { throw new LingxiDataError(status, code); }

// Confirm the audience of the message inside its write transaction. The Data service
// still checks and activates the grant only after this message is durable.
export async function confirmShareDelivery(tx, team, thread, input, uid, share) {
  const ref = share.reference;
  const grantPath = ref.kind === 'pageVersion'
    ? `datasetPages/${ref.page}/versions/${ref.version}`
    : `datasetV2/${ref.dataset}/grants/${share.grant}`;
  const grant = (await tx.get(team.firestore.doc(grantPath))).data();
  const matches = ref.kind === 'pageVersion'
    ? grant?.uid === uid && grant.grant === share.grant && grant.title === input.text
    : grant?.issuer === uid && grant.textDigest === createHash('sha256').update(input.text).digest('base64url');
  if (!matches || !['pending', 'active'].includes(grant.status) ||
      grant.destination.team !== input.teamId || grant.destination.conversation !== input.conversationId ||
      grant.message !== input.messageId) {
    fail(409, 'DATA_SHARE_CHANGED');
  }
  const recipients = Object.keys(grant.audience);
  if (recipients.length !== thread.memberUids.length || !recipients.every(id => thread.memberUids.includes(id))) fail(409, 'DATA_SHARE_CHANGED');
  const members = await tx.getAll(...recipients.map(id => team.collection('members').doc(id)));
  if (members.some(member => !member.exists || member.data().generation !== grant.audience[member.id])) fail(409, 'DATA_SHARE_CHANGED');
}
