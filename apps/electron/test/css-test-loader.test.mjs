import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

test('renderer stylesheets can be imported while their contents remain readable', async () => {
  const stylesheet = new URL('../src/renderer/components/CommandResultPanel.css', import.meta.url);
  await import(stylesheet.href);
  assert.ok(readFileSync(stylesheet, 'utf8').length > 0);
});

test('missing stylesheets still fail module resolution', async () => {
  await assert.rejects(import('./fixtures/nonexistent-stylesheet.css'), { code: 'ERR_MODULE_NOT_FOUND' });
});
