import { normalizeAudioUsage, type AudioUsageRecord } from '../../shared/audioUsage.js';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { StringDecoder } from 'node:string_decoder';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

import type {
  AudioErrorKindDto,
  AudioOperationDto,
  AudioOperationIdDto,
  AudioOperationRequestDto,
  AudioOperationResultDto,
  AudioOwnerDto,
  MediaSessionModeDto,
  MediaErrorKindDto,
  MediaDeviceRequestEnvelopeDto,
  MediaDeviceResponseDto,
} from '@lingxi/bridge-client';

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
  type NativeAudioCloseContext,
  type NativeAudioEngineResponse,
  type NativeAudioErrorCode,
  type NativeAudioEvent,
  type NativeAudioHelperCommandEnvelope,
  type NativeAudioHelperEnvelope,
  type NativeAudioOwner,
  type NativeAudioResponse,
  type NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';
import { nativeMediaOwnerKey, validateMediaDeviceRequest, validateMediaDeviceResponse } from '../../shared/nativeMedia.js';

const AUDIO_HELPER_APP = 'LingXiAudioHelper.app';
const AUDIO_HELPER_BIN = 'LingXiAudioHelper';
const AUDIO_HELPER_ENV = 'LINGXI_AUDIO_HELPER_BIN';
const REQUEST_TIMEOUT_MS = 5 * 60_000;
const SPEAK_TIMEOUT_MS = 20 * 60_000;
// Reserve room for stop/end/status commands even when uncertain work is retained.
const AUDIO_OPERATION_CAPACITY = 512;
const AUDIO_ORDINARY_OPERATION_CAPACITY = 384;
const SUSPEND_TEARDOWN_TIMEOUT_MS = 3_000;
const MEDIA_DEVICE_REQUEST_TIMEOUT_MS = 5 * 60_000;
const MEDIA_DEVICE_OPERATION_TIMEOUT_MS = 30_000;
const MEDIA_LATE_OPEN_CLEANUP_MS = 5 * 60_000;
const MAX_UNSETTLED_MEDIA_OPENS = 64;

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

interface PendingMediaDeviceRequest {
  kind: 'media';
  request: MediaDeviceRequestEnvelopeDto;
  resolve: (response: MediaDeviceResponseDto) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
  timedOut: boolean;
  helper: HelperProcess;
  flight?: MediaOpenFlight;
  lease?: MediaLeaseOrigin;
  lateCleanupTimer?: NodeJS.Timeout;
}

interface MediaLeaseOrigin {
  helper: HelperProcess;
  openRequestId: string;
  ownerKey: string;
  identity: { id: string; generation: number; service_epoch: number };
  leaseId: string;
}

interface MediaOpenFlight {
  helper: HelperProcess;
  requestId: string;
  ownerKey: string;
  identity: MediaLeaseOrigin['identity'];
  lease?: MediaLeaseOrigin;
  cleanup?: Promise<void>;
  resolveAfterCleanup?: () => void;
}

interface MediaDrainWaiter {
  resolve(): void;
  reject(error: Error): void;
  detach(): void;
}

interface InFlightAudioRequest {
  request: AudioOperationRequestDto;
  helperId?: string;
  helper?: HelperProcess;
  closeContext?: NativeAudioCloseContext;
  closeVerified?: boolean;
  cancelled: boolean;
  timedOut: boolean;
  resolveCancellation: () => void;
  cancellation: Promise<AudioOperationResultDto>;
}

interface AudioCloseBinding {
  request: AudioOperationRequestDto;
  helper: HelperProcess;
  context: NativeAudioCloseContext;
  inFlight: InFlightAudioRequest;
  flight?: Promise<void>;
  verified: boolean;
  onExactClose?: () => void;
}

interface ActiveRecordingOrigin {
  request: AudioOperationRequestDto;
  owner: AudioOwnerDto;
  helper?: HelperProcess;
  closeContext?: NativeAudioCloseContext;
  handle: string;
  configurationRevision: number;
}

// This is an in-process admission result, not a native close receipt.
export type MediaCaptureStartResult = AudioOperationResultDto & {
  captureAdmission?: 'never_admitted' | 'unknown';
  captureClosed?: Promise<void>;
};

export class MediaCaptureStopError extends Error {
  constructor(message: string, readonly cleanupConfirmed: boolean, readonly whenClosed?: Promise<void>) { super(message); }
}

interface MediaCaptureSlot {
  kind: 'capture' | 'owner';
  ready: Promise<void>;
  finishStart: () => void;
  stopping: boolean;
  admitted: boolean;
  identity?: AudioOperationIdDto;
  helper?: HelperProcess;
  handle?: string;
  closed?: Promise<void>;
  confirmClosed?: () => void;
  stop?: Promise<void>;
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

type PendingRequest = PendingCommandRequest | PendingEngineRequest | PendingMediaDeviceRequest;

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
  closeAckRetryMs?: number;
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
    ...(snapshot.mediaSessionModes === undefined ? {} : { mediaSessionModes: [...snapshot.mediaSessionModes] }),
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

function failedWithoutResource(kind: AudioErrorKindDto, message: string): AudioOperationResultDto {
  return { type: 'failed_without_resource', error: { kind, message } };
}

function mediaDeviceFailure(
  requestId: string,
  kind: MediaErrorKindDto,
  message: string,
): MediaDeviceResponseDto {
  let bounded = message;
  while (Buffer.byteLength(bounded, 'utf8') > 1_024) bounded = bounded.slice(0, -1);
  return { request_id: requestId, result: { type: 'error', error: { kind, message: bounded || 'Native media request failed' } } };
}

function mediaOpenFailedWithoutResource(requestId: string, kind: MediaErrorKindDto, message: string): MediaDeviceResponseDto {
  const failure = mediaDeviceFailure(requestId, kind, message);
  if (failure.result.type !== 'error') throw new Error('invalid media failure');
  return { request_id: requestId, result: { type: 'open_failed_without_resource', error: failure.result.error } };
}

function mediaDeviceTimeoutMs(request: MediaDeviceRequestEnvelopeDto): number {
  if (request.request.type === 'open') {
    return Math.min(request.request.request.timeout_budget_ms ?? MEDIA_DEVICE_OPERATION_TIMEOUT_MS, MEDIA_DEVICE_REQUEST_TIMEOUT_MS);
  }
  return request.request.type === 'next_input' ? MEDIA_DEVICE_REQUEST_TIMEOUT_MS : MEDIA_DEVICE_OPERATION_TIMEOUT_MS;
}

function audioIdentityKey(identity: AudioOperationIdDto): string {
  return `${identity.service_epoch}:${identity.generation}:${identity.id}`;
}

function audioOwnerKey(owner: AudioOwnerDto): string {
  switch (owner.type) {
    case 'session': return `session:${owner.session_id}`;
    case 'ui': return `ui:${owner.instance_id}`;
    case 'system': return `system:${owner.instance_id}`;
    case 'attached': return `attached:${JSON.stringify([owner.attachment_host_id, owner.attachment_generation, owner.logical_kind, owner.logical_owner_id])}`;
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
    case 'timeout': return 'timeout';
    case 'no-speech': return 'no_speech';
    case 'not-recording': return 'not_recording';
    case 'unsupported': return 'unsupported';
    case 'voice-missing': return 'voice_missing';
    case 'synthesis-failed': return 'synthesis_failed';
    case 'media-too-large': return 'media_too_large';
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
  private readonly mediaOpenFlights = new Map<string, MediaOpenFlight>();
  private readonly mediaLeases = new Map<string, MediaLeaseOrigin>();
  private readonly mediaDrainWaiters = new Set<MediaDrainWaiter>();
  private readonly seenMediaRequestIds = new Set<string>();
  private readonly mediaRequestHistory: string[] = [];
  private readonly pendingAudioRequests = new Map<string, InFlightAudioRequest>();
  // A submission ACK, timeout, or helper exit is not proof of native cleanup.
  private readonly uncertainAudioRequests = new Map<string, AudioOperationRequestDto>();
  private readonly audioCloseBindings = new Map<string, AudioCloseBinding>();
  private readonly pendingCloseAcks = new Map<string, { binding: AudioCloseBinding; timer?: NodeJS.Timeout }>();
  // Reservation begins before any asynchronous capability/permission work. A
  // duplicate cannot receive a no-resource result for another live request.
  private readonly preparingAudioIdentities = new Map<string, { owner: AudioOwnerDto; done: Promise<void>; finish(): void }>();
  private readonly reservedAudioAdmissions = new Map<string, { token: object; owner: AudioOwnerDto; done: Promise<void>; finish(): void }>();
  // Keep the admitting runtime's token for the entire native/hosted flight.
  // A duplicate identity from another runtime cannot cancel that flight.
  private readonly audioAdmissionClaims = new Map<string, object>();
  private readonly audioOwnerEpochs = new Map<string, number>();
  private readonly closingAudioOwners = new Map<string, string>();
  private readonly audioOwnerTeardowns = new Map<string, { helper: HelperProcess | null | undefined; task: Promise<boolean> }>();
  private readonly seenAudioIdentities = new Set<string>();
  private readonly audioIdentityHistory: string[] = [];
  private readonly activeRecordingOrigins = new Map<string, ActiveRecordingOrigin>();
  private readonly uiGenerationByInstance = new Map<string, number>();
  private readonly uiListenOperations = new Map<string, UiListenOperation[]>();
  private readonly uiOperationsByInstance = new Map<string, Set<UiOperationCancellation>>();
  private readonly uiConfigurationPins = new Map<string, { revision: number; configuration: AudioConfigurationV4 }>();
  private readonly suspendedHelpers = new WeakSet<object>();
  private readonly helperConnections = new WeakMap<object, string>();
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
  private readonly hostedOperations = new Map<string, { controller: AbortController; owner: AudioOwnerDto; request: AudioOperationRequestDto; done: Promise<void>; finish(): void; captureExactClose: boolean; onExactClose?: () => void }>();
  private readonly streamCaptureOwners = new Set<string>();
  private readonly streamCaptureIdentities = new Map<string, AudioOperationIdDto>();
  private streamGeneration = 0;
  private readonly realtimeRecordingHandles = new Map<string, string>();
  // Hold the original slot after an uncertain stop so an old native command
  // cannot overlap a new stream for the same owner through this manager.
  private readonly mediaCaptureSlots = new Map<string, MediaCaptureSlot>();
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

  private async executeHostedRequest(request: AudioOperationRequestDto, configuration: AudioConfigurationV4, revision: number, onAdmission?: () => void | Promise<void>, shouldContinue?: () => boolean, onExactClose?: () => void): Promise<AudioOperationResultDto> {
    const key = audioIdentityKey(request.identity);
    this.rememberAudioIdentity(key);
    const controller = new AbortController();
    let finishHosted!: () => void;
    const done = new Promise<void>(resolve => { finishHosted = resolve; });
    const hosted = { controller, owner: request.owner, request, done, finish: () => finishHosted(), captureExactClose: false, onExactClose };
    this.hostedOperations.set(key, hosted);
    controller.signal.addEventListener('abort', () => this.uncertainAudioRequests.set(key, request), { once: true });
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
      const abort = () => { void this.cancelOwnedAudioRequest(identity).catch(() => undefined); };
      controller.signal.addEventListener('abort', abort, { once: true });
      try { return await this.executeAudioRequest({ ...request, identity, operation }, { configuration: frozen, revision }, undefined, () => !controller.signal.aborted && (!shouldContinue || shouldContinue())); }
      finally { controller.signal.removeEventListener('abort', abort); }
    };
    const execute = async (): Promise<AudioOperationResultDto> => {
      const operation = request.operation;
      if (operation.type === 'listen') {
        if (!this.opts.providerAudio) return failedAudio('unavailable', 'Provider audio host is unavailable');
        const admissionFailure = await this.opts.providerAudio.preflight('recognition', frozen, session, controller.signal);
        if (admissionFailure) return admissionFailure;
      }
      if (shouldContinue && !shouldContinue()) return failedAudio('unavailable', 'previous audio cleanup for this owner remains unconfirmed');
      if (operation.type === 'capture' || operation.type === 'listen') {
        const started = await device({ type: 'start_recording', sample_rate_hz: operation.type === 'capture' ? operation.sample_rate_hz : 16_000, format: operation.type === 'capture' ? operation.format : 'wav' });
        if (started.type !== 'recording_started') {
          if (operation.type === 'capture' && controller.signal.aborted && started.type === 'failed_without_resource') hosted.captureExactClose = true;
          return started;
        }
        const ownerKey = audioOwnerKey(request.owner);
        let finish!: () => void;
        let durationTimer: NodeJS.Timeout | undefined;
        const stopped = new Promise<void>((resolve) => { finish = resolve; });
        if (this.captureFinishes.has(ownerKey)) { await device({ type: 'stop_recording', handle: started.handle }); return failedAudio('busy', 'another capture is active for this audio owner'); }
        this.captureFinishes.set(ownerKey, finish);
        durationTimer = setTimeout(finish, 15_000);
        controller.signal.addEventListener('abort', finish, { once: true });
        if (controller.signal.aborted) finish();
        try { await onAdmission?.(); await stopped; }
        finally { clearTimeout(durationTimer); controller.signal.removeEventListener('abort', finish); if (this.captureFinishes.get(ownerKey) === finish) this.captureFinishes.delete(ownerKey); }
        if (controller.signal.aborted) {
          // An owner teardown already owns the capture's exact close. Calling
          // it recursively would race that close and kill the shared helper.
          if (!this.closingAudioOwners.has(ownerKey)) {
            const origin = [...this.activeRecordingOrigins.values()].find(item => item.handle === started.handle && audioOwnerKey(item.owner) === ownerKey);
            if (origin && await this.cancelOwnedAudioRequest(origin.request.identity) === 'verified' && operation.type === 'capture') hosted.captureExactClose = true;
          }
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
    };
    try {
      const result = await execute();
      if (controller.signal.aborted || (result.type === 'failed' && result.error.kind === 'cancelled')) {
        this.uncertainAudioRequests.set(key, request);
        return failedAudio('unavailable', 'hosted audio cancellation was requested; cleanup remains unconfirmed');
      }
      return result.type === 'failed_without_resource' ? { type: 'failed', error: result.error } : result;
    } catch (error) {
      if (controller.signal.aborted) return failedAudio('unavailable', 'hosted audio cancellation was requested; cleanup remains unconfirmed');
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }

  async startMediaCapture(owner: AudioOwnerDto, sampleRateHz: number): Promise<MediaCaptureStartResult> {
    const ownerKey = audioOwnerKey(owner);
    if (this.mediaCaptureSlots.has(ownerKey)) {
      return { ...failedAudio('busy', 'the previous audio capture has not finished'), captureAdmission: 'never_admitted' };
    }
    let finishStart!: () => void;
    const ready = new Promise<void>((resolve) => { finishStart = resolve; });
    let confirmClosed!: () => void;
    const closed = new Promise<void>((resolve) => { confirmClosed = resolve; });
    const slot: MediaCaptureSlot = { kind: 'capture', ready, finishStart, stopping: false, admitted: false, closed, confirmClosed };
    this.mediaCaptureSlots.set(ownerKey, slot);
    try {
      try { await this.ensureCapabilities(); }
      catch (error) {
        return { ...failedAudio('unavailable', error instanceof Error ? error.message : String(error)), captureAdmission: 'never_admitted' };
      }
      if (slot.stopping) return { ...failedAudio('cancelled', 'capture was stopped before native admission'), captureAdmission: 'never_admitted' };
      this.streamCaptureOwners.add(ownerKey);
      const identity = { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch };
      this.streamCaptureIdentities.set(ownerKey, identity);
      slot.identity = identity;
      const result = await this.executeAudioRequest({ identity, owner,
        operation: { type: 'start_recording', sample_rate_hz: sampleRateHz, format: 'wav' }, max_payload_bytes: this.getCapabilities().max_payload_bytes },
      undefined, () => { slot.admitted = true; slot.helper = this.helper ?? undefined; }, undefined, () => {
        slot.admitted = false;
        slot.confirmClosed?.();
        if (!slot.handle && this.mediaCaptureSlots.get(ownerKey) === slot) this.mediaCaptureSlots.delete(ownerKey);
      });
      if (result.type === 'recording_started') {
        slot.handle = result.handle;
        this.realtimeRecordingHandles.set(ownerKey, result.handle);
        return result;
      }
      // The callback is set synchronously immediately after helper dispatch.
      // Once sent, failures do not prove the native resource was never opened.
      return { ...result, captureAdmission: slot.admitted ? 'unknown' : 'never_admitted', ...(slot.admitted ? { captureClosed: slot.closed } : {}) };
    } finally {
      if (!slot.handle) {
        this.streamCaptureOwners.delete(ownerKey);
        this.streamCaptureIdentities.delete(ownerKey);
        if (!slot.admitted && this.mediaCaptureSlots.get(ownerKey) === slot) this.mediaCaptureSlots.delete(ownerKey);
      }
      slot.finishStart();
    }
  }

  async startRealtimeCapture(sessionId: string, sampleRateHz: number): Promise<MediaCaptureStartResult> {
    return this.startMediaCapture({ type: 'session', session_id: sessionId }, sampleRateHz);
  }

  async stopMediaCapture(owner: AudioOwnerDto, expectedHandle?: string): Promise<void> {
    const ownerKey = audioOwnerKey(owner);
    let slot = this.mediaCaptureSlots.get(ownerKey);
    if (slot?.kind === 'owner' && expectedHandle === undefined && slot.stop) return slot.stop;
    if (expectedHandle !== undefined && (!slot || slot.kind !== 'capture' || slot.handle !== expectedHandle)) {
      throw new Error('audio capture handle is no longer current');
    }
    if (slot && expectedHandle === undefined) throw new Error('an exact audio capture handle is required');
    if (slot?.stop) return slot.stop;
    if (!slot) {
      if (expectedHandle !== undefined) throw new Error('audio capture handle is no longer current');
      // The owner may only have playback. Reserve the same local owner fence
      // before end_owner so a delayed teardown cannot race a new stream start.
      slot = { kind: 'owner', ready: Promise.resolve(), finishStart: () => undefined, stopping: true, admitted: false };
      this.mediaCaptureSlots.set(ownerKey, slot);
      const ownerSlot = slot;
      ownerSlot.stop = Promise.resolve().then(async () => {
        await this.endAudioOwner(owner);
        if (this.mediaCaptureSlots.get(ownerKey) === ownerSlot) this.mediaCaptureSlots.delete(ownerKey);
      });
      return ownerSlot.stop;
    }
    slot.stopping = true;
    // Stop forwarding before waiting for start or the helper response.
    this.streamCaptureOwners.delete(ownerKey);
    this.streamCaptureIdentities.delete(ownerKey);
    const captureSlot = slot;
    captureSlot.stop = (async () => {
      await captureSlot.ready;
      if (!captureSlot.handle) {
        if (captureSlot.admitted) throw new Error('audio capture admission and cleanup are unconfirmed');
        if (this.mediaCaptureSlots.get(ownerKey) === captureSlot) this.mediaCaptureSlots.delete(ownerKey);
        return;
      }
      if (!captureSlot.helper || this.helper !== captureSlot.helper) throw new Error('audio capture helper changed before stop');
      const origin = [...this.activeRecordingOrigins.values()].find((candidate) =>
        candidate.handle === captureSlot.handle && candidate.helper === captureSlot.helper && audioOwnerKey(candidate.owner) === ownerKey);
      let nativeClosed = false;
      const recover = () => { if (origin) void this.cancelOwnedAudioRequest(origin.request.identity).catch(() => undefined); };
      const confirmed = () => {
        nativeClosed = true;
        captureSlot.confirmClosed?.();
        if (this.mediaCaptureSlots.get(ownerKey) === captureSlot) {
          this.realtimeRecordingHandles.delete(ownerKey);
          this.mediaCaptureSlots.delete(ownerKey);
        }
      };
      const result = await this.executeAudioRequest({ identity: { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch }, owner,
        operation: { type: 'stop_recording', handle: captureSlot.handle }, max_payload_bytes: this.getCapabilities().max_payload_bytes },
      undefined, undefined, () => this.helper === captureSlot.helper, undefined, confirmed);
      if (this.helper !== captureSlot.helper) throw new MediaCaptureStopError('audio capture helper changed during stop', false, captureSlot.closed);
      if (!nativeClosed) recover();
      if (result.type === 'failed' || result.type === 'failed_without_resource') throw new MediaCaptureStopError(`audio capture stop failed (${result.error.kind}): ${result.error.message}`, nativeClosed, captureSlot.closed);
      if (result.type !== 'status' || result.status.recording !== false) throw new MediaCaptureStopError('audio capture stop returned an unexpected result', nativeClosed, captureSlot.closed);
      if (!nativeClosed) throw new MediaCaptureStopError('audio capture stop remains unconfirmed', false, captureSlot.closed);
    })();
    return captureSlot.stop;
  }

  async stopRealtimeCapture(sessionId: string, expectedHandle?: string): Promise<void> {
    return this.stopMediaCapture({ type: 'session', session_id: sessionId }, expectedHandle);
  }

  async playMediaAudio(owner: AudioOwnerDto, pcmBase64: string, sampleRateHz: number): Promise<AudioOperationResultDto> {
    await this.ensureCapabilities();
    return this.executeAudioRequest({ identity: { id: randomUUID(), generation: ++this.streamGeneration, service_epoch: this.getCapabilities().service_epoch }, owner,
      operation: { type: 'play', pcm_base64: pcmBase64, sample_rate_hz: sampleRateHz }, max_payload_bytes: this.getCapabilities().max_payload_bytes });
  }

  async playRealtimeAudio(sessionId: string, pcmBase64: string, sampleRateHz: number): Promise<AudioOperationResultDto> {
    return this.playMediaAudio({ type: 'session', session_id: sessionId }, pcmBase64, sampleRateHz);
  }

  getCapabilities(): NonNullable<NativeAudioSnapshot['capabilities']> {
    return this.getSnapshot().capabilities ?? defaultNativeAudioSnapshot().capabilities!;
  }

  getMediaSessionModes(): MediaSessionModeDto[] {
    return [...(this.getSnapshot().mediaSessionModes ?? [])];
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
      let helperCommand = command;
      if (command.type === 'get_snapshot') {
        // Native readiness must use the same language and sources as hosted readiness.
        // The helper previews this configuration without changing any active operation.
        let configuration = command.configuration;
        if (!configuration) {
          try { configuration = this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults(); } catch { /* Keep settings readable when saved preferences are invalid. */ }
        }
        helperCommand = { type: 'get_snapshot', ...(configuration ? { configuration } : {}) };
      }
      const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'command', command: helperCommand }, null);
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

  /** Includes unresolved opens and leases even after bounded history rolls over. */
  hasMediaRequestId(id: string): boolean {
    return this.seenMediaRequestIds.has(id) || this.mediaOpenFlights.has(id)
      || [...this.mediaLeases.values()].some(lease => lease.openRequestId === id)
      || [...this.pending.values()].some(pending => pending.kind === 'media' && pending.request.request_id === id);
  }

  /** Execute one host-private native device callback on the existing helper. */
  async executeMediaDevice(value: unknown): Promise<MediaDeviceResponseDto> {
    let request: MediaDeviceRequestEnvelopeDto;
    try {
      request = validateMediaDeviceRequest(value);
    } catch (error) {
      const requestId = value && typeof value === 'object' && 'request_id' in value
        && typeof value.request_id === 'string' && value.request_id.length <= 256
        ? value.request_id
        : 'invalid';
      return mediaDeviceFailure(requestId, 'invalid_request', error instanceof Error ? error.message : 'Invalid native media request');
    }
    if (this.seenMediaRequestIds.has(request.request_id) || this.mediaOpenFlights.has(request.request_id)
      || [...this.mediaLeases.values()].some((lease) => lease.openRequestId === request.request_id)
      || [...this.pending.values()].some((pending) => pending.kind === 'media' && pending.request.request_id === request.request_id)) {
      return mediaDeviceFailure(request.request_id, 'invalid_request', 'The native media request identity is stale');
    }
    this.rememberMediaRequestId(request.request_id);
    const beforeDispatch = (kind: MediaErrorKindDto, message: string): MediaDeviceResponseDto =>
      request.request.type === 'open' ? mediaOpenFailedWithoutResource(request.request_id, kind, message)
        : mediaDeviceFailure(request.request_id, kind, message);
    if (request.request.type === 'open' && this.mediaOpenFlights.size >= MAX_UNSETTLED_MEDIA_OPENS) {
      return beforeDispatch('busy', 'Previous native media opens are awaiting confirmed cleanup');
    }
    if (!this.helperPath) {
      return beforeDispatch('unavailable', this.snapshot.helper.message ?? 'The native audio helper is unavailable');
    }

    const timeoutMs = mediaDeviceTimeoutMs(request);
    const deadlineAt = performance.now() + timeoutMs;
    let dispatched = false;
    try {
      if (request.request.type === 'open' && request.request.request.request.mode !== 'audio') {
        return beforeDispatch('unsupported', 'The Desktop native media helper supports audio sessions only');
      }
      if (request.request.type === 'open' && this.opts.isForeground && !this.opts.isForeground()) {
        return beforeDispatch('unavailable', 'A foreground desktop window is required to open native audio');
      }
      await this.ensureHelper();
      if (request.request.type === 'open' && this.opts.isPackaged) {
        const remaining = Math.max(0, deadlineAt - performance.now());
        if (remaining === 0) return beforeDispatch('timeout', 'The native media open deadline expired');
        let permissionTimer: NodeJS.Timeout | undefined;
        try {
          await Promise.race([
            this.resolvePackagedPermissions(['microphone'], () => performance.now() < deadlineAt),
            new Promise<void>((resolvePermission) => {
              permissionTimer = setTimeout(resolvePermission, remaining);
            }),
          ]);
        } finally {
          if (permissionTimer) clearTimeout(permissionTimer);
        }
        if (performance.now() >= deadlineAt) {
          return beforeDispatch('timeout', 'The native media open deadline expired');
        }
      }
      // A second already-bound session must not race an earlier Unknown open,
      // including after the original helper was replaced.
      while (request.request.type === 'open' && this.mediaOpenFlights.size > 0) {
        const remaining = Math.max(0, deadlineAt - performance.now());
        if (remaining === 0) return beforeDispatch('timeout', 'Previous native media cleanup is not confirmed');
        for (const flight of this.mediaOpenFlights.values()) this.retryOrphanedMediaLease(flight);
        await new Promise<void>((resolve) => setTimeout(resolve, Math.min(25, remaining)));
      }
      const remaining = Math.max(0, deadlineAt - performance.now());
      if (remaining === 0) return beforeDispatch('timeout', 'The native media request deadline expired');
      if (request.request.type === 'open' && this.mediaOpenFlights.size >= MAX_UNSETTLED_MEDIA_OPENS) {
        return beforeDispatch('busy', 'Previous native media opens are awaiting confirmed cleanup');
      }
      const response = this.sendHelperMediaRequest(request, remaining);
      dispatched = true;
      return await response;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.opts.diagnostics.add('warn', 'host', `native media device request failed: ${message}`);
      const kind = /timed out/i.test(message) ? 'timeout' : 'native_failure';
      return dispatched ? mediaDeviceFailure(request.request_id, kind, message) : beforeDispatch(kind, message);
    }
  }

  /** Atomically fence a rejected identity so it cannot be dispatched later. */
  rejectAudioRequestBeforeDispatch(value: AudioOperationIdDto): boolean {
    const key = audioIdentityKey(validateNativeAudioOperationIdentity(value));
    if (this.audioAdmissionClaims.has(key) || this.reservedAudioAdmissions.has(key) || this.preparingAudioIdentities.has(key) || this.pendingAudioRequests.has(key)
      || this.hostedOperations.has(key) || this.uncertainAudioRequests.has(key)
      || this.activeRecordingOrigins.has(key) || this.audioCloseBindings.has(key)
      || this.seenAudioIdentities.has(key)) return false;
    this.rememberAudioIdentity(key);
    return true;
  }

  /** Reserve a host request before waiting on an attachment context. */
  reserveAudioAdmission(value: AudioOperationRequestDto): object | undefined {
    const request = validateNativeAudioOperationRequest(value);
    const key = audioIdentityKey(request.identity);
    if (this.audioAdmissionClaims.has(key) || this.reservedAudioAdmissions.has(key) || this.preparingAudioIdentities.has(key) || this.pendingAudioRequests.has(key)
      || this.hostedOperations.has(key) || this.uncertainAudioRequests.has(key) || this.activeRecordingOrigins.has(key)
      || this.audioCloseBindings.has(key) || this.seenAudioIdentities.has(key) || this.reservedAudioAdmissions.size >= 4096
      || this.closingAudioOwners.has(audioOwnerKey(request.owner))) return undefined;
    const token = {};
    let finish!: () => void;
    const done = new Promise<void>(resolve => { finish = resolve; });
    this.reservedAudioAdmissions.set(key, { token, owner: request.owner, done, finish });
    return token;
  }

  cancelReservedAudioAdmission(identity: AudioOperationIdDto, token: object): boolean {
    const key = audioIdentityKey(validateNativeAudioOperationIdentity(identity));
    const reservation = this.reservedAudioAdmissions.get(key);
    if (reservation?.token !== token) return false;
    this.reservedAudioAdmissions.delete(key);
    reservation.finish();
    this.rememberAudioIdentity(key);
    return true;
  }

  executeAudioRequestWithCloseReceipt(request: AudioOperationRequestDto, onExactClose: () => void, admissionToken?: object): Promise<AudioOperationResultDto> {
    return this.executeAudioRequest(request, undefined, undefined, undefined, undefined, undefined, onExactClose, admissionToken);
  }

  async executeAudioRequest(
    value: unknown,
    configurationSnapshot?: { configuration: AudioConfigurationV4; revision: number },
    onNativeAdmission?: () => void | Promise<void>,
    shouldContinue?: () => boolean,
    onNeverAdmitted?: () => void,
    onNativeClose?: () => void,
    onExactClose?: () => void,
    admissionToken?: object,
  ): Promise<AudioOperationResultDto> {
    const request = validateNativeAudioOperationRequest(value);
    const key = audioIdentityKey(request.identity);
    const reservation = this.reservedAudioAdmissions.get(key);
    if ((reservation && reservation.token !== admissionToken) || (admissionToken && reservation?.token !== admissionToken)) {
      return failedAudio('cancelled', 'the audio operation identity is reserved');
    }
    if (reservation) { this.reservedAudioAdmissions.delete(key); reservation.finish(); }
    if (this.audioAdmissionClaims.has(key) || this.preparingAudioIdentities.has(key) || this.pendingAudioRequests.has(key)
      || this.hostedOperations.has(key) || this.uncertainAudioRequests.has(key)
      || this.activeRecordingOrigins.has(key) || this.audioCloseBindings.has(key)
      || this.seenAudioIdentities.has(key)) {
      return failedAudio('cancelled', 'the audio operation identity is stale');
    }
    const ownerKey = audioOwnerKey(request.owner);
    const ownerClose = this.closingAudioOwners.get(ownerKey);
    if (ownerClose && request.operation.type !== 'status'
      && !(request.operation.type === 'end_owner' && request.identity.id === ownerClose)) {
      this.rememberAudioIdentity(key);
      return failedWithoutResource('busy', 'the audio owner is still closing');
    }
    const reservesOwner = request.operation.type === 'end_owner' && !ownerClose;
    if (reservesOwner) this.closingAudioOwners.set(ownerKey, request.identity.id);
    if (admissionToken) this.audioAdmissionClaims.set(key, admissionToken);
    const ownerEpoch = this.audioOwnerEpochs.get(ownerKey) ?? 0;
    let finishPreparing!: () => void;
    const preparing = { owner: request.owner, done: new Promise<void>(resolve => { finishPreparing = resolve; }), finish: () => finishPreparing() };
    this.preparingAudioIdentities.set(key, preparing);
    const stillCurrent = () => (this.audioOwnerEpochs.get(ownerKey) ?? 0) === ownerEpoch && (!shouldContinue || shouldContinue());
    try {
      const result = await this.executeReservedAudioRequest(request, configurationSnapshot, onNativeAdmission, stillCurrent, onNeverAdmitted, onNativeClose, onExactClose);
      if (result.type === 'failed_without_resource') this.rememberAudioIdentity(key);
      return result;
    } finally {
      if (this.preparingAudioIdentities.get(key) === preparing) this.preparingAudioIdentities.delete(key);
      preparing.finish();
      this.retireAudioAdmissionClaim(key);
      if (reservesOwner && this.closingAudioOwners.get(ownerKey) === request.identity.id) this.closingAudioOwners.delete(ownerKey);
    }
  }

  private async executeReservedAudioRequest(
    request: AudioOperationRequestDto,
    configurationSnapshot?: { configuration: AudioConfigurationV4; revision: number },
    onNativeAdmission?: () => void | Promise<void>,
    shouldContinue?: () => boolean,
    onNeverAdmitted?: () => void,
    onNativeClose?: () => void,
    onExactClose?: () => void,
  ): Promise<AudioOperationResultDto> {
    const suspensionRevision = this.suspensionRevision;
    const identityKey = audioIdentityKey(request.identity);
    const control = request.operation.type === 'stop_recording' || request.operation.type === 'end_owner' || request.operation.type === 'status';
    const ownerCleanupUncertain = () => !control && [...this.uncertainAudioRequests.values()].some((old) => audioOwnerKey(old.owner) === audioOwnerKey(request.owner));
    if (ownerCleanupUncertain()) {
      return failedWithoutResource('unavailable', 'previous audio cleanup for this owner remains unconfirmed');
    }
    if (this.seenAudioIdentities.has(identityKey)) {
      return failedAudio('cancelled', 'the audio operation identity is stale');
    }
    if (this.audioSuspending && request.operation.type !== 'end_owner' && request.operation.type !== 'status') {
      return failedWithoutResource('cancelled', 'audio was suspended because the desktop window is hidden');
    }
    if (shouldContinue && !shouldContinue()) {
      return failedWithoutResource('cancelled', 'the audio operation was cancelled');
    }
    const deadlineAt = request.timeout_budget_ms === undefined
      ? null
      : performance.now() + request.timeout_budget_ms;
    const timeoutResult = () => failedWithoutResource('timeout', 'the audio operation deadline expired before native admission');
    const admittedTimeoutResult = () => failedAudio('timeout', 'the audio operation deadline expired after native admission');
    const remainingBudget = () => deadlineAt === null ? undefined : Math.max(0, deadlineAt - performance.now());
    if (request.timeout_budget_ms === 0) {
      return timeoutResult();
    }
    let earlyConfiguration: AudioConfigurationV4 | undefined;
    if (['capture', 'listen', 'synthesize', 'speak'].includes(request.operation.type)) {
      try { earlyConfiguration = structuredClone(configurationSnapshot?.configuration ?? this.opts.getAudioConfiguration?.() ?? audioConfigurationDefaults()); }
      catch (error) { return failedWithoutResource('unavailable', error instanceof Error ? error.message : 'the saved audio configuration is unavailable'); }
    }
    if (request.route_policy === 'host_configured_local_only') {
      const operationType = request.operation.type;
      if (!['listen', 'synthesize', 'speak'].includes(operationType) || !earlyConfiguration) {
        return failedWithoutResource('invalid_request', 'the host-local route policy is invalid for this audio operation');
      }
      const providerRoute = operationType === 'listen'
        ? earlyConfiguration.recognition.source === 'provider'
        : earlyConfiguration.speech.source === 'provider'
          || earlyConfiguration.speech.voice?.source === 'provider';
      if (providerRoute) {
        return failedWithoutResource('invalid_request', 'the native audio snapshot selects a provider route for a host-local operation');
      }
    }
    const hosted = request.operation.type === 'capture'
      || (request.operation.type === 'listen' && earlyConfiguration?.recognition.source === 'provider')
      || ((request.operation.type === 'speak' || request.operation.type === 'synthesize') && earlyConfiguration?.speech.source === 'provider');
    if (hosted) {
      if (this.uncertainAudioRequests.size + this.pendingCloseAcks.size + this.activeRecordingOrigins.size + this.pendingAudioRequests.size + this.hostedOperations.size >= AUDIO_ORDINARY_OPERATION_CAPACITY) {
        return failedWithoutResource('unavailable', 'native audio cleanup capacity is exhausted');
      }
      if (request.identity.service_epoch !== this.getCapabilities().service_epoch) return failedWithoutResource('cancelled', 'the audio service changed before this request started');
      const work = this.executeHostedRequest(request, earlyConfiguration ?? audioConfigurationDefaults(), configurationSnapshot?.revision ?? this.opts.getAudioConfigurationRevision?.() ?? 0, onNativeAdmission,
        () => !ownerCleanupUncertain() && (!shouldContinue || shouldContinue()), onExactClose);
      const operation = this.hostedOperations.get(identityKey);
      if (operation) {
        const settled = () => {
          if (this.hostedOperations.get(identityKey) === operation) this.hostedOperations.delete(identityKey);
          if (operation.captureExactClose && this.uncertainAudioRequests.get(identityKey) === operation.request) {
            this.uncertainAudioRequests.delete(identityKey);
            try { operation.onExactClose?.(); }
            catch (error) { this.opts.diagnostics.add('warn', 'host', `audio close observer failed: ${String(error)}`); }
            operation.onExactClose = undefined;
            this.retireAudioAdmissionClaim(identityKey);
          }
          operation.finish();
        };
        void work.then(settled, settled);
      }
      return work;
    }
    if (request.operation.type === 'listen' || request.operation.type === 'synthesize' || request.operation.type === 'speak') this.executedRoute = undefined;
    if (!this.helperPath) {
      return failedWithoutResource('unavailable', this.snapshot.helper.message ?? 'native audio helper is unavailable');
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
      return failedWithoutResource('unavailable', message);
    }
    if (this.seenAudioIdentities.has(identityKey)) {
      return failedAudio('cancelled', 'the audio operation identity is stale');
    }
    if ((this.audioSuspending || this.suspensionRevision !== suspensionRevision)
      && request.operation.type !== 'end_owner' && request.operation.type !== 'status') {
      return failedWithoutResource('cancelled', 'audio was suspended because the desktop window is hidden');
    }
    if (shouldContinue && !shouldContinue()) {
      return failedWithoutResource('cancelled', 'the audio operation was cancelled');
    }
    if (remainingBudget() === 0) return timeoutResult();
    const capabilities = this.getCapabilities();
    if (request.identity.service_epoch !== capabilities.service_epoch) {
      return failedWithoutResource('cancelled', 'the audio service changed before this request started');
    }
    const kind = operationKind(request.operation);
    if (kind && !capabilities.supported_operations.some((operation) => operation === kind)) {
      return failedWithoutResource('unsupported', 'this device does not support the requested audio operation');
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
        return failedWithoutResource('unavailable', error instanceof Error ? error.message : 'the saved audio configuration is unavailable');
      }
    }
    if (
      (request.operation.type === 'start_recording' || request.operation.type === 'listen')
      && this.opts.isForeground
      && !this.opts.isForeground()
    ) {
      return failedWithoutResource('unavailable', 'a foreground desktop window is required to start microphone capture');
    }
    if (remainingBudget() === 0) return timeoutResult();

    const key = identityKey;
    let resolveCancellation!: () => void;
    let inFlight!: InFlightAudioRequest;
    let closeReceiptVerified = false;
    const unconfirmedCancellation = () => {
      if (inFlight.helperId && !inFlight.closeVerified) this.uncertainAudioRequests.set(key, request);
      return failedAudio('unavailable', 'native audio cancellation was requested; cleanup remains unconfirmed');
    };
    const cancellation = new Promise<AudioOperationResultDto>((resolve) => {
      resolveCancellation = () => {
        if (inFlight.closeVerified) { onNeverAdmitted?.(); resolve(failedWithoutResource('cancelled', 'the audio operation was cancelled')); return; }
        if (!inFlight.helperId) { resolve(failedWithoutResource('cancelled', 'the audio operation was cancelled')); return; }
        void this.submitNativeClose(key, 'native audio cancellation').then(
          () => { onNeverAdmitted?.(); resolve(failedWithoutResource('cancelled', 'the audio operation was cancelled')); },
          () => resolve(unconfirmedCancellation()),
        );
      };
    });
    inFlight = {
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
      this.uncertainAudioRequests.set(key, request);
      // Keep the original helper response routable: a late identity-bound
      // witness may still resolve this exact cleanup flight.
      void this.submitNativeClose(key, context).catch(() => undefined);
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
        return inFlight.helperId ? unconfirmedCancellation() : failedWithoutResource('cancelled', 'audio was suspended before native admission');
      }
      if (shouldContinue && !shouldContinue() && !inFlight.cancelled) {
        inFlight.cancelled = true;
        inFlight.resolveCancellation();
      }
      if (inFlight.cancelled) return inFlight.helperId ? unconfirmedCancellation() : failedWithoutResource('cancelled', 'the audio operation was cancelled');
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
          return failedWithoutResource('unavailable', 'a foreground desktop window is required to start microphone capture');
        }

        const budget = remainingBudget();
        const helperBudget = budget === undefined ? undefined : Math.floor(budget);
        if (helperBudget === 0) {
          inFlight.timedOut = true;
          return timeoutResult();
        }
        if (ownerCleanupUncertain()) return failedWithoutResource('unavailable', 'previous audio cleanup for this owner remains unconfirmed');
        const limit = control ? AUDIO_OPERATION_CAPACITY : AUDIO_ORDINARY_OPERATION_CAPACITY;
        if (this.uncertainAudioRequests.size + this.pendingCloseAcks.size + this.activeRecordingOrigins.size + this.pendingAudioRequests.size + this.hostedOperations.size > limit) {
          return failedWithoutResource('unavailable', 'native audio cleanup capacity is exhausted');
        }
        const id = randomUUID();
        inFlight.helperId = id;
        inFlight.helper = this.helper ?? undefined;
        const helperInstance = this.getSnapshot().helper.instanceId;
        const helperConnection = inFlight.helper && this.helperConnections.get(inFlight.helper as object);
        if (helperInstance && helperConnection) {
          inFlight.closeContext = { helper_instance: helperInstance, connection_id: helperConnection,
            resource_incarnation: randomUUID(), flight_id: randomUUID() };
          this.audioCloseBindings.set(key, { request, helper: inFlight.helper!, context: inFlight.closeContext, inFlight, verified: false, onExactClose });
        }
        const responseTimeoutMs = request.operation.type === 'speak' ? SPEAK_TIMEOUT_MS : REQUEST_TIMEOUT_MS;
        const responsePromise = this.sendHelperEnvelope({
          id,
          kind: 'engine_request',
          request: helperBudget === undefined ? request : { ...request, timeout_budget_ms: helperBudget },
          configuration: operationConfiguration,
          configurationRevision: operationConfigurationRevision,
          ...(request.operation.type === 'start_recording' && this.streamCaptureOwners.has(audioOwnerKey(request.owner)) ? { streamPcm: true } : {}),
          ...(inFlight.closeContext ? { closeContext: inFlight.closeContext } : {}),
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
        if (response.type === 'engine_result' && response.closeReceipt && request.operation.type === 'stop_recording'
          && inFlight.helper === this.helper && this.matchesCloseReceipt(response.closeReceipt, request, inFlight.helper, response.snapshot.helper.instanceId)) {
          closeReceiptVerified = true;
          const originKey = audioIdentityKey(response.closeReceipt.identity);
          this.uncertainAudioRequests.delete(originKey);
          const binding = this.audioCloseBindings.get(originKey);
          if (binding) {
            binding.verified = true;
            binding.inFlight.closeVerified = true;
            this.queueCloseAcknowledgement(originKey, binding);
            this.notifyExactAudioClose(binding);
          }
          this.audioCloseBindings.delete(originKey);
          onNativeClose?.();
        }
        const afterNativeOperation = terminalFailure();
        if (afterNativeOperation) {
          // Cancellation and the original response can race. Do not let the
          // latter overtake the exact close receipt for this request.
          if (inFlight.cancelled && inFlight.helperId) return await cancellation;
          return afterNativeOperation;
        }
        if (response.type !== 'engine_result') {
          return response.type === 'error'
            ? failedAudio(nativeErrorKind(response.error.code), response.error.message)
            : failedAudio('native_failure', 'the audio helper returned a command response for an operation request');
        }
        if (request.operation.type === 'stop_recording'
          && (response.result.type === 'recording' || response.result.type === 'status') && !closeReceiptVerified) {
          return failedAudio('unavailable', 'the audio capture stop remains unconfirmed');
        }
        if (response.result.type === 'failed' && response.result.error.kind === 'cancelled') return unconfirmedCancellation();
        if (response.result.type === 'failed' && response.result.error.kind === 'timeout') {
          inFlight.timedOut = true;
          cancelAdmittedOperationForTimeout('native engine timeout cancellation');
          return admittedTimeoutResult();
        }
        if (sameIdentity(response.snapshot.currentOperation?.identity, request.identity) || isCurrentOrIdleSnapshot(response.snapshot, request)) {
          this.applySnapshot(response.snapshot);
        }
        // A helper payload alone is not an authenticated no-resource receipt.
        return response.result.type === 'failed_without_resource'
          ? { type: 'failed', error: response.result.error } : response.result;
      } catch (error) {
        const terminal = terminalFailure();
        if (terminal) {
          if (inFlight.cancelled && inFlight.helperId) return await cancellation;
          return terminal;
        }
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
        this.activeRecordingOrigins.set(key, { request, owner: request.owner, helper: inFlight.helper, closeContext: inFlight.closeContext, handle: result.handle, configurationRevision: operationConfigurationRevision });
      } else if (
        request.operation.type === 'stop_recording' && closeReceiptVerified
        && (result.type === 'recording' || (result.type === 'status' && !result.status.recording))
      ) {
        const ownerKey = audioOwnerKey(request.owner);
        const stopHandle = request.operation.type === 'stop_recording' ? request.operation.handle : undefined;
        for (const [identity, recording] of this.activeRecordingOrigins) {
          if (audioOwnerKey(recording.owner) === ownerKey && (stopHandle === undefined || recording.handle === stopHandle)) {
            this.activeRecordingOrigins.delete(identity);
            this.retireAudioAdmissionClaim(identity);
          }
        }
      }
      if (result.type === 'failed' && inFlight.helperId && !inFlight.closeVerified
        && ['start_recording', 'stop_recording', 'listen', 'play', 'speak', 'synthesize', 'end_owner'].includes(request.operation.type)) {
        this.uncertainAudioRequests.set(key, request);
        if (!inFlight.cancelled && (request.operation.type === 'start_recording' || request.operation.type === 'stop_recording' || request.operation.type === 'play')) {
          void this.submitNativeClose(key, 'native failed audio cleanup').catch(() => undefined);
        }
        if (request.operation.type === 'stop_recording') {
          // A failed stop and its recording are distinct flights. Reconcile
          // the exact original capture too; never infer its closure from the
          // stop flight or from another recording of the same owner.
          const stopHandle = request.operation.handle;
          const origin = [...this.activeRecordingOrigins.entries()].find(([, candidate]) =>
            candidate.handle === stopHandle && candidate.helper === inFlight.helper
            && audioOwnerKey(candidate.owner) === audioOwnerKey(request.owner));
          if (origin) {
            this.uncertainAudioRequests.set(origin[0], origin[1].request);
            void this.submitNativeClose(origin[0], 'native failed capture origin cleanup').catch(() => undefined);
          }
        }
      }
      if (result.type !== 'recording_started' && !this.uncertainAudioRequests.has(key)) {
        this.audioCloseBindings.delete(key);
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

  // A dispatched operation resolves cancellation only after an identity-bound
  // helper receipt; failed/unknown observations retain the original flight.
  async cancelAudioRequest(value: unknown, admissionToken?: object): Promise<'verified' | undefined> {
    const identity = validateNativeAudioOperationIdentity(value);
    const key = audioIdentityKey(identity);
    const claim = this.audioAdmissionClaims.get(key);
    if ((claim && claim !== admissionToken) || (admissionToken && claim !== admissionToken)) return undefined;
    if (this.reservedAudioAdmissions.has(key)) return undefined;
    const hosted = this.hostedOperations.get(key);
    if (hosted) { hosted.controller.abort(); throw new Error('hosted audio cancellation remains unconfirmed'); }
    const inFlight = this.pendingAudioRequests.get(key);
    if (!inFlight || !sameIdentity(inFlight.request.identity, identity)) {
      const preparingBeforeDispatch = this.preparingAudioIdentities.has(key) && !this.seenAudioIdentities.has(key)
        && !this.audioCloseBindings.has(key) && !this.activeRecordingOrigins.has(key) && !this.uncertainAudioRequests.has(key);
      this.rememberAudioIdentity(key);
      const recording = this.activeRecordingOrigins.get(key);
      if (recording) this.uncertainAudioRequests.set(key, recording.request);
      if (this.uncertainAudioRequests.has(key)) {
        await this.submitNativeClose(key, 'native audio cancellation');
        return 'verified';
      }
      return preparingBeforeDispatch ? 'verified' : undefined;
    }
    inFlight.cancelled = true;
    if (inFlight.helperId && !inFlight.closeVerified) this.uncertainAudioRequests.set(key, inFlight.request);
    inFlight.resolveCancellation();
    if (!inFlight.helperId || inFlight.closeVerified) return 'verified';
    await this.submitNativeClose(key, 'native audio cancellation');
    return 'verified';
  }

  private async submitNativeClose(key: string, context: string): Promise<void> {
    const binding = this.audioCloseBindings.get(key);
    if (!binding || binding.helper !== this.helper || !binding.helper.stdin?.writable) throw new Error(`${context}: cleanup remains unconfirmed`);
    if (binding.verified) return;
    if (binding.flight) return binding.flight;
    const flight = (async () => {
      const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'command',
        command: { type: 'cancel_operation', identity: binding.request.identity, owner: binding.request.owner, closeContext: binding.context } }, null);
      if (binding.helper !== this.helper || response.type !== 'close_receipt'
        || !this.matchesCancellationReceipt(response.closeReceipt, binding, response.snapshot.helper.instanceId)) {
        throw new Error(`${context}: cleanup remains unconfirmed`);
      }
      binding.verified = true;
      binding.inFlight.closeVerified = true;
      this.uncertainAudioRequests.delete(key);
      const recording = this.activeRecordingOrigins.get(key);
      if (recording && recording.helper === binding.helper && recording.closeContext === binding.context) this.activeRecordingOrigins.delete(key);
      const slot = this.mediaCaptureSlots.get(audioOwnerKey(binding.request.owner));
      if (slot?.identity && sameIdentity(slot.identity, binding.request.identity) && slot.helper === binding.helper
        && (slot.handle === undefined || slot.handle === response.closeReceipt.handle)) {
        slot.confirmClosed?.();
        this.realtimeRecordingHandles.delete(audioOwnerKey(binding.request.owner));
        this.mediaCaptureSlots.delete(audioOwnerKey(binding.request.owner));
      }
      // The acknowledgement only retires the helper's replay record. It is
      // not itself the close proof and never targets a different helper.
      this.queueCloseAcknowledgement(key, binding);
      this.notifyExactAudioClose(binding);
      this.audioCloseBindings.delete(key);
      this.retireAudioAdmissionClaim(key);
    })();
    binding.flight = flight;
    try { await flight; }
    finally { if (binding.flight === flight) binding.flight = undefined; }
  }

  private cancelOwnedAudioRequest(identity: AudioOperationIdDto): Promise<'verified' | undefined> {
    return this.cancelAudioRequest(identity, this.audioAdmissionClaims.get(audioIdentityKey(identity)));
  }

  private retireAudioAdmissionClaim(key: string): void {
    if (this.reservedAudioAdmissions.has(key) || this.preparingAudioIdentities.has(key)
      || this.pendingAudioRequests.has(key) || this.hostedOperations.has(key)
      || this.uncertainAudioRequests.has(key) || this.activeRecordingOrigins.has(key)
      || this.audioCloseBindings.has(key)) return;
    this.audioAdmissionClaims.delete(key);
  }

  private notifyExactAudioClose(binding: AudioCloseBinding): void {
    const callback = binding.onExactClose;
    binding.onExactClose = undefined;
    try { callback?.(); }
    catch (error) { this.opts.diagnostics.add('warn', 'host', `audio close observer failed: ${String(error)}`); }
  }

  private queueCloseAcknowledgement(key: string, binding: AudioCloseBinding): void {
    if (this.pendingCloseAcks.has(key)) return;
    const pending: { binding: AudioCloseBinding; timer?: NodeJS.Timeout } = { binding };
    this.pendingCloseAcks.set(key, pending);
    const send = async (): Promise<void> => {
      if (this.pendingCloseAcks.get(key) !== pending) return;
      if (this.helper !== binding.helper) { this.pendingCloseAcks.delete(key); return; }
      try {
        const response = await this.sendHelperEnvelope({ id: randomUUID(), kind: 'command',
          command: { type: 'ack_close_receipt', identity: binding.request.identity, owner: binding.request.owner, closeContext: binding.context } }, null, 5_000);
        if (response.type !== 'close_ack' || this.helper !== binding.helper) throw new Error('close acknowledgement failed');
        this.pendingCloseAcks.delete(key);
      } catch {
        if (this.helper !== binding.helper) { this.pendingCloseAcks.delete(key); return; }
        pending.timer = setTimeout(() => { pending.timer = undefined; void send(); }, this.opts.closeAckRetryMs ?? 5_000);
        pending.timer.unref();
      }
    };
    void send();
  }

  private matchesCancellationReceipt(receipt: NonNullable<NativeAudioEngineResponse['closeReceipt']>, binding: AudioCloseBinding, snapshotHelper?: string): boolean {
    if (receipt.disposition !== 'closed' && receipt.disposition !== 'not_admitted') return false;
    const origin = this.activeRecordingOrigins.get(audioIdentityKey(binding.request.identity));
    if (receipt.disposition === 'closed' && origin && receipt.handle !== origin.handle) return false;
    return snapshotHelper === binding.context.helper_instance && sameIdentity(receipt.identity, binding.request.identity)
      && audioOwnerKey(receipt.owner) === audioOwnerKey(binding.request.owner)
      && receipt.context.helper_instance === binding.context.helper_instance && receipt.context.connection_id === binding.context.connection_id
      && receipt.context.resource_incarnation === binding.context.resource_incarnation && receipt.context.flight_id === binding.context.flight_id;
  }

  async endAudioOwner(owner: AudioOwnerDto): Promise<void> {
    const expectedHelper = this.helper;
    if (await this.endAudioOwnerWithResult(owner, expectedHelper)) return;
    this.stopHelperForTeardown('owner teardown failed', expectedHelper);
    throw new Error('native audio owner could not be ended');
  }

  private endAudioOwnerWithResult(owner: AudioOwnerDto, expectedHelper?: HelperProcess | null): Promise<boolean> {
    const ownerKey = audioOwnerKey(owner);
    const existing = this.audioOwnerTeardowns.get(ownerKey);
    if (existing) return existing.helper === expectedHelper ? existing.task : Promise.resolve(false);
    if (this.closingAudioOwners.has(ownerKey)) return Promise.resolve(false);
    const endRequestId = randomUUID();
    this.audioOwnerEpochs.set(ownerKey, (this.audioOwnerEpochs.get(ownerKey) ?? 0) + 1);
    this.closingAudioOwners.set(ownerKey, endRequestId);
    const task = (async () => {
      try {
        const ended = await this.endReservedAudioOwner(owner, endRequestId, expectedHelper);
        if (ended) this.audioOwnerEpochs.delete(ownerKey);
        return ended;
      } finally {
        if (this.closingAudioOwners.get(ownerKey) === endRequestId) this.closingAudioOwners.delete(ownerKey);
        this.audioOwnerTeardowns.delete(ownerKey);
      }
    })();
    this.audioOwnerTeardowns.set(ownerKey, { helper: expectedHelper, task });
    return task;
  }

  private async endReservedAudioOwner(owner: AudioOwnerDto, endRequestId: string, expectedHelper?: HelperProcess | null): Promise<boolean> {
    const ownerKey = audioOwnerKey(owner);
    // Closing increments the owner epoch before waiting, so a request still
    // preparing cannot cross native admission while this owner is retired.
    const preparing = [...this.preparingAudioIdentities.entries()].filter(([key, item]) => audioOwnerKey(item.owner) === ownerKey
      && !this.pendingAudioRequests.has(key) && !this.hostedOperations.has(key)).map(([, item]) => item.done);
    const reserved = [...this.reservedAudioAdmissions.values()].filter(item => audioOwnerKey(item.owner) === ownerKey).map(item => item.done);
    await Promise.all([...preparing, ...reserved]);
    const hasWork = [...this.pendingAudioRequests.values()].some(item => audioOwnerKey(item.request.owner) === ownerKey)
      || [...this.uncertainAudioRequests.values()].some(item => audioOwnerKey(item.owner) === ownerKey)
      || [...this.activeRecordingOrigins.values()].some(item => audioOwnerKey(item.owner) === ownerKey)
      || [...this.audioCloseBindings.values()].some(item => audioOwnerKey(item.request.owner) === ownerKey)
      || [...this.hostedOperations.values()].some(item => audioOwnerKey(item.owner) === ownerKey);
    if (!hasWork && owner.type === 'attached') return true;
    const helperMatches = () => expectedHelper === undefined || (expectedHelper !== null && this.helper === expectedHelper);
    if (!helperMatches()) return false;
    const hosted = [...this.hostedOperations.entries()].filter(([, operation]) => audioOwnerKey(operation.owner) === ownerKey);
    for (const [, operation] of hosted) operation.controller.abort();
    // Capture is local work: join its task before inspecting and closing its
    // native recording. Provider operations retain their existing Unknown gate.
    const captures = hosted.filter(([, operation]) => operation.request.operation.type === 'capture');
    await Promise.all(captures.map(([, operation]) => operation.done));
    for (const pending of [...this.pendingAudioRequests.values()]) {
      if (audioOwnerKey(pending.request.owner) === ownerKey) {
        if (pending.closeVerified) continue;
        try { await this.cancelOwnedAudioRequest(pending.request.identity); } catch { return false; }
      }
    }
    // An owner-ended ACK does not identify or drain a persistent recording.
    // Close each known origin against its own helper/context before asking
    // for the broad owner teardown. A failed receipt leaves it fenced.
    for (const [key, origin] of [...this.activeRecordingOrigins]) {
      if (audioOwnerKey(origin.owner) !== ownerKey) continue;
      try { await this.cancelOwnedAudioRequest(origin.request.identity); } catch { return false; }
      if (!helperMatches() || this.activeRecordingOrigins.has(key)) return false;
    }
    const captureKeys = new Set(captures.map(([key]) => key));
    if ([...this.uncertainAudioRequests.entries()].some(([key, request]) => audioOwnerKey(request.owner) === ownerKey && !captureKeys.has(key))) return false;
    for (const [key, operation] of captures) {
      if (this.uncertainAudioRequests.get(key) === operation.request) this.uncertainAudioRequests.delete(key);
      try { operation.onExactClose?.(); }
      catch (error) { this.opts.diagnostics.add('warn', 'host', `audio close observer failed: ${String(error)}`); }
      operation.onExactClose = undefined;
      this.retireAudioAdmissionClaim(key);
    }
    try {
      await this.ensureCapabilities();
    } catch (error) {
      this.opts.diagnostics.add('warn', 'host', `audio owner teardown could not refresh helper capabilities: ${String(error)}`);
      return false;
    }
    if (!helperMatches()) return false;
    const capabilities = this.getCapabilities();
    const result = await this.executeAudioRequest({
      identity: { id: endRequestId, generation: 1, service_epoch: capabilities.service_epoch },
      owner,
      max_payload_bytes: capabilities.max_payload_bytes,
      operation: { type: 'end_owner' },
    }, undefined, undefined, helperMatches);
    if (!helperMatches()) return false;
    if (result.type === 'owner_ended') {
      return ![...this.activeRecordingOrigins.values()].some((recording) => audioOwnerKey(recording.owner) === ownerKey)
        && ![...this.uncertainAudioRequests.values()].some((request) => audioOwnerKey(request.owner) === ownerKey);
    }
    if ((result.type === 'failed' || result.type === 'failed_without_resource') && result.error.kind !== 'unavailable') {
      this.opts.diagnostics.add('warn', 'host', `audio owner teardown failed (${result.error.kind}): ${result.error.message}`);
    } else if (result.type !== 'failed' && result.type !== 'failed_without_resource') {
      this.opts.diagnostics.add('warn', 'host', `audio owner teardown returned an unexpected result: ${result.type}`);
    }
    return false;
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
    const expectedHelper = this.helper;
    let timedOut = false;
    let timeoutTimer: NodeJS.Timeout | undefined;
    const deadline = new Promise<void>((resolve) => {
      timeoutTimer = setTimeout(() => {
        timedOut = true;
        this.stopHelperForTeardown(`suspend (${reason}) timed out`, expectedHelper);
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
        for (const uncertain of this.uncertainAudioRequests.values()) remember(uncertain.owner);
        for (const pending of this.pending.values()) {
          if (pending.kind === 'command' && pending.owner) rememberSnapshotOwner(pending.owner);
          if (pending.kind === 'media' && pending.request.request.type === 'open') {
            const mediaOwner: unknown = pending.request.request.request.owner;
            // Attached media owners are not AudioOwnerDto and must never be
            // projected to their logical owner during suspension.
            if (typeof mediaOwner === 'object' && mediaOwner !== null && 'type' in mediaOwner && mediaOwner.type === 'attached') hasUnmappableOwner = true;
            else remember(mediaOwner as AudioOwnerDto);
          }
        }
        if (this.snapshot.currentOperation) remember(this.snapshot.currentOperation.owner);
        rememberSnapshotOwner(this.snapshot.owner);

        let cancellationUnconfirmed = false;
        for (const pending of [...this.pendingAudioRequests.values()]) {
          try { await this.cancelOwnedAudioRequest(pending.request.identity); }
          catch { cancellationUnconfirmed = true; }
          if (timedOut) return;
        }
        if (!this.helper) {
          if (cancellationUnconfirmed || this.uncertainAudioRequests.size) throw new Error('native audio suspension cleanup remains unconfirmed');
          this.releaseOwner();
          return;
        }
        if (this.helper !== expectedHelper) throw new Error('native audio suspension helper changed; cleanup remains unconfirmed');
        let ownerTeardownFailed = cancellationUnconfirmed;
        for (const owner of owners.values()) {
          if (timedOut) return;
          if (!await this.endAudioOwnerWithResult(owner, expectedHelper)) ownerTeardownFailed = true;
        }
        if (!timedOut && (hasUnmappableOwner || ownerTeardownFailed)) {
          this.stopHelperForTeardown(`suspend (${reason}) could not confirm owner teardown`, expectedHelper);
          throw new Error('native audio suspension cleanup remains unconfirmed');
        }
      };
      await Promise.race([cleanup(), deadline]);
      if (timedOut) throw new Error('native audio suspension cleanup remains unconfirmed');
    } catch (error) {
      this.stopHelperForTeardown(`suspend (${reason}) failed`, expectedHelper);
      throw error;
    } finally {
      if (timeoutTimer) clearTimeout(timeoutTimer);
    }
  }

  private stopHelperForTeardown(reason: string, expectedHelper?: HelperProcess | null): void {
    const helper = this.helper;
    if (!helper || (expectedHelper !== undefined && helper !== expectedHelper)) return;
    this.retainAdmittedAudioRequests();
    this.opts.diagnostics.add('warn', 'host', `native audio ${reason}; stopping the helper to release audio resources`);
    this.suspendedHelpers.add(helper as object);
    for (const pending of [...this.pendingAudioRequests.values()]) {
      pending.cancelled = true;
      if (pending.helperId) this.uncertainAudioRequests.set(audioIdentityKey(pending.request.identity), pending.request);
      pending.resolveCancellation();
    }
    this.pendingAudioRequests.clear();
    this.activeRecordingOrigins.clear();
    this.failPending(`native audio helper stopped during ${reason}`);
    helper.kill();
  }

  async dispose(): Promise<void> {
    for (const waiter of this.mediaDrainWaiters) {
      waiter.detach();
      waiter.reject(new Error('Native media helper disposed before cleanup was confirmed'));
    }
    this.mediaDrainWaiters.clear();
    for (const operation of this.hostedOperations.values()) operation.controller.abort();
    for (const ack of this.pendingCloseAcks.values()) if (ack.timer) clearTimeout(ack.timer);
    this.pendingCloseAcks.clear();
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
      return response.result.type === 'failed' || response.result.type === 'failed_without_resource'
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
    const previousMediaModes = JSON.stringify(this.snapshot.mediaSessionModes);
    this.snapshot = cloneSnapshot(snapshot);
    if (JSON.stringify(this.snapshot.capabilities ?? null) !== previousCapabilities
      || JSON.stringify(this.snapshot.mediaSessionModes) !== previousMediaModes) {
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
      ...(state === 'running' ? {} : { mediaSessionModes: [] }),
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
    this.helperConnections.set(child as object, randomUUID());
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
      this.retainAdmittedAudioRequests();
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
    this.retainAdmittedAudioRequests();
    this.helper = null;
    if (this.capabilitiesHelper === child) this.capabilitiesHelper = null;
    this.helperStdoutBuffer = '';
    this.activeRecordingOrigins.clear();
    this.failPending(detail);
    this.releaseOwner();
    this.setHelperState(suspended ? 'stopped' : 'failed', detail);
    child.kill();
  }

  private matchesCloseReceipt(receipt: NonNullable<NativeAudioEngineResponse['closeReceipt']>, stop: AudioOperationRequestDto, helper: HelperProcess | undefined, snapshotHelper?: string): boolean {
    if (stop.operation.type !== 'stop_recording') return false;
    const origin = this.activeRecordingOrigins.get(audioIdentityKey(receipt.identity));
    const expected = origin?.closeContext;
    return !!origin && !!expected && !!helper && origin.helper === helper && snapshotHelper === expected.helper_instance
      && receipt.disposition === 'closed' && receipt.handle === stop.operation.handle && receipt.handle === origin.handle
      && sameIdentity(receipt.identity, origin.request.identity)
      && audioOwnerKey(receipt.owner) === audioOwnerKey(origin.owner) && audioOwnerKey(stop.owner) === audioOwnerKey(origin.owner)
      && receipt.context.helper_instance === expected.helper_instance && receipt.context.connection_id === expected.connection_id
      && receipt.context.resource_incarnation === expected.resource_incarnation && receipt.context.flight_id === expected.flight_id;
  }

  private retainAdmittedAudioRequests(): void {
    for (const [key, recording] of this.activeRecordingOrigins) this.uncertainAudioRequests.set(key, recording.request);
    for (const pending of this.pendingAudioRequests.values()) {
      if (pending.helperId) this.uncertainAudioRequests.set(audioIdentityKey(pending.request.identity), pending.request);
    }
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
        this.handleHelperEnvelope(payload, child);
      } catch (error) {
        this.failHelper(child, `native audio helper emitted an invalid envelope: ${String(error)}`);
        return;
      }
    }
  }

  private handleHelperEnvelope(value: unknown, child: HelperProcess): void {
    if (value && typeof value === 'object' && !Array.isArray(value)
      && (value as { type?: unknown }).type === 'media_response') {
      this.handleMediaHelperResponse(value, child);
      return;
    }
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
        else if (pending.kind === 'engine') pending.resolve({
          type: 'engine_result',
          snapshot: this.getSnapshot(),
          result: failedAudio('native_failure', envelope.error.message),
        });
        else pending.reject(new Error(envelope.error.message));
        return;
      }
      if ((envelope.result as { type?: string }).type === 'engine_result') {
        const response = validateNativeAudioEngineResponse(envelope.result);
        if (pending.kind === 'command') pending.resolve(this.normalizeCommandResponse(response));
        else if (pending.kind === 'engine') {
          // The caller independently decides whether to apply the snapshot.
          // Preserve exact helper receipts even when another operation owns it.
          pending.resolve(response);
        } else pending.reject(new Error('native audio helper returned an audio response for a media request'));
      } else {
        const response = validateNativeAudioResponse(envelope.result);
        if (pending.kind === 'command') pending.resolve(response);
        else if (pending.kind === 'engine') pending.resolve({
          type: 'engine_result',
          snapshot: this.getSnapshot(),
          result: failedAudio('native_failure', 'the audio helper returned a command response for an operation request'),
        });
        else pending.reject(new Error('native audio helper returned an audio response for a media request'));
      }
    } catch (error) {
      pending.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private handleMediaHelperResponse(value: unknown, helper: HelperProcess): void {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return;
    const envelope = value as { id?: unknown; response?: unknown; close_receipt?: unknown };
    if (typeof envelope.id !== 'string') return;
    const pending = this.pending.get(envelope.id);
    if (!pending || pending.kind !== 'media' || pending.helper !== helper) return;
    try {
      const response = validateMediaDeviceResponse(envelope.response);
      if (response.request_id !== pending.request.request_id) throw new Error('Native media helper returned a mismatched request ID');
      clearTimeout(pending.timer);
      if (pending.lateCleanupTimer) clearTimeout(pending.lateCleanupTimer);
      this.pending.delete(envelope.id);
      if (response.result.type === 'open_failed_without_resource') {
        if (pending.request.request.type !== 'open' || !pending.flight) throw new Error('Unexpected native open failure receipt');
        this.settleMediaOpenFlight(pending.flight);
        pending.resolve(response);
        return;
      } else if (pending.request.request.type === 'open' && response.result.type === 'opened') {
        const flight = pending.flight!;
        const leaseId = response.result.lease.lease_id;
        if (this.mediaLeases.has(leaseId)) throw new Error('Native media returned a duplicate lease ID');
        const origin: MediaLeaseOrigin = { helper, openRequestId: flight.requestId, ownerKey: flight.ownerKey,
          identity: flight.identity, leaseId };
        this.mediaLeases.set(leaseId, origin);
        flight.lease = origin;
        if (pending.timedOut) {
          flight.resolveAfterCleanup = () => pending.resolve(mediaOpenFailedWithoutResource(
            pending.request.request_id, 'timeout', 'The native media open timed out and was closed',
          ));
          this.retryOrphanedMediaLease(flight);
        } else this.settleMediaOpenFlight(flight);
      } else if (pending.request.request.type === 'stop' && response.result.type === 'stopped') {
        const origin = pending.lease;
        if (!origin || !this.matchesMediaCloseReceipt(envelope.close_receipt, origin)) {
          throw new Error('Native media stop did not confirm the exact lease and open flight');
        }
        if (this.mediaLeases.get(origin.leaseId) === origin) this.mediaLeases.delete(origin.leaseId);
        const flight = this.mediaOpenFlights.get(origin.openRequestId);
        if (flight?.lease === origin) this.settleMediaOpenFlight(flight);
        pending.resolve(response);
        return;
      } else if (pending.request.request.type === 'stop' && response.result.type === 'error') {
        // Unknown is not a receipt. Returning the failure lets the same
        // original helper/lease be retried instead of being silently retired.
        pending.resolve(response);
        return;
      }
      if (!pending.timedOut) pending.resolve(response);
    } catch (error) {
      clearTimeout(pending.timer);
      if (pending.lateCleanupTimer) clearTimeout(pending.lateCleanupTimer);
      this.pending.delete(envelope.id);
      if (!pending.timedOut || pending.request.request.type === 'stop') {
        pending.reject(error instanceof Error ? error : new Error(String(error)));
      }
    }
  }

  private rememberMediaRequestId(requestId: string): void {
    this.seenMediaRequestIds.add(requestId);
    this.mediaRequestHistory.push(requestId);
    if (this.mediaRequestHistory.length > 4_096) {
      const oldest = this.mediaRequestHistory.shift();
      if (oldest) this.seenMediaRequestIds.delete(oldest);
    }
  }

  private sendHelperMediaRequest(
    request: MediaDeviceRequestEnvelopeDto,
    timeoutMs: number,
    originalHelper?: HelperProcess,
  ): Promise<MediaDeviceResponseDto> {
    const helper = originalHelper ?? this.helper;
    if (!helper?.stdin?.writable || this.helper !== helper) throw new Error('original native audio helper is not writable');
    const lease = request.request.type === 'stop' ? this.mediaLeases.get(request.request.lease_id) : undefined;
    if (request.request.type === 'stop' && (!lease || lease.helper !== helper)) {
      throw new Error('native media lease does not belong to this helper');
    }
    let flight: MediaOpenFlight | undefined;
    if (request.request.type === 'open') {
      const open = request.request.request;
      flight = { helper, requestId: request.request_id, ownerKey: nativeMediaOwnerKey(open.owner),
        identity: { ...open.identity } };
      this.mediaOpenFlights.set(request.request_id, flight);
    }
    const id = randomUUID();
    return new Promise((resolveMedia, rejectMedia) => {
      const timer = setTimeout(() => {
        const pending = this.pending.get(id);
        if (!pending || pending.kind !== 'media') return;
        pending.timedOut = true;
        this.sendHelperMediaCancel(helper, id);
        if (request.request.type !== 'open' && request.request.type !== 'stop') {
          pending.resolve(mediaDeviceFailure(request.request_id, 'timeout', 'The native media request timed out'));
          pending.lateCleanupTimer = setTimeout(() => {
            if (this.pending.get(id) === pending) this.pending.delete(id);
          }, MEDIA_LATE_OPEN_CLEANUP_MS);
        }
      }, Math.max(1, timeoutMs));
      this.pending.set(id, {
        kind: 'media',
        request,
        resolve: resolveMedia,
        reject: rejectMedia,
        timer,
        timedOut: false,
        helper,
        flight,
        lease,
      });
      try {
        helper.stdin.write(`${JSON.stringify({ id, kind: 'media_request', request })}\n`, (error) => {
          if (error) this.failHelper(helper, `native audio helper media input failed: ${error.message}`);
        });
      } catch (error) {
        this.failHelper(helper, `native audio helper media input failed: ${String(error)}`);
      }
    });
  }

  private sendHelperMediaCancel(helper: HelperProcess, targetId: string): void {
    if (this.helper !== helper || !helper.stdin?.writable) return;
    const envelope = { id: randomUUID(), kind: 'media_cancel', target_id: targetId };
    try {
      helper.stdin.write(`${JSON.stringify(envelope)}\n`, (error) => {
        if (error) this.opts.diagnostics.add('warn', 'host', `native media cancellation write failed: ${error.message}`);
      });
    } catch (error) {
      this.opts.diagnostics.add('warn', 'host', `native media cancellation write failed: ${String(error)}`);
    }
  }

  private async stopOrphanedMediaLease(origin: MediaLeaseOrigin): Promise<void> {
    const request: MediaDeviceRequestEnvelopeDto = {
      request_id: randomUUID(),
      request: { type: 'stop', lease_id: origin.leaseId },
    };
    const response = await this.sendHelperMediaRequest(request, MEDIA_DEVICE_OPERATION_TIMEOUT_MS, origin.helper);
    if (response.result.type !== 'stopped') {
      this.opts.diagnostics.add('warn', 'host', 'native media helper did not acknowledge cleanup of a late-opened lease');
    }
  }

  private settleMediaOpenFlight(flight: MediaOpenFlight): void {
    if (this.mediaOpenFlights.get(flight.requestId) !== flight) return;
    this.mediaOpenFlights.delete(flight.requestId);
    flight.resolveAfterCleanup?.();
    if (this.mediaOpenFlights.size === 0) {
      for (const waiter of this.mediaDrainWaiters) { waiter.detach(); waiter.resolve(); }
      this.mediaDrainWaiters.clear();
    }
  }

  private matchesMediaCloseReceipt(value: unknown, origin: MediaLeaseOrigin): boolean {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
    const receipt = value as Record<string, unknown>;
    if (Object.keys(receipt).sort().join(',') !== 'identity,lease_id,open_request_id,owner'
      || receipt.lease_id !== origin.leaseId || receipt.open_request_id !== origin.openRequestId) return false;
    const identity = receipt.identity as Record<string, unknown> | null;
    if (!identity || typeof identity !== 'object' || Array.isArray(identity)
      || Object.keys(identity).sort().join(',') !== 'generation,id,service_epoch'
      || identity.id !== origin.identity.id || identity.generation !== origin.identity.generation
      || identity.service_epoch !== origin.identity.service_epoch) return false;
    try { return nativeMediaOwnerKey(receipt.owner) === origin.ownerKey; } catch { return false; }
  }

  private retryOrphanedMediaLease(flight: MediaOpenFlight): void {
    if (!flight.lease || flight.cleanup) return;
    const cleanup = this.stopOrphanedMediaLease(flight.lease).catch((error: unknown) => {
      this.opts.diagnostics.add('warn', 'host', `native media orphan cleanup remains unconfirmed: ${String(error)}`);
    }).finally(() => { if (flight.cleanup === cleanup) flight.cleanup = undefined; });
    flight.cleanup = cleanup;
  }

  /** Wait only for explicit terminal open/close receipts; Unknown never expires. */
  drainMediaOpenFlights(signal?: AbortSignal): Promise<void> {
    for (const flight of this.mediaOpenFlights.values()) this.retryOrphanedMediaLease(flight);
    if (signal?.aborted) return Promise.reject(Object.assign(new Error('Native media drain was cancelled'), { name: 'AbortError' }));
    if (this.mediaOpenFlights.size === 0) return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      const abort = () => {
        this.mediaDrainWaiters.delete(waiter);
        waiter.detach();
        reject(Object.assign(new Error('Native media drain was cancelled'), { name: 'AbortError' }));
      };
      const waiter: MediaDrainWaiter = { resolve, reject, detach: () => signal?.removeEventListener('abort', abort) };
      this.mediaDrainWaiters.add(waiter);
      signal?.addEventListener('abort', abort, { once: true });
    });
  }

  private failPending(message: string): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      if (pending.kind === 'media' && pending.lateCleanupTimer) clearTimeout(pending.lateCleanupTimer);
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
