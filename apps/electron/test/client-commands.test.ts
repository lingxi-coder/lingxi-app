import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import {
  ALLOWED_CLIENT_COMMAND_TYPES,
  ALLOWED_REFRESH_LISTING_KINDS,
  CLIENT_COMMAND_DISPOSITIONS,
  CLIENT_EVENT_DISPOSITIONS,
  REFRESH_LISTING_DISPOSITIONS,
} from '../src/shared/clientCommands';

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
    'login',
    'logout',
    'force_compact',
    'list_session_agents',
    'load_session_agent_transcript',
    'update_settings',
    'update_permission_rules',
    'set_default_permission_mode',
    'update_workspace_directories',
    'upsert_mcp_server',
    'remove_mcp_server',
    'ui_attach',
    'ui_detach',
  ];
  for (const name of newCommands) {
    assert.ok(shared.includes(`'${name}'`), `expected AllowedClientCommand to mention '${name}'`);
  }
  const newListingKinds = ['auth', 'settings', 'mcp', 'skills', 'hooks', 'agents'];
  for (const kind of newListingKinds) {
    assert.ok(shared.includes(`'${kind}'`), `expected refresh_listings.which to mention '${kind}'`);
  }
});

test('the allowlists are the exposed subset of the exhaustive desktop disposition tables', () => {
  assert.deepEqual(
    [...ALLOWED_CLIENT_COMMAND_TYPES].sort(),
    Object.entries(CLIENT_COMMAND_DISPOSITIONS)
      .filter(([, disposition]) => disposition === 'exposed')
      .map(([type]) => type)
      .filter((type) => type !== 'refresh_listings')
      .sort(),
  );
  assert.deepEqual(
    [...ALLOWED_REFRESH_LISTING_KINDS].sort(),
    Object.entries(REFRESH_LISTING_DISPOSITIONS)
      .filter(([, disposition]) => disposition === 'exposed')
      .map(([type]) => type)
      .filter((type) => type !== 'models')
      .sort(),
  );
  assert.equal(CLIENT_COMMAND_DISPOSITIONS.clear_session, 'host_private');
  assert.equal(CLIENT_COMMAND_DISPOSITIONS.request_exit, 'not_applicable');
  assert.equal(CLIENT_COMMAND_DISPOSITIONS.resume_workflow, 'not_applicable');
  assert.equal(CLIENT_EVENT_DISPOSITIONS.turn_recovery_state, 'degraded');
  assert.equal(CLIENT_EVENT_DISPOSITIONS.session_agent_tombstone, 'exposed');
  assert.equal(CLIENT_EVENT_DISPOSITIONS.app_event, 'not_applicable');
});

test('engine audio response and capability commands stay private to main', () => {
  assert.equal(ALLOWED_CLIENT_COMMAND_TYPES.includes('audio_response' as never), false);
  assert.equal(ALLOWED_CLIENT_COMMAND_TYPES.includes('update_audio_capabilities' as never), false);
  assert.equal(CLIENT_COMMAND_DISPOSITIONS.audio_response, 'host_private');
  assert.equal(CLIENT_COMMAND_DISPOSITIONS.update_audio_capabilities, 'host_private');
});
