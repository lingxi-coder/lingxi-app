import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { validateClientEvent, validateUiClientOperationResponseJson } from '../src/validation.js';

const captureDirectory = process.env.LINGXI_TEST_CAPTURE_MOD_BRIDGE;

test('actual Worker bridge outcomes survive the strict client validator', { skip: !captureDirectory }, () => {
  for (const [file, requestId, expected] of [
    ['unknown.json', 'failed-live', 'unknown'],
    ['not-started.json', 'refused-live', 'not_started'],
  ] as const) {
    const rawFrame = JSON.parse(readFileSync(join(captureDirectory!, file), 'utf8'));
    assert.equal(rawFrame.type, 'event');
    const event = validateClientEvent(rawFrame.payload);
    assert.equal(event.type, 'ui_control_result');
    if (event.type !== 'ui_control_result' || event.response_json === undefined) throw new Error('missing operation response');
    assert.equal(event.request_id, requestId);
    const rawResponse = JSON.parse(event.response_json);
    const operation = { type: 'runHeld', runtimeId: rawResponse.runtimeId,
      render_revision: rawResponse.renderRevision, handle: 1 } as const;
    const response = validateUiClientOperationResponseJson(operation, event.response_json);
    assert.ok('outcome' in response);
    if (!('outcome' in response)) throw new Error('missing unavailable outcome');
    assert.deepEqual(response.outcome, { status: expected, runtime: 'unavailable' });
    assert.throws(() => validateUiClientOperationResponseJson(
      { ...operation, runtimeId: `${operation.runtimeId}-other` }, event.response_json!,
    ));
    assert.throws(() => validateUiClientOperationResponseJson(operation,
      JSON.stringify({ ...rawResponse, outcome: { ...rawResponse.outcome, unexpected: true } }),
    ));
  }
});
