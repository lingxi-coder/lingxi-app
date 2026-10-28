import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { waitForRtcFixture } from './fixtures/remote-rtc-process.mjs';

async function withChild(executable, args, check, timeouts = {}) {
  const child = spawn(executable, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const closed = new Promise((resolve) => child.once('close', resolve));
  try {
    await check(waitForRtcFixture(child, { startupTimeoutMs: 1500, fixtureTimeoutMs: 5000, ...timeouts }));
  } finally {
    if (child.pid !== undefined && child.exitCode === null && child.signalCode === null) child.kill();
    await closed;
  }
}

const withNode = (script, check, timeouts) => withChild(process.execPath, ['-e', script], check, timeouts);

test('RTC fixture launch errors fail instead of skipping', async () => {
  const missing = join(tmpdir(), `missing-rtc-electron-${randomUUID()}`, 'electron');
  await withChild(missing, [], (result) => assert.rejects(result, /Electron could not be launched/));
});

test('RTC fixture exit before exact READY fails with its diagnostics', async () => {
  await withNode("process.stdout.write('NOT_READY\\n'); process.stderr.write('startup failed\\n'); process.exitCode = 17;", async (result) => {
    await assert.rejects(result, (error) => /exited 17 before READY/.test(error.message) && /startup failed/.test(error.message));
  });
});

test('RTC fixture startup timeout is an error and terminates the child', async () => {
  await withNode('setInterval(() => {}, 1000)', (result) => assert.rejects(result, /did not report READY/), { startupTimeoutMs: 250 });
});

test('RTC fixture exit after READY without a passing result fails', async () => {
  await withNode("process.stdout.write('READY\\n'); process.exitCode = 9;", (result) => assert.rejects(result, /exited 9 without a passing result/));
});

test('RTC fixture timeout after READY fails and terminates the child', async () => {
  await withNode("process.stdout.write('READY\\n'); setInterval(() => {}, 1000)", (result) => assert.rejects(result,
    (error) => /RTC fixture timed out/.test(error.message) && /Output: READY/.test(error.message)),
  { startupTimeoutMs: 5000, fixtureTimeoutMs: 2500 });
});

test('RTC fixture reports assertion failures after READY as failures', async () => {
  await withNode("process.stdout.write('READY\\n' + JSON.stringify({passed:false,error:'permission denied check failed'}) + '\\n');", (result) => assert.rejects(result, /Fixture failed: permission denied check failed/));
});

test('RTC fixture cannot report success before READY', async () => {
  await withNode("process.stdout.write(JSON.stringify({passed:true}) + '\\n');", (result) => assert.rejects(result, /success before READY/));
});

test('RTC fixture accepts a complete READY and result across output chunks', async () => {
  await withNode("process.stdout.write('RE'); setTimeout(() => process.stdout.write('ADY\\n' + JSON.stringify({passed:true,bytes:42}) + '\\n'), 20);", async (result) => {
    assert.deepEqual(await result, { passed: true, bytes: 42 });
  });
});
