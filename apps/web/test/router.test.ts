import assert from 'node:assert/strict';
import test from 'node:test';
import { normalizePath, resolveRoute } from '../src/app/router';

test('normalizes duplicate slashes, query strings, casing, and trailing slashes', () => {
  assert.equal(normalizePath('CONSOLE//API-KEYS/?tab=all'), '/console/api-keys');
  assert.equal(normalizePath('/docs#quick-start'), '/docs');
});

test('resolves known routes and falls back to the public homepage', () => {
  assert.equal(resolveRoute('/console/usage'), '/console/usage');
  assert.equal(resolveRoute('/not-a-route'), '/');
  assert.equal(resolveRoute('/DOCS/LLM-CLIENT/?from=home'), '/docs/llm-client');
  assert.equal(resolveRoute('/docs/mobile-ios'), '/docs/mobile-ios');
  assert.equal(resolveRoute('/docs/no-such-sdk'), '/');
});
