import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  CLIENT_COMMAND_DISPOSITIONS,
  CLIENT_COMMAND_REMOTE_DISPOSITIONS,
  CLIENT_EVENT_DISPOSITIONS,
  CLIENT_EVENT_REMOTE_DISPOSITIONS,
  REMOTE_SLASH_COMMANDS,
  REMOTE_STATE_EVENT_TYPES,
} from '../src/shared/clientCommands.js';
import { slashRequestEchoes } from '../src/main/bridgeEvents.js';
import { slashCommandEchoesRequest } from '../src/renderer/bridge/conversation.js';
import { eventDisposition } from '../src/main/remoteControl/snapshot.js';

test('the remote event table names exactly the events the desktop table names', () => {
  // Both are `satisfies Record<ClientEvent['type'], …>`: a new event type fails to compile until both list it.
  assert.deepEqual(Object.keys(CLIENT_EVENT_REMOTE_DISPOSITIONS).sort(), Object.keys(CLIENT_EVENT_DISPOSITIONS).sort());
});

test('the remote command table names exactly the commands the desktop table names', () => {
  assert.deepEqual(Object.keys(CLIENT_COMMAND_REMOTE_DISPOSITIONS).sort(), Object.keys(CLIENT_COMMAND_DISPOSITIONS).sort());
});

test('events that carry secrets, credentials, configuration or other chats are withheld from viewers', () => {
  const mustWithhold = [
    'provider_model_catalog', 'provider_credential_status', 'provider_connection_tested', 'openai_oauth_updated', 'auth_state', 'settings_snapshot',
    'mcp_servers', 'mcp_configuration_snapshot', 'hooks', 'skills', 'agents', 'memory_entries', 'plugin_catalog',
    'doctor_report', 'task_output_chunk', 'configuration_operation',
    'side_chat_created', 'side_chat_list', 'side_chat_snapshot', 'side_chat_event', 'side_chat_permission', 'side_chat_submitted', 'side_chat_ended',
    'cron_result', 'ui_control_result', 'terminal_api_error',
    'realtime_audio_event', 'audio_session_context',
  ];
  for (const type of mustWithhold) {
    assert.ok(type in CLIENT_EVENT_REMOTE_DISPOSITIONS, `${type} is an event type`);
    assert.equal((CLIENT_EVENT_REMOTE_DISPOSITIONS as Record<string, string>)[type], 'withheld', type);
  }
  for (const [type, disposition] of Object.entries(CLIENT_EVENT_REMOTE_DISPOSITIONS)) {
    if (/^(provider_|side_chat_|audio_)/.test(type)) assert.equal(disposition, 'withheld', type);
  }
  // The tables hold something back and let something through.
  const values = new Set(Object.values(CLIENT_EVENT_REMOTE_DISPOSITIONS));
  for (const kind of ['forward', 'coalesce', 'state', 'drop', 'withheld']) assert.ok(values.has(kind as never), kind);
});

test('an event the table does not know is withheld', () => {
  assert.equal(eventDisposition('some_event_from_the_future'), 'withheld');
  assert.equal(eventDisposition('text_delta'), 'coalesce');
  assert.equal(eventDisposition('tool_use_started'), 'forward');
});

test('the cached state events are the ones the table calls state, minus the base events', () => {
  const state = Object.entries(CLIENT_EVENT_REMOTE_DISPOSITIONS).filter(([, d]) => d === 'state').map(([type]) => type);
  assert.deepEqual([...REMOTE_STATE_EVENT_TYPES].sort(), state.filter((type) => type !== 'session_started' && type !== 'session_resumed').sort());
});

test('commands a viewer can send are a short list; shell, file and credential commands are host-only', () => {
  const remote = Object.entries(CLIENT_COMMAND_REMOTE_DISPOSITIONS).filter(([, d]) => d === 'remote').map(([type]) => type).sort();
  assert.deepEqual(remote, [
    'answer_ask_user_question', 'approve_permission', 'cancel', 'cancel_ask_user_question', 'deny_permission',
    'get_conversation_controls', 'list_models', 'run_slash_command', 'send_prompt', 'set_model',
    'set_permission_mode', 'set_reasoning_selection',
  ]);
  for (const type of ['update_settings', 'upsert_mcp_server', 'login', 'logout', 'ui_attach', 'clear_session', 'update_permission_rules']) {
    if (type in CLIENT_COMMAND_REMOTE_DISPOSITIONS) assert.equal((CLIENT_COMMAND_REMOTE_DISPOSITIONS as Record<string, string>)[type], 'host_only', type);
  }
});

test('only a short list of slash commands may be run from a viewer', () => {
  assert.deepEqual([...REMOTE_SLASH_COMMANDS], ['/compact', '/model', '/effort', '/plan']);
});

test('the host decides which slash commands echo as a prompt exactly as the window does', () => {
  const samples = [
    '/compact', '/compact keep the tests', '/goal ship it', '/fusion a b', '/plan', '/plan open', '/plan share', '/plan add a login page',
    '/model', '/model x', '/effort high', '/help', '/clear', '', '   ', '/PLAN something', '/Compact',
  ];
  for (const raw of samples) assert.equal(slashRequestEchoes(raw), slashCommandEchoesRequest(raw), JSON.stringify(raw));
});
