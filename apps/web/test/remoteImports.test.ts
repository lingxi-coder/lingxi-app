import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { collectImportGraph, packageNameOf, runtimeSpecifiers, undeclaredImports } from './support/importGraph';

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const entries = ['src/pages/RemotePage.tsx', 'src/pages/RemoteSignInPage.tsx'].map((entry) => path.join(webRoot, entry));

test('everything the Remote Control pages import by bare name is declared by apps/web', () => {
  const graph = collectImportGraph({ entries, webRoot });
  // A walk that found nothing would pass vacuously: it must have reached the shared code.
  const reached = [...graph.files].map((file) => path.relative(path.resolve(webRoot, '..', '..'), file));
  assert.ok(reached.some((file) => file.startsWith(path.join('packages', 'bridge-client', 'src', 'remote'))), 'bridge-client remote code not reached');
  assert.ok(reached.some((file) => file.startsWith(path.join('apps', 'electron', 'src'))), 'the shared reducer was not reached');
  assert.ok(graph.bare.some((entry) => entry.pkg === 'react') && graph.bare.some((entry) => entry.pkg === 'firebase'), 'no bare imports collected');

  const missing = undeclaredImports(graph, webRoot).map((entry) => `${entry.specifier} (from ${path.relative(webRoot, entry.from)}${entry.builtin ? ', a Node builtin' : ''})`);
  assert.deepEqual(missing, []);
});

test('the walk does not reach the Electron-only or Node-only entry points', () => {
  const graph = collectImportGraph({ entries, webRoot });
  const bridgeIndex = [...graph.files].filter((file) => file.endsWith(path.join('packages', 'bridge-client', 'src', 'index.ts')));
  assert.deepEqual(bridgeIndex, [], 'the bridge-client index pulls in ws and node builtins');
});

test('type-only imports are not runtime imports', () => {
  const specifiers = runtimeSpecifiers(
    'x.ts',
    [
      "import type { A } from 'only-types';",
      "import { type B } from 'also-types';",
      "import { type C, d } from 'mixed';",
      "export type { E } from 'reexported-types';",
      "export { f } from 'reexported';",
      "const g = await import('dynamic');",
      "const h = require('required');",
    ].join('\n'),
  );
  assert.deepEqual(specifiers, ['mixed', 'reexported', 'dynamic', 'required']);
});

test('package names', () => {
  assert.equal(packageNameOf('react'), 'react');
  assert.equal(packageNameOf('firebase/auth'), 'firebase');
  assert.equal(packageNameOf('@scope/name/deep/file'), '@scope/name');
});

// ─────────────────────────────────────────────────────────────────────────────
// Negative: the check must fail when a file outside apps/web needs a package that
// apps/web does not declare, even though a symlinked node_modules resolves it.
// ─────────────────────────────────────────────────────────────────────────────

function syntheticRepo(webDependencies: Record<string, string>): { root: string; web: string; entry: string; cleanup(): void } {
  const root = mkdtempSync(path.join(tmpdir(), 'lingxi-imports-'));
  const web = path.join(root, 'apps', 'web');
  const shared = path.join(root, 'packages', 'shared');
  const farm = path.join(root, 'other-checkout', 'node_modules', 'ws');
  mkdirSync(path.join(web, 'src'), { recursive: true });
  mkdirSync(path.join(shared, 'src'), { recursive: true });
  mkdirSync(farm, { recursive: true });
  writeFileSync(path.join(farm, 'package.json'), JSON.stringify({ name: 'ws', version: '1.0.0', main: 'index.js' }));
  writeFileSync(path.join(farm, 'index.js'), 'module.exports = {};\n');
  // The shared package finds `ws` through a symlinked node_modules, like the worktree's farm.
  symlinkSync(path.join(root, 'other-checkout', 'node_modules'), path.join(shared, 'node_modules'));
  writeFileSync(path.join(web, 'package.json'), JSON.stringify({ name: 'web', dependencies: webDependencies }));
  writeFileSync(path.join(shared, 'src', 'lib.ts'), "import WebSocket from 'ws';\nexport const socket = WebSocket;\n");
  const entry = path.join(web, 'src', 'entry.ts');
  writeFileSync(entry, "import { socket } from '../../../packages/shared/src/lib';\nexport const used = socket;\n");
  return { root, web, entry, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test('negative: a package that only a symlinked node_modules provides is reported', () => {
  const repo = syntheticRepo({ react: '^18.0.0' });
  try {
    const graph = collectImportGraph({ entries: [repo.entry], webRoot: repo.web });
    const ws = graph.bare.find((entry) => entry.pkg === 'ws');
    assert.ok(ws, 'the import of ws was not collected');
    // It resolves (that is the masking), and is still reported.
    assert.ok(ws.resolvedTo !== undefined && !ws.resolvedTo.startsWith(repo.web), `expected ws to resolve outside apps/web, got ${ws.resolvedTo}`);
    assert.deepEqual(undeclaredImports(graph, repo.web).map((entry) => entry.pkg), ['ws']);
  } finally {
    repo.cleanup();
  }
});

test('negative: declaring the package makes the same graph pass', () => {
  const repo = syntheticRepo({ ws: '^8.0.0' });
  try {
    const graph = collectImportGraph({ entries: [repo.entry], webRoot: repo.web });
    assert.deepEqual(undeclaredImports(graph, repo.web), []);
  } finally {
    repo.cleanup();
  }
});

test('negative: a Node builtin is reported even if the manifest names it', () => {
  const repo = syntheticRepo({ ws: '^8.0.0', 'node:fs': '*' });
  try {
    writeFileSync(repo.entry, "import { readFileSync } from 'node:fs';\nexport const f = readFileSync;\n");
    const graph = collectImportGraph({ entries: [repo.entry], webRoot: repo.web });
    assert.deepEqual(undeclaredImports(graph, repo.web).map((entry) => entry.specifier), ['node:fs']);
  } finally {
    repo.cleanup();
  }
});
