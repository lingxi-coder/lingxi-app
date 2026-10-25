import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { collectImportGraph, runtimeSpecifiers } from './support/importGraph';

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const entry = path.join(webRoot, 'src/main.tsx');

test('the initial public page graph does not load handbook content or its renderer', () => {
  const graph = collectImportGraph({ entries: [entry], webRoot, followDynamicImports: false });
  const files = [...graph.files].map((file) => path.relative(webRoot, file));
  assert.ok(files.includes('src/app/App.tsx') && files.includes('src/app/router.ts'), 'the graph must reach the app and router');
  assert.ok(files.includes('src/pages/PublicPages.tsx'), 'the public pages must remain in the initial graph');
  assert.deepEqual(files.filter((file) => file.startsWith('src/data/docs/') || /(?:DocsPage|DocsLayout)\.tsx$/u.test(file)), []);

  // Verify the walk can still see the deferred documentation, rather than passing on an incomplete graph.
  const full = collectImportGraph({ entries: [entry], webRoot });
  assert.ok(full.files.has(path.join(webRoot, 'src/pages/DocsPage.tsx')));
  assert.ok(full.files.has(path.join(webRoot, 'src/data/docs/index.ts')));
});

test('documentation styling is loaded with the deferred documentation page', () => {
  const appPath = path.join(webRoot, 'src/app/App.tsx');
  const app = readFileSync(appPath, 'utf8');
  assert.ok(runtimeSpecifiers(appPath, app).includes('../docs.css'));
  assert.ok(!runtimeSpecifiers(appPath, app, false).includes('../docs.css'), 'documentation CSS must not be a static import');
  assert.ok(!runtimeSpecifiers(entry, readFileSync(entry, 'utf8')).includes('./docs.css'));
});
