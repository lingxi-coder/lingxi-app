import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as delay } from 'node:timers/promises';
import { test, type TestContext } from 'node:test';

import { ProjectSessionCatalog } from '../src/main/session-catalog.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const opaqueReceipt = '  {"untrusted":"雪\\n","keys":[3,1]}\nnot-a-parsed-token==  ';
const title = '新标题 "A"\nline two';

function controlledChild() {
  const process = new EventEmitter() as EventEmitter & {
    stdin: PassThrough;
    stdout: PassThrough;
    stderr: PassThrough;
    kill(): boolean;
    unref(): void;
  };
  process.stdin = new PassThrough();
  process.stdout = new PassThrough();
  process.stderr = new PassThrough();
  let input = '';
  let kills = 0;
  let unrefs = 0;
  process.stdin.on('data', (chunk: Buffer) => { input += chunk.toString(); });
  process.kill = () => { kills++; queueMicrotask(() => process.emit('close', null, 'SIGTERM')); return true; };
  process.unref = () => { unrefs++; };
  return {
    process,
    get input() { return input; },
    get kills() { return kills; },
    get unrefs() { return unrefs; },
    finish(result: unknown) {
      process.stdout.write(JSON.stringify(result));
      process.emit('close', 0, null);
    },
  };
}

type ControlledChild = ReturnType<typeof controlledChild>;
type Call = { bin: string; args: string[]; options: { cwd: string; env: NodeJS.ProcessEnv; stdio: string[]; detached: boolean }; child: ControlledChild };

function fixture(t: TestContext, options: { timeoutMs?: number; configHome?: string } = {}) {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-metadata-project-'));
  t.after(() => rmSync(project, { recursive: true, force: true }));
  const calls: Call[] = [];
  const catalog = new ProjectSessionCatalog({ serverBin: '/fixture/bridge-server', ...options,
    spawnProcess: ((bin: string, args: readonly string[], spawnOptions: Call['options']) => {
      const child = controlledChild();
      calls.push({ bin, args: [...args], options: spawnOptions, child });
      return child.process;
    }) as any,
  });
  return { project, catalog, calls };
}

async function rejectBeforeDeadline(promise: Promise<unknown>, expected: RegExp) {
  // The production observer timer is unref'ed. Keep this test alive while it
  // waits, without requiring an actual child process or an OS process lease.
  const deadline = setTimeout(() => {}, 2_000);
  try { await assert.rejects(promise, expected); }
  finally { clearTimeout(deadline); }
}

test('metadata subprocess receives the exact project, selected source and opaque receipt', async (t) => {
  const configHome = '/scope config/配置 home';
  const { project, catalog, calls } = fixture(t, { configHome });
  const transcriptPath = join(project, 'selected source', '跨空间.jsonl');
  const pending = catalog.captureMetadataSource(project, sessionId, transcriptPath, 'code');
  const capture = calls[0]!;
  assert.equal(capture.bin, '/fixture/bridge-server');
  assert.deepEqual(capture.args, ['--session-metadata-json', '--session-config-home', configHome]);
  assert.equal(capture.options.cwd, realpathSync.native(project));
  assert.equal(capture.options.env.LINGXI_CONFIG_DIR, configHome);
  assert.deepEqual(capture.options.stdio, ['pipe', 'pipe', 'pipe']);
  assert.equal(capture.options.detached, false);
  assert.equal(capture.child.input, `${JSON.stringify({ action: 'capture', sessionId, transcriptPath, sessionMode: 'code' })}\n`);
  // Separate asynchronous output events are assembled before decoding.
  const encoded = JSON.stringify({ receipt: opaqueReceipt });
  capture.child.process.stdout.write(encoded.slice(0, 10));
  await delay(0);
  capture.child.process.stdout.write(encoded.slice(10));
  capture.child.process.emit('close', 0, null);
  assert.equal(await pending, opaqueReceipt);

  for (const [action, suppliedTitle] of [['rename', title], ['touch', undefined], ['verify', undefined]] as const) {
    const pendingMutation = catalog.mutateMetadataSource(project, opaqueReceipt, action, suppliedTitle);
    const call = calls.at(-1)!;
    assert.deepEqual(call.args, capture.args);
    assert.equal(call.options.cwd, capture.options.cwd);
    assert.equal(call.options.detached, action !== 'verify');
    assert.equal(call.child.input, `${JSON.stringify({ action, receipt: opaqueReceipt,
      ...(suppliedTitle === undefined ? {} : { title: suppliedTitle }) })}\n`);
    call.child.finish({ ok: true });
    await pendingMutation;
  }
  assert.equal(calls.length, 4);
});

test('metadata subprocess uses the default configuration when no home is specified', async (t) => {
  const { project, catalog, calls } = fixture(t);
  const pending = catalog.captureMetadataSource(project, sessionId, '/another scope/session.jsonl', 'chat');
  assert.deepEqual(calls[0]!.args, ['--session-metadata-json']);
  assert.deepEqual(JSON.parse(calls[0]!.child.input), {
    action: 'capture', sessionId, transcriptPath: '/another scope/session.jsonl', sessionMode: 'chat',
  });
  calls[0]!.child.finish({ receipt: opaqueReceipt });
  assert.equal(await pending, opaqueReceipt);
});

test('metadata input and child confirmations are checked without resolving a different source', async (t) => {
  const { project, catalog, calls } = fixture(t);
  for (const [id, path] of [['not-a-uuid', '/absolute'], [sessionId, 'relative'], [sessionId, '/bad\0path']]) {
    await assert.rejects(catalog.captureMetadataSource(project, id!, path!, 'chat'), /Invalid metadata source/);
  }
  assert.equal(calls.length, 0);
  for (const invalid of [null, [], {}, { receipt: '' }, { receipt: 1 }, { receipt: 'x'.repeat(128_001) }]) {
    const pending = catalog.captureMetadataSource(project, sessionId, '/exact.jsonl', 'chat');
    const rejected = assert.rejects(pending, /Invalid (?:session metadata response|metadata source receipt)/);
    calls.at(-1)!.child.finish(invalid);
    await rejected;
  }
  for (const invalid of [null, [], {}, { ok: false }, { ok: 'true' }]) {
    const pending = catalog.mutateMetadataSource(project, opaqueReceipt, 'verify');
    const rejected = assert.rejects(pending, /Invalid session metadata (?:response|result)/);
    calls.at(-1)!.child.finish(invalid);
    await rejected;
  }
});

for (const action of ['rename', 'touch'] as const) {
  for (const reason of ['timeout', 'oversized output'] as const) {
    test(`${action} ${reason} reports unconfirmed without killing or mixing the still-draining child`, { timeout: 5_000 }, async (t) => {
      const { project, catalog, calls } = fixture(t, { timeoutMs: 30 });
      const pending = catalog.mutateMetadataSource(project, opaqueReceipt, action, action === 'rename' ? title : undefined);
      const first = calls[0]!;
      assert.equal(first.options.detached, true);
      const rejected = rejectBeforeDeadline(pending, /unconfirmed/i);
      if (reason === 'oversized output') first.child.process.stdout.write(Buffer.alloc(8 * 1024 * 1024 + 1, 0x61));
      await rejected;
      assert.equal(first.child.kills, 0);
      assert.equal(first.child.unrefs, 1);

      // An old child's late stdout, stderr and close must not settle or poison
      // another request. Data listeners still drain both old pipes.
      let subsequentSettled = false;
      const subsequent = catalog.mutateMetadataSource(project, 'second opaque receipt', 'verify')
        .finally(() => { subsequentSettled = true; });
      const second = calls[1]!;
      let stdoutDrained = 0, stderrDrained = 0;
      first.child.process.stdout.on('data', (chunk: Buffer) => { stdoutDrained += chunk.length; });
      first.child.process.stderr.on('data', (chunk: Buffer) => { stderrDrained += chunk.length; });
      first.child.process.stdout.write('late stdout');
      first.child.process.stderr.write('late stderr');
      first.child.process.emit('close', 0, null);
      await delay(0);
      assert.equal(stdoutDrained, Buffer.byteLength('late stdout'));
      assert.equal(stderrDrained, Buffer.byteLength('late stderr'));
      assert.equal(subsequentSettled, false);
      assert.equal(first.child.kills, 0);
      assert.deepEqual(JSON.parse(second.child.input), { action: 'verify', receipt: 'second opaque receipt' });
      second.child.finish({ ok: true });
      await subsequent;
      assert.equal(second.child.kills, 0);
      assert.equal(second.child.unrefs, 0);
    });
  }
}

for (const operation of ['capture', 'verify'] as const) {
  test(`read-only metadata ${operation} may kill a timed-out child`, { timeout: 5_000 }, async (t) => {
    const { project, catalog, calls } = fixture(t, { timeoutMs: 20 });
    const pending = operation === 'capture'
      ? catalog.captureMetadataSource(project, sessionId, '/exact.jsonl', 'chat')
      : catalog.mutateMetadataSource(project, opaqueReceipt, 'verify');
    await rejectBeforeDeadline(pending, /session catalog timed out/);
    assert.equal(calls[0]!.options.detached, false);
    assert.equal(calls[0]!.child.kills, 1);
    assert.equal(calls[0]!.child.unrefs, 0);
  });
}
