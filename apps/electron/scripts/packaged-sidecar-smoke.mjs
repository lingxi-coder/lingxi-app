#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BridgeClient } from '@lingxi/bridge-client';
import { verifyMacGStreamerRuntime } from './gstreamer-macos.mjs';
import { verifyPortableGStreamerRuntime } from './gstreamer-portable.mjs';

import {
  PACKAGE_OUTPUT_ROOT_ENV,
  artifactPaths,
  desktopArtifactPaths,
  formatError,
  packageRoot,
  parseDesktopTargetArgs,
  walkTree,
} from './package-support.mjs';

export const GSTREAMER_PLUGIN_ENV = 'LINGXI_GSTREAMER_PLUGIN_DIR';

/** The signed Flare package and the generic Desktop stage use different roots. */
export function parsePackagedSidecarSmokeArgs(argv) {
  let signedMacPackage = false;
  const targetArgs = [];
  for (const arg of argv) {
    if (arg === '--signed-mac-package') {
      if (signedMacPackage) throw new Error('--signed-mac-package may only be supplied once');
      signedMacPackage = true;
    } else {
      targetArgs.push(arg);
    }
  }
  const target = parseDesktopTargetArgs(targetArgs);
  if (signedMacPackage && target.id !== 'darwin-arm64') {
    throw new Error('--signed-mac-package requires the darwin-arm64 target');
  }
  return { ...target, signedMacPackage };
}

export function packagedSidecarPayload(root, platform, arch, {
  signedMacPackage = false,
  outputRoot = process.env[PACKAGE_OUTPUT_ROOT_ENV],
} = {}) {
  const paths = desktopArtifactPaths(root, platform, arch);
  if (!signedMacPackage) return paths.payloadRoot;
  if (platform !== 'darwin' || arch !== 'arm64') {
    throw new Error('--signed-mac-package requires the darwin-arm64 target');
  }
  // This is the same versioned app and output-root selection as verify:package.
  return artifactPaths(root, undefined, outputRoot).appPath;
}

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
  } else if (platform === 'darwin') {
    for (const name of Object.keys(env)) {
      if (/^(?:LD_|DYLD_|GST_|GSTREAMER_|LINGXI_GSTREAMER_SDK_)/.test(name)
          || name === GSTREAMER_PLUGIN_ENV || name === 'PKG_CONFIG_PATH' || name === 'PKG_CONFIG_LIBDIR') delete env[name];
    }
    env.PATH = '/usr/bin:/bin:/usr/sbin:/sbin';
    env[GSTREAMER_PLUGIN_ENV] = resolve(resources, '..', 'Frameworks', 'GStreamerPlugins');
    env.GST_PLUGIN_PATH_1_0 = '';
    env.GST_PLUGIN_SYSTEM_PATH_1_0 = '';
    env.GST_REGISTRY_FORK = 'no';
    env.GST_REGISTRY_1_0 = join(tempRoot, 'gst-registry.bin');
    env.LINGXI_REMOTE_STUN = 'stun:127.0.0.1:9';
  }
  return env;
}

function within(parent, path) {
  const rest = relative(parent, path);
  return rest === '' || (rest !== '..' && !rest.startsWith(`..${sep}`) && !isAbsolute(rest));
}

/** Compile against SDK headers, but link and execute only the packaged runtime. */
export function runPackagedMacMediaProbe(resources, tempRoot, arch, source = process.env) {
  const appRoot = resolve(resources, '..', '..');
  const frameworks = resolve(resources, '..', 'Frameworks');
  const libraries = join(frameworks, 'GStreamerLibraries');
  const plugins = join(frameworks, 'GStreamerPlugins');
  if (lstatSync(appRoot).isSymbolicLink() || lstatSync(frameworks).isSymbolicLink()) {
    throw new Error('packaged media probe application or Frameworks is a symbolic link');
  }
  const appReal = realpathSync(appRoot);
  const frameworksReal = realpathSync(frameworks);
  if (!within(appReal, frameworksReal)) throw new Error('packaged media probe Frameworks escapes its application');
  for (const directory of [libraries, plugins]) {
    if (lstatSync(directory).isSymbolicLink() || !within(frameworksReal, realpathSync(directory))) {
      throw new Error(`packaged media probe directory escapes Frameworks: ${directory}`);
    }
  }
  const sdkValue = source.LINGXI_GSTREAMER_SDK_MACOS?.trim();
  if (!sdkValue) throw new Error('the pinned macOS GStreamer SDK is required to compile the media smoke probe');
  const sdk = realpathSync(sdkValue);
  const env = sanitizedEnvironment(tempRoot, resources, 'darwin', source);
  const pkgEnv = { ...env, PATH: source.PATH, PKG_CONFIG_PATH: '', PKG_CONFIG_LIBDIR: join(sdk, 'lib', 'pkgconfig') };
  const pkgConfig = source.PKG_CONFIG?.trim() || 'pkg-config';
  const variable = (name, module) => {
    const path = execFileSync(pkgConfig, [`--variable=${name}`, module], { env: pkgEnv, encoding: 'utf8' }).trim();
    const real = realpathSync(path);
    if (!within(sdk, real)) throw new Error(`media probe SDK path escapes the pinned SDK: ${real}`);
    return real;
  };
  const gstInclude = join(variable('includedir', 'gstreamer-1.0'), 'gstreamer-1.0');
  const glibInclude = join(variable('includedir', 'glib-2.0'), 'glib-2.0');
  const glibConfig = join(variable('libdir', 'glib-2.0'), 'glib-2.0', 'include');
  const library = (prefix) => {
    const matches = readdirSync(libraries).filter((name) =>
      new RegExp(`^lib${prefix}\\.(?:\\d+\\.)*dylib$`).test(name));
    if (matches.length !== 1) throw new Error(`packaged media probe needs exactly one ${prefix} dylib`);
    const path = join(libraries, matches[0]);
    if (!within(realpathSync(libraries), realpathSync(path))) throw new Error(`packaged media probe library escapes its bundle: ${path}`);
    return path;
  };
  const executable = join(tempRoot, 'packaged-media-probe');
  const probeSource = join(dirname(fileURLToPath(import.meta.url)), 'packaged-macos-media-probe.c');
  execFileSync('/usr/bin/clang', [
    '-arch', arch === 'x64' ? 'x86_64' : 'arm64',
    `-I${gstInclude}`, `-I${glibInclude}`, `-I${glibConfig}`,
    '-o', executable, probeSource, library('gstreamer-1\\.0'),
    library('gobject-2\\.0'), library('glib-2\\.0'),
    '-Xlinker', '-rpath', '-Xlinker', libraries,
  ], { cwd: tempRoot, env, stdio: 'pipe', timeout: 60_000 });
  execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', executable], { stdio: 'pipe' });
  const result = execFileSync(executable, [plugins, libraries], {
    cwd: tempRoot, env, encoding: 'utf8', stdio: 'pipe', timeout: 60_000,
  }).trim();
  if (result !== 'packaged media plugins and audio/video pipelines OK') throw new Error(`unexpected packaged media probe result: ${result}`);
}

/** Reach the sidecar's native media initialization without a real viewer. */
export async function runPackagedMacBridgeMediaSmoke(client) {
  const route = randomUUID();
  const cancellationMessage = 'packaged media smoke closes its test route';
  await client.registerMediaConnection(route, 'packaged media smoke', ['audio_video'], ['microphone', 'camera']);
  let offered = false;
  let handlerError;
  const handleOffer = (request) => {
    try {
      if (request.connection_id !== route || request.mode !== 'audio_video' || request.description?.kind !== 'offer') {
        throw new Error('packaged media smoke received an unexpected offer');
      }
      offered = true;
      client.sendCommand({ type: 'media_connection_response', response: {
        request_id: request.request_id, connection_id: request.connection_id,
        media_session_ref: request.media_session_ref,
        error: { kind: 'cancelled', message: cancellationMessage },
      } });
    } catch (error) { handlerError = error; }
  };
  try {
    const { result } = await client.executeMedia({ type: 'sources', usage: 'session' });
    const audio = `remote_${route}_microphone`;
    const video = `remote_${route}_camera`;
    if (result.type !== 'sources' || !result.sources.some((source) => source.reference.id === audio)
        || !result.sources.some((source) => source.reference.id === video)) {
      throw new Error('packaged media smoke could not find its registered test sources');
    }
    client.on('mediaConnectionRequest', handleOffer);
    let failure;
    try {
      await client.executeMedia({ type: 'session_start', request: {
        mode: 'audio_video', intent: 'observation', audio_source: { id: audio }, video_source: { id: video },
      } }, { timeout_budget_ms: 20_000 });
    } catch (error) { failure = error; }
    if (handlerError) throw handlerError;
    if (!offered || failure?.name !== 'MediaServiceError:cancelled' || !failure.message.includes(cancellationMessage)) {
      throw new Error(`packaged media initialization did not reach the test offer: ${failure?.message ?? 'unexpected success'}`);
    }
  } finally {
    client.off('mediaConnectionRequest', handleOffer);
    await client.unregisterMediaConnection(route);
  }
}

/** Check the packaged link mode before exercising the native media offer. */
export async function runPackagedMacMediaSmoke(resources, tempRoot, arch, client, {
  verifyRuntime = verifyMacGStreamerRuntime,
  runProbe = runPackagedMacMediaProbe,
} = {}) {
  const frameworks = resolve(resources, '..', 'Frameworks');
  const sidecar = join(resources, 'bin', 'bridge-server');
  const runtime = verifyRuntime(frameworks, sidecar, {
    expectedArch: arch === 'x64' ? 'x86_64' : arch,
    appRoot: resolve(resources, '..', '..'),
  });
  if (runtime.bundled) runProbe(resources, tempRoot, arch);
  await runPackagedMacBridgeMediaSmoke(client);
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

export async function runPackagedSidecarSmoke(root, platform, arch, options = {}) {
  if (process.platform !== platform || process.arch !== arch) {
    throw new Error(`native ${platform}-${arch} host required for packaged sidecar smoke`);
  }
  const payloadRoot = packagedSidecarPayload(root, platform, arch, options);
  const resources = platform === 'darwin'
    ? join(payloadRoot, 'Contents', 'Resources')
    : join(payloadRoot, 'resources');
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
    if (platform === 'darwin') {
      const client = new BridgeClient({ lockfilePath: join(bridgeDir, lockfile) });
      try {
        await client.connect();
        await runPackagedMacMediaSmoke(resources, tempRoot, arch, client);
      } finally { client.close(); }
    }
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
  return payloadRoot;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const target = parsePackagedSidecarSmokeArgs(process.argv.slice(2));
    const payloadRoot = await runPackagedSidecarSmoke(packageRoot, target.platform, target.arch, target);
    process.stdout.write(`[verify:sidecar-smoke] OK ${target.id} ${payloadRoot}\n`);
  } catch (error) {
    process.stderr.write(`[verify:sidecar-smoke] ERROR: ${formatError(error)}\n`);
    process.exitCode = 1;
  }
}
