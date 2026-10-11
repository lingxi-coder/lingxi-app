import { normalizeAudioUsage, type AudioUsageRecord } from '../../shared/audioUsage.js';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { StringDecoder } from 'node:string_decoder';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

import type { AudioErrorKindDto, AudioOperationDto, AudioOperationIdDto, AudioOperationRequestDto, AudioOperationResultDto, AudioOwnerDto } from '@lingxi/bridge-client';

import type { ProviderAudioHost, ProviderAudioSession } from './providerAudioHost.js';
import type { DiagnosticBuffer } from '../host-utils.js';
import type { MicrophonePermissionStatus } from '../../shared/microphoneAccess.js';
import { offlineVoiceModelById } from '../../shared/voiceModelCatalog.js';
import { audioConfigurationDefaults, normalizeAudioConfiguration, type AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import {
  defaultNativeAudioSnapshot,
  validateNativeAudioCommand,
  validateNativeAudioEngineResponse,
  validateNativeAudioOperationRequest,
  validateNativeAudioOperation,
  validateNativeAudioOperationIdentity,
  validateNativeAudioEvent,
  validateNativeAudioResponse,
  type NativeAudioCommand,
  type NativeAudioEngineResponse,
  type NativeAudioErrorCode,
  type NativeAudioEvent,
  type NativeAudioHelperCommandEnvelope,
  type NativeAudioHelperEnvelope,
  type NativeAudioOwner,
  type NativeAudioResponse,
  type NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';

const AUDIO_HELPER_APP = 'LingXiAudioHelper.app';
const AUDIO_HELPER_BIN = 'LingXiAudioHelper';
const AUDIO_HELPER_ENV = 'LINGXI_AUDIO_HELPER_BIN';
const REQUEST_TIMEOUT_MS = 5 * 60_000;
const SPEAK_TIMEOUT_MS = 20 * 60_000;
const SUSPEND_TEARDOWN_TIMEOUT_MS = 3_000;

type HelperProcess = Pick<ChildProcessWithoutNullStreams, 'pid' | 'stdin' | 'stdout' | 'stderr' | 'kill' | 'once'>;
type SpawnHelper = (path: string, args: string[], env: NodeJS.ProcessEnv) => HelperProcess;
type AudioPermission = 'microphone' | 'speech';
type LaunchPermissionHelper = (appPath: string, permissions: AudioPermission[]) => Promise<void>;
type VerifyPackagedHelper = (appPath: string) => void;
type RequestMicrophoneAccess = () => Promise<MicrophonePermissionStatus>;

interface PendingCommandRequest {
  kind: 'command';
  owner: NativeAudioOwner | null;
  resolve: (response: NativeAudioResponse) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface PendingEngineRequest {
  kind: 'engine';
  request: AudioOperationRequestDto;
  resolve: (response: NativeAudioEngineResponse) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface InFlightAudioRequest {
  request: AudioOperationRequestDto;
  helperId?: string;
  cancelled: boolean;
  timedOut: boolean;
  resolveCancellation: () => void;
  cancellation: Promise<AudioOperationResultDto>;
}

interface ActiveRecordingOrigin {
  owner: AudioOwnerDto;
  configurationRevision: number;
}

interface UiListenOperation {
  identity?: AudioOperationIdDto;
  finishRequested: boolean;
  finishAcknowledged: boolean;
  admitted: boolean;
  finishCommand?: Promise<void>;
}

interface UiOperationCancellation {
  cancelled: boolean;
}

type PendingRequest = PendingCommandRequest | PendingEngineRequest;

export interface NativeAudioManagerOptions {
  providerAudio?: ProviderAudioHost;
  getAudioSessionContext?: (owner?: AudioOwnerDto) => ProviderAudioSession | undefined;
  isPackaged: boolean;
  resourcesPath: string;
  userDataPath: string;
  diagnostics: DiagnosticBuffer;
  spawnHelper?: SpawnHelper;
  launchPermissionHelper?: LaunchPermissionHelper;
  verifyPackagedHelper?: VerifyPackagedHelper;
  requestMicrophoneAccess?: RequestMicrophoneAccess;
  helperPath?: string;
  getAudioConfiguration?: () => AudioConfigurationV4;
  getAudioConfigurationRevision?: () => number;
  isForeground?: () => boolean;
  suspendTeardownTimeoutMs?: number;
}

function launchPermissionHelper(appPath: string, permissions: AudioPermission[]): Promise<void> {
  return new Promise((resolveLaunch, rejectLaunch) => {
    const child = spawn('/usr/bin/open', [
      '-W',
      '-n',
      appPath,
      '--args',
      '--request-permissions',
      permissions.join(','),
    ], { stdio: 'ignore' });
    const timer = setTimeout(() => {
      child.kill();
      rejectLaunch(new Error('timed out waiting for the macOS audio permission prompt'));
    }, REQUEST_TIMEOUT_MS);
    child.once('error', (error) => {
      clearTimeout(timer);
      rejectLaunch(error);
    });
    child.once('exit', (code, signal) => {
      clearTimeout(timer);
      if (code === 0) {
        resolveLaunch();
        return;
      }
      rejectLaunch(new Error(`audio permission helper exited (code=${String(code)} signal=${String(signal)})`));
    });
  });
}

function cloneOwner(owner: NativeAudioOwner | null): NativeAudioOwner | null {
  return owner ? { ...owner } : null;
}

function cloneSnapshot(snapshot: NativeAudioSnapshot): NativeAudioSnapshot {
  return {
    helper: { ...snapshot.helper },
    permissions: { ...snapshot.permissions },
    owner: cloneOwner(snapshot.owner),
    activity: snapshot.activity,
    ...(snapshot.localeTag === undefined ? {} : { localeTag: snapshot.localeTag }),
    ...(snapshot.recognizerAvailable === undefined ? {} : { recognizerAvailable: snapshot.recognizerAvailable }),
    ...(snapshot.recognition ? { recognition: { ...snapshot.recognition } } : {}),
    ...(snapshot.playback ? { playback: { ...snapshot.playback } } : {}),
    ...(snapshot.capabilities ? {
      capabilities: {
        ...snapshot.capabilities,
        supported_operations: [...snapshot.capabilities.supported_operations],
        readiness: snapshot.capabilities.readiness.map((entry) => ({ ...entry })),
      },
    } : {}),
    ...(snapshot.configurationRevision === undefined ? {} : { configurationRevision: snapshot.configurationRevision }),
    ...(snapshot.currentOperation ? {
      currentOperation: {
        identity: { ...snapshot.currentOperation.identity },
        owner: { ...snapshot.currentOperation.owner },
      },
    } : {}),
    audioOperations: snapshot.audioOperations.map((operation) => ({
      ...operation,
      identity: { ...operation.identity },
      owner: { ...operation.owner },
    })),
    activeOperationCount: snapshot.activeOperationCount,
    pendingOperationCount: snapshot.pendingOperationCount,
    activeRecordingCount: snapshot.activeRecordingCount,
    activePlaybackCount: snapshot.activePlaybackCount,
    activeModelReferenceCount: snapshot.activeModelReferenceCount,
    voices: snapshot.voices.map((voice) => ({ ...voice })),
    models: snapshot.models.map((model) => ({ ...model })),
  };
}

function sameOwner(left: NativeAudioOwner | null, right: NativeAudioOwner | null): boolean {
  return Boolean(left && right && left.kind === right.kind && left.id === right.id);
}

function audioOwnerFromSnapshotOwner(owner: NativeAudioOwner): AudioOwnerDto | null {
  switch (owner.kind) {
    case 'session':
    case 'engine':
      return { type: 'session', session_id: owner.id };
    case 'ui':
    case 'system':
      return { type: owner.kind, instance_id: owner.id };
    case 'dictation':
    case 'flow':
    case 'preview':
    case 'autoplay':
      return null;
  }
}

function failedAudio(kind: AudioErrorKindDto, message: string): AudioOperationResultDto {
  return { type: 'failed', error: { kind, message } };
}

function audioIdentityKey(identity: AudioOperationIdDto): string {
  return `${identity.service_epoch}:${identity.generation}:${identity.id}`;
}

function audioOwnerKey(owner: AudioOwnerDto): string {
  switch (owner.type) {
    case 'session': return `session:${owner.session_id}`;
    case 'ui': return `ui:${owner.instance_id}`;
    case 'system': return `system:${owner.instance_id}`;
  }
}

function sameIdentity(left: AudioOperationIdDto | undefined, right: AudioOperationIdDto): boolean {
  return left?.id === right.id
    && left.generation === right.generation
    && left.service_epoch === right.service_epoch;
}

function operationKind(operation: AudioOperationRequestDto['operation']): 'record' | 'listen' | 'synthesize' | 'speak' | null {
  switch (operation.type) {
    case 'capture':
    case 'start_recording':
    case 'stop_recording': return 'record';
    case 'listen': return 'listen';
    case 'synthesize': return 'synthesize';
    case 'play':
    case 'speak': return 'speak';
    case 'status':
    case 'end_owner': return null;
  }
}

function nativeErrorKind(code: NativeAudioErrorCode): AudioErrorKindDto {
  switch (code) {
    case 'permission': return 'permission_denied';
    case 'busy': return 'busy';
    case 'cancelled': return 'cancelled';
    case 'model-missing': return 'model_missing';
    case 'unavailable': return 'unavailable';
    case 'invalid-request': return 'invalid_request';
    case 'download':
    case 'checksum': return 'native_failure';
    case 'native-error': return 'native_failure';
  }
}

function isCurrentOrIdleSnapshot(snapshot: NativeAudioSnapshot, request: AudioOperationRequestDto): boolean {
  if (snapshot.currentOperation) return sameIdentity(snapshot.currentOperation.identity, request.identity);
  return snapshot.activity === 'idle' && snapshot.owner === null;
}

function commandModelId(command: NativeAudioCommand): string | null {
  switch (command.type) {
    case 'install_model':
    case 'cancel_model':
    case 'remove_model':
      return command.modelId;
    default:
      return null;
  }
}

export function resolveNativeAudioHelperPath(opts: Pick<NativeAudioManagerOptions, 'isPackaged' | 'resourcesPath' | 'helperPath'>): string | null {
  if (opts.helperPath) return resolve(opts.helperPath);
  if (!opts.isPackaged) {
    const explicit = process.env[AUDIO_HELPER_ENV]?.trim();
    if (!explicit) return null;
    const resolved = resolve(explicit);
    return existsSync(resolved) ? resolved : null;
  }
  const packaged = join(opts.resourcesPath, AUDIO_HELPER_APP, 'Contents', 'MacOS', AUDIO_HELPER_BIN);
  return existsSync(packaged) ? packaged : null;
}

export class NativeAudioManager {
  private readonly storageRoot: string;
  private readonly spawnHelper: SpawnHelper;
  private readonly launchPermissionHelper: LaunchPermissionHelper;
  private readonly subscribers = new Set<(event: NativeAudioEvent) => void>();
  private readonly pending = new Map<string, PendingRequest>();
  private readonly pendingAudioRequests = new Map<string, InFlightAudioRequest>();
  private readonly seenAudioIdentities = new Set<string>();
  private readonly audioIdentityHistory: string[] = [];
  private readonly activeRecordingOrigins = new Map<string, ActiveRecordingOrigin>();
  private readonly uiGenerationByInstance = new Map<string, number>();
  private readonly uiListenOperations = new Map<string, UiListenOperation[]>();
  private readonly uiOperationsByInstance = new Map<string, Set<UiOperationCancellation>>();
  private readonly uiConfigurationPins = new Map<string, { revision: number; configuration: AudioConfigurationV4 }>();
  private readonly suspendedHelpers = new WeakSet<object>();
  private readonly helperPath: string | null;
  private helper: HelperProcess | null = null;
  private helperStartup: Promise<void> | null = null;
  private capabilitiesHelper: HelperProcess | null = null;
  private capabilityInitialization: Promise<void> | null = null;
  private permissionResolution: Promise<NativeAudioSnapshot> | null = null;
  private helperStdoutBuffer = '';
  private snapshot = defaultNativeAudioSnapshot();
  private audioSuspending = false;
  private suspensionRevision = 0;
  private suspensionTask: Promise<void> | null = null;

  constructor(private readonly opts: NativeAudioManagerOptions) {
    this.storageRoot = join(opts.userDataPath, 'voice-models');
    this.spawnHelper = opts.spawnHelper ?? ((path, args, env) => spawn(path, args, {
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
    }) as ChildProcessWithoutNullStreams);
    this.launchPermissionHelper = opts.launchPermissionHelper ?? launchPermissionHelper;
    this.helperPath = resolveNativeAudioHelperPath(opts);
    if (!this.helperPath) {
      this.snapshot = {
        ...this.snapshot,
        helper: {
          state: 'stopped',
          message: opts.isPackaged
            ? 'the packaged native audio helper is missing from application resources'
            : `set ${AUDIO_HELPER_ENV} to the built helper binary to enable native audio in development`,
        },
      };
    }
  }

  private readonly audioUsageLedger: AudioUsageRecord[] = [];
  private audioUsageSequence = 0;
  getAudioUsageLedger(): AudioUsageRecord[] { return structuredClone(this.audioUsageLedger); }
  recordAudioUsage(metadata: Omit<AudioUsageRecord, 'sequence' | 'usage'>, value: unknown): void {
    const usage = normalizeAudioUsage(value);
    if (!Object.keys(usage).length) return;
    this.audioUsageLedger.push({ ...metadata, sequence: ++this.audioUsageSequence, usage });
    if (this.audioUsageLedger.length > 128) this.audioUsageLedger.shift();
  }

  private executedRoute?: NativeAudioSnapshot['executedRoute'];
  private hostedSnapshot: Pick<NativeAudioSnapshot, 'providerCapabilities' | 'providerCatalog' | 'sessionContext' | 'realtimeReadiness'> = {};
  private readonly hostedOperations = new Map<string, { controller: AbortController; owner: AudioOwnerDto }>();
  private readonly streamCaptureOwners = new Set<string>();
  private readonly streamCaptureIdentities = new Map<string, AudioOperationIdDto>();
  private streamGeneration = 0;
  private readonly realtimeRecordingHandles = new Map<string, string>();
  private readonly captureFinishes = new Map<string, () => void>();

  getSnapshot(): NativeAudioSnapshot {
    const session = this.opts.getAudioSessionContext?.();
    const hosted = JSON.stringify(this.hostedSnapshot.sessionContext) === JSON.stringify(session) ? this.hostedSnapshot : {};
    return { ...cloneSnapshot(this.snapshot), ...structuredClone(hosted), ...(this.audioUsageLedger.length ? { usageLedger: this.getAudioUsageLedger() } : {}), ...(this.executedRoute && (!this.executedRoute.sessionId || this.executedRoute.sessionId === session?.sessionId) ? { executedRoute: { ...this.executedRoute } } : {}), activeOperationCount: this.snapshot.activeOperationCount + this.hostedOperations.size };
  }

  private async refreshHostedSnapshot(configurationOverride?: AudioConfigurationV4): Promise<Pick<NativeAudioSnapshot, 'providerCapabilities' | 'providerCatalog' | 'sessionContext' | 'realtimeReadiness'>> {
    if (!this.opts.providerAudio) return {};
    const session = this.opts.getAudioSessionContext?.();
    const sessionKey = JSON.stringify(session);
    const configuration = configurationOverride ?? this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults();
    const revision = this.opts.getAudioConfigurationRevision?.() ?? 0;
    const snapshot = await this.opts.providerAudio.capabilities(configuration, session);
    if (!configurationOverride && sessionKey === JSON.stringify(this.opts.getAudioSessionContext?.()) && revision === (this.opts.getAudioConfigurationRevision?.() ?? 0)) {
      this.hostedSnapshot = snapshot;
      this.emit({ type: 'snapshot_changed', snapshot: this.getSnapshot() });
    }
    return sessionKey === JSON.stringify(this.opts.getAudioSessionContext?.()) ? snapshot : {};
  }

  private async executeHostedRequest(request: AudioOperationRequestDto, configuration: AudioConfigurationV4, revision: number, onAdmission?: () => void | Promise<void>, shouldContinue?: () => boolean): Promise<AudioOperationResultDto> {
    const key = audioIdentityKey(request.identity);
    this.rememberAudioIdentity(key);
    const controller = new AbortController();
    this.hostedOperations.set(key, { controller, owner: request.owner });
    const frozen = structuredClone(configuration);
    const session = this.opts.getAudioSessionContext?.(request.owner);
    const resolvedRoute = (kind: 'recognition' | 'speech') => (route: { profileId: string; providerId: string; modelId: string | null; voiceId?: string }) => {
      if (!controller.signal.aborted && (!shouldContinue || shouldContinue())) this.executedRoute = { ...route, kind, source: 'provider', operationId: request.identity.id, configurationRevision: revision, ...(session ? { sessionId: session.sessionId } : {}) };
    };
    const reportedUsage = (kind: 'recognition' | 'speech') => (usage: unknown, route: { profileId: string; providerId: string; modelId: string | null; accountScope?: string }) => {
      this.recordAudioUsage({ ...route, kind, operationId: request.identity.id, configurationRevision: revision,
        ...(session ? { sessionId: session.sessionId } : {}), accountScope: route.accountScope }, usage);
    };
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; controller.abort(); }, request.timeout_budget_ms ?? REQUEST_TIMEOUT_MS);
    const device = async (operation: AudioOperationDto) => {
      if (controller.signal.aborted || (shouldContinue && !shouldContinue())) return failedAudio('cancelled', 'the audio operation was cancelled');
      const identity = { ...request.identity, id: randomUUID(), service_epoch: this.getCapabilities().service_epoch };
      const abort = () => { void this.cancelAudioRequest(identity); };
      controller.signal.addEventListener('abort', abort, { once: true });
      try { return await this.executeAudioRequest({ ...request, identity, operation }, { configuration: frozen, revision }, undefined, () => !controller.signal.aborted && (!shouldContinue || shouldContinue())); }
      finally { controller.signal.removeEventListener('abort', abort); }
    };
    try {
      const operation = request.operation;
      if (operation.type === 'listen') {
        if (!this.opts.providerAudio) return failedAudio('unavailable', 'Provider audio host is unavailable');
        const admissionFailure = await this.opts.providerAudio.preflight('recognition', frozen, session, controller.signal);
        if (admissionFailure) return admissionFailure;
      }
      if (operation.type === 'capture' || operation.type === 'listen') {
        const started = await device({ type: 'start_recording', sample_rate_hz: operation.type === 'capture' ? operation.sample_rate_hz : 16_000, format: operation.type === 'capture' ? operation.format : 'wav' });
        if (started.type !== 'recording_started') return started;
        const ownerKey = audioOwnerKey(request.owner);
        let finish!: () => void;
        let durationTimer: NodeJS.Timeout | undefined;
        const stopped = new Promise<void>((resolve) => { finish = resolve; });
        if (this.captureFinishes.has(ownerKey)) { await device({ type: 'stop_recording', handle: started.handle }); return failedAudio('busy', 'another capture is active for this audio owner'); }
        this.captureFinishes.set(ownerKey, finish);
        durationTimer = setTimeout(finish, 15_000);
        controller.signal.addEventListener('abort', finish, { once: true });
        try { await onAdmission?.(); await stopped; }
        finally { clearTimeout(durationTimer); controller.signal.removeEventListener('abort', finish); if (this.captureFinishes.get(ownerKey) === finish) this.captureFinishes.delete(ownerKey); }
        if (controller.signal.aborted) {
          await this.endAudioOwner(request.owner);
          return failedAudio(timedOut ? 'timeout' : 'cancelled', timedOut ? 'audio capture timed out' : 'audio capture cancelled');
        }
        const recording = await device({ type: 'stop_recording', handle: started.handle });
        if (operation.type === 'capture' || recording.type !== 'recording') return recording;
        if (!this.opts.providerAudio) return failedAudio('unavailable', 'Provider audio host is unavailable');
        return await this.opts.providerAudio.execute('recognition', frozen, { audioBase64: recording.audio_base64, mimeType: recording.mime_type, language: operation.language, onResolvedRoute: resolvedRoute('recognition'), onUsage: reportedUsage('recognition'), maxPayloadBytes: request.max_payload_bytes, timeoutMs: request.timeout_budget_ms }, session, controller.signal);
      }
      if (operation.type !== 'synthesize' && operation.type !== 'speak') return failedAudio('unsupported', 'Unsupported provider audio operation');
      if (!this.opts.providerAudio) return failedAudio('unavailable', 'Provider audio host is unavailable');
      const synthesized = await this.opts.providerAudio.execute('speech', frozen, { text: operation.text, language: operation.language, rate: operation.rate, voice: operation.voice, onResolvedRoute: resolvedRoute('speech'), onUsage: reportedUsage('speech'), maxPayloadBytes: request.max_payload_bytes, timeoutMs: request.timeout_budget_ms }, session, controller.signal);
      if (operation.type === 'synthesize' || synthesized.type !== 'synthesized') return synthesized;
      return await device({ type: 'play', pcm_base64: synthesized.pcm_base64, sample_rate_hz: synthesized.sample_rate_hz });
    } finally {
      clearTimeout(timer);
      this.hostedOperations.delete(key);
    }
  }

  async startRealtimeCapture(sessionId: string, sampleRateHz: number): Promise<AudioOperationResultDto> {
    await this.ensureCapabilities();
    const owner: AudioOwnerDto = { type: 'session', session_id: sessionId };
    this.streamCaptureOwners.add(audioOwnerKey(owner));
    const identity = { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch };
    this.streamCaptureIdentities.set(audioOwnerKey(owner), identity);
    try {
      const result = await this.executeAudioRequest({ identity, owner,
        operation: { type: 'start_recording', sample_rate_hz: sampleRateHz, format: 'wav' }, max_payload_bytes: this.getCapabilities().max_payload_bytes });
      if (result.type === 'recording_started') this.realtimeRecordingHandles.set(sessionId, result.handle);
      else this.streamCaptureOwners.delete(audioOwnerKey(owner));
      return result;
    } catch (error) { this.streamCaptureOwners.delete(audioOwnerKey(owner)); throw error; }
  }

  async stopRealtimeCapture(sessionId: string): Promise<void> {
    const handle = this.realtimeRecordingHandles.get(sessionId);
    if (handle) {
      try { await this.executeAudioRequest({ identity: { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch }, owner: { type: 'session', session_id: sessionId }, operation: { type: 'stop_recording', handle }, max_payload_bytes: this.getCapabilities().max_payload_bytes }); }
      finally { this.realtimeRecordingHandles.delete(sessionId); this.streamCaptureOwners.delete(`session:${sessionId}`); }
    } else { this.streamCaptureOwners.delete(`session:${sessionId}`); await this.endAudioOwner({ type: 'session', session_id: sessionId }); }
  }

  async playRealtimeAudio(sessionId: string, pcmBase64: string, sampleRateHz: number): Promise<AudioOperationResultDto> {
    await this.ensureCapabilities();
    return this.executeAudioRequest({ identity: { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch }, owner: { type: 'session', session_id: sessionId },
      operation: { type: 'play', pcm_base64: pcmBase64, sample_rate_hz: sampleRateHz }, max_payload_bytes: this.getCapabilities().max_payload_bytes });
  }

  getCapabilities(): NonNullable<NativeAudioSnapshot['capabilities']> {
    return this.getSnapshot().capabilities ?? defaultNativeAudioSnapshot().capabilities!;
  }

  async refreshHostedCapabilities(): Promise<void> { await this.refreshHostedSnapshot(); }

  async initializeCapabilities(): Promise<void> {
    await this.ensureCapabilities();
  }

  onEvent(callback: (event: NativeAudioEvent) => void): () => void {
    this.subscribers.add(callback);
    return () => this.subscribers.delete(callback);
  }

  async request(value: unknown): Promise<NativeAudioResponse> {
    const command = validateNativeAudioCommand(value);
    let previewHosted: Pick<NativeAudioSnapshot, 'providerCapabilities' | 'providerCatalog' | 'sessionContext' | 'realtimeReadiness'> | undefined;
    if (command.type === 'get_snapshot') {
      try { previewHosted = await this.refreshHostedSnapshot(command.configuration); } catch { if (!command.configuration) this.hostedSnapshot = {}; }
    }
    if (command.type === 'get_snapshot' && !this.helperPath) {
      return { type: 'snapshot', snapshot: { ...this.getSnapshot(), ...previewHosted } };
    }
    const modelId = commandModelId(command);
    if (modelId && !offlineVoiceModelById(modelId)) {
      return this.errorResponse('invalid-request', `unknown model id: ${modelId}`);
    }
    if (!this.helperPath) return this.unavailableResponse();
    try {
      await this.ensureHelper();
      if (command.type === 'request_authorization' && this.opts.isPackaged) {
        const snapshot = await this.resolvePackagedPermissions(command.permissions);
        return { type: 'authorization', snapshot };
      }
      const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'command', command: command.type === 'get_snapshot' ? { type: 'get_snapshot' } : command }, null);
      const normalized = this.normalizeCommandResponse(response);
      if (this.helper && normalized.type !== 'error' && normalized.snapshot.capabilities) {
        this.capabilitiesHelper = this.helper;
      }
      return previewHosted ? { ...normalized, snapshot: { ...normalized.snapshot, ...previewHosted } } : normalized;
    } catch (error) {
      this.opts.diagnostics.add('error', 'host', `native audio request failed: ${String(error)}`);
      return this.errorResponse('native-error', error instanceof Error ? error.message : String(error));
    }
  }

  async executeAudioRequest(
    value: unknown,
    configurationSnapshot?: { configuration: AudioConfigurationV4; revision: number },
    onNativeAdmission?: () => void | Promise<void>,
    shouldContinue?: () => boolean,
  ): Promise<AudioOperationResultDto> {
    const request = validateNativeAudioOperationRequest(value);
    const suspensionRevision = this.suspensionRevision;
    const identityKey = audioIdentityKey(request.identity);
    if (this.seenAudioIdentities.has(identityKey)) {
      return failedAudio('cancelled', 'the audio operation identity is stale');
    }
    if (this.audioSuspending && request.operation.type !== 'end_owner' && request.operation.type !== 'status') {
      return failedAudio('cancelled', 'audio was suspended because the desktop window is hidden');
    }
    if (shouldContinue && !shouldContinue()) {
      return failedAudio('cancelled', 'the audio operation was cancelled');
    }
    const deadlineAt = request.timeout_budget_ms === undefined
      ? null
      : performance.now() + request.timeout_budget_ms;
    const timeoutResult = () => failedAudio('timeout', 'the audio operation deadline expired before native admission');
    const admittedTimeoutResult = () => failedAudio('timeout', 'the audio operation deadline expired after native admission');
    const remainingBudget = () => deadlineAt === null ? undefined : Math.max(0, deadlineAt - performance.now());
    if (request.timeout_budget_ms === 0) {
      return timeoutResult();
    }
    let earlyConfiguration: AudioConfigurationV4 | undefined;
    if (['capture', 'listen', 'synthesize', 'speak'].includes(request.operation.type)) {
      try { earlyConfiguration = structuredClone(configurationSnapshot?.configuration ?? this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults()); }
      catch (error) { return failedAudio('unavailable', error instanceof Error ? error.message : 'the saved audio configuration is unavailable'); }
    }
    const hosted = request.operation.type === 'capture'
      || (request.operation.type === 'listen' && earlyConfiguration?.recognition.source === 'provider')
      || ((request.operation.type === 'speak' || request.operation.type === 'synthesize') && earlyConfiguration?.speech.source === 'provider');
    if (hosted) {
      if (request.identity.service_epoch !== this.getCapabilities().service_epoch) return failedAudio('cancelled', 'the audio service changed before this request started');
      return this.executeHostedRequest(request, earlyConfiguration ?? audioConfigurationDefaults(), configurationSnapshot?.revision ?? this.opts.getAudioConfigurationRevision?.() ?? 0, onNativeAdmission, shouldContinue);
    }
    if (request.operation.type === 'listen' || request.operation.type === 'synthesize' || request.operation.type === 'speak') this.executedRoute = undefined;
    if (!this.helperPath) {
      return failedAudio('unavailable', this.snapshot.helper.message ?? 'native audio helper is unavailable');
    }
    const capabilityBudget = remainingBudget();
    if (capabilityBudget === 0) return timeoutResult();
    try {
      await this.ensureCapabilities(capabilityBudget ?? REQUEST_TIMEOUT_MS);
    } catch (error) {
      if (this.seenAudioIdentities.has(identityKey)) {
        return failedAudio('cancelled', 'the audio operation identity is stale');
      }
      const message = error instanceof Error ? error.message : String(error);
      if (remainingBudget() === 0 || /timed out/i.test(message)) return timeoutResult();
      this.opts.diagnostics.add('error', 'host', `native audio capability initialization failed: ${message}`);
      return failedAudio('unavailable', message);
    }
    if (this.seenAudioIdentities.has(identityKey)) {
      return failedAudio('cancelled', 'the audio operation identity is stale');
    }
    if ((this.audioSuspending || this.suspensionRevision !== suspensionRevision)
      && request.operation.type !== 'end_owner' && request.operation.type !== 'status') {
      return failedAudio('cancelled', 'audio was suspended because the desktop window is hidden');
    }
    if (shouldContinue && !shouldContinue()) {
      return failedAudio('cancelled', 'the audio operation was cancelled');
    }
    if (remainingBudget() === 0) return timeoutResult();
    const capabilities = this.getCapabilities();
    if (request.identity.service_epoch !== capabilities.service_epoch) {
      return failedAudio('cancelled', 'the audio service changed before this request started');
    }
    const kind = operationKind(request.operation);
    if (kind && !capabilities.supported_operations.some((operation) => operation === kind)) {
      return failedAudio('unsupported', 'this device does not support the requested audio operation');
    }
    this.rememberAudioIdentity(identityKey);
    let operationConfiguration: AudioConfigurationV4;
    let operationConfigurationRevision: number;
    if (request.operation.type === 'status' || request.operation.type === 'end_owner') {
      operationConfiguration = audioConfigurationDefaults();
      operationConfigurationRevision = 0;
    } else if (request.operation.type === 'stop_recording') {
      operationConfiguration = configurationSnapshot?.configuration ?? audioConfigurationDefaults();
      const ownerKey = audioOwnerKey(request.owner);
      const activeRecording = [...this.activeRecordingOrigins.values()].find(
        (recording) => audioOwnerKey(recording.owner) === ownerKey,
      );
      operationConfigurationRevision = configurationSnapshot?.revision ?? activeRecording?.configurationRevision ?? 0;
    } else {
      try {
        operationConfiguration = earlyConfiguration ?? structuredClone(configurationSnapshot?.configuration ?? this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults());
        operationConfigurationRevision = configurationSnapshot?.revision ?? this.opts.getAudioConfigurationRevision?.() ?? 0;
      } catch (error) {
        return failedAudio('unavailable', error instanceof Error ? error.message : 'the saved audio configuration is unavailable');
      }
    }
    if (
      (request.operation.type === 'start_recording' || request.operation.type === 'listen')
      && this.opts.isForeground
      && !this.opts.isForeground()
    ) {
      return failedAudio('unavailable', 'a foreground desktop window is required to start microphone capture');
    }
    if (remainingBudget() === 0) return timeoutResult();

    const key = identityKey;
    let resolveCancellation!: () => void;
    const cancellation = new Promise<AudioOperationResultDto>((resolve) => {
      resolveCancellation = () => resolve(failedAudio('cancelled', 'the audio operation was cancelled'));
    });
    const inFlight: InFlightAudioRequest = {
      request,
      cancelled: false,
      timedOut: false,
      resolveCancellation,
      cancellation,
    };
    this.pendingAudioRequests.set(key, inFlight);
    let resolveTimeout!: (result: AudioOperationResultDto) => void;
    const timeout = deadlineAt === null ? null : new Promise<AudioOperationResultDto>((resolve) => {
      resolveTimeout = resolve;
    });
    let helperCancellationSent = false;
    const cancelAdmittedOperationForTimeout = (context: string): void => {
      const helperId = inFlight.helperId;
      if (!helperId || helperCancellationSent) return;
      helperCancellationSent = true;
      const pending = this.pending.get(helperId);
      if (pending) {
        clearTimeout(pending.timer);
        this.pending.delete(helperId);
        if (pending.kind === 'engine') {
          pending.resolve({ type: 'engine_result', snapshot: this.getSnapshot(), result: admittedTimeoutResult() });
        } else {
          pending.reject(new Error('the audio operation deadline expired after native admission'));
        }
      }
      void this.cancelHelperOperation(request.identity, context);
    };
    const timeoutTimer = deadlineAt === null ? undefined : setTimeout(() => {
      if (inFlight.cancelled || inFlight.timedOut) return;
      inFlight.timedOut = true;
      const result = inFlight.helperId ? admittedTimeoutResult() : timeoutResult();
      resolveTimeout(result);
      if (inFlight.helperId) {
        cancelAdmittedOperationForTimeout('native audio deadline cancellation');
      }
    }, Math.max(0, deadlineAt - performance.now()));
    const terminalFailure = (): AudioOperationResultDto | null => {
      if ((this.audioSuspending || this.suspensionRevision !== suspensionRevision)
        && request.operation.type !== 'end_owner' && request.operation.type !== 'status') {
        return failedAudio('cancelled', 'audio was suspended because the desktop window is hidden');
      }
      if (shouldContinue && !shouldContinue()) inFlight.cancelled = true;
      if (inFlight.cancelled) return failedAudio('cancelled', 'the audio operation was cancelled');
      if (inFlight.timedOut || remainingBudget() === 0) {
        inFlight.timedOut = true;
        if (inFlight.helperId) cancelAdmittedOperationForTimeout('native audio deadline cancellation');
        return inFlight.helperId ? admittedTimeoutResult() : timeoutResult();
      }
      return null;
    };

    const execute = async (): Promise<AudioOperationResultDto> => {
      try {
        await this.ensureHelper();
        const afterHelperStartup = terminalFailure();
        if (afterHelperStartup) return afterHelperStartup;
        if (request.operation.type === 'start_recording' && this.opts.isPackaged) {
          await this.resolvePackagedPermissions(['microphone'], () => terminalFailure() === null);
        }
        const afterMicrophonePermission = terminalFailure();
        if (afterMicrophonePermission) return afterMicrophonePermission;
        if (
          request.operation.type === 'listen'
          && this.opts.isPackaged
        ) {
          await this.resolvePackagedPermissions(['microphone'], () => terminalFailure() === null);
          const afterMicrophonePermission = terminalFailure();
          if (afterMicrophonePermission) return afterMicrophonePermission;
          if (operationConfiguration.recognition.source === 'automatic' || operationConfiguration.recognition.source === 'system') {
            await this.resolvePackagedPermissions(['speech'], () => terminalFailure() === null);
          }
        }
        const beforeNativeAdmission = terminalFailure();
        if (beforeNativeAdmission) return beforeNativeAdmission;
        if (
          (request.operation.type === 'start_recording' || request.operation.type === 'listen')
          && this.opts.isForeground
          && !this.opts.isForeground()
        ) {
          return failedAudio('unavailable', 'a foreground desktop window is required to start microphone capture');
        }

        const budget = remainingBudget();
        const helperBudget = budget === undefined ? undefined : Math.floor(budget);
        if (helperBudget === 0) {
          inFlight.timedOut = true;
          return timeoutResult();
        }
        const id = randomUUID();
        inFlight.helperId = id;
        const responseTimeoutMs = request.operation.type === 'speak' ? SPEAK_TIMEOUT_MS : REQUEST_TIMEOUT_MS;
        const responsePromise = this.sendHelperEnvelope({
          id,
          kind: 'engine_request',
          request: helperBudget === undefined ? request : { ...request, timeout_budget_ms: helperBudget },
          configuration: operationConfiguration,
          configurationRevision: operationConfigurationRevision,
          ...(request.operation.type === 'start_recording' && this.streamCaptureOwners.has(audioOwnerKey(request.owner)) ? { streamPcm: true } : {}),
        }, null, helperBudget === undefined ? responseTimeoutMs : Math.min(helperBudget, responseTimeoutMs));
        if (onNativeAdmission) {
          try {
            const notification = onNativeAdmission();
            if (notification && typeof notification.then === 'function') {
              void notification.catch((error: unknown) => {
                this.opts.diagnostics.add('warn', 'host', `native audio admission callback failed: ${String(error)}`);
              });
            }
          } catch (error) {
            this.opts.diagnostics.add('warn', 'host', `native audio admission callback failed: ${String(error)}`);
          }
        }
        const response = await responsePromise;
        const afterNativeOperation = terminalFailure();
        if (afterNativeOperation) return afterNativeOperation;
        if (response.type !== 'engine_result') {
          return response.type === 'error'
            ? failedAudio(nativeErrorKind(response.error.code), response.error.message)
            : failedAudio('native_failure', 'the audio helper returned a command response for an operation request');
        }
        if (response.result.type === 'failed' && response.result.error.kind === 'timeout') {
          inFlight.timedOut = true;
          cancelAdmittedOperationForTimeout('native engine timeout cancellation');
          return admittedTimeoutResult();
        }
        if (sameIdentity(response.snapshot.currentOperation?.identity, request.identity) || isCurrentOrIdleSnapshot(response.snapshot, request)) {
          this.applySnapshot(response.snapshot);
        }
        return response.result;
      } catch (error) {
        const terminal = terminalFailure();
        if (terminal) return terminal;
        const message = error instanceof Error ? error.message : String(error);
        if (inFlight.helperId && /timed out/i.test(message)) {
          inFlight.timedOut = true;
          cancelAdmittedOperationForTimeout('native helper timeout cancellation');
          return admittedTimeoutResult();
        }
        this.opts.diagnostics.add('error', 'host', 'native engine audio request failed: ' + String(error));
        return failedAudio(/timed out/i.test(message) ? 'timeout' : 'native_failure', message);
      }
    };
    try {
      const races: Promise<AudioOperationResultDto>[] = [execute(), cancellation];
      if (timeout) races.push(timeout);
      const result = await Promise.race(races);
      if (result.type === 'recording_started' && request.operation.type === 'start_recording') {
        this.activeRecordingOrigins.set(key, { owner: request.owner, configurationRevision: operationConfigurationRevision });
      } else if (
        result.type !== 'failed'
        && (request.operation.type === 'stop_recording' || request.operation.type === 'end_owner'
          || (request.operation.type === 'status' && result.type === 'status' && !result.status.recording))
      ) {
        const ownerKey = audioOwnerKey(request.owner);
        for (const [identity, recording] of this.activeRecordingOrigins) {
          if (audioOwnerKey(recording.owner) === ownerKey) this.activeRecordingOrigins.delete(identity);
        }
      }
      return result;
    } finally {
      if (timeoutTimer) clearTimeout(timeoutTimer);
      if (this.pendingAudioRequests.get(key) === inFlight) this.pendingAudioRequests.delete(key);
    }
  }

  async executeUiAudioOperation(
    value: unknown,
    instanceId: string,
    requestedConfigurationRevision?: number,
    configurationOverride?: unknown,
  ): Promise<{ snapshot: NativeAudioSnapshot; result: AudioOperationResultDto }> {
    const operation: AudioOperationDto = validateNativeAudioOperation(value);
    const suspensionRevision = this.suspensionRevision;
    if (!instanceId || instanceId.length > 128) throw new Error('invalid desktop UI audio instance');
    const cancellation: UiOperationCancellation = { cancelled: false };
    let uiOperations = this.uiOperationsByInstance.get(instanceId);
    if (!uiOperations) {
      uiOperations = new Set();
      this.uiOperationsByInstance.set(instanceId, uiOperations);
    }
    uiOperations.add(cancellation);
    const listenOperation: UiListenOperation | undefined = operation.type === 'listen'
      ? { finishRequested: false, finishAcknowledged: false, admitted: false }
      : undefined;
    if (listenOperation) {
      const listenOperations = this.uiListenOperations.get(instanceId) ?? [];
      listenOperations.push(listenOperation);
      this.uiListenOperations.set(instanceId, listenOperations);
    }
    try {
      if (!this.helperPath) {
        return {
          snapshot: this.getSnapshot(),
          result: failedAudio('unavailable', this.snapshot.helper.message ?? 'native audio helper is unavailable'),
        };
      }
      try {
        await this.ensureCapabilities();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        this.opts.diagnostics.add('error', 'host', `native audio capability initialization failed: ${message}`);
        return { snapshot: this.getSnapshot(), result: failedAudio('unavailable', message) };
      }
      if (this.audioSuspending || this.suspensionRevision !== suspensionRevision) {
        return { snapshot: this.getSnapshot(), result: failedAudio('cancelled', 'audio was suspended because the desktop window is hidden') };
      }
      if (cancellation.cancelled) {
        return { snapshot: this.getSnapshot(), result: failedAudio('cancelled', 'the audio operation was cancelled') };
      }
      if (listenOperation && !this.uiListenOperations.get(instanceId)?.includes(listenOperation)) {
        return { snapshot: this.getSnapshot(), result: failedAudio('cancelled', 'a newer UI listen operation superseded this request') };
      }
      const requestedKind = operationKind(operation);
      if (requestedKind && !this.getCapabilities().supported_operations.includes(requestedKind)) {
        return {
          snapshot: this.getSnapshot(),
          result: failedAudio('unsupported', 'this device does not support the requested audio operation'),
        };
      }
      let configurationSnapshot: { configuration: AudioConfigurationV4; revision: number } | undefined;
      if (configurationOverride !== undefined) {
        if (requestedConfigurationRevision !== undefined) {
          throw new Error('an audio configuration override cannot be combined with a saved configuration revision');
        }
        if (typeof configurationOverride !== 'object' || configurationOverride === null || Array.isArray(configurationOverride)) {
          throw new Error('invalid audio configuration override');
        }
        const rawConfiguration = configurationOverride as Record<string, unknown>;
        if (rawConfiguration['schemaVersion'] !== 4) throw new Error('invalid audio configuration override version');
        configurationSnapshot = {
          configuration: normalizeAudioConfiguration(rawConfiguration),
          revision: 0,
        };
      } else if (requestedConfigurationRevision !== undefined) {
        if (!Number.isSafeInteger(requestedConfigurationRevision) || requestedConfigurationRevision < 0) {
          throw new Error('invalid audio configuration revision');
        }
        const currentPin = this.uiConfigurationPins.get(instanceId);
        if (currentPin?.revision === requestedConfigurationRevision) {
          configurationSnapshot = currentPin;
        } else {
          const currentRevision = this.opts.getAudioConfigurationRevision?.() ?? 0;
          if (requestedConfigurationRevision !== currentRevision) {
            throw new Error('audio configuration changed before this Flow turn began; start a new listen operation');
          }
          configurationSnapshot = {
            configuration: this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults(),
            revision: currentRevision,
          };
          this.uiConfigurationPins.set(instanceId, configurationSnapshot);
        }
      }

      const previous = this.uiGenerationByInstance.get(instanceId) ?? 0;
      if (previous >= Number.MAX_SAFE_INTEGER) throw new Error('desktop UI audio operation sequence is exhausted');
      const generation = previous + 1;
      this.uiGenerationByInstance.set(instanceId, generation);
      const capabilities = this.getCapabilities();
      const identity: AudioOperationIdDto = {
        id: randomUUID(),
        generation,
        service_epoch: capabilities.service_epoch,
      };
      if (listenOperation) {
        listenOperation.identity = identity;
      }
      if (listenOperation && !this.uiListenOperations.get(instanceId)?.includes(listenOperation)) {
        return { snapshot: this.getSnapshot(), result: failedAudio('cancelled', 'a newer UI listen operation superseded this request') };
      }
      const result = await this.executeAudioRequest({
        identity,
        owner: { type: 'ui', instance_id: instanceId },
        max_payload_bytes: capabilities.max_payload_bytes,
        operation,
      }, configurationSnapshot, listenOperation ? () => {
        listenOperation.admitted = true;
        if (listenOperation.finishRequested) {
          void this.sendUiListenFinish(instanceId, listenOperation).catch((error: unknown) => {
            this.opts.diagnostics.add('warn', 'host', `UI listen finish request failed: ${String(error)}`);
          });
        }
      } : undefined, () => !cancellation.cancelled);
      return { snapshot: this.getSnapshot(), result };
    } finally {
      if (listenOperation) {
        const listenOperations = this.uiListenOperations.get(instanceId);
        if (listenOperations) {
          const index = listenOperations.indexOf(listenOperation);
          if (index >= 0) listenOperations.splice(index, 1);
          if (listenOperations.length === 0 && this.uiListenOperations.get(instanceId) === listenOperations) {
            this.uiListenOperations.delete(instanceId);
          }
        }
      }
      uiOperations.delete(cancellation);
      if (uiOperations.size === 0 && this.uiOperationsByInstance.get(instanceId) === uiOperations) {
        this.uiOperationsByInstance.delete(instanceId);
      }
    }
  }

  async finishUiAudioListen(instanceId: string): Promise<void> {
    const captureFinish = this.captureFinishes.get(`ui:${instanceId}`);
    if (captureFinish) { captureFinish(); return; }
    if (!instanceId || instanceId.length > 128) return;
    const listenOperation = this.uiListenOperations.get(instanceId)?.find((operation) => !operation.finishAcknowledged);
    if (!listenOperation) return;
    listenOperation.finishRequested = true;
    if (!listenOperation.identity || !listenOperation.admitted) return;
    try {
      await this.sendUiListenFinish(instanceId, listenOperation);
    } catch (error) {
      this.opts.diagnostics.add('warn', 'host', `UI listen finish request failed: ${String(error)}`);
      if (this.uiListenOperations.get(instanceId)?.includes(listenOperation)) throw error;
    }
  }

  private sendUiListenFinish(instanceId: string, listenOperation: UiListenOperation): Promise<void> {
    const captureFinish = this.captureFinishes.get(`ui:${instanceId}`);
    if (captureFinish) { captureFinish(); listenOperation.finishAcknowledged = true; return Promise.resolve(); }
    if (!listenOperation.identity) return Promise.resolve();
    if (listenOperation.finishCommand) return listenOperation.finishCommand;
    const identity = listenOperation.identity;
    const finishCommand = this.sendHelperEnvelope({
      id: randomUUID(),
      kind: 'command',
      command: {
        type: 'finish_listening',
        owner: { kind: 'ui', id: instanceId },
        identity,
      },
    }, { kind: 'ui', id: instanceId }).then((response) => {
      if (response.type === 'error') throw new Error(response.error.message);
      if (response.type === 'listening_finished') {
        listenOperation.finishAcknowledged = true;
        const snapshot = response.snapshot;
        if (sameIdentity(snapshot.currentOperation?.identity, identity) || (snapshot.activity === 'idle' && snapshot.owner === null)) {
          this.applySnapshot(snapshot);
        }
      }
    }).catch((error: unknown) => {
      if (listenOperation.finishCommand === finishCommand) listenOperation.finishCommand = undefined;
      throw error;
    });
    listenOperation.finishCommand = finishCommand;
    return finishCommand;
  }

  async cancelUiAudioOperations(instanceId: string, releaseConfigurationPin = false): Promise<void> {
    if (!instanceId || instanceId.length > 128) return;
    const uiOperations = this.uiOperationsByInstance.get(instanceId);
    const ownerKey = `ui:${instanceId}`;
    for (const operation of this.hostedOperations.values()) if (audioOwnerKey(operation.owner) === ownerKey) operation.controller.abort();
    const hasActiveOwner = (this.snapshot.owner?.kind === 'ui' && this.snapshot.owner.id === instanceId)
      || (this.snapshot.currentOperation?.owner.type === 'ui' && this.snapshot.currentOperation.owner.instance_id === instanceId)
      || [...this.pendingAudioRequests.values()].some((pending) => audioOwnerKey(pending.request.owner) === ownerKey)
      || [...this.activeRecordingOrigins.values()].some((recording) => audioOwnerKey(recording.owner) === ownerKey);
    for (const operation of uiOperations ?? []) operation.cancelled = true;
    this.uiListenOperations.delete(instanceId);
    try {
      if (uiOperations?.size || hasActiveOwner) {
        await this.endAudioOwner({ type: 'ui', instance_id: instanceId });
      }
    } finally {
      if (releaseConfigurationPin) this.uiConfigurationPins.delete(instanceId);
    }
  }

  async cancelAudioRequest(value: unknown): Promise<void> {
    const identity = validateNativeAudioOperationIdentity(value);
    const key = audioIdentityKey(identity);
    const hosted = this.hostedOperations.get(key);
    if (hosted) { hosted.controller.abort(); return; }
    const inFlight = this.pendingAudioRequests.get(key);
    if (!inFlight || !sameIdentity(inFlight.request.identity, identity)) {
      this.rememberAudioIdentity(key);
      if (this.activeRecordingOrigins.has(key)
        && await this.cancelHelperOperation(identity, 'native completed recording rollback')) {
        this.activeRecordingOrigins.delete(key);
      }
      return;
    }
    inFlight.cancelled = true;
    inFlight.resolveCancellation();
    const helperId = inFlight.helperId;
    if (helperId) {
      const pending = this.pending.get(helperId);
      if (pending) {
        clearTimeout(pending.timer);
        this.pending.delete(helperId);
      }
      if (pending) {
        if (pending.kind === 'engine') {
          pending.resolve({
            type: 'engine_result',
            snapshot: this.getSnapshot(),
            result: failedAudio('cancelled', 'the audio operation was cancelled'),
          });
        } else {
          pending.reject(new Error('audio operation was cancelled'));
        }
      }
      if (this.pendingAudioRequests.get(key) === inFlight) this.pendingAudioRequests.delete(key);
      if (!this.helperPath || !this.helper?.stdin?.writable) return;
      try {
        const response = await this.sendHelperEnvelope({
          id: randomUUID(),
          kind: 'command',
          command: { type: 'cancel_operation', identity },
        }, null);
        if (response.type === 'error') {
          this.opts.diagnostics.add('warn', 'host', 'native audio cancellation failed: ' + response.error.message);
        }
      } catch (error) {
        this.opts.diagnostics.add('warn', 'host', 'native audio cancellation failed: ' + String(error));
      }
    }
  }

  private async cancelHelperOperation(identity: AudioOperationIdDto, context: string): Promise<boolean> {
    if (!this.helperPath || !this.helper?.stdin?.writable) return false;
    try {
      const response = await this.sendHelperEnvelope({
        id: randomUUID(),
        kind: 'command',
        command: { type: 'cancel_operation', identity },
      }, null);
      if (response.type === 'error') {
        this.opts.diagnostics.add('warn', 'host', `${context} failed: ${response.error.message}`);
        return false;
      }
      if (response.type !== 'cancelled') {
        this.opts.diagnostics.add('warn', 'host', `${context} returned an unexpected response: ${response.type}`);
        return false;
      }
      return true;
    } catch (error) {
      this.opts.diagnostics.add('warn', 'host', `${context} failed: ${String(error)}`);
      return false;
    }
  }

  async endAudioOwner(owner: AudioOwnerDto): Promise<void> {
    if (await this.endAudioOwnerWithResult(owner)) return;
    this.stopHelperForTeardown('owner teardown failed');
    throw new Error('native audio owner could not be ended');
  }

  private async endAudioOwnerWithResult(owner: AudioOwnerDto): Promise<boolean> {
    const ownerKey = audioOwnerKey(owner);
    for (const operation of this.hostedOperations.values()) if (audioOwnerKey(operation.owner) === ownerKey) operation.controller.abort();
    for (const pending of [...this.pendingAudioRequests.values()]) {
      if (audioOwnerKey(pending.request.owner) === ownerKey) {
        await this.cancelAudioRequest(pending.request.identity);
      }
    }
    try {
      await this.ensureCapabilities();
    } catch (error) {
      this.opts.diagnostics.add('warn', 'host', `audio owner teardown could not refresh helper capabilities: ${String(error)}`);
      return false;
    }
    const capabilities = this.getCapabilities();
    const result = await this.executeAudioRequest({
      identity: { id: randomUUID(), generation: 1, service_epoch: capabilities.service_epoch },
      owner,
      max_payload_bytes: capabilities.max_payload_bytes,
      operation: { type: 'end_owner' },
    });
    if (result.type !== 'failed') {
      for (const [identity, recording] of this.activeRecordingOrigins) {
        if (audioOwnerKey(recording.owner) === ownerKey) this.activeRecordingOrigins.delete(identity);
      }
    }
    if (result.type === 'failed' && result.error.kind !== 'unavailable') {
      this.opts.diagnostics.add('warn', 'host', `audio owner teardown failed (${result.error.kind}): ${result.error.message}`);
    }
    return result.type !== 'failed';
  }

  private rememberAudioIdentity(identityKey: string): void {
    if (this.seenAudioIdentities.has(identityKey)) return;
    this.seenAudioIdentities.add(identityKey);
    this.audioIdentityHistory.push(identityKey);
    if (this.audioIdentityHistory.length > 4_096) {
      const oldest = this.audioIdentityHistory.shift();
      if (oldest) this.seenAudioIdentities.delete(oldest);
    }
  }

  suspend(reason: string): Promise<void> {
    if (this.suspensionTask) return this.suspensionTask;
    for (const operation of this.hostedOperations.values()) operation.controller.abort();
    this.audioSuspending = true;
    this.suspensionRevision += 1;
    const task = this.performSuspend(reason);
    this.suspensionTask = task;
    const clear = (): void => {
      if (this.suspensionTask === task) {
        this.suspensionTask = null;
        this.audioSuspending = false;
      }
    };
    void task.then(clear, clear);
    return task;
  }

  private async performSuspend(reason: string): Promise<void> {
    let timedOut = false;
    let timeoutTimer: NodeJS.Timeout | undefined;
    const deadline = new Promise<void>((resolve) => {
      timeoutTimer = setTimeout(() => {
        timedOut = true;
        this.stopHelperForTeardown(`suspend (${reason}) timed out`);
        resolve();
      }, this.opts.suspendTeardownTimeoutMs ?? SUSPEND_TEARDOWN_TIMEOUT_MS);
    });
    try {
      const cleanup = async (): Promise<void> => {
        const owners = new Map<string, AudioOwnerDto>();
        let hasUnmappableOwner = false;
        const remember = (owner: AudioOwnerDto): void => { owners.set(audioOwnerKey(owner), owner); };
        const rememberSnapshotOwner = (owner: NativeAudioOwner | null): void => {
          if (!owner) return;
          const mapped = audioOwnerFromSnapshotOwner(owner);
          if (mapped) remember(mapped);
          else hasUnmappableOwner = true;
        };

        for (const pending of this.pendingAudioRequests.values()) remember(pending.request.owner);
        for (const recording of this.activeRecordingOrigins.values()) remember(recording.owner);
        for (const pending of this.pending.values()) {
          if (pending.kind === 'command' && pending.owner) rememberSnapshotOwner(pending.owner);
        }
        if (this.snapshot.currentOperation) remember(this.snapshot.currentOperation.owner);
        rememberSnapshotOwner(this.snapshot.owner);

        for (const pending of [...this.pendingAudioRequests.values()]) {
          await this.cancelAudioRequest(pending.request.identity);
          if (timedOut) return;
        }
        if (!this.helper) {
          this.releaseOwner();
          return;
        }
        let ownerTeardownFailed = false;
        for (const owner of owners.values()) {
          if (timedOut) return;
          if (!await this.endAudioOwnerWithResult(owner)) ownerTeardownFailed = true;
        }
        if (!timedOut && (hasUnmappableOwner || ownerTeardownFailed)) {
          this.stopHelperForTeardown(`suspend (${reason}) could not confirm owner teardown`);
        }
      };
      await Promise.race([cleanup(), deadline]);
    } catch (error) {
      this.stopHelperForTeardown(`suspend (${reason}) failed`);
      throw error;
    } finally {
      if (timeoutTimer) clearTimeout(timeoutTimer);
    }
  }

  private stopHelperForTeardown(reason: string): void {
    const helper = this.helper;
    if (!helper) return;
    this.opts.diagnostics.add('warn', 'host', `native audio ${reason}; stopping the helper to release audio resources`);
    this.suspendedHelpers.add(helper as object);
    for (const pending of [...this.pendingAudioRequests.values()]) {
      pending.cancelled = true;
      pending.resolveCancellation();
    }
    this.pendingAudioRequests.clear();
    this.activeRecordingOrigins.clear();
    this.failPending(`native audio helper stopped during ${reason}`);
    helper.kill();
  }

  async dispose(): Promise<void> {
    for (const operation of this.hostedOperations.values()) operation.controller.abort();
    this.streamCaptureOwners.clear();
    this.streamCaptureIdentities.clear();
    this.realtimeRecordingHandles.clear();
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new Error('native audio helper disposed'));
      this.pending.delete(id);
    }
    this.helper?.kill();
    this.helper = null;
    this.helperStartup = null;
    this.capabilitiesHelper = null;
    this.capabilityInitialization = null;
    this.helperStdoutBuffer = '';
    this.releaseOwner();
    this.setHelperState('stopped', 'native audio helper stopped');
  }

  private unavailableResponse(): NativeAudioResponse {
    return this.errorResponse('unavailable', this.snapshot.helper.message ?? 'native audio helper is unavailable');
  }

  private errorResponse(code: NativeAudioErrorCode, message: string): NativeAudioResponse {
    return {
      type: 'error',
      snapshot: this.getSnapshot(),
      error: { code, message },
    };
  }

  private normalizeCommandResponse(response: NativeAudioResponse | NativeAudioEngineResponse): NativeAudioResponse {
    if (response.type === 'engine_result') {
      this.applySnapshot(response.snapshot);
      return response.result.type === 'failed'
        ? {
            type: 'error',
            snapshot: this.getSnapshot(),
            error: { code: 'native-error', message: response.result.error.message },
          }
        : { type: 'snapshot', snapshot: this.getSnapshot() };
    }
    this.applySnapshot(response.snapshot);
    return { ...response, snapshot: this.getSnapshot() };
  }

  private emit(event: NativeAudioEvent): void {
    for (const subscriber of this.subscribers) {
      try {
        subscriber(event);
      } catch (error) {
        this.opts.diagnostics.add('warn', 'host', `native audio event subscriber failed: ${String(error)}`);
      }
    }
  }

  private applySnapshot(snapshot: NativeAudioSnapshot): void {
    const previousCapabilities = JSON.stringify(this.snapshot.capabilities ?? null);
    this.snapshot = cloneSnapshot(snapshot);
    if (JSON.stringify(this.snapshot.capabilities ?? null) !== previousCapabilities) {
      this.emit({ type: 'snapshot_changed', snapshot: this.getSnapshot() });
    }
  }

  private releaseOwner(): void {
    if (
      !this.snapshot.owner
      && this.snapshot.activity === 'idle'
      && !this.snapshot.currentOperation
      && this.snapshot.activeOperationCount === 0
      && this.snapshot.pendingOperationCount === 0
      && this.snapshot.activeRecordingCount === 0
      && this.snapshot.activePlaybackCount === 0
      && this.snapshot.activeModelReferenceCount === 0
    ) return;
    this.snapshot = {
      ...this.snapshot,
      owner: null,
      activity: 'idle',
      currentOperation: undefined,
      activeOperationCount: 0,
      pendingOperationCount: 0,
      activeRecordingCount: 0,
      activePlaybackCount: 0,
      activeModelReferenceCount: 0,
    };
    const cloned = this.getSnapshot();
    this.emit({ type: 'owner_changed', snapshot: cloned, owner: null });
    this.emit({ type: 'snapshot_changed', snapshot: cloned });
  }

  private setHelperState(state: NativeAudioSnapshot['helper']['state'], message?: string): void {
    this.snapshot = {
      ...this.snapshot,
      helper: {
        state,
        ...(message ? { message } : {}),
      },
    };
    const snapshot = this.getSnapshot();
    this.emit({ type: 'helper_state', snapshot, state, ...(message ? { message } : {}) });
    this.emit({ type: 'snapshot_changed', snapshot });
  }

  private async ensureHelper(): Promise<void> {
    if (this.helper) return;
    if (this.helperStartup) return this.helperStartup;
    if (!this.helperPath) throw new Error(this.snapshot.helper.message ?? 'native audio helper is unavailable');
    this.helperStartup = this.startHelper();
    try {
      await this.helperStartup;
    } finally {
      this.helperStartup = null;
    }
  }

  private async ensureCapabilities(timeoutMs = REQUEST_TIMEOUT_MS): Promise<void> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('native audio capability initialization timed out')), Math.max(0, timeoutMs));
    });
    try {
      // The shared refresh has its own lifetime. A caller timing out must not
      // cancel it or inherit another caller's deadline.
      await Promise.race([this.initializeHelperCapabilities(), timeout]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  private async initializeHelperCapabilities(): Promise<void> {
    if (!this.helperPath) return;
    await this.ensureHelper();
    const helper = this.helper;
    if (!helper) throw new Error('native audio helper failed to start');
    if (this.capabilitiesHelper === helper) return;
    if (this.capabilityInitialization) {
      try {
        await this.capabilityInitialization;
      } catch (error) {
        if (this.helper !== helper) return this.initializeHelperCapabilities();
        throw error;
      }
      if (this.helper !== helper) return this.initializeHelperCapabilities();
      if (this.capabilitiesHelper === helper) return;
    }
    const initialization = (async () => {
      const snapshot = await this.refreshHelperSnapshot(REQUEST_TIMEOUT_MS);
      if (this.helper !== helper) throw new Error('native audio helper restarted while refreshing capabilities');
      if (!snapshot.capabilities) throw new Error('native audio helper did not return its capabilities');
      this.capabilitiesHelper = helper;
    })();
    this.capabilityInitialization = initialization;
    try {
      await initialization;
    } finally {
      if (this.capabilityInitialization === initialization) this.capabilityInitialization = null;
    }
  }

  private helperAppPath(): string {
    if (!this.helperPath) throw new Error('native audio helper is unavailable');
    return resolve(this.helperPath, '..', '..', '..');
  }

  private async refreshHelperSnapshot(timeoutMs = REQUEST_TIMEOUT_MS): Promise<NativeAudioSnapshot> {
    const response = await this.sendHelperEnvelope({
      id: randomUUID(),
      kind: 'command',
      command: { type: 'get_snapshot' },
    }, null, timeoutMs);
    const normalized = this.normalizeCommandResponse(response);
    if (normalized.type === 'error') throw new Error(normalized.error.message);
    return this.getSnapshot();
  }

  private async resolvePackagedPermissions(
    permissions: AudioPermission[],
    shouldContinue: () => boolean = () => true,
  ): Promise<NativeAudioSnapshot> {
    if (!shouldContinue()) return this.getSnapshot();
    if (this.permissionResolution) {
      await this.permissionResolution;
      if (!shouldContinue()) return this.getSnapshot();
      return this.resolvePackagedPermissions(permissions, shouldContinue);
    }
    const resolution = this.performPackagedPermissionResolution(permissions, shouldContinue);
    this.permissionResolution = resolution;
    try {
      return await resolution;
    } finally {
      if (this.permissionResolution === resolution) this.permissionResolution = null;
    }
  }

  private async performPackagedPermissionResolution(
    permissions: AudioPermission[],
    shouldContinue: () => boolean = () => true,
  ): Promise<NativeAudioSnapshot> {
    if (!shouldContinue()) return this.getSnapshot();
    if (permissions.includes('microphone')) {
      if (!this.opts.requestMicrophoneAccess) {
        throw new Error('outer application microphone authorization is unavailable');
      }
      await this.opts.requestMicrophoneAccess();
      if (!shouldContinue()) return this.getSnapshot();
    }
    const before = await this.refreshHelperSnapshot();
    if (!shouldContinue()) return before;
    const unresolved = permissions.filter((permission, index) => {
      if (permissions.indexOf(permission) !== index) return false;
      if (permission === 'microphone') return before.permissions.microphone === 'prompt';
      return before.permissions.speech === 'not_determined';
    });
    if (unresolved.length === 0) return before;
    if (!shouldContinue()) return before;
    await this.launchPermissionHelper(this.helperAppPath(), unresolved);
    if (!shouldContinue()) return this.getSnapshot();
    return this.refreshHelperSnapshot();
  }

  private async startHelper(): Promise<void> {
    this.setHelperState('starting');
    let child: HelperProcess;
    try {
      this.verifyHelper();
      const env = {
        PATH: process.env['PATH'],
        TMPDIR: process.env['TMPDIR'],
        TMP: process.env['TMP'],
        TEMP: process.env['TEMP'],
        LANG: process.env['LANG'],
        LC_ALL: process.env['LC_ALL'],
        LINGXI_AUDIO_MODELS_ROOT: this.storageRoot,
      };
      child = this.spawnHelper(this.helperPath!, ['--jsonl'], env);
      if (!child.stdin || !child.stdout || !child.stderr) {
        child.kill();
        throw new Error('native audio helper stdio is unavailable');
      }
    } catch (error) {
      this.setHelperState('failed', error instanceof Error ? error.message : String(error));
      throw error;
    }
    this.helper = child;
    const stdoutDecoder = new StringDecoder('utf8');
    child.stdout.on('data', (chunk: Buffer | string) => {
      if (this.helper !== child) return;
      this.helperStdoutBuffer += typeof chunk === 'string' ? chunk : stdoutDecoder.write(chunk);
      this.drainStdout(child);
    });
    child.stderr.on('data', (chunk: Buffer | string) => {
      this.opts.diagnostics.add('warn', 'host', `audio-helper: ${chunk.toString().trim()}`);
    });
    child.stdin.on('error', (error: Error) => {
      this.failHelper(child, `native audio helper input failed: ${error.message}`);
    });
    child.once('error', (error: Error) => {
      this.failHelper(child, `native audio helper failed: ${error.message}`);
    });
    child.once('exit', (code, signal) => {
      if (this.helper !== child) return;
      const detail = `native audio helper exited (code=${String(code)} signal=${String(signal)})`;
      const suspended = this.suspendedHelpers.has(child as object);
      this.helper = null;
      if (this.capabilitiesHelper === child) this.capabilitiesHelper = null;
      this.helperStdoutBuffer = '';
      this.activeRecordingOrigins.clear();
      this.failPending(detail);
      this.releaseOwner();
      this.setHelperState(suspended || code === 0 ? 'stopped' : 'failed', detail);
    });
    this.setHelperState('running');
  }

  private failHelper(child: HelperProcess, detail: string): void {
    if (this.helper !== child) return;
    const suspended = this.suspendedHelpers.has(child as object);
    this.helper = null;
    if (this.capabilitiesHelper === child) this.capabilitiesHelper = null;
    this.helperStdoutBuffer = '';
    this.activeRecordingOrigins.clear();
    this.failPending(detail);
    this.releaseOwner();
    this.setHelperState(suspended ? 'stopped' : 'failed', detail);
    child.kill();
  }

  private verifyHelper(): void {
    if (!this.helperPath) return;
    if (!this.opts.isPackaged) return;
    const helperAppPath = this.helperAppPath();
    if (this.opts.verifyPackagedHelper) {
      this.opts.verifyPackagedHelper(helperAppPath);
      return;
    }
    const verification = spawnSync('/usr/bin/codesign', ['--verify', '--strict', helperAppPath], {
      encoding: 'utf8',
    });
    if (verification.status !== 0) throw new Error('packaged native audio helper signature is invalid');
    // `codesign -d` writes signing details to stderr by design.
    const description = spawnSync('/usr/bin/codesign', ['-dv', helperAppPath], { encoding: 'utf8' });
    if (description.status !== 0) throw new Error('packaged native audio helper signature cannot be inspected');
    const detail = `${description.stdout ?? ''}\n${description.stderr ?? ''}`;
    if (!/TeamIdentifier=AZ4AX7J833\b/.test(detail)) {
      throw new Error('packaged native audio helper is signed by the wrong Apple team');
    }
    if (!/Identifier=com\.lingxi\.code\.audio-helper(?:\.development)?\b/.test(detail)) {
      throw new Error('packaged native audio helper has an unexpected bundle identifier');
    }
  }

  private drainStdout(child: HelperProcess): void {
    for (;;) {
      const newline = this.helperStdoutBuffer.indexOf('\n');
      if (newline < 0) return;
      const line = this.helperStdoutBuffer.slice(0, newline).trim();
      this.helperStdoutBuffer = this.helperStdoutBuffer.slice(newline + 1);
      if (!line) continue;
      let payload: unknown;
      try {
        payload = JSON.parse(line);
      } catch (error) {
        this.opts.diagnostics.add('warn', 'host', `native audio helper emitted invalid JSON: ${String(error)}`);
        continue;
      }
      try {
        this.handleHelperEnvelope(payload);
      } catch (error) {
        this.failHelper(child, `native audio helper emitted an invalid envelope: ${String(error)}`);
        return;
      }
    }
  }

  private handleHelperEnvelope(value: unknown): void {
    const envelope = value as NativeAudioHelperEnvelope;
    if (envelope.type === 'event') {
      const event = validateNativeAudioEvent(envelope.event);
      if (event.type !== 'input_level' && event.type !== 'capture_chunk') this.applySnapshot(event.snapshot);
      if (event.type === 'capture_chunk' && (!this.streamCaptureOwners.has(`${event.owner.kind}:${event.owner.id}`) || !sameIdentity(this.streamCaptureIdentities.get(`${event.owner.kind}:${event.owner.id}`), event.identity))) return;
      this.emit(event);
      return;
    }
    if (typeof envelope.id !== 'string') return;
    const pending = this.pending.get(envelope.id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(envelope.id);
    try {
      if (envelope.type === 'error') {
        if (typeof envelope.error?.message !== 'string' || !envelope.error.message.trim()) {
          throw new Error('native audio helper returned an invalid error response');
        }
        const response = this.errorResponse('native-error', envelope.error.message);
        if (pending.kind === 'command') pending.resolve(response);
        else pending.resolve({
          type: 'engine_result',
          snapshot: this.getSnapshot(),
          result: failedAudio('native_failure', envelope.error.message),
        });
        return;
      }
      if ((envelope.result as { type?: string }).type === 'engine_result') {
        const response = validateNativeAudioEngineResponse(envelope.result);
        if (pending.kind === 'command') pending.resolve(this.normalizeCommandResponse(response));
        else if (sameIdentity(response.snapshot.currentOperation?.identity, pending.request.identity) || isCurrentOrIdleSnapshot(response.snapshot, pending.request)) {
          pending.resolve(response);
        } else {
          pending.resolve({ type: 'engine_result', snapshot: this.getSnapshot(), result: response.result });
        }
      } else {
        const response = validateNativeAudioResponse(envelope.result);
        if (pending.kind === 'command') pending.resolve(response);
        else pending.resolve({
          type: 'engine_result',
          snapshot: this.getSnapshot(),
          result: failedAudio('native_failure', 'the audio helper returned a command response for an operation request'),
        });
      }
    } catch (error) {
      pending.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private failPending(message: string): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.reject(new Error(message));
    }
  }

  private sendHelperEnvelope(
    envelope: NativeAudioHelperCommandEnvelope,
    owner: NativeAudioOwner | null,
    timeoutMs = REQUEST_TIMEOUT_MS,
  ): Promise<NativeAudioResponse | NativeAudioEngineResponse> {
    const helper = this.helper;
    if (!helper?.stdin?.writable) throw new Error('native audio helper is not writable');
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(envelope.id);
        reject(new Error(`native audio helper timed out waiting for ${envelope.kind}`));
      }, timeoutMs);
      this.pending.set(envelope.id, envelope.kind === 'engine_request'
        ? {
            kind: 'engine',
            request: envelope.request,
            resolve,
            reject,
            timer,
          }
        : {
            kind: 'command',
            owner,
            resolve,
            reject,
            timer,
          });
      try {
        helper.stdin.write(`${JSON.stringify(envelope)}\n`, (error) => {
          if (error) this.failHelper(helper, `native audio helper input failed: ${error.message}`);
        });
      } catch (error) {
        this.failHelper(helper, `native audio helper input failed: ${String(error)}`);
      }
    });
  }
}
