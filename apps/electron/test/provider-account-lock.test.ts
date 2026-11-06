import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, type TestContext } from 'node:test';
import { withProviderAccountLock } from '../src/main/provider-accounts.ts';

const account = 'acct.11111111-1111-4111-8111-111111111111';
const loader = fileURLToPath(new URL('../../../packages/bridge-client/node_modules/tsx/dist/loader.mjs', import.meta.url));
const worker = fileURLToPath(new URL('./fixtures/provider-account-lock-worker.mjs', import.meta.url));
async function directory(t: TestContext) {
  const root = await mkdtemp(join(tmpdir(), 'provider-lock-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}
function run(t: TestContext, root: string, mode: string) {
  const child = spawn(process.execPath, ['--import', loader, worker, root, account, mode], { stdio: ['ignore', 'ignore', 'pipe', 'ipc'] });
  const messages: Array<{ phase: string; code?: string; remaining?: string[] }> = [];
  let stderr = '';
  child.stderr!.on('data', bytes => { stderr += bytes; });
  child.on('message', message => messages.push(message as typeof messages[number]));
  const exited = once(child, 'exit').then(([code, signal]) => ({ code, signal }));
  t.after(async () => { if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await exited; } });
  return { child, messages, exited, async phase(name: string) {
    for (let i = 0; i < 500; i++) {
      const found = messages.find(message => message.phase === name);
      if (found) return found;
      if (child.exitCode !== null || child.signalCode !== null) assert.fail(`worker exited before ${name}: ${stderr}`);
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.fail(`worker did not reach ${name}: ${stderr}`);
  } };
}
async function kill(child: ChildProcess, exited: Promise<{ signal: unknown }>) {
  assert.equal(child.kill('SIGKILL'), true);
  assert.equal((await exited).signal, 'SIGKILL');
}

test('independent processes publish complete locks and exclusively update shared state', async t => {
  const root = await directory(t);
  const jobs = Array.from({ length: 8 }, () => run(t, root, 'counter'));
  for (const job of jobs) { await job.phase('done'); assert.equal((await job.exited).code, 0); }
  assert.equal(await readFile(join(root, 'counter'), 'utf8'), '8');
  assert.deepEqual(await readdir(root), ['counter']);
});

test('a process killed before atomic publication leaves no account lock', async t => {
  const root = await directory(t), job = run(t, root, 'before_publish');
  await job.phase('before_publish');
  assert.equal((await readdir(root)).includes(account), false);
  await kill(job.child, job.exited);
  let entered = false;
  await withProviderAccountLock(root, account, async () => { entered = true; });
  assert.equal(entered, true);
  assert.equal((await readdir(root)).every(name => name.startsWith('.') && name.endsWith('.tmp')), true);
});

test('competing reclaimers recover a dead owner without removing a live successor', async t => {
  const root = await directory(t), owner = run(t, root, 'held');
  await owner.phase('acquired');
  const claim = JSON.parse(await readFile(join(root, account), 'utf8'));
  assert.equal(claim.pid, owner.child.pid);
  assert.equal(claim.version, 2);
  await kill(owner.child, owner.exited);
  const jobs = Array.from({ length: 8 }, () => run(t, root, 'counter'));
  for (const job of jobs) { await job.phase('done'); assert.equal((await job.exited).code, 0); }
  assert.equal(await readFile(join(root, 'counter'), 'utf8'), '8');
  assert.equal((await readdir(root)).includes(account), false);
  assert.equal((await readdir(root)).includes(`${account}.reclaim`), false);
});

for (const mode of ['write_failure', 'publish_failure']) test(`${mode} removes only its unpublished candidate`, async t => {
  const root = await directory(t), job = run(t, root, mode);
  const failure = await job.phase('failed');
  assert.equal(failure.code, 'EIO');
  assert.deepEqual(failure.remaining, []);
  assert.equal((await job.exited).code, 0);
  await withProviderAccountLock(root, account, async () => {});
});

test('an operation failure releases its own claim and permits the next caller', async t => {
  const root = await directory(t);
  await assert.rejects(withProviderAccountLock(root, account, async () => { throw new Error('operation failed'); }), /operation failed/);
  await withProviderAccountLock(root, account, async () => {});
  assert.deepEqual(await readdir(root), []);
});

test('an existing directory or unknown owner is preserved without interpreting old formats', async t => {
  const root = await directory(t), path = join(root, account);
  await mkdir(path);
  await writeFile(join(path, 'owner'), String(process.pid));
  await assert.rejects(withProviderAccountLock(root, account, async () => assert.fail('entered')), /needs recovery/);
  assert.equal(await readFile(join(path, 'owner'), 'utf8'), String(process.pid));
  await rm(path, { recursive: true });
  await writeFile(path, 'unknown owner', { mode: 0o600 });
  await assert.rejects(withProviderAccountLock(root, account, async () => assert.fail('entered')), /needs recovery/);
  assert.equal(await readFile(path, 'utf8'), 'unknown owner');
});

test('a crashed recovery gate is preserved instead of guessing which account claim it protected', async t => {
  const root = await directory(t), owner = run(t, root, 'held');
  await owner.phase('acquired');
  const claim = await readFile(join(root, account), 'utf8');
  await kill(owner.child, owner.exited);
  await writeFile(join(root, `${account}.reclaim`), claim, { mode: 0o600 });
  await assert.rejects(withProviderAccountLock(root, account, async () => assert.fail('entered')), /needs recovery/);
  assert.equal(await readFile(join(root, account), 'utf8'), claim);
  assert.equal(await readFile(join(root, `${account}.reclaim`), 'utf8'), claim);
});


test('a normal owner handoff between stat and open retries without misreporting recovery', async t => {
  const root = await directory(t), first = run(t, root, 'held');
  await first.phase('acquired');
  const waiting = run(t, root, 'pause_read');
  await waiting.phase('reading_owner');
  first.child.send('release');
  await first.phase('done');
  await first.exited;
  const second = run(t, root, 'held');
  await second.phase('acquired');
  waiting.child.send('resume');
  await waiting.phase('opened_owner');
  second.child.send('release');
  await second.phase('done');
  await waiting.phase('done');
  assert.equal((await waiting.exited).code, 0);
  assert.equal(await readFile(join(root, 'counter'), 'utf8'), '3');
});
