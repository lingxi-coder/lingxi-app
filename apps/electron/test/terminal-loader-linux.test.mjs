import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { terminalBrokerEnvironment } from '../src/main/terminal.ts';
import { verifyBundledLinuxDependencies } from '../scripts/packaged-sidecar-smoke.mjs';

test('Linux loader resolves one app-owned ORIGIN entry through separator and token characters', {
  skip: process.platform === 'linux' ? false : 'requires the Linux ELF loader and C compiler',
}, (t) => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-loader-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const resources = join(root, 'install:$ORIGIN;directory', 'resources');
  const bin = join(resources, 'bin');
  const lib = join(resources, 'gstreamer', 'lib');
  const untrusted = join(root, 'untrusted');
  for (const directory of [bin, lib, untrusted]) mkdirSync(directory, { recursive: true });
  const source = join(root, 'library.c');
  const driverSource = join(root, 'driver.c');
  const name = 'libgstreamer-1.0.so.0';
  const executable = join(bin, 'bridge-server');
  const compile = (args) => {
    const result = spawnSync('cc', args, { encoding: 'utf8' });
    assert.equal(result.status, 0, `C fixture compilation failed: ${result.stderr || result.error}`);
  };
  writeFileSync(source, 'int lingxi_loader_probe(void) { return 42; }\n');
  writeFileSync(driverSource, '#include <stdio.h>\nextern int lingxi_loader_probe(void);\nint main(void) { printf("%d\\n", lingxi_loader_probe()); return 0; }\n');
  compile(['-shared', '-fPIC', `-Wl,-soname,${name}`, source, '-o', join(lib, name)]);
  compile([driverSource, `-L${lib}`, `-l:${name}`, '-o', executable]);
  writeFileSync(source, 'int lingxi_loader_probe(void) { return 99; }\n');
  compile(['-shared', '-fPIC', `-Wl,-soname,${name}`, source, '-o', join(untrusted, name)]);

  const env = terminalBrokerEnvironment({ isPackaged: true, resourcesPath: resources }, {
    PATH: process.env.PATH, HOME: root, LD_LIBRARY_PATH: `${untrusted}::${lib}`, LD_PRELOAD: join(untrusted, name),
  });
  assert.equal(env.LD_LIBRARY_PATH, '$ORIGIN/../gstreamer/lib');
  assert.equal(env.LD_PRELOAD, undefined);
  const result = spawnSync(executable, [], { cwd: untrusted, env, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '42', 'must load the app-owned library, not the untrusted one');

  const trace = spawnSync(executable, [], { cwd: untrusted, env: { ...env, LD_TRACE_LOADED_OBJECTS: '1' }, encoding: 'utf8' });
  assert.equal(trace.status, 0, trace.stderr);
  verifyBundledLinuxDependencies(trace.stdout, resources, { runtimeFiles: [{ kind: 'library', packagePath: `lib/${name}` }] });

  const unsafe = spawnSync(executable, [], { cwd: untrusted, env: { ...env, LD_LIBRARY_PATH: lib }, encoding: 'utf8' });
  assert.notEqual(unsafe.status, 0, 'the literal separator-containing path must not masquerade as a safe loader path');
});
