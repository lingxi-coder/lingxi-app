#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BridgeClient } from '@lingxi/bridge-client';
import { verifyPortableGStreamerRuntime } from './gstreamer-portable.mjs';

import {
  desktopArtifactPaths,
  formatError,
  packageRoot,
  parseDesktopTargetArgs,
  walkTree,
} from './package-support.mjs';

export const GSTREAMER_PLUGIN_ENV = 'LINGXI_GSTREAMER_PLUGIN_DIR';

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

export function sanitizedEnvironment(tempRoot, resources, platform, source = process.env) {
  const env = { ...source };
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
  if (platform === 'linux') {
    // The runner's SDK and loader hooks are not evidence that the packaged
    // sidecar can start using its own runtime. Mirror the packaged host path.
    for (const name of Object.keys(env)) {
      if (/^(?:LD_|GST_|GSTREAMER_)/.test(name) || name === GSTREAMER_PLUGIN_ENV) delete env[name];
    }
    env.LD_LIBRARY_PATH = '$ORIGIN/../gstreamer/lib';
    env[GSTREAMER_PLUGIN_ENV] = join(resources, 'gstreamer', 'lib', 'gstreamer-1.0');
  }
  return env;
}

/** Require every loaded manifest library to resolve to its exact packaged file. */
export function verifyBundledLinuxDependencies(output, resources, manifest) {
  const runtimePath = join(resources, 'gstreamer');
  const libraryPath = join(runtimePath, 'lib');
  const runtimeDirectory = realpathSync(runtimePath);
  const libraryDirectory = realpathSync(libraryPath);
  if (!lstatSync(runtimePath).isDirectory() || !lstatSync(libraryPath).isDirectory()
    || dirname(runtimeDirectory) !== realpathSync(resources) || dirname(libraryDirectory) !== runtimeDirectory) {
    throw new Error('packaged Linux runtime directory escapes resources');
  }
  const expected = new Map();
  for (const file of manifest.runtimeFiles) {
    if (file.kind !== 'library') continue;
    const parts = typeof file.packagePath === 'string' ? file.packagePath.split('/') : [];
    if (parts.length !== 2 || parts[0] !== 'lib' || !/^lib[^/\\:]+\.so(?:\.\d+)*$/.test(parts[1])) {
      throw new Error(`invalid packaged Linux library path: ${file.packagePath}`);
    }
    const path = join(libraryDirectory, parts[1]);
    if (!lstatSync(path).isFile() || dirname(realpathSync(path)) !== libraryDirectory || expected.has(parts[1])) {
      throw new Error(`invalid packaged Linux library: ${file.packagePath}`);
    }
    expected.set(parts[1], realpathSync(path));
  }
  const resolved = new Set();
  for (const line of output.split('\n')) {
    if (/=>\s+not found\s*$/.test(line)) throw new Error(`packaged sidecar dependency is missing: ${line.trim()}`);
    const match = /^\s*(\S+)\s+=>\s+(.+?)\s+\(0x[\da-f]+\)\s*$/i.exec(line);
    if (!match) {
      if (line.includes('=>')) throw new Error(`unrecognized packaged dependency: ${line.trim()}`);
      continue; // The ELF interpreter and vdso do not use `=>`.
    }
    const [, name, path] = match;
    if (expected.has(name)) {
      if (!isAbsolute(path) || realpathSync(path) !== expected.get(name)) {
        throw new Error(`packaged dependency ${name} resolved outside its bundled file: ${path}`);
      }
      resolved.add(name);
    } else if (/^libgst/i.test(name)) {
      throw new Error(`unbundled GStreamer dependency: ${name} => ${path}`);
    }
  }
  if (!resolved.has('libgstreamer-1.0.so.0')) throw new Error('packaged sidecar did not resolve its bundled GStreamer runtime');
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
  let child;
  try {
    const env = sanitizedEnvironment(tempRoot, resources, platform);
    if (platform === 'linux') {
      const runtime = verifyPortableGStreamerRuntime(resources, platform, arch);
      const manifest = JSON.parse(readFileSync(runtime.manifestPath, 'utf8'));
      // Trace the actual packaged executable directly so $ORIGIN refers to
      // resources/bin, not to an intermediate ldd shell or interpreter.
      const dependencies = execFileSync(sidecar, [], { env: { ...env, LD_TRACE_LOADED_OBJECTS: '1' }, encoding: 'utf8', timeout: 15_000 });
      verifyBundledLinuxDependencies(dependencies, resources, manifest);
    }
    child = spawn(sidecar, [
      '--cwd', workspace,
      '--bridge-dir', bridgeDir,
      '--credential-stdin',
      '--packaged-credential-stdin-only',
    ], {
      cwd: workspace,
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => { stderr += chunk.toString(); });
    child.stdin.end('{"provider_keys":{}}\n');

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
    if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    rmSync(tempRoot, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const target = parseDesktopTargetArgs(process.argv.slice(2));
    await runPackagedSidecarSmoke(packageRoot, target.platform, target.arch);
    process.stdout.write(`[verify:sidecar-smoke] OK ${target.id}\n`);
  } catch (error) {
    process.stderr.write(`[verify:sidecar-smoke] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
