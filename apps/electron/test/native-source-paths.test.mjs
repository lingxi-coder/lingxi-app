import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, symlinkSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { nativeSourcePrefixMaps, xcodeArgument } from '../scripts/desktop-crashlytics.mjs';

test('native __FILE__ cannot disclose physical paths through a symlinked cache', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'native paths '));
  try {
    const project = join(root, 'project');
    const physical = join(root, 'shared cache');
    mkdirSync(project); mkdirSync(physical);
    const logical = join(project, 'cache');
    symlinkSync(physical, logical);
    const source = join(realpathSync(physical), 'fixture.c');
    writeFileSync(source, '#include <stdio.h>\nint main(void) { puts(__FILE__); }\n');
    const binary = join(root, 'fixture');
    // Reproduce SPM handing the compiler the physical checkout path.
    // Control: the previous project-only mapping leaves the shared path intact.
    execFileSync('xcrun', ['clang', `-ffile-prefix-map=${project}=.`, source, '-o', binary]);
    assert.equal(execFileSync(binary, [], { encoding: 'utf8' }), source + '\n');
    const maps = nativeSourcePrefixMaps([project, logical]);
    execFileSync('xcrun', ['clang', ...maps.map(map => `-ffile-prefix-map=${map}`), source, '-o', binary]);
    const output = execFileSync(binary, [], { encoding: 'utf8' });
    assert.match(output, /^(\.\/)?fixture\.c\n$/);
    // Xcode's argument-list parsing must preserve spaces in each mapping.
    const settings = maps.map(map => xcodeArgument(`-ffile-prefix-map=${map}`)).join(' ');
    const split = execFileSync('python3', ['-c', 'import json,shlex,sys; print(json.dumps(shlex.split(sys.argv[1])))', settings], { encoding: 'utf8' });
    assert.deepEqual(JSON.parse(split), maps.map(map => `-ffile-prefix-map=${map}`));
  } finally { rmSync(root, { recursive: true, force: true }); }
});
