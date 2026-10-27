import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { GSTREAMER_PLUGIN_ENV, sanitizedEnvironment, verifyBundledLinuxDependencies } from '../scripts/packaged-sidecar-smoke.mjs';

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
  for (const platform of ['darwin', 'win32']) {
    const other = sanitizedEnvironment('/isolated', '/app resources', platform, source);
    for (const name of ['LD_LIBRARY_PATH', 'LD_PRELOAD', 'GST_PLUGIN_PATH', 'GSTREAMER_ROOT', GSTREAMER_PLUGIN_ENV]) {
      assert.equal(other[name], source[name], `${platform} ${name}`);
    }
    assert.equal(other.OPENAI_API_KEY, undefined);
  }
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
