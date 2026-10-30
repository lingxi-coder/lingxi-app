// Explicit demo-project acceptance only. Never uses saved/production accounts.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { realpathSync, statfsSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { triggerIdentity, botOutputIdentity } from '../src/team-bots.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
for (const name of ['FIRESTORE_EMULATOR_HOST', 'FIREBASE_AUTH_EMULATOR_HOST']) {
  assert.match(process.env[name] ?? '', /^127\.0\.0\.1:\d+$/u, `${name} must be explicit loopback`);
}
const space = statfsSync(root);
assert.ok(space.bavail * space.bsize >= 2 * 1024 ** 3, '2 GiB test floor');
const binary = realpathSync(process.env.TEAM_CONNECTOR_TEST_BINARY ?? '');
const allowed = realpathSync(resolve(root, '../harness-runtime/target/debug/deps')) + sep;
assert.ok(binary.startsWith(allowed));
assert.match(binary.slice(allowed.length), /^team_connector-[a-f0-9]+$/u);
const projectId = 'demo-lingxi-teams-t0';
assert.equal((await fetch(`http://${process.env.FIRESTORE_EMULATOR_HOST}/emulator/v1/projects/${projectId}/databases/(default)/documents`, { method: 'DELETE' })).status, 200);
const app = initializeApp({ projectId }, 'teams-connector-emulator');
const auth = getAuth(app), db = getFirestore(app);
const owner = { uid: 'fixture-owner', name: 'Fixture owner' }, human = { uid: 'fixture-human', name: 'Fixture human' };
const teamId = 'fixture-team', conversationId = 'fixture-conversation', hostId = 'h'.repeat(22), botId = 'b'.repeat(22);
const botKey = `bot:${owner.uid}:${hostId}:${botId}`, scope = { teamId, generation: 1 }, thread = { ...scope, conversationId };
const teams = new TeamsStore(db), messages = new TeamMessagesStore(db), actions = [];
const handler = createTeamsHandler({ store: teams, verifyToken: token => auth.verifyIdToken(token, true), createCustomToken: uid => auth.createCustomToken(uid) });
const server = createServer(async (request, response) => {
  try {
    let raw = ''; for await (const chunk of request) { raw += chunk; assert.ok(Buffer.byteLength(raw) <= 65536); }
    const command = JSON.parse(raw || '{}');
    const input = new Request(`http://127.0.0.1${request.url}`, { method: request.method, headers: request.headers, body: raw });
    const result = await handler(input);
    actions.push({ action: command.action, status: result.status });
    response.writeHead(result.status, Object.fromEntries(result.headers)); response.end(await result.text());
  } catch { response.writeHead(500); response.end('{}'); }
});
let child;
try {
  for (const user of [owner, human]) await auth.createUser({ uid: user.uid, displayName: user.name });
  const token = async uid => {
    const credential = await auth.createCustomToken(uid);
    const response = await fetch(`http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}/identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key=synthetic-emulator-key`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token: credential, returnSecureToken: true }),
    });
    assert.equal(response.status, 200); return (await response.json()).idToken;
  };
  const ownerToken = await token(owner.uid), humanToken = await token(human.uid);
  await teams.execute(owner, { action: 'create', teamId, name: 'Connector fixture team' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'connector-invite', role: 'member', maxUses: 1, ttlSeconds: 3600 });
  await teams.execute(human, { action: 'join', code, requestId: 'connector-join' });
  await messages.execute(owner, { action: 'bindBot', ...scope, hostId, botId, expectedRevision: 0, name: 'Connector fixture Bot', description: '', use: 'all', uids: [] });
  await messages.execute(human, { action: 'create', ...thread, kind: 'bot_dm', title: 'Connector fixture', participantUids: [human.uid], botKeys: [botKey] });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const endpoint = `http://127.0.0.1:${server.address().port}/v1/teams/messages`;
  const post = (credential, body) => fetch(endpoint, { method: 'POST', headers: { Authorization: `Bearer ${credential}`, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const triggerId = triggerIdentity('human-message01', botKey);
  // Exercise the real authenticated HTTP create path, not Admin-forged evidence.
  assert.equal((await post(humanToken, { action: 'mutate', ...thread, operation: 'create', messageId: 'human-message01', changeId: 'human-change01', text: 'Only the authenticated original human input.' })).status, 200);
  const run = { ...thread, botKey, triggerId, attemptId: 'forged-attempt' };
  assert.equal((await post('invalid-synthetic-token', { action: 'claimBot', ...run })).status, 401);
  assert.equal((await post(humanToken, { action: 'claimBot', ...run })).status, 403);
  assert.equal((await post(ownerToken, { action: 'startBot', ...run, execute: true, execution: { origin: { uid: owner.uid, generation: 1 } } })).status, 400);
  child = spawn(binary, ['--ignored', '--exact', 'actual_firebase_backend_and_worker', '--test-threads=1', '--nocapture'], {
    cwd: root, env: { ...process.env, TEAM_CONNECTOR_BACKEND: endpoint, TEAM_CONNECTOR_OWNER_TOKEN: ownerToken }, stdio: 'inherit',
  });
  const [exit, signal] = await once(child, 'exit', { signal: AbortSignal.timeout(90000) }); assert.equal(signal, null); assert.equal(exit, 0);
  const ref = db.doc(`teams/${teamId}/conversations/${conversationId}`);
  const runDoc = (await ref.collection('botRuns').doc(triggerId).get()).data();
  assert.equal(runDoc.state, 'completed');
  assert.equal(runDoc.replyId, botOutputIdentity(triggerId, 'reply'));
  assert.equal((await ref.collection('messages').get()).size, 3);
  assert.equal((await ref.collection('triggers').get()).size, 1);
  assert.equal((await ref.collection('botCursors').get()).docs[0].data().seq, 1);
  const accepted = action => actions.filter(value => value.action === action && value.status === 200).length;
  assert.equal(accepted('claimBot'), 1); assert.equal(accepted('startBot'), 1); assert.equal(accepted('finishBot'), 2);
  assert.ok(accepted('botPoll') >= 1);
  console.log(JSON.stringify({ fixture: 'Teams connector real Auth/Firestore/HTTP/worker', projectId,
    model: 'synthetic loopback SSE', state: runDoc.state, messages: 3, triggers: 1, claims: 1, starts: 1, finishes: 2,
    rejected: { invalidToken: 401, memberClaim: 403, forgedPermit: 400 } }));
} finally {
  if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  server.closeAllConnections(); if (server.listening) await new Promise(resolve => server.close(resolve));
  await db.terminate(); await deleteApp(app);
}
