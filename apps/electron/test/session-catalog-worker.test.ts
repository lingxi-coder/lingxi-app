import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, realpathSync, rmSync, appendFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { test, type TestContext } from 'node:test';

import { ProjectSessionCatalog } from '../src/main/session-catalog.js';

const uuid = '11111111-2222-4333-8444-555555555555';
const row = (id = uuid, title = '会话😀\ud800') => ({ uuid: id, title, modified_rfc3339: '', message_count: 1,
  mode: 'code', path: `/source/${id}.jsonl`, empty_session: false });
const output = (id = uuid, title?: string) => JSON.stringify({ version: 1, sessions: [row(id, title)] });

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
        child.stdout.write(`${JSON.stringify({ version: 1, id: call.queries[index].id, output: value })}\n`);
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
  assert.deepEqual(worker.queries.at(-1), { version: 1, id: 3, cwd, sessionId: uuid,
    scope: 'repository', transcriptPath: row().path });
  // Split even a multibyte codepoint across raw pipe chunks.
  const bytes = Buffer.from(`${JSON.stringify({ version: 1, id: 3, output: output() })}\n`);
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
  try { await assert.rejects(catalog.find(cwd, uuid), /timed out/); }
  finally { clearTimeout(keepAlive); }
  assert.equal(process.calls[0].kills, 1);
  const next = catalog.find(cwd, uuid);
  await nextTurn();
  assert.equal(process.calls.length, 2);
  process.calls[1].reply();
  assert.equal((await next)?.uuid, uuid);
});

test('worker idle expiry releases its process resources', async (t) => {
  const cwd = project(t), process = processes();
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge', spawnProcess: process.spawnProcess, workerIdleMs: 5 });
  t.after(() => catalog.dispose());
  const pending = catalog.find(cwd, uuid);
  await nextTurn(); process.calls[0].reply(); await pending;
  await new Promise(resolve => setTimeout(resolve, 15));
  assert.equal(process.calls[0].kills, 1);
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
