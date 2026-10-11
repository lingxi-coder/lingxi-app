import { parseAudioUsageContext } from '../../shared/audioUsage.js';
import type { AudioUsageRecord } from '../../shared/audioUsage.js';
import { randomUUID } from 'node:crypto';
import type { AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import type { NativeAudioEvent } from '../../shared/nativeAudio.js';
import type { NativeAudioManager } from './nativeAudioManager.js';
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

/** The authenticated current Agent owns provider realtime history, tools, and permissions. */
export class RealtimeAudioController {
  private generation = 0;
  private runtime?: RealtimeAudioRuntime;
  private operationId?: string;
  private offRuntime?: () => void;
  private offDevice?: () => void;
  private capturing = false;
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
      if (!this.current(generation)) { if (generation === this.generation) void this.stop(); return; }
      try { this.receive(JSON.parse(eventJson), generation); }
      catch (error) { void this.fail(error instanceof Error ? error.message : '实时事件无效'); }
    });
    this.offDevice = this.options.audio.onEvent((event) => this.deviceEvent(event, generation));
    this.publish('requestingPermission', '连接当前 Agent 的实时音频服务…');
    runtime.startRealtimeAudio(JSON.stringify({ operationId: this.operationId, kind: 'realtime', cloud: frozen.conversation.cloud,
      language: frozen.language, rate: frozen.rate, interaction: frozen.conversation.interaction, ...(frozen.conversation.voice ? { voice: frozen.conversation.voice.id } : {}), maxPayloadBytes: 8 * 1024 * 1024, timeoutMs: 120_000 }));
  }

  private deviceEvent(event: NativeAudioEvent, generation: number) {
    if (!this.current(generation)) { if (generation === this.generation) void this.stop(); return; }
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
      case 'closed': void this.stop(); break;
      default: break;
    }
  }

  private async capture(generation: number) {
    if (!this.current(generation) || this.capturing) return;
    this.inputIdentity = undefined; this.inputSequence = -1; this.capturing = true;
    try {
      const result = await this.options.audio.startRealtimeCapture(this.runtime!.sessionId, this.inputRate);
      if (!this.current(generation)) return;
      if (result.type === 'failed') { await this.fail(result.error.message); return; }
      this.publish('listening', '正在聆听 · 说完后轻点 Orb 发送');
    } catch (error) { if (this.current(generation)) await this.fail(error instanceof Error ? error.message : '麦克风采集失败'); }
  }

  async commit(): Promise<void> {
    const runtime = this.runtime;
    if (!runtime || !this.capturing) return;
    await this.options.audio.stopRealtimeCapture(runtime.sessionId);
    this.capturing = false;
    if (runtime !== this.runtime || runtime !== this.options.currentRuntime()) return;
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
        if (result.type === 'failed') throw new Error(result.error.message);
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

  private async fail(message: string) { const stoppedGeneration = this.generation + 1; await this.stop(); if (this.generation === stoppedGeneration) this.publish('failed', message); }

  async stop(): Promise<void> {
    const generation = ++this.generation;
    const runtime = this.runtime; this.runtime = undefined; this.operationId = undefined;
    this.offRuntime?.(); this.offRuntime = undefined; this.offDevice?.(); this.offDevice = undefined;
    this.capturing = false; this.queue = []; this.queuedBytes = 0; this.completedTurn = false; this.playedItems.clear(); this.playback = undefined;
    if (runtime) {
      try { runtime.stopRealtimeAudio(); } catch { /* A disconnected runtime still needs device capture released. */ }
      await this.options.audio.stopRealtimeCapture(runtime.sessionId).catch(() => undefined);
    }
    if (generation === this.generation) this.publish('paused', '实时会话已停止');
  }
}
