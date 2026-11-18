// Production acceptance using disposable accounts only. Tokens remain in memory.
// Run with the repository's tsx loader to exercise the shared Inbox listener.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase/app';
import { initializeAuth, inMemoryPersistence, createUserWithEmailAndPassword, signInWithEmailAndPassword, deleteUser } from 'firebase/auth';
import * as sdk from 'firebase/firestore';
import { Firestore } from 'firebase-admin/firestore';
import { OAuth2Client } from 'google-auth-library';
import { watchInbox } from '../../../packages/inbox/firestore.ts';

const project = 'lingxi-5e4b2', origin = 'https://lingxios.net';
const run = `inboxlive_${Date.now()}_${randomBytes(3).toString('hex')}`;
const uids = [];
const hash = value => createHash('sha256').update(value).digest('hex');
const operatorToken = execFileSync('gcloud', ['auth', 'print-access-token', `--project=${project}`], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const oauth = new OAuth2Client(); oauth.setCredentials({ access_token: operatorToken });
const admin = new Firestore({ projectId: project, authClient: oauth, preferRest: true });
const clients = [], stops = [], teams = new Set(), codes = new Set(), invites = new Set();
const accounts = new Map();
const report = { run, startedAt: new Date().toISOString(), checks: [], cleanup: false };
const output = process.env.INBOX_LIVE_REPORT;
if (!output) throw new Error('INBOX_LIVE_REPORT is required');
const record = (name, detail = {}) => { report.checks.push({ name, ...detail }); console.log(name, JSON.stringify(detail)); };
async function client(role, suffix) {
  const existing = accounts.get(role);
  const credentials = existing ?? { email: `${run}_${role}@inbox-live.invalid`, password: randomBytes(32).toString('base64url') };
  const app = initializeApp({ projectId: project, apiKey: 'AIzaSyAW8Uc_kkM1oSKUsP3whISCTx64G7ci_h0' }, `${run}_${suffix}`);
  const auth = initializeAuth(app, { persistence: inMemoryPersistence });
  const entry = { app, auth, db: sdk.getFirestore(app), suffix }; clients.push(entry);
  entry.user = (await (existing ? signInWithEmailAndPassword : createUserWithEmailAndPassword)(auth, credentials.email, credentials.password)).user;
  entry.uid = entry.user.uid;
  if (!existing) { accounts.set(role, credentials); uids.push(entry.uid); }
  console.log(`Authenticated disposable ${suffix}`);
  return entry;
}
async function api(c, path, body, expected = 200) {
  const response = await fetch(origin + path, { method: body ? 'POST' : 'GET', headers: { Authorization: `Bearer ${await c.user.getIdToken()}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(60000) });
  const text = await response.text(); let value;
  try { value = JSON.parse(text); } catch { throw new Error(`${path}: non-JSON HTTP ${response.status}`); }
  assert.equal(response.status, expected, `${path}: ${JSON.stringify(value)}`); return value;
}
const inbox = (c, body) => api(c, '/v1/inbox', body);
const wait = async (fn, label, timeout = 120000) => {
  const start = Date.now(); let last;
  while (Date.now() - start < timeout) { last = await fn(); if (last) return { value: last, ms: Date.now() - start }; await new Promise(r => setTimeout(r, 350)); }
  throw new Error(`Timeout: ${label}`);
};
async function listen(c) {
  c.rows = new Map(); c.badge = -1; c.signals = 0; c.errors = [];
  stops.push(watchInbox(sdk, c.db, c.uid, () => {
    const revision = ++c.signals;
    void inbox(c, { action: 'list' }).then(page => {
      if (revision === c.signals) c.rows = new Map(page.items.map(row => [row.key, row]));
    }).catch(e => c.errors.push(e.message));
  }, count => { c.badge = count; }, e => c.errors.push(e.message)));
  await wait(() => { if (c.errors.length) throw new Error(c.errors.join('; ')); return c.signals && c.badge >= 0; }, `initial listener ${c.suffix}`);
  console.log(`Subscribed ${c.suffix}`);
}
let a1, a2, b, outsider;
try {
  a1 = await client('a', 'device1'); a2 = await client('a', 'device2');
  b = await client('b', 'sender'); outsider = await client('outsider', 'outsider');
  await listen(a1); await listen(a2);
  const profile = await api(a1, '/v1/friends/profile'); codes.add(hash(profile.friendCode.code));
  const linked = await api(b, '/v1/friends', { action: 'redeem', code: profile.friendCode.code, requestId: `${run}_redeem` });
  teams.add(linked.teamId);
  const teamId = `${run}_team`; teams.add(teamId);
  await api(a1, '/v1/teams', { action: 'create', teamId, name: 'Inbox synthetic acceptance' });
  const invite = await api(a1, '/v1/teams', { action: 'invite', teamId, requestId: `${run}_invite`, role: 'member', maxUses: 1, ttlSeconds: 3600 });
  invites.add(hash(invite.code));
  await api(b, '/v1/teams', { action: 'join', code: invite.code, requestId: `${run}_join` });
  const scopes = [{ teamId: linked.teamId, generation: 1, conversationId: 'friend_chat', category: 'friend' }, { teamId, generation: 1, conversationId: `${run}_channel`, category: 'team' }];
  await api(a1, '/v1/teams/messages', { action: 'create', ...Object.fromEntries(Object.entries(scopes[1]).filter(([k]) => k !== 'category')), kind: 'channel', title: 'Inbox acceptance', participantUids: uids.slice(0, 2) });
  for (const [i, item] of scopes.entries()) {
    const { category, ...scope } = item, key = `team:${scope.teamId}:${scope.conversationId}`;
    await wait(() => a1.rows.get(key) && a2.rows.get(key), `${category} summary`);
    const send = { action: 'mutate', ...scope, messageId: `${run}_message_${i}`, changeId: `${run}_change_${i}`, operation: 'create', text: 'Synthetic Inbox verification' };
    await api(b, '/v1/teams/messages', send);
    await api(b, '/v1/teams/messages', send); // retry same accepted operation
    const arrived = await wait(() => a1.rows.get(key)?.unreadCount === 1 && a2.rows.get(key)?.unreadCount === 1, `${category} unread on both devices`);
    await wait(() => a1.badge === 1 && a2.badge === 1, 'one attention conversation on both devices');
    assert.equal(a1.rows.get(key).category, category);
    assert.equal((await inbox(b, { action: 'get', key })).item.unreadCount, 0);
    record(`${category}: deployed event/task fanout, both summary listeners, idempotent incoming and no self unread`, { convergenceMs: arrived.ms });
    // Sending from the reader must preserve the earlier incoming unread.
    await api(a1, '/v1/teams/messages', { ...send, messageId: `${run}_ownmsg_${i}`, changeId: `${run}_ownchg_${i}` });
    await wait(() => a1.rows.get(key)?.sourceCursor === 2 && a2.rows.get(key)?.sourceCursor === 2, 'own outgoing projection');
    assert.equal(a1.rows.get(key).unreadCount, 1);
    const row = a1.rows.get(key), read = { action: 'markRead', key, generation: 1, through: row.sourceCursor, manualUnreadRevision: row.manualUnreadRevision, operationId: `${run}_read_${i}` };
    await inbox(a1, read);
    const converged = await wait(() => a2.rows.get(key)?.unreadCount === 0 && a2.rows.get(key)?.readThrough === 2, 'device2 receives device1 read');
    assert.equal((await admin.doc(`teams/${scope.teamId}/conversations/${scope.conversationId}/reads/${uids[0]}`).get()).data().lastReadSeq, 2);
    record(`${category}: read commits canonical watermark and propagates to independent second client`, { convergenceMs: converged.ms });
    await inbox(a2, { action: 'markUnread', key, generation: 1, operationId: `${run}_unread_${i}`, expectedManualUnreadRevision: a2.rows.get(key).manualUnreadRevision });
    await inbox(a1, read); // stale old device read cannot clear newer reminder
    await wait(() => a1.rows.get(key)?.manualUnread && a2.rows.get(key)?.manualUnread, 'manual reminder preserved');
    assert.equal(a2.rows.get(key).readThrough, 2);
    const latest = a2.rows.get(key);
    await inbox(a2, { ...read, manualUnreadRevision: latest.manualUnreadRevision, operationId: `${run}_clear_${i}` });
    await wait(() => !a1.rows.get(key)?.manualUnread && !a2.rows.get(key)?.manualUnread, 'explicit reminder clear');
    // Legacy canonical SDK read is reconciled by the deployed read trigger.
    await api(b, '/v1/teams/messages', { ...send, messageId: `${run}_legacymsg_${i}`, changeId: `${run}_legacychg_${i}` });
    await wait(() => a2.rows.get(key)?.unreadCount === 1, 'new incoming');
    await sdk.setDoc(sdk.doc(a1.db, `teams/${scope.teamId}/conversations/${scope.conversationId}/reads/${uids[0]}`), { generation: 1, lastReadSeq: 3 });
    await wait(() => a2.rows.get(key)?.unreadCount === 0 && a2.rows.get(key)?.readThrough === 3, 'legacy read trigger');
    const denied = await fetch(origin + '/v1/inbox', { method: 'POST', headers: { Authorization: `Bearer ${await outsider.user.getIdToken()}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'get', key }) });
    assert.ok([403, 404].includes(denied.status));
    record(`${category}: manual unread survives stale read; legacy SDK watermark reconciles; outsider denied`);
  }
  await wait(() => a1.badge === 0 && a2.badge === 0, 'global badges converge');
  await assert.rejects(sdk.getDocs(sdk.collection(outsider.db, 'users', uids[0], 'inboxSignals')), e => e.code === 'permission-denied');
  for (const c of [a1, a2]) assert.deepEqual(c.errors, []);
  record('owner-only Firestore subscriptions and both global badges verified', { device1Signals: a1.signals, device2Signals: a2.signals });
  report.passed = true;
} catch (error) {
  report.passed = false; report.error = error.message; console.error(error.message); process.exitCode = 1;
  report.clients = [a1, a2].filter(Boolean).map(c => ({ suffix: c.suffix, signals: c.signals, badge: c.badge, keys: [...(c.rows?.keys() ?? [])], errors: c.errors }));
  report.testUids = uids;
  report.testTeams = [...teams];
  try {
    report.projectionDiagnostics = await Promise.all(uids.map(async uid => ({ uid, inbox: (await admin.collection(`users/${uid}/inbox`).get()).docs.map(d => d.data()), jobs: (await admin.collection(`users/${uid}/inboxJobs`).get()).docs.map(d => d.data()) })));
  } catch (e) { report.diagnosticError = e.message; }
}
finally {
  for (const stop of stops) stop();
  try {
    // Remove source membership first, then private projections; only this run's exact IDs.
    for (const id of teams) await admin.recursiveDelete(admin.doc(`teams/${id}`));
    if (uids.length >= 2) await admin.recursiveDelete(admin.doc(`friendships/${hash(uids.slice(0, 2).sort().join('|'))}`));
    for (const uid of uids) {
      const profile = await admin.doc(`profiles/${uid}`).get();
      if (profile.data()?.friendCodeId) codes.add(profile.data().friendCodeId);
      await admin.recursiveDelete(admin.doc(`profiles/${uid}`));
      await admin.recursiveDelete(admin.doc(`users/${uid}`));
      for (const bucket of ['lookup', 'request']) await admin.doc(`friendLimits/${hash(`${bucket}:${uid}`)}`).delete();
    }
    for (const code of codes) await admin.doc(`friendCodes/${code}`).delete();
    for (const invite of invites) await admin.doc(`teamInvites/${invite}`).delete();
    for (const uid of uids) await deleteUser(clients.find(c => c.uid === uid).user);
    report.cleanup = true;
  } catch (e) { report.cleanupError = e.message; process.exitCode = 1; }
  for (const c of clients) { await sdk.terminate(c.db); await deleteApp(c.app); }
  await admin.terminate();
  report.completedAt = new Date().toISOString(); writeFileSync(output, JSON.stringify(report, null, 2));
}
