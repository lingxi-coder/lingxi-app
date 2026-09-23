/**
 * The transport's last line of defence: a frame the engine cannot read must
 * fail the COMMAND, never the connection.
 *
 * `BridgeClient` sends a command as one unfragmented WebSocket text frame, and
 * the engine reads with a bounded `max_frame_size`. Handing the socket an
 * over-length frame does not produce a rejected command — tungstenite yields
 * `Err(Capacity(MessageTooLong))`, `run_frame_pump` breaks its loop, and
 * `BridgeConnection::close_connection` aborts the running turn and drains every
 * broker. Every caller loses its session because one caller built one payload
 * that was too big.
 *
 * Callers are still expected to stay inside their own bounds (the audio
 * response derives its base64 limit from {@link MAX_BRIDGE_FRAME_BYTES} for
 * exactly that reason) — this is what catches the one that does not.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { BridgeClient, frameSizeError } from '../src/client.js';
import { MAX_BRIDGE_FRAME_BYTES } from '../src/protocol.js';

test('a frame inside the engine\'s read limit is not refused', () => {
  assert.equal(frameSizeError(JSON.stringify({ type: 'cancel' }), 'cancel'), null);
});

test('a frame over the engine\'s read limit is refused with its size and the limit named', () => {
  const error = frameSizeError('x'.repeat(MAX_BRIDGE_FRAME_BYTES + 1), 'audio_response');
  assert.ok(error, 'an over-length frame must not be handed to the socket');
  assert.match(error.message, /audio_response/);
  assert.match(error.message, new RegExp(String(MAX_BRIDGE_FRAME_BYTES + 1)));
  assert.match(error.message, new RegExp(String(MAX_BRIDGE_FRAME_BYTES)));
});

test('sendCommand refuses an oversize command instead of putting it on the socket', () => {
  const sent: string[] = [];
  const client = new BridgeClient();
  // The socket is the only thing this needs; `sendFrame` checks `readyState`
  // against `WebSocket.OPEN` (1) and then sends the serialized frame.
  (client as unknown as { ws: unknown }).ws = {
    readyState: 1,
    send: (data: string) => { sent.push(data); },
  };

  assert.throws(
    () => client.sendCommand({
      type: 'audio_response',
      identity: { id: '00000000-0000-4000-8000-000000000001', generation: 1, service_epoch: 1 },
      result: { type: 'recording', audio_base64: 'A'.repeat(MAX_BRIDGE_FRAME_BYTES), mime_type: 'audio/wav' },
    } as never),
    /too large/,
    'an over-length command must fail here, not tear the connection down at the engine',
  );
  assert.deepEqual(sent, [], 'nothing may reach the socket once the frame is known to be unreadable');

  client.sendCommand({ type: 'cancel' } as never);
  assert.equal(sent.length, 1, 'an ordinary command still goes out');
});
