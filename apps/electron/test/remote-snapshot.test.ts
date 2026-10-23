import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  EVENT_MAX_BYTES,
  INITIAL_HISTORY_TAIL_MESSAGES,
  PAGE_MAX_BYTES,
  TOOL_FIELD_MAX_BYTES,
  baseImageRef,
  buildSnapshot,
  coalesceRecords,
  historyPage,
  pageBySize,
  parseImageRef,
  promptImageRef,
  recordsAfter,
  resolveImage,
} from '../src/main/remoteControl/snapshot.js';
import { FakeRuntime, PNG_BASE64, SESSION_A, permissionRequest } from './remote-session-helpers.js';

const header = { verb: 'run', label: 'Run' } as any;
const text = (value: string) => ({ role: 'assistant', blocks: [{ type: 'text', text: value }] }) as any;
const userText = (value: string) => ({ role: 'user', blocks: [{ type: 'text', text: value }] }) as any;

function frames(runtime: FakeRuntime) {
  return buildSnapshot(runtime.remoteView());
}

test('a snapshot is begin, every messages page, every records page, then end', () => {
  const runtime = new FakeRuntime();
  runtime.resume([userText('hi'), text('hello')]);
  runtime.push({ type: 'turn_started', turn_id: 3 } as any);
  runtime.push({ type: 'text_delta', text: 'a' } as any);
  const all = frames(runtime);
  assert.deepEqual(all.map((frame) => frame.phase), ['begin', 'messages', 'records', 'end']);
  const [begin, messages, records, end] = all as any[];
  assert.equal(begin.epoch, runtime.epoch);
  assert.equal(begin.first_message_index, 0);
  assert.equal(messages.messages.length, 2);
  assert.deepEqual(records.records.map((item: any) => item.seq), [2, 3]);
  assert.deepEqual(end.cursor, { epoch: runtime.epoch, seq: 3 });
  assert.deepEqual(end.turn, { running: false });
});

test('records of the stream are merged by sequence: events, prompts, interactions and the latest state', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  runtime.journal('first', { origin: 'desktop' });
  runtime.push({ type: 'turn_started', turn_id: 1 } as any);
  runtime.push({ type: 'model_changed', model: 'm2' } as any);
  runtime.askPermission(permissionRequest(9));
  runtime.push({ type: 'tool_use_started', id: 't', tool: 'Bash', input_json: '{}', header } as any);
  const records = recordsAfter(runtime.remoteView(), runtime.baseSequence);
  assert.deepEqual(records.map((item) => item.seq), [2, 3, 4, 5, 6]);
  assert.deepEqual(records.map((item) => item.record.type === 'event' ? item.record.event.type : item.record.type),
    ['prompt', 'turn_started', 'model_changed', 'permission_request', 'tool_use_started']);
});

test('the latest state is part of a snapshot even when it was set before the base', () => {
  const runtime = new FakeRuntime();
  runtime.push({ type: 'model_changed', model: 'early' } as any); // seq 1
  runtime.resume(); // seq 2: the base
  const snapshot = frames(runtime) as any[];
  const records = snapshot.filter((frame) => frame.phase === 'records').flatMap((frame) => frame.records);
  assert.equal(records.length, 0, 'the fake keeps no state across a base, like the real runtime after a reset');
  // A state event cached before the base but kept in `state` (the real runtime keeps it) is still sent.
  runtime.state = [{ sessionId: SESSION_A, sequence: 1, event: { type: 'model_changed', model: 'early' } as any }];
  const again = (frames(runtime) as any[]).filter((frame) => frame.phase === 'records').flatMap((frame) => frame.records);
  assert.deepEqual(again.map((item: any) => item.record.event.model), ['early']);
  // …but a resuming viewer past the base is not sent state it has already seen.
  assert.deepEqual(recordsAfter(runtime.remoteView(), 2), []);
});

test('adjacent deltas are merged into one record at the later sequence, and a held-back event does not separate them', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  runtime.push({ type: 'text_delta', text: 'Hel' } as any); // 2
  runtime.push({ type: 'tool_heartbeat', id: 'x', tool: 'Bash', elapsed_ms: 1 } as any); // 3, dropped
  runtime.push({ type: 'text_delta', text: 'lo' } as any); // 4
  runtime.push({ type: 'thinking_delta', thinking: 'a', signature: 'SECRET-SIG' } as any); // 5
  runtime.push({ type: 'thinking_delta', thinking: 'b' } as any); // 6
  runtime.push({ type: 'text_delta', text: '!' } as any); // 7
  const records = recordsAfter(runtime.remoteView(), 1);
  assert.deepEqual(records.map((item) => item.seq), [4, 6, 7]);
  assert.deepEqual(records.map((item) => item.record.type === 'event' ? item.record.event : undefined), [
    { type: 'text_delta', text: 'Hello' },
    { type: 'thinking_delta', thinking: 'ab' },
    { type: 'text_delta', text: '!' },
  ]);
  assert.ok(!JSON.stringify(records).includes('SECRET-SIG'), 'thinking signatures are not sent');
  assert.equal(coalesceRecords([]).length, 0);
});

test('events the table withholds never reach a snapshot, whatever they carry', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  runtime.push({ type: 'provider_credential_status', secret: 'sk-live-123' } as any);
  runtime.push({ type: 'openai_oauth_updated', access_token: 'tok-456' } as any);
  runtime.push({ type: 'side_chat_event', side_id: 's', event_json: '{"text":"private"}' } as any);
  runtime.push({ type: 'mcp_servers', servers: [{ env: { TOKEN: 'abc' } }] } as any);
  runtime.push({ type: 'brand_new_event_type', payload: 'leak' } as any);
  runtime.push({ type: 'text_delta', text: 'visible' } as any);
  const serialized = JSON.stringify(frames(runtime));
  for (const secret of ['sk-live-123', 'tok-456', 'private', 'abc', 'leak']) assert.ok(!serialized.includes(secret), secret);
  assert.ok(serialized.includes('visible'));
});

test('tool fields above 64 KiB are cut, and the record says so', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  runtime.push({ type: 'tool_use_started', id: 't1', tool: 'Write', input_json: 'x'.repeat(TOOL_FIELD_MAX_BYTES + 10), header } as any);
  runtime.push({ type: 'tool_use_result', id: 't1', tool: 'Write', result_json: 'é'.repeat(TOOL_FIELD_MAX_BYTES), is_error: false, display: {} } as any);
  runtime.push({ type: 'tool_use_started', id: 't2', tool: 'Read', input_json: '{"small":true}', header } as any);
  const [started, result, small] = recordsAfter(runtime.remoteView(), 1).map((item) => item.record as any);
  assert.equal(started.truncated, true);
  assert.equal(Buffer.byteLength(started.event.input_json), TOOL_FIELD_MAX_BYTES);
  assert.equal(result.truncated, true);
  assert.ok(Buffer.byteLength(result.event.result_json) <= TOOL_FIELD_MAX_BYTES);
  assert.doesNotThrow(() => new TextDecoder('utf-8', { fatal: true }).decode(Buffer.from(result.event.result_json)), 'cut on a character boundary');
  assert.equal(small.truncated, undefined);
  assert.equal(small.event.input_json, '{"small":true}');
});

test('an event above 1 MiB becomes an omitted record', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  const big = Array.from({ length: 40 }, (_, i) => ({ role: 'assistant', blocks: [{ type: 'text', text: `${i}`.repeat(40_000) }] }));
  runtime.push({ type: 'message_complete', message: { role: 'assistant', blocks: big.flatMap((m) => m.blocks) } } as any);
  const [record] = recordsAfter(runtime.remoteView(), 1);
  assert.equal(record!.record.type, 'omitted');
  assert.equal((record!.record as any).event_type, 'message_complete');
  assert.ok((record!.record as any).size_bytes > EVENT_MAX_BYTES);
});

test('message images become references the viewer can fetch; only data URLs are served', () => {
  const runtime = new FakeRuntime();
  runtime.resume([{
    role: 'user',
    blocks: [{ type: 'text', text: 'look' }],
    images: [{ media_type: 'image/png', url: `data:image/png;base64,${PNG_BASE64}` }, { media_type: 'image/png', url: 'file:///etc/passwd' }],
  } as any]);
  const [, messages] = frames(runtime) as any[];
  const [message] = messages.messages;
  assert.deepEqual(message.images, [{ media_type: 'image/png', url: baseImageRef(SESSION_A, 0, 0) }]);
  assert.ok(!JSON.stringify(messages).includes('passwd'));
  const view = runtime.remoteView();
  assert.deepEqual(resolveImage(view, baseImageRef(SESSION_A, 0, 0), () => undefined), { media_type: 'image/png', base64: PNG_BASE64 });
  assert.equal(resolveImage(view, baseImageRef(SESSION_A, 0, 1), () => undefined), undefined);
  assert.equal(resolveImage(view, baseImageRef('12121212-3434-4567-8899-ffffffffffff', 0, 0), () => undefined), undefined, 'a ref of another session is not this one');
});

test('prompt images are refs while the journal holds the bytes, and the ref round-trips', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  const entry = runtime.journal('see', { origin: 'device', deviceName: 'Phone' }, [{ media_type: 'image/png', base64: PNG_BASE64 }]);
  const [record] = recordsAfter(runtime.remoteView(), 1).map((item) => item.record as any);
  assert.equal(record.type, 'prompt');
  assert.equal(record.origin, 'device');
  assert.equal(record.device_name, 'Phone');
  assert.deepEqual(record.images, [{ ref: promptImageRef(SESSION_A, entry.sequence, 0), media_type: 'image/png' }]);
  assert.deepEqual(parseImageRef(record.images[0].ref), { sessionId: SESSION_A, kind: 'prompt', sequence: entry.sequence, index: 0 });
  assert.deepEqual(resolveImage(runtime.remoteView(), record.images[0].ref, (s, i) => runtime.promptImage(s, i)), { media_type: 'image/png', base64: PNG_BASE64 });
  for (const bad of ['', 'nope', `${SESSION_A}:x:1:0`, `${SESSION_A}:p:-1:0`, `${SESSION_A}:p:1`, `../${SESSION_A}:p:1:0`]) assert.equal(parseImageRef(bad), undefined, bad);
});

test('a conversation above 4 MiB sends its last 200 messages and says where they start; history pages go back', () => {
  const runtime = new FakeRuntime();
  const messages = Array.from({ length: 700 }, (_, i) => text(`${i}:${'x'.repeat(8_000)}`));
  runtime.resume(messages);
  const all = frames(runtime) as any[];
  const begin = all[0];
  assert.equal(begin.first_message_index, 700 - INITIAL_HISTORY_TAIL_MESSAGES);
  const sent = all.filter((frame) => frame.phase === 'messages').flatMap((frame) => frame.messages);
  assert.equal(sent.length, INITIAL_HISTORY_TAIL_MESSAGES);
  assert.ok(sent[0].blocks[0].text.startsWith('500:'));
  const page = historyPage(runtime.remoteView(), 500, 100);
  assert.equal(page.first_index, 400);
  assert.equal(page.has_more, true);
  assert.equal(page.messages.length, 100);
  assert.ok((page.messages[0] as any).blocks[0].text.startsWith('400:'));
  assert.equal(historyPage(runtime.remoteView(), 50, 100).has_more, false);
  assert.equal(historyPage(runtime.remoteView(), 9999, 5).first_index, 695);
});

test('a small conversation is sent whole', () => {
  const runtime = new FakeRuntime();
  runtime.resume(Array.from({ length: 300 }, (_, i) => text(`m${i}`)));
  const begin = (frames(runtime) as any[])[0];
  assert.equal(begin.first_message_index, 0);
});

test('pages hold at most 256 KiB each, and an oversized item gets a page of its own', () => {
  const item = (n: number) => ({ n, pad: 'x'.repeat(100_000) });
  const pages = pageBySize([item(1), item(2), item(3), item(4), item(5)]);
  assert.deepEqual(pages.map((page) => page.length), [2, 2, 1]);
  for (const page of pages) assert.ok(Buffer.byteLength(JSON.stringify(page)) <= PAGE_MAX_BYTES + 2);
  const huge = pageBySize([{ pad: 'y'.repeat(PAGE_MAX_BYTES * 2) }, item(1)]);
  assert.deepEqual(huge.map((page) => page.length), [1, 1]);
  assert.deepEqual(pageBySize([]), []);
});

test('records are paged in snapshots too, in sequence order', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  for (let i = 0; i < 12; i += 1) runtime.push({ type: 'tool_use_result', id: `t${i}`, tool: 'Read', result_json: 'r'.repeat(50_000), is_error: false, display: {} } as any);
  const all = frames(runtime) as any[];
  const recordFrames = all.filter((frame) => frame.phase === 'records');
  assert.ok(recordFrames.length >= 2);
  const seqs = recordFrames.flatMap((frame) => frame.records.map((item: any) => item.seq));
  assert.deepEqual(seqs, [...seqs].sort((a, b) => a - b));
  assert.equal(seqs.length, 12);
  assert.deepEqual(all.map((frame) => frame.phase).filter((phase, i, list) => list.indexOf(phase) === i), ['begin', 'records', 'end']);
});
