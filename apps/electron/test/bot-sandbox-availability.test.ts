import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';

import { BotSandboxUnavailableError, SANDBOX_REASON, sandboxAvailability, systemSandboxHost, type SandboxHost } from '../src/main/bots/sandboxAvailability.js';
import { BOT_SANDBOX_UNAVAILABLE_MESSAGE } from '../src/shared/bots.js';

const host = (programs: readonly string[] = [], procVersion?: string): SandboxHost => ({ onPath: (program) => programs.includes(program), procVersion: () => procVersion });

// What `/proc/version` says, as the engine's own tests have it.
const WSL2 = 'Linux version 5.15.90.1-microsoft-standard-WSL2 (oe-user@oe-host) (gcc 11.2.0) #1 SMP';
const WSL1 = 'Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com) (gcc version 5.4.0 (GCC) ) #1237-Microsoft';
const PLAIN = 'Linux version 6.5.0-generic (buildd@lcy02) (gcc 13.2.0) #1 SMP PREEMPT_DYNAMIC';

test('a Mac can isolate a Bot when sandbox-exec is there and not without it', () => {
  assert.deepEqual(sandboxAvailability('darwin', host(['sandbox-exec'])), { available: true });
  assert.deepEqual(sandboxAvailability('darwin', host(['bwrap'])), { available: false, reason: SANDBOX_REASON.noSandboxExec });
});

test('Linux needs bwrap, and a socat that is missing is no reason to refuse', () => {
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap'], PLAIN)), { available: true });
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap', 'socat'], PLAIN)), { available: true });
  assert.deepEqual(sandboxAvailability('linux', host(['socat', 'sandbox-exec'], PLAIN)), { available: false, reason: SANDBOX_REASON.noBwrap });
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap'])), { available: true }, 'no /proc/version to read is not WSL');
});

test('WSL2 is Linux, and WSL1 cannot isolate a Bot whatever is installed', () => {
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap'], WSL2)), { available: true });
  assert.deepEqual(sandboxAvailability('linux', host([], WSL2)), { available: false, reason: SANDBOX_REASON.noBwrap });
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap'], WSL1)), { available: false, reason: SANDBOX_REASON.wsl1 });
  assert.deepEqual(sandboxAvailability('linux', host(['bwrap'], 'Linux version 4.4.0 (WSL2) Microsoft')), { available: true }, 'an explicit WSL2 marker wins over the name Microsoft');
});

test('Windows and every other system cannot, with nothing installed able to change that', () => {
  for (const platform of ['win32', 'freebsd', 'android', 'aix'] as const) {
    assert.deepEqual(sandboxAvailability(platform, host(['sandbox-exec', 'bwrap'], PLAIN)), { available: false, reason: SANDBOX_REASON.platform }, platform);
  }
});

test('every reason is the key of a text of the Bots catalog', async () => {
  const { botsCatalog } = await import('../src/renderer/i18n/botsCatalog.js');
  for (const key of Object.values(SANDBOX_REASON)) {
    assert.deepEqual(Object.keys(botsCatalog[key] ?? {}).sort(), ['en', 'ja', 'ko', 'zh-Hans', 'zh-Hant'], key);
  }
});

test('this computer is asked by default, through its PATH and /proc/version', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-sandbox-probe-'));
  const path = process.env['PATH'];
  t.after(() => {
    if (path === undefined) delete process.env['PATH']; else process.env['PATH'] = path;
    rmSync(root, { recursive: true, force: true });
  });
  const bin = join(root, 'bin');
  const empty = join(root, 'empty');
  mkdirSync(bin);
  mkdirSync(empty);
  writeFileSync(join(bin, 'bwrap'), '#!/bin/sh\n');
  chmodSync(join(bin, 'bwrap'), 0o755);
  writeFileSync(join(bin, 'sandbox-exec'), 'not a program');
  chmodSync(join(bin, 'sandbox-exec'), 0o644);
  mkdirSync(join(bin, 'socat'));
  process.env['PATH'] = [empty, '', bin].join(delimiter);
  assert.equal(systemSandboxHost.onPath('bwrap'), true, 'an executable file further along the PATH');
  assert.equal(systemSandboxHost.onPath('sandbox-exec'), false, 'a file that cannot be run is not a program');
  assert.equal(systemSandboxHost.onPath('socat'), false, 'nor is a folder');
  assert.equal(systemSandboxHost.onPath('missing'), false);
  process.env['PATH'] = empty;
  assert.equal(systemSandboxHost.onPath('bwrap'), false);
  delete process.env['PATH'];
  assert.equal(systemSandboxHost.onPath('bwrap'), false, 'no PATH, no program');
  const text = systemSandboxHost.procVersion();
  assert.ok(text === undefined || typeof text === 'string');
});

test('the refusal says what the window knows it by', () => {
  const error = new BotSandboxUnavailableError();
  assert.ok(error instanceof Error);
  assert.equal(error.message, BOT_SANDBOX_UNAVAILABLE_MESSAGE);
  assert.equal(error.name, 'BotSandboxUnavailableError');
});
