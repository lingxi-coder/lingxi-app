import type { AudioErrorKindDto, AudioOpDto, AudioResultDto } from '@lingxi/bridge-client';
import type { MicrophonePermissionStatus } from './microphoneAccess.js';
import {
  isSendableAudioBase64,
  isSendableAudioSampleRate,
  isSendableAudioText,
  MAX_AUDIO_FAILURE_MESSAGE_LENGTH,
  MAX_AUDIO_MIME_TYPE_LENGTH,
} from './audioResponse.js';

const MAX_OWNER_ID_LENGTH = 512;
const MAX_LANGUAGE_LENGTH = 64;
const MAX_TEXT_LENGTH = 256 * 1024;
const MAX_VOICE_ID_LENGTH = 512;
const MAX_MODEL_ID_LENGTH = 256;
const MIN_SAMPLE_RATE_HZ = 8_000;
const MAX_SAMPLE_RATE_HZ = 48_000;
const MIN_RATE = 0.5;
const MAX_RATE = 2.0;

export const CH_NATIVE_AUDIO_REQUEST = 'lingxi:audio:request';
export const CH_NATIVE_AUDIO_EVENT = 'lingxi:audio:event';
export const CH_NATIVE_AUDIO_ENGINE_REQUEST = 'lingxi:audio:engine-request';

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
  kind: 'dictation' | 'flow' | 'preview' | 'autoplay' | 'engine';
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
  voices: NativeAudioVoiceOption[];
  models: NativeAudioModelSnapshot[];
}

export interface NativeAudioRecognitionProgress {
  owner: NativeAudioOwner;
  text: string;
  isFinal: boolean;
}

export type NativeAudioEvent =
  | { type: 'snapshot_changed'; snapshot: NativeAudioSnapshot }
  | { type: 'helper_state'; snapshot: NativeAudioSnapshot; state: NativeAudioSnapshot['helper']['state']; message?: string }
  | { type: 'owner_changed'; snapshot: NativeAudioSnapshot; owner: NativeAudioOwner | null }
  | { type: 'recognition_state'; snapshot: NativeAudioSnapshot; progress: NativeAudioRecognitionProgress }
  | { type: 'speech_state'; snapshot: NativeAudioSnapshot; owner: NativeAudioOwner; state: 'starting' | 'speaking' | 'finished' | 'interrupted' }
  | { type: 'model_state'; snapshot: NativeAudioSnapshot; model: NativeAudioModelSnapshot }
  | { type: 'error'; snapshot: NativeAudioSnapshot; owner?: NativeAudioOwner; error: NativeAudioError };

export type NativeAudioCommand =
  | { type: 'get_snapshot' }
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
  result: AudioResultDto;
}

export type NativeAudioCommandResult = NativeAudioResponse | NativeAudioEngineResponse;

export type NativeAudioHelperCommandEnvelope =
  | { id: string; kind: 'command'; command: NativeAudioCommand }
  | { id: string; kind: 'engine_request'; owner: NativeAudioOwner; op: AudioOpDto };

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
  if (kind !== 'dictation' && kind !== 'flow' && kind !== 'preview' && kind !== 'autoplay' && kind !== 'engine') {
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

export function defaultNativeAudioSnapshot(): NativeAudioSnapshot {
  return {
    helper: { state: 'stopped' },
    permissions: { microphone: 'unavailable', speech: 'unavailable' },
    owner: null,
    activity: 'idle',
    voices: [],
    models: [],
  };
}

export function validateNativeAudioSnapshot(value: unknown): NativeAudioSnapshot {
  const input = object(value, 'audio snapshot');
  exactKeys(
    input,
    ['helper', 'permissions', 'owner', 'activity', 'localeTag', 'recognizerAvailable', 'recognition', 'playback', 'voices', 'models'],
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

export function validateNativeAudioEngineRequest(sessionId: unknown, op: unknown): { sessionId: string; op: AudioOpDto } {
  const input = object(op, 'audio op');
  const type = boundedString(input['type'], 'audio op type', 32);
  switch (type) {
    case 'is_recording':
    case 'stop_recording':
      exactKeys(input, ['type'], 'audio op');
      return { sessionId: boundedString(sessionId, 'audio session id', 128), op: { type } };
    case 'start_recording':
      exactKeys(input, ['type', 'sample_rate_hz', 'format'], 'audio op');
      return {
        sessionId: boundedString(sessionId, 'audio session id', 128),
        op: {
          type,
          sample_rate_hz: boundedInteger(input['sample_rate_hz'], 'audio op sample rate', MIN_SAMPLE_RATE_HZ, MAX_SAMPLE_RATE_HZ),
          format: boundedString(input['format'], 'audio op format', 32),
        },
      };
    case 'transcribe':
      exactKeys(input, ['type', 'language'], 'audio op');
      return {
        sessionId: boundedString(sessionId, 'audio session id', 128),
        op: {
          type,
          ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio op language', MAX_LANGUAGE_LENGTH) }),
        },
      };
    case 'synthesize':
      exactKeys(input, ['type', 'text', 'voice'], 'audio op');
      return {
        sessionId: boundedString(sessionId, 'audio session id', 128),
        op: {
          type,
          text: boundedString(input['text'], 'audio op text', MAX_TEXT_LENGTH),
          ...(input['voice'] === undefined ? {} : { voice: boundedString(input['voice'], 'audio op voice', MAX_VOICE_ID_LENGTH) }),
        },
      };
    default:
      throw new Error('invalid audio op');
  }
}

function validateAudioResult(value: unknown): AudioResultDto {
  const input = object(value, 'audio engine result');
  const type = boundedString(input['type'], 'audio engine result type', 32);
  switch (type) {
    case 'ok':
      exactKeys(input, ['type'], 'audio engine result');
      return { type };
    case 'recording_state':
      exactKeys(input, ['type', 'recording'], 'audio engine result');
      if (typeof input['recording'] !== 'boolean') throw new Error('invalid audio engine recording state');
      return { type, recording: input['recording'] };
    case 'recording':
      exactKeys(input, ['type', 'audio_base64', 'mime_type'], 'audio engine result');
      if (!isSendableAudioBase64(input['audio_base64'])) throw new Error('invalid audio engine recording');
      if (!isSendableAudioText(input['mime_type'], MAX_AUDIO_MIME_TYPE_LENGTH)) throw new Error('invalid audio engine recording mime');
      return { type, audio_base64: input['audio_base64'], mime_type: input['mime_type'] };
    case 'transcript':
      exactKeys(input, ['type', 'text', 'language', 'confidence'], 'audio engine result');
      return {
        type,
        text: boundedString(input['text'], 'audio engine transcript text', MAX_TEXT_LENGTH),
        ...(input['language'] === undefined ? {} : { language: boundedString(input['language'], 'audio engine transcript language', MAX_LANGUAGE_LENGTH) }),
        ...(input['confidence'] === undefined ? {} : { confidence: boundedNumber(input['confidence'], 'audio engine transcript confidence', 0, 1) }),
      };
    case 'audio':
      exactKeys(input, ['type', 'pcm_base64', 'sample_rate_hz'], 'audio engine result');
      if (!isSendableAudioBase64(input['pcm_base64'])) throw new Error('invalid audio engine pcm');
      if (!isSendableAudioSampleRate(input['sample_rate_hz'])) throw new Error('invalid audio engine sample rate');
      return { type, pcm_base64: input['pcm_base64'], sample_rate_hz: input['sample_rate_hz'] };
    case 'failed': {
      exactKeys(input, ['type', 'kind', 'message'], 'audio engine result');
      const kind = boundedString(input['kind'], 'audio engine error kind', 32) as AudioErrorKindDto;
      if (!['permission_denied', 'no_speech', 'not_recording', 'unavailable', 'busy', 'retriable', 'synthesis_failed', 'other'].includes(kind)) {
        throw new Error('invalid audio engine error kind');
      }
      if (!isSendableAudioText(input['message'], MAX_AUDIO_FAILURE_MESSAGE_LENGTH)) throw new Error('invalid audio engine error message');
      return { type, kind, message: input['message'] };
    }
    default:
      throw new Error('invalid audio engine result type');
  }
}

export function validateNativeAudioEngineResponse(value: unknown): NativeAudioEngineResponse {
  const input = object(value, 'audio engine response');
  exactKeys(input, ['type', 'snapshot', 'result'], 'audio engine response');
  if (input['type'] !== 'engine_result') throw new Error('invalid audio engine response type');
  return {
    type: 'engine_result',
    snapshot: validateNativeAudioSnapshot(input['snapshot']),
    result: validateAudioResult(input['result']),
  };
}

export function validateNativeAudioEvent(value: unknown): NativeAudioEvent {
  const input = object(value, 'audio event');
  exactKeys(input, ['type', 'snapshot', 'state', 'message', 'owner', 'progress', 'model', 'error'], 'audio event');
  const type = boundedString(input['type'], 'audio event type', 32);
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

function errorCodeToKind(code: NativeAudioErrorCode): AudioErrorKindDto {
  switch (code) {
    case 'permission':
      return 'permission_denied';
    case 'busy':
      return 'busy';
    case 'cancelled':
      return 'retriable';
    case 'model-missing':
    case 'unavailable':
      return 'unavailable';
    case 'download':
    case 'checksum':
    case 'invalid-request':
    case 'native-error':
      return 'other';
  }
}

export function nativeAudioResponseToEngineResult(response: NativeAudioResponse): AudioResultDto {
  switch (response.type) {
    case 'listening_finished':
      if (response.recording) {
        return {
          type: 'recording',
          audio_base64: response.recording.audioBase64,
          mime_type: response.recording.mimeType,
        };
      }
      if (response.transcript) {
        return {
          type: 'transcript',
          text: response.transcript.text,
          ...(response.transcript.language ? { language: response.transcript.language } : {}),
          ...(response.transcript.confidence === undefined ? {} : { confidence: response.transcript.confidence }),
        };
      }
      return { type: 'failed', kind: 'other', message: 'native audio finished without a transcript or recording' };
    case 'snapshot':
    case 'authorization':
    case 'listening_started':
    case 'speaking_started':
    case 'speaking_stopped':
    case 'models':
    case 'model_operation':
      return { type: 'ok' };
    case 'cancelled':
      return { type: 'failed', kind: 'retriable', message: 'native audio operation was cancelled' };
    case 'error':
      return { type: 'failed', kind: errorCodeToKind(response.error.code), message: response.error.message };
  }
}
