#!/usr/bin/env node

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BridgeClient } from '@lingxi/bridge-client';

import {
  desktopArtifactPaths,
  formatError,
  packageRoot,
  parseDesktopTargetArgs,
  walkTree,
} from './package-support.mjs';

function delay(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

async function waitFor(predicate, label, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = predicate();
    if (value) return value;
    if (Date.now() >= deadline) throw new Error(`${label} did not complete within ${timeoutMs}ms`);
    await delay(100);
  }
}

function sanitizedEnvironment(tempRoot) {
  const env = { ...process.env };
  for (const name of [
    'ANTHROPIC_API_KEY',
    'ANTHROPIC_AUTH_TOKEN',
    'OPENAI_API_KEY',
    'LINGXI_API_BASE_URL',
    'LINGXI_BRIDGE_SERVER_BIN',
  ]) delete env[name];
  env.HOME = join(tempRoot, 'home');
  env.USERPROFILE = join(tempRoot, 'home');
  env.APPDATA = join(tempRoot, 'appdata');
  env.LOCALAPPDATA = join(tempRoot, 'localappdata');
  env.TMPDIR = join(tempRoot, 'tmp');
  env.TMP = join(tempRoot, 'tmp');
  env.TEMP = join(tempRoot, 'tmp');
  return env;
}

async function terminate(child, lockfilePath) {
  if (process.platform === 'win32') {
    const client = new BridgeClient({ lockfilePath });
    try {
      await client.connect();
      client.sendCommand({ type: 'request_exit' });
      await waitFor(() => child.exitCode !== null || child.signalCode !== null,
        'authenticated sidecar exit', 30_000);
      assert.equal(child.exitCode, 0, 'RequestExit must complete graceful process teardown');
    } finally { client.close(); }
    return;
  }
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill('SIGTERM');
  await Promise.race([
    new Promise((resolvePromise) => child.once('exit', resolvePromise)),
    delay(10_000).then(() => {
      child.kill('SIGKILL');
      throw new Error('packaged bridge-server did not exit after SIGTERM');
    }),
  ]);
}

export async function runPackagedSidecarSmoke(root, platform, arch) {
  if (process.platform !== platform || process.arch !== arch) {
    throw new Error(`native ${platform}-${arch} host required for packaged sidecar smoke`);
  }
  const paths = desktopArtifactPaths(root, platform, arch);
  const resources = platform === 'darwin'
    ? join(paths.payloadRoot, 'Contents', 'Resources')
    : join(paths.payloadRoot, 'resources');
  const sidecar = join(resources, 'bin', platform === 'win32' ? 'bridge-server.exe' : 'bridge-server');
  if (!existsSync(sidecar)) throw new Error(`packaged sidecar is missing: ${sidecar}`);

  const tempRoot = mkdtempSync(join(tmpdir(), 'lingxi-sidecar-smoke-'));
  const workspace = join(tempRoot, 'workspace');
  const bridgeDir = join(tempRoot, 'runtime');
  for (const path of [workspace, bridgeDir, join(tempRoot, 'home'), join(tempRoot, 'appdata'), join(tempRoot, 'localappdata'), join(tempRoot, 'tmp')]) {
    mkdirSync(path, { recursive: true, mode: 0o700 });
  }
  const child = spawn(sidecar, [
    '--cwd', workspace,
    '--bridge-dir', bridgeDir,
    '--credential-stdin',
    '--packaged-credential-stdin-only',
  ], {
    cwd: workspace,
    env: sanitizedEnvironment(tempRoot),
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });
  child.stdin.end('{"provider_keys":{}}\n');

  try {
    const lockfile = await waitFor(
      () => readdirSync(bridgeDir).find((entry) => entry.endsWith('.lock')),
      'packaged keyless sidecar lockfile',
    );
    assert.ok(lockfile, 'packaged sidecar must publish a discovery lockfile');
    assert.equal(child.exitCode, null, `packaged sidecar exited early: ${stderr}`);
    await terminate(child, join(bridgeDir, lockfile));
    await waitFor(
      () => readdirSync(bridgeDir).every((entry) => !entry.endsWith('.lock') && !entry.startsWith('launch-')),
      'packaged sidecar runtime cleanup',
      10_000,
    );
    const plaintextCredentials = walkTree(tempRoot).filter((path) => path.endsWith('.credentials.json'));
    assert.deepEqual(plaintextCredentials, [], 'packaged keyless smoke must not create plaintext credentials');
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    rmSync(tempRoot, { recursive: true, force: true });
  }
}

try {
  const target = parseDesktopTargetArgs(process.argv.slice(2));
  await runPackagedSidecarSmoke(packageRoot, target.platform, target.arch);
  process.stdout.write(`[verify:sidecar-smoke] OK ${target.id}\n`);
} catch (error) {
  process.stderr.write(`[verify:sidecar-smoke] ERROR: ${formatError(error)}\n`);
  process.exitCode = 1;
}
