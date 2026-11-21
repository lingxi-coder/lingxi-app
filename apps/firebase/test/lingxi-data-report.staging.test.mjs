import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

const base = process.env.LINGXI_REPORT_STAGING;
test('publish consistent datasets and signed MySQL captures through authenticated Team and Chat', { skip: !base }, async t => {
  const fixture = JSON.parse(await readFile(process.env.LINGXI_DATA_FIXTURE, 'utf8'));
  const [owner, reader] = [fixture.owner[0], fixture.reader[0]];
  const tokens = await Promise.all([fixture.owner[1], fixture.reader[1]].map(file => readFile(file, 'utf8')));
  const room = process.env.LINGXI_REPORT_ROOM ?? 'orders-chat';
  const scope = { kind: 'team', teamId: fixture.team, conversationId: room, generation: 1 };
  const run = randomUUID().replaceAll('-', '').slice(0, 12);
  const data = async (command, actor = 0, operation, expected = 200) => {
    const response = await fetch(new URL('/v1/teams/data', base), { method: 'POST', headers: {
      'content-type': 'application/json', authorization: 'Bearer ' + tokens[actor].trim(),
    }, body: JSON.stringify({ scope, command, ...(operation ? { operation } : {}) }) });
    const result = await response.json();
    assert.equal(response.status, expected, command.action + ': ' + JSON.stringify(result.error ?? {}));
    return result;
  };
  const audience = await data({ action: 'previewAudience', editors: [] });
  const schema = { type: 'object', additionalProperties: false, properties: {
    region: { type: 'string' }, day: { type: 'string' }, amount: { type: 'string' },
  }, required: ['region', 'day', 'amount'] };
  const { dataset } = await data({ action: 'createTeam', editors: [], audienceRevision: audience.audienceRevision, schema }, 0, run);
  async function put(record, value, expectedRevision = 0) {
    return data({ action: 'put', dataset, schemaVersion: 1, record, expectedRevision, value }, 0, randomUUID());
  }
  for (let i = 0; i < 53; i++) await put('row' + i, { region: i === 0 ? 'west' : 'east', day: 'Monday',
    amount: i === 0 ? '9007199254740993.01' : '0.01' });
  const ctx = await data({ action: 'reportContext', dataset });
  const { epoch } = await data({ action: 'describe', dataset });
  const spec = { title: 'Weekly sales', partition: 'region', group: 'day', metric: { kind: 'sum', field: 'amount', scale: 2 } };
  const plan = { origin: { kind: 'team', dataset, epoch }, spec, parent: ctx.head, audienceRevision: ctx.audienceRevision,
    readers: [{ uid: owner, partitions: '*' }, { uid: reader, partitions: ['east'] }], bots: [] };
  const prepare = plan => data({ action: 'prepareReport', dataset, plan });
  const draft = await prepare(plan);
  const publish = (plan, draft) => ({ action: 'publishReport', dataset, plan, confirmation: draft.confirmation });
  const head = await data(publish(plan, draft));
  const get = release => ({ action: 'getReport', dataset, release });
  assert.deepEqual((await data(get(head.release))).points, [{ group: 'Monday', value: '9007199254740993.53' }]);
  assert.deepEqual((await data(get(head.release), 1)).points, [{ group: 'Monday', value: '0.52' }]);
  const review = { action: 'openReportReview', ...head, partition: 'west', group: 'Monday' };
  const case1 = await data(review);
  assert.deepEqual(await data(review), case1);
  await data(review, 1, undefined, 403);
  await put('row1', { region: 'east', day: 'Monday', amount: '9.01' }, 1);
  assert.deepEqual(await data(publish(plan, draft)), head);
  const nextEpoch = (await data({ action: 'describe', dataset })).epoch;
  const newer = { ...plan, origin: { ...plan.origin, epoch: nextEpoch }, parent: head.release };
  const other = { ...newer, spec: { ...spec, title: 'Another release' } };
  const nextDraft = await prepare(newer), otherDraft = await prepare(other);
  const next = await data(publish(newer, nextDraft));
  await data(publish(other, otherDraft), 0, undefined, 409);
  assert.equal((await data(get(null), 1)).points[0].value, '9.52');
  assert.equal((await data(get(head.release), 1)).points[0].value, '0.52');
  await data({ action: 'limitReportReader', dataset, uid: reader, generation: 1, expectedVersion: 0, partitions: [] });
  assert.deepEqual((await data(get(head.release), 1)).points, []);
  await data({ action: 'limitReportReader', dataset, uid: reader, generation: 1, expectedVersion: 1, partitions: '*' });
  assert.equal((await data(get(null), 1)).points[0].value, '9.52');
  t.diagnostic('53 records, exact decimal sum, per-reader filtering, pinned/latest, CAS, revocation, idempotent review passed');

  const f = fixture.private;
  assert.ok(f?.config);
  const work = await mkdtemp(join(tmpdir(), 'lx-report-test-'));
  t.after(() => rm(work, { recursive: true, force: true }));
  const source = 'src-' + run;
  const config = JSON.parse(await readFile(f.config, 'utf8')); config.source = source;
  const configPath = join(work, 'source.json'), intentPath = join(work, 'intent.json');
  await writeFile(configPath, JSON.stringify(config), { mode: 0o600 });
  const configure = async (enabled, expectedVersion) => {
    const result = await fetch(new URL('/v1/teams/data', base), { method: 'POST', headers: {
      authorization: 'Bearer ' + tokens[0].trim(), 'content-type': 'application/json',
    }, body: JSON.stringify({ action: 'configureDataSource', scope, source, expectedVersion,
      name: 'Test register', kind: 'mysql', key: f.publicKey, enabled, requesters: [], grants: [] }) });
    assert.equal(result.status, 200, await result.text());
  };
  await configure(true, 0);
  const privatePlan = { ...plan, parent: next.release, origin: { kind: 'source', source, version: 1 },
    spec: { title: 'Source report', partition: 'id', group: 'id', metric: { kind: 'count' } },
    readers: [{ uid: owner, partitions: '*' }, { uid: reader, partitions: ['a'] }] };
  const { binding } = await prepare(privatePlan);
  await writeFile(intentPath, JSON.stringify({ binding, spec: privatePlan.spec }), { mode: 0o600 });
  const capture = JSON.parse(execFileSync(f.cli, ['capture-report', '--config', configPath, '--request', intentPath], { encoding: 'utf8' }));
  const bytes = Buffer.from(capture.payload, 'base64url');
  assert.ok(!bytes.includes('private_note') && !bytes.includes('mysql://'));
  const confirmation = createHash('sha256').update(bytes).digest('hex');
  const release = { ...publish(privatePlan, { confirmation }), capture };
  await data({ ...release, plan: { ...privatePlan, readers: [{ uid: owner, partitions: '*' }] } }, 0, undefined, 403);
  const result = await data(release);
  await configure(false, 1);
  assert.deepEqual((await data(get(result.release), 1)).points, [{ group: 'a', value: '1' }]);
  const text = 'lingxi-data:release/' + dataset + '/' + result.release;
  const msg = await fetch(new URL('/v1/teams/messages', base), { method: 'POST', headers: {
    authorization: 'Bearer ' + tokens[0].trim(), 'content-type': 'application/json',
  }, body: JSON.stringify({ action: 'mutate', teamId: scope.teamId, generation: 1, conversationId: room, operation: 'create',
    messageId: 'report-' + run, changeId: 'change-' + run, text }) });
  assert.equal(msg.status, 200, await msg.text());
  t.diagnostic('Signed MySQL capture, destination/audience binding, source grant revocation, historical receipt, and Chat link passed. ' + text);
});
