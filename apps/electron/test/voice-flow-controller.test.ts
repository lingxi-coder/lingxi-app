import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEFAULT_VOICE_FLOW_STATE,
  VoiceFlowController,
  type VoiceFlowTrackedSpeechEvent,
  type VoiceFlowTurnToken,
} from '../src/renderer/audio/flow/controller';
import { audioConfigurationDefaults, type AudioConfigurationV3 } from '../src/shared/generatedAudioConfiguration';
import type { AudioOperationDto, AudioOperationResultDto } from '@lingxi/bridge-client';
import { canvasColorWithAlpha } from '../src/renderer/components/voice/VoiceOrbCanvas';
import { shouldAutoplayTrackedReply } from '../src/renderer/audio/autoplay';

function flush(): Promise<void> { return new Promise((resolve) => setImmediate(resolve)); }

test('ordinary autoplay rejects hidden or stale-session completions', () => {
  assert.equal(shouldAutoplayTrackedReply('session-1', 'session-1', false), true);
  assert.equal(shouldAutoplayTrackedReply('session-2', 'session-1', false), false);
  assert.equal(shouldAutoplayTrackedReply('session-1', 'session-1', true), false);
});

test('the flow orb adds alpha without corrupting modern theme colors', () => {
  assert.equal(canvasColorWithAlpha('oklch(72% 0.18 268)', 0.27), 'oklch(72% 0.18 268 / 0.27)');
  assert.equal(canvasColorWithAlpha('oklch(72% 0.18 268 / 0.8)', 0.2), 'oklch(72% 0.18 268 / 0.2)');
  assert.equal(canvasColorWithAlpha('#336699', 0.5), '#33669980');
});

class FakeTimers {
  private now = 0;
  private nextId = 1;
  private tasks = new Map<number, { at: number; callback: () => void }>();
  setTimeout(callback: () => void, delayMs: number): number {
    const id = this.nextId++;
    this.tasks.set(id, { at: this.now + delayMs, callback });
    return id;
  }
  clearTimeout(handle: unknown): void { if (typeof handle === 'number') this.tasks.delete(handle); }
  advance(delayMs: number): void {
    const target = this.now + delayMs;
    for (;;) {
      const next = [...this.tasks.entries()].filter(([, task]) => task.at <= target)
        .sort((left, right) => left[1].at - right[1].at)[0];
      if (!next) break;
      const [id, task] = next;
      this.tasks.delete(id);
      this.now = task.at;
      task.callback();
    }
    this.now = target;
  }
}

class FakeAudio {
  readonly operations: Array<{ operation: AudioOperationDto; revision: number }> = [];
  readonly cancellations: number[] = [];
  readonly finishRequests: number[] = [];
  private readonly results: Array<AudioOperationResultDto | Promise<AudioOperationResultDto>> = [];
  finishResult: Promise<void> | null = null;
  queue(...results: Array<AudioOperationResultDto | Promise<AudioOperationResultDto>>): void { this.results.push(...results); }
  async execute(operation: AudioOperationDto, revision: number): Promise<{ result: AudioOperationResultDto }> {
    this.operations.push({ operation, revision });
    const queued = this.results.shift();
    if (!queued) throw new Error(`missing audio result for ${operation.type}`);
    const result = await queued;
    return { result };
  }
  async cancel(): Promise<void> { this.cancellations.push(1); }
  async finishListen(): Promise<void> {
    this.finishRequests.push(1);
    if (this.finishResult) await this.finishResult;
  }
}

class FakeBridge {
  private sequence = 0;
  private readonly listeners = new Map<string, Set<(event: VoiceFlowTrackedSpeechEvent) => void>>();
  readonly sent: Array<{ text: string; token: VoiceFlowTurnToken }> = [];
  readonly cancelCalls: Array<number | undefined> = [];
  readonly cancelledTokens: VoiceFlowTurnToken[] = [];
  private readonly turnIds = new Map<string, number>();
  sendTrackedPrompt(text: string, _images = [], _imageNames = [], _filePaths = [], options: { purpose?: 'composer' | 'flow' } = {}) {
    this.sequence += 1;
    const token: VoiceFlowTurnToken = { sessionId: 'session-1', clientTurnId: `turn-${this.sequence}`, purpose: options.purpose ?? 'composer' };
    this.sent.push({ text, token });
    return { token, queued: Promise.resolve() };
  }
  subscribeTrackedSpeech(token: VoiceFlowTurnToken, listener: (event: VoiceFlowTrackedSpeechEvent) => void): () => void {
    const current = this.listeners.get(token.clientTurnId) ?? new Set();
    current.add(listener);
    this.listeners.set(token.clientTurnId, current);
    return () => {
      const next = this.listeners.get(token.clientTurnId);
      if (!next) return;
      next.delete(listener);
      if (next.size === 0) this.listeners.delete(token.clientTurnId);
    };
  }
  async cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void> {
    this.cancelledTokens.push(token);
    this.cancelCalls.push(token.turnId ?? this.turnIds.get(token.clientTurnId));
  }
  emit(token: VoiceFlowTurnToken, event: Omit<VoiceFlowTrackedSpeechEvent, 'token'>): void {
    if (event.turnId !== undefined) this.turnIds.set(token.clientTurnId, event.turnId);
    for (const listener of this.listeners.get(token.clientTurnId) ?? []) listener({ ...event, token });
  }
}

function preferences(revision: number, configuration = audioConfigurationDefaults()) {
  return { configuration, revision };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise; });
  return { promise, resolve };
}

test('Flow pins the saved audio revision through each spoken reply and refreshes on the next listen', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  let current = preferences(4, {
    ...audioConfigurationDefaults(),
    language: 'zh-CN',
    speech: { source: 'system', offlineModelId: null, voice: { source: 'system', id: 'Tingting' } },
  });
  audio.queue(
    { type: 'transcript', text: '总结一下这个改动' },
    { type: 'playback_completed', duration_ms: 800 },
    { type: 'transcript', text: '再问一个问题' },
  );
  const states: string[] = [];
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => current,
    timers,
    onStateChange: (state) => states.push(`${state.phase}:${state.detail}`),
  });

  await controller.start();
  assert.equal(audio.operations[0]?.operation.type, 'listen');
  assert.equal(audio.operations[0]?.revision, 4);
  assert.equal(bridge.sent[0]?.text, '总结一下这个改动');
  const token = bridge.sent[0]!.token;

  current = preferences(5, { ...current.configuration, rate: 1.5 });
  bridge.emit(token, { type: 'message', text: '第一句已经到了。', turnId: 77 });
  await flush();
  assert.deepEqual(audio.operations[1], {
    operation: {
      type: 'speak', text: '第一句已经到了。', language: 'zh-CN', rate: 1,
      voice: 'system:Tingting',
    },
    revision: 4,
  });
  bridge.emit(token, { type: 'completion', text: '第一句已经到了。', turnId: 77 });
  timers.advance(350);
  await flush();

  assert.equal(audio.operations[2]?.operation.type, 'listen');
  assert.equal(audio.operations[2]?.revision, 5);
  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['总结一下这个改动', '再问一个问题']);
  assert.ok(states.some((entry) => entry.startsWith('thinking:总结一下这个改动')));
  controller.dispose();
});

test('a model-missing Listen fails honestly and does not manufacture a transcript', async () => {
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  audio.queue({ type: 'failed', error: { kind: 'model_missing', message: 'chosen model is not installed' } });
  const states: string[] = [];
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => preferences(0),
    timers: new FakeTimers(),
    onStateChange: (state) => states.push(state.phase),
  });
  await controller.start();
  assert.equal(controller.getState().phase, 'configurationRequired');
  assert.deepEqual(bridge.sent, []);
  assert.ok(states.includes('requestingPermission'));
  controller.dispose();
});

test('repeated Flow retry during an active listen does not start a competing capture', async () => {
  const audio = new FakeAudio();
  const listen = deferred<AudioOperationResultDto>();
  audio.queue(listen.promise);
  const controller = new VoiceFlowController({
    audio,
    bridge: new FakeBridge(),
    getPreferences: () => preferences(3),
    timers: new FakeTimers(),
    onStateChange: () => {},
  });

  const start = controller.start();
  await controller.retry();
  assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen']);
  assert.equal(controller.getState().phase, 'listening');
  listen.resolve({ type: 'failed', error: { kind: 'cancelled', message: 'test complete' } });
  await start;
  controller.dispose();
});

test('ending Flow listening waits for and sends the finalized in-flight transcript', async () => {
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  const listen = deferred<AudioOperationResultDto>();
  audio.queue(listen.promise);
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => preferences(3),
    timers: new FakeTimers(),
    onStateChange: () => {},
  });

  const start = controller.start();
  await flush();
  assert.equal(controller.getState().phase, 'listening');
  await controller.orb();

  assert.equal(controller.getState().phase, 'recognizing');
  assert.deepEqual(audio.finishRequests, [1]);
  assert.deepEqual(audio.cancellations, []);
  assert.deepEqual(bridge.sent, []);

  listen.resolve({ type: 'transcript', text: '请总结刚才的讨论' });
  await start;
  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['请总结刚才的讨论']);
  assert.equal(controller.getState().phase, 'thinking');
  controller.dispose();
});

test('a stale finish-listen response cannot revive Flow after stop', async () => {
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  const listen = deferred<AudioOperationResultDto>();
  const finish = deferred<void>();
  audio.queue(listen.promise);
  audio.finishResult = finish.promise;
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => preferences(3),
    timers: new FakeTimers(),
    onStateChange: () => {},
  });

  const start = controller.start();
  await flush();
  const finishTap = controller.orb();
  await flush();
  assert.deepEqual(audio.finishRequests, [1]);
  await controller.stop();
  finish.resolve();
  await finishTap;
  listen.resolve({ type: 'transcript', text: '停止之后的旧识别结果' });
  await start;

  assert.equal(controller.getState().phase, 'paused');
  assert.deepEqual(bridge.sent, []);
  controller.dispose();
});

test('interrupting cancels current UI audio and sends only the replacement prompt to the active turn', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  audio.queue(
    { type: 'transcript', text: '原问题' },
    { type: 'playback_completed', duration_ms: 500 },
    { type: 'transcript', text: '替代问题' },
  );
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => preferences(8),
    timers,
    onStateChange: () => {},
  });
  await controller.start();
  const originalToken = bridge.sent[0]!.token;
  bridge.emit(originalToken, { type: 'message', text: '先回答这个。', turnId: 91 });
  await flush();
  await controller.orb();
  await flush();

  assert.deepEqual(bridge.cancelCalls, [91]);
  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['原问题', '替代问题']);
  assert.ok(audio.cancellations.length > 0);
  controller.dispose();
});

test('old reply completion does not start another listen during interruption', async () => {
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  const timers = new FakeTimers();
  const interruption = deferred<AudioOperationResultDto>();
  audio.queue(
    { type: 'transcript', text: '原问题' },
    { type: 'playback_completed', duration_ms: 500 },
    interruption.promise,
  );
  const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
  await controller.start();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'message', text: '这句话已经说完了。', turnId: 91 });
  await flush();
  const interrupt = controller.orb();
  await flush();
  bridge.emit(token, { type: 'completion', terminal: 'turn_ended', text: '这句话已经说完了。', turnId: 91 });
  timers.advance(350);
  await flush();
  assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen', 'speak', 'listen']);
  assert.equal(controller.getState().phase, 'interrupting');
  interruption.resolve({ type: 'transcript', text: '替代问题' });
  await interrupt;
  assert.deepEqual(bridge.sent.map(({ text }) => text), ['原问题', '替代问题']);
  assert.deepEqual(bridge.cancelCalls, [], 'the matching terminal already released the old backend owner');
  controller.dispose();
});

test('a delayed stop cannot pause a restarted Flow or discard its result', async () => {
  const audio = new FakeAudio(), bridge = new FakeBridge(), timers = new FakeTimers();
  const cancellation = deferred<void>();
  audio.cancel = () => cancellation.promise;
  const previous = deferred<AudioOperationResultDto>();
  const current = deferred<AudioOperationResultDto>();
  audio.queue(previous.promise, current.promise);
  const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
  const oldStart = controller.start();
  const stopped = controller.stop();
  const newStart = controller.start();
  cancellation.resolve();
  await stopped;
  assert.equal(controller.getState().phase, 'listening');
  previous.resolve({ type: 'failed', error: { kind: 'cancelled', message: 'stopped' } });
  current.resolve({ type: 'transcript', text: 'new question' });
  await Promise.all([oldStart, newStart]);
  assert.deepEqual(bridge.sent.map(({ text }) => text), ['new question']);
  assert.equal(controller.getState().phase, 'thinking');
  controller.dispose();
});

for (const failure of ['native_failure', 'model_missing', 'invalid_result', 'exception'] as const) {
  test(`Flow stays stopped after ${failure} until explicit retry`, async () => {
    const audio = new FakeAudio();
    const bridge = new FakeBridge();
    const timers = new FakeTimers();
    audio.queue({ type: 'transcript', text: 'first question' });
    const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
    await controller.start();
    const oldToken = bridge.sent[0]!.token;
    audio.queue(failure === 'exception'
      ? Promise.reject(new Error('playback rejected'))
      : failure === 'invalid_result'
        ? { type: 'transcript', text: 'unexpected' }
        : { type: 'failed', error: { kind: failure, message: 'playback failed' } });
    bridge.emit(oldToken, { type: 'message', text: '这是回复第一句。这是已经排队的第二句。', turnId: 91 });
    await flush();
    const failedState = controller.getState();
    assert.equal(failedState.phase, failure === 'model_missing' ? 'configurationRequired' : 'failed');
    bridge.emit(oldToken, { type: 'message', text: '这是迟到的第三句。', turnId: 91 });
    bridge.emit(oldToken, { type: 'completion', text: '这是回复第一句。这是已经排队的第二句。这是迟到的第三句。', turnId: 91 });
    timers.advance(1000);
    await flush();
    assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen', 'speak']);
    assert.deepEqual(controller.getState(), failedState);

    audio.queue({ type: 'transcript', text: 'retry question' }, { type: 'playback_completed', duration_ms: 1 });
    await controller.retry();
    bridge.emit(oldToken, { type: 'message', text: '重试后的旧回复。', turnId: 91 });
    bridge.emit(bridge.sent[1]!.token, { type: 'message', text: '这是重试后的新回复。', turnId: 92 });
    await flush();
    assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen', 'speak', 'listen', 'speak']);
    assert.deepEqual(bridge.sent.map(({ text }) => text), ['first question', 'retry question']);
    assert.equal(controller.getState().phase, 'thinking');
    controller.dispose();
  });
}

test('TTS failure cancels its captured backend owner before Flow Stop or Retry can lose it', async () => {
  const audio = new FakeAudio(), bridge = new FakeBridge(), timers = new FakeTimers();
  audio.queue({ type: 'transcript', text: 'first question' },
    { type: 'failed', error: { kind: 'native_failure', message: 'fake TTS failure' } });
  const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
  await controller.start();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'message', text: 'Accepted tool preamble.', turnId: 101 });
  await flush();
  assert.equal(controller.getState().phase, 'failed');
  assert.deepEqual(bridge.cancelledTokens, [token]);
  await controller.stop();
  assert.equal(bridge.cancelledTokens.length, 1);
  controller.dispose();
});

test('failed backend cancellation remains retryable and blocks a competing Flow prompt', async () => {
  class RejectingBridge extends FakeBridge {
    rejectsCancellation = true;
    override async cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void> {
      await super.cancelTrackedPrompt(token);
      if (this.rejectsCancellation) throw new Error('fake backend cancel rejected');
    }
  }
  const audio = new FakeAudio(), bridge = new RejectingBridge(), timers = new FakeTimers();
  audio.queue({ type: 'transcript', text: 'first question' },
    { type: 'failed', error: { kind: 'native_failure', message: 'fake TTS failure' } });
  const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
  await controller.start();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'message', text: 'Accepted preamble.', turnId: 102 });
  await flush();
  assert.match(controller.getState().detail, /backend cancel rejected/);
  await controller.stop();
  await controller.retry();
  assert.equal(bridge.sent.length, 1);
  assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen', 'speak']);
  assert.equal(bridge.cancelledTokens.length, 3);
  assert.ok(bridge.cancelledTokens.every((entry) => entry === token));
  bridge.rejectsCancellation = false;
  audio.queue({ type: 'transcript', text: 'safe retry question' });
  await controller.retry();
  assert.equal(bridge.cancelledTokens.length, 4);
  assert.deepEqual(bridge.sent.map(({ text }) => text), ['first question', 'safe retry question']);
  assert.equal(controller.getState().phase, 'thinking');
  controller.dispose();
});

test('an owned terminal after failed TTS releases cancellation without clobbering a newer prompt', async () => {
  let rejectCancel!: (cause: Error) => void;
  class DelayedCancelBridge extends FakeBridge {
    override async cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void> {
      await super.cancelTrackedPrompt(token);
      await new Promise<void>((_resolve, reject) => { rejectCancel = reject; });
    }
  }
  const audio = new FakeAudio(), bridge = new DelayedCancelBridge(), timers = new FakeTimers();
  audio.queue({ type: 'transcript', text: 'first question' },
    { type: 'failed', error: { kind: 'native_failure', message: 'fake TTS failure' } });
  const controller = new VoiceFlowController({ audio, bridge, timers, getPreferences: () => preferences(1), onStateChange: () => {} });
  await controller.start();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'message', text: 'Accepted preamble.', turnId: 103 });
  await flush();
  bridge.emit(token, { type: 'completion', terminal: 'turn_ended', text: 'Accepted preamble.', turnId: 103 });
  audio.queue({ type: 'transcript', text: 'new question' });
  await controller.retry();
  assert.equal(bridge.sent.length, 2);
  rejectCancel(new Error('late old cancel failure'));
  await flush();
  assert.equal(controller.getState().phase, 'thinking');
  assert.equal(controller.getState().detail, 'new question');
  controller.dispose();
  await flush();
  rejectCancel(new Error('test teardown'));
  await flush();
});

test('Stop invalidates Retry while the captured backend cancellation is still pending', async () => {
  const cancellation = deferred<void>();
  class DelayedCancelBridge extends FakeBridge {
    override async cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void> {
      await super.cancelTrackedPrompt(token);
      await cancellation.promise;
    }
  }
  const audio = new FakeAudio(), bridge = new DelayedCancelBridge();
  audio.queue({ type: 'transcript', text: 'first question' },
    { type: 'failed', error: { kind: 'native_failure', message: 'fake TTS failure' } });
  const controller = new VoiceFlowController({ audio, bridge, timers: new FakeTimers(),
    getPreferences: () => preferences(1), onStateChange: () => {} });
  await controller.start();
  bridge.emit(bridge.sent[0]!.token, { type: 'message', text: 'Accepted preamble.', turnId: 104 });
  await flush();
  const retry = controller.retry();
  const stop = controller.stop();
  cancellation.resolve();
  await Promise.all([retry, stop]);
  assert.equal(controller.getState().phase, 'paused');
  assert.deepEqual(audio.operations.map(({ operation }) => operation.type), ['listen', 'speak']);
  assert.equal(bridge.sent.length, 1);
  assert.equal(bridge.cancelledTokens.length, 1);
  controller.dispose();
});

test('dispose can retry a failed captured cancellation without publishing to a disposed view', async () => {
  class RejectingBridge extends FakeBridge {
    rejectsCancellation = true;
    override async cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void> {
      await super.cancelTrackedPrompt(token);
      if (this.rejectsCancellation) throw new Error('fake backend cancel rejected');
    }
  }
  const audio = new FakeAudio(), bridge = new RejectingBridge();
  audio.queue({ type: 'transcript', text: 'first question' },
    { type: 'failed', error: { kind: 'native_failure', message: 'fake TTS failure' } });
  const published: string[] = [];
  const controller = new VoiceFlowController({ audio, bridge, timers: new FakeTimers(),
    getPreferences: () => preferences(1), onStateChange: ({ phase }) => { published.push(phase); } });
  await controller.start();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'message', text: 'Accepted preamble.', turnId: 105 });
  await flush();
  const publishedBeforeDispose = published.length;
  controller.dispose();
  await flush();
  assert.equal(bridge.cancelledTokens.length, 2);
  bridge.rejectsCancellation = false;
  controller.dispose();
  await flush();
  controller.dispose();
  assert.equal(bridge.cancelledTokens.length, 3);
  assert.ok(bridge.cancelledTokens.every((entry) => entry === token));
  assert.equal(published.length, publishedBeforeDispose);
});

test('interrupt cancels a turn whose identity arrives during replacement listening', async () => {
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  const replacement = deferred<AudioOperationResultDto>();
  audio.queue({ type: 'transcript', text: 'first' }, replacement.promise);
  const controller = new VoiceFlowController({ audio, bridge, getPreferences: () => preferences(0),
    timers: new FakeTimers(), onStateChange: () => {} });
  await controller.start();
  const token = bridge.sent[0]!.token;
  const interruption = controller.orb();
  await flush();
  bridge.emit(token, { type: 'message', text: '原回答现在才开始。', turnId: 77 });
  replacement.resolve({ type: 'transcript', text: 'replacement' });
  await interruption;
  assert.deepEqual(bridge.cancelCalls, [77]);
  assert.deepEqual(bridge.sent.map(({ text }) => text), ['first', 'replacement']);
  controller.dispose();
});
