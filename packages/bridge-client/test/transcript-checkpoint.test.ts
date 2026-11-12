import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import type { ClientEvent, Frame } from '../src/protocol.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const checkpoint: Extract<ClientEvent, { type: 'session_resumed' }> = {
  type: 'session_resumed', session_id: sessionId, mode: 'code',
  messages: [{ role: 'assistant', transcript_uuid: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    blocks: [{ type: 'text', text: 'preserved \ud800 and \u{1f680}' }] }],
};

function wire(reply: (request: Extract<Frame, { type: 'request' }>, deliver: (frame: Frame) => void) => void) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: { readyState: number; send(value: string): void };
    onMessage(bytes: Buffer): void;
    onClose(code: number, reason: string): void;
    pendingResponses: Map<number, unknown>;
  };
  const deliver = (frame: Frame) => internal.onMessage(Buffer.from(JSON.stringify(frame)));
  internal.ws = { readyState: 1, send: value => reply(JSON.parse(value), deliver) };
  return { client, internal };
}

test('transcript checkpoint is a correlated read, preserves exact text and does not emit a resume event', async () => {
  const live: ClientEvent[] = [];
  const { client, internal } = wire((frame, deliver) => {
    assert.equal(frame.payload.method, 'desktop_transcript_checkpoint');
    assert.deepEqual(frame.payload.params, { session_id: sessionId });
    deliver({ type: 'event', payload: { type: 'text_delta', text: 'racing live output' } });
    deliver({ type: 'response', payload: { id: frame.payload.id + 100, result: checkpoint } });
    deliver({ type: 'response', payload: { id: frame.payload.id, result: checkpoint } });
  });
  client.on('event', event => live.push(event));
  assert.deepEqual(await client.captureTranscriptCheckpoint(sessionId), checkpoint);
  assert.deepEqual(live, [{ type: 'text_delta', text: 'racing live output' }]);
  assert.equal(internal.pendingResponses.size, 0);
});

test('transcript checkpoint rejects a different owner and malformed baselines', async () => {
  for (const result of [null, { type: 'text_delta', text: 'not a baseline' },
    { ...checkpoint, session_id: '22222222-3333-4444-8555-666666666666' },
    { ...checkpoint, mode: 'unknown' }, { ...checkpoint, messages: [{}] }]) {
    const { client, internal } = wire((frame, deliver) => {
      deliver({ type: 'response', payload: { id: frame.payload.id, result } });
    });
    await assert.rejects(client.captureTranscriptCheckpoint(sessionId));
    assert.equal(internal.pendingResponses.size, 0);
  }
});

test('transcript checkpoint cannot cross a socket replacement or a disconnect', async () => {
  let deliver!: (frame: Frame) => void;
  let request!: Extract<Frame, { type: 'request' }>;
  const { client, internal } = wire((frame, send) => { request = frame; deliver = send; });
  const pending = client.captureTranscriptCheckpoint(sessionId);
  deliver({ type: 'response', payload: { id: request.payload.id, result: checkpoint } });
  internal.ws = { readyState: 1, send() {} };
  await assert.rejects(pending, /connection changed/);
  const lost = client.captureTranscriptCheckpoint(sessionId);
  const rejected = assert.rejects(lost, /connection closed/);
  internal.onClose(1006, 'synthetic disconnect');
  await rejected;
  assert.equal(internal.pendingResponses.size, 0);
});
