/**
 * The numbers this client and the engine have to agree on, checked against the
 * engine's own source rather than against a copy of it.
 *
 * Both families here failed the same way before: a constant on this side was
 * chosen against an assumption about the other side, the assumption was wrong,
 * and nothing anywhere compared the two. A comment saying "must stay under the
 * transport limit" is not a check; reading the Rust constant is.
 *
 * `clients/shared/test/snapshots.test.ts` already loads `lingxi-code/`'s golden
 * snapshots for the same reason, so reaching across the language boundary in a
 * test is the established way to pin a cross-language contract here.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { MAX_BRIDGE_FRAME_BYTES } from '@lingxi/bridge-client/protocol';

import {
  MAX_AUDIO_BASE64_LENGTH,
  MAX_AUDIO_MIME_TYPE_LENGTH,
} from '../src/shared/audioResponse';

const ENGINE_ROOT = join(import.meta.dirname, '..', '..', '..', 'lingxi-code');

function engineSource(...parts: string[]): string {
  return readFileSync(join(ENGINE_ROOT, ...parts), 'utf8');
}

/**
 * The value of a `const NAME: <type> = <expr>;` in a Rust file, evaluated for
 * the small arithmetic these constants use. Anything else throws rather than
 * quietly returning a number that is not the engine's.
 */
function rustConst(source: string, name: string): number {
  const match = new RegExp(`const ${name}: \\w+ = ([0-9_ */+]+);`).exec(source);
  assert.ok(match, `the engine no longer declares ${name}; this pin is measuring nothing`);
  const expression = match[1].replace(/_/g, '');
  assert.match(expression, /^[0-9 */+]+$/, `unexpected expression for ${name}: ${match[1]}`);
  // eslint-disable-next-line no-new-func
  const value = Number(new Function(`return (${expression});`)());
  assert.ok(Number.isSafeInteger(value), `${name} did not evaluate to an integer: ${match[1]}`);
  return value;
}

// ---------------------------------------------------------------------------
// Frame size.
//
// `client.ts` sends a command as ONE unfragmented WebSocket text frame. The
// engine reads it with a tungstenite `max_frame_size`; a frame past that is
// not a rejected command, it is `Err(Capacity(MessageTooLong))` in
// `run_frame_pump`, which breaks the loop and runs `close_connection` — the
// turn is aborted and every broker drained. The user loses the session, not
// the recording. So the largest response this client will ever BUILD has to
// fit inside the largest frame the engine will ever READ.
// ---------------------------------------------------------------------------

test('the engine\'s inbound frame limit is declared, not inherited from a dependency default', () => {
  const source = engineSource('bridge', 'src', 'mcp_endpoint.rs');
  assert.ok(
    source.includes('accept_hdr_async_with_config('),
    'the WebSocket is accepted with no config, so its frame limit is whatever tungstenite defaults to — '
    + 'a number no code here states and a dependency bump can move silently',
  );
  assert.equal(
    rustConst(source, 'MAX_INBOUND_FRAME_BYTES'),
    MAX_BRIDGE_FRAME_BYTES,
    'the engine reads a different maximum frame than this client believes it may send',
  );
});

test('the largest audio response this client can build fits in one engine frame', () => {
  // Exactly the shape `client.ts` puts on the wire for the largest legal
  // `stop_recording` answer: `sendCommand` wraps the command in a
  // `Frame::Request` and sends `JSON.stringify(frame)` as one text frame.
  const command = {
    type: 'audio_response',
    request_id: Number.MAX_SAFE_INTEGER,
    result: {
      type: 'recording',
      audio_base64: 'A'.repeat(MAX_AUDIO_BASE64_LENGTH),
      mime_type: 'x'.repeat(MAX_AUDIO_MIME_TYPE_LENGTH),
    },
  };
  const frame = {
    type: 'request',
    payload: { id: Number.MAX_SAFE_INTEGER, method: command.type, params: command },
  };
  const bytes = Buffer.byteLength(JSON.stringify(frame), 'utf8');

  assert.ok(
    bytes <= MAX_BRIDGE_FRAME_BYTES,
    `a maximal audio_response is ${bytes} bytes, over the engine's ${MAX_BRIDGE_FRAME_BYTES}-byte frame limit: `
    + 'the engine would not reject the response, it would tear down the connection',
  );
});
