/**
 * TypeScript mirror of `client::protocol::media`.
 *
 * Keep serde tags and field names in lockstep with Harness. Provider
 * credentials, SDP and continuous audio/video bytes never enter model DTOs.
 */

export const MAX_MEDIA_DEVICE_FRAME_BYTES = 8 * 1024 * 1024;
export const MAX_MEDIA_DEVICE_WIRE_BYTES = 16 * 1024 * 1024;
export const MAX_REMOTE_MEDIA_SDP_BYTES = 64 * 1024;
export const MAX_MEDIA_AUDIO_SETTINGS_JSON_BYTES = 64 * 1024;

export interface MediaOperationIdDto { id: string; generation: number; service_epoch: number }
export type MediaOwnerDto =
  | { type: 'session'; session_id: string }
  | { type: 'ui'; instance_id: string }
  | { type: 'system'; instance_id: string };
export interface MediaInitiatorDto { agent_id?: string; tool_use_id?: string; request_id?: string }
export type MediaKindDto = 'audio' | 'image' | 'video';
export type MediaSourceKindDto = 'microphone' | 'camera' | 'screen' | 'speaker' | 'library' | 'remote';
export type MediaSourceUsageDto = 'capture' | 'record' | 'transcribe' | 'play' | 'speak' | 'session' | 'read_image';
export type MediaOperationKindDto =
  | 'sources' | 'status' | 'capture' | 'record' | 'transcribe' | 'synthesize' | 'play' | 'speak'
  | 'session_start' | 'session_update' | 'observe' | 'wait' | 'stop';
export type MediaReadinessStateDto = 'ready' | 'needs_permission' | 'busy' | 'missing_model' | 'unavailable';
export interface MediaOperationReadinessDto { operation: MediaOperationKindDto; state: MediaReadinessStateDto }
export interface MediaCapabilitiesDto {
  service_epoch: number;
  support_revision: number;
  supported_operations: MediaOperationKindDto[];
  supported_media_kinds: MediaKindDto[];
  readiness: MediaOperationReadinessDto[];
  max_payload_bytes: number;
}
export interface MediaSourceRefDto { id: string }
export type MediaResourceRefDto =
  | { type: 'source'; source: MediaSourceRefDto }
  | { type: 'recording'; recording_id: string }
  | { type: 'session'; session_id: string }
  | { type: 'media'; media_ref: string }
  | { type: 'operation'; operation_id: MediaOperationIdDto };
export interface MediaSourceDto {
  reference: MediaSourceRefDto;
  kind: MediaSourceKindDto;
  media_kind?: MediaKindDto | null;
  label: string;
  usages: MediaSourceUsageDto[];
  supported_operations: MediaOperationKindDto[];
  is_default: boolean;
}
export interface MediaMetadataDto {
  kind: MediaKindDto;
  mime_type: string;
  byte_len: number;
  captured_at_unix_ms?: number | null;
  duration_ms?: number | null;
  width_px?: number | null;
  height_px?: number | null;
  sample_rate_hz?: number | null;
  channels?: number | null;
}
export interface MediaAssetDto { media_ref: string; metadata: MediaMetadataDto }
export interface MediaCaptureOptionsDto {
  max_duration_ms?: number | null;
  max_dimension_px?: number | null;
  jpeg_quality?: number | null;
}
export interface MediaRecordOptionsDto { max_duration_ms?: number | null; mime_type?: string | null }
export type MediaTranscriptionInputDto =
  | { type: 'live_source'; source: MediaSourceRefDto }
  | { type: 'stored_media'; media_ref: string };
export type MediaSessionModeDto = 'audio' | 'video' | 'audio_video';
export type MediaSessionIntentDto = 'observation' | 'conversation';
export interface MediaSessionRequestDto {
  mode: MediaSessionModeDto;
  intent: MediaSessionIntentDto;
  audio_source?: MediaSourceRefDto | null;
  video_source?: MediaSourceRefDto | null;
}
export type MediaSessionUpdateDto =
  | { type: 'set_enabled'; kind: MediaKindDto; enabled: boolean }
  | { type: 'select_source'; kind: MediaKindDto; source: MediaSourceRefDto }
  | { type: 'set_playback_gain'; gain: number };
export type MediaSessionStateDto = 'preparing' | 'ready' | 'reconnecting' | 'ended' | 'failed';
export interface MediaSessionStatusDto { state: MediaSessionStateDto; audio_enabled: boolean; video_enabled: boolean }

export type MediaOperationDto =
  | { type: 'sources'; kind?: MediaKindDto; usage?: MediaSourceUsageDto }
  | { type: 'status'; target?: MediaResourceRefDto }
  | { type: 'capture'; source: MediaSourceRefDto; options: MediaCaptureOptionsDto }
  | { type: 'record'; source: MediaSourceRefDto; options: MediaRecordOptionsDto }
  | { type: 'transcribe'; input: MediaTranscriptionInputDto; language?: string }
  | { type: 'synthesize'; text: string; language?: string; rate?: number; voice?: string }
  | { type: 'play'; media_ref: string; source?: MediaSourceRefDto }
  | { type: 'speak'; text: string; language?: string; rate?: number; voice?: string }
  | { type: 'session_start'; request: MediaSessionRequestDto }
  | { type: 'session_update'; session_id: string; update: MediaSessionUpdateDto }
  | { type: 'observe'; target: MediaResourceRefDto; after_sequence?: number; limit: number }
  | { type: 'wait'; target: MediaResourceRefDto; after_sequence?: number; timeout_ms: number; limit: number }
  | { type: 'stop'; target: MediaResourceRefDto };

export type MediaResourceStateDto = 'ready' | 'active' | 'stopping' | 'completed' | 'failed';
export interface MediaResourceStatusDto {
  resource: MediaResourceRefDto;
  state: MediaResourceStateDto;
  started_at_unix_ms?: number;
  updated_at_unix_ms?: number;
}
export interface MediaEventBatchDto {
  target: MediaResourceRefDto;
  events: MediaEventDto[];
  next_sequence?: number;
  dropped_before_cursor: boolean;
}
export interface MediaEventDto {
  sequence: number;
  occurred_at_unix_ms: number;
  target: MediaResourceRefDto;
  operation_id?: MediaOperationIdDto;
  kind: MediaEventKindDto;
}
export type MediaEventKindDto =
  | { type: 'state_changed'; resource: MediaResourceRefDto; state: MediaResourceStateDto }
  | { type: 'frame'; source: MediaSourceRefDto; asset: MediaAssetDto }
  | { type: 'transcript'; text: string; language?: string; confidence?: number }
  | { type: 'error'; kind: MediaErrorKindDto; message: string };
export type MediaErrorKindDto =
  | 'permission_denied' | 'busy' | 'cancelled' | 'timeout' | 'no_data' | 'not_found' | 'unavailable'
  | 'unsupported' | 'model_missing' | 'invalid_request' | 'processing_failed' | 'native_failure'
  | 'store_failure' | 'media_too_large';
export interface MediaErrorDto { kind: MediaErrorKindDto; message: string }
export interface MediaContentDto { media_ref: string; metadata: MediaMetadataDto; bytes: number[] }

export type MediaResultDto =
  | { type: 'sources'; sources: MediaSourceDto[] }
  | { type: 'status'; resources: MediaResourceStatusDto[] }
  | { type: 'captured'; asset: MediaAssetDto }
  | { type: 'recording_started'; recording_id: string }
  | { type: 'recording_stopped'; recording_id: string; asset: MediaAssetDto }
  | { type: 'transcript'; text: string; language?: string; confidence?: number }
  | { type: 'synthesized'; asset: MediaAssetDto }
  | { type: 'playback_completed'; duration_ms: number }
  | { type: 'speech_completed'; duration_ms: number }
  | { type: 'session_started'; session_id: string; status: MediaSessionStatusDto }
  | { type: 'session_updated'; session_id: string; status: MediaSessionStatusDto }
  | { type: 'events'; batch: MediaEventBatchDto }
  | { type: 'stopped'; resource: MediaResourceRefDto };

export type MediaServiceActionDto =
  | { type: 'execute'; operation: MediaOperationDto; timeout_budget_ms?: number | null; max_payload_bytes?: number | null }
  | { type: 'read_media'; media_ref: string }
  | { type: 'cancel'; operation_id: MediaOperationIdDto }
  | { type: 'update_audio_settings'; configuration_json: string }
  | { type: 'register_connection'; connection_id: string; label: string; supported_modes: MediaSessionModeDto[]; source_kinds: Array<'microphone' | 'camera' | 'screen'> }
  | { type: 'unregister_connection'; connection_id: string };
export interface MediaServiceRequestDto { request_id: string; action: MediaServiceActionDto }
export type MediaServiceResultDto =
  | { type: 'operation'; operation_id: MediaOperationIdDto; result: MediaResultDto }
  | { type: 'content'; content: MediaContentDto }
  | { type: 'cancelled' }
  | { type: 'settings_updated' }
  | { type: 'connection_updated' };
export interface MediaServiceResponseDto {
  request_id: string;
  result?: MediaServiceResultDto | null;
  error?: MediaErrorDto | null;
}

export interface MediaDeviceLeaseDto { lease_id: string }
export interface MediaPcmFrameDto { sample_rate_hz: number; channels: number; pcm16le: number[] }
export type MediaVideoFrameDto =
  | { type: 'rgba8'; width_px: number; height_px: number; stride_bytes: number; bytes: number[] }
  | { type: 'i420'; width_px: number; height_px: number; y_stride_bytes: number; u_stride_bytes: number; v_stride_bytes: number; y: number[]; u: number[]; v: number[] };
export type MediaFramePayloadDto =
  | { type: 'audio'; frame: MediaPcmFrameDto }
  | { type: 'video'; frame: MediaVideoFrameDto };
export interface MediaFrameDto {
  source: MediaSourceRefDto;
  sequence: number;
  captured_at_unix_ms: number;
  payload: MediaFramePayloadDto;
}
export interface MediaRenderReceiptDto {
  accepted_sample_frames: number;
  played_sample_frames: number;
  rendered_at_unix_ms?: number | null;
}
export interface MediaDeviceOpenRequestDto {
  identity: MediaOperationIdDto;
  owner: MediaOwnerDto;
  initiator?: MediaInitiatorDto | null;
  timeout_budget_ms?: number | null;
  max_payload_bytes: number;
  request: MediaSessionRequestDto;
}
export interface MediaDeviceUpdateRequestDto { lease_id: string; update: MediaSessionUpdateDto }
export interface MediaDeviceAudioWriteRequestDto { lease_id: string; frame: MediaPcmFrameDto }
export interface MediaDeviceVideoWriteRequestDto { lease_id: string; frame: MediaVideoFrameDto }
export interface MediaDeviceAudioControlRequestDto { lease_id: string }
export type MediaDeviceRequestDto =
  | { type: 'open'; request: MediaDeviceOpenRequestDto }
  | { type: 'next_input'; lease_id: string }
  | { type: 'write_audio'; request: MediaDeviceAudioWriteRequestDto }
  | { type: 'write_video'; request: MediaDeviceVideoWriteRequestDto }
  | { type: 'flush_audio'; request: MediaDeviceAudioControlRequestDto }
  | { type: 'interrupt_audio'; request: MediaDeviceAudioControlRequestDto }
  | { type: 'update'; request: MediaDeviceUpdateRequestDto }
  | { type: 'stop'; lease_id: string };
export interface MediaDeviceRequestEnvelopeDto { request_id: string; request: MediaDeviceRequestDto }
export type MediaDeviceResultDto =
  | { type: 'opened'; lease: MediaDeviceLeaseDto }
  | { type: 'input'; frame: MediaFrameDto | null }
  | { type: 'audio_written'; receipt: MediaRenderReceiptDto }
  | { type: 'audio_flushed'; receipt: MediaRenderReceiptDto }
  | { type: 'audio_interrupted'; receipt: MediaRenderReceiptDto }
  | { type: 'video_written' }
  | { type: 'updated'; status: MediaSessionStatusDto }
  | { type: 'stopped' }
  | { type: 'error'; error: MediaErrorDto };
export interface MediaDeviceResponseDto { request_id: string; result: MediaDeviceResultDto }

export type MediaSessionDescriptionTypeDto = 'offer' | 'answer';
export interface MediaSessionDescriptionDto { kind: MediaSessionDescriptionTypeDto; sdp: string }
export interface MediaConnectionRequestDto {
  request_id: string;
  connection_id: string;
  session_id: string;
  media_session_ref: MediaSessionRefDto;
  operation_id: MediaOperationIdDto;
  owner: MediaOwnerDto;
  mode: MediaSessionModeDto;
  intent: MediaSessionIntentDto;
  audio_source?: MediaSourceRefDto | null;
  video_source?: MediaSourceRefDto | null;
  description: MediaSessionDescriptionDto;
}
export interface MediaSessionRefDto { id: string }
export interface MediaConnectionResponseDto {
  request_id: string;
  connection_id: string;
  media_session_ref: MediaSessionRefDto;
  description?: MediaSessionDescriptionDto | null;
  error?: MediaErrorDto | null;
}
export type MediaControlActionDto =
  | { type: 'flush_audio'; target_sequence: number; target_sample_frames: number }
  | { type: 'interrupt_audio'; target_sequence: number; target_sample_frames: number }
  | { type: 'close' };
export interface MediaControlRequestDto {
  request_id: string;
  connection_id: string;
  media_session_ref: MediaSessionRefDto;
  generation: number;
  action: MediaControlActionDto;
}
export interface MediaControlResponseDto {
  request_id: string;
  connection_id: string;
  media_session_ref: MediaSessionRefDto;
  generation: number;
  receipt?: MediaRenderReceiptDto | null;
  error?: MediaErrorDto | null;
}

type JsonObject = Record<string, unknown>;

function record(value: unknown, name: string): JsonObject {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${name}`);
  return value as JsonObject;
}

function exact(value: JsonObject, required: readonly string[], optional: readonly string[], name: string): void {
  for (const key of required) if (!(key in value)) throw new Error(`missing ${name}.${key}`);
  for (const key of Object.keys(value)) if (!required.includes(key) && !optional.includes(key)) throw new Error(`unexpected ${name}.${key}`);
}

function identifier(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > 128 || !/^[A-Za-z0-9_-]+$/u.test(value)) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function safeInteger(value: unknown, name: string, maximum = Number.MAX_SAFE_INTEGER): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || value > maximum) throw new Error(`invalid ${name}`);
  return value;
}

function optionalNullable<T>(value: unknown, parse: (value: unknown) => T): T | null | undefined {
  if (value === undefined) return undefined;
  if (value === null) return null;
  return parse(value);
}

function mediaError(value: unknown): MediaErrorDto {
  const error = record(value, 'media error');
  exact(error, ['kind', 'message'], [], 'media error');
  const kinds: readonly string[] = [
    'permission_denied', 'busy', 'cancelled', 'timeout', 'no_data', 'not_found', 'unavailable', 'unsupported',
    'model_missing', 'invalid_request', 'processing_failed', 'native_failure', 'store_failure', 'media_too_large',
  ];
  if (typeof error['kind'] !== 'string' || !kinds.includes(error['kind'])) throw new Error('invalid media error kind');
  if (typeof error['message'] !== 'string' || error['message'].length > 4096) throw new Error('invalid media error message');
  return { kind: error['kind'] as MediaErrorKindDto, message: error['message'] };
}

function mediaOperationId(value: unknown): MediaOperationIdDto {
  const id = record(value, 'media operation id');
  exact(id, ['id', 'generation', 'service_epoch'], [], 'media operation id');
  return {
    id: identifier(id['id'], 'media operation id'),
    generation: safeInteger(id['generation'], 'media operation generation'),
    service_epoch: safeInteger(id['service_epoch'], 'media service epoch'),
  };
}

function mediaContent(value: unknown): MediaContentDto {
  const content = record(value, 'media content');
  exact(content, ['media_ref', 'metadata', 'bytes'], [], 'media content');
  const ref = identifier(content['media_ref'], 'media content reference');
  const metadata = record(content['metadata'], 'media metadata');
  const bytes = content['bytes'];
  if (!Array.isArray(bytes) || bytes.length > MAX_MEDIA_DEVICE_FRAME_BYTES
    || bytes.some((byte) => typeof byte !== 'number' || !Number.isInteger(byte) || byte < 0 || byte > 255)) {
    throw new Error('invalid media content bytes');
  }
  if (metadata['byte_len'] !== bytes.length) throw new Error('media byte count does not match metadata');
  return { media_ref: ref, metadata: metadata as unknown as MediaMetadataDto, bytes: bytes as number[] };
}

function byteArray(value: unknown, name: string): number[] {
  if (!Array.isArray(value) || value.length > MAX_MEDIA_DEVICE_FRAME_BYTES
    || value.some((byte) => typeof byte !== 'number' || !Number.isInteger(byte) || byte < 0 || byte > 255)) {
    throw new Error(`invalid ${name}`);
  }
  return value as number[];
}

function mediaSourceRef(value: unknown): MediaSourceRefDto {
  const source = record(value, 'media source reference');
  exact(source, ['id'], [], 'media source reference');
  return { id: identifier(source['id'], 'media source reference') };
}

function mediaPcmFrame(value: unknown): MediaPcmFrameDto {
  const frame = record(value, 'media PCM frame');
  exact(frame, ['sample_rate_hz', 'channels', 'pcm16le'], [], 'media PCM frame');
  const sample_rate_hz = safeInteger(frame['sample_rate_hz'], 'media sample rate');
  const channels = safeInteger(frame['channels'], 'media channel count');
  const pcm16le = byteArray(frame['pcm16le'], 'media PCM payload');
  if (sample_rate_hz === 0 || channels === 0 || pcm16le.length === 0 || pcm16le.length % 2 !== 0) throw new Error('invalid media PCM format');
  return { sample_rate_hz, channels, pcm16le };
}

function mediaVideoFrame(value: unknown): MediaVideoFrameDto {
  const frame = record(value, 'media video frame');
  if (frame['type'] === 'rgba8') {
    exact(frame, ['type', 'width_px', 'height_px', 'stride_bytes', 'bytes'], [], 'RGBA frame');
    const width_px = safeInteger(frame['width_px'], 'video width');
    const height_px = safeInteger(frame['height_px'], 'video height');
    const stride_bytes = safeInteger(frame['stride_bytes'], 'RGBA stride');
    const bytes = byteArray(frame['bytes'], 'RGBA bytes');
    if (width_px === 0 || height_px === 0 || width_px > 4096 || height_px > 4096
      || stride_bytes < width_px * 4 || stride_bytes * height_px !== bytes.length) throw new Error('invalid RGBA geometry');
    return { type: 'rgba8', width_px, height_px, stride_bytes, bytes };
  }
  if (frame['type'] === 'i420') {
    exact(frame, ['type', 'width_px', 'height_px', 'y_stride_bytes', 'u_stride_bytes', 'v_stride_bytes', 'y', 'u', 'v'], [], 'I420 frame');
    const width_px = safeInteger(frame['width_px'], 'video width');
    const height_px = safeInteger(frame['height_px'], 'video height');
    const y_stride_bytes = safeInteger(frame['y_stride_bytes'], 'I420 Y stride');
    const u_stride_bytes = safeInteger(frame['u_stride_bytes'], 'I420 U stride');
    const v_stride_bytes = safeInteger(frame['v_stride_bytes'], 'I420 V stride');
    const y = byteArray(frame['y'], 'I420 Y plane');
    const u = byteArray(frame['u'], 'I420 U plane');
    const v = byteArray(frame['v'], 'I420 V plane');
    const cw = Math.ceil(width_px / 2), ch = Math.ceil(height_px / 2);
    if (width_px === 0 || height_px === 0 || width_px > 4096 || height_px > 4096
      || y_stride_bytes < width_px || u_stride_bytes < cw || v_stride_bytes < cw
      || y_stride_bytes * height_px !== y.length || u_stride_bytes * ch !== u.length || v_stride_bytes * ch !== v.length
      || y.length + u.length + v.length > MAX_MEDIA_DEVICE_FRAME_BYTES) throw new Error('invalid I420 geometry');
    return { type: 'i420', width_px, height_px, y_stride_bytes, u_stride_bytes, v_stride_bytes, y, u, v };
  }
  throw new Error('invalid media video frame type');
}

function mediaFrame(value: unknown): MediaFrameDto {
  const frame = record(value, 'media frame');
  exact(frame, ['source', 'sequence', 'captured_at_unix_ms', 'payload'], [], 'media frame');
  const payload = record(frame['payload'], 'media frame payload');
  let normalizedPayload: MediaFramePayloadDto;
  if (payload['type'] === 'audio') {
    exact(payload, ['type', 'frame'], [], 'audio media payload');
    normalizedPayload = { type: 'audio', frame: mediaPcmFrame(payload['frame']) };
  } else if (payload['type'] === 'video') {
    exact(payload, ['type', 'frame'], [], 'video media payload');
    normalizedPayload = { type: 'video', frame: mediaVideoFrame(payload['frame']) };
  } else throw new Error('invalid media frame payload kind');
  return {
    source: mediaSourceRef(frame['source']),
    sequence: safeInteger(frame['sequence'], 'media frame sequence'),
    captured_at_unix_ms: safeInteger(frame['captured_at_unix_ms'], 'media capture timestamp'),
    payload: normalizedPayload,
  };
}

function mediaSessionStatus(value: unknown): MediaSessionStatusDto {
  const status = record(value, 'media session status');
  exact(status, ['state', 'audio_enabled', 'video_enabled'], [], 'media session status');
  if (!['preparing', 'ready', 'reconnecting', 'ended', 'failed'].includes(String(status['state']))
    || typeof status['audio_enabled'] !== 'boolean' || typeof status['video_enabled'] !== 'boolean') throw new Error('invalid media session status');
  return { state: status['state'] as MediaSessionStateDto, audio_enabled: status['audio_enabled'], video_enabled: status['video_enabled'] };
}

function mediaSessionRequest(value: unknown): MediaSessionRequestDto {
  const request = record(value, 'media session request');
  exact(request, ['mode', 'intent'], ['audio_source', 'video_source'], 'media session request');
  const mode = request['mode'];
  const intent = request['intent'];
  if (!['audio', 'video', 'audio_video'].includes(String(mode)) || !['observation', 'conversation'].includes(String(intent))) throw new Error('invalid media session request mode');
  return {
    mode: mode as MediaSessionModeDto,
    intent: intent as MediaSessionIntentDto,
    ...(request['audio_source'] === undefined ? {} : { audio_source: optionalNullable(request['audio_source'], mediaSourceRef) }),
    ...(request['video_source'] === undefined ? {} : { video_source: optionalNullable(request['video_source'], mediaSourceRef) }),
  };
}

function mediaDeviceOpenRequest(value: unknown): MediaDeviceOpenRequestDto {
  const request = record(value, 'media device open request');
  exact(request, ['identity', 'owner', 'max_payload_bytes', 'request'], ['initiator', 'timeout_budget_ms'], 'media device open request');
  const identity = mediaOperationId(request['identity']);
  const max_payload_bytes = safeInteger(request['max_payload_bytes'], 'media device payload cap');
  if (max_payload_bytes === 0 || max_payload_bytes > MAX_MEDIA_DEVICE_FRAME_BYTES) throw new Error('invalid media device payload cap');
  const owner = record(request['owner'], 'media owner');
  if (owner['type'] === 'session') { exact(owner, ['type', 'session_id'], [], 'media owner'); identifier(owner['session_id'], 'media owner session'); }
  else if (owner['type'] === 'ui' || owner['type'] === 'system') { exact(owner, ['type', 'instance_id'], [], 'media owner'); identifier(owner['instance_id'], 'media owner instance'); }
  else throw new Error('invalid media owner');
  const initiator = request['initiator'] === undefined || request['initiator'] === null
    ? request['initiator']
    : record(request['initiator'], 'media initiator') as unknown as MediaInitiatorDto;
  return {
    identity,
    owner: owner as unknown as MediaOwnerDto,
    ...(initiator === undefined ? {} : { initiator }),
    ...(request['timeout_budget_ms'] === undefined ? {} : { timeout_budget_ms: optionalNullable(request['timeout_budget_ms'], (value) => safeInteger(value, 'media timeout')) }),
    max_payload_bytes,
    request: mediaSessionRequest(request['request']),
  };
}

/** Validate a bounded native callback envelope before the Electron adapter sees it. */
export function validateMediaDeviceRequestEnvelope(value: unknown): MediaDeviceRequestEnvelopeDto {
  const envelope = record(value, 'media device request envelope');
  exact(envelope, ['request_id', 'request'], [], 'media device request envelope');
  const request = record(envelope['request'], 'media device request');
  const type = request['type'];
  if (type === 'open') {
    exact(request, ['type', 'request'], [], 'media device open');
    mediaDeviceOpenRequest(request['request']);
  } else if (type === 'next_input' || type === 'stop') {
    exact(request, ['type', 'lease_id'], [], 'media device lease request');
    identifier(request['lease_id'], 'media device lease id');
  } else if (type === 'write_audio') {
    exact(request, ['type', 'request'], [], 'media device audio write');
    const write = record(request['request'], 'media device audio write request');
    exact(write, ['lease_id', 'frame'], [], 'media device audio write request');
    identifier(write['lease_id'], 'media device lease id');
    mediaPcmFrame(write['frame']);
  } else if (type === 'write_video') {
    exact(request, ['type', 'request'], [], 'media device video write');
    const write = record(request['request'], 'media device video write request');
    exact(write, ['lease_id', 'frame'], [], 'media device video write request');
    identifier(write['lease_id'], 'media device lease id');
    mediaVideoFrame(write['frame']);
  } else if (type === 'flush_audio' || type === 'interrupt_audio') {
    exact(request, ['type', 'request'], [], 'media device audio barrier');
    const barrier = record(request['request'], 'media device audio barrier request');
    exact(barrier, ['lease_id'], [], 'media device audio barrier request');
    identifier(barrier['lease_id'], 'media device lease id');
  } else if (type === 'update') {
    exact(request, ['type', 'request'], [], 'media device update');
    const update = record(request['request'], 'media device update request');
    exact(update, ['lease_id', 'update'], [], 'media device update request');
    identifier(update['lease_id'], 'media device lease id');
    record(update['update'], 'media device update');
  } else throw new Error('invalid media device request type');
  return { request_id: identifier(envelope['request_id'], 'media device request id'), request: request as unknown as MediaDeviceRequestDto };
}

/** Validate one correlated native callback result before its caller consumes it. */
export function validateMediaDeviceResponse(value: unknown): MediaDeviceResponseDto {
  const response = record(value, 'media device response');
  exact(response, ['request_id', 'result'], [], 'media device response');
  const result = record(response['result'], 'media device result');
  let normalized: MediaDeviceResultDto;
  switch (result['type']) {
    case 'opened': {
      exact(result, ['type', 'lease'], [], 'opened media device result');
      const lease = record(result['lease'], 'media device lease');
      exact(lease, ['lease_id'], [], 'media device lease');
      normalized = { type: 'opened', lease: { lease_id: identifier(lease['lease_id'], 'media device lease id') } };
      break;
    }
    case 'input':
      exact(result, ['type', 'frame'], [], 'media input result');
      normalized = { type: 'input', frame: result['frame'] === null ? null : mediaFrame(result['frame']) };
      break;
    case 'audio_written':
    case 'audio_flushed':
    case 'audio_interrupted':
      exact(result, ['type', 'receipt'], [], 'media audio result');
      {
        const kind = result['type'] as 'audio_written' | 'audio_flushed' | 'audio_interrupted';
        const receipt = mediaRenderReceipt(result['receipt']);
        if (kind !== 'audio_written' && receipt.accepted_sample_frames !== 0) {
        throw new Error('media audio barrier cannot accept new sample frames');
        }
        normalized = { type: kind, receipt };
      }
      break;
    case 'video_written':
    case 'stopped':
      exact(result, ['type'], [], 'media device acknowledgement');
      normalized = { type: result['type'] };
      break;
    case 'updated':
      exact(result, ['type', 'status'], [], 'media device update result');
      normalized = { type: 'updated', status: mediaSessionStatus(result['status']) };
      break;
    case 'error':
      exact(result, ['type', 'error'], [], 'media device error result');
      normalized = { type: 'error', error: mediaError(result['error']) };
      break;
    default:
      throw new Error('invalid media device result type');
  }
  return { request_id: identifier(response['request_id'], 'media device response request id'), result: normalized };
}

/** Validate the host-private correlated response before resolving a proxy promise. */
export function validateMediaServiceResponse(value: unknown): MediaServiceResponseDto {
  const response = record(value, 'media service response');
  exact(response, ['request_id'], ['result', 'error'], 'media service response');
  const request_id = identifier(response['request_id'], 'media service request id');
  const hasResult = response['result'] !== undefined && response['result'] !== null;
  const hasError = response['error'] !== undefined && response['error'] !== null;
  if (hasResult === hasError) throw new Error('media response must contain exactly one result or error');
  const normalized: MediaServiceResponseDto = { request_id };
  if (response['result'] !== undefined) {
    normalized.result = response['result'] === null ? null : mediaServiceResult(response['result']);
  }
  if (response['error'] !== undefined) {
    normalized.error = response['error'] === null ? null : mediaError(response['error']);
  }
  return normalized;
}

function mediaServiceResult(value: unknown): MediaServiceResultDto {
  const result = record(value, 'media service result');
  if (result['type'] === 'cancelled' || result['type'] === 'settings_updated' || result['type'] === 'connection_updated') {
    exact(result, ['type'], [], 'media service acknowledgement');
    return { type: result['type'] };
  }
  if (result['type'] === 'content') {
    exact(result, ['type', 'content'], [], 'media content result');
    return { type: 'content', content: mediaContent(result['content']) };
  }
  if (result['type'] === 'operation') {
    exact(result, ['type', 'operation_id', 'result'], [], 'media operation result');
    const operationId = mediaOperationId(result['operation_id']);
    const operationResult = record(result['result'], 'media operation result value');
    const operationTypes: readonly string[] = [
      'sources', 'status', 'captured', 'recording_started', 'recording_stopped', 'transcript', 'synthesized',
      'playback_completed', 'speech_completed', 'session_started', 'session_updated', 'events', 'stopped',
    ];
    if (typeof operationResult['type'] !== 'string' || !operationTypes.includes(operationResult['type'])) throw new Error('invalid media operation result type');
    return { type: 'operation', operation_id: operationId, result: operationResult as unknown as MediaResultDto };
  }
  throw new Error('invalid media service result type');
}

function mediaOperation(value: unknown): MediaOperationDto {
  const operation = record(value, 'media operation');
  const type = operation['type'];
  if (typeof type !== 'string') throw new Error('invalid media operation type');
  switch (type) {
    case 'sources': {
      exact(operation, ['type'], ['kind', 'usage'], 'sources operation');
      if (operation['kind'] !== undefined && operation['kind'] !== null
        && !['audio', 'image', 'video'].includes(String(operation['kind']))) throw new Error('invalid media source kind');
      if (operation['usage'] !== undefined && operation['usage'] !== null
        && !['capture', 'record', 'transcribe', 'play', 'speak', 'session', 'read_image'].includes(String(operation['usage']))) throw new Error('invalid media source usage');
      return operation as unknown as MediaOperationDto;
    }
    case 'status':
      exact(operation, ['type'], ['target'], 'status operation');
      return operation as unknown as MediaOperationDto;
    case 'capture':
      exact(operation, ['type', 'source', 'options'], [], 'capture operation');
      mediaSourceRef(operation['source']);
      record(operation['options'], 'capture options');
      return operation as unknown as MediaOperationDto;
    case 'record':
      exact(operation, ['type', 'source', 'options'], [], 'record operation');
      mediaSourceRef(operation['source']);
      record(operation['options'], 'record options');
      return operation as unknown as MediaOperationDto;
    case 'transcribe':
      exact(operation, ['type', 'input'], ['language'], 'transcribe operation');
      record(operation['input'], 'transcription input');
      return operation as unknown as MediaOperationDto;
    case 'synthesize':
    case 'speak':
      exact(operation, ['type', 'text'], ['language', 'rate', 'voice'], `${type} operation`);
      if (typeof operation['text'] !== 'string' || operation['text'].length > 64 * 1024) throw new Error('invalid media text');
      return operation as unknown as MediaOperationDto;
    case 'play':
      exact(operation, ['type', 'media_ref'], ['source'], 'play operation');
      identifier(operation['media_ref'], 'media reference');
      if (operation['source'] !== undefined && operation['source'] !== null) mediaSourceRef(operation['source']);
      return operation as unknown as MediaOperationDto;
    case 'session_start':
      exact(operation, ['type', 'request'], [], 'session start operation');
      mediaSessionRequest(operation['request']);
      return operation as unknown as MediaOperationDto;
    case 'session_update':
      exact(operation, ['type', 'session_id', 'update'], [], 'session update operation');
      identifier(operation['session_id'], 'media session reference');
      record(operation['update'], 'media session update');
      return operation as unknown as MediaOperationDto;
    case 'observe':
      exact(operation, ['type', 'target', 'limit'], ['after_sequence'], 'observe operation');
      safeInteger(operation['limit'], 'media event limit');
      return operation as unknown as MediaOperationDto;
    case 'wait':
      exact(operation, ['type', 'target', 'timeout_ms', 'limit'], ['after_sequence'], 'wait operation');
      safeInteger(operation['timeout_ms'], 'media wait timeout');
      safeInteger(operation['limit'], 'media event limit');
      return operation as unknown as MediaOperationDto;
    case 'stop':
      exact(operation, ['type', 'target'], [], 'stop operation');
      return operation as unknown as MediaOperationDto;
    default:
      throw new Error('invalid media operation type');
  }
}

/** Validate an Electron→BridgeServer host-only media service request. */
export function validateMediaServiceRequest(value: unknown): MediaServiceRequestDto {
  const request = record(value, 'media service request');
  exact(request, ['request_id', 'action'], [], 'media service request');
  const request_id = identifier(request['request_id'], 'media service request id');
  const action = record(request['action'], 'media service action');
  const type = action['type'];
  if (type === 'execute') {
    exact(action, ['type', 'operation'], ['timeout_budget_ms', 'max_payload_bytes'], 'media execute action');
    const timeout_budget_ms = optionalNullable(action['timeout_budget_ms'], (field) => safeInteger(field, 'media timeout'));
    const max_payload_bytes = optionalNullable(action['max_payload_bytes'], (field) => safeInteger(field, 'media payload cap', MAX_MEDIA_DEVICE_FRAME_BYTES));
    return { request_id, action: {
      type,
      operation: mediaOperation(action['operation']),
      ...(action['timeout_budget_ms'] === undefined ? {} : { timeout_budget_ms }),
      ...(action['max_payload_bytes'] === undefined ? {} : { max_payload_bytes }),
    } };
  }
  if (type === 'read_media') {
    exact(action, ['type', 'media_ref'], [], 'media read action');
    return { request_id, action: { type, media_ref: identifier(action['media_ref'], 'media reference') } };
  }
  if (type === 'cancel') {
    exact(action, ['type', 'operation_id'], [], 'media cancel action');
    return { request_id, action: { type, operation_id: mediaOperationId(action['operation_id']) } };
  }
  if (type === 'update_audio_settings') {
    exact(action, ['type', 'configuration_json'], [], 'audio settings action');
    const configuration_json = action['configuration_json'];
    if (typeof configuration_json !== 'string' || new TextEncoder().encode(configuration_json).length > MAX_MEDIA_AUDIO_SETTINGS_JSON_BYTES) throw new Error('invalid audio settings JSON size');
    JSON.parse(configuration_json);
    return { request_id, action: { type, configuration_json } };
  }
  if (type === 'register_connection') {
    exact(action, ['type', 'connection_id', 'label', 'supported_modes', 'source_kinds'], [], 'media route registration');
    const connection_id = identifier(action['connection_id'], 'media connection id');
    if (typeof action['label'] !== 'string' || action['label'].trim().length === 0 || action['label'].length > 256) throw new Error('invalid media connection label');
    if (!Array.isArray(action['supported_modes']) || action['supported_modes'].length === 0 || action['supported_modes'].length > 3) throw new Error('invalid media connection modes');
    const modes = action['supported_modes'].map((mode) => {
      if (!['audio', 'video', 'audio_video'].includes(String(mode))) throw new Error('invalid media connection mode');
      return mode as MediaSessionModeDto;
    });
    if (new Set(modes).size !== modes.length) throw new Error('duplicate media connection mode');
    if (!Array.isArray(action['source_kinds']) || action['source_kinds'].length === 0 || action['source_kinds'].length > 3) throw new Error('invalid media route sources');
    const source_kinds = action['source_kinds'].map((source) => {
      if (source !== 'microphone' && source !== 'camera' && source !== 'screen') throw new Error('invalid media route source kind');
      return source;
    });
    if (new Set(source_kinds).size !== source_kinds.length) throw new Error('duplicate media route source kind');
    const audio = source_kinds.includes('microphone');
    const video = source_kinds.includes('camera') || source_kinds.includes('screen');
    const audioMode = modes.some((mode) => mode === 'audio' || mode === 'audio_video');
    const videoMode = modes.some((mode) => mode === 'video' || mode === 'audio_video');
    if (audioMode !== audio || videoMode !== video) throw new Error('media route modes and source inventory disagree');
    return { request_id, action: { type, connection_id, label: action['label'], supported_modes: modes, source_kinds } };
  }
  if (type === 'unregister_connection') {
    exact(action, ['type', 'connection_id'], [], 'media route unregistration');
    return { request_id, action: { type, connection_id: identifier(action['connection_id'], 'media connection id') } };
  }
  throw new Error('invalid media service action');
}

/** Validate bounded host-private connection negotiation requests. */
export function validateMediaConnectionRequest(value: unknown): MediaConnectionRequestDto {
  const request = record(value, 'media connection request');
  exact(request, ['request_id', 'connection_id', 'session_id', 'media_session_ref', 'operation_id', 'owner', 'mode', 'intent', 'description'], ['audio_source', 'video_source'], 'media connection request');
  const description = record(request['description'], 'media session description');
  exact(description, ['kind', 'sdp'], [], 'media session description');
  if (description['kind'] !== 'offer' || typeof description['sdp'] !== 'string'
    || description['sdp'].length === 0 || description['sdp'].length > MAX_REMOTE_MEDIA_SDP_BYTES) throw new Error('invalid media offer description');
  const ref = record(request['media_session_ref'], 'media session reference');
  exact(ref, ['id'], [], 'media session reference');
  const mode = request['mode'];
  const intent = request['intent'];
  if (!['audio', 'video', 'audio_video'].includes(String(mode)) || !['observation', 'conversation'].includes(String(intent))) throw new Error('invalid media mode or intent');
  const owner = record(request['owner'], 'media owner');
  if (owner['type'] === 'session') { exact(owner, ['type', 'session_id'], [], 'media owner'); identifier(owner['session_id'], 'media owner session'); }
  else if (owner['type'] === 'ui' || owner['type'] === 'system') { exact(owner, ['type', 'instance_id'], [], 'media owner'); identifier(owner['instance_id'], 'media owner instance'); }
  else throw new Error('invalid media owner');
  return {
    request_id: identifier(request['request_id'], 'media connection request id'),
    connection_id: identifier(request['connection_id'], 'media connection id'),
    session_id: identifier(request['session_id'], 'media session id'),
    media_session_ref: { id: identifier(ref['id'], 'media session reference') },
    operation_id: mediaOperationId(request['operation_id']),
    owner: owner as unknown as MediaOwnerDto,
    mode: mode as MediaSessionModeDto,
    intent: intent as MediaSessionIntentDto,
    ...(request['audio_source'] === undefined ? {} : { audio_source: optionalNullable(request['audio_source'], mediaSourceRef) }),
    ...(request['video_source'] === undefined ? {} : { video_source: optionalNullable(request['video_source'], mediaSourceRef) }),
    description: { kind: 'offer', sdp: description['sdp'] },
  };
}

function mediaSessionDescription(value: unknown, expectedKind: MediaSessionDescriptionTypeDto): MediaSessionDescriptionDto {
  const description = record(value, 'media session description');
  exact(description, ['kind', 'sdp'], [], 'media session description');
  if (description['kind'] !== expectedKind || typeof description['sdp'] !== 'string'
    || description['sdp'].length === 0 || description['sdp'].length > MAX_REMOTE_MEDIA_SDP_BYTES) throw new Error('invalid media session description');
  return { kind: expectedKind, sdp: description['sdp'] };
}

/** Validate the exact correlated host answer or safe connection error. */
export function validateMediaConnectionResponse(value: unknown): MediaConnectionResponseDto {
  const response = record(value, 'media connection response');
  exact(response, ['request_id', 'connection_id', 'media_session_ref'], ['description', 'error'], 'media connection response');
  const reference = record(response['media_session_ref'], 'media session reference');
  exact(reference, ['id'], [], 'media session reference');
  const description = optionalNullable(response['description'], (field) => mediaSessionDescription(field, 'answer'));
  const error = optionalNullable(response['error'], mediaError);
  if ((description !== undefined && description !== null) === (error !== undefined && error !== null)) {
    throw new Error('media connection response must contain exactly one answer or error');
  }
  return {
    request_id: identifier(response['request_id'], 'media connection request id'),
    connection_id: identifier(response['connection_id'], 'media connection id'),
    media_session_ref: { id: identifier(reference['id'], 'media session reference') },
    ...(response['description'] === undefined ? {} : { description }),
    ...(response['error'] === undefined ? {} : { error }),
  };
}

function mediaRenderReceipt(value: unknown): MediaRenderReceiptDto {
  const receipt = record(value, 'media render receipt');
  exact(receipt, ['accepted_sample_frames', 'played_sample_frames'], ['rendered_at_unix_ms'], 'media render receipt');
  return {
    accepted_sample_frames: safeInteger(receipt['accepted_sample_frames'], 'accepted media sample frames', 0xffff_ffff),
    played_sample_frames: safeInteger(receipt['played_sample_frames'], 'played media sample frames', 0xffff_ffff),
    ...(receipt['rendered_at_unix_ms'] === undefined ? {} : {
      rendered_at_unix_ms: receipt['rendered_at_unix_ms'] === null
        ? null
        : safeInteger(receipt['rendered_at_unix_ms'], 'media render timestamp'),
    }),
  };
}

/** Validate the exact correlated peer control result before resolving a barrier. */
export function validateMediaControlResponse(value: unknown): MediaControlResponseDto {
  const response = record(value, 'media control response');
  exact(response, ['request_id', 'connection_id', 'media_session_ref', 'generation'], ['receipt', 'error'], 'media control response');
  const reference = record(response['media_session_ref'], 'media session reference');
  exact(reference, ['id'], [], 'media session reference');
  const receipt = optionalNullable(response['receipt'], mediaRenderReceipt);
  const error = optionalNullable(response['error'], mediaError);
  if ((receipt !== undefined && receipt !== null) === (error !== undefined && error !== null)) {
    throw new Error('media control response must contain exactly one receipt or error');
  }
  if (receipt !== undefined && receipt !== null && receipt.accepted_sample_frames !== 0) {
    throw new Error('media barrier receipt cannot accept additional sample frames');
  }
  return {
    request_id: identifier(response['request_id'], 'media control request id'),
    connection_id: identifier(response['connection_id'], 'media connection id'),
    media_session_ref: { id: identifier(reference['id'], 'media session reference') },
    generation: safeInteger(response['generation'], 'media control generation'),
    ...(response['receipt'] === undefined ? {} : { receipt }),
    ...(response['error'] === undefined ? {} : { error }),
  };
}

/** Validate a host-private playback barrier before sending it to the route registry. */
export function validateMediaControlRequest(value: unknown): MediaControlRequestDto {
  const request = record(value, 'media control request');
  exact(request, ['request_id', 'connection_id', 'media_session_ref', 'generation', 'action'], [], 'media control request');
  const ref = record(request['media_session_ref'], 'media session reference');
  exact(ref, ['id'], [], 'media session reference');
  const action = record(request['action'], 'media control action');
  if (action['type'] === 'close') {
    exact(action, ['type'], [], 'media control action');
    return {
      request_id: identifier(request['request_id'], 'media control request id'),
      connection_id: identifier(request['connection_id'], 'media connection id'),
      media_session_ref: { id: identifier(ref['id'], 'media session reference') },
      generation: safeInteger(request['generation'], 'media control generation'),
      action: { type: 'close' },
    };
  }
  if (action['type'] !== 'flush_audio' && action['type'] !== 'interrupt_audio') throw new Error('invalid media control action');
  exact(action, ['type', 'target_sequence', 'target_sample_frames'], [], 'media control action');
  const targetSequence = safeInteger(action['target_sequence'], 'media target sequence');
  const targetSampleFrames = safeInteger(action['target_sample_frames'], 'media target sample frames');
  if (targetSequence > 180_000 || targetSampleFrames > 172_800_000
    || ((targetSequence === 0) !== (targetSampleFrames === 0))) throw new Error('media control target exceeds its bounds');
  return {
    request_id: identifier(request['request_id'], 'media control request id'),
    connection_id: identifier(request['connection_id'], 'media connection id'),
    media_session_ref: { id: identifier(ref['id'], 'media session reference') },
    generation: safeInteger(request['generation'], 'media control generation'),
    action: {
      type: action['type'],
      target_sequence: targetSequence,
      target_sample_frames: targetSampleFrames,
    },
  };
}
