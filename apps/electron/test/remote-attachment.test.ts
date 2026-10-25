import assert from 'node:assert/strict';
import { test } from 'node:test';

import { LIVE_BATCH_MS, SessionAttachment, canResume } from '../src/main/remoteControl/sessionAttachment.js';
import { FakeRuntime, RecordingChannel, newClock, permissionRequest, questionRequest } from './remote-session-helpers.js';

function setup() {
  const runtime = new FakeRuntime();
  runtime.resume();
  const channel = new RecordingChannel();
  const clock = newClock();
  const ended: string[] = [];
  const attachment = new SessionAttachment({ runtime, channel, clock, onEnded: () => ended.push('ended') });
  return { runtime, channel, clock, attachment, ended };
}

test('attaching without a position sends a snapshot: begin … end, and the reply says snapshot', () => {
  const { runtime, channel, attachment } = setup();
  runtime.push({ type: 'text_delta', text: 'hi' } as any);
  const result = attachment.attach(undefined);
  assert.deepEqual(result, { mode: 'snapshot', epoch: runtime.epoch });
  assert.deepEqual(channel.methods().map((method) => method), ['session.snapshot', 'session.snapshot', 'session.snapshot']);
  assert.deepEqual(channel.of('session.snapshot').map((frame) => frame.phase), ['begin', 'records', 'end']);
});

test('the same epoch and a position the runtime has reached resume with a delta of only what is missing', () => {
  const { runtime, channel, attachment } = setup();
  runtime.push({ type: 'turn_started', turn_id: 1 } as any); // 2
  runtime.push({ type: 'text_delta', text: 'a' } as any); // 3
  runtime.push({ type: 'text_delta', text: 'b' } as any); // 4
  const result = attachment.attach({ epoch: runtime.epoch, after_seq: 2 }) as any;
  assert.equal(result.mode, 'delta');
  assert.equal(result.epoch, runtime.epoch);
  assert.deepEqual(result.pending, { permissions: [], questions: [], computer_access: [] });
  assert.deepEqual(channel.records(), [{ seq: 4, record: { type: 'event', event: { type: 'text_delta', text: 'ab' } } }]);
  assert.deepEqual(channel.methods(), ['session.event']);
});

test('a viewer that is already up to date gets an empty delta', () => {
  const { runtime, channel, attachment } = setup();
  runtime.push({ type: 'text_delta', text: 'a' } as any);
  const result = attachment.attach({ epoch: runtime.epoch, after_seq: runtime.lastSequence }) as any;
  assert.equal(result.mode, 'delta');
  assert.equal(channel.sent.length, 0);
});

test('an older epoch, a position below the base or beyond the end, or a forgotten stretch get a snapshot', () => {
  const { runtime } = setup();
  runtime.push({ type: 'text_delta', text: 'a' } as any);
  const view = runtime.remoteView();
  assert.equal(canResume(view, { epoch: runtime.epoch, after_seq: 1 }), true);
  assert.equal(canResume(view, { epoch: 'rt0:1', after_seq: 1 }), false, 'another runtime');
  assert.equal(canResume(view, { epoch: 'rt1:0', after_seq: 1 }), false, 'an older base');
  assert.equal(canResume(view, { epoch: runtime.epoch, after_seq: 0 }), false, 'before the base');
  assert.equal(canResume(view, { epoch: runtime.epoch, after_seq: 99 }), false, 'beyond the end');
  assert.equal(canResume(view, { epoch: runtime.epoch, after_seq: 1.5 }), false);
  assert.equal(canResume(view, undefined), false);
  runtime.droppedThroughSequence = 5;
  runtime.push({ type: 'text_delta', text: 'b' } as any);
  runtime.push({ type: 'text_delta', text: 'c' } as any);
  runtime.push({ type: 'text_delta', text: 'd' } as any);
  runtime.push({ type: 'text_delta', text: 'e' } as any);
  assert.equal(canResume(runtime.remoteView(), { epoch: runtime.epoch, after_seq: 3 }), false, 'records at or below the dropped floor may be missing');
  assert.equal(canResume(runtime.remoteView(), { epoch: runtime.epoch, after_seq: 5 }), true);
});

test('after a new base the resume epoch is stale: a snapshot, not a delta', () => {
  const { runtime, attachment } = setup();
  const before = runtime.epoch;
  runtime.push({ type: 'text_delta', text: 'a' } as any);
  runtime.resume(); // /clear
  assert.notEqual(runtime.epoch, before);
  const result = attachment.attach({ epoch: before, after_seq: 2 });
  assert.equal(result.mode, 'snapshot');
  assert.equal(result.epoch, runtime.epoch);
});

test('live records are batched for 50 ms, merged, and sent in order', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.push({ type: 'turn_started', turn_id: 1 } as any);
  runtime.push({ type: 'text_delta', text: 'Hel' } as any);
  runtime.push({ type: 'text_delta', text: 'lo' } as any);
  assert.equal(channel.sent.length, 0, 'nothing is sent before the batch timer');
  clock.advance(LIVE_BATCH_MS - 1);
  assert.equal(channel.sent.length, 0);
  clock.advance(1);
  assert.deepEqual(channel.records().map((item) => item.seq), [2, 4]);
  assert.deepEqual((channel.records()[1]!.record as any).event, { type: 'text_delta', text: 'Hello' });
  runtime.push({ type: 'text_delta', text: '!' } as any);
  clock.advance(LIVE_BATCH_MS);
  assert.deepEqual(channel.records().map((item) => item.seq), [2, 4, 5], 'sequence numbers only grow');
});

test('events the table withholds are not sent live either', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.push({ type: 'provider_credential_status', secret: 'nope' } as any);
  runtime.push({ type: 'tool_heartbeat', id: 'x', tool: 'Bash', elapsed_ms: 5 } as any);
  clock.advance(LIVE_BATCH_MS);
  assert.equal(channel.sent.length, 0);
});

test('a new base tells the viewer at once and sends a fresh snapshot by itself after the batch timer', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.push({ type: 'text_delta', text: 'old' } as any);
  runtime.resume([{ role: 'user', blocks: [{ type: 'text', text: 'after clear' }] } as any]);
  assert.deepEqual(channel.methods(), ['session.reset'], 'reset is immediate');
  runtime.push({ type: 'text_delta', text: 'during the wait' } as any);
  assert.deepEqual(channel.methods(), ['session.reset']);
  clock.advance(LIVE_BATCH_MS);
  const frames = channel.of('session.snapshot');
  assert.deepEqual(frames.map((frame) => frame.phase), ['begin', 'messages', 'records', 'end']);
  assert.equal(frames[0].epoch, runtime.epoch);
  assert.ok(JSON.stringify(frames).includes('after clear'));
  assert.ok(JSON.stringify(frames).includes('during the wait'), 'the snapshot is taken when the timer fires');
  assert.ok(!JSON.stringify(channel.sent).includes('"old"'), 'nothing of the old base is sent');
  assert.equal(channel.of('session.reset').length, 1);
  // …and the stream goes on in the new epoch.
  channel.clear();
  runtime.push({ type: 'text_delta', text: 'more' } as any);
  clock.advance(LIVE_BATCH_MS);
  assert.deepEqual(channel.methods(), ['session.event']);
});

test('two new bases in a row make one reset and one snapshot', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.resume();
  runtime.resume();
  clock.advance(LIVE_BATCH_MS);
  assert.equal(channel.of('session.reset').length, 1);
  assert.equal(channel.of('session.snapshot').filter((frame) => frame.phase === 'begin').length, 1);
});

test('a pending change is announced after the records that led to it, and only when the set really changed', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.askPermission(permissionRequest(7));
  assert.deepEqual(channel.methods(), ['session.event', 'session.pending'], 'the request record, then the new pending set');
  assert.equal(channel.of('session.pending')[0].pending.permissions[0].request_id, 7);
  channel.clear();
  runtime.emit({ type: 'pending' }); // nothing changed
  assert.equal(channel.sent.length, 0);
  runtime.askQuestion(questionRequest(8));
  assert.equal(channel.of('session.pending')[0].pending.questions[0].request_id, 8);
  channel.clear();
  runtime.settle(7);
  runtime.settle(8);
  const pending = channel.of('session.pending');
  assert.deepEqual(pending.at(-1).pending, { permissions: [], questions: [], computer_access: [] });
  clock.advance(LIVE_BATCH_MS);
});

test('a delta resume reports what is pending now', () => {
  const { runtime, attachment } = setup();
  runtime.askPermission(permissionRequest(7));
  const result = attachment.attach({ epoch: runtime.epoch, after_seq: runtime.lastSequence }) as any;
  assert.equal(result.mode, 'delta');
  assert.equal(result.pending.permissions[0].request_id, 7);
});

test('closing stops the stream; a close with a reason tells the viewer and flushes first', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.push({ type: 'text_delta', text: 'last' } as any);
  attachment.close('remote_disabled');
  assert.deepEqual(channel.methods(), ['session.event', 'session.closed']);
  assert.equal(channel.of('session.closed')[0].reason, 'remote_disabled');
  runtime.push({ type: 'text_delta', text: 'ignored' } as any);
  clock.advance(1000);
  assert.equal(channel.sent.length, 2);
  assert.equal(runtime.observers.size, 0);
});

test('a viewer that detaches is not told anything', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.push({ type: 'text_delta', text: 'x' } as any);
  attachment.close();
  clock.advance(1000);
  assert.equal(channel.sent.length, 0);
});

test('a runtime that is disposed ends the attachment with a notice', () => {
  const { runtime, channel, attachment, ended } = setup();
  attachment.attach(undefined);
  channel.clear();
  runtime.dispose();
  assert.equal(channel.of('session.closed')[0].reason, 'ended');
  assert.deepEqual(ended, ['ended']);
  assert.equal(attachment.isClosed, true);
});

test('presence defaults to background and follows what the viewer says', () => {
  const { attachment } = setup();
  assert.equal(attachment.foreground, false);
  attachment.setPresence(true);
  assert.equal(attachment.foreground, true);
});

test('there is no gap between the snapshot and the live stream: an event right after attach is sent live', () => {
  const { runtime, channel, clock, attachment } = setup();
  attachment.attach(undefined);
  const lastSnapshotSeq = channel.of('session.snapshot').at(-1).cursor.seq;
  channel.clear();
  runtime.push({ type: 'text_delta', text: 'x' } as any);
  clock.advance(LIVE_BATCH_MS);
  assert.equal(channel.records()[0]!.seq, lastSnapshotSeq + 1);
});
