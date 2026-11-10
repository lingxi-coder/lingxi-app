import assert from 'node:assert/strict';
import { test } from 'node:test';
import { assertDecisionImageSelectionBudget, assertDecisionInputBudget, DECISION_INPUT_MAX_BYTES } from '../src/shared/decisionInputBudget.js';
import type { DecisionInput } from '../src/shared/decision.js';

const mib = 1024 * 1024;
const input: DecisionInput = { state: 'Evidence', questions: { answer: { type: 'binary', instructions: 'Is it valid?' } } };

test('image preflight rejects aggregate base64 size while preserving valid multi-image selections', () => {
  const large = { type: 'image/png', size: 8 * mib };
  assert.throws(() => assertDecisionImageSelectionBudget([large, large], input), /16 MiB/);
  assert.throws(() => assertDecisionImageSelectionBudget(Array(8).fill(large), input), /16 MiB/);
  assert.doesNotThrow(() => assertDecisionImageSelectionBudget([large, { type: 'image/jpeg', size: 3 * mib }], input));
});

test('image preflight includes evidence and question bytes before image allocation', () => {
  const file = { type: 'image/jpeg', size: 3 * mib };
  assert.throws(() => assertDecisionImageSelectionBudget([file], { ...input, state: 'x'.repeat(12 * mib) }), /16 MiB/);
  assert.throws(() => assertDecisionImageSelectionBudget([file], {
    ...input, questions: { answer: { type: 'binary', instructions: 'x'.repeat(12 * mib) } },
  }), /16 MiB/);
  assert.doesNotThrow(() => assertDecisionImageSelectionBudget([file], { ...input, state: 'x'.repeat(mib) }));
});

test('image preflight retains per-image format, count and size limits', () => {
  for (const files of [
    [{ type: 'image/gif', size: 1 }],
    [{ type: 'image/png', size: 8 * mib + 1 }],
    Array(9).fill({ type: 'image/png', size: 1 }),
  ]) assert.throws(() => assertDecisionImageSelectionBudget(files, input), /eight PNG\/JPEG/);
  assert.doesNotThrow(() => assertDecisionImageSelectionBudget([], input));
});

test('final input budget enforces the existing 16 MiB input boundary', () => {
  assert.equal(DECISION_INPUT_MAX_BYTES, 16 * mib);
  const metadataBytes = Buffer.byteLength(JSON.stringify({ ...input, state: '' }));
  const boundary = { ...input, state: 'x'.repeat(DECISION_INPUT_MAX_BYTES - metadataBytes) };
  assert.doesNotThrow(() => assertDecisionInputBudget(boundary));
  assert.throws(() => assertDecisionInputBudget({ ...boundary, state: `${boundary.state}x` }), /16 MiB/);
  assert.throws(() => assertDecisionInputBudget({ ...input, images: [{ data_url: 'x'.repeat(16 * mib) }] }), /16 MiB/);
});

test('final input budget measures UTF-8 and JSON escaping rather than source character count', () => {
  assert.throws(() => assertDecisionInputBudget({ ...input, state: '界'.repeat(Math.floor(DECISION_INPUT_MAX_BYTES / 3)) }), /16 MiB/);
  assert.throws(() => assertDecisionInputBudget({ ...input, state: '\0'.repeat(Math.ceil(DECISION_INPUT_MAX_BYTES / 6)) }), /16 MiB/);
  assert.doesNotThrow(() => assertDecisionInputBudget({ ...input, state: '界'.repeat(1024) }));
});
