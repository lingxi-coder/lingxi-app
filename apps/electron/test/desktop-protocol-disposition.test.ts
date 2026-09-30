import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  ALL_APP_EVENT_TYPES,
  ALL_CLIENT_COMMAND_TYPES,
  ALL_CLIENT_EVENT_TYPES,
  ALL_PLUGIN_COMMAND_TYPES,
} from '../../../packages/bridge-client/src/protocolCoverage';
import {
  APP_EVENT_DISPOSITIONS,
  CLIENT_COMMAND_DISPOSITIONS,
  CLIENT_EVENT_DISPOSITIONS,
  PLUGIN_COMMAND_DISPOSITIONS,
  REFRESH_LISTING_DISPOSITIONS,
} from '../src/shared/clientCommands';

test('every client command variant has a desktop disposition', () => {
  assert.deepEqual(
    Object.keys(CLIENT_COMMAND_DISPOSITIONS).sort(),
    Object.keys(ALL_CLIENT_COMMAND_TYPES).sort(),
  );
});

test('every client event variant has a desktop disposition', () => {
  assert.deepEqual(
    Object.keys(CLIENT_EVENT_DISPOSITIONS).sort(),
    Object.keys(ALL_CLIENT_EVENT_TYPES).sort(),
  );
});

test('every nested plugin command variant has a desktop disposition', () => {
  assert.deepEqual(
    Object.keys(PLUGIN_COMMAND_DISPOSITIONS).sort(),
    Object.keys(ALL_PLUGIN_COMMAND_TYPES).sort(),
  );
});

test('every nested app event variant has a desktop disposition', () => {
  assert.deepEqual(
    Object.keys(APP_EVENT_DISPOSITIONS).sort(),
    Object.keys(ALL_APP_EVENT_TYPES).sort(),
  );
});

test('every refresh_listings kind has a desktop disposition', () => {
  assert.deepEqual(
    Object.keys(REFRESH_LISTING_DISPOSITIONS).sort(),
    ['agents', 'auth', 'doctor', 'hooks', 'mcp', 'memory', 'models', 'sessions', 'settings', 'skills', 'slash_commands', 'status', 'tasks'].sort(),
  );
});
