import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { extractFile } from '@electron/asar';

async function verifyOwnerCapture({ page, evaluate, userData, uid, scope, dataset }) {
  const source = 'report-qa-' + randomUUID();
  const folder = join(userData, 'data-sources', createHash('sha256').update(uid).digest('hex'));
  await mkdir(folder, { recursive: true, mode: 0o700 });
  const rows = join(folder, source + '-rows.json');
  const key = join(folder, source + '.key');
  execFileSync(process.env.LINGXI_DATA_PREVIEW_BIN, ['test-key', '--output', key]);
  let secret;
  try { secret = (await readFile(key, 'utf8')).trim(); }
  finally { await rm(key, { force: true }); }
  assert.match(secret, /^[A-Za-z0-9_-]{43}$/u);
  const service = 'com.lingxi.data.host-key';
  const match = ['-s', service, '-a', source];
  const probe = () => spawnSync('/usr/bin/security', ['find-generic-password', ...match], { stdio: 'ignore' }).status;
  assert.equal(probe(), 44);
  // security's interactive input keeps the temporary key out of command-line arguments.
  execFileSync('/usr/bin/security', ['-i'], { input: `add-generic-password -s ${service} -a ${source} -w ${secret} -T /usr/bin/security\n`, stdio: ['pipe', 'ignore', 'ignore'] });
  try {
    assert.equal(probe(), 0);
    await writeFile(rows, JSON.stringify([{ id: 'a', region: 'east' }, { id: 'b', region: 'east' }]), { mode: 0o600 });
    await writeFile(join(folder, source + '.json'), JSON.stringify({ owner: uid, source, key: { keychain: { service, account: source } },
      presets: { main: { keyField: 'id', fields: ['id', 'region'], input: { kind: 'json', path: rows } } } }), { mode: 0o600 });
    const native = request => evaluate(page, `window.lingxi.teams.localDataSource(${JSON.stringify(uid)},${JSON.stringify(request)})`);
    const data = command => evaluate(page, `window.lingxi.teams.data(${JSON.stringify(uid)},${JSON.stringify({ scope, command })})`);
    const input = { source, name: 'Signed source', scope, requesters: [], grants: [] };
    await native({ action: 'configure', input: { ...input, enabled: true, expectedVersion: 0 } });
    const context = await data({ action: 'reportContext', dataset });
    const plan = { parent: context.head, audienceRevision: context.audienceRevision, readers: [{ uid, partitions: '*' }], bots: [],
      spec: { title: 'Signed owner capture', partition: 'region', group: 'region', metric: { kind: 'count' } },
      origin: { kind: 'source', source, version: 1 } };
    const draft = await native({ action: 'captureReport', scope, dataset, source, plan });
    assert.deepEqual(draft.facts, [{ partition: 'east', group: 'east', units: '2' }]);
    const saved = await data({ action: 'publishReport', dataset, plan, capture: draft.capture, confirmation: draft.confirmation });
    await native({ action: 'configure', input: { ...input, enabled: false, expectedVersion: 1 } });
    assert.deepEqual((await data({ action: 'getReport', ...saved })).points, [{ group: 'east', value: '2' }]);
  } finally {
    execFileSync('/usr/bin/security', ['delete-generic-password', ...match], { stdio: 'ignore' });
    assert.equal(probe(), 44);
    await Promise.all([rows, join(folder, source + '.json')].map(file => rm(file, { force: true })));
  }
}

export async function verifySignedReport({ appPath, userData, page, evaluate, waitFor }) {
  const compiled = extractFile(join(appPath, 'Contents/Resources/app.asar'), 'out/main/index.js');
  assert.ok(compiled.includes('LINGXI_SIGNED_DATA_DEMO_ONLY'), 'Requires a dedicated signed demo build');
  const line = (await readFile(process.env.LINGXI_DATA_STAGING_ENDPOINTS, 'utf8')).split('\n').find(value => value.startsWith('{"web"'));
  const fixture = JSON.parse(line);
  assert.equal(fixture.project, 'demo-lingxi-data');
  assert.match(fixture.firestore, /^127\.0\.0\.1:\d+$/u);
  const match = /^lingxi-data:release\/([A-Za-z0-9_-]{1,64})\/([a-f0-9]{64})$/u.exec(process.env.LINGXI_TEST_REPORT_LINK);
  assert.ok(match);
  const [, dataset, release] = match;
  const scope = { kind: 'team', teamId: fixture.teamId, generation: 1, conversationId: fixture.conversationId };
  const caseId = createHash('sha256').update(JSON.stringify([release, 'c', 'c', 'review'])).digest('hex');
  function storedCase() {
    const path = '/v1/projects/demo-lingxi-data/databases/(default)/documents/lingxiReportSeries/' + dataset + '/reviews/' + caseId;
    return fetch('http://' + fixture.firestore + path, { headers: { authorization: 'Bearer owner' } });
  }
  for (const account of ['reader', 'owner']) {
    const state = await evaluate(page, `window.lingxi.account.request({type:'signIn',email:'${account}@fixture.invalid',password:'AccountFixture123'}).then(s=>({uid:s.user?.uid}))`);
    assert.ok(state.uid);
    try {
      const response = await evaluate(page, `window.lingxi.teams.data(${JSON.stringify(state.uid)}, ${JSON.stringify({
        scope, command: { action: 'getReport', dataset, release },
      })})`);
      assert.equal(response.spec.title, 'Source report');
      assert.equal(response.points.length, account === 'reader' ? 1 : 6);
      console.log('[signed-report] Authenticated ' + account + ' read ' + response.points.length + ' groups through the native bridge.');
      await waitFor(() => evaluate(page, `!!document.querySelector('.desktop-rail-teams')`), { label: 'signed Desktop Teams entry' });
      await evaluate(page, `document.querySelector('.desktop-rail-teams').click()`);
      const room = `document.querySelector('button[data-row="orders-chat"]')`;
      try {
        await waitFor(() => evaluate(page, `Boolean(${room})`), { timeoutMs: 45000, label: 'signed Desktop staging room' });
      } catch (error) {
        const state = await evaluate(page, `({ sidebar:!!document.querySelector('.teams-sidebar'),
          team:document.querySelector('.tw-switcher-name')?.textContent, rooms:document.querySelectorAll('button[data-row]').length,
          status:document.querySelector('.tw-sidebar [role="status"]')?.textContent })`);
        throw new Error('Signed staging directory: ' + JSON.stringify(state), { cause: error });
      }
      await evaluate(page, `(${room}).click()`);
      const card = `Array.from(document.querySelectorAll('.tw-report')).at(-1)`;
      await waitFor(() => evaluate(page, `Boolean(${card})`), { label: 'published report in signed Desktop chat' });
      await evaluate(page, `(${card}).querySelector('button').click()`);
      await waitFor(() => evaluate(page, `(${card})?.querySelector('h4')?.textContent==='Source report'`), { label: 'signed Desktop report title' });
      const rows = await evaluate(page, `Array.from((${card}).querySelectorAll('tbody tr'),r=>[r.querySelector('th')?.textContent,r.querySelector('td:last-child')?.textContent])`);
      assert.deepEqual(rows, response.points.map(point => [point.group, point.value]));
      console.log('[signed-report] Rendered the ' + account + ' projection from the Chat entry.');
      if (account === 'reader') {
        assert.equal(await evaluate(page, `(${card}).querySelectorAll('details p button').length`), 0);
      } else {
        await evaluate(page, `{const c=(${card}), p=[...c.querySelectorAll('details p')].find(p=>p.textContent.startsWith('c · c · ')); p.closest('details').open=true; p.querySelector('button').click();}`);
        await waitFor(() => evaluate(page, `/Review case open|复核单已打开/.test((${card}).textContent)`), { label: 'signed Desktop review receipt' });
        const saved = await storedCase();
        assert.equal(saved.status, 200);
        const fields = (await saved.json()).fields;
        assert.equal(fields.state.stringValue, 'open');
        assert.equal(fields.actor.mapValue.fields.uid.stringValue, state.uid);
        await verifyOwnerCapture({ page, evaluate, userData, uid: state.uid, scope, dataset });
      }
    } finally {
      await evaluate(page, `window.lingxi.account.request({type:'signOut'}).then(s=>!s.user)`);
    }
  }
  console.log('[signed-report] Reader saw partition a. Owner saw six groups, opened c and matched the persisted review ' + caseId);
  console.log('[signed-report] Published a Keychain-signed source capture through Desktop and verified its pinned release after disabling the source.');
}
