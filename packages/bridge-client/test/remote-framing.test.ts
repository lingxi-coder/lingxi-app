/**
 * Message fragmentation and reassembly, against the shared fixtures and at the
 * size boundaries.
 */

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { RemoteProtocolError } from '../src/remote/errors.js';
import { encodeMessage, FRAGMENT_KIND, Reassembler } from '../src/remote/framing.js';
import { FRAGMENT_MAX_BYTES, MESSAGE_MAX_BYTES, TUNNEL_FRAGMENT_MAX_BYTES } from '../src/remote/version.js';
import { fixture, hex, unhex } from './remote-helpers.js';

type MessageSpec = { text: string } | { prefix?: string; unit: string; count: number; suffix?: string };
interface Case {
  name: string;
  message: MessageSpec;
  max_fragment_bytes: number;
  message_utf8_length: number;
  message_sha256_hex: string;
  fragments: Array<{ kind: number; length: number; sha256_hex: string; hex?: string }>;
}
interface Sequence {
  name: string;
  fragments: Array<{ hex: string } | { recipe: { kind: number; payload_length: number; fill_hex: string } }>;
  message_utf8_hex?: string;
}

const framing = fixture<{ cases: Case[]; invalid_sequences: Sequence[]; valid_sequences: Sequence[] }>('framing');

const sha = (bytes: Uint8Array): string => createHash('sha256').update(bytes).digest('hex');

function textOf(spec: MessageSpec): string {
  return 'text' in spec ? spec.text : (spec.prefix ?? '') + spec.unit.repeat(spec.count) + (spec.suffix ?? '');
}

function fragmentOf(entry: Sequence['fragments'][number]): Uint8Array {
  if ('hex' in entry) return unhex(entry.hex);
  const { kind, payload_length: length, fill_hex: fill } = entry.recipe;
  const bytes = new Uint8Array(1 + length).fill(unhex(fill)[0]!);
  bytes[0] = kind;
  return bytes;
}

function reassemble(fragments: Uint8Array[]): string[] {
  const reassembler = new Reassembler();
  const messages: string[] = [];
  for (const fragment of fragments) {
    const message = reassembler.push(fragment);
    if (message !== null) messages.push(message);
  }
  return messages;
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

test('encodeMessage reproduces every fixture layout (kind, length, content hash)', () => {
  assert.ok(framing.cases.length >= 12);
  for (const c of framing.cases) {
    const text = textOf(c.message);
    const bytes = new TextEncoder().encode(text);
    assert.equal(bytes.length, c.message_utf8_length, c.name);
    assert.equal(sha(bytes), c.message_sha256_hex, c.name);

    const fragments = encodeMessage(text, c.max_fragment_bytes);
    assert.deepEqual(
      fragments.map((f) => ({ kind: f[0], length: f.length, sha256_hex: sha(f), ...(f.length <= 64 ? { hex: hex(f) } : {}) })),
      c.fragments,
      c.name,
    );
    for (const f of fragments) assert.ok(f.length <= c.max_fragment_bytes, `${c.name}: fragment of ${f.length}`);
  }
});

test('reassembling every fixture\'s fragments returns the original message', () => {
  for (const c of framing.cases) {
    const text = textOf(c.message);
    assert.deepEqual(reassemble(encodeMessage(text, c.max_fragment_bytes)), [text], c.name);
  }
});

test('the boundary cases split exactly where the plan says', () => {
  const layout = (name: string) => framing.cases.find((c) => c.name.startsWith(name))!.fragments.map((f) => [f.kind, f.length]);
  assert.deepEqual(layout('empty'), [[0, 1]]);
  assert.deepEqual(layout('16383 bytes'), [[0, 16384]]);
  assert.deepEqual(layout('16384 bytes'), [[1, 16384], [2, 2]]);
  assert.deepEqual(layout('16385 bytes'), [[1, 16384], [2, 3]]);
  assert.deepEqual(layout('32766 bytes'), [[1, 16384], [2, 16384]]);
  assert.deepEqual(layout('50000 bytes'), [[1, 16384], [1, 16384], [1, 16384], [2, 852]]);
});

test('a multi-byte character cut by a fragment boundary survives the round trip', () => {
  for (const name of ['2-byte characters', '4-byte characters', '3-byte characters']) {
    const c = framing.cases.find((x) => x.name.startsWith(name))!;
    const fragments = encodeMessage(textOf(c.message));
    assert.ok(fragments.length >= 2, name);
    // The first fragment alone is not valid UTF-8: the split really is inside a character.
    assert.throws(() => new TextDecoder('utf-8', { fatal: true }).decode(fragments[0]!.subarray(1)), name);
    assert.deepEqual(reassemble(fragments), [textOf(c.message)], name);
  }
});

test('the fixtures\' invalid sequences all end in a RemoteProtocolError', () => {
  assert.ok(framing.invalid_sequences.length >= 10);
  for (const sequence of framing.invalid_sequences) {
    assert.throws(() => reassemble(sequence.fragments.map(fragmentOf)), RemoteProtocolError, sequence.name);
  }
});

test('the fixtures\' valid sequences reassemble, keeping a byte order mark', () => {
  for (const sequence of framing.valid_sequences) {
    const [message] = reassemble(sequence.fragments.map(fragmentOf));
    assert.equal(hex(new TextEncoder().encode(message)), sequence.message_utf8_hex, sequence.name);
  }
});

// ── Behaviour ────────────────────────────────────────────────────────────────

test('fragments are at most 16384 bytes whatever the message, and the kinds follow the pattern 1…1 2', () => {
  for (const size of [0, 1, 16382, 16383, 16384, 16385, 32766, 32767, 32768, 100_000, 1_000_003]) {
    const text = 'a'.repeat(size);
    const fragments = encodeMessage(text);
    assert.ok(fragments.every((f) => f.length >= 1 && f.length <= FRAGMENT_MAX_BYTES), `size ${size}`);
    const kinds = fragments.map((f) => f[0]);
    assert.deepEqual(kinds.slice(0, -1).every((k) => k === FRAGMENT_KIND.more), true);
    assert.equal(kinds.at(-1), fragments.length === 1 ? FRAGMENT_KIND.whole : FRAGMENT_KIND.last);
    assert.deepEqual(reassemble(fragments), [text]);
  }
});

test('a tunnel-sized fragment size (16374) is accepted and honoured; invalid sizes are refused', () => {
  const fragments = encodeMessage('z'.repeat(100_000), TUNNEL_FRAGMENT_MAX_BYTES);
  assert.ok(fragments.every((f) => f.length <= TUNNEL_FRAGMENT_MAX_BYTES));
  assert.equal(fragments[0]!.length, TUNNEL_FRAGMENT_MAX_BYTES);
  assert.deepEqual(reassemble(fragments), ['z'.repeat(100_000)]);
  for (const bad of [0, 1, 1.5, 16385, -3, Number.NaN]) assert.throws(() => encodeMessage('x', bad), RangeError, String(bad));
  assert.deepEqual(encodeMessage('ab', 2).map((f) => [...f]), [[1, 0x61], [2, 0x62]], 'one payload byte per fragment');
});

test('messages and fragments interleave cleanly: several messages through one Reassembler', () => {
  const reassembler = new Reassembler();
  const messages = ['', 'short', 'x'.repeat(40_000), '灵'.repeat(9_000), '{"a":1}'];
  const received: string[] = [];
  for (const message of messages) {
    for (const fragment of encodeMessage(message)) {
      const done = reassembler.push(fragment);
      if (done !== null) received.push(done);
    }
    assert.equal(reassembler.pendingBytes, 0);
  }
  assert.deepEqual(received, messages);
});

test('pendingBytes tracks a partly received message and reset() discards it', () => {
  const reassembler = new Reassembler();
  const [first, second, third] = encodeMessage('q'.repeat(40_000));
  assert.equal(reassembler.push(first!), null);
  assert.equal(reassembler.pendingBytes, 16383);
  assert.equal(reassembler.push(second!), null);
  assert.equal(reassembler.pendingBytes, 32766);
  reassembler.reset();
  assert.equal(reassembler.pendingBytes, 0);
  assert.throws(() => reassembler.push(third!), /continuation without a start/);
});

test('the Reassembler copies fragments it keeps, so a reused buffer cannot corrupt a message', () => {
  const reassembler = new Reassembler();
  const buffer = new Uint8Array(1 + 3);
  buffer.set([FRAGMENT_KIND.more, 0x61, 0x62, 0x63]);
  assert.equal(reassembler.push(buffer), null);
  buffer.set([FRAGMENT_KIND.last, 0x64, 0x65, 0x66]);
  assert.equal(reassembler.push(buffer), 'abcdef');
});

test('an error resets the Reassembler so the next message starts clean', () => {
  const reassembler = new Reassembler();
  assert.equal(reassembler.push(Uint8Array.from([1, 0x61])), null);
  assert.throws(() => reassembler.push(Uint8Array.from([9, 0x61])), RemoteProtocolError);
  assert.equal(reassembler.pendingBytes, 0);
  assert.equal(reassembler.push(Uint8Array.from([0, 0x62])), 'b');
});

// ── Limits ───────────────────────────────────────────────────────────────────

test('a message of exactly 16 MiB passes and one byte more is refused, by sender and receiver', () => {
  const exact = 'a'.repeat(MESSAGE_MAX_BYTES);
  const fragments = encodeMessage(exact);
  assert.equal(fragments.length, Math.ceil(MESSAGE_MAX_BYTES / (FRAGMENT_MAX_BYTES - 1)));
  const [message] = reassemble(fragments);
  assert.equal(message!.length, MESSAGE_MAX_BYTES);

  assert.throws(() => encodeMessage('a'.repeat(MESSAGE_MAX_BYTES + 1)), RemoteProtocolError);

  // A receiver rejects at the fragment that crosses the limit, before holding more than 16 MiB.
  const reassembler = new Reassembler();
  const payload = new Uint8Array(FRAGMENT_MAX_BYTES).fill(0x61);
  payload[0] = FRAGMENT_KIND.more;
  let held = 0;
  assert.throws(() => {
    for (;;) {
      reassembler.push(payload);
      held += FRAGMENT_MAX_BYTES - 1;
      assert.ok(held <= MESSAGE_MAX_BYTES, 'never accepted past the limit');
    }
  }, /size limit/);
  assert.ok(held > MESSAGE_MAX_BYTES - FRAGMENT_MAX_BYTES, 'accepted right up to it');
});

test('a fragment above 16384 bytes is refused even when it would complete a small message', () => {
  const reassembler = new Reassembler();
  const big = new Uint8Array(FRAGMENT_MAX_BYTES + 1);
  assert.throws(() => reassembler.push(big), /fragment exceeds/);
  const full = new Uint8Array(FRAGMENT_MAX_BYTES).fill(0x61);
  full[0] = 0;
  assert.equal(reassembler.push(full)?.length, FRAGMENT_MAX_BYTES - 1);
});
