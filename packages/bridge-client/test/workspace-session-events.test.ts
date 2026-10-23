import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateClientEvent } from '../src/validation.js';

test('directory events retain exact session and transcript coordinates', () => {
  const event = { type: 'workspace_changed', state: {
    session_id: 'same-session', cwd: '/home/lingxi/b', project_root: '/home/lingxi/a',
    additional_directories: ['/home/lingxi/shared'], transcript_path: '/home/lingxi/.lingxi/projects/b/same-session.jsonl',
  } };
  assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ ...event, state: { ...event.state, additional_directories: [42] } }));
  assert.throws(() => validateClientEvent({ ...event, state: { ...event.state, transcript_path: null } }));
});

test('session catalogs accept and preserve nullable loader workspace hints', () => {
  const row = { uuid: 'session', title: 'Saved', modified_rfc3339: '2026-10-23T00:00:00Z',
    message_count: 2, mode: 'code', path: '/catalog/session.jsonl' };
  for (const workspace_path of [null, '/home/lingxi/b']) {
    const event = { type: 'session_list', sessions: [{ ...row, workspace_path }] };
    assert.deepEqual(validateClientEvent(event), event);
  }
  assert.throws(() => validateClientEvent({ type: 'session_list', sessions: [{ ...row, workspace_path: 42 }] }));
});
