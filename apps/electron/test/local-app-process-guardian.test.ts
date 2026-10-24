import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { guardedProcessArguments } from '../src/main/local-apps/processGuardian.ts';

test('guardian preserves command status and closes with the command', { skip: process.platform === 'win32', timeout: 10000 }, async () => {
  const child = spawn('/bin/bash', guardedProcessArguments('printf "completed"; exit 17'), { detached: true, stdio: ['ignore', 'pipe', 'pipe', 'pipe'] });
  let text = ''; child.stdout!.on('data', chunk => { text += chunk; });
  assert.equal(await new Promise(resolve => child.once('close', resolve)), 17);
  assert.equal(text, 'completed');
});

test('guardian reaps only its app process group after abrupt host death', { skip: process.platform === 'win32', timeout: 15000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-parent-death-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const parentSource = join(root, 'parent.mjs'), receipt = join(root, 'child.json');
  const module = pathToFileURL(resolve('src/main/local-apps/processGuardian.ts')).href;
  await fs.writeFile(parentSource, `import {spawn} from 'node:child_process'; import {writeFileSync} from 'node:fs'; import {guardedProcessArguments} from ${JSON.stringify(module)};
const app = spawn('/bin/bash', guardedProcessArguments(${JSON.stringify('/bin/bash -c \'trap "" TERM; exec /bin/sleep 120\' & wait')}), {detached:true,stdio:['ignore','pipe','pipe','pipe']});
app.once('spawn',()=>writeFileSync(${JSON.stringify(receipt)},JSON.stringify({pid:app.pid}))); setInterval(()=>{},1000);`);
  const host = spawn(process.execPath, ['--import', resolve('../../packages/bridge-client/node_modules/tsx/dist/loader.mjs'), parentSource], { stdio: 'ignore' });
  let appPid: number | undefined;
  t.after(() => { host.kill('SIGKILL'); if (appPid) { try { process.kill(-appPid, 'SIGKILL'); } catch { /* already reaped */ } } });
  for (let i = 0; i < 100; i++) {
    try { appPid = JSON.parse(await fs.readFile(receipt, 'utf8')).pid; break; } catch { await delay(30); }
  }
  assert.ok(appPid, 'isolated host starts an owned app');
  process.kill(appPid!, 0);
  host.kill('SIGKILL');
  for (let i = 0; i < 150; i++) {
    try { process.kill(-appPid!, 0); } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return; throw error; }
    await delay(40);
  }
  assert.fail('app process group survived its parent-owned pipe closing');
});
