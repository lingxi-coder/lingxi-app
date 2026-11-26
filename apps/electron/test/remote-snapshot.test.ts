import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ManualClock, MESSAGE_MAX_BYTES } from '@lingxi/bridge-client/remote';

import {
  EVENT_MAX_BYTES,
  INITIAL_HISTORY_MAX_BYTES,
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
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands.js';

const header = { verb: 'run', label: 'Run' } as any;
const text = (value: string) => ({ role: 'assistant', blocks: [{ type: 'text', text: value }] }) as any;
const userText = (value: string) => ({ role: 'user', blocks: [{ type: 'text', text: value }] }) as any;

function frames(runtime: FakeRuntime) {
  return buildSnapshot(runtime.remoteView());
}

test('remote history exposes a reference and sends an original only in requested ranges', async () => {
  const runtime = new FakeRuntime();
  const reference = `lingxi-history-image:${SESSION_A}:0:png`;
  runtime.resume([{ ...userText('attached'), images: [{ media_type: 'image/png', url: reference }] }]);
  const remoteRef = baseImageRef(runtime.sessionId, 0, 0);
  const frame = frames(runtime).find(frame => frame.phase === 'messages');
  assert.equal(frame?.messages[0]?.images?.[0]?.url, remoteRef);
  const calls: string[] = [];
  const original = 'A'.repeat(1024 * 1024) + 'QUJD';
  const commands = new RemoteCommands({
    clock: new ManualClock(), allowAlwaysAllow: () => false, echo: () => {},
    readHistoryImage: async (_runtime, requested, size) => {
      assert.equal(requested, reference);
      calls.push(size);
      return { mediaType: 'image/png', base64: size === 'thumbnail' ? PNG_BASE64 : original };
    },
  });
  assert.deepEqual(calls, []);
  const preview = await commands.imageGet(runtime, { ref: remoteRef, size: 'thumbnail' });
  assert.equal(preview.base64, PNG_BASE64);
  const first = await commands.imageGet(runtime, { ref: remoteRef, size: 'original' });
  const rest = await commands.imageGet(runtime, { ref: remoteRef, size: 'original', offset: first.next_offset });
  assert.equal(first.next_offset, 1024 * 1024);
  assert.equal(rest.next_offset, undefined);
  assert.equal(first.base64 + rest.base64, original);
  assert.deepEqual(calls, ['thumbnail', 'original']);
  assert.throws(() => commands.imageGet(runtime, { ref: remoteRef, size: 'original', offset: 3 }), /invalid image offset/);
});

test('two interleaved 20 MiB originals each read storage once', async () => {
  const runtime = new FakeRuntime();
  const refs = [0, 1, 2].map(index => 'lingxi-history-image:' + SESSION_A + ':' + index + ':png');
  runtime.resume([{ ...userText('attached'), images: refs.map(url => ({ media_type: 'image/png', url })) }]);
  const clock = new ManualClock();
  const reads: string[] = [];
  const originals = [0x39, 0x62, 0x24].map(byte => Buffer.alloc(20 * 1024 * 1024, byte).toString('base64'));
  const commands = new RemoteCommands({
    clock, allowAlwaysAllow: () => false, echo: () => {},
    readHistoryImage: async (_runtime, reference) => {
      reads.push(reference);
      return { mediaType: 'image/png', base64: originals[refs.indexOf(reference)]! };
    },
  });
  const owners = [{}, {}, {}];
  const read = (index: number, offset: number) => commands.imageGet(runtime,
    { ref: baseImageRef(runtime.sessionId, 0, index), size: 'original', offset }, undefined, owners[index]);
  let offsets: Array<number | undefined> = [0, 0];
  let parts = 0;
  while (offsets.some(offset => offset !== undefined)) {
    if (parts === 2) await assert.rejects(async () => read(2, 0), { code: 'capacity' });
    offsets = await Promise.all(offsets.map(async (offset, index) => {
      if (offset === undefined) return undefined;
      const reply = await read(index, offset);
      assert.equal(reply.base64, originals[index]!.slice(offset, offset + 1024 * 1024));
      parts++;
      return reply.next_offset;
    }));
  }
  assert.equal(parts, 54);
  assert.deepEqual(reads, refs.slice(0, 2));
  assert.equal(clock.pending, 0);
  await read(2, 0);
  assert.equal(clock.pending, 1);
  commands.clearImageTransfers(owners[2]!);
  assert.equal(clock.pending, 0);
  await read(2, 0);
  clock.advance(15_000);
  assert.equal(clock.pending, 0);
});

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
  runtime.push({ type: 'provider_accounts_updated', secret: 'sk-live-123' } as any);
  runtime.push({ type: 'provider_account_refresh_requested', access_token: 'tok-456' } as any);
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

test('a long conversation sends a bounded tail with absolute image references and pages backward', () => {
  const runtime = new FakeRuntime();
  const messages = Array.from({ length: 700 }, (_, i) => text(`${i}:${'x'.repeat(8_000)}`));
  const oldestBlocks = messages[0].blocks;
  let oldestReads = 0;
  Object.defineProperty(messages[0], 'blocks', { enumerable: true, get() {
    oldestReads += 1;
    return oldestBlocks;
  } });
  const first = 700 - INITIAL_HISTORY_TAIL_MESSAGES;
  messages[first].images = [{ media_type: 'image/png', url: `data:image/png;base64,${PNG_BASE64}` }];
  runtime.resume(messages);
  const all = frames(runtime) as any[];
  const begin = all[0];
  assert.equal(begin.first_message_index, first);
  const sent = all.filter((frame) => frame.phase === 'messages').flatMap((frame) => frame.messages);
  assert.equal(sent.length, INITIAL_HISTORY_TAIL_MESSAGES);
  assert.ok(sent[0].blocks[0].text.startsWith(String(first) + ':'));
  assert.equal(sent[0].images[0].url, baseImageRef(SESSION_A, first, 0));
  assert.equal(oldestReads, 0, 'initial resume can leave the oldest message unprojected');
  const page = historyPage(runtime.remoteView(), first, 100);
  assert.equal(page.first_index, first - 100);
  assert.equal(page.has_more, true);
  assert.equal(page.messages.length, 100);
  assert.ok((page.messages[0] as any).blocks[0].text.startsWith(String(first - 100) + ':'));
  const oldest = historyPage(runtime.remoteView(), 50, 100);
  assert.equal(oldest.has_more, false);
  assert.deepEqual(oldest.messages[0]?.blocks, oldestBlocks);
  assert.ok(oldestReads > 0, 'the oldest message is still available on demand');
  assert.equal(historyPage(runtime.remoteView(), 9999, 5).first_index, 695);
});

test('the initial history cutoff uses exact projected UTF-8 array bytes', () => {
  const runtime = new FakeRuntime();
  const messages = Array.from({ length: INITIAL_HISTORY_TAIL_MESSAGES }, () => text(''));
  const oldest = messages[0].blocks[0];
  oldest.text = '雪"';
  oldest.text += 'x'.repeat(INITIAL_HISTORY_MAX_BYTES - Buffer.byteLength(JSON.stringify(messages)));
  runtime.resume(messages);
  const exact = frames(runtime)[0];
  assert.equal(exact?.phase === 'begin' && exact.first_message_index, 0);

  oldest.text += 'x';
  runtime.resume(messages);
  const above = frames(runtime)[0];
  assert.equal(above?.phase === 'begin' && above.first_message_index, 1);
});

test('a message larger than the initial history byte budget remains available on demand', () => {
  const runtime = new FakeRuntime();
  const large = text('x'.repeat(INITIAL_HISTORY_MAX_BYTES));
  runtime.resume([userText('older'), large]);
  const snapshot = frames(runtime);
  assert.equal(snapshot[0]?.phase === 'begin' && snapshot[0].first_message_index, 2);
  assert.equal(snapshot.some(frame => frame.phase === 'messages'), false);
  assert.deepEqual(historyPage(runtime.remoteView(), 2, 1), { messages: [large], first_index: 1, has_more: true });
});

test('large earlier pages stay within the RPC limit and follow a gapless cursor', () => {
  const runtime = new FakeRuntime();
  const messages = Array.from({ length: 50 }, (_, index) => text(`${index}:${'x'.repeat(512 * 1024)}`));
  runtime.resume(messages);
  const received: number[] = [];
  let before = messages.length;
  while (before > 0) {
    const page = historyPage(runtime.remoteView(), before, 50);
    assert.ok(Buffer.byteLength(JSON.stringify(page)) < MESSAGE_MAX_BYTES);
    assert.equal(page.first_index + page.messages.length, before);
    assert.ok(page.first_index < before);
    received.unshift(...page.messages.map((row: any) => Number(row.blocks[0].text.split(':', 1)[0])));
    before = page.first_index;
  }
  assert.deepEqual(received, messages.map((_, index) => index));
});

test('compact history continues on disk and its historical image references expire at a new base', async () => {
  const anchor = '44444444-4444-4444-8444-444444444444';
  const older = '55555555-5555-4555-8555-555555555555';
  const oldest = '66666666-6666-4666-8666-666666666666';
  const reference = `lingxi-history-image:${older}:0:png:73:YWJj`;
  class ArchivedRuntime extends FakeRuntime {
    override remoteView() { return { ...super.remoteView(), historyBefore: { uuid: anchor } }; }
  }
  const runtime = new ArchivedRuntime();
  runtime.resume([{ ...text('current'), transcript_uuid: anchor }]);
  const begin = frames(runtime)[0];
  assert.equal(begin?.phase, 'begin');
  if (!begin || begin.phase !== 'begin') return;
  const reads: import('../src/shared/sessionHistory.js').TranscriptHistoryCursor[] = [];
  const physical = { uuid: older, offset: 73, identity: 'abc' };
  const commands = new RemoteCommands({
    clock: new ManualClock(), allowAlwaysAllow: () => false, echo: () => {},
    readHistory: async (_runtime, cursor, limit) => {
      reads.push(cursor);
      assert.equal(limit, 1);
      return cursor.uuid === anchor
        ? { messages: [{ ...userText('older'), transcript_uuid: older, images: [{ media_type: 'image/png', url: reference }] }], before: physical }
        : { messages: [{ ...text('oldest'), transcript_uuid: oldest }], before: null };
    },
    readHistoryImage: async (_runtime, ref) => {
      assert.equal(ref, reference);
      return { mediaType: 'image/png', base64: PNG_BASE64 };
    },
  });
  const first = await commands.history(runtime, { before_index: begin.first_message_index, limit: 1 });
  assert.equal(first.has_more, true);
  assert.equal(first.first_index, begin.first_message_index - 1);
  assert.equal(first.messages[0]?.transcript_uuid, older);
  const image = first.messages[0]?.images?.[0]?.url;
  assert.ok(image);
  assert.equal((await commands.imageGet(runtime, { ref: image, size: 'thumbnail' })).base64, PNG_BASE64);
  assert.throws(() => commands.imageGet(runtime, { ref: image.slice(0, -4) + 'AAAA' }), /no longer available/);
  const last = await commands.history(runtime, { before_index: first.first_index, limit: 1 });
  assert.deepEqual([last.first_index, last.has_more, last.messages[0]?.transcript_uuid],
    [first.first_index - 1, false, oldest]);
  assert.deepEqual(reads, [{ uuid: anchor }, physical]);
  runtime.resume();
  assert.throws(() => commands.imageGet(runtime, { ref: image }), /no longer available/);
});

test('a conversation smaller than a page is sent whole', () => {
  const runtime = new FakeRuntime();
  runtime.resume(Array.from({ length: 30 }, (_, i) => text(`m${i}`)));
  const begin = (frames(runtime) as any[])[0];
  assert.equal(begin.first_message_index, 0);
});

test('100 and 600 short history rows send exactly the same first page', () => {
  const results = [100, 600].map(count => {
    const runtime = new FakeRuntime();
    const tail = Array.from({ length: INITIAL_HISTORY_TAIL_MESSAGES }, (_, index) => userText('visible ' + index));
    const older = Array.from({ length: count - tail.length }, (_, index) => userText('older ' + index));
    runtime.resume([...older, ...tail]);
    const snapshot = frames(runtime);
    const first = snapshot[0];
    assert.equal(first?.phase === 'begin' && first.first_message_index, older.length);
    const olderPage = historyPage(runtime.remoteView(), older.length, tail.length);
    assert.deepEqual(olderPage.messages, older.slice(-tail.length));
    return JSON.stringify(snapshot.filter(frame => frame.phase === 'messages'));
  });
  assert.equal(results[0], results[1]);
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


test('snapshot mode comes from the current session base', () => {
  const runtime = new FakeRuntime();
  runtime.resume();
  runtime.base = { ...runtime.base!, event: { ...runtime.base!.event, mode: 'code' } as any };
  assert.equal((frames(runtime)[0] as any).session_mode, 'code');
  runtime.base = { ...runtime.base!, event: { ...runtime.base!.event, mode: 'chat' } as any };
  assert.equal((frames(runtime)[0] as any).session_mode, 'chat');
});

test('an admitted draft exposes its confirmed mode without a transcript base', () => {
  const runtime = new FakeRuntime();
  const begin = buildSnapshot({ ...runtime.remoteView(), sessionMode: 'chat' })[0] as any;
  assert.equal(begin.session_mode, 'chat');
});
