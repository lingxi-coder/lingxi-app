import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { AppEventDto } from '../src/protocol';

test('v2 stream frames are part of the shared AppEventDto contract', () => {
  const event = {
    type: 'app_bridge_stream_frame',
    frame: {
      type: 'data',
      appId: 'abc12345',
      requestId: 'request-1',
      streamId: 'stream-1',
      seq: 0,
      dataJson: '{}',
    },
    frameJson: '{"type":"data"}',
  } satisfies AppEventDto;

  assert.equal(event.type, 'app_bridge_stream_frame');
  assert.equal(event.frame.streamId, 'stream-1');
});

test('dependency confirmation event carries review and rollback evidence', () => {
  const event = {
    type: 'app_dependency_change_confirmation_requested',
    request: {
      requestId: 'dependency-request-1',
      appId: 'abc12345',
      reason: 'pre_resolution_no_network',
      changes: [{
        kind: 'add',
        package: 'dayjs',
        version: '1.11.13',
        cacheStatus: 'unknown_until_resolution',
        downloadStatus: 'may_be_required',
      }],
      licenseRisk: 'unknown_until_resolution',
      sbomRisk: 'unknown_until_resolution',
      lifecycleScriptsBlocked: true,
      nativeAddonsBlocked: true,
      rollbackPolicy: 'rollback_on_validation_failure',
    },
  } satisfies AppEventDto;

  assert.equal(event.request.changes[0]?.package, 'dayjs');
  assert.equal(event.request.lifecycleScriptsBlocked, true);
  assert.equal(event.request.rollbackPolicy, 'rollback_on_validation_failure');
});
