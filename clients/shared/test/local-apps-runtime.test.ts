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
