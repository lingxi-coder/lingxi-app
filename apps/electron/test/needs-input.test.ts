import assert from 'node:assert/strict';
import { test } from 'node:test';
import { sessionNeedsInput } from '../src/renderer/components/NeedsInputBadge';
test('session attention covers approvals and questions and clears after response', () => {
  assert.equal(sessionNeedsInput(), false);
  assert.equal(sessionNeedsInput({pendingInteractions: 1, pendingAskUserQuestions: 0}), true);
  assert.equal(sessionNeedsInput({pendingInteractions: 0, pendingAskUserQuestions: 1}), true);
  assert.equal(sessionNeedsInput({pendingInteractions: 0, pendingAskUserQuestions: 0}), false);
});
