import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateClientEvent } from '../src/validation.js';

test('ui_log preserves separate plain-text plugin and line fields', () => {
  const event = { type: 'ui_log', plugin: 'review', text: '<not markup>' };
  assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ ...event, message: 'extra' }), /client event/);
  assert.throws(() => validateClientEvent({ ...event, plugin: 42 }), /plugin/);
});

test('ui_toast keeps its lifetime and rejects extra transcript fields', () => {
  const event = { type: 'ui_toast', plugin: 'review', text: 'Done', timeout_ms: 4000 };
  assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ ...event, message: 'extra' }), /client event/);
  assert.throws(() => validateClientEvent({ ...event, timeout_ms: '4000' }), /timeout_ms/);
});

test('ui_status accepts a text line or null clear', () => {
  const event = { type: 'ui_status', plugin: 'review', text: 'Working' };
  assert.deepEqual(validateClientEvent(event), event);
  assert.deepEqual(validateClientEvent({ ...event, text: null }), { ...event, text: null });
  assert.throws(() => validateClientEvent({ ...event, text: 42 }), /text/);
});
