import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import type { TestContext } from 'node:test';
import type { ClientEvent } from '@lingxi/bridge-client';
import type { SequencedRuntimeEventEnvelope } from '../src/main/bridgeTypes.js';
import { SessionRuntime } from '../src/main/bridge.js';
import { replayCheckpointState, REPLAY_LIVE_TAIL_LIMIT, RuntimeEventReplay } from '../src/main/runtimeEventReplay.js';
import { canResume, LIVE_BATCH_MS, SessionAttachment } from '../src/main/remoteControl/sessionAttachment.js';
import { recordsAfter } from '../src/main/remoteControl/snapshot.js';
import { createRuntimeEventReplayBuffer, mergeRuntimeEventReplay } from '../src/preload/event-replay.js';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation.js';
import { initialHistory } from '../src/shared/sessionHistory.js';
import { newClock, permissionRequest, RecordingChannel } from './remote-session-helpers.js';

const SESSION = '11111111-2222-4333-8444-555555555555';
type Envelope = SequencedRuntimeEventEnvelope<ClientEvent>;
const COST = { total_usd: 0, input_tokens: 10, output_tokens: 20, api_calls: 1,
  session_duration_secs: 2, formatted: '2s', by_account: [] };
const HEADER = { verb: 'generic' as const, label: 'Tool', title: 'Tool' };
const DISPLAY = { headline: 'Completed', headline_kind: 'plain' as const,
  headline_args: [], body: 'ok', body_lines: 1 };
const fold = (events: readonly Envelope[]) => events.reduce(
  (state, envelope) => reduceEvent(state, envelope.event, 1_000), emptyConversation());

function fixture() {
  const replay = new RuntimeEventReplay();
  const original: Envelope[] = [];
  let sequence = 0;
  const emit = (event: ClientEvent): Envelope => {
    const envelope = Object.freeze({ sessionId: SESSION, sequence: ++sequence, event });
    original.push(envelope);
    if (event.type === 'session_started' || event.type === 'session_resumed') replay.reset(envelope);
    else replay.append(envelope);
    return envelope;
  };
  emit({ type: 'session_started', session_id: SESSION, mode: 'code' });
  return { replay, original, emit, skip: () => { sequence++; } };
}

test('repeated compacts retain exact conversation, tool, block and UUID state with a bounded raw tail', t => {
  const { replay, original, emit } = fixture();
  const retained: number[] = [];
  for (let turn = 0; turn < 3; turn++) {
    emit({ type: 'turn_started', turn_id: turn });
    emit({ type: 'query_model_change', to_model: 'provider/model' });
    emit({ type: 'assistant_block_start', block_key: turn * 10 });
    for (let i = 0; i < 2_000; i++) emit({ type: 'thinking_delta', thinking: `reason ${i};` });
    emit({ type: 'assistant_block_start', block_key: turn * 10 + 1 });
    emit({ type: 'tool_use_started', id: `tool-${turn}`, tool: 'Read', input_json: '{"path":"file"}', header: HEADER });
    emit({ type: 'assistant_block_identity', block_key: turn * 10 + 1, message_uuid: `tool-row-${turn}` });
    emit({ type: 'assistant_block_start', block_key: turn * 10 + 2 });
    for (let i = 0; i < 18_000; i++) {
      emit({ type: 'text_delta', text: `part ${i};` });
      if (i % 10 === 0) emit({ type: 'tool_heartbeat', id: `tool-${turn}`, tool: 'Read', elapsed_ms: i * 1_000 });
    }
    emit({ type: 'message_identity', message_id: `attempt-${turn}` });
    emit({ type: 'assistant_block_identity', block_key: turn * 10 + 2, message_uuid: `answer-row-${turn}` });
    emit({ type: 'tool_use_result', id: `tool-${turn}`, tool: 'Read', result_json: '{"done":true}', is_error: false, display: DISPLAY });
    emit({ type: 'message_complete', message: { role: 'assistant', blocks: [{ type: 'text', text: 'durable payload' }] } });
    emit({ type: 'turn_ended', outcome: { type: turn === 1 ? 'cancelled' : 'end_turn' }, cost: COST });
    emit({ type: 'compaction_status', phase: 'summarizing' });
    emit({ type: 'compaction_status', phase: 'complete' });
    emit({ type: 'compaction_completed', messages_before: 100, messages_after: 3, bytes_saved: 400, summary: `summary-${turn}` });
    assert.deepEqual(fold(replay.snapshot()), fold(original), 'coalescing preserves the complete reducer state');
    assert.ok(replay.length < REPLAY_LIVE_TAIL_LIMIT + (turn + 1) * 30,
      '20k fragments per compact add only semantic block/lifecycle entries');
    retained.push(replay.length);
  }
  assert.ok(original.length > 60_000);
  assert.equal(replay.snapshot().filter(({ event }) => event.type === 'message_complete').length, 3,
    'completed message payloads remain available for remote images and metadata');
  t.diagnostic(`semantic spans: ${original.length} input frames; retained checkpoints ${retained.join(' -> ')}`);
});

test('retry retraction, refusal replacement, tombstones and cancellation remain equivalent after checkpointing', () => {
  const { replay, original, emit } = fixture();
  emit({ type: 'turn_started', turn_id: 1 });
  emit({ type: 'assistant_block_start', block_key: 91 });
  for (let i = 0; i < 400; i++) emit({ type: 'text_delta', text: 'discarded ' });
  emit({ type: 'message_identity', message_id: 'retry' });
  emit({ type: 'assistant_block_identity', block_key: 91, message_uuid: 'old-row' });
  emit({ type: 'message_complete' });
  emit({ type: 'message_retracted', message_id: 'retry' });
  emit({ type: 'tombstone', display_only: true, message: {
    uuid: 'old-row', type: 'assistant', timestamp: '2026-11-12T00:00:00.000Z',
    message: { content_json: '[]' },
  } });
  emit({ type: 'refusal_continuation', phase: 'begin', salvage_text: 'kept', join: 'exact', replaces_uuids: [], display_salvage_text: true });
  emit({ type: 'assistant_block_start', block_key: 92 });
  emit({ type: 'thinking_delta', thinking: 'signed', signature: 'opaque-proof' });
  for (let i = 0; i < 400; i++) emit({ type: 'text_delta', text: ' answer' });
  emit({ type: 'assistant_block_identity', block_key: 92, message_uuid: 'new-row' });
  emit({ type: 'tool_use_started', id: 'interrupted', tool: 'Read', input_json: '{}', header: HEADER });
  for (let i = 0; i < 400; i++) emit({ type: 'tool_heartbeat', id: 'interrupted', tool: 'Read', elapsed_ms: i * 1_000 });
  emit({ type: 'turn_ended', outcome: { type: 'cancelled' }, cost: COST });
  assert.deepEqual(fold(replay.snapshot()), fold(original));
  const interrupted = fold(replay.snapshot()).items.find(item => item.type === 'tool' && item.id === 'interrupted');
  assert.ok(interrupted?.type === 'tool' && interrupted.status === 'error');
  assert.ok(replay.snapshot().some(({ event }) => event.type === 'thinking_delta' && event.signature === 'opaque-proof'));
});

test('a captured replay merges concurrently buffered fragments exactly once, including a live last fragment', () => {
  const { replay, original, emit } = fixture();
  emit({ type: 'turn_started', turn_id: 1 });
  emit({ type: 'tool_use_started', id: 'running', tool: 'Read', input_json: '{}', header: HEADER });
  for (let i = 0; i < 1_000; i++) {
    emit({ type: 'text_delta', text: String(i) });
    if (i % 10 === 0) emit({ type: 'tool_heartbeat', id: 'running', tool: 'Read', elapsed_ms: i * 1_000 });
  }
  const captured = replay.snapshot();
  const stable = structuredClone(captured);
  const buffered = original.slice(2);
  for (let i = 0; i < 400; i++) buffered.push(emit({ type: 'text_delta', text: `tail-${i}` }));
  buffered.push(emit({ type: 'turn_ended', outcome: { type: 'cancelled' }, cost: COST }));
  const delivered: Envelope[] = [];
  const receiver = createRuntimeEventReplayBuffer<ClientEvent>(envelope => delivered.push(envelope));
  for (const event of buffered) receiver.push(event);
  receiver.resolve(captured);
  assert.deepEqual(captured, stable, 'later coalescing never mutates a captured replay');
  assert.deepEqual(fold(delivered), fold(original));
  assert.deepEqual(mergeRuntimeEventReplay(captured, buffered), delivered);
});

test('spans never cover unretained events or another session', () => {
  const { replay, original, emit, skip } = fixture();
  for (let i = 0; i < 300; i++) emit({ type: 'text_delta', text: 'a' });
  skip(); // A model/state/SideChat event is outside transcript replay.
  const outside: Envelope = { sessionId: SESSION, sequence: 302, event: { type: 'model_changed', model: 'other' } };
  for (let i = 0; i < 300; i++) emit({ type: 'text_delta', text: 'b' });
  const other: Envelope = { sessionId: 'other-session', sequence: 2, event: { type: 'text_delta', text: 'other' } };
  const merged = mergeRuntimeEventReplay(replay.snapshot(), [outside, other, ...original]);
  assert.ok(merged.includes(outside));
  assert.ok(merged.includes(other));
  assert.deepEqual(fold(merged.filter(envelope => envelope.sessionId === SESSION)), fold(original));
});

test('remote cursors in rewritten spans resnapshot while recent cursors receive only their raw suffix', async t => {
  const runtime = new SessionRuntime({ sessionId: SESSION, projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId: SESSION, trusted: true }) });
  t.after(() => runtime.dispose());
  const emit = (event: ClientEvent) => (runtime as any).broadcastClientEvent(event);
  emit({ type: 'session_started', session_id: SESSION, mode: 'code' });
  for (let i = 0; i < 1_000; i++) emit({ type: 'text_delta', text: 'x' });
  const view = runtime.remoteView();
  assert.equal(canResume(view, { epoch: view.epoch, after_seq: 500 }), false);
  assert.equal(canResume(view, { epoch: view.epoch, after_seq: 900 }), true);
  const suffix = recordsAfter(view, 900);
  assert.equal(suffix.length, 1);
  assert.equal(suffix[0]?.seq, 1_001);
  assert.deepEqual(suffix[0]?.record, { type: 'event', event: { type: 'text_delta', text: 'x'.repeat(101) } });
});

test('source checkpoint overlays preserve plans, merged usage and pinned state without replaying stale totals', () => {
  const { replay, emit } = fixture();
  emit({ type: 'plan_updated', tasks: [{ id: 'step', subject: 'continue', state: 'in_progress' }] });
  emit({ type: 'ui_status', plugin: 'one', text: 'latest' });
  emit({ type: 'usage_update', input_tokens: 10, output_tokens: 1, cache_read_tokens: 20, cache_creation_tokens: 30 });
  emit({ type: 'usage_update', input_tokens: 0, output_tokens: 9, cache_read_tokens: 0, cache_creation_tokens: 0 });
  const base: Envelope = { sessionId: SESSION, sequence: 1, event: { type: 'session_resumed', session_id: SESSION, mode: 'code', messages: [] } };
  const restored = [base, ...replayCheckpointState(replay.snapshot()).map((event, index) => ({ sessionId: SESSION, sequence: index + 2, event }))];
  assert.deepEqual(fold(restored).usage, fold(replay.snapshot()).usage);
  assert.deepEqual(fold(restored).plan, fold(replay.snapshot()).plan);
  assert.deepEqual(fold(restored).modStatuses, fold(replay.snapshot()).modStatuses);
  emit({ type: 'compaction_status', phase: 'complete' });
  assert.equal(replayCheckpointState(replay.snapshot()).some(event => event.type === 'usage_update'), false);
});

type Resumed = Extract<ClientEvent, { type: 'session_resumed' }>;
const BOUNDARY = '55555555-6666-4777-8888-999999999999';
function sourceCheckpoint(boundary = BOUNDARY): Resumed {
  return { type: 'session_resumed', session_id: SESSION, mode: 'code', messages: [
    { role: 'system', transcript_uuid: boundary, blocks: [
      { type: 'compact_boundary', messages_before: 100, messages_after: 3, summary: 'source summary' },
    ] },
    { role: 'user', transcript_uuid: '66666666-7777-4888-8999-aaaaaaaaaaaa', blocks: [{ type: 'text', text: 'persisted prompt' }] },
    { role: 'assistant', transcript_uuid: '77777777-8888-4999-8aaa-bbbbbbbbbbbb', blocks: [
      { type: 'text', text: 'persisted response' },
      { type: 'tool_use', id: 'finished-tool', tool: 'Read', input_json: '{"path":"file"}', header: HEADER },
      { type: 'tool_result', id: 'finished-tool', tool: 'Read', result_json: '{"done":true}', is_error: false, display: DISPLAY },
    ] },
  ] };
}

class CheckpointClient extends EventEmitter {
  readonly captures: Array<{ resolve(event: Resumed): void; reject(error: Error): void }> = [];
  sendCommand(_command: unknown): void {}
  sendPrompt(_text: string, _options: unknown): void {}
  close(): void {}
  captureTranscriptCheckpoint(sessionId: string): Promise<Resumed> {
    assert.equal(sessionId, SESSION);
    return new Promise((resolve, reject) => { this.captures.push({ resolve, reject }); });
  }
}

function checkpointRuntime(t: TestContext) {
  const runtime = new SessionRuntime({ sessionId: SESSION, projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId: SESSION, trusted: true }) });
  t.after(() => runtime.dispose());
  const client = new CheckpointClient();
  const internal = runtime as any;
  internal.generation = 1;
  internal.desiredRunning = true;
  internal.client = client;
  internal.activeWorkspace = '/workspace';
  internal.activeWorkspaceTrusted = true;
  internal.state = { status: 'connected' };
  internal.wireClient(client, 1);
  const delivered: ClientEvent[] = [];
  internal.broadcast = (_channel: string, event: ClientEvent) => delivered.push(event);
  const emit = (event: ClientEvent) => client.emit('event', event);
  emit({ type: 'session_started', session_id: SESSION, mode: 'code' });
  return { runtime, client, internal, emit, delivered };
}
const flush = () => new Promise<void>(resolve => setImmediate(resolve));

test('a quiet source checkpoint retires old spans without changing live renderer, journals or source identities', async t => {
  const { runtime, client, internal, emit, delivered } = checkpointRuntime(t);
  runtime.sendPrompt('persisted prompt', [], 1);
  emit({ type: 'turn_started', turn_id: 1 });
  for (let i = 0; i < 1_000; i++) emit({ type: 'text_delta', text: 'old content' });
  emit({ type: 'message_complete' });
  emit({ type: 'plan_updated', tasks: [{ id: 'plan', subject: 'continue', state: 'in_progress' }] });
  internal.recordInteraction({ type: 'permission_request', request: permissionRequest(7) });
  const prompts = runtime.remoteView().prompts;
  const interactions = runtime.remoteView().interactions;
  const before = runtime.remoteView();
  const channel = new RecordingChannel();
  const clock = newClock();
  const attachment = new SessionAttachment({ runtime, channel, clock });
  t.after(() => attachment.close());
  attachment.attach(undefined);
  channel.clear();
  emit({ type: 'compaction_status', phase: 'complete' });
  await flush();
  assert.equal(client.captures.length, 0, 'compaction alone cannot checkpoint an active turn');
  emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  await flush();
  assert.equal(client.captures.length, 1);
  const deliveredBeforeCheckpoint = [...delivered];
  const source = sourceCheckpoint();
  client.captures[0]!.resolve(source);
  await flush();
  const after = runtime.remoteView();
  assert.notEqual(after.epoch, before.epoch);
  assert.equal(canResume(after, { epoch: before.epoch, after_seq: before.lastSequence }), false);
  assert.deepEqual(delivered, deliveredBeforeCheckpoint, 'existing local renderer receives no replacement baseline');
  assert.equal(after.prompts, prompts, 'source refresh preserves prompt/image journal ownership');
  assert.equal(after.interactions, interactions, 'source refresh preserves interaction history');
  assert.deepEqual(after.base?.event, source, 'persisted UUIDs and derived tool metadata remain exact');
  assert.ok(runtime.replaySnapshot().length < 10, 'the real source replaces pre-compact stream history');
  assert.deepEqual(fold(runtime.replaySnapshot()).plan, [{ id: 'plan', subject: 'continue', state: 'in_progress' }]);
  assert.deepEqual(initialHistory(source.messages).before, { uuid: BOUNDARY });
  clock.advance(LIVE_BATCH_MS);
  assert.equal(channel.of('session.reset').length, 1);
  assert.equal(channel.of('session.snapshot').find(frame => frame.phase === 'begin')?.epoch, after.epoch);
  for (let compact = 1; compact < 3; compact++) {
    emit({ type: 'turn_started', turn_id: compact + 10 });
    for (let i = 0; i < 10_000; i++) {
      emit({ type: 'text_delta', text: `round-${compact};` });
      emit({ type: 'thinking_delta', thinking: 'distinct reasoning block' });
    }
    emit({ type: 'message_complete' });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
    emit({ type: 'compaction_status', phase: 'complete' });
    await flush();
    assert.equal(client.captures.length, compact + 1);
    const oldFrames = runtime.replaySnapshot().length;
    assert.ok(oldFrames > 20_000, 'distinct alternating blocks stay intact until a real source checkpoint exists');
    const nextSource = sourceCheckpoint(`55555555-6666-4777-8888-99999999999${compact}`);
    const localCount = delivered.length;
    client.captures[compact]!.resolve(nextSource);
    await flush();
    assert.deepEqual(runtime.remoteView().base?.event, nextSource);
    assert.ok(runtime.replaySnapshot().length < 10, 'each successful source checkpoint releases the preceding compact prefix');
    assert.equal(delivered.length, localCount);
    assert.equal(runtime.remoteView().prompts, prompts);
    assert.equal(runtime.remoteView().interactions, interactions);
    t.diagnostic(`source checkpoint ${compact + 1}: ${oldFrames} retained frames -> ${runtime.replaySnapshot().length}`);
  }
});

test('unconsumed queued prompts prevent refresh until their own turn has started and ended', async t => {
  const { runtime, client, emit } = checkpointRuntime(t);
  runtime.sendPrompt('first prompt', [], 1);
  emit({ type: 'turn_started', turn_id: 1 });
  runtime.sendPrompt('queued prompt', [], 2);
  emit({ type: 'compaction_status', phase: 'complete' });
  emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  await flush();
  assert.equal(client.captures.length, 0, 'an idle gap before a queued turn is not a safe baseline');
  assert.deepEqual(runtime.remoteView().prompts.map(prompt => prompt.text), ['first prompt', 'queued prompt']);
  emit({ type: 'turn_started', turn_id: 2 });
  emit({ type: 'text_delta', text: 'queue output' });
  emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  await flush();
  assert.equal(client.captures.length, 1);
  client.captures[0]!.resolve(sourceCheckpoint());
  await flush();
  assert.equal(runtime.replaySnapshot()[0]?.event.type, 'session_resumed');
  assert.equal(runtime.remoteView().prompts.length, 2);
});

test('pending interactions defer capture and a prompt racing a capture prevents its installation', async t => {
  const { runtime, client, internal, emit } = checkpointRuntime(t);
  internal.pendingPermissionIds.set(7, { ...permissionRequest(7), backgroundOwned: true });
  emit({ type: 'compaction_status', phase: 'complete' });
  await flush();
  assert.equal(client.captures.length, 0);
  assert.equal(runtime.remotePending().permissions[0]?.request_id, 7);
  emit({ type: 'permission_request_resolved', request_id: 7, resolution: 'approved' });
  await flush();
  assert.equal(client.captures.length, 1);
  const prior = runtime.replaySnapshot();
  runtime.sendPrompt('arrived during source I/O', [], 9);
  client.captures[0]!.resolve(sourceCheckpoint());
  await flush();
  assert.deepEqual(runtime.replaySnapshot(), prior, 'an in-flight source result never hides a new prompt');
  assert.equal(runtime.remoteView().prompts.at(-1)?.text, 'arrived during source I/O');
  assert.equal(runtime.turnActive, true);
  emit({ type: 'turn_started', turn_id: 9 });
  emit({ type: 'turn_ended', outcome: { type: 'cancelled' }, cost: COST });
  await flush();
  assert.equal(client.captures.length, 2, 'a later quiet boundary retries the requested checkpoint');
  client.captures[1]!.resolve(sourceCheckpoint());
  await flush();
  assert.equal(runtime.replaySnapshot()[0]?.event.type, 'session_resumed');
});

test('failed or stale owner captures leave spans usable and do not spin on the same sequence', async t => {
  const { runtime, client, internal, emit } = checkpointRuntime(t);
  emit({ type: 'compaction_status', phase: 'complete' });
  await flush();
  const original = runtime.replaySnapshot();
  client.captures[0]!.reject(new Error('committed source is busy'));
  await flush();
  internal.notifyActivityChanged();
  await flush();
  assert.equal(client.captures.length, 1);
  assert.deepEqual(runtime.replaySnapshot(), original);
  emit({ type: 'system_notice', message: 'another stable boundary', is_error: false });
  await flush();
  assert.equal(client.captures.length, 2);
  internal.generation += 1;
  const replacement = new CheckpointClient();
  internal.client = replacement;
  client.captures[1]!.resolve(sourceCheckpoint());
  await flush();
  assert.equal(runtime.replaySnapshot()[0]?.event.type, 'session_started', 'a new native owner invalidates the old capture');
  assert.equal(replacement.captures.length, 1, 'an owner change permits a fresh capture at the same event sequence');
  replacement.captures[0]!.reject(new Error('source unavailable'));
  await flush();
  assert.equal(replacement.captures.length, 1);
});
