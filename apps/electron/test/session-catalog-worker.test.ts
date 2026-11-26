import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, realpathSync, rmSync, appendFileSync, readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { test, type TestContext } from 'node:test';

import { ProjectSessionCatalog } from '../src/main/session-catalog.js';
import { conversationFromMessages, prependHistory } from '../src/renderer/bridge/conversation.js';

const uuid = '11111111-2222-4333-8444-555555555555';
const row = (id = uuid, title = '会话😀\ud800') => ({ uuid: id, title, modified_rfc3339: '', message_count: 1,
  mode: 'code', path: `/source/${id}.jsonl`, empty_session: false });
const output = (id = uuid, title?: string) => JSON.stringify({ version: 1, sessions: [row(id, title)] });
const frame = (id: number, body: string, kind = 'output') => Buffer.concat([
  Buffer.from(`${JSON.stringify({ version: 2, id, bytes: Buffer.byteLength(body), kind })}\n`),
  Buffer.from(body), Buffer.from('\n'),
]);

function project(t: TestContext) {
  const path = realpathSync(mkdtempSync(join(tmpdir(), 'catalog-worker-')));
  t.after(() => rmSync(path, { recursive: true, force: true }));
  return path;
}

function processes() {
  let live = 0, peak = 0;
  const calls: Array<{ args: string[]; options: any; process: any; queries: any[]; kills: number;
    reply(value?: string, index?: number): void; finish(value: unknown): void; close(): void }> = [];
  const spawnProcess = ((_bin: string, args: readonly string[], options: any) => {
    const child = new EventEmitter() as any;
    child.stdin = new PassThrough(); child.stdout = new PassThrough(); child.stderr = new PassThrough();
    child.unref = () => {};
    live++; peak = Math.max(peak, live);
    let closed = false;
    const close = (code: number | null = 0, signal: string | null = null) => {
      if (closed) return;
      closed = true; live--;
      child.emit('close', code, signal);
    };
    const call = { args: [...args], options, process: child, queries: [] as any[], kills: 0,
      reply(value = output(), index = call.queries.length - 1) {
        child.stdout.write(frame(call.queries[index].id, value));
      },
      finish(value: unknown) { child.stdout.write(JSON.stringify(value)); close(); },
      close() { close(); },
    };
    child.kill = () => { call.kills++; queueMicrotask(() => close(null, 'SIGTERM')); return true; };
    let input = '';
    child.stdin.on('data', (bytes: Buffer) => {
      input += bytes.toString();
      if (args[0] !== '--session-catalog-worker-json') return;
      while (input.includes('\n')) {
        const newline = input.indexOf('\n');
        call.queries.push(JSON.parse(input.slice(0, newline)));
        input = input.slice(newline + 1);
      }
    });
    calls.push(call);
    return child;
  }) as typeof spawn;
  return { calls, spawnProcess, get live() { return live; }, get peak() { return peak; } };
}

test('abandoning thumbnails cancels queued reads and retires a busy worker', { timeout: 5000 }, async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const transcript = join(cwd, `${uuid}.jsonl`);
  writeFileSync(transcript, '');
  const read = (index: number, signal?: AbortSignal) =>
    catalog.readImage(cwd, uuid, transcript, `lingxi-history-image:${uuid}:${index}:png`, 'thumbnail', signal);

  const busy = new AbortController(), queued = new AbortController();
  const first = assert.rejects(read(0, busy.signal), /cancelled/);
  while (!children.calls[0]?.queries.length) await nextTurn();
  const second = assert.rejects(read(1, queued.signal), { name: 'AbortError' });
  for (let turn = 0; turn < 5; turn++) await nextTurn();
  queued.abort();
  await second;
  assert.equal(children.calls[0].queries.length, 1);

  busy.abort();
  await first;
  const visible = read(2);
  while (!children.calls[1]?.queries.length) await nextTurn();
  assert.equal(children.calls[0].kills, 1);
  assert.equal(children.calls[0].queries.length, 1);
  assert.equal(children.calls[1].queries[0].historyImage.reference, `lingxi-history-image:${uuid}:2:png`);
  const image = { mediaType: 'image/png', base64: 'AQID' };
  children.calls[1].reply(JSON.stringify(image));
  assert.deepEqual(await visible, image);
  assert.deepEqual(await read(2), image);
  assert.equal(children.calls[1].queries.length, 1);
});

test('a located thumbnail survives append, and refreshes after the source changes in place', { timeout: 5_000 }, async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const transcript = join(cwd, `${uuid}.jsonl`);
  writeFileSync(transcript, 'first row\n');
  const read = () => catalog.readImage(cwd, uuid, transcript, `lingxi-history-image:${uuid}:0:png:0:YWJj`, 'thumbnail');
  const first = read();
  while (!children.calls[0]?.queries.length) await nextTurn();
  const child = children.calls[0];
  child.reply(JSON.stringify({ mediaType: 'image/png', base64: 'AQID' }));
  assert.equal((await first).base64, 'AQID');
  appendFileSync(transcript, 'newer row\n');
  catalog.invalidate(cwd, { preserveImages: true });
  assert.equal((await read()).base64, 'AQID');
  assert.equal(child.queries.length, 1);
  writeFileSync(transcript, 'replacement\n');
  const changed = read();
  while (child.queries.length < 2) await nextTurn();
  child.reply(JSON.stringify({ mediaType: 'image/png', base64: 'BAUG' }));
  assert.equal((await changed).base64, 'BAUG');
});

test('durable Inbox activity can be read without a session runtime and keeps exact text', async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const transcript = join(cwd, `${uuid}.jsonl`);
  const preview = catalog.readActivityPreview(cwd, uuid, transcript, 4);
  await nextTurn();
  const child = children.calls[0];
  assert.equal(child.queries[0].activityPreview, 4);
  child.reply(JSON.stringify([{ uuid, at: 40, blocks: [[65, 0xd800, 66]] }]));
  assert.deepEqual(await preview, { available: true, messages: [
    { role: 'assistant', transcript_uuid: uuid, sent_at_ms: 40, blocks: [{ type: 'text', text: 'A\ud800B' }] },
  ] });
});

test('an exact cold selection shares its in-flight read and validates the page separately', async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const selection = { scope: 'current' as const, transcriptPath: row().path };
  const first = catalog.selectForOpen(cwd, uuid, selection);
  assert.equal(catalog.selectForOpen(cwd, uuid, selection), first);
  await nextTurn();
  const child = children.calls[0];
  assert.equal(child.queries[0].transcriptPath, selection.transcriptPath);
  assert.equal(child.queries[0].scope, selection.scope);
  const messages = [{ role: 'user', transcript_uuid: uuid, blocks: [{ type: 'text', text: 'recent' }] }];
  child.reply();
  const firstResult = await first;
  assert.equal(firstResult.session?.path, selection.transcriptPath);
  assert.equal(catalog.selectForOpen(cwd, uuid, selection), first);
  await nextTurn();
  const history = children.calls[1];
  assert.deepEqual(history.queries[0], { version: 2, id: 1, cwd, sessionId: uuid, transcriptPath: selection.transcriptPath, historyLatest: true });
  history.reply(JSON.stringify({ messages, before: null }));
  assert.deepEqual((await firstResult.preview)?.messages, messages);

  await nextTurn();
  const changed = catalog.selectForOpen(cwd, uuid, selection);
  await nextTurn();
  child.reply();
  const next = await changed;
  await nextTurn();
  history.reply(JSON.stringify({ messages, before: { uuid: 'invalid' } }));
  assert.equal(next.session?.path, selection.transcriptPath);
  assert.equal(await next.preview, undefined);
});

test('exact cold open lets startup reuse the selected reader before preview takes a process slot', async t => {
  const cwd = project(t), other = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const open = catalog.selectForOpen(cwd, uuid, { scope: 'current', transcriptPath: row().path });
  await nextTurn();
  const selected = children.calls[0];
  const listing = catalog.listComplete(other);
  await nextTurn();
  const directory = children.calls[1];
  selected.reply();
  const { preview } = await open;
  const startup = catalog.workspaceSettings(cwd);
  await nextTurn();
  assert.equal(selected.queries.at(-1)?.workspaceEnvironment, true);
  assert.equal(selected.kills, 0);
  assert.equal(children.calls.length, 2);
  const settings = { version: 1, config_home: cwd, environment: { FROM_DISK: 'one' } };
  selected.reply(JSON.stringify(settings));
  assert.deepEqual((await startup).environment, settings.environment);
  directory.reply();
  await listing;
  await nextTurn();
  const history = children.calls.find(call => call.queries.at(-1)?.historyLatest);
  assert.ok(history);
  const messages = [{ role: 'user', transcript_uuid: uuid, blocks: [{ type: 'text', text: 'recent' }] }];
  history.reply(JSON.stringify({ messages, before: null }));
  assert.deepEqual((await preview)?.messages, messages);
});

test('history images share an in-flight thumbnail and cache each variant separately', { timeout: 5_000 }, async t => {
  const cwd = project(t), children = processes();
  const transcript = join(cwd, `${uuid}.jsonl`);
  writeFileSync(transcript, '{}\n');
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const reference = `lingxi-history-image:${uuid}:0:png`;
  const thumbnail = { mediaType: 'image/png', base64: 'AQID' };
  const original = { mediaType: 'image/png', base64: 'BAUG' };
  const first = catalog.readImage(cwd, uuid, transcript, reference, 'thumbnail');
  const second = catalog.readImage(cwd, uuid, transcript, reference, 'thumbnail');
  for (let i = 0; i < 100 && !children.calls[0]?.queries.length; i++) await new Promise(resolve => setTimeout(resolve, 5));
  const child = children.calls[0];
  assert.deepEqual(child.queries[0].historyImage, { reference, size: 'thumbnail' });
  child.reply(JSON.stringify(thumbnail));
  assert.deepEqual(await Promise.all([first, second]), [thumbnail, thumbnail]);
  assert.deepEqual(await catalog.readImage(cwd, uuid, transcript, reference, 'thumbnail'), thumbnail);
  assert.equal(child.queries.length, 1);

  const requested = catalog.readImage(cwd, uuid, transcript, reference, 'original');
  for (let i = 0; i < 100 && child.queries.length < 2; i++) await new Promise(resolve => setTimeout(resolve, 5));
  assert.deepEqual(child.queries[1].historyImage, { reference, size: 'original' });
  child.reply(JSON.stringify(original));
  assert.deepEqual(await requested, original);
});

test('a reused selection worker claims its slot before a parallel reader can reclaim it', async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const selection = { scope: 'current' as const, transcriptPath: row().path };
  const warm = catalog.select(cwd, uuid, selection);
  await nextTurn();
  const reader = children.calls[0];
  reader.reply();
  await warm;

  const history = catalog.readHistory(cwd, uuid, selection.transcriptPath, { uuid });
  await nextTurn();
  const historyWorker = children.calls[1];
  const selected = catalog.select(cwd, uuid, selection);
  const listing = catalog.listComplete(cwd);
  await nextTurn();
  assert.equal(reader.queries.length, 2);
  assert.equal(reader.kills, 0);
  reader.reply();
  assert.equal((await selected)?.uuid, uuid);
  await nextTurn();
  assert.equal(children.calls.length, 3);
  children.calls[2].reply();
  assert.equal((await listing).sessions.length, 1);
  historyWorker.reply(JSON.stringify({ messages: [], before: null }));
  await history;
  assert.equal(children.peak, 2);
});

test('workspace launch settings reuse only an idle reader started in the requested cwd', async t => {
  const cwd = project(t), other = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const listing = catalog.listComplete(cwd);
  await nextTurn();
  const reader = children.calls[0];
  const settings = { version: 1, config_home: cwd, environment: { FROM_DISK: 'one' } };
  const concurrent = catalog.workspaceSettings(cwd);
  await nextTurn();
  assert.ok(children.calls[1].args.includes('--workspace-environment-json'));
  children.calls[1].finish(settings);
  await concurrent;
  reader.reply();
  await listing;
  const same = catalog.workspaceSettings(cwd);
  await nextTurn();
  assert.deepEqual(reader.queries.at(-1), { version: 2, id: 2, cwd, workspaceEnvironment: true });
  reader.reply(JSON.stringify(settings));
  assert.deepEqual((await same).environment, settings.environment);

  const different = catalog.workspaceSettings(other);
  await nextTurn();
  assert.ok(children.calls[2].args.includes('--workspace-environment-json'));
  children.calls[2].finish({ ...settings, config_home: other });
  assert.equal((await different).configHome, other);
});

test('plain account listings share queued reads, keep a slot for session startup and return independent snapshots', async t => {
  const cwd = project(t), children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const list = () => catalog.providerAccountCommand({ action: 'list', expectedRevision: 0 });
  const snapshot = (configRevision: number) => ({ storeScope: 'owner', configRevision, accounts: [], health: [] });
  const first = list(), sameBatch = list();
  await nextTurn();
  assert.equal(children.calls.length, 1);

  // These calls arrived after dispatch. Even an external registry mutation
  // between the two batches must be visible to the second group.
  const later = list(), sameLaterBatch = list();
  const settings = catalog.workspaceSettings(cwd);
  await nextTurn();
  assert.equal(children.calls.length, 2);
  assert.ok(children.calls[1].args.includes('--workspace-environment-json'));
  children.calls[1].finish({ version: 1, config_home: cwd, environment: {} });
  await settings;
  children.calls[0].finish(snapshot(1));
  const [a, b] = await Promise.all([first, sameBatch]);
  assert.equal(a.configRevision, 1);
  assert.notStrictEqual(a.accounts, b.accounts);
  await nextTurn();
  assert.equal(children.calls.length, 3);
  children.calls[2].finish(snapshot(2));
  assert.deepEqual((await Promise.all([later, sameLaterBatch])).map(row => row.configRevision), [2, 2]);

  const retry = list();
  await nextTurn();
  children.calls[3].finish(snapshot(3));
  assert.equal((await retry).configRevision, 3);
  assert.equal(children.peak, 2);
});

test('credential presence, revision checks and mutations bypass queued metadata listings', async t => {
  const children = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: children.spawnProcess });
  t.after(() => catalog.dispose());
  const plain = { action: 'list', expectedRevision: 0 } as const;
  const read = catalog.providerAccountCommand(plain);
  await nextTurn();
  const after = catalog.providerAccountCommand(plain);
  const presence = catalog.providerAccountCommand(plain, undefined, []);
  const pinned = catalog.providerAccountCommand({ ...plain, expectedRevision: 8 });
  const mutation = catalog.providerAccountCommand({ action: 'enable', accountId: uuid, enabled: false, expectedRevision: 8 });
  const snapshot = (configRevision: number) => ({ storeScope: 'owner', configRevision, accounts: [], health: [] });
  await nextTurn();
  children.calls[1].finish(snapshot(7));
  assert.equal((await presence).configRevision, 7);
  await nextTurn();
  children.calls[2].finish(snapshot(8));
  assert.equal((await pinned).configRevision, 8);
  await nextTurn();
  children.calls[3].finish(snapshot(9));
  assert.equal((await mutation).configRevision, 9);
  children.calls[0].finish({ error: 'failed' });
  await assert.rejects(read, /Provider account operation failed/);
  await nextTurn();
  children.calls[4].finish(snapshot(9));
  assert.equal((await after).configRevision, 9);
});

test('real warm worker reads current workspace settings and preserves native merge errors',
  { skip: !process.env.LINGXI_SESSION_CATALOG_TEST_BIN, timeout: 60_000 }, async t => {
    const cwd = project(t), home = join(cwd, 'config');
    mkdirSync(home);
    const file = join(home, 'settings.json');
    let spawns = 0;
    const options = { serverBin: process.env.LINGXI_SESSION_CATALOG_TEST_BIN, configHome: home };
    const catalog = new ProjectSessionCatalog({ ...options, spawnProcess: ((...args: Parameters<typeof spawn>) => {
      spawns++;
      return spawn(...args);
    }) as typeof spawn });
    const oneshot = new ProjectSessionCatalog({ ...options, catalogWorker: false });
    t.after(() => { catalog.dispose(); oneshot.dispose(); });
    await catalog.listComplete(cwd);
    for (const value of ['first', 'updated']) {
      writeFileSync(file, JSON.stringify({ env: { TEST_RESUME_SETTING: value } }));
      const actual = await catalog.workspaceSettings(cwd);
      assert.deepEqual(actual, await oneshot.workspaceSettings(cwd));
      assert.equal(actual.environment.TEST_RESUME_SETTING, value);
    }
    assert.equal(spawns, 1);
    writeFileSync(file, '{');
    await assert.rejects(catalog.workspaceSettings(cwd), /workspace environment settings are unavailable/);
  });

test('history pages reuse their worker and validate that cursors advance', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const before = { uuid }, path = `/source/${uuid}.jsonl`;
  const page = catalog.readHistory(cwd, uuid, path, before);
  await nextTurn();
  const worker = process.calls[0];
  assert.equal(worker.queries[0].transcriptPath, path);
  assert.deepEqual(worker.queries[0].historyBefore, before);
  const next = { uuid: '22222222-3333-4444-8555-666666666666', offset: 10, identity: '1:2' };
  worker.reply(JSON.stringify({ messages: [], before: next }));
  assert.deepEqual(await page, { messages: [], before: next });
  const invalid = catalog.readHistory(cwd, uuid, path, next);
  await nextTurn();
  assert.equal(process.calls.length, 1);
  worker.reply(JSON.stringify({ messages: [], before: next }));
  await assert.rejects(invalid, /cursor did not advance/);
});

test('history advances across repeated compact UUIDs only within the same file and decreasing byte offsets', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const path = `/source/${uuid}.jsonl`, before = { uuid, offset: 200, identity: '1:2' };
  const page = catalog.readHistory(cwd, uuid, path, before);
  await nextTurn();
  const worker = process.calls[0], next = { ...before, offset: 100 };
  worker.reply(JSON.stringify({ messages: [], before: next }));
  assert.deepEqual((await page).before, next, 'a repeated durable UUID may represent an earlier persisted copy');
  for (const invalid of [{ ...next, offset: 200 }, { ...next, identity: '1:3' }, { uuid }]) {
    const request = catalog.readHistory(cwd, uuid, path, next);
    await nextTurn();
    worker.reply(JSON.stringify({ messages: [], before: invalid }));
    await assert.rejects(request, /cursor did not advance/);
  }
});

test('held history for A cannot block exact selection for B across projects, and equal requests coalesce', async t => {
  const cwd = project(t), otherCwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const idB = '22222222-3333-4444-8555-666666666666';
  let historyFinished = false;
  const history = catalog.readHistory(cwd, uuid, row().path, { uuid }).then(page => { historyFinished = true; return page; });
  const sameHistory = catalog.readHistory(cwd, uuid, row().path, { uuid });
  await nextTurn();
  const historyWorker = process.calls[0];
  const selection = catalog.select(otherCwd, idB, { scope: 'repository', transcriptPath: row(idB).path });
  await nextTurn();
  assert.equal(process.calls.length, 2);
  const foregroundWorker = process.calls[1];
  assert.deepEqual(foregroundWorker.queries[0], { version: 2, id: 1, cwd: otherCwd, sessionId: idB,
    scope: 'repository', transcriptPath: row(idB).path });
  foregroundWorker.reply(output(idB));
  assert.equal((await selection)?.uuid, idB);
  assert.equal(historyFinished, false, 'B finishes while A remains held');
  assert.equal(historyWorker.queries.length, 1, 'equal history cursors share one in-flight query');
  const first = catalog.find(otherCwd, idB), same = catalog.find(otherCwd, idB);
  await nextTurn();
  assert.equal(foregroundWorker.queries.length, 2, 'equal foreground lookups retain their existing coalescing');
  foregroundWorker.reply(output(idB));
  assert.deepEqual(await first, await same);
  historyWorker.reply(JSON.stringify({ messages: [], before: null }));
  assert.deepEqual(await history, await sameHistory);
  assert.equal(process.peak, 2);
});

test('concurrent exact selection shares one read for the same scope and transcript', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const selection = { scope: 'current', transcriptPath: row().path } as const;
  const otherPath = `/other/${uuid}.jsonl`;
  const first = catalog.select(cwd, uuid, selection);
  const same = catalog.select(cwd, uuid, selection);
  const otherScope = catalog.select(cwd, uuid, { ...selection, scope: 'all' });
  const otherSource = catalog.select(cwd, uuid, { ...selection, transcriptPath: otherPath });

  await nextTurn();
  const worker = process.calls[0];
  worker.reply(output(uuid, 'current'));
  await nextTurn();
  assert.equal(worker.queries[1].scope, 'all');
  const [firstRow, sameRow] = await Promise.all([first, same]);
  assert.equal(firstRow?.title, 'current');
  assert.deepEqual(firstRow, sameRow);
  assert.notEqual(firstRow, sameRow);
  worker.reply(output(uuid, 'all'));
  assert.equal((await otherScope)?.title, 'all');

  await nextTurn();
  assert.equal(worker.queries[2].transcriptPath, otherPath);
  worker.reply(JSON.stringify({ version: 1, sessions: [{ ...row(), path: otherPath }] }));
  assert.equal((await otherSource)?.path, otherPath);

  const fresh = catalog.select(cwd, uuid, selection);
  await nextTurn();
  assert.equal(worker.queries.length, 4);
  worker.reply(output(uuid, 'updated'));
  assert.equal((await fresh)?.title, 'updated');
});

test('selected transcripts resolve while directory scans are pending, in selection order', async t => {
  const cwd = project(t), other = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const selection = { scope: 'repository', transcriptPath: row().path } as const;
  const firstScan = catalog.list(cwd);
  const nextScan = catalog.list(cwd, 'all');
  const first = catalog.select(other, uuid, selection);
  const same = catalog.select(other, uuid, selection);
  const changedScope = catalog.select(other, uuid, { ...selection, scope: 'all' });
  await nextTurn();
  assert.equal(process.calls.length, 2);
  const [scans, selections] = process.calls;
  assert.deepEqual(scans.queries, [{ version: 2, id: 1, cwd, scope: 'current' }]);
  assert.deepEqual(selections.queries, [{ version: 2, id: 1, cwd: other, sessionId: uuid, ...selection }]);
  selections.reply(output(uuid, 'selected source'));
  assert.deepEqual((await Promise.all([first, same])).map(hit => hit?.title), ['selected source', 'selected source']);
  await nextTurn();
  assert.equal(selections.queries.length, 2);
  assert.equal(selections.queries[1].scope, 'all');
  selections.reply(output(uuid, 'second selection'));
  assert.equal((await changedScope)?.title, 'second selection');
  scans.reply(output(uuid, 'directory'));
  await firstScan;
  await nextTurn();
  assert.equal(scans.queries[1].scope, 'all');
  scans.reply();
  await nextScan;
  assert.equal(process.peak, 2);
});

test('invalidation retires a selected-source worker without waiting for an older directory scan', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const selection = { scope: 'current', transcriptPath: row().path } as const;
  const oldScan = catalog.list(cwd);
  const oldSelection = catalog.select(cwd, uuid, selection);
  await nextTurn();
  const [scan, selected] = process.calls;
  catalog.invalidate(cwd);
  const refreshed = catalog.select(cwd, uuid, selection);
  selected.reply(output(uuid, 'previous revision'));
  assert.equal((await oldSelection)?.title, 'previous revision');
  await nextTurn();
  assert.equal(selected.kills, 1);
  assert.equal(process.calls.length, 3);
  const current = process.calls[2];
  assert.deepEqual(current.queries, [{ version: 2, id: 1, cwd, sessionId: uuid, ...selection }]);
  current.reply(output(uuid, 'new revision'));
  assert.equal((await refreshed)?.title, 'new revision');
  scan.reply();
  await oldScan;
  catalog.dispose();
  await nextTurn();
  assert.equal(process.live, 0);
  assert.equal(process.peak, 2);
});

test('history and foreground lanes keep one active request each and share the bounded operation budget', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const history = Array.from({ length: 511 }, (_, index) => catalog.readHistory(cwd, uuid, row().path,
    { uuid: `00000000-0000-4000-8000-${String(index).padStart(12, '0')}` }));
  const settledHistory = Promise.allSettled(history);
  const foreground = catalog.find(cwd, uuid);
  await nextTurn();
  assert.equal(process.calls.length, 2);
  assert.deepEqual(process.calls.map(call => call.queries.length), [1, 1]);
  await assert.rejects(catalog.find(cwd, '22222222-3333-4444-8555-666666666666'), /busy/);
  // An equal pending history request does not consume another queue entry.
  const same = catalog.readHistory(cwd, uuid, row().path, { uuid: '00000000-0000-4000-8000-000000000000' });
  catalog.dispose();
  const historyWorker = process.calls.find(call => call.queries[0].historyBefore)!;
  const foregroundWorker = process.calls.find(call => !call.queries[0].historyBefore)!;
  historyWorker.reply(JSON.stringify({ messages: [], before: null }));
  foregroundWorker.reply();
  assert.equal((await foreground)?.uuid, uuid);
  assert.deepEqual(await same, { messages: [], before: null });
  const results = await settledHistory;
  assert.equal(results.filter(result => result.status === 'fulfilled').length, 1);
  assert.ok(results.slice(1).every(result => result.status === 'rejected' && /disposed/.test(String(result.reason))));
  await nextTurn();
  assert.equal(process.live, 0);
  assert.equal(process.peak, 2);
});

test('raw worker framing preserves multiline JSON, escapes and lone surrogates under byte fragments', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const pending = catalog.find(cwd, uuid);
  await nextTurn();
  const title = '中文🧪 \"\\\n\ud800\udc00\ud800\udfff';
  const body = JSON.stringify({ version: 1, sessions: [row(uuid, title)] }, null, 2);
  const bytes = frame(1, body);
  assert.ok(bytes.subarray(bytes.indexOf(10) + 1, -1).equals(Buffer.from(body)), 'the frame embeds exact original JSON bytes');
  for (const byte of bytes) process.calls[0].process.stdout.write(Buffer.from([byte]));
  assert.equal((await pending)?.title, title);
});

test('worker framing rejects invalid IDs, lengths, headers, UTF-8 and delimiters before reuse', async t => {
  const cwd = project(t);
  const header = (patch: Record<string, unknown>) => Buffer.from(`${JSON.stringify({ version: 2, id: 1, bytes: 2, kind: 'output', ...patch })}\n`);
  const cases: Array<[Buffer, RegExp]> = [
    [Buffer.from('null\n'), /invalid worker response/],
    [Buffer.from('{\n'), /invalid worker JSON/],
    [header({ version: 1 }), /invalid worker response/],
    [header({ id: 0 }), /invalid worker response/],
    [header({ id: 2 }), /invalid worker response/],
    [header({ id: Number.MAX_SAFE_INTEGER + 1 }), /invalid worker response/],
    [header({ bytes: -1 }), /invalid worker response/],
    [header({ bytes: 0 }), /invalid worker response/],
    [header({ bytes: 1.5 }), /invalid worker response/],
    [header({ bytes: 8 * 1024 * 1024 + 1 }), /invalid worker response/],
    [header({ kind: 'other' }), /invalid worker response/],
    [header({ extra: true }), /invalid worker response/],
    [Buffer.alloc(4_097, 120), /header is too large/],
    [Buffer.concat([header({}), Buffer.from('{}x')]), /invalid worker frame/],
    [Buffer.concat([header({}), Buffer.from([0xc0, 0x80, 10])]), /invalid worker UTF-8/],
    [frame(1, '{}', 'error'), /invalid worker error/],
  ];
  for (const [bytes, error] of cases) {
    const process = processes();
    const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
    const pending = catalog.find(cwd, uuid);
    await nextTurn();
    const failed = assert.rejects(pending, error);
    process.calls[0].process.stdout.write(bytes);
    await failed;
    await nextTurn();
    assert.equal(process.calls[0].kills, 1);
    assert.equal(process.live, 0);
    catalog.dispose();
  }
});

test('EOF during a header or body rejects the active request and a coalesced unsolicited frame retires the worker', async t => {
  const cwd = project(t);
  for (const bytes of [Buffer.from('{"version":2'), frame(1, output()).subarray(0, -3)]) {
    const process = processes();
    const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
    const pending = catalog.find(cwd, uuid);
    await nextTurn();
    const failed = assert.rejects(pending, /worker closed/);
    process.calls[0].process.stdout.write(bytes);
    process.calls[0].close();
    await failed;
    catalog.dispose();
  }
  const process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const first = catalog.find(cwd, uuid);
  await nextTurn();
  process.calls[0].process.stdout.write(Buffer.concat([frame(1, output()), frame(2, output())]));
  assert.equal((await first)?.uuid, uuid);
  assert.equal(process.calls[0].kills, 1);
  const next = catalog.find(cwd, uuid);
  await nextTurn();
  assert.equal(process.calls.length, 2);
  process.calls[1].reply();
  await next;
});

test('default metadata worker survives expired TTL and preserves source/scope/UTF-16 payloads', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess,
    configHome: '/separate home', cacheTtlMs: 0 });
  t.after(() => catalog.dispose());
  const first = catalog.list(cwd);
  await nextTurn();
  assert.equal(process.calls.length, 1);
  const worker = process.calls[0];
  assert.deepEqual(worker.args, ['--session-catalog-worker-json']);
  assert.equal(worker.options.env.LINGXI_CONFIG_DIR, '/separate home');
  assert.equal(worker.options.env.ANTHROPIC_API_KEY, undefined);
  worker.reply();
  assert.equal((await first).sessions[0].title, row().title);
  const second = catalog.list(cwd);
  await nextTurn();
  assert.equal(process.calls.length, 1, 'TTL expiration reuses the metadata process');
  worker.reply(); await second;
  const exact = catalog.select(cwd, uuid, { scope: 'repository', transcriptPath: row().path });
  await nextTurn();
  assert.deepEqual(worker.queries.at(-1), { version: 2, id: 3, cwd, sessionId: uuid,
    scope: 'repository', transcriptPath: row().path });
  // Split even a multibyte codepoint across raw pipe chunks.
  const bytes = frame(3, output());
  for (const byte of bytes) worker.process.stdout.write(Buffer.from([byte]));
  assert.equal((await exact)?.path, row().path);
  assert.equal(process.peak, 1);
});

test('invalidation retires the captured worker and prevents an old response repopulating the catalog', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const old = catalog.list(cwd);
  await nextTurn();
  catalog.invalidate(cwd);
  assert.equal(process.calls[0].kills, 0, 'the captured response may complete');
  const current = catalog.list(cwd);
  process.calls[0].reply(output(uuid, 'old'));
  assert.equal((await old).sessions[0].title, 'old');
  await nextTurn();
  assert.equal(process.calls[0].kills, 1);
  assert.equal(process.calls.length, 2);
  process.calls[0].process.stdout.write('late output from retired source');
  process.calls[1].reply(output(uuid, 'new'));
  assert.equal((await current).sessions[0].title, 'new');
  assert.equal((await catalog.list(cwd)).sessions[0].title, 'new');
});

test('invalidation retires both captured lanes and does not coalesce new history with an older generation', async t => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const oldHistory = catalog.readHistory(cwd, uuid, row().path, { uuid });
  const oldCatalog = catalog.list(cwd);
  await nextTurn();
  catalog.invalidate();
  const currentHistory = catalog.readHistory(cwd, uuid, row().path, { uuid });
  const currentCatalog = catalog.list(cwd);
  assert.ok(process.calls.every(call => call.kills === 0), 'captured responses may finish in both lanes');
  process.calls.find(call => call.queries[0].historyBefore)!.reply(JSON.stringify({ messages: [], before: null }));
  process.calls.find(call => !call.queries[0].historyBefore)!.reply(output(uuid, 'old'));
  await Promise.all([oldHistory, oldCatalog]);
  await nextTurn();
  assert.equal(process.calls.length, 4);
  assert.ok(process.calls.slice(0, 2).every(call => call.kills === 1));
  const workers = process.calls.slice(2);
  workers.find(call => call.queries[0].historyBefore)!.reply(JSON.stringify({ messages: [], before: null }));
  workers.find(call => !call.queries[0].historyBefore)!.reply(output(uuid, 'new'));
  await Promise.all([currentHistory, currentCatalog]);
  assert.equal((await catalog.list(cwd)).sessions[0].title, 'new');
  assert.equal(process.peak, 2);
});

test('global admission caps child lifetimes and retires idle workers for a queued metadata writer', async (t) => {
  const cwd = project(t), process = processes();
  const catalogs = Array.from({ length: 3 }, () => new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess }));
  t.after(() => catalogs.forEach(catalog => catalog.dispose()));
  const reads = catalogs.map(catalog => catalog.find(cwd, uuid));
  await nextTurn();
  assert.equal(process.calls.length, 2, 'the third cold request waits for a child lease');
  process.calls[0].reply(); await reads[0];
  await nextTurn();
  assert.equal(process.calls[0].kills, 1, 'an idle metadata worker yields to the admission queue');
  assert.equal(process.calls.length, 3);
  process.calls[1].reply(); await reads[1];
  const mutation = catalogs[0].mutateMetadataSource(cwd, 'opaque source receipt', 'touch');
  await nextTurn();
  assert.equal(process.calls[1].kills, 1, 'a queued writer does not wait for the worker idle TTL');
  const writer = process.calls[3];
  assert.deepEqual(writer.args, ['--session-metadata-json']);
  writer.finish({ ok: true }); await mutation;
  process.calls[2].reply(); await reads[2];
  assert.equal(process.peak, 2);
});

test('many distinct cold session keys share bounded worker IO and retain exact lookup identities', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess });
  t.after(() => catalog.dispose());
  const ids = Array.from({ length: 256 }, (_, index) => `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`);
  const pending = ids.map(id => catalog.find(cwd, id));
  for (let index = 0; index < ids.length; index++) {
    await nextTurn();
    const worker = process.calls[0];
    assert.equal(worker.queries[index].sessionId, ids[index]);
    assert.equal(worker.queries[index].scope, undefined);
    worker.reply(output(ids[index]));
  }
  const result = await Promise.all(pending);
  assert.deepEqual(result.map(row => row?.uuid), ids);
  assert.equal(process.calls.length, 1);
  assert.equal(process.peak, 1);
});

test('a queued writer expires before launch when both child leases remain busy', async (t) => {
  const cwd = project(t), process = processes();
  const readers = Array.from({ length: 2 }, () => new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess }));
  const writer = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, timeoutMs: 5 });
  t.after(() => { readers.forEach(reader => reader.dispose()); writer.dispose(); });
  const pending = readers.map(reader => reader.find(cwd, uuid));
  await nextTurn();
  const keepAlive = setTimeout(() => {}, 2_000);
  try { await assert.rejects(writer.mutateMetadataSource(cwd, 'not-yet-written', 'touch'), /busy/); }
  finally { clearTimeout(keepAlive); }
  assert.equal(process.calls.length, 2, 'the queued write never acquired a child');
  for (const call of process.calls) call.reply();
  await Promise.all(pending);
  await nextTurn();
  assert.equal(process.calls.length, 2, 'expired admission cannot start a late durable write');
});

test('worker timeout retires the child before a later request starts cleanly', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, timeoutMs: 15 });
  t.after(() => catalog.dispose());
  const keepAlive = setTimeout(() => {}, 2_000);
  const pending = catalog.find(cwd, uuid);
  await nextTurn();
  process.calls[0].process.stdout.write(`${JSON.stringify({ version: 2, id: 1, bytes: 100, kind: 'output' })}\n{`);
  try { await assert.rejects(pending, /timed out/); }
  finally { clearTimeout(keepAlive); }
  assert.equal(process.calls[0].kills, 1);
  const next = catalog.find(cwd, uuid);
  await nextTurn();
  assert.equal(process.calls.length, 2);
  process.calls[1].reply();
  assert.equal((await next)?.uuid, uuid);
});

test('worker idle expiry releases both lane process resources', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, workerIdleMs: 5 });
  t.after(() => catalog.dispose());
  const pending = catalog.find(cwd, uuid), history = catalog.readHistory(cwd, uuid, row().path, { uuid });
  await nextTurn();
  process.calls.find(call => !call.queries[0].historyBefore)!.reply();
  process.calls.find(call => call.queries[0].historyBefore)!.reply(JSON.stringify({ messages: [], before: null }));
  await Promise.all([pending, history]);
  await new Promise(resolve => setTimeout(resolve, 15));
  assert.ok(process.calls.every(call => call.kills === 1));
  assert.equal(process.live, 0);
});

test('worker errors and failed signals retain leases through retry and delayed close', async (t) => {
  const cwd = project(t), process = processes();
  const catalogs = Array.from({ length: 3 }, (_, index) => new ProjectSessionCatalog({ serverBin: '/bridge',
    spawnProcess: process.spawnProcess, timeoutMs: index === 2 ? 20 : 1_000 }));
  t.after(() => { catalogs.forEach(catalog => catalog.dispose()); process.calls.forEach(call => call.close()); });
  const first = catalogs[0].find(cwd, uuid), second = catalogs[1].find(cwd, uuid);
  void first.catch(() => {}); void second.catch(() => {});
  await nextTurn();
  const child = process.calls[0].process;
  const signals: NodeJS.Signals[] = [];
  child.kill = (signal: NodeJS.Signals) => {
    signals.push(signal);
    // Node can emit an error for failed signal delivery while the child lives.
    child.emit('error', new Error('kill EPERM'));
    return false;
  };
  const failed = assert.rejects(first, /kill EPERM/);
  child.emit('error', new Error('kill EPERM'));
  await failed;
  const keepAlive = setTimeout(() => {}, 2_000);
  try { await assert.rejects(catalogs[2].find(cwd, uuid), /busy.*not closed/); }
  finally { clearTimeout(keepAlive); }
  assert.equal(process.calls.length, 2, 'an error event is not a reaped child');
  await new Promise(resolve => setTimeout(resolve, 120));
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL'], 'failed termination gets a bounded retry without an unhandled second error');
  assert.equal(process.calls.length, 2);
  // Give the next writer a longer admission window than the failed lookup.
  const writer = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, timeoutMs: 1_000 });
  t.after(() => writer.dispose());
  const write = writer.mutateMetadataSource(cwd, 'opaque receipt', 'touch');
  await nextTurn();
  assert.equal(process.calls.length, 2);
  process.calls[0].close();
  await nextTurn();
  assert.deepEqual(process.calls[2].args, ['--session-metadata-json']);
  process.calls[2].finish({ ok: true }); await write;
  process.calls[1].reply(); await second;
  assert.equal(process.peak, 2);
});

test('oneshot error-before-close and throwing or failed kill cannot admit a third child', async (t) => {
  const cwd = project(t), process = processes();
  const catalogs = Array.from({ length: 3 }, (_, index) => new ProjectSessionCatalog({ serverBin: '/bridge', catalogWorker: false,
    spawnProcess: process.spawnProcess, timeoutMs: index === 2 ? 20 : 1_000 }));
  t.after(() => { catalogs.forEach(catalog => catalog.dispose()); process.calls.forEach(call => call.close()); });
  const first = catalogs[0].list(cwd), second = catalogs[1].list(cwd);
  void first.catch(() => {}); void second.catch(() => {});
  const child = process.calls[0].process;
  const signals: NodeJS.Signals[] = [];
  child.kill = (signal: NodeJS.Signals) => {
    signals.push(signal);
    if (signal === 'SIGTERM') throw new Error('signal EPERM');
    child.emit('error', new Error('signal EPERM'));
    return false;
  };
  const failed = assert.rejects(first, /read failure/);
  child.emit('error', new Error('read failure'));
  await failed;
  const keepAlive = setTimeout(() => {}, 2_000);
  try { await assert.rejects(catalogs[2].mutateMetadataSource(cwd, 'never written', 'touch'), /busy.*not closed/); }
  finally { clearTimeout(keepAlive); }
  assert.equal(process.calls.length, 2, 'failed signalling still owns its process lease');
  await new Promise(resolve => setTimeout(resolve, 120));
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
  process.calls[1].finish({ version: 1, sessions: [row()] }); await second;
  await nextTurn();
  assert.equal(process.calls.length, 2, 'expired queued writer never launches later');
  process.calls[0].close();
  assert.equal(process.peak, 2);
});

test('disposal cancels retries on exit and queued writers still wait for close', async (t) => {
  const cwd = project(t), process = processes();
  const readers = Array.from({ length: 2 }, () => new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess }));
  const writer = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, timeoutMs: 1_000 });
  t.after(() => { readers.forEach(reader => reader.dispose()); writer.dispose(); process.calls.forEach(call => call.close()); });
  const first = readers[0].find(cwd, uuid), second = readers[1].find(cwd, uuid);
  await nextTurn();
  process.calls[0].reply(); process.calls[1].reply();
  await Promise.all([first, second]);
  const signals: NodeJS.Signals[] = [];
  process.calls[0].process.kill = (signal: NodeJS.Signals) => { signals.push(signal); return false; };
  process.calls[1].process.kill = () => true; // Signal accepted, close deliberately delayed.
  readers[0].dispose();
  process.calls[0].process.emit('exit', 0, null);
  const write = writer.mutateMetadataSource(cwd, 'waiting for actual close', 'touch');
  await nextTurn();
  assert.equal(process.calls.length, 2, 'exit and successful signal delivery do not release undrained child leases');
  process.calls[1].close();
  await nextTurn();
  assert.deepEqual(process.calls[2].args, ['--session-metadata-json']);
  process.calls[2].finish({ ok: true }); await write;
  await new Promise(resolve => setTimeout(resolve, 120));
  assert.deepEqual(signals, ['SIGTERM'], 'never signal the handle again after its exit event');
  process.calls[0].close();
  assert.equal(process.peak, 2);
  await assert.rejects(readers[0].find(cwd, uuid), /disposed/);
});

test('real exact-open worker previews the newest page only within the selected scope',
  { skip: !process.env.LINGXI_SESSION_CATALOG_TEST_BIN, timeout: 60_000 }, async t => {
    const cwd = project(t), foreign = project(t), home = join(cwd, 'isolated-home');
    const path = join(home, 'projects', cwd.replace(/[^a-zA-Z0-9]/g, '-'), `${uuid}.jsonl`);
    const ids = Array.from({ length: 600 }, (_, index) => `10000000-0000-4000-8000-${String(index).padStart(12, '0')}`);
    const rows = ids.map((id, index) => ({
      type: 'user', uuid: id, sessionId: uuid, cwd, isSidechain: false, parentUuid: index ? ids[index - 1] : null,
      timestamp: '2026-11-12T00:00:00Z', message: { role: 'user', content: `message ${index}` },
    }));
    const mode = { type: 'fixed-session-mode', sessionId: uuid, sessionMode: 'code', instructionEpoch: 0 };
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, [mode, ...rows].map(row => JSON.stringify(row)).join('\n') + '\n');
    const catalog = new ProjectSessionCatalog({ serverBin: process.env.LINGXI_SESSION_CATALOG_TEST_BIN, configHome: home });
    t.after(() => catalog.dispose());
    const selection = { scope: 'current', transcriptPath: path } as const;
    const result = await catalog.selectForOpen(cwd, uuid, selection);
    assert.equal(result.session?.path, path);
    const preview = await result.preview;
    assert.deepEqual(preview?.messages.map(row => row.transcript_uuid), ids.slice(-60));
    assert.equal(preview?.before?.uuid, ids[540]);
    assert.deepEqual(await catalog.selectForOpen(foreign, uuid, selection), { session: undefined });
  });

test('real bridge history worker pages across compacts without gaps while the source grows',
  { skip: !process.env.LINGXI_SESSION_CATALOG_TEST_BIN, timeout: 60_000 }, async t => {
    const cwd = project(t), home = join(cwd, 'isolated-home'), path = join(cwd, `${uuid}.jsonl`);
    const ids = Array.from({ length: 182 }, (_, index) => `10000000-0000-4000-8000-${String(index).padStart(12, '0')}`);
    const rows = ids.map((id, index) => ({
      uuid: id, sessionId: uuid, timestamp: '2026-11-12T00:00:00Z', parentUuid: index ? ids[index - 1] : null,
      ...(index === 80 || index === 161
        ? { type: 'system', subtype: 'compact_boundary', content: 'compact', compactMetadata: {} }
        : { type: 'user', message: { role: 'user', content: `message ${index} 中文` } }),
    }));
    writeFileSync(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    let spawns = 0;
    const catalog = new ProjectSessionCatalog({ serverBin: process.env.LINGXI_SESSION_CATALOG_TEST_BIN, configHome: home,
      spawnProcess: ((...args: Parameters<typeof spawn>) => { spawns++; return spawn(...args); }) as typeof spawn });
    t.after(() => catalog.dispose());
    let before: { uuid: string; offset?: number | null; identity?: string | null } | null = { uuid: ids[161] };
    const pages: string[][] = [];
    while (before) {
      assert.ok(pages.length < 8, 'pagination must terminate');
      const page = await catalog.readHistory(cwd, uuid, path, before);
      pages.unshift(page.messages.map(message => message.transcript_uuid!));
      assert.ok(page.messages.every(message => message.sent_at_ms === Date.parse('2026-11-12T00:00:00Z')));
      before = page.before;
      if (pages.length === 1) appendFileSync(path, JSON.stringify({ type: 'custom-title', sessionId: uuid, customTitle: 'still streaming' }) + '\n');
    }
    assert.deepEqual(pages.flat(), ids.slice(0, 161));
    assert.equal(spawns, 1, 'all pages share a worker without account or runtime initialization');
  });

test('real history worker preserves Agent payloads and child identity across the page boundary',
  { skip: !process.env.LINGXI_SESSION_CATALOG_TEST_BIN, timeout: 60_000 }, async t => {
    const cwd = project(t), path = join(cwd, `${uuid}.jsonl`);
    const ids = Array.from({ length: 62 }, (_, index) => `10000000-0000-4000-8000-${String(index).padStart(12, '0')}`);
    const rows = ids.map((id, index) => ({ uuid: id, sessionId: uuid, parentUuid: index ? ids[index - 1] : null,
      timestamp: '2026-11-12T00:00:00Z',
      ...(index === 0 ? { type: 'assistant', message: { role: 'assistant', content: [{ type: 'tool_use', id: 'agent', name: 'Agent', input: { description: 'Inspect source' } }] } }
        : index === 1 ? { type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'agent', content: 'model-facing text' }] }, toolUseResult: { agentId: 'saved-child', status: 'completed' } }
          : { type: 'user', message: { role: 'user', content: `message ${index}` } }),
    }));
    const source = rows.map(row => JSON.stringify(row)).join('\n') + '\n';
    writeFileSync(path, source);
    const catalog = new ProjectSessionCatalog({ serverBin: process.env.LINGXI_SESSION_CATALOG_TEST_BIN, configHome: join(cwd, 'isolated-home') });
    t.after(() => catalog.dispose());
    const newer = await catalog.readHistory(cwd, uuid, path, { uuid: ids[61] });
    assert.equal(newer.messages.length, 59);
    assert.ok(newer.before);
    const older = await catalog.readHistory(cwd, uuid, path, newer.before);
    assert.equal(older.messages.length, 2);
    assert.equal(older.before, null);
    const item = prependHistory(conversationFromMessages(newer.messages), older.messages, 'older').items.find(item => item.id === 'agent');
    assert.equal(item?.type, 'tool');
    if (item?.type !== 'tool') return;
    assert.equal(item.tool, 'Agent');
    assert.equal(item.agentId, 'saved-child');
    assert.deepEqual(item.nativeInput, { description: 'Inspect source' });
    assert.deepEqual(item.nativeOutput, { agentId: 'saved-child', status: 'completed' });
    assert.equal(readFileSync(path, 'utf8'), source, 'display paging cannot change the durable agent transcript');
  });

test('real bridge worker refreshes over 128 saved sessions and revalidates additions, revisions and removals',
  { skip: !process.env.LINGXI_SESSION_CATALOG_TEST_BIN, timeout: 60_000 }, async (t) => {
    const cwd = project(t), home = join(cwd, 'isolated-home');
    const bucket = cwd.replace(/[^a-zA-Z0-9]/g, '-');
    const ids = Array.from({ length: 256 }, (_, index) => `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`);
    function save(id: string) {
      const path = join(home, 'projects', bucket, `${id}.jsonl`);
      mkdirSync(dirname(path), { recursive: true });
      const fixed = { type: 'fixed-session-mode', sessionId: id, sessionMode: 'code', instructionEpoch: 0 };
      const message = { type: 'user', uuid: `10000000-0000-4000-8000-${id.slice(-12)}`, parentUuid: null,
        sessionId: id, timestamp: '2026-11-10T00:00:00Z', cwd, isSidechain: false,
        message: { role: 'user', content: `session ${id}` } };
      writeFileSync(path, `${JSON.stringify(fixed)}\n${JSON.stringify(message)}\n`);
      return path;
    }
    const paths = ids.map(save);
    let spawns = 0;
    const catalog = new ProjectSessionCatalog({ serverBin: process.env.LINGXI_SESSION_CATALOG_TEST_BIN,
      configHome: home, cacheTtlMs: 0, spawnProcess: ((...args: Parameters<typeof spawn>) => { spawns++; return spawn(...args); }) as typeof spawn });
    t.after(() => catalog.dispose());
    assert.equal((await catalog.listComplete(cwd)).sessions.length, 256);
    assert.equal((await catalog.listComplete(cwd)).sessions.length, 256);
    assert.equal(spawns, 1, 'both uncached Desktop refreshes use one metadata process');
    appendFileSync(paths[0], `${JSON.stringify({ type: 'custom-title', sessionId: ids[0], customTitle: 'revised title' })}\n`);
    const added = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'; save(added);
    unlinkSync(paths[1]);
    const refreshed = await catalog.listComplete(cwd);
    assert.equal(refreshed.sessions.length, 256);
    assert.equal(refreshed.sessions.find(row => row.uuid === ids[0])?.title, 'revised title');
    assert.ok(refreshed.sessions.some(row => row.uuid === added));
    assert.ok(!refreshed.sessions.some(row => row.uuid === ids[1]));
    assert.equal(spawns, 1);
    const emptyId = 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff';
    const emptyPath = join(home, 'projects', bucket, `${emptyId}.jsonl`);
    writeFileSync(emptyPath, `${JSON.stringify({ type: 'fixed-session-mode', sessionId: emptyId, sessionMode: 'code', instructionEpoch: 0 })}\n${JSON.stringify({
      type: 'custom-title', sessionId: emptyId, customTitle: 'empty anchor', mobileEmptySession: 1, cwd,
    })}\n`);
    for (let refresh = 0; refresh < 2; refresh++) {
      const empty = (await catalog.listComplete(cwd)).sessions.find(row => row.uuid === emptyId);
      assert.equal(empty?.empty_session, true);
    }
    appendFileSync(emptyPath, `${JSON.stringify({ type: 'user', uuid: 'cccccccc-dddd-4eee-8fff-000000000000', parentUuid: null,
      sessionId: emptyId, timestamp: '2026-11-10T00:00:00Z', cwd, isSidechain: false,
      message: { role: 'user', content: 'first real turn' } })}\n`);
    assert.equal((await catalog.listComplete(cwd)).sessions.find(row => row.uuid === emptyId)?.empty_session, false);
    assert.equal(spawns, 1);
  });
