import { parseAudioUsageContext } from '../../shared/audioUsage.js';
import type { AudioUsageRecord } from '../../shared/audioUsage.js';
import { randomUUID } from 'node:crypto';
import type { AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import type { NativeAudioEvent } from '../../shared/nativeAudio.js';
import { MediaCaptureStopError, type MediaCaptureStartResult, type NativeAudioManager } from './nativeAudioManager.js';
import type { NativeRealtimeAudioState } from '../../shared/realtimeAudio.js';

export interface RealtimeAudioRuntime {
  sessionId: string;
  realtimeAudioSupported: boolean;
  onRealtimeAudioEvent(callback: (eventJson: string) => void): () => void;
  startRealtimeAudio(requestJson: string): void;
  realtimeAudioInput(inputJson: string): void;
  stopRealtimeAudio(): void;
}
interface AudioDelta { audioBase64: string; sampleRateHz: number; itemId?: string }
interface CaptureAttempt {
  runtime: RealtimeAudioRuntime;
  generation: number;
  start: Promise<MediaCaptureStartResult>;
}
interface InputCloseFlight {
  attempt?: CaptureAttempt;
  observed: Promise<void>;
  observers: number;
}
const INPUT_CLOSE_OBSERVER_MS = 30_000;
const INPUT_CLOSE_MAX_OBSERVERS = 64;

/** The authenticated current Agent owns provider realtime history, tools, and permissions. */
export class RealtimeAudioController {
  private generation = 0;
  private runtime?: RealtimeAudioRuntime;
  private operationId?: string;
  private offRuntime?: () => void;
  private offDevice?: () => void;
  private capturing = false;
  private captureAttempt?: CaptureAttempt;
  private inputClose?: InputCloseFlight;
  private inputFault?: Error;
  private inputRate = 24_000;
  private inputIdentity?: string;
  private inputSequence = -1;
  private queue: AudioDelta[] = [];
  private queuedBytes = 0;
  private playback?: Promise<void>;
  private completedTurn = false;
  private readonly playedItems = new Set<string>();
  private usageContext?: Omit<AudioUsageRecord, 'sequence' | 'usage' | 'operationId' | 'modelId'>;

  constructor(private readonly options: {
    audio: Pick<NativeAudioManager, 'onEvent' | 'startRealtimeCapture' | 'stopRealtimeCapture' | 'playRealtimeAudio'>;
    currentRuntime(): RealtimeAudioRuntime | undefined;
    publish(state: NativeRealtimeAudioState): void;
    recordUsage?(metadata: Omit<AudioUsageRecord, 'sequence' | 'usage'>, usage: unknown): void;
    inputCloseObserverMs?: number;
  }) {}

  private current(generation: number): boolean { return this.generation === generation && this.runtime === this.options.currentRuntime(); }
  private publish(phase: NativeRealtimeAudioState['phase'], detail: string) {
    this.options.publish({ phase, detail, generation: this.generation, ...(this.runtime ? { sessionId: this.runtime.sessionId } : {}) });
  }

  async start(configuration: AudioConfigurationV4, usageContext?: Omit<AudioUsageRecord, 'sequence' | 'usage' | 'operationId' | 'modelId'>): Promise<void> {
    const stopping = this.stop();
    const stoppedGeneration = this.generation;
    await stopping;
    if (this.generation !== stoppedGeneration) return;
    const runtime = this.options.currentRuntime();
    if (!runtime?.realtimeAudioSupported) throw new Error('当前 Agent 宿主尚未提供已验证的实时会话契约。');
    if (configuration.conversation.interaction !== 'turn_based') throw new Error('此设备尚未验证回声消除。请使用轮流说话模式。');
    this.runtime = runtime;
    const generation = ++this.generation;
    const frozen = structuredClone(configuration);
    this.operationId = randomUUID();
    this.usageContext = usageContext;
    this.offRuntime = runtime.onRealtimeAudioEvent((eventJson) => {
      if (!this.current(generation)) { if (generation === this.generation) void this.stop().catch(() => undefined); return; }
      try { this.receive(JSON.parse(eventJson), generation); }
      catch (error) { void this.fail(error instanceof Error ? error.message : '实时事件无效'); }
    });
    this.offDevice = this.options.audio.onEvent((event) => this.deviceEvent(event, generation));
    this.publish('requestingPermission', '连接当前 Agent 的实时音频服务…');
    runtime.startRealtimeAudio(JSON.stringify({ operationId: this.operationId, kind: 'realtime', cloud: frozen.conversation.cloud,
      language: frozen.language, rate: frozen.rate, interaction: frozen.conversation.interaction, ...(frozen.conversation.voice ? { voice: frozen.conversation.voice.id } : {}), maxPayloadBytes: 8 * 1024 * 1024, timeoutMs: 120_000 }));
  }

  private deviceEvent(event: NativeAudioEvent, generation: number) {
    if (!this.current(generation)) { if (generation === this.generation) void this.stop().catch(() => undefined); return; }
    if (event.type !== 'capture_chunk' || !this.capturing || event.owner.kind !== 'session' || event.owner.id !== this.runtime?.sessionId) return;
    if (event.sampleRateHz !== this.inputRate) { void this.fail('麦克风采样率与实时会话输入格式不一致。'); return; }
    const identity = `${event.identity.service_epoch}:${event.identity.generation}:${event.identity.id}`;
    if (this.inputIdentity === undefined) this.inputIdentity = identity;
    if (identity !== this.inputIdentity || event.sequence <= this.inputSequence) return;
    if (event.sequence !== this.inputSequence + 1) { void this.fail('实时输入序列中断，请重新开始。'); return; }
    this.inputSequence = event.sequence;
    try { this.runtime.realtimeAudioInput(JSON.stringify({ type: 'audio', operationId: this.operationId, audioBase64: event.pcmBase64 })); }
    catch (error) { void this.fail(error instanceof Error ? error.message : '实时音频输入发送失败'); }
  }

  private receive(event: Record<string, unknown>, generation: number) {
    if (event.operationId !== this.operationId || (event.sessionId !== undefined && event.sessionId !== this.runtime?.sessionId)) return;
    switch (event.type) {
      case 'usage':
        {
          const context = parseAudioUsageContext(event.usageContext);
          if (this.usageContext && context && context.operationId === this.operationId) this.options.recordUsage?.({ ...this.usageContext, ...context, ...(typeof event.turnId === 'string' ? { turnId: event.turnId } : {}) }, event);
        }
        break;
      case 'session_ready':
        {
          const format = event.inputFormat && typeof event.inputFormat === 'object' ? event.inputFormat as Record<string, unknown> : {};
          if (format.encoding !== 'pcm16' || format.channels !== 1 || typeof format.sampleRateHz !== 'number' || !Number.isInteger(format.sampleRateHz) || format.sampleRateHz < 8_000 || format.sampleRateHz > 768_000) throw new Error('实时音频输入格式不受支持。');
          this.inputRate = format.sampleRateHz;
        }
        void this.capture(generation);
        break;
      case 'audio_delta': {
        if (typeof event.audioBase64 !== 'string' || event.audioBase64.length > 2 * 1024 * 1024 || !/^[A-Za-z0-9+/]*={0,2}$/.test(event.audioBase64)
          || event.encoding !== 'pcm16' || event.channels !== 1 || typeof event.sampleRateHz !== 'number' || !Number.isInteger(event.sampleRateHz) || event.sampleRateHz < 8_000 || event.sampleRateHz > 768_000) throw new Error('实时音频输出格式无效。');
        const bytes = Math.floor(event.audioBase64.length * 3 / 4);
        if (this.queue.length >= 128 || this.queuedBytes + bytes > 8 * 1024 * 1024) throw new Error('实时音频输出队列已满，请重新开始。');
        this.queue.push({ audioBase64: event.audioBase64, sampleRateHz: event.sampleRateHz, ...(typeof event.itemId === 'string' ? { itemId: event.itemId } : {}) });
        this.queuedBytes += bytes;
        this.publish('speaking', '正在播放实时回复 · 轮流说话模式');
        this.drain(generation);
        break;
      }
      case 'turn_completed': this.completedTurn = true; this.drain(generation); break;
      case 'error': void this.fail(typeof event.message === 'string' ? event.message : '实时音频服务失败'); break;
      case 'closed': void this.stop().catch(() => undefined); break;
      default: break;
    }
  }

  private async capture(generation: number) {
    if (!this.current(generation) || this.capturing || this.captureAttempt || this.inputClose || this.inputFault) return;
    this.inputIdentity = undefined; this.inputSequence = -1; this.capturing = true;
    const runtime = this.runtime!;
    let resolveStart!: (result: MediaCaptureStartResult) => void;
    let rejectStart!: (error: unknown) => void;
    const start = new Promise<MediaCaptureStartResult>((resolve, reject) => { resolveStart = resolve; rejectStart = reject; });
    const attempt: CaptureAttempt = { runtime, generation, start };
    this.captureAttempt = attempt;
    try {
      void this.options.audio.startRealtimeCapture(runtime.sessionId, this.inputRate).then(resolveStart, rejectStart);
    } catch (error) {
      rejectStart(error);
    }
    try {
      const result = await start;
      if (!this.current(generation) || !this.capturing || this.captureAttempt !== attempt) return;
      if (result.type === 'failed' || result.type === 'failed_without_resource') {
        if (result.captureAdmission === 'never_admitted') { this.captureAttempt = undefined; this.capturing = false; }
        await this.fail(result.error.message); return;
      }
      if (result.type !== 'recording_started') { await this.fail('麦克风采集返回无效结果'); return; }
      this.publish('listening', '正在聆听 · 说完后轻点 Orb 发送');
    } catch (error) {
      if (this.current(generation) && this.capturing && this.captureAttempt === attempt) {
        await this.fail(error instanceof Error ? error.message : '麦克风采集失败');
      }
    }
  }

  // A caller's deadline is distinct from the cleanup task. The latter still
  // waits for a late start and sends at most one stop, even after a timeout.
  private closeInput(runtime: RealtimeAudioRuntime, attempt?: CaptureAttempt): Promise<void> {
    if (this.inputClose) {
      if (this.inputClose.attempt !== attempt) return Promise.reject(new Error('another input close is unresolved'));
      return this.observeInputClose(this.inputClose);
    }
    const cleanup = Promise.resolve().then(async () => {
      if (attempt) {
        const start = await attempt.start;
        if ((start.type === 'failed' || start.type === 'failed_without_resource') && start.captureAdmission === 'never_admitted') return;
        if (start.type !== 'recording_started') {
          throw new MediaCaptureStopError('capture admission and cleanup are unconfirmed', false, start.captureClosed);
        }
        await this.options.audio.stopRealtimeCapture(runtime.sessionId, start.handle);
      } else {
        await this.options.audio.stopRealtimeCapture(runtime.sessionId);
      }
    });
    const configured = this.options.inputCloseObserverMs ?? INPUT_CLOSE_OBSERVER_MS;
    const budget = Number.isFinite(configured) ? Math.max(1, Math.min(INPUT_CLOSE_OBSERVER_MS, configured)) : INPUT_CLOSE_OBSERVER_MS;
    let timer: ReturnType<typeof setTimeout>;
    const observed = new Promise<void>((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('audio capture stop remains unconfirmed')), budget);
      cleanup.then(() => { clearTimeout(timer); resolve(); }, (error: unknown) => { clearTimeout(timer); reject(error); });
    });
    const flight: InputCloseFlight = { attempt, observed, observers: 0 };
    this.inputClose = flight;
    // Exactly one continuation is installed on the close flight, regardless of
    // how many callers observe it. Failed/expired flights remain fenced.
    const retireConfirmed = () => {
      if (this.inputClose === flight) { this.inputClose = undefined; this.inputFault = undefined; }
      if (this.captureAttempt === attempt) this.captureAttempt = undefined;
    };
    void observed.then(retireConfirmed, (error: unknown) => {
      if (this.inputClose === flight) this.inputFault = error instanceof Error ? error : new Error(String(error));
      if (error instanceof MediaCaptureStopError) {
        if (error.cleanupConfirmed) retireConfirmed();
        else if (error.whenClosed) void error.whenClosed.then(retireConfirmed);
      }
    });
    // The observation deadline does not cancel the original cleanup. A later
    // verified close can unblock a new session without replaying this commit.
    void cleanup.then(retireConfirmed, (error: unknown) => {
      if (error instanceof MediaCaptureStopError) {
        if (error.cleanupConfirmed) retireConfirmed();
        else if (error.whenClosed) void error.whenClosed.then(retireConfirmed);
      }
    });
    return this.observeInputClose(flight);
  }

  private observeInputClose(flight: InputCloseFlight): Promise<void> {
    if (flight.observers >= INPUT_CLOSE_MAX_OBSERVERS) return Promise.reject(new Error('too many audio close observers'));
    flight.observers += 1;
    return flight.observed.then(
      () => { flight.observers -= 1; },
      (error: unknown) => { flight.observers -= 1; throw error; },
    );
  }

  async commit(): Promise<void> {
    const runtime = this.runtime;
    const attempt = this.captureAttempt;
    if (!runtime || !this.capturing || !attempt || attempt.runtime !== runtime) return;
    const generation = this.generation;
    // Close the input gate before the first await or native stop dispatch.
    this.capturing = false;
    try {
      await this.closeInput(runtime, attempt);
    } catch (error) {
      if (this.current(generation)) await this.fail(error instanceof Error ? error.message : '麦克风停止未确认');
      throw error;
    }
    if (!this.current(generation) || runtime !== this.runtime) return;
    runtime.realtimeAudioInput(JSON.stringify({ type: 'commit', operationId: this.operationId }));
    this.publish('thinking', '当前 Agent 正在生成实时回复…');
  }

  private drain(generation: number) {
    if (this.playback) return;
    const task = (async () => {
      while (this.current(generation) && this.queue.length) {
        const delta = this.queue.shift()!; this.queuedBytes -= Math.floor(delta.audioBase64.length * 3 / 4);
        const result = await this.options.audio.playRealtimeAudio(this.runtime!.sessionId, delta.audioBase64, delta.sampleRateHz);
        if (!this.current(generation)) return;
        if (result.type === 'failed' || result.type === 'failed_without_resource') throw new Error(result.error.message);
        if (delta.itemId) this.playedItems.add(delta.itemId);
      }
      if (this.current(generation) && this.completedTurn && !this.queue.length) {
        this.completedTurn = false;
        if (this.playedItems.size) for (const itemId of this.playedItems) this.runtime!.realtimeAudioInput(JSON.stringify({ type: 'playback_completed', operationId: this.operationId, itemId }));
        else this.runtime!.realtimeAudioInput(JSON.stringify({ type: 'playback_completed', operationId: this.operationId }));
        this.playedItems.clear();
        await this.capture(generation);
      }
    })();
    this.playback = task;
    void task.catch((error) => { if (this.current(generation)) void this.fail(error instanceof Error ? error.message : '实时播放失败'); }).finally(() => {
      if (this.playback === task) { this.playback = undefined; if (this.current(generation) && (this.queue.length || this.completedTurn)) this.drain(generation); }
    });
  }

  private async fail(message: string) {
    const stoppedGeneration = this.generation + 1;
    try { await this.stop(); } catch { /* The close flight remains fenced. */ }
    if (this.generation === stoppedGeneration) this.publish('failed', message);
  }

  async stop(): Promise<void> {
    const generation = ++this.generation;
    const attempt = this.captureAttempt;
    const playbackActive = this.playback !== undefined;
    const runtime = this.runtime; this.runtime = undefined; this.operationId = undefined;
    this.offRuntime?.(); this.offRuntime = undefined; this.offDevice?.(); this.offDevice = undefined;
    this.capturing = false; this.queue = []; this.queuedBytes = 0; this.completedTurn = false; this.playedItems.clear(); this.playback = undefined;
    if (runtime) {
      try { runtime.stopRealtimeAudio(); } catch { /* A disconnected runtime still needs device capture released. */ }
    }
    try {
      if (this.inputClose) await this.observeInputClose(this.inputClose);
      else if (attempt && runtime) await this.closeInput(runtime, attempt);
      else if (this.inputFault) throw this.inputFault;
      else if (runtime && playbackActive) await this.closeInput(runtime);
    } catch (error) {
      if (generation === this.generation) this.publish('failed', error instanceof Error ? error.message : '麦克风停止未确认');
      throw error;
    }
    if (generation === this.generation) this.publish('paused', '实时会话已停止');
  }
}
