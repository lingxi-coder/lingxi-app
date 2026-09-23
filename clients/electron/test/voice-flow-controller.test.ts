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
  async cancel(turnId?: number): Promise<void> { this.cancelCalls.push(turnId); }
  emit(token: VoiceFlowTurnToken, event: Omit<VoiceFlowTrackedSpeechEvent, 'token'>): void {
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
  bridge.emit(token, { type: 'delta', text: '第一句已经到了。', turnId: 77 });
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
  bridge.emit(originalToken, { type: 'delta', text: '先回答这个。', turnId: 91 });
  await flush();
  await controller.orb();
  await flush();

  assert.deepEqual(bridge.cancelCalls, [91]);
  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['原问题', '替代问题']);
  assert.ok(audio.cancellations.length > 0);
  controller.dispose();
});
