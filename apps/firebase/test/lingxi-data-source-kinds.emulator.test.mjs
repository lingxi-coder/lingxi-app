import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';
import { promisify } from 'node:util';
import test from 'node:test';

import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';

import { TeamMessagesStore } from '../src/team-messages.mjs';

const exec = promisify(execFile);

test('every database kind registers, and PostgreSQL previews and publishes a signed report through the product gateway', {
  skip: !process.env.LINGXI_DATA_PREVIEW_KEY, timeout: 180_000,
}, async t => {
  const fixture = JSON.parse(await readFile(process.env.LINGXI_DATA_FIXTURE, 'utf8'));
  const { ca, ice, cli, folder, publicKey, postgres } = fixture.private;
  assert.ok(postgres, 'run scripts/test-lingxi-data.py with --private-preview --postgres-bin');
  const [owner, ownerFile] = fixture.owner, [reader, readerFile] = fixture.reader;
  const [ownerToken, readerToken] = await Promise.all([ownerFile, readerFile].map(path => readFile(path, 'utf8')));
  const app = initializeApp({ projectId: process.env.GCLOUD_PROJECT }, 'source-kinds-test');
  const db = getFirestore(app);
  t.after(async () => { await db.terminate(); await deleteApp(app); });

  const children = new Set();
  t.after(async () => { await Promise.all([...children].map(stop)); });
  function child(program, args) {
    const subprocess = spawn(program, args, { stdio: 'ignore' });
    children.add(subprocess);
    subprocess.once('exit', () => children.delete(subprocess));
    return subprocess;
  }
  async function stop(subprocess) {
    if (subprocess.exitCode !== null || subprocess.signalCode !== null) return;
    const exit = once(subprocess, 'exit');
    subprocess.kill();
    await exit;
  }
  const post = (path, token, body) => fetch(fixture.endpoint + path, {
    method: 'POST', headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  const adapter = child(process.execPath, ['scripts/serve-lingxi-data.mjs']);
  for (let attempt = 0; ; attempt++) {
    assert.equal(adapter.exitCode, null, 'product adapter exited');
    try { await post('/v1/product-data', 'invalid', {}); break; }
    catch { assert.ok(attempt < 100, 'product adapter did not start'); await wait(100); }
  }

  const register = (source, kind, scope, fields) => post('/v1/product-data', ownerToken, {
    action: 'configureDataSource', source, name: `${kind} orders`, kind, key: publicKey, scope, expectedVersion: 0, enabled: true,
    requesters: [], grants: [], ...(fields ? { fields } : {}),
  });
  // Registration forwards the kind's wire name to the Data service, which rejects a name it does not know.
  for (const kind of ['sqlite', 'mongodb', 'redis']) {
    const registered = await register(`personal-${kind}`, kind, { kind: 'personal' }, ['id', 'title']);
    assert.equal(registered.status, 200, await registered.clone().text());
    assert.equal((await registered.json()).kind, kind);
  }
  const hostConfig = async source => {
    const path = join(folder, source + '.json');
    await writeFile(path, JSON.stringify({ ...JSON.parse(await readFile(postgres, 'utf8')), source }), { mode: 0o600 });
    return path;
  };

  const personal = 'personal-postgres';
  const registered = await register(personal, 'postgres', { kind: 'personal' }, ['id', 'title', 'exact_id', 'exact_value']);
  assert.equal(registered.status, 200, await registered.clone().text());
  const host = child(cli, ['host', '--endpoint', fixture.endpoint, '--auth-file', ownerFile, '--ca', ca, '--config', await hostConfig(personal)]);
  let online = false;
  for (let spin = 0; spin < 60 && !online; spin++) {
    assert.equal(host.exitCode, null, 'PostgreSQL source host exited');
    const directory = await post('/v1/data/signaling', ownerToken, { action: 'directory' });
    assert.equal(directory.status, 200);
    online = (await directory.json())[personal]?.online === true;
    if (!online) await wait(150);
  }
  assert.ok(online, 'PostgreSQL source host did not come online');
  const preview = await exec(cli, ['preview', '--endpoint', fixture.endpoint, '--auth-file', ownerFile, '--ca', ca,
    '--source', personal, '--ice', ice], { timeout: 30_000 });
  assert.deepEqual(JSON.parse(preview.stdout).rows[0], { id: 'a', title: 'Original title',
    exact_id: '18446744073709551615', exact_value: '12345678901234567890.12345678' });
  assert.ok(!preview.stdout.includes('private_note') && !preview.stdout.includes('PREVIEW_ONLY'));
  await stop(host);

  const team = db.collection('teams').doc(fixture.team);
  await db.batch().set(team, { state: 'active' }, { merge: true })
    .set(team.collection('members').doc(owner), { role: 'owner', generation: 1, displayName: 'Maintainer' }, { merge: true })
    .set(team.collection('members').doc(reader), { role: 'member', generation: 1, displayName: 'Requester' }, { merge: true }).commit();
  const room = 'postgres-report';
  await new TeamMessagesStore(db).execute({ uid: owner }, { action: 'create', teamId: fixture.team, generation: 1,
    conversationId: room, kind: 'group', title: 'PostgreSQL report', participantUids: [owner, reader] });
  const scope = { kind: 'team', teamId: fixture.team, conversationId: room, generation: 1 };
  const source = 'postgres-orders';
  const configured = await register(source, 'postgres', scope);
  assert.equal(configured.status, 200, await configured.clone().text());
  const data = async (command, operation, token = ownerToken) => {
    const response = await post('/v1/product-data', token, { scope, command, ...(operation ? { operation } : {}) });
    assert.equal(response.status, 200, command.action + ': ' + await response.clone().text());
    return response.json();
  };
  const audience = await data({ action: 'previewAudience', editors: [] });
  const { dataset } = await data({ action: 'createTeam', audienceRevision: audience.audienceRevision, editors: [],
    schema: { type: 'object', additionalProperties: false, properties: { title: { type: 'string' } } } }, 'postgres-report-dataset');
  const ctx = await data({ action: 'reportContext', dataset });
  const plan = { origin: { kind: 'source', source, version: 1 }, parent: ctx.head, audienceRevision: ctx.audienceRevision,
    readers: [{ uid: owner, partitions: '*' }, { uid: reader, partitions: ['a'] }], bots: [],
    spec: { title: 'PostgreSQL tickets', partition: 'id', group: 'id', metric: { kind: 'count' } } };
  const { binding } = await data({ action: 'prepareReport', dataset, plan });
  const intent = join(folder, 'postgres-report.capture');
  await writeFile(intent, JSON.stringify({ binding, spec: plan.spec }), { mode: 0o600 });
  const captured = await exec(cli, ['capture-report', '--config', await hostConfig(source), '--request', intent], { timeout: 30_000 });
  const capture = JSON.parse(captured.stdout);
  const bytes = Buffer.from(capture.payload, 'base64url');
  assert.ok(!bytes.includes('private_note') && !bytes.includes('postgres://'));
  const confirmation = createHash('sha256').update(bytes).digest('hex');
  const { release } = await data({ action: 'publishReport', dataset, plan, confirmation, capture });
  const points = async token => (await data({ action: 'getReport', dataset, release }, undefined, token)).points;
  assert.deepEqual(await points(readerToken), [{ group: 'a', value: '1' }]);
  assert.deepEqual(await points(ownerToken), ['a', 'b', 'c', 'd', 'e', 'f'].map(group => ({ group, value: '1' })));
});
