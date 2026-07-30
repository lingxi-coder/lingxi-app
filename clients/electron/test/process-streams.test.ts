import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { ignoreBrokenPipe } from '../src/main/process-streams';

test('broken process pipes are ignored without swallowing other stream failures', () => {
  const stream = new EventEmitter();
  ignoreBrokenPipe(stream);

  assert.doesNotThrow(() => stream.emit('error', Object.assign(new Error('write EPIPE'), { code: 'EPIPE' })));
  assert.throws(
    () => stream.emit('error', Object.assign(new Error('write failed'), { code: 'EIO' })),
    /write failed/,
  );
});
