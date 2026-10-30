import type { ImageRefDto, AudioOperationDto, AudioOperationResultDto } from '@lingxi/bridge-client';

import type { AudioConfigurationV4 } from '../../../shared/generatedAudioConfiguration.js';
import { resolveAudioLanguage } from '../../../shared/generatedAudioConfiguration.js';
import { StreamingSpeechSegmenter } from './segmenter.js';
import { tr } from '../../i18n/locale.js';

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
  detailTranslation?: { message: VoiceFlowMessage; value: string };
  generation: number;
  activeTurnId?: number;
}

interface VoiceFlowMessage {
  key: string;
  args: readonly (string | number | VoiceFlowMessage)[];
}

function flowMessage(key: string, ...args: VoiceFlowMessage['args']): VoiceFlowMessage {
  return { key, args };
}

function resolveFlowMessage(message: VoiceFlowMessage): string {
  return tr(message.key, ...message.args.map(argument => typeof argument === 'object' ? resolveFlowMessage(argument) : argument));
}

function flowDetail(message: string | VoiceFlowMessage): Pick<VoiceFlowState, 'detail' | 'detailTranslation'> {
  if (typeof message === 'string') return { detail: message, detailTranslation: undefined };
  const value = resolveFlowMessage(message);
  return { detail: value, detailTranslation: { message, value } };
}

/** Re-resolve app copy when the UI language changes; backend and user text stay intact. */
export function voiceFlowDetail(state: VoiceFlowState): string {
  const translation = state.detailTranslation;
  // External state adapters may spread the default state and replace its detail.
  return translation && state.detail === translation.value ? resolveFlowMessage(translation.message) : state.detail;
}

export const DEFAULT_VOICE_FLOW_STATE: VoiceFlowState = {
  phase: 'paused',
  ...flowDetail(flowMessage('desktop_flow_idle')),
  generation: 0,
};

export interface VoiceFlowPreferences {
  configuration: AudioConfigurationV4;
  revision: number;
}

export interface VoiceFlowTurnToken {
  sessionId: string;
  clientTurnId: string;
  purpose: 'composer' | 'flow';
  turnId?: number;
}

export interface VoiceFlowTrackedSpeechEvent {
  type: 'delta' | 'message' | 'completion';
  token: VoiceFlowTurnToken;
  text: string;
  turnId?: number;
  terminal?: 'turn_ended' | 'stale';
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
  cancelTrackedPrompt(token: VoiceFlowTurnToken): Promise<void>;
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
  return result.type === 'failed' || result.type === 'failed_without_resource' ? `${result.error.kind}: ${result.error.message}` : null;
}

function requiresConfiguration(result: AudioOperationResultDto): boolean {
  if (result.type !== 'failed' && result.type !== 'failed_without_resource') return false;
  return ['permission_denied', 'model_missing', 'voice_missing', 'unavailable', 'unsupported', 'invalid_request'].includes(result.error.kind);
}

function voiceOverride(configuration: AudioConfigurationV4): string | undefined {
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
  private cancellationTask: { token: VoiceFlowTurnToken; promise: Promise<boolean> } | null = null;
  private currentTurnId: number | undefined;
  private speechQueue: string[] = [];
  private currentSpeechSegment: string | null = null;
  private speechInFlight = false;
  private streamComplete = false;
  private acceptedSpeechText = '';
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
    const generationBeforeCancellation = this.generation;
    // A playback failure can leave the backend turn alive. Do not let Retry
    // silently replace its cancellation handle with a competing submission.
    if (this.currentToken && !await this.cancelOwnedTurn(this.currentToken)) return;
    if (!this.isCurrent(generationBeforeCancellation) || this.listeningActive || this.currentToken) return;
    const generation = this.bumpGeneration();
    this.clearTimers();
    this.resetTurnState();
    this.update({ phase: 'requestingPermission', ...flowDetail(flowMessage('desktop_flow_preparing_audio')) });
    await this.startListening(generation, false);
  }

  async stop(): Promise<void> {
    const generation = this.bumpGeneration();
    const token = this.currentToken;
    this.clearTimers();
    this.listeningActive = false;
    // Revoke a queued credential hydration immediately, even if native audio
    // teardown is slow. Both cancellations retain the captured old owner.
    const cancelTurn = token ? this.cancelOwnedTurn(token) : Promise.resolve(true);
    try { await this.options.audio.cancel(); } catch { /* best-effort owner teardown */ }
    const released = await cancelTurn;
    if (!this.isCurrent(generation) || !released) return;
    this.cleanupTrackedSpeech();
    this.resetTurnState();
    this.update({ phase: 'paused', ...flowDetail(flowMessage('desktop_flow_idle')) });
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
    if (this.disposed) {
      if (this.currentToken) void this.cancelOwnedTurn(this.currentToken);
      return;
    }
    this.disposed = true;
    const token = this.currentToken;
    this.clearTimers();
    this.cleanupTrackedSpeech();
    void this.options.audio.cancel().catch(() => undefined);
    if (token) void this.cancelOwnedTurn(token);
    else this.resetTurnState();
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
      ...flowDetail(flowMessage(interrupting ? 'desktop_flow_replacement_question' : 'voice_listening')),
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
      this.fail(cause instanceof Error ? cause.message : flowMessage('desktop_flow_recognition_failed'));
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
      this.fail(flowMessage('desktop_flow_recognition_invalid'));
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
    await this.beginTrackedPrompt(transcript);
  }

  private async finishListening(generation: number): Promise<void> {
    if (!this.listeningActive || this.listeningFinalizeRequested || !this.isCurrent(generation)) return;
    this.listeningFinalizeRequested = true;
    this.update(this.interruptContext
      ? { phase: 'interrupting', ...flowDetail(flowMessage('desktop_flow_wait_interruption')) }
      : { phase: 'recognizing', ...flowDetail(flowMessage('voice_flow_finalizing_hint')) });
    try {
      await this.options.audio.finishListen();
    } catch (cause) {
      if (!this.isCurrent(generation) || !this.listeningActive || !this.listeningFinalizeRequested) return;
      this.listeningFinalizeRequested = false;
      const error = cause instanceof Error ? cause.message : flowMessage('desktop_flow_finish_failed');
      this.update(this.interruptContext
        ? { phase: 'interrupting', ...flowDetail(flowMessage('desktop_flow_wait_interruption_failed', error)) }
        : { phase: 'recognizing', ...flowDetail(flowMessage('desktop_flow_wait_recognition_failed', error)) });
    }
  }

  private async beginTrackedPrompt(text: string): Promise<void> {
    const generationBeforeCancellation = this.generation;
    if (this.currentToken && !await this.cancelOwnedTurn(this.currentToken)) return;
    if (!this.isCurrent(generationBeforeCancellation)) return;
    this.cleanupTrackedSpeech();
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.acceptedSpeechText = '';
    this.currentTurnId = undefined;
    const tracked = this.options.bridge.sendTrackedPrompt(text, [], [], [], { purpose: 'flow' });
    if (!tracked) {
      this.fail(flowMessage('desktop_flow_send_unavailable'));
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
      this.fail(cause instanceof Error ? cause.message : flowMessage('desktop_flow_send_failed'));
    });
  }

  private handleTrackedSpeech(event: VoiceFlowTrackedSpeechEvent, generation: number): void {
    if (!this.ownsToken(event.token)) return;
    if (event.type === 'completion') {
      // Even a failed/stopped playback keeps this subscription until its
      // captured backend owner is actually terminal or cancellation succeeds.
      this.releaseOwnedTurn(event.token);
    }
    if (!this.isCurrent(generation)) return;
    if (event.turnId !== undefined) {
      this.currentTurnId = event.turnId;
      this.update({ activeTurnId: event.turnId });
    }
    // Raw deltas may belong to an attempt the SDK immediately retracts. Only
    // confirmed message chunks enter playback; completion owns relistening.
    if (event.type === 'delta') return;
    if (event.type === 'completion' && event.terminal === 'stale') {
      void this.stop();
      return;
    }
    let text = event.text;
    if (event.type === 'message') this.acceptedSpeechText += text;
    else {
      text = text.startsWith(this.acceptedSpeechText) ? text.slice(this.acceptedSpeechText.length) : text;
      this.streamComplete = true;
    }
    const segments = new StreamingSpeechSegmenter().finish(text);
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
        if (this.interruptContext && (result.type === 'failed' || result.type === 'failed_without_resource') && result.error.kind === 'cancelled') return;
        this.handleFailure(result, detail);
        return;
      }
      if (result.type !== 'playback_completed') {
        this.speechInFlight = false;
        this.currentSpeechSegment = null;
        this.fail(flowMessage('desktop_flow_playback_unconfirmed'));
        return;
      }
      this.speechInFlight = false;
      this.currentSpeechSegment = null;
      if (this.interruptContext) return;
      if (this.speechQueue.length > 0) void this.maybeStartSpeaking(generation);
      else if (this.streamComplete) this.scheduleRelisten();
      else this.update({ phase: 'thinking', ...flowDetail(flowMessage('desktop_flow_continuing_reply')) });
    } catch (cause) {
      if (!this.isCurrent(generation)) return;
      this.speechInFlight = false;
      this.currentSpeechSegment = null;
      this.fail(cause instanceof Error ? cause.message : flowMessage('desktop_flow_playback_failed'));
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
    this.update({ phase: 'interrupting', ...flowDetail(flowMessage('desktop_flow_replacement_question')) });
    try { await this.options.audio.cancel(); } catch { /* pending playback may already be done */ }
    if (!this.isCurrent(generation)) return;
    await this.startListening(generation, true);
  }

  private async commitInterrupt(transcript: string): Promise<void> {
    const token = this.currentToken;
    const replacementPreferences = this.turnPreferences;
    this.interruptContext = null;
    const generation = this.bumpGeneration();
    if (token) {
      if (!await this.cancelOwnedTurn(token)) return;
      if (!this.isCurrent(generation)) return;
    }
    this.resetTurnState();
    this.turnPreferences = replacementPreferences;
    await this.beginTrackedPrompt(transcript);
  }

  private resumeAfterEmptyInterrupt(): void {
    const paused = this.interruptContext;
    this.interruptContext = null;
    if (paused) this.turnPreferences = paused.previousPreferences;
    if (paused && paused.pausedSegments.length > 0) {
      this.speechQueue = [...paused.pausedSegments, ...this.speechQueue];
      this.update({ phase: 'paused', ...flowDetail(flowMessage('desktop_flow_resume_reply')) });
      const generation = this.generation;
      this.relistenTimer = this.options.timers.setTimeout(() => {
        void this.maybeStartSpeaking(generation);
      }, 0);
      return;
    }
    this.update({ phase: 'paused', ...flowDetail(flowMessage('desktop_flow_no_interruption')) });
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

  private fail(detail: string | VoiceFlowMessage, phase: 'failed' | 'configurationRequired' = 'failed'): void {
    const token = this.currentToken;
    this.bumpGeneration();
    this.clearTimers();
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.acceptedSpeechText = '';
    this.interruptContext = null;
    this.listeningActive = false;
    this.listeningFinalizeRequested = false;
    this.turnPreferences = null;
    this.update({ phase, ...flowDetail(detail) });
    if (token) void this.cancelOwnedTurn(token, detail);
    else this.cleanupTrackedSpeech();
  }

  private resetTurnState(): void {
    this.speechQueue = [];
    this.currentSpeechSegment = null;
    this.speechInFlight = false;
    this.streamComplete = false;
    this.acceptedSpeechText = '';
    this.currentToken = null;
    this.currentTurnId = undefined;
    this.interruptContext = null;
    this.listeningFinalizeRequested = false;
    this.turnPreferences = null;
  }

  private cleanupTrackedSpeech(): void {
    this.offTrackedSpeech?.();
    this.offTrackedSpeech = null;
  }

  private ownsToken(token: VoiceFlowTurnToken): boolean {
    return this.currentToken?.sessionId === token.sessionId
      && this.currentToken?.clientTurnId === token.clientTurnId;
  }

  private releaseOwnedTurn(token: VoiceFlowTurnToken): void {
    if (!this.ownsToken(token)) return;
    this.currentToken = null;
    this.cleanupTrackedSpeech();
  }

  private cancelOwnedTurn(token: VoiceFlowTurnToken, failureDetail?: string | VoiceFlowMessage): Promise<boolean> {
    if (!this.ownsToken(token)) return Promise.resolve(true);
    if (this.cancellationTask?.token.sessionId === token.sessionId
      && this.cancellationTask.token.clientTurnId === token.clientTurnId) return this.cancellationTask.promise;
    const attempt = { token, promise: Promise.resolve(false) };
    attempt.promise = this.options.bridge.cancelTrackedPrompt(token).then(() => {
      this.releaseOwnedTurn(token);
      return true;
    }).catch((cause: unknown) => {
      if (!this.ownsToken(token)) return true;
      if (!this.disposed) {
        const message = cause instanceof Error ? cause.message : flowMessage('desktop_flow_stop_failed');
        this.update({ phase: 'failed', ...flowDetail(failureDetail
          ? flowMessage('desktop_flow_stop_previous_with_detail', failureDetail, message)
          : flowMessage('desktop_flow_stop_previous', message)) });
      }
      return false;
    }).finally(() => {
      if (this.cancellationTask === attempt) this.cancellationTask = null;
    });
    this.cancellationTask = attempt;
    return attempt.promise;
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
    this.state = {
      ...this.state,
      ...(Object.prototype.hasOwnProperty.call(patch, 'detail') ? { detailTranslation: undefined } : {}),
      ...patch,
      generation: this.generation,
    };
    this.publish();
  }

  private publish(): void {
    this.options.onStateChange(this.state);
  }
}
