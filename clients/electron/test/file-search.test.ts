import { afterEach, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  MAX_FILE_SEARCH_QUERY_LENGTH,
  WorkspaceFileSearch,
} from '../src/main/file-search';

const temporaryDirectories: string[] = [];

function workspace(): string {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-file-search-'));
  temporaryDirectories.push(root);
  return root;
}

function file(root: string, relative: string): void {
  const parts = relative.split('/');
  parts.pop();
  if (parts.length > 0) mkdirSync(join(root, ...parts), { recursive: true });
  writeFileSync(join(root, relative), relative);
}

afterEach(() => {
  for (const path of temporaryDirectories.splice(0)) rmSync(path, { recursive: true, force: true });
});

test('workspace file search ranks filename prefixes and returns portable relative paths', async () => {
  const root = workspace();
  file(root, 'docs/application-notes.md');
  file(root, 'src/app.ts');
  file(root, 'src/nested/AppShell.tsx');
  file(root, 'src/unrelated.ts');

  const result = await new WorkspaceFileSearch().search(root, 'app');

  assert.deepEqual(result.files, [
    'src/app.ts',
    'src/nested/AppShell.tsx',
    'docs/application-notes.md',
  ]);
  assert.equal(result.truncated, false);
});

test('workspace file search skips dependency/build roots and never follows symlinks', async () => {
  const root = workspace();
  const outside = workspace();
  file(root, 'src/visible.ts');
  file(root, 'node_modules/pkg/visible-dependency.ts');
  file(root, 'target/debug/visible-build.rs');
  file(root, 'feature/mission/build/generated/visible-generated.kt');
  file(root, 'app/.gradle/caches/visible-cache.bin');
  file(outside, 'visible-secret.txt');
  symlinkSync(join(outside, 'visible-secret.txt'), join(root, 'visible-link.txt'));

  const result = await new WorkspaceFileSearch().search(root, 'visible');

  assert.deepEqual(result.files, ['src/visible.ts']);
});

test('long filename queries do not return unrelated cross-path fuzzy matches', async () => {
  const root = workspace();
  file(root, 'sdk/map/src/main/RouteAccumulator.kt');
  file(root, 'feature/mission/src/Plants_constraint.kt');
  file(root, 'sdk/uav/src/SimulatorControlWidget.kt');
  file(root, 'sdk/uav/build/kotlin/compileDebug/cacheable/caches-jvm/source-to-output.tab.keystream');
  file(root, 'lib/map/build/generated/source/RouteAccumulator.kt');

  const result = await new WorkspaceFileSearch().search(root, 'accumulator.kt');

  assert.deepEqual(result.files, ['sdk/map/src/main/RouteAccumulator.kt']);
  assert.equal(result.truncated, false);
});

test('hidden paths only appear when the query explicitly includes a dot', async () => {
  const root = workspace();
  file(root, '.github/workflows/check.yml');
  file(root, 'src/check.ts');
  const search = new WorkspaceFileSearch();

  assert.equal((await search.search(root, '')).files.includes('.github/workflows/check.yml'), false);
  assert.deepEqual((await search.search(root, 'check')).files, ['src/check.ts']);
  assert.deepEqual((await search.search(root, '.github/check')).files, ['.github/workflows/check.yml']);
});

test('workspace file search cache can be invalidated after files change', async () => {
  const root = workspace();
  file(root, 'one.ts');
  const search = new WorkspaceFileSearch();
  assert.deepEqual((await search.search(root, 'two')).files, []);

  file(root, 'two.ts');
  assert.deepEqual((await search.search(root, 'two')).files, []);
  search.invalidate();
  assert.deepEqual((await search.search(root, 'two')).files, ['two.ts']);
});

test('workspace file search rejects unbounded or NUL-bearing queries', async () => {
  const root = workspace();
  const search = new WorkspaceFileSearch();

  await assert.rejects(() => search.search(root, 'x'.repeat(MAX_FILE_SEARCH_QUERY_LENGTH + 1)), /invalid workspace file query/);
  await assert.rejects(() => search.search(root, 'bad\0query'), /invalid workspace file query/);
});
