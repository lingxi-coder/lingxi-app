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
