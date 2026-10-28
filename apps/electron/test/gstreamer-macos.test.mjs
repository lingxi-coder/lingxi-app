import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  copyFileSync,
  existsSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir as systemTmpdir } from 'node:os';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { prepareMacGStreamerRuntime as prepareMacFixture, verifyMacAppSignature, verifyMacGStreamerRuntime } from '../scripts/gstreamer-macos.mjs';
import { preflightDesktopSidecar, resetDesktopOutputs, signGenericDesktopFrameworks, stageDarwinSidecar } from '../scripts/package-desktop.mjs';
import { preflightSignedSidecar, resetSignedOutputs, stageSignedSidecar } from '../scripts/package-mac.mjs';
import { verifyMacFrameworkArchitectures } from '../scripts/verify-desktop-package.mjs';
import { verifyTeamIdentifier } from '../scripts/credential-broker.mjs';
import { copyElectronApp, scanTreeForForbiddenContent } from '../scripts/package-support.mjs';

const pluginNames = [
  'libgstapp.dylib', 'libgstaudioconvert.dylib', 'libgstaudioresample.dylib',
  'libgstcoreelements.dylib', 'libgstopus.dylib', 'libgstrtp.dylib',
  'libgstrtpmanager.dylib', 'libgstvpx.dylib', 'libgstvideoconvertscale.dylib',
];
const macOnly = { skip: process.platform !== 'darwin' };
const electronRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
// macOS exposes /var as a symlink; use its real location as a trusted root.
const tmpdir = () => realpathSync(systemTmpdir());

function run(command, args) {
  return execFileSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function signFixture(sidecar, prepared) {
  for (const binary of [sidecar, ...prepared.libraries, ...prepared.plugins]) {
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', binary]);
  }
}

function hashFile(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function universal(root, output, source, flags = () => []) {
  const slices = ['x86_64', 'arm64'].map((arch) => {
    const thin = join(root, `${arch}-${output.split('/').at(-1)}`);
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', ...flags(arch), '-o', thin, source]);
    return thin;
  });
  run('/usr/bin/lipo', ['-create', ...slices, '-output', output]);
}

function sdkFixture(root) {
  // Spaces in the prefix also exercise otool's real path formatting.
  const sdk = join(root, 'GStreamer SDK');
  const lib = join(sdk, 'lib');
  const plugins = join(lib, 'gstreamer-1.0');
  const pkgconfig = join(lib, 'pkgconfig');
  mkdirSync(plugins, { recursive: true });
  mkdirSync(pkgconfig, { recursive: true });
  writeFileSync(join(pkgconfig, 'gstreamer-1.0.pc'), `prefix=${sdk}\nlibdir=\${prefix}/lib\npluginsdir=\${libdir}/gstreamer-1.0\nName: gstreamer\nDescription: test SDK\nVersion: 1.0\n`);
  const baseSource = join(root, 'base.c');
  const pluginSource = join(root, 'plugin.c');
  const mainSource = join(root, 'main.c');
  writeFileSync(baseSource, 'int gst_fixture(void) { return 1; }\n');
  writeFileSync(pluginSource, 'int gst_fixture(void); int plugin(void) { return gst_fixture(); }\n');
  writeFileSync(mainSource, 'int gst_fixture(void); int main(void) { return gst_fixture() - 1; }\n');
  const base = join(lib, 'libgstfixture.dylib');
  universal(root, base, baseSource, (arch) => [
    '-dynamiclib', '-Wl,-install_name,@rpath/libgstfixture.dylib',
    ...(arch === 'arm64' ? ['-Wl,-rpath,@loader_path/arm-only'] : []),
  ]);
  const plugin = join(root, 'plugin.dylib');
  universal(root, plugin, pluginSource, (arch) => [
    '-dynamiclib', '-Wl,-install_name,@rpath/libgstplugin.dylib', base,
    ...(arch === 'x86_64' ? ['-Wl,-rpath,@loader_path/intel-only'] : []),
  ]);
  for (const name of pluginNames) copyFileSync(plugin, join(plugins, name));
  const sidecar = join(root, 'App.app', 'Contents', 'Resources', 'bin', 'bridge-server');
  mkdirSync(join(root, 'App.app', 'Contents', 'Resources', 'bin'), { recursive: true });
  universal(root, sidecar, mainSource, (arch) => [
    base, `-Wl,-rpath,${lib}`,
    ...(arch === 'arm64' ? ['-Wl,-rpath,@loader_path/arm-only'] : []),
  ]);
  return { sdk, lib, plugins, sidecar, base, baseSource };
}

function withSdk(sdk, action) {
  const previous = process.env.LINGXI_GSTREAMER_SDK_MACOS;
  process.env.LINGXI_GSTREAMER_SDK_MACOS = sdk;
  try {
    return action();
  } finally {
    if (previous === undefined) delete process.env.LINGXI_GSTREAMER_SDK_MACOS;
    else process.env.LINGXI_GSTREAMER_SDK_MACOS = previous;
  }
}

function prepareMacGStreamerRuntime(sidecar, frameworks, arch) {
  let stageRoot = dirname(frameworks);
  while (relative(stageRoot, sidecar).startsWith('..')) {
    const parent = dirname(stageRoot);
    if (parent === stageRoot) throw new Error('fixture has no common staging root');
    stageRoot = parent;
  }
  return prepareMacFixture(sidecar, frameworks, arch, { stageRoot });
}

function ancestorRunpathFixture(root, { decoy = false, parentRpath = false } = {}) {
  const fixture = sdkFixture(root);
  const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
  const nonstandard = join(fixture.sdk, 'nonstandard', 'private libraries');
  mkdirSync(nonstandard, { recursive: true });
  const leaf = join(nonstandard, 'libgstleaf.dylib');
  const leafSource = join(root, 'leaf.c');
  writeFileSync(leafSource, 'int gst_leaf(void) { return 11; }\n');
  run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
    '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', leaf, leafSource]);
  if (decoy) {
    const decoySource = join(root, 'decoy.c');
    writeFileSync(decoySource, 'int gst_leaf(void) { return 33; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', join(fixture.lib, 'libgstleaf.dylib'), decoySource]);
  }
  if (parentRpath) {
    const parentChoice = join(fixture.sdk, 'parent-choice');
    mkdirSync(parentChoice, { recursive: true });
    const choiceSource = join(root, 'parent-choice.c');
    writeFileSync(choiceSource, 'int gst_leaf(void) { return 22; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', join(parentChoice, 'libgstleaf.dylib'), choiceSource]);
  }
  const parent = join(fixture.lib, 'libgstparent.dylib');
  const parentSource = join(root, 'parent.c');
  writeFileSync(parentSource, 'int gst_leaf(void); int gst_parent(void) { return gst_leaf(); }\n');
  run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
    '-Wl,-install_name,@rpath/libgstparent.dylib',
    ...(parentRpath ? ['-Wl,-rpath,@loader_path/../parent-choice'] : []),
    '-o', parent, parentSource, leaf]);
  const main = join(root, 'ancestor-main.c');
  writeFileSync(main, '#include <stdio.h>\nint gst_parent(void); int main(void) { printf("%d\\n", gst_parent()); return 0; }\n');
  const relativeCustom = relative(dirname(fixture.sidecar), nonstandard);
  const relativeLib = relative(dirname(fixture.sidecar), fixture.lib);
  run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names',
    `-Wl,-rpath,@executable_path/${relativeCustom}`, `-Wl,-rpath,@executable_path/${relativeLib}`,
    '-o', fixture.sidecar, main, parent]);
  const sidecarDependencies = run('/usr/bin/otool', ['-L', fixture.sidecar]);
  assert.match(sidecarDependencies, /@rpath\/libgstparent\.dylib/);
  assert.doesNotMatch(sidecarDependencies, /@rpath\/libgstleaf\.dylib/);
  const parentRpaths = run('/usr/bin/otool', ['-l', parent]);
  assert.equal(parentRpaths.includes('LC_RPATH'), parentRpath);
  return { ...fixture, arch, nonstandard, leaf, parent };
}

function runCleanSidecar(sidecar, args = []) {
  const env = { ...process.env };
  for (const name of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH']) delete env[name];
  return execFileSync(sidecar, args, { env, encoding: 'utf8', stdio: 'pipe' }).trim();
}

test('packaging strips SDK debug paths while retaining runtime path checks', macOnly, () => {
  for (const runtimePath of [false, true]) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-debug-path-'));
    try {
      const fixture = sdkFixture(root);
      const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
      const object = join(root, 'debug-fixture.o');
      writeFileSync(fixture.baseSource, 'int gst_fixture(void) { return 1; }\n'
        + (runtimePath ? 'const char *gst_marker(void) { return "/Users/packaging-fixture/private/source.c"; }\n' : ''));
      run('clang', ['-arch', arch, '-g', '-c', fixture.baseSource, '-o', object]);
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        '-Wl,-install_name,@rpath/libgstfixture.dylib', '-o', fixture.base, object]);
      assert.ok(readFileSync(fixture.base).includes(Buffer.from(object)), 'fixture must contain an actual debug-object path');
      const sourceHash = hashFile(fixture.base);
      const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
      mkdirSync(frameworks, { recursive: true });
      const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
      const stagedLibrary = prepared.libraries.find((path) => path.endsWith('/libgstfixture.dylib'));
      assert.ok(stagedLibrary);
      assert.equal(readFileSync(stagedLibrary).includes(Buffer.from(object)), false);
      assert.equal(hashFile(fixture.base), sourceHash, 'the SDK source must remain untouched');
      assert.match(run('/usr/bin/nm', ['-gU', stagedLibrary]), /_gst_fixture/);
      signFixture(fixture.sidecar, prepared);
      assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }).bundled, true);
      assert.equal(runCleanSidecar(fixture.sidecar), '');
      if (runtimePath) {
        assert.throws(() => scanTreeForForbiddenContent(frameworks), /absolute macOS user path found/);
      } else {
        assert.doesNotThrow(() => scanTreeForForbiddenContent(frameworks));
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('dyld uses ancestor rpaths for a transitive library and ignores an SDK search-dir decoy', macOnly, () => {
  for (const decoy of [true, false]) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-ancestor-'));
    try {
      const fixture = ancestorRunpathFixture(root, { decoy });
      assert.equal(runCleanSidecar(fixture.sidecar), '11', 'the real dyld must load the ancestor-rpath copy');
      const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
      mkdirSync(frameworks, { recursive: true });
      const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, fixture.arch));
      assert.ok(prepared.libraries.some((path) => path.endsWith('/libgstleaf.dylib')));
      signFixture(fixture.sidecar, prepared);
      assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar).bundled, true);
      assert.equal(runCleanSidecar(fixture.sidecar), '11', 'rebasing must retain the actual dyld choice');
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('plugin caller contexts must all have a complete dependency closure', macOnly, () => {
  for (const sidecarHasPluginRunpath of [false, true]) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-dlopen-'));
    try {
      const fixture = sdkFixture(root);
      const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
      const loaderDir = join(fixture.sdk, 'loader');
      mkdirSync(loaderDir);
      const loaderSource = join(root, 'loader.c');
      writeFileSync(loaderSource, '#include <dlfcn.h>\nint gst_load(const char *p) { void *h = dlopen(p, RTLD_NOW); if (!h) return -1; int (*f)(void) = dlsym(h, "plugin"); return f ? f() : -2; }\n');
      const loader = join(loaderDir, 'libgstloader.dylib');
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        '-Wl,-install_name,@rpath/libgstloader.dylib', '-Wl,-rpath,@loader_path/../lib', '-o', loader, loaderSource]);
      const main = join(root, 'dlopen-main.c');
      writeFileSync(main, '#include <stdio.h>\nint gst_load(const char *); int main(int argc, char **argv) { if (argc != 2) return 1; printf("%d\\n", gst_load(argv[1])); return 0; }\n');
      run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${loaderDir}`,
        ...(sidecarHasPluginRunpath ? [`-Wl,-rpath,${fixture.lib}`] : []),
        '-o', fixture.sidecar, main, loader]);
      assert.equal(runCleanSidecar(fixture.sidecar, [join(fixture.plugins, pluginNames[0])]), '1');
      const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
      mkdirSync(frameworks, { recursive: true });
      if (!sidecarHasPluginRunpath) {
        // This concrete dlopen happens to work, but Mach-O metadata does not
        // prove that it is the only possible plugin caller.
        assert.throws(
          () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch)),
          /cannot prove a complete plugin load context.*libgstfixture/,
        );
      } else {
        const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
        signFixture(fixture.sidecar, prepared);
        assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }).bundled, true);
        assert.equal(runCleanSidecar(fixture.sidecar, [prepared.plugins[0]]), '1');
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('plugin dependencies cannot be combined from different incomplete caller contexts', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-split-context-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const leaves = ['A', 'B'].map((name, index) => {
      const directory = join(fixture.sdk, `caller-${name}`);
      mkdirSync(directory);
      const source = join(root, `split-${name}.c`);
      writeFileSync(source, `int gst_split_${name}(void) { return ${index ? 22 : 11}; }\n`);
      const binary = join(directory, `libgstsplit${name}.dylib`);
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        `-Wl,-install_name,@rpath/libgstsplit${name}.dylib`, '-o', binary, source]);
      return binary;
    });
    const plugin = join(fixture.plugins, pluginNames[0]);
    const pluginSource = join(root, 'split-plugin.c');
    writeFileSync(pluginSource, 'int gst_split_A(void); int gst_split_B(void); int plugin(void) { return gst_split_A() + gst_split_B(); }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstsplitplugin.dylib', '-o', plugin, pluginSource, ...leaves]);
    const loaders = ['A', 'B'].map((name) => {
      const source = join(root, `split-loader-${name}.c`);
      writeFileSync(source, `#include <dlfcn.h>\nint gst_load_${name}(const char *p) { void *h = dlopen(p, RTLD_NOW); if (!h) return -1; int (*f)(void) = dlsym(h, "plugin"); return f ? f() : -2; }\n`);
      const binary = join(fixture.lib, `libgstcaller${name}.dylib`);
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        `-Wl,-install_name,@rpath/libgstcaller${name}.dylib`, `-Wl,-rpath,@loader_path/../caller-${name}`,
        '-o', binary, source]);
      return binary;
    });
    const main = join(root, 'split-main.c');
    writeFileSync(main, '#include <stdio.h>\nint gst_load_A(const char *); int gst_load_B(const char *); int main(int argc, char **argv) { if (argc != 2) return 1; printf("%d,%d\\n", gst_load_A(argv[1]), gst_load_B(argv[1])); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, main, ...loaders]);
    assert.equal(runCleanSidecar(fixture.sidecar, [plugin]), '-1,-1', 'neither real dyld caller has a complete plugin closure');
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    let failure;
    let prepared;
    try {
      prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
    } catch (error) {
      failure = error;
    }
    if (!failure) {
      assert.fail(`packaging combined incompatible caller contexts; rebased result: ${runCleanSidecar(fixture.sidecar, [prepared.plugins[0]])}`);
    }
    assert.match(failure.message, /cannot resolve GStreamer dependency @rpath\/libgstsplit[AB]\.dylib/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('distinct plugin runpaths are checked without generating caller permutations', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-distinct-rpaths-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    for (const [index, name] of pluginNames.slice(0, 6).entries()) {
      run('/usr/bin/install_name_tool', ['-add_rpath', `@loader_path/unique-${index}`, join(fixture.plugins, name)]);
    }
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
    assert.equal(prepared.plugins.length, 9);
    signFixture(fixture.sidecar, prepared);
    assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }).bundled, true);
    runCleanSidecar(fixture.sidecar);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a shared Mach-O dependency DAG is processed without enumerating every path', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-shared-dag-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    let previous = [];
    for (let level = 0; level < 10; level += 1) {
      const current = [];
      for (const branch of ['a', 'b']) {
        const name = `gstdag_${level}_${branch}`;
        const source = join(root, `${name}.c`);
        const binary = join(fixture.lib, `lib${name}.dylib`);
        const body = level === 0 ? 'return 1;' :
          `return gstdag_${level - 1}_a() + gstdag_${level - 1}_b();`;
        const declarations = level === 0 ? '' :
          `int gstdag_${level - 1}_a(void); int gstdag_${level - 1}_b(void);`;
        writeFileSync(source, `${declarations}\nint ${name}(void) { ${body} }\n`);
        run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
          `-Wl,-install_name,@rpath/lib${name}.dylib`, '-o', binary, source, ...previous]);
        current.push(binary);
      }
      previous = current;
    }
    const main = join(root, 'dag-main.c');
    writeFileSync(main, '#include <stdio.h>\nint gstdag_9_a(void); int gstdag_9_b(void); int main(void) { printf("%d\\n", gstdag_9_a() + gstdag_9_b()); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, main, ...previous]);
    assert.equal(runCleanSidecar(fixture.sidecar), '1024');
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
    assert.ok(prepared.libraries.length >= 20);
    signFixture(fixture.sidecar, prepared);
    assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }).bundled, true);
    assert.equal(runCleanSidecar(fixture.sidecar), '1024');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('cyclic Mach-O dependencies reach a stable runpath state', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-cycle-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const a = join(fixture.lib, 'libgstcyclea.dylib');
    const b = join(fixture.lib, 'libgstcycleb.dylib');
    const aSource = join(root, 'cycle-a.c');
    const bSource = join(root, 'cycle-b.c');
    writeFileSync(bSource, 'int gst_cycle_b(void) { return 1; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstcycleb.dylib', '-o', b, bSource]);
    writeFileSync(aSource, 'int gst_cycle_b(void); int gst_cycle_a(void) { return gst_cycle_b() + 1; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstcyclea.dylib', '-o', a, aSource, b]);
    writeFileSync(bSource, 'int gst_cycle_a(void); int gst_cycle_b(void) { return 1; } int gst_cycle_reference(void) { return gst_cycle_a(); }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstcycleb.dylib', '-o', b, bSource, a]);
    const main = join(root, 'cycle-main.c');
    writeFileSync(main, '#include <stdio.h>\nint gst_cycle_a(void); int main(void) { printf("%d\\n", gst_cycle_a()); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, main, a]);
    assert.equal(runCleanSidecar(fixture.sidecar), '2');
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
    signFixture(fixture.sidecar, prepared);
    assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }).bundled, true);
    assert.equal(runCleanSidecar(fixture.sidecar), '2');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('unknown plugin caller that shadows a dependency with another SDK library is rejected', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-caller-shadow-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const alternate = join(fixture.sdk, 'alternate');
    mkdirSync(alternate);
    const alternateSource = join(root, 'alternate.c');
    writeFileSync(alternateSource, 'int gst_fixture(void) { return 99; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstfixture.dylib', '-o', join(alternate, 'libgstfixture.dylib'), alternateSource]);
    const loaderSource = join(root, 'shadow-loader.c');
    writeFileSync(loaderSource, '#include <dlfcn.h>\nint gst_load(const char *p) { void *h = dlopen(p, RTLD_NOW); if (!h) return -1; int (*f)(void) = dlsym(h, "plugin"); return f ? f() : -2; }\n');
    const loader = join(fixture.lib, 'libgstloader.dylib');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstloader.dylib', '-Wl,-rpath,@loader_path/../alternate',
      '-o', loader, loaderSource]);
    const main = join(root, 'shadow-main.c');
    writeFileSync(main, '#include <stdio.h>\n#include <string.h>\n#include <dlfcn.h>\nint gst_load(const char *); int main(int argc, char **argv) { if (argc != 3) return 1; if (!strcmp(argv[1], "loader")) { printf("%d\\n", gst_load(argv[2])); return 0; } void *h = dlopen(argv[2], RTLD_NOW); if (!h) return 2; int (*f)(void) = dlsym(h, "plugin"); printf("%d\\n", f ? f() : -2); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, main, loader]);
    const plugin = join(fixture.plugins, pluginNames[0]);
    assert.equal(runCleanSidecar(fixture.sidecar, ['direct', plugin]), '1');
    assert.equal(runCleanSidecar(fixture.sidecar, ['loader', plugin]), '99');
    const frameworks = join(root, 'Frameworks');
    mkdirSync(frameworks);
    assert.throws(
      () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch)),
      /different sources under different loader paths/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('dyld prefers the immediate loader rpath to the sidecar rpath', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-parent-order-'));
  try {
    const fixture = ancestorRunpathFixture(root, { parentRpath: true, decoy: true });
    assert.equal(runCleanSidecar(fixture.sidecar), '22', 'the immediate loader path wins over the ancestor path');
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, fixture.arch));
    assert.equal(runCleanSidecar(fixture.sidecar), '22');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('dyld skips incompatible slices in ordered rpaths without accepting missing or escaped dependencies', macOnly, () => {
  for (const arch of ['arm64', 'x86_64']) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-arch-order-'));
    try {
      const fixture = sdkFixture(root);
      const nativeArch = process.arch === 'x64' ? 'x86_64' : 'arm64';
      const wrongArch = arch === 'arm64' ? 'x86_64' : 'arm64';
      const wrong = join(fixture.sdk, 'wrong-arch');
      const correct = join(fixture.sdk, 'correct-arch');
      const escape = join(fixture.sdk, 'escape');
      const outside = join(root, 'outside-sdk');
      for (const directory of [wrong, correct, escape, outside]) mkdirSync(directory);
      const librarySource = join(root, 'arch-choice.c');
      const mainSource = join(root, 'arch-choice-main.c');
      writeFileSync(librarySource, 'int gst_choice(void) { return 42; }\n');
      writeFileSync(mainSource, '#include <stdio.h>\nint gst_choice(void); int main(void) { printf("%d\\n", gst_choice()); return 0; }\n');
      for (const [directory, slice] of [[wrong, wrongArch], [correct, arch]]) {
        run('clang', ['-arch', slice, '-dynamiclib', '-Wl,-headerpad_max_install_names',
          '-Wl,-install_name,@rpath/libgstchoice.dylib', '-o', join(directory, 'libgstchoice.dylib'), librarySource]);
      }
      copyFileSync(join(correct, 'libgstchoice.dylib'), join(outside, 'libgstchoice.dylib'));
      symlinkSync(join(outside, 'libgstchoice.dylib'), join(escape, 'libgstchoice.dylib'));

      const variants = [
        { name: 'wrong-first', paths: [wrong, correct] },
        { name: 'correct-first', paths: [correct, wrong] },
        { name: 'missing', paths: [wrong, join(fixture.sdk, 'missing')], error: /cannot resolve GStreamer dependency @rpath\/libgstchoice/ },
        { name: 'escaped', paths: [wrong, escape], error: /resolves outside LINGXI_GSTREAMER_SDK_MACOS/ },
      ];
      for (const variant of variants) {
        const source = join(root, `sidecar-${variant.name}`);
        run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names',
          ...variant.paths.map((path) => `-Wl,-rpath,${path}`), `-Wl,-rpath,${fixture.lib}`,
          '-o', source, mainSource, join(correct, 'libgstchoice.dylib')]);
        if (arch === nativeArch) {
          if (variant.name === 'missing') assert.throws(() => runCleanSidecar(source), /Command failed/);
          else assert.equal(runCleanSidecar(source), '42', `${variant.name}: actual dyld choice`);
        }
        const stageRoot = join(root, `stage-${variant.name}`);
        const app = join(stageRoot, 'App.app');
        const frameworks = join(app, 'Contents', 'Frameworks');
        const sidecar = join(app, 'Contents', 'Resources', 'bin', 'bridge-server');
        mkdirSync(frameworks, { recursive: true });
        const stage = () => withSdk(fixture.sdk, () => stageDarwinSidecar(source, sidecar, frameworks, arch, stageRoot));
        if (variant.error) {
          assert.throws(stage, variant.error, `${variant.name}: ${arch}`);
        } else {
          const prepared = stage();
          signFixture(sidecar, prepared);
          assert.equal(verifyMacGStreamerRuntime(frameworks, sidecar, { expectedArch: arch, appRoot: app }).bundled, true);
          if (arch === nativeArch) assert.equal(runCleanSidecar(sidecar), '42', `${variant.name}: rebased dyld choice`);
        }
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('an SDK directory is not guessed when dyld has no runpath to it', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-no-runpath-'));
  try {
    const fixture = ancestorRunpathFixture(root, { decoy: true });
    const executableDir = dirname(fixture.sidecar);
    run('/usr/bin/install_name_tool', ['-change', '@rpath/libgstparent.dylib', fixture.parent, fixture.sidecar]);
    for (const directory of [fixture.nonstandard, fixture.lib]) {
      run('/usr/bin/install_name_tool', ['-delete_rpath', `@executable_path/${relative(executableDir, directory)}`, fixture.sidecar]);
    }
    assert.throws(() => runCleanSidecar(fixture.sidecar), /Command failed/);
    const frameworks = join(root, 'Frameworks');
    mkdirSync(frameworks);
    assert.throws(
      () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, fixture.arch)),
      /cannot resolve GStreamer dependency @rpath\/libgstleaf/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('same install name with different loader contexts is rejected', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-ambiguous-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const parents = [11, 22].map((value, index) => {
      const choice = join(fixture.sdk, `choice-${index}`);
      mkdirSync(choice, { recursive: true });
      const leafSource = join(root, `ambiguous-leaf-${index}.c`);
      writeFileSync(leafSource, `int gst_leaf(void) { return ${value}; }\n`);
      const leaf = join(choice, 'libgstleaf.dylib');
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', leaf, leafSource]);
      const parentSource = join(root, `ambiguous-parent-${index}.c`);
      writeFileSync(parentSource, `int gst_leaf(void); int gst_parent_${index}(void) { return gst_leaf(); }\n`);
      const parent = join(fixture.lib, `libgstparent${index}.dylib`);
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        `-Wl,-install_name,@rpath/libgstparent${index}.dylib`,
        `-Wl,-rpath,@loader_path/../choice-${index}`, '-o', parent, parentSource, leaf]);
      return parent;
    });
    const main = join(root, 'ambiguous-main.c');
    writeFileSync(main, '#include <stdio.h>\nint gst_parent_0(void); int gst_parent_1(void); int main(void) { printf("%d,%d\\n", gst_parent_0(), gst_parent_1()); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names',
      `-Wl,-rpath,${fixture.lib}`, '-o', fixture.sidecar, main, ...parents]);
    const actual = runCleanSidecar(fixture.sidecar);
    assert.equal(actual, '11,11', 'dyld reuses the first loaded image for the shared install name');
    const frameworks = join(root, 'Frameworks');
    mkdirSync(frameworks);
    assert.throws(
      () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch)),
      /different sources under different loader paths|conflicting dylib names/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('one shared dylib is checked again under each inherited runpath stack', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-dyld-shared-context-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const commonDir = join(fixture.sdk, 'common');
    mkdirSync(commonDir);
    const first = join(fixture.sdk, 'choice-0');
    mkdirSync(first);
    const firstLeaf = join(first, 'libgstleaf.dylib');
    const leafSource = join(root, 'shared-leaf.c');
    writeFileSync(leafSource, 'int gst_leaf(void) { return 11; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', firstLeaf, leafSource]);
    const commonSource = join(root, 'common.c');
    const common = join(commonDir, 'libgstcommon.dylib');
    writeFileSync(commonSource, 'int gst_leaf(void); int gst_common(void) { return gst_leaf(); }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libgstcommon.dylib', '-o', common, commonSource, firstLeaf]);
    const parents = [11, 22].map((value, index) => {
      const choice = join(fixture.sdk, `choice-${index}`);
      if (index) mkdirSync(choice);
      const leaf = join(choice, 'libgstleaf.dylib');
      if (index) {
        const source = join(root, 'shared-leaf-other.c');
        writeFileSync(source, `int gst_leaf(void) { return ${value}; }\n`);
        run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
          '-Wl,-install_name,@rpath/libgstleaf.dylib', '-o', leaf, source]);
      }
      symlinkSync(common, join(choice, 'libgstcommon.dylib'));
      const source = join(root, `shared-parent-${index}.c`);
      writeFileSync(source, `int gst_common(void); int gst_parent_${index}(void) { return gst_common(); }\n`);
      const parent = join(fixture.lib, `libgstparent${index}.dylib`);
      run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
        `-Wl,-install_name,@rpath/libgstparent${index}.dylib`,
        `-Wl,-rpath,@loader_path/../choice-${index}`, '-o', parent, source, common]);
      return parent;
    });
    const main = join(root, 'shared-main.c');
    writeFileSync(main, '#include <stdio.h>\nint gst_parent_0(void); int gst_parent_1(void); int main(void) { printf("%d,%d\\n", gst_parent_0(), gst_parent_1()); return 0; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, main, ...parents]);
    assert.equal(runCleanSidecar(fixture.sidecar), '11,11', 'dyld reuses the already-loaded shared image');
    const frameworks = join(root, 'Frameworks');
    mkdirSync(frameworks);
    assert.throws(
      () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch)),
      /different sources under different loader paths|conflicting dylib names/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('static macOS sidecars do not retain an orphan GStreamer bundle', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-static-'));
  try {
    const source = join(root, 'main.c');
    const sidecar = join(root, 'bridge-server');
    const frameworks = join(root, 'Frameworks');
    const libraryDirectory = join(frameworks, 'GStreamerLibraries');
    const pluginDirectory = join(frameworks, 'GStreamerPlugins');
    writeFileSync(source, 'int main(void) { return 0; }\n');
    universal(root, sidecar, source);
    mkdirSync(libraryDirectory, { recursive: true });
    mkdirSync(pluginDirectory, { recursive: true });
    assert.equal(prepareMacGStreamerRuntime(sidecar, frameworks, 'arm64').bundled, false);
    assert.equal(run('/usr/bin/lipo', ['-archs', sidecar]), 'arm64');
    assert.equal(existsSync(libraryDirectory), false);
    assert.equal(existsSync(pluginDirectory), false);
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', sidecar]);
    assert.equal(verifyMacGStreamerRuntime(frameworks, sidecar).bundled, false);
    mkdirSync(libraryDirectory);
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, sidecar), /libraries exist without the plugin directory/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a sidecar cannot hide indirect GStreamer dependencies behind the static link mode', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-indirect-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const wrapperSource = join(root, 'wrapper.c');
    const mainSource = join(root, 'indirect-main.c');
    const wrapper = join(fixture.lib, 'libwrapper.dylib');
    writeFileSync(wrapperSource, 'int gst_fixture(void); int wrapped(void) { return gst_fixture(); }\n');
    writeFileSync(mainSource, 'int wrapped(void); int main(void) { return wrapped() - 1; }\n');
    run('clang', ['-arch', arch, '-dynamiclib', '-Wl,-headerpad_max_install_names',
      '-Wl,-install_name,@rpath/libwrapper.dylib', `-Wl,-rpath,${fixture.lib}`,
      '-o', wrapper, wrapperSource, fixture.base]);
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', `-Wl,-rpath,${fixture.lib}`,
      '-o', fixture.sidecar, mainSource, wrapper]);
    runCleanSidecar(fixture.sidecar);
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    assert.throws(
      () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch)),
      /unbundled non-system dependency.*@rpath\/libwrapper/,
    );
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', fixture.sidecar]);
    assert.throws(
      () => verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }),
      /unbundled non-system dependency.*@rpath\/libwrapper/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a sidecar with direct dynamic GStreamer cannot pass without its plugin bundle', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-missing-bundle-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
    signFixture(fixture.sidecar, prepared);
    rmSync(prepared.pluginDirectory, { recursive: true });
    assert.throws(
      () => verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: arch }),
      /packaged GStreamer plugin is missing/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('real universal Mach-O dependencies are bundled and verified for the selected slice', macOnly, () => {
  for (const arch of ['arm64', 'x86_64']) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi gstreamer mac-'));
    try {
      const fixture = sdkFixture(root);
      const original = run('/usr/bin/otool', ['-L', join(fixture.plugins, pluginNames[0])]);
      assert.match(original, /\(architecture arm64\):/);
      assert.match(original, /\(architecture x86_64\):/);
      const frameworks = join(root, 'App.app', 'Contents', 'Frameworks');
      mkdirSync(frameworks, { recursive: true });
      const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, arch));
      assert.equal(prepared.bundled, true);
      assert.equal(prepared.plugins.length, pluginNames.length);
      assert.deepEqual(prepared.libraries.map((path) => path.split('/').at(-1)), ['libgstfixture.dylib']);
      for (const binary of [fixture.sidecar, ...prepared.libraries, ...prepared.plugins]) {
        assert.equal(run('/usr/bin/lipo', ['-archs', binary]), arch);
        assert.doesNotMatch(run('/usr/bin/otool', ['-L', binary]), /GStreamer SDK/);
      }
      signFixture(fixture.sidecar, prepared);
      assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar).bundled, true);
      if ((process.arch === 'arm64' && arch === 'arm64') || (process.arch === 'x64' && arch === 'x86_64')) {
        const env = { ...process.env };
        for (const name of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH']) delete env[name];
        execFileSync(fixture.sidecar, [], { env, stdio: 'pipe' });
      }

      // The validator must still reject actual foreign loads and missing edges.
      const plugin = prepared.plugins[0];
      run('/usr/bin/install_name_tool', ['-change', '@rpath/libgstfixture.dylib', '/tmp/foreign.dylib', plugin]);
      assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /external load path/);
      run('/usr/bin/install_name_tool', ['-change', '/tmp/foreign.dylib', '@rpath/libgstfixture.dylib', plugin]);
      rmSync(prepared.libraries[0]);
      assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /dependency is missing/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('real Mach-O packaging rejects a wrong-architecture plugin and an SDK symlink escape', macOnly, () => {
  for (const variant of ['wrong-architecture', 'outside-sdk']) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-negative-'));
    try {
      const fixture = sdkFixture(root);
      const plugin = join(fixture.plugins, pluginNames[0]);
      rmSync(plugin);
      if (variant === 'wrong-architecture') {
        run('/usr/bin/lipo', [join(root, 'plugin.dylib'), '-thin', 'x86_64', '-output', plugin]);
      } else {
        symlinkSync(join(root, 'plugin.dylib'), plugin);
      }
      const frameworks = join(root, 'Frameworks');
      mkdirSync(frameworks);
      assert.throws(
        () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, 'arm64')),
        variant === 'wrong-architecture' ? /expected arm64/ : /plugin resolves outside configured SDK/,
      );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('staging preserves the original signed sidecar and rejects aliases to it', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-source-'));
  try {
    const fixture = sdkFixture(root);
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', fixture.sidecar]);
    const before = hashFile(fixture.sidecar);
    run('/usr/bin/codesign', ['--verify', '--strict', fixture.sidecar]);
    const frameworks = join(root, 'staging', 'App.app', 'Contents', 'Frameworks');
    const bin = join(root, 'staging', 'App.app', 'Contents', 'Resources', 'bin');
    mkdirSync(frameworks, { recursive: true });
    mkdirSync(bin, { recursive: true });
    const staged = join(bin, 'bridge-server');
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const prepared = withSdk(fixture.sdk, () => stageDarwinSidecar(fixture.sidecar, staged, frameworks, arch, join(root, 'staging')));
    assert.equal(hashFile(fixture.sidecar), before);
    run('/usr/bin/codesign', ['--verify', '--strict', fixture.sidecar]);
    assert.notEqual(hashFile(staged), before);
    signFixture(staged, prepared);
    assert.equal(verifyMacGStreamerRuntime(frameworks, staged, {
      expectedArch: arch, appRoot: join(root, 'staging', 'App.app'),
    }).bundled, true);
    const alias = join(bin, 'alias');
    linkSync(fixture.sidecar, alias);
    assert.throws(() => stageDarwinSidecar(fixture.sidecar, alias, frameworks, arch, join(root, 'staging')), /destination already exists/);
    assert.throws(() => prepareMacFixture(alias, frameworks, arch, {
      stageRoot: join(root, 'staging'), sourceSidecar: fixture.sidecar,
    }), /must not alias the source executable/);
    assert.equal(hashFile(fixture.sidecar), before);
    run('/usr/bin/codesign', ['--verify', '--strict', fixture.sidecar]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('generic and signed staging resolve source-relative runpaths from the original sidecar', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-source-relative-rpath-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const sourceDir = join(fixture.sdk, 'bin');
    mkdirSync(sourceDir);
    const original = join(sourceDir, 'bridge-server');
    const main = join(root, 'source-relative.c');
    writeFileSync(main, 'int gst_fixture(void); int main(void) { return gst_fixture() - 1; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', '-Wl,-rpath,@executable_path/../lib',
      '-o', original, main, fixture.base]);
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', original]);
    const originalHash = hashFile(original);
    runCleanSidecar(original);
    for (const [label, stage] of [['generic', stageDarwinSidecar], ['signed', stageSignedSidecar]]) {
      const stageRoot = join(root, `stage-${label}`);
      const app = join(stageRoot, 'Test.app');
      const contents = join(app, 'Contents');
      const frameworks = join(contents, 'Frameworks');
      const staged = join(contents, 'Resources', 'bin', 'bridge-server');
      mkdirSync(frameworks, { recursive: true });
      const prepared = withSdk(fixture.sdk, () => stage(original, staged, frameworks, arch, stageRoot));
      signFixture(staged, prepared);
      assert.equal(verifyMacGStreamerRuntime(frameworks, staged, { expectedArch: arch, appRoot: app }).bundled, true);
      runCleanSidecar(staged);
      assert.equal(hashFile(original), originalHash);
      run('/usr/bin/codesign', ['--verify', '--strict', original]);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('dependency analysis uses the copied sidecar when the source changes after copying', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-source-snapshot-'));
  try {
    const fixture = sdkFixture(root);
    const arch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    const sourceDir = join(fixture.sdk, 'bin');
    mkdirSync(sourceDir);
    const original = join(sourceDir, 'bridge-server');
    const main = join(root, 'snapshot-main.c');
    writeFileSync(main, 'int gst_fixture(void); int main(void) { return gst_fixture() - 1; }\n');
    run('clang', ['-arch', arch, '-Wl,-headerpad_max_install_names', '-Wl,-rpath,@executable_path/../lib',
      '-o', original, main, fixture.base]);
    const stageRoot = join(root, 'stage');
    const app = join(stageRoot, 'Test.app');
    const contents = join(app, 'Contents');
    const frameworks = join(contents, 'Frameworks');
    const staged = join(contents, 'Resources', 'bin', 'bridge-server');
    mkdirSync(frameworks, { recursive: true });
    mkdirSync(dirname(staged), { recursive: true });
    copyFileSync(original, staged);

    // Simulate an independently rebuilt source after the copy. Its load
    // commands no longer describe the executable that will be packaged.
    writeFileSync(main, 'int main(void) { return 0; }\n');
    run('clang', ['-arch', arch, '-o', original, main]);
    const prepared = withSdk(fixture.sdk, () => prepareMacFixture(staged, frameworks, arch, {
      stageRoot, sourceSidecar: original,
    }));
    assert.equal(prepared.bundled, true);
    signFixture(staged, prepared);
    assert.equal(verifyMacGStreamerRuntime(frameworks, staged, { expectedArch: arch, appRoot: app }).bundled, true);
    runCleanSidecar(staged);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('generic and signed staging reject symlink escapes before changing external files', macOnly, () => {
  for (const [label, stage] of [['generic', stageDarwinSidecar], ['signed', stageSignedSidecar]]) {
    for (const variant of ['destination', 'frameworks', 'library-directory', 'bin-directory']) {
      const root = mkdtempSync(join(tmpdir(), 'lingxi-stage-symlink-'));
      try {
        const source = join(root, 'original');
        writeFileSync(source, 'source bytes', { mode: 0o755 });
        const outside = join(root, 'outside');
        mkdirSync(outside);
        const sentinel = join(outside, 'sentinel');
        writeFileSync(sentinel, 'must remain unchanged');
        const sourceHash = hashFile(source);
        const sentinelHash = hashFile(sentinel);
        const stageRoot = join(root, `stage-${label}`);
        const contents = join(stageRoot, 'Test.app', 'Contents');
        const resources = join(contents, 'Resources');
        const frameworks = join(contents, 'Frameworks');
        mkdirSync(resources, { recursive: true });
        if (variant === 'frameworks') symlinkSync(outside, frameworks);
        else mkdirSync(frameworks);
        if (variant === 'library-directory') symlinkSync(outside, join(frameworks, 'GStreamerLibraries'));
        const bin = join(resources, 'bin');
        if (variant === 'bin-directory') symlinkSync(outside, bin);
        else mkdirSync(bin);
        const destination = join(bin, 'bridge-server');
        if (variant === 'destination') symlinkSync(sentinel, destination);
        assert.throws(() => stage(source, destination, frameworks, 'arm64', stageRoot), /symbolic link/);
        assert.equal(hashFile(source), sourceHash);
        assert.equal(hashFile(sentinel), sentinelHash);
        assert.equal(existsSync(join(outside, 'bridge-server')), false);
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    }
  }
});

test('generic and signed paths reject a symlink above the stage root before reset or copy', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-stage-ancestor-'));
  try {
    const source = join(root, 'source');
    writeFileSync(source, 'source bytes', { mode: 0o755 });
    const sourceHash = hashFile(source);
    for (const [label, reset, stage] of [
      ['generic', resetDesktopOutputs, stageDarwinSidecar],
      ['signed', resetSignedOutputs, stageSignedSidecar],
    ]) {
      const outside = join(root, `outside-${label}`);
      const realStage = join(outside, 'stage');
      const contents = join(realStage, 'Test.app', 'Contents');
      const frameworks = join(contents, 'Frameworks');
      mkdirSync(frameworks, { recursive: true });
      const sentinel = join(realStage, 'sentinel');
      writeFileSync(sentinel, 'must remain unchanged');
      const before = hashFile(sentinel);
      const alias = join(root, `redirect-${label}`);
      symlinkSync(outside, alias);
      const stageRoot = join(alias, 'stage');
      const destination = join(stageRoot, 'Test.app', 'Contents', 'Resources', 'bin', 'bridge-server');
      const aliasedFrameworks = join(stageRoot, 'Test.app', 'Contents', 'Frameworks');
      const paths = label === 'generic'
        ? { stageRoot, artifactPath: join(root, `${label}.zip`), checksumPath: join(root, `${label}.sha`), metadataPath: join(root, `${label}.json`) }
        : { appContainer: stageRoot, zipPath: join(root, `${label}.zip`), checksumPath: join(root, `${label}.sha`) };
      assert.throws(() => reset(paths, source), /symbolic link/);
      assert.throws(() => stage(source, destination, aliasedFrameworks, 'arm64', stageRoot), /symbolic link/);
      assert.equal(hashFile(sentinel), before);
      assert.equal(hashFile(source), sourceHash);
      assert.equal(existsSync(destination), false);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('both resets preserve sources inside or aliased to their outputs', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-stage-reset-'));
  try {
    const external = join(root, 'external-sidecar');
    writeFileSync(external, 'untouched source');
    const hash = hashFile(external);
    for (const [label, preflight, reset] of [
      ['generic', preflightDesktopSidecar, resetDesktopOutputs],
      ['signed', preflightSignedSidecar, resetSignedOutputs],
    ]) {
      const output = join(root, `output-${label}`);
      const container = join(output, 'stage');
      mkdirSync(container, { recursive: true });
      const actual = join(container, 'bridge-server');
      writeFileSync(actual, 'inside output');
      const actualHash = hashFile(actual);
      const outsideAlias = join(root, `alias-${label}`);
      symlinkSync(actual, outsideAlias);
      const insideAlias = join(container, 'external-alias');
      symlinkSync(external, insideAlias);
      const paths = label === 'generic'
        ? { stageRoot: container, artifactPath: join(output, 'app.zip'), checksumPath: join(output, 'app.sha'), metadataPath: join(output, 'metadata') }
        : { releaseRoot: output, stem: 'stage', appContainer: container, appPath: join(container, 'LingXi Code.app'),
          zipPath: join(output, 'stage.zip'), checksumPath: join(output, 'stage.zip.sha256') };
      for (const source of [actual, outsideAlias, insideAlias]) {
        assert.throws(() => reset(paths, source), /inside a package output path/);
      }
      assert.doesNotThrow(() => preflight(paths, external));
      assert.equal(hashFile(actual), actualHash);
      assert.equal(hashFile(external), hash);
      reset(paths, external);
      assert.equal(existsSync(actual), false);
      assert.equal(hashFile(external), hash);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('real Electron distribution can be signed inside-out and rejects a broken nested signature', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-real-electron-signing-'));
  try {
    const source = join(electronRoot, 'node_modules', 'electron', 'dist', 'Electron.app');
    assert.ok(existsSync(source), 'the installed Electron.app is required on macOS');
    const app = join(root, 'Electron.app');
    copyElectronApp(source, app);
    const frameworks = join(app, 'Contents', 'Frameworks');
    const frameworkBinary = join(frameworks, 'Electron Framework.framework', 'Versions', 'A', 'Electron Framework');
    assert.ok(existsSync(frameworkBinary), 'Electron framework binary exists in the real distribution');
    run('/usr/bin/codesign', ['--remove-signature', frameworkBinary]);
    assert.throws(() => verifyMacAppSignature(app));
    const signingDirectory = join(root, 'signing');
    mkdirSync(signingDirectory);
    const entitlements = signGenericDesktopFrameworks(frameworks, signingDirectory);
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', '--options', 'runtime',
      '--entitlements', entitlements, app]);
    verifyMacAppSignature(app);
    const expectedArch = process.arch === 'x64' ? 'x86_64' : 'arm64';
    assert.ok(verifyMacFrameworkArchitectures(frameworks, expectedArch).length > 0);
    const env = { ...process.env, ELECTRON_RUN_AS_NODE: '1' };
    for (const name of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'DYLD_FRAMEWORK_PATH']) delete env[name];
    const output = execFileSync(join(app, 'Contents', 'MacOS', 'Electron'), ['--version'], {
      env, encoding: 'utf8', stdio: 'pipe',
    });
    assert.match(output.trim(), /^v\d+\./);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('macOS verification rejects wrong target, escaped Frameworks and tampered signed nested code', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-signature-'));
  try {
    const fixture = sdkFixture(root);
    const app = join(root, 'App.app');
    const contents = join(app, 'Contents');
    const frameworks = join(contents, 'Frameworks');
    mkdirSync(frameworks, { recursive: true });
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, 'arm64'));
    signFixture(fixture.sidecar, prepared);
    const executable = join(contents, 'MacOS', 'Fixture');
    mkdirSync(dirname(executable), { recursive: true });
    const appSource = join(root, 'app.c');
    writeFileSync(appSource, 'int main(void) { return 0; }\n');
    run('clang', ['-arch', 'arm64', '-o', executable, appSource]);
    writeFileSync(join(contents, 'Info.plist'), '<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.example.fixture</string><key>CFBundleExecutable</key><string>Fixture</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>');
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', app]);
    verifyMacAppSignature(app);
    assert.equal(verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: 'arm64', appRoot: app }).bundled, true);
    assert.throws(
      () => verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: 'x86_64', appRoot: app }),
      /architecture arm64; expected x86_64/,
    );
    // Ad-hoc is valid for the test-only Desktop package, but cannot satisfy
    // an explicitly required signed-release team identity.
    assert.throws(() => verifyTeamIdentifier(prepared.libraries[0], 'AZ4AX7J833'), /unexpected TeamIdentifier/);

    const externalFrameworks = join(root, 'outside-frameworks');
    const otherApp = join(root, 'other.app');
    const otherContents = join(otherApp, 'Contents');
    mkdirSync(externalFrameworks);
    mkdirSync(otherContents, { recursive: true });
    symlinkSync(externalFrameworks, join(otherContents, 'Frameworks'));
    assert.throws(
      () => verifyMacGStreamerRuntime(join(otherContents, 'Frameworks'), fixture.sidecar, { appRoot: otherApp }),
      /Frameworks directory is a symlink/,
    );

    // Corrupt executable code without damaging its load commands. otool still
    // reads the graph, while codesign must reject the changed image.
    const binary = prepared.libraries[0];
    const loadCommands = run('/usr/bin/otool', ['-l', binary]);
    const section = loadCommands.match(/sectname __text\s+segname __TEXT\s+addr \S+\s+size \S+\s+offset (\d+)/);
    assert.ok(section, 'fixture has a real __TEXT,__text section');
    const data = readFileSync(binary);
    const offset = Number(section[1]);
    assert.ok(offset > 0 && offset < data.length);
    data[offset] ^= 1;
    writeFileSync(binary, data);
    assert.throws(() => run('/usr/bin/codesign', ['--verify', '--strict', binary]));
    assert.throws(() => verifyMacAppSignature(app));
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar, { expectedArch: 'arm64', appRoot: app }));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('real Mach-O verification rejects traversal and symlinks outside the bundled library directory', macOnly, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-paths-'));
  try {
    const fixture = sdkFixture(root);
    const frameworks = join(root, 'Frameworks');
    mkdirSync(frameworks);
    const prepared = withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, 'arm64'));
    const plugin = prepared.plugins[0];
    const outside = join(frameworks, 'outside.dylib');
    copyFileSync(prepared.libraries[0], outside);
    run('/usr/bin/install_name_tool', ['-change', '@rpath/libgstfixture.dylib', '@rpath/../outside.dylib', plugin]);
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /invalid library name/);
    run('/usr/bin/install_name_tool', ['-change', '@rpath/../outside.dylib', '@rpath/libgstfixture.dylib', plugin]);
    rmSync(prepared.libraries[0]);
    symlinkSync(outside, prepared.libraries[0]);
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /dependency resolves outside|binary resolves outside/);
    assert.equal(existsSync(outside), true);
    rmSync(prepared.libraries[0]);
    copyFileSync(outside, prepared.libraries[0]);
    const extra = join(prepared.pluginDirectory, 'libgstunexpected.dylib');
    copyFileSync(plugin, extra);
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /unexpected entries/);
    rmSync(extra);
    // Non-.dylib names cannot evade dependency graph verification either.
    const unverified = join(frameworks, 'GStreamerLibraries', 'additional-library');
    copyFileSync(plugin, unverified);
    run('/usr/bin/install_name_tool', ['-change', '@rpath/libgstfixture.dylib', '/tmp/foreign.dylib', unverified]);
    assert.throws(() => verifyMacGStreamerRuntime(frameworks, fixture.sidecar), /external load path/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('real Mach-O dependency graph rejects external and conflicting SDK dylibs', macOnly, () => {
  for (const variant of ['external', 'collision']) {
    const root = mkdtempSync(join(tmpdir(), 'lingxi-gstreamer-graph-'));
    try {
      const fixture = sdkFixture(root);
      const directories = variant === 'external'
        ? [join(root, 'outside-sdk')]
        : [join(fixture.sdk, 'first'), join(fixture.sdk, 'second')];
      const libraries = directories.map((directory, index) => {
        mkdirSync(directory, { recursive: true });
        const source = join(directory, `source-${index}.c`);
        const library = join(directory, 'libgstcollision.dylib');
        writeFileSync(source, `int dependency_${index}(void) { return ${index}; }\n`);
        run('clang', [
          '-arch', 'arm64', '-dynamiclib', '-Wl,-headerpad_max_install_names',
          `-Wl,-install_name,${library}`, '-o', library, source,
        ]);
        return library;
      });
      const main = join(root, 'graph-main.c');
      const declarations = libraries.map((_, index) => `int dependency_${index}(void);`).join('\n');
      const calls = libraries.map((_, index) => `dependency_${index}()`).join(' + ');
      writeFileSync(main, `${declarations}\nint main(void) { return ${calls}; }\n`);
      run('clang', ['-arch', 'arm64', '-Wl,-headerpad_max_install_names', '-o', fixture.sidecar, main, ...libraries]);
      const inspection = run('/usr/bin/otool', ['-L', fixture.sidecar]);
      for (const library of libraries) assert.ok(inspection.includes(library));
      const frameworks = join(root, 'Frameworks');
      mkdirSync(frameworks);
      assert.throws(
        () => withSdk(fixture.sdk, () => prepareMacGStreamerRuntime(fixture.sidecar, frameworks, 'arm64')),
        variant === 'external' ? /resolves outside LINGXI_GSTREAMER_SDK_MACOS/ : /conflicting (?:sources|dylib names)/,
      );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});
