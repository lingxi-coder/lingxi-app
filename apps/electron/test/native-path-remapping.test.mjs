import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

test('native remapping rejects whitespace paths before exporting malformed compiler flags', () => {
  const helper = join(dirname(fileURLToPath(import.meta.url)), '../scripts/native-path-remapping.sh');
  for (const index of [0, 1, 2, 3]) {
    const paths = ['/source', '/cargo-home', '/rustup', '/target'];
    paths[index] += ' with spaces';
    const result = spawnSync('/bin/bash', ['-c', `
      source "$1"
      configure_native_path_remapping "$2" "$3" "$4" "$5"
    `, 'native-path-test', helper, ...paths], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /directories without whitespace/);
  }
});

test('native dependency diagnostics omit build paths and retain caller compiler flags', () => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-native-paths-'));
  const helper = join(dirname(fileURLToPath(import.meta.url)), '../scripts/native-path-remapping.sh');
  try {
    const cargoHome = join(directory, 'cargo-home');
    const target = join(directory, 'target');
    mkdirSync(cargoHome);
    mkdirSync(target);
    const source = join(cargoHome, 'dependency.c');
    writeFileSync(source, '#include <stdio.h>\nint main(void) { printf("%d %s\\n", CALLER_FLAG, __FILE__); return 0; }\n');
    execFileSync('/bin/bash', ['-c', `
      set -e
      source "$1"
      export CFLAGS=-DCALLER_FLAG=7 CXXFLAGS=-DCALLER_FLAG=9
      configure_native_path_remapping "$2" "$3" "$4" "$5"
      cc $CFLAGS "$6" -o "$5/c-probe"
      c++ $CXXFLAGS -x c++ "$6" -o "$5/cxx-probe"
    `, 'native-path-test', helper, directory, cargoHome, join(directory, 'rustup'), target, source]);
    for (const [name, flag] of [['c-probe', 7], ['cxx-probe', 9]]) {
      const binary = join(target, name);
      assert.equal(execFileSync(binary, { encoding: 'utf8' }).trim(), `${flag} /cargo-home/dependency.c`);
      assert.equal(readFileSync(binary).includes(Buffer.from(directory)), false);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
