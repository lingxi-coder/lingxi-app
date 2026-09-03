import type { ImageRefDto } from '@lingxi/bridge-client';

import type {
  NativeAudioCommand,
  NativeAudioEvent,
  NativeAudioOwner,
  NativeAudioResponse,
} from '../../../shared/nativeAudio.js';
import { LANGUAGE_AUTO } from '../../../shared/voicePreferences.js';
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
  recognitionMode: 'automatic' | 'localOnly';
  language: string;
  voiceSelection: string;
  rate: number;
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
  request(command: NativeAudioCommand): Promise<NativeAudioResponse>;
  onEvent(listener: (event: NativeAudioEvent) => void): () => void;
}

export interface VoiceFlowTimers {
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface VoiceFlowControllerOptions {
  audio: VoiceFlowAudio;
  bridge: VoiceFlowBridge;
  getPreferences: () => VoiceFlowPreferences;
  createOwner: (kind: NativeAudioOwner['kind']) => NativeAudioOwner;
  timers: VoiceFlowTimers;
  onStateChange: (state: VoiceFlowState) => void;
}

interface InterruptContext {
  pausedSegments: string[];
  turnId?: number;
}

function resolvedLanguage(configured: string): string {
  if (configured === LANGUAGE_AUTO) {
    return typeof navigator !== 'undefined' && navigator.language ? navigator.language : 'en-US';
  }
  return configured;
}

function isConfigError(response: NativeAudioResponse): boolean {
  return response.type === 'error'
    && (response.error.code === 'permission' || response.error.code === 'model-missing' || response.error.code === 'unavailable');
}

export class VoiceFlowController {
  private generation = 0;
  private state: VoiceFlowState = DEFAULT_VOICE_FLOW_STATE;
  private readonly owner: NativeAudioOwner;
  private readonly offAudio: () => void;
  private offTrackedSpeech: (() => void) | null = null;
  private silenceTimer: unknown = null;
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
  private disposed = false;

  constructor(private readonly options: VoiceFlowControllerOptions) {
    this.owner = options.createOwner('flow');
    this.offAudio = options.audio.onEvent((event) => {
      this.handleAudioEvent(event);
    });
    this.publish();
  }

  getState(): VoiceFlowState {
    return this.state;
  }

  async start(): Promise<void> {
    const generation = this.bumpGeneration();
    this.clearTimers();
    this.resetTurnState();
    this.listeningActive = false;
    this.update({ phase: 'requestingPermission', detail: '正在请求麦克风与语音权限…' });
    const permissions = await this.options.audio.request({
      type: 'request_authorization',
      permissions: ['microphone', 'speech'],
    });
    if (!this.isCurrent(generation)) return;
    if (permissions.type === 'error') {
      this.handleFailure(permissions);
      return;
    }
    await this.startListening(generation, false);
  }

  async stop(): Promise<void> {
    this.bumpGeneration();
    const turnId = this.currentTurnId;
    this.clearTimers();
    this.cleanupTrackedSpeech();
    this.listeningActive = false;
    try {
      await this.options.audio.request({ type: 'cancel', owner: this.owner });
    } catch {}
    try {
      await this.options.audio.request({ type: 'stop_speaking', owner: this.owner });
    } catch {}
    if (turnId !== undefined) {
      try {
        await this.options.bridge.cancel(turnId);
      } catch {}
    }
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
    this.offAudio();
    void this.options.audio.request({ type: 'cancel', owner: this.owner }).catch(() => undefined);
    void this.options.audio.request({ type: 'stop_speaking', owner: this.owner }).catch(() => undefined);
    if (turnId !== undefined) {
      void this.options.bridge.cancel(turnId).catch(() => undefined);
    }
  }

  private async startListening(generation: number, interrupting: boolean): Promise<void> {
    const preferences = this.options.getPreferences();
    const response = await this.options.audio.request({
      type: 'start_listening',
      owner: this.owner,
      recognitionMode: preferences.recognitionMode,
      language: resolvedLanguage(preferences.language),
      sampleRateHz: 16_000,
      format: 'wav',
    });
    if (!this.isCurrent(generation)) return;
    if (response.type === 'error') {
      this.handleFailure(response);
      return;
    }
    this.listeningActive = true;
    this.update({
      phase: interrupting ? 'interrupting' : 'listening',
      detail: interrupting ? '请说出新的问题…' : '正在聆听…',
    });
  }

  private async finishListening(generation: number): Promise<void> {
    if (!this.listeningActive) return;
    this.listeningActive = false;
    this.options.timers.clearTimeout(this.silenceTimer);
    this.silenceTimer = null;
    const response = await this.options.audio.request({ type: 'finish_listening', owner: this.owner });
    if (!this.isCurrent(generation)) return;
    if (response.type === 'error') {
      this.handleFailure(response);
      return;
    }
    if (response.type !== 'listening_finished') return;
    const transcript = response.transcript?.text.trim() ?? '';
    if (this.interruptContext) {
      if (transcript) {
        await this.commitInterrupt(transcript);
      } else {
        this.resumeAfterEmptyInterrupt();
      }
      return;
    }
    if (!transcript) {
      this.scheduleRelisten();
      return;
    }
    this.beginTrackedPrompt(transcript);
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
      this.update({ phase: 'failed', detail: '无法发送当前心流问题。' });
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
      this.update({ phase: 'failed', detail: cause instanceof Error ? cause.message : '心流问题发送失败。' });
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
    } else if (event.type === 'completion' && !this.speechInFlight) {
      this.scheduleRelisten();
    }
  }

  private async maybeStartSpeaking(generation: number): Promise<void> {
    if (!this.isCurrent(generation) || this.speechInFlight || this.speechQueue.length === 0 || this.interruptContext) return;
    const segment = this.speechQueue.shift()!;
    const preferences = this.options.getPreferences();
    this.currentSpeechSegment = segment;
    this.speechInFlight = true;
    this.update({ phase: 'speaking', detail: segment });
    const response = await this.options.audio.request({
      type: 'speak',
      owner: this.owner,
      text: segment,
      voiceId: preferences.voiceSelection,
      rate: preferences.rate,
    });
    if (!this.isCurrent(generation)) return;
    if (response.type === 'error') {
      this.speechInFlight = false;
      this.currentSpeechSegment = null;
      this.handleFailure(response);
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
    };
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.update({ phase: 'interrupting', detail: '请说出新的问题…' });
    try {
      await this.options.audio.request({ type: 'stop_speaking', owner: this.owner });
    } catch {}
    if (!this.isCurrent(generation)) return;
    await this.startListening(generation, true);
  }

  private async commitInterrupt(transcript: string): Promise<void> {
    const turnId = this.interruptContext?.turnId;
    this.interruptContext = null;
    this.cleanupTrackedSpeech();
    this.resetTurnState();
    const generation = this.bumpGeneration();
    if (turnId !== undefined) {
      try {
        await this.options.bridge.cancel(turnId);
      } catch {}
      if (!this.isCurrent(generation)) return;
    }
    this.beginTrackedPrompt(transcript);
  }

  private resumeAfterEmptyInterrupt(): void {
    const paused = this.interruptContext;
    this.interruptContext = null;
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

  private handleAudioEvent(event: NativeAudioEvent): void {
    if (event.type === 'recognition_state' && event.progress.owner.id === this.owner.id && this.listeningActive) {
      const text = event.progress.text.trim();
      if (event.progress.isFinal) {
        this.listeningActive = false;
        this.options.timers.clearTimeout(this.silenceTimer);
        this.silenceTimer = null;
        if (this.interruptContext) {
          if (text) void this.commitInterrupt(text);
          else this.resumeAfterEmptyInterrupt();
        } else if (text) {
          this.beginTrackedPrompt(text);
        } else {
          this.scheduleRelisten();
        }
        return;
      }
      this.update({
        phase: this.interruptContext ? 'interrupting' : (text ? 'recognizing' : 'listening'),
        detail: text || (this.interruptContext ? '请说出新的问题…' : '正在聆听…'),
      });
      if (text) {
        this.options.timers.clearTimeout(this.silenceTimer);
        const generation = this.generation;
        this.silenceTimer = this.options.timers.setTimeout(() => {
          void this.finishListening(generation);
        }, 1_200);
      }
      return;
    }
    if (event.type === 'speech_state' && event.owner.id === this.owner.id) {
      if (event.state === 'starting' || event.state === 'speaking') {
        this.update({ phase: 'speaking', detail: this.currentSpeechSegment ?? this.state.detail });
        return;
      }
      if (event.state === 'finished' || event.state === 'interrupted') {
        this.speechInFlight = false;
        this.currentSpeechSegment = null;
        if (this.interruptContext) return;
        if (this.speechQueue.length > 0) {
          void this.maybeStartSpeaking(this.generation);
        } else if (this.streamComplete) {
          this.scheduleRelisten();
        } else {
          this.update({ phase: 'thinking', detail: '正在继续生成回复…' });
        }
      }
      return;
    }
    if (event.type === 'error' && event.owner?.id === this.owner.id) {
      this.listeningActive = false;
      this.handleFailure({
        type: 'error',
        snapshot: event.snapshot,
        error: event.error,
      });
    }
  }

  private scheduleRelisten(): void {
    this.options.timers.clearTimeout(this.relistenTimer);
    const generation = this.generation;
    this.relistenTimer = this.options.timers.setTimeout(() => {
      if (!this.isCurrent(generation)) return;
      this.update({ phase: 'listening', detail: '正在重新聆听…' });
      void this.startListening(generation, false);
    }, 350);
  }

  private handleFailure(response: Extract<NativeAudioResponse, { type: 'error' }>): void {
    if (isConfigError(response)) {
      this.update({ phase: 'configurationRequired', detail: response.error.message });
      return;
    }
    this.update({ phase: 'failed', detail: response.error.message });
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
  }

  private cleanupTrackedSpeech(): void {
    this.offTrackedSpeech?.();
    this.offTrackedSpeech = null;
    this.currentToken = null;
  }

  private clearTimers(): void {
    this.options.timers.clearTimeout(this.silenceTimer);
    this.options.timers.clearTimeout(this.relistenTimer);
    this.silenceTimer = null;
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
    this.state = {
      ...this.state,
      ...patch,
      generation: this.generation,
    };
    this.publish();
  }

  private publish(): void {
    this.options.onStateChange(this.state);
  }
}
