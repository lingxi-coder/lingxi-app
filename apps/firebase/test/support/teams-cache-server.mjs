import { createServer } from 'node:http';
import { writeFile } from 'node:fs/promises';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamMessagesStore } from '../../src/team-messages.mjs';

if (!/^127\.0\.0\.1:\d+$/u.test(process.env.FIRESTORE_EMULATOR_HOST ?? '') || !/^127\.0\.0\.1:\d+$/u.test(process.env.FIREBASE_AUTH_EMULATOR_HOST ?? '')) throw new Error('Local emulators required');
const projectId = 'demo-lingxi-teams-t0';
const app = initializeApp({ projectId }, 'teams-cache-acceptance'), db = getFirestore(app), auth = getAuth(app);
const store = new TeamMessagesStore(db), fixtures = new Map(), unavailable = new Set(), commands = [], reconciledControls = [];
let ordinaryReads = 0;
async function fixture(client) {
  if (!['ios', 'android', 'web', 'desktop'].includes(client)) throw new Error('Invalid fixture client');
  if (fixtures.has(client)) return fixtures.get(client);
  const uid = `cache-user-${client}`, teamId = `sdk-cache-team-${client}`, conversationId = `sdk-cache-chat-${client}`;
  const root = db.doc(`teams/${teamId}`), conversation = root.collection('conversations').doc(conversationId);
  const batch = db.batch();
  batch.set(root, { state: 'active', name: 'SDK Cache Fixture' });
  batch.set(root.collection('members').doc(uid), { generation: 1, role: 'owner', displayName: 'Cache reader' });
  batch.set(conversation, { kind: 'dm', title: 'SDK Cache Fixture', participants: [`user:${uid}`], memberUids: [uid], lastSeq: 125, lastChangeSeq: 125, lastMessage: { id: 'message-125', seq: 125, at: new Date() }, lastChange: { id: 'change-125', seq: 125 } });
  for (let seq = 1; seq <= 125; seq++) batch.set(conversation.collection('messages').doc(`message-${seq}`), { seq, author: { kind: 'user', uid }, kind: 'text', text: `Body ${seq}`, changeSeq: seq, changeId: `change-${seq}`, createdAt: new Date() });
  await batch.commit();
  const value = { uid, teamId, conversationId, generation: 1, config: { projectId, apiKey: 'emulator-only', appId: `cache-${client}` }, customToken: await auth.createCustomToken(uid) };
  fixtures.set(client, value); return value;
}
const server = createServer((request, response) => { void handle(request, response).catch(error => {
  const path = new URL(request.url, 'http://127.0.0.1:41791').pathname;
  console.warn(JSON.stringify({ path, status: error.status ?? 500, code: typeof error.code === 'number' ? error.code : 'SERVICE_UNAVAILABLE' }));
  response.writeHead(error.status ?? 500, { 'Content-Type': 'application/json' }); response.end(JSON.stringify({ error: { code: error.code ?? 'SERVICE_UNAVAILABLE' } }));
}); });
async function handle(request, response) {
  const url = new URL(request.url, 'http://127.0.0.1:41791');
  let value;
  if (url.pathname === '/fixture') value = await fixture(url.searchParams.get('client'));
  else if (url.pathname === '/stats') value = { ordinaryReads, commands, reconciledControls };
  else if (url.pathname === '/v1/teams/messages') {
    let body = ''; for await (const chunk of request) { body += chunk; if (body.length > 65536) throw new Error('Body too large'); }
    const input = JSON.parse(body), identity = await auth.verifyIdToken(request.headers.authorization?.replace(/^Bearer /u, '') ?? '');
    const client = [...fixtures].find(([, value]) => value.uid === identity.uid)?.[0];
    if (!client) throw new Error('Unknown test identity');
    commands.push({ client, action: input.action, changeId: input.changeId, text: input.text });
    if (['list', 'sync', 'directory'].includes(input.action)) { ordinaryReads++; throw new Error('Ordinary HTTP read forbidden'); }
    if (unavailable.has(client)) { response.writeHead(503, { 'Content-Type': 'application/json' }); response.end(JSON.stringify({ error: { code: 'SERVICE_UNAVAILABLE' } })); return; }
    value = await store.execute(identity, input);
  } else if (['/edit', '/read', '/generation', '/revoke', '/offline', '/online'].includes(url.pathname)) {
    const client = url.searchParams.get('client'), f = await fixture(client), root = db.doc(`teams/${f.teamId}`), conversation = root.collection('conversations').doc(f.conversationId);
    if (url.pathname === '/edit') {
      const batch = db.batch(); batch.update(conversation.collection('messages').doc('message-1'), { text: 'Edited oldest', changeSeq: 126 }); batch.update(conversation.collection('messages').doc('message-2'), { text: '', recalled: true, changeSeq: 127 }); batch.update(conversation, { lastChangeSeq: 127 }); await batch.commit();
    } else if (url.pathname === '/read') await conversation.collection('reads').doc(f.uid).set({ generation: 1, lastReadSeq: 120 });
    else if (url.pathname === '/generation') await root.collection('members').doc(f.uid).update({ generation: 2 });
    else if (url.pathname === '/revoke') {
      const member = root.collection('members').doc(f.uid);
      try { await member.delete(); }
      catch (error) {
        // The emulator can report CANCELLED after delivering revocation to a
        // listener that immediately unsubscribes. Confirm the committed delete.
        if (error.code !== 1 || (await member.get()).exists) throw error;
        reconciledControls.push({ client, action: 'revoke', code: 'CANCELLED', confirmedAbsent: true });
      }
    }
    else if (url.pathname === '/offline') unavailable.add(client);
    else unavailable.delete(client);
    value = { ok: true };
  } else if (url.pathname === '/finish') {
    const result = { projectId, clients: [...fixtures.keys()], ordinaryReads, commands, reconciledControls };
    if (process.env.TEAM_CACHE_RESULT) await writeFile(process.env.TEAM_CACHE_RESULT, JSON.stringify(result, null, 2));
    value = { ok: true, ordinaryReads };
    setTimeout(() => { server.close(async () => { clearTimeout(deadline); await db.terminate(); await deleteApp(app); }); }, 50);
  } else { response.writeHead(404); response.end(); return; }
  response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); response.end(JSON.stringify(value));
}
const deadline = setTimeout(() => { server.close(); process.exitCode = 1; }, 60 * 60 * 1000);
server.listen(41791, '127.0.0.1', () => console.log(JSON.stringify({ projectId, port: 41791, fixtureOnly: true })));
