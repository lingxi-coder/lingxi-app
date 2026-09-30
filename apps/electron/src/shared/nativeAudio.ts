import type {
  AudioCapabilitySnapshotDto,
  AudioErrorKindDto,
  AudioOperationDto,
  AudioOperationIdDto,
  AudioOperationRequestDto,
  AudioOperationResultDto,
  AudioOwnerDto,
} from '@lingxi/bridge-client';
import type { MicrophonePermissionStatus } from './microphoneAccess.js';
import type { AudioConfigurationV3 } from './generatedAudioConfiguration.js';
import {
  isSendableAudioBase64,
  isSendableAudioText,
  MAX_AUDIO_FAILURE_MESSAGE_LENGTH,
  MAX_AUDIO_BASE64_LENGTH,
  MAX_AUDIO_MIME_TYPE_LENGTH,
  MAX_AUDIO_PAYLOAD_BYTES,
} from './audioResponse.js';

const MAX_OWNER_ID_LENGTH = 512;
const MAX_LANGUAGE_LENGTH = 64;
const MAX_TEXT_LENGTH = 256 * 1024;
const MAX_VOICE_ID_LENGTH = 512;
const MAX_MODEL_ID_LENGTH = 256;
const MIN_SAMPLE_RATE_HZ = 8_000;
const MAX_SAMPLE_RATE_HZ = 768_000;
const MIN_RATE = 0.5;
const MAX_RATE = 2.0;

export const CH_NATIVE_AUDIO_REQUEST = 'lingxi:audio:request';
export const CH_NATIVE_AUDIO_EVENT = 'lingxi:audio:event';
export const CH_NATIVE_AUDIO_OPERATION = 'lingxi:audio:execute';
export const CH_NATIVE_AUDIO_CANCEL = 'lingxi:audio:cancel';
export const CH_NATIVE_AUDIO_FINISH_LISTEN = 'lingxi:audio:finish-listen';

export type SpeechPermissionStatus =
  | 'authorized'
  | 'denied'
  | 'restricted'
  | 'not_determined'
  | 'unavailable';

export type NativeAudioErrorCode =
  | 'permission'
  | 'unavailable'
  | 'busy'
  | 'cancelled'
  | 'model-missing'
  | 'download'
  | 'checksum'
  | 'invalid-request'
  | 'native-error';

export interface NativeAudioError {
  code: NativeAudioErrorCode;
  message: string;
}

export interface NativeAudioOwner {
  kind: 'dictation' | 'flow' | 'preview' | 'autoplay' | 'engine' | 'session' | 'local_app' | 'ui' | 'system';
  id: string;
}

export type NativeAudioOwnerToken = NativeAudioOwner;

export interface NativeAudioVoiceOption {
  id: string;
  label: string;
  languageTag: string;
  source: 'system' | 'sherpa';
  familyId: string;
  isDefault?: boolean;
  networkRequired?: boolean;
}

export type NativeAudioModelState =
  | { type: 'not-installed' }
  | { type: 'queued' }
  | { type: 'downloading'; receivedBytes: number; totalBytes: number }
  | { type: 'verifying' }
  | { type: 'extracting' }
  | { type: 'ready' }
  | { type: 'failed'; message: string };

export interface NativeAudioModelSnapshot {
  modelId: string;
  state: NativeAudioModelState;
}

export interface NativeAudioRecognitionSnapshot {
  requestedMode: 'automatic' | 'localOnly';
  effectiveBackend: 'apple' | 'sherpa' | 'unavailable';
  effectiveLanguage: string;
  detail: string;
  fallbackReason?: string;
}

export interface NativeAudioPlaybackSnapshot {
  requestedVoiceSelection: string;
  effectiveVoiceId: string;
  effectiveVoiceLabel: string;
}

export type NativeAudioTraceOperation =
  | 'start_recording'
  | 'stop_recording'
  | 'listen'
  | 'synthesize'
  | 'speak'
  | 'status'
  | 'end_owner';

export interface NativeAudioOperationTrace {
  identity: AudioOperationIdDto;
  owner: AudioOwnerDto;
  operation: NativeAudioTraceOperation;
  configurationRevision: number;
  requestedSource?: string;
  requestedModelId?: string;
  requestedVoiceId?: string;
  effectiveSource?: string;
  effectiveModelId?: string;
  effectiveVoiceId?: string;
  fallbackReason?: string;
}

export interface NativeAudioSnapshot {
  helper: {
    state: 'stopped' | 'starting' | 'running' | 'failed';
    message?: string;
  };
  permissions: {
    microphone: MicrophonePermissionStatus;
    speech: SpeechPermissionStatus;
  };
  owner: NativeAudioOwner | null;
  activity: 'idle' | 'listening' | 'recognizing' | 'speaking';
  localeTag?: string;
  recognizerAvailable?: boolean;
  recognition?: NativeAudioRecognitionSnapshot;
  playback?: NativeAudioPlaybackSnapshot;
  capabilities?: AudioCapabilitySnapshotDto;
  configurationRevision?: number;
  currentOperation?: { identity: AudioOperationIdDto; owner: AudioOwnerDto };
  audioOperations: NativeAudioOperationTrace[];
  activeOperationCount: number;
  pendingOperationCount: number;
  activeRecordingCount: number;
  activePlaybackCount: number;
  activeModelReferenceCount: number;
  voices: NativeAudioVoiceOption[];
  models: NativeAudioModelSnapshot[];
}

export interface NativeAudioRecognitionProgress {
  owner: NativeAudioOwner;
  text: string;
  isFinal: boolean;
}

export type NativeAudioEvent =
  | { type: 'input_level'; owner: NativeAudioOwner; level: number }
  | { type: 'snapshot_changed'; snapshot: NativeAudioSnapshot }
  | { type: 'helper_state'; snapshot: NativeAudioSnapshot; state: NativeAudioSnapshot['helper']['state']; message?: string }
  | { type: 'owner_changed'; snapshot: NativeAudioSnapshot; owner: NativeAudioOwner | null }
  | { type: 'recognition_state'; snapshot: NativeAudioSnapshot; progress: NativeAudioRecognitionProgress }
  | { type: 'speech_state'; snapshot: NativeAudioSnapshot; owner: NativeAudioOwner; state: 'starting' | 'speaking' | 'finished' | 'interrupted' }
  | { type: 'model_state'; snapshot: NativeAudioSnapshot; model: NativeAudioModelSnapshot }
  | { type: 'error'; snapshot: NativeAudioSnapshot; owner?: NativeAudioOwner; error: NativeAudioError };

export type NativeAudioCommand =
  | { type: 'get_snapshot' }
  | { type: 'cancel_operation'; identity: AudioOperationIdDto }
  | { type: 'request_authorization'; permissions: Array<'microphone' | 'speech'> }
  | { type: 'start_listening'; owner: NativeAudioOwner; recognitionMode: 'automatic' | 'localOnly'; language?: string; sampleRateHz?: number; format?: 'wav' | 'm4a' }
  | { type: 'finish_listening'; owner: NativeAudioOwner }
  | { type: 'cancel'; owner: NativeAudioOwner }
  | { type: 'speak'; owner: NativeAudioOwner; text: string; voiceId?: string; rate?: number }
  | { type: 'stop_speaking'; owner: NativeAudioOwner }
  | { type: 'list_models' }
  | { type: 'install_model'; modelId: string }
  | { type: 'cancel_model'; modelId: string }
  | { type: 'remove_model'; modelId: string };

export type NativeAudioHelperCommand = NativeAudioCommand | {
  type: 'finish_listening';
  owner: NativeAudioOwner;
  identity: AudioOperationIdDto;
};

export type NativeAudioResponse =
  | { type: 'snapshot'; snapshot: NativeAudioSnapshot }
  | { type: 'authorization'; snapshot: NativeAudioSnapshot }
  | { type: 'listening_started'; snapshot: NativeAudioSnapshot }
  | { type: 'listening_finished'; snapshot: NativeAudioSnapshot; transcript?: { text: string; language?: string; confidence?: number }; recording?: { audioBase64: string; mimeType: string } }
  | { type: 'cancelled'; snapshot: NativeAudioSnapshot }
  | { type: 'speaking_started'; snapshot: NativeAudioSnapshot }
  | { type: 'speaking_stopped'; snapshot: NativeAudioSnapshot }
  | { type: 'models'; snapshot: NativeAudioSnapshot; models: NativeAudioModelSnapshot[] }
  | { type: 'model_operation'; snapshot: NativeAudioSnapshot; model: NativeAudioModelSnapshot }
  | { type: 'error'; snapshot: NativeAudioSnapshot; error: NativeAudioError };

export interface NativeAudioEngineResponse {
  type: 'engine_result';
  snapshot: NativeAudioSnapshot;
  result: AudioOperationResultDto;
}

export interface NativeAudioOperationResponse {
  snapshot: NativeAudioSnapshot;
  result: AudioOperationResultDto;
}

export type NativeAudioCommandResult = NativeAudioResponse | NativeAudioEngineResponse;

export type NativeAudioHelperCommandEnvelope =
  | { id: string; kind: 'command'; command: NativeAudioHelperCommand }
  | {
      id: string;
      kind: 'engine_request';
      request: AudioOperationRequestDto;
      configuration: AudioConfigurationV3;
      configurationRevision: number;
    };

export type NativeAudioHelperEnvelope =
  | { type: 'event'; event: NativeAudioEvent }
  | { id: string; type: 'response'; result: NativeAudioCommandResult }
  | { id: string; type: 'error'; error: NativeAudioError };

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function object(value: unknown, label: string): Record<string, unknown> {
  if (!isObject(value)) throw new Error(`invalid ${label}`);
  return value;
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[], label: string): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw new Error(`invalid ${label}`);
}

function boundedString(value: unknown, label: string, max = 4_096): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) {
    throw new Error(`invalid ${label}`);
  }
  return value;
}

function boundedNumber(value: unknown, label: string, min = -Number.MAX_SAFE_INTEGER, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < min || value > max) throw new Error(`invalid ${label}`);
  return value;
}

function boundedInteger(value: unknown, label: string, min: number, max: number): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) throw new Error(`invalid ${label}`);
  const numeric = value as number;
  if (numeric < min || numeric > max) throw new Error(`invalid ${label}`);
  return numeric;
}

function validateOwner(value: unknown): NativeAudioOwner {
  const input = object(value, 'audio owner');
  exactKeys(input, ['kind', 'id'], 'audio owner');
  const kind = boundedString(input['kind'], 'audio owner kind', 32);
  if (
    kind !== 'dictation'
    && kind !== 'flow'
    && kind !== 'preview'
    && kind !== 'autoplay'
    && kind !== 'engine'
    && kind !== 'session'
    && kind !== 'local_app'
    && kind !== 'ui'
    && kind !== 'system'
  ) {
    throw new Error('invalid audio owner kind');
  }
  return { kind, id: boundedString(input['id'], 'audio owner id', MAX_OWNER_ID_LENGTH) };
}

function validateError(value: unknown): NativeAudioError {
  const input = object(value, 'audio error');
  exactKeys(input, ['code', 'message'], 'audio error');
  const code = boundedString(input['code'], 'audio error code', 32);
  if (
    code !== 'permission'
    && code !== 'unavailable'
    && code !== 'busy'
    && code !== 'cancelled'
    && code !== 'model-missing'
    && code !== 'download'
    && code !== 'checksum'
    && code !== 'invalid-request'
    && code !== 'native-error'
  ) {
    throw new Error('invalid audio error code');
  }
  return { code, message: boundedString(input['message'], 'audio error message', 2_048) };
}

function validateVoiceOption(value: unknown): NativeAudioVoiceOption {
  const input = object(value, 'audio voice');
  exactKeys(input, ['id', 'label', 'languageTag', 'source', 'familyId', 'isDefault', 'networkRequired'], 'audio voice');
  const source = boundedString(input['source'], 'audio voice source', 16);
  if (source !== 'system' && source !== 'sherpa') throw new Error('invalid audio voice source');
  return {
    id: boundedString(input['id'], 'audio voice id', MAX_VOICE_ID_LENGTH),
    label: boundedString(input['label'], 'audio voice label', 256),
    languageTag: boundedString(input['languageTag'], 'audio voice language', MAX_LANGUAGE_LENGTH),
    source,
    familyId: boundedString(input['familyId'], 'audio voice family', MAX_MODEL_ID_LENGTH),
    ...(typeof input['isDefault'] === 'boolean' ? { isDefault: input['isDefault'] } : {}),
    ...(typeof input['networkRequired'] === 'boolean' ? { networkRequired: input['networkRequired'] } : {}),
  };
}

function validateModelState(value: unknown): NativeAudioModelState {
  const input = object(value, 'audio model state');
  const type = boundedString(input['type'], 'audio model state type', 32);
  switch (type) {
    case 'not-installed':
    case 'queued':
    case 'verifying':
    case 'extracting':
    case 'ready':
      exactKeys(input, ['type'], 'audio model state');
      return { type };
    case 'downloading':
      exactKeys(input, ['type', 'receivedBytes', 'totalBytes'], 'audio model state');
      return {
        type,
        receivedBytes: boundedInteger(input['receivedBytes'], 'audio model received bytes', 0, Number.MAX_SAFE_INTEGER),
        totalBytes: boundedInteger(input['totalBytes'], 'audio model total bytes', 0, Number.MAX_SAFE_INTEGER),
      };
    case 'failed':
      exactKeys(input, ['type', 'message'], 'audio model state');
      return { type, message: boundedString(input['message'], 'audio model error', 2_048) };
    default:
      throw new Error('invalid audio model state type');
  }
}

function validateModel(value: unknown): NativeAudioModelSnapshot {
  const input = object(value, 'audio model');
  exactKeys(input, ['modelId', 'state'], 'audio model');
  return {
    modelId: boundedString(input['modelId'], 'audio model id', MAX_MODEL_ID_LENGTH),
    state: validateModelState(input['state']),
  };
}

function validateAudioOperationTrace(value: unknown): NativeAudioOperationTrace {
  const input = object(value, 'audio operation trace');
  exactKeys(input, [
    'identity', 'owner', 'operation', 'configurationRevision',
    'requestedSource', 'requestedModelId', 'requestedVoiceId',
    'effectiveSource', 'effectiveModelId', 'effectiveVoiceId', 'fallbackReason',
  ], 'audio operation trace');
  const operation = boundedString(input['operation'], 'audio trace operation', 32);
  if (![
    'start_recording', 'stop_recording', 'listen', 'synthesize', 'speak', 'status', 'end_owner',
  ].includes(operation)) throw new Error('invalid audio trace operation');
  return {
    identity: validateNativeAudioOperationIdentity(input['identity']),
    owner: validateAudioOwnerDto(input['owner']),
    operation: operation as NativeAudioTraceOperation,
    configurationRevision: boundedInteger(
      input['configurationRevision'], 'audio trace configuration revision', 0, Number.MAX_SAFE_INTEGER,
    ),
    ...(input['requestedSource'] === undefined ? {} : {
      requestedSource: boundedString(input['requestedSource'], 'audio requested source', 64),
    }),
    ...(input['requestedModelId'] === undefined ? {} : {
      requestedModelId: boundedString(input['requestedModelId'], 'audio requested model id', MAX_MODEL_ID_LENGTH),
    }),
    ...(input['requestedVoiceId'] === undefined ? {} : {
      requestedVoiceId: boundedString(input['requestedVoiceId'], 'audio requested voice id', MAX_VOICE_ID_LENGTH),
    }),
    ...(input['effectiveSource'] === undefined ? {} : {
      effectiveSource: boundedString(input['effectiveSource'], 'audio effective source', 64),
    }),
    ...(input['effectiveModelId'] === undefined ? {} : {
      effectiveModelId: boundedString(input['effectiveModelId'], 'audio effective model id', MAX_MODEL_ID_LENGTH),
    }),
    ...(input['effectiveVoiceId'] === undefined ? {} : {
      effectiveVoiceId: boundedString(input['effectiveVoiceId'], 'audio effective voice id', MAX_VOICE_ID_LENGTH),
    }),
    ...(input['fallbackReason'] === undefined ? {} : {
      fallbackReason: boundedString(input['fallbackReason'], 'audio fallback reason', 2_048),
    }),
  };
}

export function defaultNativeAudioSnapshot(): NativeAudioSnapshot {
  return {
    helper: { state: 'stopped' },
    permissions: { microphone: 'unavailable', speech: 'unavailable' },
    owner: null,
    activity: 'idle',
    voices: [],
    models: [],
    capabilities: {
      service_epoch: 0,
      support_revision: 0,
      supported_operations: [],
      readiness: [],
      max_payload_bytes: 0,
    },
    configurationRevision: 0,
    audioOperations: [],
    activeOperationCount: 0,
    pendingOperationCount: 0,
    activeRecordingCount: 0,
    activePlaybackCount: 0,
    activeModelReferenceCount: 0,
  };
}

export function validateNativeAudioSnapshot(value: unknown): NativeAudioSnapshot {
  const input = object(value, 'audio snapshot');
  exactKeys(
    input,
    [
      'helper', 'permissions', 'owner', 'activity', 'localeTag', 'recognizerAvailable', 'recognition', 'playback',
      'voices', 'models', 'capabilities', 'configurationRevision', 'currentOperation', 'audioOperations',
      'activeOperationCount', 'pendingOperationCount', 'activeRecordingCount', 'activePlaybackCount',
      'activeModelReferenceCount',
    ],
    'audio snapshot',
  );
  const helper = object(input['helper'], 'audio helper');
  exactKeys(helper, ['state', 'message'], 'audio helper');
  const helperState = boundedString(helper['state'], 'audio helper state', 16);
  if (helperState !== 'stopped' && helperState !== 'starting' && helperState !== 'running' && helperState !== 'failed') {
    throw new Error('invalid audio helper state');
  }
  const permissions = object(input['permissions'], 'audio permissions');
  exactKeys(permissions, ['microphone', 'speech'], 'audio permissions');
  const microphone = boundedString(permissions['microphone'], 'microphone permission', 16) as MicrophonePermissionStatus;
  if (microphone !== 'granted' && microphone !== 'denied' && microphone !== 'prompt' && microphone !== 'unavailable') {
    throw new Error('invalid microphone permission');
  }
  const speech = boundedString(permissions['speech'], 'speech permission', 32);
  if (speech !== 'authorized' && speech !== 'denied' && speech !== 'restricted' && speech !== 'not_determined' && speech !== 'unavailable') {
    throw new Error('invalid speech permission');
  }
  const activity = boundedString(input['activity'], 'audio activity', 16);
  if (activity !== 'idle' && activity !== 'listening' && activity !== 'recognizing' && activity !== 'speaking') {
    throw new Error('invalid audio activity');
  }
  const snapshot: NativeAudioSnapshot = {
    helper: {
      state: helperState,
      ...(helper['message'] === undefined ? {} : { message: boundedString(helper['message'], 'audio helper message', 2_048) }),
    },
    permissions: { microphone, speech },
    owner: input['owner'] === null || input['owner'] === undefined ? null : validateOwner(input['owner']),
    activity,
    voices: Array.isArray(input['voices']) ? input['voices'].map((entry) => validateVoiceOption(entry)) : [],
    models: Array.isArray(input['models']) ? input['models'].map((entry) => validateModel(entry)) : [],
    ...(input['capabilities'] === undefined ? {} : { capabilities: validateAudioCapability(input['capabilities']) }),
    ...(input['configurationRevision'] === undefined ? {} : {
      configurationRevision: boundedInteger(input['configurationRevision'], 'audio configuration revision', 0, Number.MAX_SAFE_INTEGER),
    }),
    ...(input['currentOperation'] === undefined ? {} : {
      currentOperation: (() => {
        const current = object(input['currentOperation'], 'audio current operation');
        exactKeys(current, ['identity', 'owner'], 'audio current operation');
        return { identity: validateNativeAudioOperationIdentity(current['identity']), owner: validateAudioOwnerDto(current['owner']) };
      })(),
    }),
    audioOperations: (() => {
      if (!Array.isArray(input['audioOperations']) || input['audioOperations'].length > 64) {
        throw new Error('invalid audio operation trace');
      }
      return input['audioOperations'].map((entry) => validateAudioOperationTrace(entry));
    })(),
    activeOperationCount: boundedInteger(input['activeOperationCount'], 'active audio operation count', 0, Number.MAX_SAFE_INTEGER),
    pendingOperationCount: boundedInteger(input['pendingOperationCount'], 'pending audio operation count', 0, Number.MAX_SAFE_INTEGER),
    activeRecordingCount: boundedInteger(input['activeRecordingCount'], 'active audio recording count', 0, Number.MAX_SAFE_INTEGER),
    activePlaybackCount: boundedInteger(input['activePlaybackCount'], 'active audio playback count', 0, Number.MAX_SAFE_INTEGER),
    activeModelReferenceCount: boundedInteger(input['activeModelReferenceCount'], 'active audio model reference count', 0, Number.MAX_SAFE_INTEGER),
  };
  if (input['localeTag'] !== undefined) snapshot.localeTag = boundedString(input['localeTag'], 'audio locale', MAX_LANGUAGE_LENGTH);
  if (typeof input['recognizerAvailable'] === 'boolean') snapshot.recognizerAvailable = input['recognizerAvailable'];
  if (input['recognition'] !== undefined) {
    const recognition = object(input['recognition'], 'audio recognition');
    exactKeys(recognition, ['requestedMode', 'effectiveBackend', 'effectiveLanguage', 'detail', 'fallbackReason'], 'audio recognition');
    const requestedMode = boundedString(recognition['requestedMode'], 'audio requested mode', 16);
    const effectiveBackend = boundedString(recognition['effectiveBackend'], 'audio recognition backend', 16);
    if (requestedMode !== 'automatic' && requestedMode !== 'localOnly') throw new Error('invalid audio recognition mode');
    if (effectiveBackend !== 'apple' && effectiveBackend !== 'sherpa' && effectiveBackend !== 'unavailable') {
      throw new Error('invalid audio recognition backend');
    }
    snapshot.recognition = {
      requestedMode,
      effectiveBackend,
      effectiveLanguage: boundedString(recognition['effectiveLanguage'], 'audio recognition language', MAX_LANGUAGE_LENGTH),
      detail: boundedString(recognition['detail'], 'audio recognition detail', 2_048),
      ...(recognition['fallbackReason'] === undefined ? {} : { fallbackReason: boundedString(recognition['fallbackReason'], 'audio fallback reason', 2_048) }),
    };
  }
  if (input['playback'] !== undefined) {
    const playback = object(input['playback'], 'audio playback');
    exactKeys(playback, ['requestedVoiceSelection', 'effectiveVoiceId', 'effectiveVoiceLabel'], 'audio playback');
    snapshot.playback = {
      requestedVoiceSelection: boundedString(playback['requestedVoiceSelection'], 'audio requested voice', MAX_VOICE_ID_LENGTH),
      effectiveVoiceId: boundedString(playback['effectiveVoiceId'], 'audio effective voice id', MAX_VOICE_ID_LENGTH),
      effectiveVoiceLabel: boundedString(playback['effectiveVoiceLabel'], 'audio effective voice label', 256),
    };
  }
  return snapshot;
}

export function validateNativeAudioCommand(value: unknown): NativeAudioCommand {
  const input = object(value, 'audio command');
  const type = boundedString(input['type'], 'audio command type', 32);
  switch (type) {
    case 'get_snapshot':
    case 'list_models':
      exactKeys(input, ['type'], 'audio command');
      return { type };
    case 'cancel_operation':
      exactKeys(input, ['type', 'identity'], 'audio command');
      return { type, identity: validateNativeAudioOperationIdentity(input['identity']) };
    case 'request_authorization': {
      exactKeys(input, ['type', 'permissions'], 'audio command');
      const permissions = input['permissions'];
      if (!Array.isArray(permissions) || permissions.length === 0 || permissions.length > 2) throw new Error('invalid audio permissions');
      return {
        type,
        permissions: permissions.map((entry) => {
          const permission = boundedString(entry, 'audio permission', 32);
          if (permission !== 'microphone' && permission !== 'speech') throw new Error('invalid audio permission');
          return permission;
        }),
      };
    }
    case 'start_listening':
      exactKeys(input, ['type', 'owner', 'recognitionMode', 'language', 'sampleRateHz', 'format'], 'audio command');
      return {
        type,
        owner: validateOwner(input['owner']),
        recognitionMode: input['recognitionMode'] === 'localOnly' ? 'localOnly' : 'automatic',
        ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio language', MAX_LANGUAGE_LENGTH) }),
        ...(input['sampleRateHz'] === undefined ? {} : { sampleRateHz: boundedInteger(input['sampleRateHz'], 'audio sample rate', MIN_SAMPLE_RATE_HZ, MAX_SAMPLE_RATE_HZ) }),
        ...(input['format'] === undefined ? {} : {
          format: (() => {
            const format = boundedString(input['format'], 'audio format', 16);
            if (format !== 'wav' && format !== 'm4a') throw new Error('invalid audio format');
            return format;
          })(),
        }),
      };
    case 'finish_listening':
    case 'cancel':
    case 'stop_speaking':
      exactKeys(input, ['type', 'owner'], 'audio command');
      return { type, owner: validateOwner(input['owner']) };
    case 'speak':
      exactKeys(input, ['type', 'owner', 'text', 'voiceId', 'rate'], 'audio command');
      return {
        type,
        owner: validateOwner(input['owner']),
        text: boundedString(input['text'], 'audio speech text', MAX_TEXT_LENGTH),
        ...(input['voiceId'] === undefined ? {} : { voiceId: boundedString(input['voiceId'], 'audio voice id', MAX_VOICE_ID_LENGTH) }),
        ...(input['rate'] === undefined ? {} : { rate: boundedNumber(input['rate'], 'audio rate', MIN_RATE, MAX_RATE) }),
      };
    case 'install_model':
    case 'cancel_model':
    case 'remove_model':
      exactKeys(input, ['type', 'modelId'], 'audio command');
      return { type, modelId: boundedString(input['modelId'], 'audio model id', MAX_MODEL_ID_LENGTH) };
    default:
      throw new Error('invalid audio command type');
  }
}

export function validateNativeAudioResponse(value: unknown): NativeAudioResponse {
  const input = object(value, 'audio response');
  exactKeys(input, ['type', 'snapshot', 'transcript', 'recording', 'models', 'model', 'error'], 'audio response');
  const type = boundedString(input['type'], 'audio response type', 32);
  const snapshot = validateNativeAudioSnapshot(input['snapshot']);
  switch (type) {
    case 'snapshot':
    case 'authorization':
    case 'listening_started':
    case 'cancelled':
    case 'speaking_started':
    case 'speaking_stopped':
      return { type, snapshot };
    case 'listening_finished':
      return {
        type,
        snapshot,
        ...(input['transcript'] === undefined ? {} : {
          transcript: (() => {
            const transcript = object(input['transcript'], 'audio transcript');
            exactKeys(transcript, ['text', 'language', 'confidence'], 'audio transcript');
            return {
              text: boundedString(transcript['text'], 'audio transcript text', MAX_TEXT_LENGTH),
              ...(transcript['language'] === undefined ? {} : { language: boundedString(transcript['language'], 'audio transcript language', MAX_LANGUAGE_LENGTH) }),
              ...(transcript['confidence'] === undefined ? {} : { confidence: boundedNumber(transcript['confidence'], 'audio transcript confidence', 0, 1) }),
            };
          })(),
        }),
        ...(input['recording'] === undefined ? {} : {
          recording: (() => {
            const recording = object(input['recording'], 'audio recording');
            exactKeys(recording, ['audioBase64', 'mimeType'], 'audio recording');
            if (!isSendableAudioBase64(recording['audioBase64'])) throw new Error('invalid audio recording');
            if (!isSendableAudioText(recording['mimeType'], MAX_AUDIO_MIME_TYPE_LENGTH)) throw new Error('invalid audio recording mime type');
            return { audioBase64: recording['audioBase64'], mimeType: recording['mimeType'] };
          })(),
        }),
      };
    case 'models':
      if (!Array.isArray(input['models'])) throw new Error('invalid audio models');
      return { type, snapshot, models: input['models'].map((entry) => validateModel(entry)) };
    case 'model_operation':
      return { type, snapshot, model: validateModel(input['model']) };
    case 'error':
      return { type, snapshot, error: validateError(input['error']) };
    default:
      throw new Error('invalid audio response type');
  }
}

export function validateNativeAudioOperationIdentity(value: unknown): AudioOperationIdDto {
  const input = object(value, 'audio operation identity');
  exactKeys(input, ['id', 'generation', 'service_epoch'], 'audio operation identity');
  const id = boundedString(input['id'], 'audio operation id', 64);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(id)) {
    throw new Error('invalid audio operation id');
  }
  return {
    id,
    generation: boundedInteger(input['generation'], 'audio operation generation', 0, Number.MAX_SAFE_INTEGER),
    service_epoch: boundedInteger(input['service_epoch'], 'audio service epoch', 0, Number.MAX_SAFE_INTEGER),
  };
}

function validateAudioOwnerDto(value: unknown): AudioOwnerDto {
  const input = object(value, 'audio operation owner');
  const type = boundedString(input['type'], 'audio owner type', 16);
  switch (type) {
    case 'session':
      exactKeys(input, ['type', 'session_id'], 'audio operation owner');
      return { type, session_id: boundedString(input['session_id'], 'audio session id', 128) };
    case 'local_app':
      exactKeys(input, ['type', 'app_id', 'runtime_generation'], 'audio operation owner');
      return {
        type,
        app_id: boundedString(input['app_id'], 'audio app id', 128),
        runtime_generation: boundedInteger(input['runtime_generation'], 'audio app runtime generation', 0, Number.MAX_SAFE_INTEGER),
      };
    case 'ui':
    case 'system':
      exactKeys(input, ['type', 'instance_id'], 'audio operation owner');
      return { type, instance_id: boundedString(input['instance_id'], 'audio owner instance id', MAX_OWNER_ID_LENGTH) };
    default:
      throw new Error('invalid audio operation owner');
  }
}

function validateAudioOperation(value: unknown): AudioOperationDto {
  const input = object(value, 'audio operation');
  const type = boundedString(input['type'], 'audio operation type', 32);
  switch (type) {
    case 'start_recording':
      exactKeys(input, ['type', 'sample_rate_hz', 'format'], 'audio operation');
      return {
        type,
        sample_rate_hz: boundedInteger(input['sample_rate_hz'], 'audio sample rate', MIN_SAMPLE_RATE_HZ, MAX_SAMPLE_RATE_HZ),
        format: boundedString(input['format'], 'audio format', 32),
      };
    case 'stop_recording':
      exactKeys(input, ['type', 'handle'], 'audio operation');
      return { type, handle: boundedString(input['handle'], 'audio recording handle', 128) };
    case 'listen':
      exactKeys(input, ['type', 'language'], 'audio operation');
      return { type, ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio language', MAX_LANGUAGE_LENGTH) }) };
    case 'synthesize':
    case 'speak':
      exactKeys(input, ['type', 'text', 'language', 'rate', 'voice'], 'audio operation');
      return {
        type,
        text: boundedString(input['text'], 'audio speech text', MAX_TEXT_LENGTH),
        ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio language', MAX_LANGUAGE_LENGTH) }),
        ...(input['rate'] === undefined ? {} : { rate: boundedNumber(input['rate'], 'audio rate', MIN_RATE, MAX_RATE) }),
        ...(input['voice'] === undefined ? {} : { voice: boundedString(input['voice'], 'audio voice', MAX_VOICE_ID_LENGTH) }),
      };
    case 'status':
      exactKeys(input, ['type', 'handle'], 'audio operation');
      return { type, ...(input['handle'] === undefined ? {} : { handle: boundedString(input['handle'], 'audio recording handle', 128) }) };
    case 'end_owner':
      exactKeys(input, ['type'], 'audio operation');
      return { type };
    default:
      throw new Error('invalid audio operation');
  }
}

export function validateNativeAudioOperation(value: unknown): AudioOperationDto {
  return validateAudioOperation(value);
}

function validateAudioKind(value: unknown): AudioErrorKindDto {
  const kind = boundedString(value, 'audio error kind', 32);
  if (![
    'permission_denied', 'busy', 'cancelled', 'timeout', 'no_speech', 'not_recording',
    'unavailable', 'unsupported', 'model_missing', 'voice_missing', 'invalid_request',
    'synthesis_failed', 'native_failure', 'media_too_large',
  ].includes(kind)) throw new Error('invalid audio error kind');
  return kind as AudioErrorKindDto;
}

function validateAudioCapability(value: unknown): AudioCapabilitySnapshotDto {
  const input = object(value, 'audio capability snapshot');
  exactKeys(input, ['service_epoch', 'support_revision', 'supported_operations', 'readiness', 'max_payload_bytes'], 'audio capability snapshot');
  if (!Array.isArray(input['supported_operations']) || !Array.isArray(input['readiness'])) throw new Error('invalid audio capability arrays');
  const operationKind = (entry: unknown): AudioCapabilitySnapshotDto['supported_operations'][number] => {
    const kind = boundedString(entry, 'audio operation kind', 16);
    if (kind !== 'record' && kind !== 'listen' && kind !== 'synthesize' && kind !== 'speak') throw new Error('invalid audio operation kind');
    return kind;
  };
  const readiness = input['readiness'].map((entry): AudioCapabilitySnapshotDto['readiness'][number] => {
    const row = object(entry, 'audio readiness entry');
    exactKeys(row, ['operation', 'state'], 'audio readiness entry');
    const state = boundedString(row['state'], 'audio readiness state', 24);
    if (!['ready', 'needs_permission', 'busy', 'missing_model', 'unavailable'].includes(state)) throw new Error('invalid audio readiness state');
    return {
      operation: operationKind(row['operation']),
      state: state as AudioCapabilitySnapshotDto['readiness'][number]['state'],
    };
  });
  return {
    service_epoch: boundedInteger(input['service_epoch'], 'audio service epoch', 0, Number.MAX_SAFE_INTEGER),
    support_revision: boundedInteger(input['support_revision'], 'audio support revision', 0, Number.MAX_SAFE_INTEGER),
    supported_operations: input['supported_operations'].map(operationKind),
    readiness,
    max_payload_bytes: boundedInteger(input['max_payload_bytes'], 'audio payload bound', 0, MAX_AUDIO_PAYLOAD_BYTES),
  };
}

export function validateNativeAudioOperationRequest(value: unknown): AudioOperationRequestDto {
  const input = object(value, 'audio operation request');
  exactKeys(input, ['identity', 'owner', 'initiator', 'timeout_budget_ms', 'max_payload_bytes', 'operation'], 'audio operation request');
  let initiator: AudioOperationRequestDto['initiator'];
  if (input['initiator'] !== undefined) {
    const raw = object(input['initiator'], 'audio operation initiator');
    exactKeys(raw, ['agent_id', 'tool_use_id', 'request_id'], 'audio operation initiator');
    initiator = {
      ...(raw['agent_id'] === undefined ? {} : { agent_id: boundedString(raw['agent_id'], 'audio agent id', 128) }),
      ...(raw['tool_use_id'] === undefined ? {} : { tool_use_id: boundedString(raw['tool_use_id'], 'audio tool use id', 128) }),
      ...(raw['request_id'] === undefined ? {} : { request_id: boundedString(raw['request_id'], 'audio request id', 128) }),
    };
  }
  const timeout = input['timeout_budget_ms'] === undefined
    ? undefined : boundedInteger(input['timeout_budget_ms'], 'audio timeout budget', 0, Number.MAX_SAFE_INTEGER);
  return {
    identity: validateNativeAudioOperationIdentity(input['identity']),
    owner: validateAudioOwnerDto(input['owner']),
    ...(initiator === undefined ? {} : { initiator }),
    ...(timeout === undefined ? {} : { timeout_budget_ms: timeout }),
    max_payload_bytes: boundedInteger(input['max_payload_bytes'], 'audio payload bound', 0, MAX_AUDIO_PAYLOAD_BYTES),
    operation: validateAudioOperation(input['operation']),
  };
}

export function validateNativeAudioOperationResult(value: unknown): AudioOperationResultDto {
  const input = object(value, 'audio operation result');
  const type = boundedString(input['type'], 'audio operation result type', 32);
  switch (type) {
    case 'recording_started':
      exactKeys(input, ['type', 'handle'], 'audio operation result');
      return { type, handle: boundedString(input['handle'], 'audio recording handle', 128) };
    case 'recording': {
      exactKeys(input, ['type', 'audio_base64', 'mime_type'], 'audio operation result');
      const audio = input['audio_base64'];
      if (!isSendableAudioBase64(audio) || audio.length > MAX_AUDIO_BASE64_LENGTH) throw new Error('invalid or oversized audio recording payload');
      if (!isSendableAudioText(input['mime_type'], MAX_AUDIO_MIME_TYPE_LENGTH)) throw new Error('invalid audio recording mime type');
      return { type, audio_base64: audio, mime_type: input['mime_type'] };
    }
    case 'transcript':
      exactKeys(input, ['type', 'text', 'language', 'confidence'], 'audio operation result');
      return {
        type,
        text: boundedString(input['text'], 'audio transcript text', MAX_TEXT_LENGTH),
        ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio transcript language', MAX_LANGUAGE_LENGTH) }),
        ...(input['confidence'] === undefined ? {} : { confidence: boundedNumber(input['confidence'], 'audio transcript confidence', 0, 1) }),
      };
    case 'synthesized': {
      exactKeys(input, ['type', 'pcm_base64', 'sample_rate_hz'], 'audio operation result');
      const pcm = input['pcm_base64'];
      if (!isSendableAudioBase64(pcm) || pcm.length === 0 || pcm.length > MAX_AUDIO_BASE64_LENGTH) throw new Error('invalid or oversized synthesized audio payload');
      const padding = pcm.endsWith('==') ? 2 : pcm.endsWith('=') ? 1 : 0;
      const decodedLength = (pcm.length / 4) * 3 - padding;
      if (decodedLength <= 0 || decodedLength % 2 !== 0 || decodedLength > MAX_AUDIO_PAYLOAD_BYTES) {
        throw new Error('synthesized PCM must be nonempty aligned PCM16');
      }
      const rate = input['sample_rate_hz'];
      if (!Number.isInteger(rate) || (rate as number) < MIN_SAMPLE_RATE_HZ || (rate as number) > MAX_SAMPLE_RATE_HZ) throw new Error('invalid synthesized audio sample rate');
      return { type, pcm_base64: pcm, sample_rate_hz: rate as number };
    }
    case 'playback_completed':
      exactKeys(input, ['type', 'duration_ms'], 'audio operation result');
      return { type, duration_ms: boundedInteger(input['duration_ms'], 'audio playback duration', 0, Number.MAX_SAFE_INTEGER) };
    case 'status': {
      exactKeys(input, ['type', 'status'], 'audio operation result');
      const status = object(input['status'], 'audio status');
      exactKeys(status, ['recording', 'playing'], 'audio status');
      if (typeof status['recording'] !== 'boolean' || typeof status['playing'] !== 'boolean') throw new Error('invalid audio status');
      return { type, status: { recording: status['recording'], playing: status['playing'] } };
    }
    case 'owner_ended':
      exactKeys(input, ['type'], 'audio operation result');
      return { type };
    case 'failed': {
      exactKeys(input, ['type', 'error'], 'audio operation result');
      const error = object(input['error'], 'audio error');
      exactKeys(error, ['kind', 'message'], 'audio error');
      const message = boundedString(error['message'], 'audio error message', MAX_AUDIO_FAILURE_MESSAGE_LENGTH);
      return { type, error: { kind: validateAudioKind(error['kind']), message } };
    }
    default:
      throw new Error('invalid audio operation result type');
  }
}

export function validateNativeAudioEngineResponse(value: unknown): NativeAudioEngineResponse {
  const input = object(value, 'audio engine response');
  exactKeys(input, ['type', 'snapshot', 'result'], 'audio engine response');
  if (input['type'] !== 'engine_result') throw new Error('invalid audio engine response type');
  return {
    type: 'engine_result',
    snapshot: validateNativeAudioSnapshot(input['snapshot']),
    result: validateNativeAudioOperationResult(input['result']),
  };
}

export function validateNativeAudioEvent(value: unknown): NativeAudioEvent {
  const input = object(value, 'audio event');
  const type = boundedString(input['type'], 'audio event type', 32);
  if (type === 'input_level') {
    exactKeys(input, ['type', 'owner', 'level'], 'audio input level event');
    return {
      type,
      owner: validateOwner(input['owner']),
      level: boundedNumber(input['level'], 'audio input level', 0, 1),
    };
  }
  exactKeys(input, ['type', 'snapshot', 'state', 'message', 'owner', 'progress', 'model', 'error'], 'audio event');
  const snapshot = validateNativeAudioSnapshot(input['snapshot']);
  switch (type) {
    case 'snapshot_changed':
      return { type, snapshot };
    case 'helper_state': {
      const state = boundedString(input['state'], 'audio helper event state', 16) as NativeAudioSnapshot['helper']['state'];
      return { type, snapshot, state, ...(input['message'] === undefined ? {} : { message: boundedString(input['message'], 'audio helper message', 2_048) }) };
    }
    case 'owner_changed':
      return { type, snapshot, owner: input['owner'] === null || input['owner'] === undefined ? null : validateOwner(input['owner']) };
    case 'recognition_state': {
      const progress = object(input['progress'], 'audio recognition progress');
      exactKeys(progress, ['owner', 'text', 'isFinal'], 'audio recognition progress');
      return {
        type,
        snapshot,
        progress: {
          owner: validateOwner(progress['owner']),
          text: boundedString(progress['text'], 'audio recognition text', MAX_TEXT_LENGTH),
          isFinal: progress['isFinal'] === true,
        },
      };
    }
    case 'speech_state': {
      const state = boundedString(input['state'], 'audio speech state', 16);
      if (state !== 'starting' && state !== 'speaking' && state !== 'finished' && state !== 'interrupted') {
        throw new Error('invalid audio speech state');
      }
      return { type, snapshot, owner: validateOwner(input['owner']), state };
    }
    case 'model_state':
      return { type, snapshot, model: validateModel(input['model']) };
    case 'error':
      return { type, snapshot, ...(input['owner'] === undefined ? {} : { owner: validateOwner(input['owner']) }), error: validateError(input['error']) };
    default:
      throw new Error('invalid audio event type');
  }
}
