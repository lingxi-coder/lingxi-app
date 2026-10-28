import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalAppProcessClient } from '../src/main/local-apps/processClient.js';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.js';
import { CoreLocalAppStore } from '../src/main/local-apps/coreStore.js';
import { LocalAppRuntime, shellQuote } from '../src/main/local-apps/runtime.js';

test('slow native log storage stays bounded while output and stop continue', { skip: process.platform === 'win32', timeout: 15000 }, async () => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-log-backpressure-'));
  const release = join(root, 'release'), receipt = join(root, 'written.jsonl');
  const script = join(root, 'slow-core.cjs'), executable = join(root, 'slow-core');
  await fs.writeFile(script, `const fs=require('fs'); const readline=require('readline');
process.stdin.pause(); const timer=setInterval(()=>{if(!fs.existsSync(${JSON.stringify(release)}))return;clearInterval(timer);let first=true;
const input=readline.createInterface({input:process.stdin});input.on('line',line=>{const row=JSON.parse(line);if(first){first=false;process.stdout.write(JSON.stringify({id:row.id,ok:false,error:{message:'controlled storage failure'}})+'\\n');return;}fs.appendFileSync(${JSON.stringify(receipt)},JSON.stringify(row.request)+'\\n');process.stdout.write(JSON.stringify({id:row.id,ok:true,value:null})+'\\n');});process.stdin.resume();},10);`);
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  const core = new LocalAppCoreClient(executable, root);
  const runtime = new LocalAppRuntime(new CoreLocalAppStore(root, core), () => undefined);
  const ready = { needle: 'AAAA', matched: false, stdout: '', stderr: '' };
  const run = { id: 'run-1', operationId: 'op-1', controller: new AbortController(), phase: 'running',
    children: new Set<LocalAppProcessClient>(), services: new Map(), readiness: new Map([['service', ready]]) };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set('app-1', run);
  const helper = new LocalAppProcessClient({ executable: process.execPath, helperArgs: [resolve('test/fixtures/local-app-process-helper.mjs')],
    command: 'fixture', args: ['flood'], cwd: root, env: {}, onOutput: (stream, text) => {
      (Reflect.get(runtime, 'log') as (...args: unknown[]) => void).call(runtime, 'app-1', run, 'service', text, stream);
    } });
  run.children.add(helper);
  try {
    await helper.waitStarted();
    for (let n = 0; n < 400 && runtime.logs('app-1').length < 180; n++) await delay(10);
    assert.ok(runtime.logs('app-1').length >= 180, 'the real helper must produce enough output to saturate the queue');
    assert.equal(ready.matched, true, 'readiness must be updated while storage is blocked');
    const pending = Reflect.get(core, 'pending') as Map<string, unknown>;
    assert.ok(pending.size <= 1, `storage must have at most one log request in flight, saw ${pending.size}`);
    const queue = (Reflect.get(runtime, 'persistentLogs') as WeakMap<object, { pending: unknown[]; bytes: number; droppedEntries: number; totalDroppedEntries: number; totalDroppedBytes: number }>).get(run)!;
    assert.ok(queue.pending.length <= 64 && queue.bytes <= 256 * 1024, 'memory used for pending log records must be bounded');
    assert.ok(queue.droppedEntries > 0, 'overflow must count discarded records');
    assert.ok(runtime.logs('app-1').some(line => /persist.*log.*discard/i.test(line.text)), 'the live log must disclose truncation');
    await runtime.stop('app-1');
    assert.equal(run.phase, 'stopped', 'process stop must not wait for blocked log storage');
    const droppedAtStop = { entries: queue.totalDroppedEntries, bytes: queue.totalDroppedBytes };
    await fs.writeFile(release, 'release');
    for (let n = 0; n < 500 && (queue.pending.length || queue.droppedEntries); n++) await delay(10);
    assert.equal(queue.pending.length, 0, 'pending logs should drain after storage recovers');
    assert.equal(queue.droppedEntries, 0, 'the truncation notice must be persisted after recovery');
    const rows = (await fs.readFile(receipt, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
    const written = rows.map(row => JSON.parse(row.text));
    const notices = written.map(line => /discarded (\d+) entries \((\d+) serialized bytes\)/.exec(line.text)).filter(Boolean);
    assert.equal(notices.reduce((total, notice) => total + Number(notice![1]), 0), droppedAtStop.entries, 'stored logs must report the exact number of discarded records');
    assert.equal(notices.reduce((total, notice) => total + Number(notice![2]), 0), droppedAtStop.bytes, 'stored logs must report the exact number of discarded bytes');
    assert.ok(written.every((line, index) => index === 0 || written[index - 1].sequence < line.sequence), 'persisted records and truncation markers must remain ordered');
    assert.ok(runtime.logs('app-1').some(line => /controlled storage failure/.test(line.text)), 'storage failures are surfaced without recursively logging them');
    assert.ok(runtime.logs('app-1').some(line => /storage recovered/.test(line.text)));
    assert.equal(Reflect.get(run, 'error'), undefined, 'a recovered storage error must not remain as the run error');
  } finally {
    await fs.writeFile(release, 'release');
    await runtime.stop('app-1').catch(() => undefined);
    await core.dispose().catch(() => undefined);
    await fs.rm(root, { recursive: true, force: true });
  }
});

test('log storage failure does not overwrite an existing application error', async () => {
  let requests = 0;
  const core = { request: async () => { requests++; throw new Error('controlled storage failure'); }, dispose: async () => undefined };
  const runtime = new LocalAppRuntime(new CoreLocalAppStore(join(tmpdir(), 'unused-local-app-log'), core), () => undefined);
  const run = { id: 'run', operationId: 'operation', error: 'service failed', readiness: new Map() };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set('app', run);
  (Reflect.get(runtime, 'log') as (...args: unknown[]) => void).call(runtime, 'app', run, 'service', 'message', 'stdout');
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(requests, 1, 'reporting persistence failure must not recursively enqueue another log write');
  assert.equal(run.error, 'service failed');
  assert.ok(runtime.logs('app').some(line => /controlled storage failure/.test(line.text)));
  Reflect.set(runtime, 'disposed', true);
  const queue = (Reflect.get(runtime, 'persistentLogs') as WeakMap<object, { retry?: ReturnType<typeof setTimeout> }>).get(run);
  if (queue?.retry) clearTimeout(queue.retry);
});

test('a committed log with a lost reply is not duplicated after a real run replacement', { skip: process.platform === 'win32', timeout: 12000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-retired-log-'));
  const receipt = join(root, 'written.jsonl'), script = join(root, 'core.cjs'), executable = join(root, 'core');
  await fs.writeFile(script, `const fs=require('fs');const readline=require('readline');let first=true;
readline.createInterface({input:process.stdin}).on('line',line=>{const row=JSON.parse(line);fs.appendFileSync(${JSON.stringify(receipt)},JSON.stringify(row.request)+'\\n');if(first){first=false;return;}process.stdout.write(JSON.stringify({id:row.id,ok:true,value:null})+'\\n');});`);
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  const core = new LocalAppCoreClient(executable, root), store = new CoreLocalAppStore(root, core);
  const work = join(root, 'work'); await fs.mkdir(work); await fs.writeFile(join(work, 'index.html'), '<!doctype html>');
  const row = { id: 'app', name: 'Fixture', revision: 1, currentRevision: 1, releaseDigest: 'fixture', grants: [],
    manifest: { schema: 2, name: 'Fixture', targets: [], entry: { type: 'static', path: 'index.html' } } } as never;
  let serial = 0;
  store.get = async () => row; store.assertTarget = async () => undefined;
  store.prepareWorkspace = async () => { serial++; return work; };
  store.run = () => ({ runId: `run-${serial}` } as never);
  store.preparationDataPath = () => work; store.updateRun = async () => undefined; store.activate = async () => undefined;
  const runtime = new LocalAppRuntime(store, () => undefined);
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let timersActive = true;
  try {
    await runtime.start('app');
    for (let n = 0; n < 300; n++) { try { await fs.access(receipt); break; } catch { await delay(10); } }
    const old = (Reflect.get(runtime, 'invocations') as Map<string, { id: string }>).get('app')!;
    (Reflect.get(runtime, 'log') as (...args: unknown[]) => void).call(runtime, 'app', old, 'service', 'old-later', 'stdout');
    await runtime.start('app', false, undefined, true);
    assert.notEqual(runtime.invocationId('app'), old.id, 'the production runtime must actually hand over to the new run');
    t.mock.timers.tick(120_000); t.mock.timers.reset(); timersActive = false;
    let records: Array<{ text: string }> = [];
    for (let n = 0; n < 500; n++) {
      records = (await fs.readFile(receipt, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
      if (records.some(record => JSON.parse(record.text).text === 'old-later')) break;
      await delay(10);
    }
    const logs = records.map(record => JSON.parse(record.text));
    assert.equal(logs.filter(line => line.runId === old.id && line.text === 'Application is ready').length, 1, 'a committed append with a lost reply must never be resent');
    assert.equal(logs.filter(line => line.runId === old.id && line.text === 'old-later').length, 1, 'unattempted retired-run records must still drain');
    assert.ok(runtime.logs('app').some(line => /not confirmed|unconfirmed/i.test(line.text) && line.text.includes(old.id)), 'the old run\'s uncertain append must be visible after replacement');
  } finally {
    if (timersActive) t.mock.timers.reset();
    await runtime.dispose().catch(() => undefined); await core.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true });
  }
});

test('a committed append whose reply times out is never sent a second time', { skip: process.platform === 'win32', timeout: 10000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-ambiguous-log-'));
  const receipt = join(root, 'written.jsonl'), script = join(root, 'core.cjs'), executable = join(root, 'core');
  await fs.writeFile(script, `const fs=require('fs');const readline=require('readline');let first=true;
readline.createInterface({input:process.stdin}).on('line',line=>{const row=JSON.parse(line);fs.appendFileSync(${JSON.stringify(receipt)},JSON.stringify(row.request)+'\\n');if(first){first=false;return;}process.stdout.write(JSON.stringify({id:row.id,ok:true,value:null})+'\\n');});`);
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  const core = new LocalAppCoreClient(executable, root), runtime = new LocalAppRuntime(new CoreLocalAppStore(root, core), () => undefined);
  const run = { id: 'run-1', operationId: 'op-1', readiness: new Map() };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set('app', run);
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let timersActive = true;
  try {
    const log = Reflect.get(runtime, 'log') as (...args: unknown[]) => void;
    log.call(runtime, 'app', run, 'service', 'committed-once', 'stdout');
    log.call(runtime, 'app', run, 'service', 'after-timeout', 'stdout');
    for (let n = 0; n < 300; n++) { try { await fs.access(receipt); break; } catch { await delay(10); } }
    t.mock.timers.tick(120_000); t.mock.timers.reset(); timersActive = false;
    let records: Array<{ text: string }> = [];
    for (let n = 0; n < 500; n++) {
      records = (await fs.readFile(receipt, 'utf8')).trim().split('\n').map(line => JSON.parse(line));
      if (records.some(record => JSON.parse(record.text).text === 'after-timeout')) break;
      await delay(10);
    }
    const logs = records.map(record => JSON.parse(record.text));
    assert.equal(logs.filter(line => line.text === 'committed-once').length, 1, 'an unacknowledged append may already be durable and must not be retried');
    assert.equal(logs.filter(line => line.text === 'after-timeout').length, 1);
    assert.ok(runtime.logs('app').some(line => /not confirmed|unconfirmed/i.test(line.text)), 'the ambiguous outcome must be visible');
  } finally {
    if (timersActive) t.mock.timers.reset();
    Reflect.set(runtime, 'disposed', true); await core.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true });
  }
});

test('new and retired runs share a bounded number of retained log queues', { skip: process.platform === 'win32', timeout: 8000 }, async () => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-log-runs-'));
  const script = join(root, 'core.cjs'), executable = join(root, 'core');
  await fs.writeFile(script, 'process.stdin.pause();setInterval(()=>{},1000);');
  await fs.writeFile(executable, `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(script)} "$@"\n`, { mode: 0o700 });
  const core = new LocalAppCoreClient(executable, root), runtime = new LocalAppRuntime(new CoreLocalAppStore(root, core), () => undefined);
  const invocations = Reflect.get(runtime, 'invocations') as Map<string, unknown>;
  const log = Reflect.get(runtime, 'log') as (...args: unknown[]) => void;
  let current!: { id: string; operationId: string; controller: AbortController; phase: string; children: Set<unknown>; services: Map<string, unknown>; readiness: Map<string, unknown> };
  try {
    for (let n = 0; n < 20; n++) {
      current = { id: `run-${n}`, operationId: `op-${n}`, controller: new AbortController(), phase: 'running', children: new Set(), services: new Map(), readiness: new Map() };
      invocations.set('app', current); log.call(runtime, 'app', current, 'service', `record-${n}`, 'stdout');
    }
    for (let n = 0; n < 200; n++) log.call(runtime, 'app', current, 'service', 'discarded', 'stdout');
    const retained = Reflect.get(runtime, 'retainedPersistentLogs') as Set<unknown>;
    const pending = Reflect.get(core, 'pending') as Map<string, unknown>;
    assert.ok(retained.size <= 16, `retired and current queues must be globally bounded, saw ${retained.size}`);
    assert.ok(pending.size <= 16, `globally retained requests must be bounded, saw ${pending.size}`);
    await runtime.stop('app');
    assert.ok(runtime.logs('app').some(line => /discarded 201 entries/.test(line.text)), 'stopping an untracked run must disclose its exact discarded count');
  } finally {
    Reflect.set(runtime, 'disposed', true); await core.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true });
  }
});

test('permanent storage failure releases bounded ownership and distinguishes unknown writes from unsent drops', async t => {
  const sent: number[] = [];
  const core = { request: async (request: Record<string, unknown>) => { sent.push(JSON.parse(String(request.text)).sequence); throw new Error('storage unavailable'); }, dispose: async () => undefined };
  const runtime = new LocalAppRuntime(new CoreLocalAppStore(join(tmpdir(), 'unused-failed-log'), core), () => undefined);
  const run = { id: 'run', operationId: 'operation', readiness: new Map() };
  (Reflect.get(runtime, 'invocations') as Map<string, unknown>).set('app', run);
  const log = Reflect.get(runtime, 'log') as (...args: unknown[]) => void;
  t.mock.timers.enable({ apis: ['setTimeout'] });
  try {
    for (let n = 0; n < 5; n++) log.call(runtime, 'app', run, 'service', `line-${n}`, 'stdout');
    await new Promise<void>(resolve => setImmediate(resolve));
    for (let n = 0; n < 2; n++) { t.mock.timers.tick(1000); await new Promise<void>(resolve => setImmediate(resolve)); }
    const queue = (Reflect.get(runtime, 'persistentLogs') as WeakMap<object, { pending: unknown[]; droppedEntries: number; unconfirmedEntries: number }>).get(run)!;
    const retained = Reflect.get(runtime, 'retainedPersistentLogs') as Set<unknown>;
    assert.equal(retained.size, 0, 'repeated failures must not retain a retired queue forever');
    assert.equal(queue.pending.length, 0);
    assert.equal(queue.droppedEntries, 2, 'two never-sent records were discarded');
    assert.equal(queue.unconfirmedEntries, 3, 'three failed responses have uncertain outcomes');
    assert.equal(new Set(sent).size, sent.length, 'a sent record must never be retried');
    assert.ok(runtime.logs('app').some(line => /discarded 2 entries.*3 entries.*unconfirmed/.test(line.text)));
  } finally { t.mock.timers.reset(); Reflect.set(runtime, 'disposed', true); }
});
