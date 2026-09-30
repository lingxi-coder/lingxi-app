/**
 * Wire-compatibility test for the `computer` tool `request_access` DTOs
 * (client-protocol's `ComputerAccessRequestDto` / `ComputerAccessResponseDto`,
 * `bridge::wire::Frame::ComputerAccessRequest`, and the
 * `approve_computer_access` / `deny_computer_access` `ClientCommand`
 * variants).
 *
 * There is no shared `client-protocol/snapshots/computer_access/*.json`
 * fixture directory yet (see `test/snapshots.test.ts` for the pattern used
 * once the Rust track lands one) — this instead pins the EXACT example JSON
 * from the frozen wire contract inline, so a tag/field-name drift in the TS
 * mirror in `../src/protocol.ts` fails loudly here rather than silently
 * `as`-casting.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import type {
  ClientCommand,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  Frame,
} from '../src/protocol.js';

// ── Frozen wire fixtures (byte-exact) ────────────────────────────────────────

const REQUEST_WITH_TCC =
  '{"request_id":42,"reason":"automate chat","apps":[{"label":"Slack"},{"label":"Chrome"}],"tier":"full","clipboard_read":false,"clipboard_write":false,"system_key_combos":false,"tcc_state":{"accessibility":true,"screen_recording":false}}';

const REQUEST_WITHOUT_TCC =
  '{"request_id":7,"reason":"read the clipboard","apps":[{"label":"Notes"}],"tier":"read","clipboard_read":true,"clipboard_write":false,"system_key_combos":false}';

const FRAME_ENVELOPE =
  '{"type":"computer_access_request","payload":{"request_id":42,"reason":"automate chat","apps":[{"label":"Slack"}],"tier":"full","clipboard_read":false,"clipboard_write":false,"system_key_combos":false}}';

const APPROVE_COMMAND =
  '{"type":"approve_computer_access","request_id":42,"response":{"granted_apps":["Slack"],"clipboard_read":false,"clipboard_write":false,"system_key_combos":false}}';

const DENY_COMMAND = '{"type":"deny_computer_access","request_id":42}';

// ── Structural validators (mirror the TS shapes exactly) ─────────────────────

const isString = (v: unknown): v is string => typeof v === 'string';
const isNumber = (v: unknown): v is number => typeof v === 'number';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

function rec(v: unknown): Record<string, unknown> {
  assert.equal(typeof v, 'object');
  assert.notEqual(v, null);
  return v as Record<string, unknown>;
}

function validateComputerAccessRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(isNumber(o['request_id']));
  assert.ok(isString(o['reason']));
  assert.ok(Array.isArray(o['apps']));
  for (const app of o['apps'] as unknown[]) {
    const a = rec(app);
    assert.ok(isString(a['label']));
  }
  assert.ok(['read', 'click', 'full'].includes(o['tier'] as string));
  assert.ok(
    isBool(o['clipboard_read']) && isBool(o['clipboard_write']) && isBool(o['system_key_combos']),
  );
  if ('tcc_state' in o) {
    const tcc = rec(o['tcc_state']);
    assert.ok(isBool(tcc['accessibility']) && isBool(tcc['screen_recording']));
  }
}

function validateComputerAccessResponse(v: unknown): void {
  const o = rec(v);
  assert.ok(Array.isArray(o['granted_apps']));
  for (const label of o['granted_apps'] as unknown[]) assert.ok(isString(label));
  assert.ok(
    isBool(o['clipboard_read']) && isBool(o['clipboard_write']) && isBool(o['system_key_combos']),
  );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

test('ComputerAccessRequestDto (tcc_state present) round-trips byte-exact', () => {
  const parsed = JSON.parse(REQUEST_WITH_TCC) as ComputerAccessRequestDto;
  validateComputerAccessRequest(parsed);
  assert.equal(parsed.request_id, 42);
  assert.equal(parsed.tier, 'full');
  assert.deepEqual(parsed.apps, [{ label: 'Slack' }, { label: 'Chrome' }]);
  assert.ok(parsed.tcc_state);
  assert.equal(parsed.tcc_state.accessibility, true);
  assert.equal(parsed.tcc_state.screen_recording, false);
  // Byte round-trip: JSON.parse/stringify preserve string-key insertion order,
  // so re-serializing must reproduce the exact wire bytes.
  assert.equal(JSON.stringify(parsed), REQUEST_WITH_TCC);
});

test('ComputerAccessRequestDto (tcc_state absent) omits the key entirely, not null', () => {
  const parsed = JSON.parse(REQUEST_WITHOUT_TCC) as ComputerAccessRequestDto;
  validateComputerAccessRequest(parsed);
  assert.equal(parsed.request_id, 7);
  assert.equal(parsed.tier, 'read');
  assert.equal('tcc_state' in parsed, false, 'skip_serializing_if omits the key rather than emitting null');
  assert.equal(JSON.stringify(parsed), REQUEST_WITHOUT_TCC);
});

test('Frame::ComputerAccessRequest is adjacently tagged (type + payload)', () => {
  const frame = JSON.parse(FRAME_ENVELOPE) as Frame;
  assert.equal(frame.type, 'computer_access_request');
  if (frame.type !== 'computer_access_request') throw new Error('unreachable');
  validateComputerAccessRequest(frame.payload);
  assert.equal(JSON.stringify(frame), FRAME_ENVELOPE);
});

test('approve_computer_access / deny_computer_access ClientCommand variants match the frozen contract', () => {
  const approve = JSON.parse(APPROVE_COMMAND) as ClientCommand;
  assert.equal(approve.type, 'approve_computer_access');
  if (approve.type !== 'approve_computer_access') throw new Error('unreachable');
  assert.equal(approve.request_id, 42);
  validateComputerAccessResponse(approve.response);
  assert.deepEqual(approve.response, {
    granted_apps: ['Slack'],
    clipboard_read: false,
    clipboard_write: false,
    system_key_combos: false,
  } satisfies ComputerAccessResponseDto);
  assert.equal(JSON.stringify(approve), APPROVE_COMMAND);

  const deny = JSON.parse(DENY_COMMAND) as ClientCommand;
  assert.equal(deny.type, 'deny_computer_access');
  if (deny.type !== 'deny_computer_access') throw new Error('unreachable');
  assert.equal(deny.request_id, 42);
  assert.equal(JSON.stringify(deny), DENY_COMMAND);
});

test('a fully-denied ComputerAccessResponseDto has no granted apps and every flag false', () => {
  const denied: ComputerAccessResponseDto = {
    granted_apps: [],
    clipboard_read: false,
    clipboard_write: false,
    system_key_combos: false,
  };
  validateComputerAccessResponse(denied);
  assert.equal(denied.granted_apps.length, 0);
});
