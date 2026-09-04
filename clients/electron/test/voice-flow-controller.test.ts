import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEFAULT_VOICE_FLOW_STATE,
  VoiceFlowController,
  type VoiceFlowTrackedSpeechEvent,
  type VoiceFlowTurnToken,
} from '../src/renderer/audio/flow/controller';
import {
  defaultNativeAudioSnapshot,
  type NativeAudioCommand,
  type NativeAudioEvent,
  type NativeAudioOwner,
  type NativeAudioResponse,
} from '../src/shared/nativeAudio';
import { canvasColorWithAlpha } from '../src/renderer/components/voice/VoiceOrbCanvas';
import { shouldAutoplayTrackedReply } from '../src/renderer/audio/autoplay';

function flush(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

test('ordinary autoplay rejects hidden or stale-session completions', () => {
  assert.equal(shouldAutoplayTrackedReply('session-1', 'session-1', false), true);
  assert.equal(shouldAutoplayTrackedReply('session-2', 'session-1', false), false);
  assert.equal(shouldAutoplayTrackedReply('session-1', 'session-1', true), false);
});

test('the flow orb adds alpha without corrupting modern theme colors', () => {
  assert.equal(
    canvasColorWithAlpha('oklch(72% 0.18 268)', 0.27),
    'oklch(72% 0.18 268 / 0.27)',
  );
  assert.equal(
    canvasColorWithAlpha('oklch(72% 0.18 268 / 0.8)', 0.2),
    'oklch(72% 0.18 268 / 0.2)',
  );
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

  clearTimeout(handle: unknown): void {
    if (typeof handle === 'number') this.tasks.delete(handle);
  }

  advance(delayMs: number): void {
    const target = this.now + delayMs;
    for (;;) {
      const next = [...this.tasks.entries()]
        .filter(([, task]) => task.at <= target)
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
  private readonly listeners = new Set<(event: NativeAudioEvent) => void>();
  readonly requests: NativeAudioCommand[] = [];
  readonly responses: Array<NativeAudioResponse | Promise<NativeAudioResponse>> = [];

  queue(...responses: Array<NativeAudioResponse | Promise<NativeAudioResponse>>): void {
    this.responses.push(...responses);
  }

  async request(command: NativeAudioCommand): Promise<NativeAudioResponse> {
    this.requests.push(command);
    const next = this.responses.shift();
    if (!next) throw new Error(`missing queued audio response for ${command.type}`);
    return await next;
  }

  onEvent(listener: (event: NativeAudioEvent) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  emit(event: NativeAudioEvent): void {
    for (const listener of this.listeners) listener(event);
  }
}

class FakeBridge {
  private sequence = 0;
  private readonly listeners = new Map<string, Set<(event: VoiceFlowTrackedSpeechEvent) => void>>();
  readonly sent: Array<{ text: string; token: VoiceFlowTurnToken }> = [];
  readonly cancelCalls: Array<number | undefined> = [];

  sendTrackedPrompt(text: string, _images = [], _imageNames = [], _filePaths = [], options: { purpose?: 'composer' | 'flow' } = {}) {
    this.sequence += 1;
    const token: VoiceFlowTurnToken = {
      sessionId: 'session-1',
      clientTurnId: `turn-${this.sequence}`,
      purpose: options.purpose ?? 'composer',
    };
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

  async cancel(turnId?: number): Promise<void> {
    this.cancelCalls.push(turnId);
  }

  emit(token: VoiceFlowTurnToken, event: Omit<VoiceFlowTrackedSpeechEvent, 'token'>): void {
    const listeners = this.listeners.get(token.clientTurnId);
    if (!listeners) return;
    for (const listener of listeners) listener({ ...event, token });
  }
}

function owner(kind: NativeAudioOwner['kind']): NativeAudioOwner {
  return { kind, id: `session-1:${kind}` };
}

function recognitionEvent(text: string, isFinal = false): NativeAudioEvent {
  return {
    type: 'recognition_state',
    snapshot: defaultNativeAudioSnapshot(),
    progress: {
      owner: owner('flow'),
      text,
      isFinal,
    },
  };
}

function speechEvent(state: 'starting' | 'speaking' | 'finished' | 'interrupted'): NativeAudioEvent {
  return {
    type: 'speech_state',
    snapshot: defaultNativeAudioSnapshot(),
    owner: owner('flow'),
    state,
  };
}

function listeningFinished(text: string): NativeAudioResponse {
  return {
    type: 'listening_finished',
    snapshot: defaultNativeAudioSnapshot(),
    transcript: { text },
  };
}

test('controller speaks the first natural sentence before completion and relistens after playback', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  const states: string[] = [];

  audio.queue(
    { type: 'authorization', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
    listeningFinished('总结一下这个改动'),
    { type: 'speaking_started', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
  );

  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => ({
      recognitionMode: 'automatic',
      language: 'zh-CN',
      voiceSelection: 'system:default',
      rate: 1,
    }),
    createOwner: owner,
    timers,
    onStateChange: (state) => {
      states.push(`${state.phase}:${state.detail}`);
    },
  });

  await controller.start();
  audio.emit(recognitionEvent('总结一下这个改动'));
  timers.advance(1_200);
  await flush();

  assert.equal(bridge.sent[0]?.text, '总结一下这个改动');
  const token = bridge.sent[0]!.token;

  bridge.emit(token, { type: 'delta', text: '第一句已经到了。', turnId: 77 });
  await flush();

  const speakRequest = audio.requests.find((request) => request.type === 'speak');
  assert.deepEqual(speakRequest, {
    type: 'speak',
    owner: owner('flow'),
    text: '第一句已经到了。',
    voiceId: 'system:default',
    rate: 1,
  });

  bridge.emit(token, { type: 'completion', text: '第一句已经到了。', turnId: 77 });
  audio.emit(speechEvent('finished'));
  timers.advance(350);
  await flush();

  assert.equal(audio.requests.at(-1)?.type, 'start_listening');
  assert.ok(states.some((entry) => entry.startsWith('thinking:总结一下这个改动')));
  controller.dispose();
});

test('helper-driven final recognition sends once without a second finish request', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();
  audio.queue(
    { type: 'authorization', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
  );
  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => ({ recognitionMode: 'localOnly', language: 'en-US', voiceSelection: 'system:default', rate: 1 }),
    createOwner: owner,
    timers,
    onStateChange: () => {},
  });

  await controller.start();
  audio.emit(recognitionEvent('offline final transcript', true));
  await flush();

  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['offline final transcript']);
  assert.equal(audio.requests.filter((request) => request.type === 'finish_listening').length, 0);
  controller.dispose();
});

test('controller interrupt cancels only the owned turn and sends the replacement prompt', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();

  audio.queue(
    { type: 'authorization', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
    listeningFinished('原问题'),
    { type: 'speaking_started', snapshot: defaultNativeAudioSnapshot() },
    { type: 'speaking_stopped', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
    listeningFinished('替代问题'),
  );

  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => ({
      recognitionMode: 'automatic',
      language: 'zh-CN',
      voiceSelection: 'system:default',
      rate: 1,
    }),
    createOwner: owner,
    timers,
    onStateChange: () => {},
  });

  await controller.start();
  audio.emit(recognitionEvent('原问题'));
  timers.advance(1_200);
  await flush();
  const originalToken = bridge.sent[0]!.token;
  bridge.emit(originalToken, { type: 'delta', text: '先回答这个。', turnId: 91 });
  await flush();

  await controller.orb();
  audio.emit(recognitionEvent('替代问题'));
  timers.advance(1_200);
  await flush();

  assert.deepEqual(bridge.cancelCalls, [91]);
  assert.deepEqual(bridge.sent.map((entry) => entry.text), ['原问题', '替代问题']);
  controller.dispose();
});

test('empty interrupt resumes paused speech instead of cancelling the turn', async () => {
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();

  audio.queue(
    { type: 'authorization', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
    listeningFinished('原问题'),
    { type: 'speaking_started', snapshot: defaultNativeAudioSnapshot() },
    { type: 'speaking_stopped', snapshot: defaultNativeAudioSnapshot() },
    { type: 'listening_started', snapshot: defaultNativeAudioSnapshot() },
    listeningFinished(''),
    { type: 'speaking_started', snapshot: defaultNativeAudioSnapshot() },
  );

  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => ({
      recognitionMode: 'automatic',
      language: 'zh-CN',
      voiceSelection: 'system:default',
      rate: 1,
    }),
    createOwner: owner,
    timers,
    onStateChange: () => {},
  });

  await controller.start();
  audio.emit(recognitionEvent('原问题'));
  timers.advance(1_200);
  await flush();
  const token = bridge.sent[0]!.token;
  bridge.emit(token, { type: 'delta', text: '继续这一句。', turnId: 92 });
  await flush();

  await controller.orb();
  await controller.orb();
  timers.advance(0);
  await flush();

  assert.equal(audio.requests.filter((request) => request.type === 'speak').length, 2);
  assert.deepEqual(bridge.cancelCalls, []);
  controller.dispose();
});

test('stale async permission completions are ignored after a newer generation stops the flow', async () => {
  let resolveAuthorization: ((value: NativeAudioResponse) => void) | null = null;
  const authorization = new Promise<NativeAudioResponse>((resolve) => {
    resolveAuthorization = resolve;
  });
  const timers = new FakeTimers();
  const audio = new FakeAudio();
  const bridge = new FakeBridge();

  audio.queue(
    authorization,
    { type: 'cancelled', snapshot: defaultNativeAudioSnapshot() },
    { type: 'speaking_stopped', snapshot: defaultNativeAudioSnapshot() },
  );

  const controller = new VoiceFlowController({
    audio,
    bridge,
    getPreferences: () => ({
      recognitionMode: 'automatic',
      language: 'zh-CN',
      voiceSelection: 'system:default',
      rate: 1,
    }),
    createOwner: owner,
    timers,
    onStateChange: () => {},
  });

  const startPromise = controller.start();
  await controller.stop();
  resolveAuthorization?.({ type: 'authorization', snapshot: defaultNativeAudioSnapshot() });
  await startPromise;

  assert.equal(audio.requests.some((request) => request.type === 'start_listening'), false);
  assert.equal(controller.getState().phase, DEFAULT_VOICE_FLOW_STATE.phase);
  assert.equal(controller.getState().detail, DEFAULT_VOICE_FLOW_STATE.detail);
  assert.ok(controller.getState().generation >= 2);
  controller.dispose();
});
