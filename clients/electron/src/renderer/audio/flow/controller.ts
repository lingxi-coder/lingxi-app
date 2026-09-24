import type { ImageRefDto, AudioOperationDto, AudioOperationResultDto } from '@lingxi/bridge-client';

import type { AudioConfigurationV3 } from '../../../shared/generatedAudioConfiguration.js';
import { resolveAudioLanguage } from '../../../shared/generatedAudioConfiguration.js';
import { StreamingSpeechSegmenter } from './segmenter.js';

export type VoiceFlowPhase =
  | 'requestingPermission'
  | 'configurationRequired'
  | 'listening'
  | 'recognizing'
  | 'thinking'
  | 'speaking'
  | 'interrupting'
  | 'paused'
  | 'failed';

export interface VoiceFlowState {
  phase: VoiceFlowPhase;
  detail: string;
  generation: number;
  activeTurnId?: number;
}

export const DEFAULT_VOICE_FLOW_STATE: VoiceFlowState = {
  phase: 'paused',
  detail: '轻点 Orb 开始聆听',
  generation: 0,
};

export interface VoiceFlowPreferences {
  configuration: AudioConfigurationV3;
  revision: number;
}

export interface VoiceFlowTurnToken {
  sessionId: string;
  clientTurnId: string;
  purpose: 'composer' | 'flow';
}

export interface VoiceFlowTrackedSpeechEvent {
  type: 'delta' | 'completion';
  token: VoiceFlowTurnToken;
  text: string;
  turnId?: number;
}

export interface VoiceFlowBridge {
  sendTrackedPrompt(
    text: string,
    images?: ImageRefDto[],
    imageNames?: string[],
    filePaths?: string[],
    options?: { purpose?: 'composer' | 'flow' },
  ): { token: VoiceFlowTurnToken; queued: Promise<void> } | null;
  subscribeTrackedSpeech(token: VoiceFlowTurnToken, listener: (event: VoiceFlowTrackedSpeechEvent) => void): () => void;
  cancel(turnId?: number): Promise<void>;
}

export interface VoiceFlowAudio {
  execute(operation: AudioOperationDto, configurationRevision: number): Promise<{ result: AudioOperationResultDto }>;
  finishListen(): Promise<void>;
  cancel(): Promise<void>;
}

export interface VoiceFlowTimers {
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface VoiceFlowControllerOptions {
  audio: VoiceFlowAudio;
  bridge: VoiceFlowBridge;
  getPreferences: () => VoiceFlowPreferences;
  timers: VoiceFlowTimers;
  onStateChange: (state: VoiceFlowState) => void;
}

interface InterruptContext {
  pausedSegments: string[];
  turnId?: number;
  previousPreferences: VoiceFlowPreferences | null;
}

function errorDetail(result: AudioOperationResultDto): string | null {
  return result.type === 'failed' ? `${result.error.kind}: ${result.error.message}` : null;
}

function requiresConfiguration(result: AudioOperationResultDto): boolean {
  if (result.type !== 'failed') return false;
  return ['permission_denied', 'model_missing', 'voice_missing', 'unavailable', 'unsupported', 'invalid_request'].includes(result.error.kind);
}

function voiceOverride(configuration: AudioConfigurationV3): string | undefined {
  const voice = configuration.speech.voice;
  if (!voice) return undefined;
  return voice.source === 'offline'
    ? `sherpa:${voice.modelId ?? configuration.speech.offlineModelId ?? ''}:${voice.id}`
    : voice.source === 'system' ? `system:${voice.id}` : undefined;
}

export class VoiceFlowController {
  private generation = 0;
  private state: VoiceFlowState = DEFAULT_VOICE_FLOW_STATE;
  private offTrackedSpeech: (() => void) | null = null;
  private relistenTimer: unknown = null;
  private currentToken: VoiceFlowTurnToken | null = null;
  private currentTurnId: number | undefined;
  private segmenter: StreamingSpeechSegmenter | null = null;
  private speechQueue: string[] = [];
  private currentSpeechSegment: string | null = null;
  private speechInFlight = false;
  private streamComplete = false;
  private interruptContext: InterruptContext | null = null;
  private listeningActive = false;
  private listeningFinalizeRequested = false;
  private turnPreferences: VoiceFlowPreferences | null = null;
  private disposed = false;

  constructor(private readonly options: VoiceFlowControllerOptions) {
    this.publish();
  }

  getState(): VoiceFlowState {
    return this.state;
  }

  async start(): Promise<void> {
    if (this.listeningActive || this.disposed) return;
    const generation = this.bumpGeneration();
    this.clearTimers();
    this.resetTurnState();
    this.update({ phase: 'requestingPermission', detail: '正在准备本机音频服务…' });
    await this.startListening(generation, false);
  }

  async stop(): Promise<void> {
    const generation = this.bumpGeneration();
    const turnId = this.currentTurnId;
    this.clearTimers();
    this.cleanupTrackedSpeech();
    this.listeningActive = false;
    try { await this.options.audio.cancel(); } catch { /* best-effort owner teardown */ }
    if (turnId !== undefined) {
      try { await this.options.bridge.cancel(turnId); } catch { /* the runtime may already be closed */ }
    }
    if (!this.isCurrent(generation)) return;
    this.resetTurnState();
    this.update({ phase: 'paused', detail: '轻点 Orb 开始聆听' });
  }

  async retry(): Promise<void> {
    await this.start();
  }

  async orb(): Promise<void> {
    switch (this.state.phase) {
      case 'listening':
      case 'recognizing':
      case 'interrupting':
        await this.finishListening(this.generation);
        return;
      case 'thinking':
      case 'speaking':
        await this.beginInterrupt();
        return;
      case 'paused':
        await this.start();
        return;
      case 'requestingPermission':
      case 'configurationRequired':
      case 'failed':
        await this.retry();
        return;
      default:
        return;
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    const turnId = this.currentTurnId;
    this.clearTimers();
    this.cleanupTrackedSpeech();
    this.resetTurnState();
    void this.options.audio.cancel().catch(() => undefined);
    if (turnId !== undefined) void this.options.bridge.cancel(turnId).catch(() => undefined);
  }

  private async startListening(generation: number, interrupting: boolean): Promise<void> {
    const preferences = this.options.getPreferences();
    this.turnPreferences = {
      configuration: structuredClone(preferences.configuration),
      revision: preferences.revision,
    };
    this.listeningActive = true;
    this.listeningFinalizeRequested = false;
    this.update({
      phase: interrupting ? 'interrupting' : 'listening',
      detail: interrupting ? '请说出新的问题…' : '正在聆听…',
    });
    let result: AudioOperationResultDto;
    try {
      ({ result } = await this.options.audio.execute({
        type: 'listen',
        language: resolveAudioLanguage(this.turnPreferences.configuration.language, typeof navigator !== 'undefined' ? navigator.language : 'en-US'),
      }, this.turnPreferences.revision));
    } catch (cause) {
      if (!this.isCurrent(generation)) return;
      this.listeningActive = false;
      this.listeningFinalizeRequested = false;
      this.fail(cause instanceof Error ? cause.message : '本机语音识别失败。');
      return;
    }
    if (!this.isCurrent(generation)) return;
    this.listeningActive = false;
    this.listeningFinalizeRequested = false;
    const detail = errorDetail(result);
    if (detail) {
      this.handleFailure(result, detail);
      return;
    }
    if (result.type !== 'transcript') {
      this.fail('本机语音服务返回了无效的识别结果。');
      return;
    }
    const transcript = result.text.trim();
    if (this.interruptContext) {
      if (transcript) await this.commitInterrupt(transcript);
      else this.resumeAfterEmptyInterrupt();
      return;
    }
    if (!transcript) {
      this.scheduleRelisten();
      return;
    }
    this.beginTrackedPrompt(transcript);
  }

  private async finishListening(generation: number): Promise<void> {
    if (!this.listeningActive || this.listeningFinalizeRequested || !this.isCurrent(generation)) return;
    this.listeningFinalizeRequested = true;
    this.update(this.interruptContext
      ? { phase: 'interrupting', detail: '正在等待插话识别完成…' }
      : { phase: 'recognizing', detail: '正在等待当前语音识别完成…' });
    try {
      await this.options.audio.finishListen();
    } catch (cause) {
      if (!this.isCurrent(generation) || !this.listeningActive || !this.listeningFinalizeRequested) return;
      this.listeningFinalizeRequested = false;
      const error = cause instanceof Error ? cause.message : '无法结束当前本机语音识别。';
      this.update(this.interruptContext
        ? { phase: 'interrupting', detail: `结束插话识别失败：${error}；仍在等待识别结果…` }
        : { phase: 'recognizing', detail: `结束识别失败：${error}；仍在等待识别结果…` });
    }
  }

  private beginTrackedPrompt(text: string): void {
    this.cleanupTrackedSpeech();
    this.segmenter = new StreamingSpeechSegmenter();
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.currentTurnId = undefined;
    const tracked = this.options.bridge.sendTrackedPrompt(text, [], [], [], { purpose: 'flow' });
    if (!tracked) {
      this.fail('无法发送当前心流问题。');
      return;
    }
    this.currentToken = tracked.token;
    const generation = this.generation;
    this.update({ phase: 'thinking', detail: text });
    this.offTrackedSpeech = this.options.bridge.subscribeTrackedSpeech(tracked.token, (event) => {
      this.handleTrackedSpeech(event, generation);
    });
    void tracked.queued.catch((cause: unknown) => {
      if (!this.isCurrent(generation)) return;
      this.fail(cause instanceof Error ? cause.message : '心流问题发送失败。');
    });
  }

  private handleTrackedSpeech(event: VoiceFlowTrackedSpeechEvent, generation: number): void {
    if (!this.isCurrent(generation) || !this.currentToken || event.token.clientTurnId !== this.currentToken.clientTurnId) return;
    if (event.turnId !== undefined) {
      this.currentTurnId = event.turnId;
      this.update({ activeTurnId: event.turnId });
    }
    if (!this.segmenter) this.segmenter = new StreamingSpeechSegmenter();
    const segments = event.type === 'delta'
      ? this.segmenter.append(event.text)
      : this.segmenter.finish(event.text);
    if (event.type === 'completion') this.streamComplete = true;
    if (segments.length > 0) {
      this.speechQueue.push(...segments);
      void this.maybeStartSpeaking(generation);
    } else if (event.type === 'completion' && !this.speechInFlight && !this.interruptContext) {
      this.scheduleRelisten();
    }
  }

  private async maybeStartSpeaking(generation: number): Promise<void> {
    if (!this.isCurrent(generation) || this.speechInFlight || this.speechQueue.length === 0 || this.interruptContext) return;
    const segment = this.speechQueue.shift()!;
    const preferences = this.turnPreferences ?? this.options.getPreferences();
    this.currentSpeechSegment = segment;
    this.speechInFlight = true;
    this.update({ phase: 'speaking', detail: segment });
    try {
      const { result } = await this.options.audio.execute({
        type: 'speak',
        text: segment,
        language: resolveAudioLanguage(preferences.configuration.language, typeof navigator !== 'undefined' ? navigator.language : 'en-US'),
        rate: preferences.configuration.rate,
        ...(voiceOverride(preferences.configuration) ? { voice: voiceOverride(preferences.configuration) } : {}),
      }, preferences.revision);
      if (!this.isCurrent(generation)) return;
      const detail = errorDetail(result);
      if (detail) {
        this.speechInFlight = false;
        this.currentSpeechSegment = null;
        if (this.interruptContext && result.type === 'failed' && result.error.kind === 'cancelled') return;
        this.handleFailure(result, detail);
        return;
      }
      if (result.type !== 'playback_completed') {
        this.speechInFlight = false;
        this.currentSpeechSegment = null;
        this.fail('本机语音服务未确认播放完成。');
        return;
      }
      this.speechInFlight = false;
      this.currentSpeechSegment = null;
      if (this.interruptContext) return;
      if (this.speechQueue.length > 0) void this.maybeStartSpeaking(generation);
      else if (this.streamComplete) this.scheduleRelisten();
      else this.update({ phase: 'thinking', detail: '正在继续生成回复…' });
    } catch (cause) {
      if (!this.isCurrent(generation)) return;
      this.speechInFlight = false;
      this.currentSpeechSegment = null;
      this.fail(cause instanceof Error ? cause.message : '语音播放失败。');
    }
  }

  private async beginInterrupt(): Promise<void> {
    if (this.interruptContext || (this.state.phase !== 'thinking' && this.state.phase !== 'speaking' && this.state.phase !== 'paused')) return;
    const generation = this.generation;
    this.options.timers.clearTimeout(this.relistenTimer);
    this.relistenTimer = null;
    this.interruptContext = {
      pausedSegments: [
        ...(this.currentSpeechSegment ? [this.currentSpeechSegment] : []),
        ...this.speechQueue,
      ],
      turnId: this.currentTurnId,
      previousPreferences: this.turnPreferences,
    };
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.update({ phase: 'interrupting', detail: '请说出新的问题…' });
    try { await this.options.audio.cancel(); } catch { /* pending playback may already be done */ }
    if (!this.isCurrent(generation)) return;
    await this.startListening(generation, true);
  }

  private async commitInterrupt(transcript: string): Promise<void> {
    const turnId = this.currentTurnId ?? this.interruptContext?.turnId;
    const replacementPreferences = this.turnPreferences;
    this.interruptContext = null;
    this.cleanupTrackedSpeech();
    this.resetTurnState();
    this.turnPreferences = replacementPreferences;
    const generation = this.bumpGeneration();
    if (turnId !== undefined) {
      try { await this.options.bridge.cancel(turnId); } catch { /* turn already ended */ }
      if (!this.isCurrent(generation)) return;
    }
    this.beginTrackedPrompt(transcript);
  }

  private resumeAfterEmptyInterrupt(): void {
    const paused = this.interruptContext;
    this.interruptContext = null;
    if (paused) this.turnPreferences = paused.previousPreferences;
    if (paused && paused.pausedSegments.length > 0) {
      this.speechQueue = [...paused.pausedSegments, ...this.speechQueue];
      this.update({ phase: 'paused', detail: '继续当前回复…' });
      const generation = this.generation;
      this.relistenTimer = this.options.timers.setTimeout(() => {
        void this.maybeStartSpeaking(generation);
      }, 0);
      return;
    }
    this.update({ phase: 'paused', detail: '没有检测到新的插话，重新开始聆听…' });
    this.scheduleRelisten();
  }

  private scheduleRelisten(): void {
    this.options.timers.clearTimeout(this.relistenTimer);
    const generation = this.generation;
    this.relistenTimer = this.options.timers.setTimeout(() => {
      if (!this.isCurrent(generation)) return;
      this.relistenTimer = null;
      void this.startListening(generation, false);
    }, 350);
  }

  private handleFailure(result: AudioOperationResultDto, detail: string): void {
    this.fail(detail, requiresConfiguration(result) ? 'configurationRequired' : 'failed');
  }

  private fail(detail: string, phase: 'failed' | 'configurationRequired' = 'failed'): void {
    this.bumpGeneration();
    this.clearTimers();
    this.cleanupTrackedSpeech();
    this.segmenter = null;
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.interruptContext = null;
    this.listeningActive = false;
    this.listeningFinalizeRequested = false;
    this.turnPreferences = null;
    this.update({ phase, detail });
  }

  private resetTurnState(): void {
    this.segmenter = null;
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.currentToken = null;
    this.currentTurnId = undefined;
    this.interruptContext = null;
    this.listeningFinalizeRequested = false;
    this.turnPreferences = null;
  }

  private cleanupTrackedSpeech(): void {
    this.offTrackedSpeech?.();
    this.offTrackedSpeech = null;
    this.currentToken = null;
  }

  private clearTimers(): void {
    this.options.timers.clearTimeout(this.relistenTimer);
    this.relistenTimer = null;
  }

  private bumpGeneration(): number {
    this.generation += 1;
    return this.generation;
  }

  private isCurrent(generation: number): boolean {
    return !this.disposed && generation === this.generation;
  }

  private update(patch: Partial<VoiceFlowState>): void {
    this.state = { ...this.state, ...patch, generation: this.generation };
    this.publish();
  }

  private publish(): void {
    this.options.onStateChange(this.state);
  }
}
