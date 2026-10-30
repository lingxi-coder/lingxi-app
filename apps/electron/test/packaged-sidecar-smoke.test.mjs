import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { GSTREAMER_PLUGIN_ENV, packagedSidecarPayload, parsePackagedSidecarSmokeArgs, runPackagedMacBridgeMediaSmoke, runPackagedMacMediaProbe, runPackagedMacMediaSmoke, runPackagedSidecarSmoke, sanitizedEnvironment, verifyBundledLinuxDependencies } from '../scripts/packaged-sidecar-smoke.mjs';

test('signed macOS smoke selects the versioned app rather than the generic Desktop stage', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi signed smoke '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const output = join(root, 'isolated output');
  mkdirSync(output);
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version: '1.2.3' }));
  const target = parsePackagedSidecarSmokeArgs(['--platform', 'darwin', '--arch', 'arm64', '--signed-mac-package']);
  assert.equal(target.signedMacPackage, true);
  const signedDefault = packagedSidecarPayload(root, 'darwin', 'arm64', { signedMacPackage: true, outputRoot: '' });
  const signedIsolated = packagedSidecarPayload(root, 'darwin', 'arm64', { signedMacPackage: true, outputRoot: output });
  const desktopStage = packagedSidecarPayload(root, 'darwin', 'arm64');
  assert.equal(signedDefault, join(root, 'dist', 'LingXi-Code-1.2.3-mac-arm64', 'LingXi Code.app'));
  assert.equal(signedIsolated, join(realpathSync(output), 'LingXi-Code-1.2.3-mac-arm64', 'LingXi Code.app'));
  assert.equal(desktopStage, join(root, 'dist', 'stage', 'darwin-arm64', 'LingXi Code.app'));
  assert.notEqual(signedDefault, desktopStage);
  assert.notEqual(signedIsolated, desktopStage);
  const previous = process.env.LINGXI_PACKAGE_OUTPUT_ROOT;
  process.env.LINGXI_PACKAGE_OUTPUT_ROOT = output;
  try {
    assert.equal(packagedSidecarPayload(root, 'darwin', 'arm64', { signedMacPackage: true }), signedIsolated);
  } finally {
    if (previous === undefined) delete process.env.LINGXI_PACKAGE_OUTPUT_ROOT;
    else process.env.LINGXI_PACKAGE_OUTPUT_ROOT = previous;
  }
});

test('signed macOS smoke rejects incompatible or ambiguous target arguments', () => {
  assert.throws(() => parsePackagedSidecarSmokeArgs(['--platform', 'linux', '--arch', 'x64', '--signed-mac-package']), /requires the darwin-arm64 target/);
  assert.throws(() => parsePackagedSidecarSmokeArgs(['--platform', 'darwin', '--arch', 'x64', '--signed-mac-package']), /requires the darwin-arm64 target/);
  assert.throws(() => parsePackagedSidecarSmokeArgs(['--platform', 'darwin', '--arch', 'arm64', '--signed-mac-package', '--signed-mac-package']), /only be supplied once/);
  assert.throws(() => packagedSidecarPayload('/unused', 'linux', 'x64', { signedMacPackage: true }), /requires the darwin-arm64 target/);
  assert.throws(() => parsePackagedSidecarSmokeArgs(['--platform', 'darwin', '--arch', 'arm64', '--unknown']), /unknown package argument/);
});

test('sidecar smoke CLI rejects a signed package flag for a portable target', () => {
  const script = new URL('../scripts/packaged-sidecar-smoke.mjs', import.meta.url);
  const result = spawnSync(process.execPath, [fileURLToPath(script), '--platform', 'linux', '--arch', 'x64', '--signed-mac-package'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /requires the darwin-arm64 target/);
});

test('missing signed sidecar never falls back to a populated generic stage', { skip: process.platform !== 'darwin' || process.arch !== 'arm64' }, async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-signed-smoke-missing-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const output = join(root, 'output');
  mkdirSync(output);
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version: '1.2.3' }));
  const generic = packagedSidecarPayload(root, 'darwin', 'arm64');
  const bin = join(generic, 'Contents', 'Resources', 'bin');
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, 'bridge-server'), 'generic stage fixture');
  const expected = join(realpathSync(output), 'LingXi-Code-1.2.3-mac-arm64', 'LingXi Code.app', 'Contents', 'Resources', 'bin', 'bridge-server');
  await assert.rejects(
    () => runPackagedSidecarSmoke(root, 'darwin', 'arm64', { signedMacPackage: true, outputRoot: output }),
    (error) => error.message === `packaged sidecar is missing: ${expected}`,
  );
});

test('Linux packaged smoke uses only its own loader and plugin paths', () => {
  const source = {
    PATH: '/usr/bin', LD_LIBRARY_PATH: '/external-sdk', LD_PRELOAD: '/injected.so',
    LD_AUDIT: '/audit.so', GST_PLUGIN_PATH: '/external-plugins', GST_REGISTRY: '/external-registry',
    GSTREAMER_ROOT: '/external-sdk', [GSTREAMER_PLUGIN_ENV]: '/external-plugin', OPENAI_API_KEY: 'secret',
  };
  const env = sanitizedEnvironment('/isolated', '/app resources', 'linux', source);
  assert.equal(env.LD_LIBRARY_PATH, '$ORIGIN/../gstreamer/lib');
  assert.equal(env[GSTREAMER_PLUGIN_ENV], join('/app resources', 'gstreamer', 'lib', 'gstreamer-1.0'));
  for (const name of ['LD_PRELOAD', 'LD_AUDIT', 'GST_PLUGIN_PATH', 'GST_REGISTRY', 'GSTREAMER_ROOT', 'OPENAI_API_KEY']) {
    assert.equal(env[name], undefined, name);
  }
  assert.equal(env.HOME, join('/isolated', 'home'));
  assert.equal(env.PATH, source.PATH);
  for (const platform of ['win32']) {
    const other = sanitizedEnvironment('/isolated', '/app resources', platform, source);
    for (const name of ['LD_LIBRARY_PATH', 'LD_PRELOAD', 'GST_PLUGIN_PATH', 'GSTREAMER_ROOT', GSTREAMER_PLUGIN_ENV]) {
      assert.equal(other[name], source[name], `${platform} ${name}`);
    }
    assert.equal(other.OPENAI_API_KEY, undefined);
  }
});

test('macOS packaged smoke removes host loader and SDK fallbacks', () => {
  const source = {
    PATH: '/sdk/bin', DYLD_LIBRARY_PATH: '/sdk/lib', DYLD_INSERT_LIBRARIES: '/injected.dylib',
    LD_LIBRARY_PATH: '/other', GST_PLUGIN_PATH: '/sdk/plugins', GST_REGISTRY_1_0: '/host-registry',
    GSTREAMER_ROOT: '/sdk', LINGXI_GSTREAMER_SDK_MACOS: '/sdk', PKG_CONFIG_PATH: '/sdk/lib/pkgconfig',
    [GSTREAMER_PLUGIN_ENV]: '/external-plugin', OPENAI_API_KEY: 'secret',
  };
  const env = sanitizedEnvironment('/isolated', '/App.app/Contents/Resources', 'darwin', source);
  for (const name of ['DYLD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES', 'LD_LIBRARY_PATH', 'GST_PLUGIN_PATH',
    'GSTREAMER_ROOT', 'LINGXI_GSTREAMER_SDK_MACOS', 'PKG_CONFIG_PATH', 'OPENAI_API_KEY']) {
    assert.equal(env[name], undefined, name);
  }
  assert.equal(env.PATH, '/usr/bin:/bin:/usr/sbin:/sbin');
  assert.equal(env[GSTREAMER_PLUGIN_ENV], resolve('/App.app/Contents/Resources', '..', 'Frameworks', 'GStreamerPlugins'));
  assert.equal(env.GST_PLUGIN_SYSTEM_PATH_1_0, '');
  assert.equal(env.GST_PLUGIN_PATH_1_0, '');
  assert.equal(env.GST_REGISTRY_1_0, '/isolated/gst-registry.bin');
  assert.equal(env.LINGXI_REMOTE_STUN, 'stun:127.0.0.1:9');
});

test('macOS media probe rejects a Frameworks symlink before probing', { skip: process.platform !== 'darwin' }, (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-media-frameworks-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const contents = join(root, 'App.app', 'Contents');
  const resources = join(contents, 'Resources');
  const outside = join(root, 'outside');
  mkdirSync(resources, { recursive: true });
  mkdirSync(outside);
  symlinkSync(outside, join(contents, 'Frameworks'));
  assert.throws(() => runPackagedMacMediaProbe(resources, root, 'arm64', {}), /symbolic link/);
});

class FakeMediaClient extends EventEmitter {
  constructor({ offer = true, outcome = 'cancelled', cancellation = 'packaged media smoke closes its test route' } = {}) {
    super();
    this.offer = offer;
    this.outcome = outcome;
    this.cancellation = cancellation;
    this.commands = [];
    this.unregistered = false;
  }
  async registerMediaConnection(route, label, modes, sources) {
    assert.equal(label, 'packaged media smoke');
    assert.deepEqual(modes, ['audio_video']);
    assert.deepEqual(sources, ['microphone', 'camera']);
    this.route = route;
  }
  async executeMedia(operation) {
    if (operation.type === 'sources') return { result: { type: 'sources', sources: [
      { reference: { id: `remote_${this.route}_microphone` } },
      { reference: { id: `remote_${this.route}_camera` } },
    ] } };
    assert.equal(operation.type, 'session_start');
    assert.equal(operation.request.mode, 'audio_video');
    assert.equal(operation.request.audio_source.id, `remote_${this.route}_microphone`);
    assert.equal(operation.request.video_source.id, `remote_${this.route}_camera`);
    if (this.offer) this.emit('mediaConnectionRequest', { request_id: 'request', connection_id: this.route,
      media_session_ref: { id: 'media' }, mode: 'audio_video', description: { kind: 'offer', sdp: 'test' } });
    if (this.outcome === 'success') return { result: { type: 'session_started' } };
    const error = new Error(this.offer ? this.cancellation : 'media codec runtime is unavailable');
    error.name = `MediaServiceError:${this.outcome}`;
    throw error;
  }
  sendCommand(command) { this.commands.push(command); }
  async unregisterMediaConnection(route) { assert.equal(route, this.route); this.unregistered = true; }
}

test('macOS bridge media smoke reaches an offer and cancels its route', async () => {
  const client = new FakeMediaClient();
  await runPackagedMacBridgeMediaSmoke(client);
  assert.equal(client.unregistered, true);
  assert.equal(client.commands.length, 1);
  assert.equal(client.commands[0].response.error.kind, 'cancelled');
});

test('macOS bridge media smoke rejects failure before initialization or unexpected success', async () => {
  for (const options of [
    { offer: false, outcome: 'unavailable' }, { outcome: 'success' }, { cancellation: 'unrelated cancellation' },
  ]) {
    const client = new FakeMediaClient(options);
    await assert.rejects(() => runPackagedMacBridgeMediaSmoke(client), /did not reach the test offer/);
    assert.equal(client.unregistered, true);
  }
});

test('macOS media smoke probes dynamic bundles and reaches an offer for both link modes', async () => {
  for (const bundled of [true, false]) {
    const client = new FakeMediaClient();
    let probeCount = 0;
    await runPackagedMacMediaSmoke('/App.app/Contents/Resources', '/isolated', 'x64', client, {
      verifyRuntime: (frameworks, sidecar, options) => {
        assert.equal(frameworks, '/App.app/Contents/Frameworks');
        assert.equal(sidecar, '/App.app/Contents/Resources/bin/bridge-server');
        assert.deepEqual(options, { expectedArch: 'x86_64', appRoot: '/App.app' });
        return { bundled };
      },
      runProbe: () => { probeCount += 1; },
    });
    assert.equal(probeCount, bundled ? 1 : 0);
    assert.equal(client.commands.length, 1);
    assert.equal(client.unregistered, true);
  }
});

test('macOS media smoke rejects a missing dynamic runtime before attempting an offer', async () => {
  const client = new FakeMediaClient();
  let probeCount = 0;
  await assert.rejects(() => runPackagedMacMediaSmoke('/App.app/Contents/Resources', '/isolated', 'arm64', client, {
    verifyRuntime: () => { throw new Error('packaged GStreamer plugin is missing'); },
    runProbe: () => { probeCount += 1; },
  }), /packaged GStreamer plugin is missing/);
  assert.equal(probeCount, 0);
  assert.equal(client.commands.length, 0);
});

test('macOS dynamic media smoke requires the SDK even when the bundle exists', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-media-missing-sdk-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const contents = join(root, 'App.app', 'Contents');
  const resources = join(contents, 'Resources');
  mkdirSync(resources, { recursive: true });
  for (const name of ['GStreamerLibraries', 'GStreamerPlugins']) mkdirSync(join(contents, 'Frameworks', name), { recursive: true });
  const client = new FakeMediaClient();
  await assert.rejects(() => runPackagedMacMediaSmoke(resources, root, 'arm64', client, {
    verifyRuntime: () => ({ bundled: true }),
    runProbe: () => runPackagedMacMediaProbe(resources, root, 'arm64', {}),
  }), /pinned macOS GStreamer SDK is required/);
  assert.equal(client.commands.length, 0);
});

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'lingxi sidecar runtime '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const resources = join(root, 'app resources');
  const library = join(resources, 'gstreamer', 'lib');
  mkdirSync(library, { recursive: true });
  const names = ['libgstreamer-1.0.so.0', 'libgstvideo-1.0.so.0', 'libglib-2.0.so.0'];
  for (const name of names) writeFileSync(join(library, name), name);
  const manifest = { runtimeFiles: names.map(name => ({ kind: 'library', packagePath: `lib/${name}` })) };
  const line = (name, path) => `\t${name} => ${path} (0x00001234)`;
  const good = names.map(name => line(name, join(library, name))).join('\n');
  return { root, resources, library, names, manifest, line, good };
}

test('Linux dependency inspection accepts packaged paths with spaces and system platform libraries', (t) => {
  const f = fixture(t);
  const output = `${f.good}\n${f.line('libc.so.6', '/usr/lib/libc.so.6')}\n\tlinux-vdso.so.1 (0x00001234)`;
  assert.doesNotThrow(() => verifyBundledLinuxDependencies(output, f.resources, f.manifest));
});

test('Linux dependency inspection rejects SDK, system, missing and unmanifested GStreamer libraries', (t) => {
  const f = fixture(t);
  const external = join(f.root, 'external-gstreamer.so');
  writeFileSync(external, 'external');
  assert.throws(() => verifyBundledLinuxDependencies(f.good.replace(join(f.library, f.names[1]), external), f.resources, f.manifest), /outside its bundled file/);
  assert.throws(() => verifyBundledLinuxDependencies(`${f.good}\n${f.line('libgstother-1.0.so.0', external)}`, f.resources, f.manifest), /unbundled GStreamer/);
  assert.throws(() => verifyBundledLinuxDependencies(`${f.good}\nlibgstmissing.so.0 => not found`, f.resources, f.manifest), /dependency is missing/);
  assert.throws(() => verifyBundledLinuxDependencies(`${f.good}\nlibgstbad.so.0 => malformed`, f.resources, f.manifest), /unrecognized packaged dependency/);
  assert.throws(() => verifyBundledLinuxDependencies(f.line('libc.so.6', '/usr/lib/libc.so.6'), f.resources, f.manifest), /did not resolve/);
});

test('Linux dependency inspection rejects unsafe manifest paths and symlink escapes', (t) => {
  const f = fixture(t);
  const outside = join(f.root, 'outside.so');
  writeFileSync(outside, 'outside');
  const alias = join(f.root, 'external-alias.so');
  symlinkSync(outside, alias);
  assert.throws(() => verifyBundledLinuxDependencies(f.good.replace(join(f.library, f.names[1]), alias), f.resources, f.manifest), /outside its bundled file/);
  const unsafe = { runtimeFiles: [...f.manifest.runtimeFiles, { kind: 'library', packagePath: 'lib/../outside.so' }] };
  assert.throws(() => verifyBundledLinuxDependencies(f.good, f.resources, unsafe), /invalid packaged Linux library path/);
  const aliasInPackage = join(f.library, 'libgstalias.so.0');
  symlinkSync(outside, aliasInPackage);
  const symlinkManifest = { runtimeFiles: [...f.manifest.runtimeFiles, { kind: 'library', packagePath: 'lib/libgstalias.so.0' }] };
  assert.throws(() => verifyBundledLinuxDependencies(f.good, f.resources, symlinkManifest), /invalid packaged Linux library/);
  const duplicate = { runtimeFiles: [...f.manifest.runtimeFiles, f.manifest.runtimeFiles[0]] };
  assert.throws(() => verifyBundledLinuxDependencies(f.good, f.resources, duplicate), /invalid packaged Linux library/);
});

test('Linux dependency inspection rejects a symlinked runtime directory', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-sidecar-runtime-escape-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const resources = join(root, 'resources');
  const external = join(root, 'external');
  mkdirSync(resources); mkdirSync(join(external, 'lib'), { recursive: true });
  symlinkSync(external, join(resources, 'gstreamer'));
  assert.throws(() => verifyBundledLinuxDependencies('', resources, { runtimeFiles: [] }), /escapes resources/);
});
