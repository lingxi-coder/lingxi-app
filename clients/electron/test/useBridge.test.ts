import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  clearCancellationRuntime,
  resetBridgeRuntimeState,
  shouldClearPendingPermissions,
  shouldResetBridgeRuntime,
} from '../src/renderer/bridge/useBridge';
import { emptyConversation } from '../src/renderer/bridge/conversation';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';

test('bridge spawning requests a full renderer reset', () => {
  assert.equal(shouldResetBridgeRuntime({ status: 'spawning' }), true);
  assert.equal(shouldResetBridgeRuntime({ status: 'connected' }), false);

  const reset = resetBridgeRuntimeState();
  assert.deepEqual(reset.conversation, emptyConversation());
  assert.deepEqual(reset.desktop, emptyDesktopState());
  assert.deepEqual(reset.permissionQueue, []);
  assert.deepEqual(reset.computerAccessQueue, []);
  assert.deepEqual(reset.askUserQuestionQueue, []);
});

test('pending permission ui is cleared across restart and disconnect states', () => {
  assert.equal(shouldClearPendingPermissions({ status: 'spawning' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'idle' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'disconnected', reason: 'socket closed' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'error', message: 'boom' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'connecting' }), false);
  assert.equal(shouldClearPendingPermissions({ status: 'connected' }), false);
});

test('failed prompt submission releases a cancellation task for the next turn', () => {
  const pending = Promise.resolve();
  const cancelling = { current: true };
  const task = { current: pending as Promise<void> | null };

  clearCancellationRuntime(cancelling, task);

  assert.equal(cancelling.current, false);
  assert.equal(task.current, null);
});
