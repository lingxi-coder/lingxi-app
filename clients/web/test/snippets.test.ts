import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';

const sourceFiles = [
  new URL('../src/pages/PublicPages.tsx', import.meta.url),
  new URL('../src/pages/DocsPage.tsx', import.meta.url),
];

test('copyable API snippets do not contain patch markers and declare JSON content', async () => {
  const sources = await Promise.all(sourceFiles.map((file) => readFile(file, 'utf8')));
  for (const source of sources) {
    assert.equal(source.includes('\\n+  -H'), false);
    assert.match(source, /Content-Type/);
  }
});
