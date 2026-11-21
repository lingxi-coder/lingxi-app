import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { Firestore } from '@google-cloud/firestore';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { GoogleAuth, OAuth2Client } from 'google-auth-library';
import { createDatasetGateway } from '../src/dataset-product.mjs';
import { FriendsStore } from '../src/friends.mjs';
import { createTeamsHandler, PAIR_CONVERSATION_ID, TeamsStore } from '../src/teams.mjs';

const [mode, projectId, tokenFile, core, dataset] = process.argv.slice(2);
if (projectId !== 'lingxi-dataset-v2-stg-261119' || !/^accept-[0-9a-f]{16}$/u.test(dataset) ||
    process.env.FIRESTORE_EMULATOR_HOST || process.env.FIREBASE_AUTH_EMULATOR_HOST) throw new Error('Expected dedicated staging');
let input = '';
for await (const chunk of process.stdin) input += chunk;
const { owner, reader } = JSON.parse(input);
// The staging test uses only the short-lived credential for this project's default database.
const credentials = new OAuth2Client();
credentials.setCredentials({ access_token: (await readFile(tokenFile, 'utf8')).trim() });
const db = new Firestore({ projectId, preferRest: true, auth: new GoogleAuth({ projectId, authClient: credentials }) });
const auth = getAuth(initializeApp({ projectId }));
const handler = createTeamsHandler({ store: new TeamsStore(db), friends: new FriendsStore(db),
  datasets: createDatasetGateway(core).datasets, verifyToken: token => auth.verifyIdToken(token) });
async function send(token, path, body) {
  const reply = await handler(new Request('https://staging.invalid' + path, { method: 'POST',
    headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(body) }));
  const value = await reply.json();
  assert.equal(reply.status, 200, JSON.stringify(value));
  return value;
}
const friend = (actor, body) => send(actor, '/v1/friends', body);
const data = (actor, body) => send(actor, '/v1/teams/datasets', body);
const command = (action, fields, via) => ({ protocol: 2, dataset, via, command: { action, ...fields } });
try {
  const peer = await auth.verifyIdToken(reader);
  if (mode === 'remove') {
    await friend(owner, { action: 'remove', otherUid: peer.uid });
    console.log(JSON.stringify({ removed: true }));
  } else {
    assert.equal(mode, 'share');
    const profile = await friend(owner, { action: 'profile', displayName: 'Staging owner' });
    await friend(reader, { action: 'profile', displayName: 'Staging collaborator' });
    const joined = await friend(reader, { action: 'redeem', code: profile.friendCode.code, requestId: randomUUID() });
    const destination = { team: joined.teamId, conversation: PAIR_CONVERSATION_ID };
    const { audienceDigest } = await data(owner, command('audience', { destination }));
    const common = { action: 'share', dataset, destination, audienceDigest, generation: 1 };
    const pinned = await data(owner, { ...common, operation: 'share-frozen', text: 'Frozen order',
      reference: JSON.parse(input).snapshot });
    const edit = await data(owner, { ...common, operation: 'share-original', mode: 'edit', text: 'Edit quantity',
      projection: { fields: ['qty'], partitions: ['west'] } });
    assert.equal(pinned.status, 'published');
    assert.equal(edit.status, 'published');
    const history = await send(reader, '/v1/teams/messages', { action: 'history', teamId: destination.team,
      conversationId: destination.conversation, generation: 1 });
    for (const publication of [pinned, edit]) {
      const matched = history.messages.filter(message => message.id === publication.messageId);
      assert.equal(matched.length, 1);
      assert.deepEqual(matched[0].resourceRef, publication.reference);
      assert.equal(matched[0].resourceGrant, publication.grant);
    }
    console.log(JSON.stringify({ pinnedGrant: pinned.grant, editGrant: edit.grant, messages: 2 }));
  }
} finally {
  await db.terminate();
}
