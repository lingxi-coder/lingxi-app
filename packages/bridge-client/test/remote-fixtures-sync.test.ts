/**
 * The committed fixtures are exactly what the generator produces, so the Rust
 * client (which reads the same JSON) and this package cannot drift apart.
 * To change a vector, edit test/gen-remote-fixtures.mjs and run it.
 */

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { test } from 'node:test';

const directory = new URL('./fixtures/remote/', import.meta.url);
const generatorPath = './gen-remote-fixtures.mjs'; // a variable keeps the type checker out of an untyped script

test('test/fixtures/remote/*.json are byte for byte what gen-remote-fixtures.mjs generates', async () => {
  const { buildFixtures } = (await import(generatorPath)) as { buildFixtures(): Record<string, unknown> };
  const built = buildFixtures();
  const committed = readdirSync(directory).filter((name) => name.endsWith('.json')).sort();
  assert.deepEqual(committed, Object.keys(built).sort(), 'one file per generated fixture, and no strays');
  for (const [name, value] of Object.entries(built)) {
    assert.equal(readFileSync(new URL(name, directory), 'utf8'), `${JSON.stringify(value, null, 2)}\n`, `${name} is stale: run node test/gen-remote-fixtures.mjs`);
  }
});

test('the generator is deterministic', async () => {
  const { buildFixtures } = (await import(generatorPath)) as { buildFixtures(): Record<string, unknown> };
  assert.deepEqual(buildFixtures(), buildFixtures());
});
