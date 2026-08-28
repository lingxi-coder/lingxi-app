import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '..');

test('the allowed-command list has exactly one definition', () => {
  const preload = readFileSync(join(root, 'src/preload/index.ts'), 'utf8');
  const rendererTypes = readFileSync(join(root, 'src/renderer/bridge/lingxi.d.ts'), 'utf8');
  const declarations = [preload, rendererTypes].filter((source) =>
    /export type AllowedClientCommand\s*=/.test(source));
  assert.equal(
    declarations.length, 0,
    'AllowedClientCommand must be declared only in src/shared/clientCommands.ts; ' +
    'two copies drifted once already and will drift again',
  );
  const shared = readFileSync(join(root, 'src/shared/clientCommands.ts'), 'utf8');
  assert.match(shared, /export type AllowedClientCommand\s*=/);
});

test('the guard can actually fail', () => {
  const seeded = 'export type AllowedClientCommand = never;';
  assert.ok(
    /export type AllowedClientCommand\s*=/.test(seeded),
    'if this regex does not match a known sample, the guard above proves nothing',
  );
});

test('the shared definition covers every newly-added command and listing kind', () => {
  const shared = readFileSync(join(root, 'src/shared/clientCommands.ts'), 'utf8');
  const newCommands = [
    'update_settings',
    'update_permission_rules',
    'set_default_permission_mode',
    'update_workspace_directories',
    'upsert_mcp_server',
    'remove_mcp_server',
  ];
  for (const name of newCommands) {
    assert.ok(shared.includes(`'${name}'`), `expected AllowedClientCommand to mention '${name}'`);
  }
  const newListingKinds = ['settings', 'mcp', 'skills'];
  for (const kind of newListingKinds) {
    assert.ok(shared.includes(`'${kind}'`), `expected refresh_listings.which to mention '${kind}'`);
  }
});
