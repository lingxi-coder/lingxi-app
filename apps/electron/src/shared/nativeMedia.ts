import {
  MAX_MEDIA_DEVICE_FRAME_BYTES,
  MAX_MEDIA_DEVICE_WIRE_BYTES,
  type MediaDeviceRequestEnvelopeDto,
  type MediaDeviceResponseDto,
  type MediaOwnerDto,
} from '@lingxi/bridge-client';

type ObjectValue = Record<string, unknown>;
const U32_MAX = 0xffff_ffff;
const ERROR_KINDS = new Set([
  'permission_denied', 'busy', 'cancelled', 'timeout', 'no_data', 'not_found',
  'unavailable', 'unsupported', 'model_missing', 'invalid_request',
  'processing_failed', 'native_failure', 'store_failure', 'media_too_large',
]);

function object(value: unknown, keys: readonly string[]): ObjectValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || Object.keys(value).some((key) => !keys.includes(key))) throw new Error('Invalid native media object');
  return value as ObjectValue;
}

function integer(value: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < min || (value as number) > max) {
    throw new Error('Invalid native media integer');
  }
}

function identifier(value: unknown): asserts value is string {
  if (typeof value !== 'string' || value.length === 0 || value.includes('\0')
    || Buffer.byteLength(value, 'utf8') > 256) throw new Error('Invalid native media reference');
}

function boolean(value: unknown): asserts value is boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid native media flag');
}

function oneOf(value: unknown, values: readonly string[]): void {
  if (typeof value !== 'string' || !values.includes(value)) throw new Error('Invalid native media discriminator');
}

function source(value: unknown): void {
  const input = object(value, ['id']);
  identifier(input.id);
}

function identity(value: unknown): void {
  const input = object(value, ['id', 'generation', 'service_epoch']);
  identifier(input.id);
  integer(input.generation);
  integer(input.service_epoch, 1);
}

export function nativeMediaOwnerKey(value: unknown): string {
  const input = object(value, ['type', 'session_id', 'instance_id']);
  oneOf(input.type, ['session', 'ui', 'system']);
  const id = input.type === 'session' ? input.session_id : input.instance_id;
  identifier(id);
  if (input.type === 'session' ? input.instance_id != null : input.session_id != null) {
    throw new Error('Invalid native media owner');
  }
  return `${input.type}:${id}`;
}

function bytes(value: unknown): number {
  if (!Array.isArray(value) || value.length === 0 || value.length > MAX_MEDIA_DEVICE_FRAME_BYTES) {
    throw new Error('Native media frame exceeds its byte limit');
  }
  for (const byte of value) integer(byte, 0, 255);
  return value.length;
}

function pcm(value: unknown): number {
  const input = object(value, ['sample_rate_hz', 'channels', 'pcm16le']);
  integer(input.sample_rate_hz, 8_000, 768_000);
  integer(input.channels, 1, 8);
  const size = bytes(input.pcm16le);
  if (size % (2 * input.channels) !== 0) throw new Error('Native PCM contains a partial sample frame');
  return size / (2 * input.channels);
}

function video(value: unknown): void {
  const input = object(value, [
    'type', 'width_px', 'height_px', 'stride_bytes', 'bytes',
    'y_stride_bytes', 'u_stride_bytes', 'v_stride_bytes', 'y', 'u', 'v',
  ]);
  integer(input.width_px, 1, 16_384);
  integer(input.height_px, 1, 16_384);
  if (input.type === 'rgba8') {
    object(value, ['type', 'width_px', 'height_px', 'stride_bytes', 'bytes']);
    integer(input.stride_bytes, input.width_px * 4, MAX_MEDIA_DEVICE_FRAME_BYTES);
    if (bytes(input.bytes) !== input.stride_bytes * input.height_px) throw new Error('Invalid native RGBA layout');
  } else if (input.type === 'i420') {
    object(value, ['type', 'width_px', 'height_px', 'y_stride_bytes', 'u_stride_bytes', 'v_stride_bytes', 'y', 'u', 'v']);
    integer(input.y_stride_bytes, input.width_px, MAX_MEDIA_DEVICE_FRAME_BYTES);
    integer(input.u_stride_bytes, Math.ceil(input.width_px / 2), MAX_MEDIA_DEVICE_FRAME_BYTES);
    integer(input.v_stride_bytes, Math.ceil(input.width_px / 2), MAX_MEDIA_DEVICE_FRAME_BYTES);
    const y = bytes(input.y); const u = bytes(input.u); const v = bytes(input.v);
    const rows = Math.ceil(input.height_px / 2);
    if (y !== input.y_stride_bytes * input.height_px || u !== input.u_stride_bytes * rows
      || v !== input.v_stride_bytes * rows || y + u + v > MAX_MEDIA_DEVICE_FRAME_BYTES) {
      throw new Error('Invalid native I420 layout');
    }
  } else throw new Error('Invalid native video format');
}

function update(value: unknown): void {
  const input = object(value, ['type', 'kind', 'enabled', 'source', 'gain']);
  switch (input.type) {
    case 'set_enabled':
      object(value, ['type', 'kind', 'enabled']);
      oneOf(input.kind, ['audio', 'image', 'video']); boolean(input.enabled); break;
    case 'select_source':
      object(value, ['type', 'kind', 'source']);
      oneOf(input.kind, ['audio', 'image', 'video']); source(input.source); break;
    case 'set_playback_gain':
      object(value, ['type', 'gain']);
      if (typeof input.gain !== 'number' || !Number.isFinite(input.gain) || input.gain < 0 || input.gain > 1) {
        throw new Error('Invalid native playback gain');
      }
      break;
    default: throw new Error('Invalid native media update');
  }
}

function wire(value: unknown): void {
  if (Buffer.byteLength(JSON.stringify(value), 'utf8') > MAX_MEDIA_DEVICE_WIRE_BYTES - 1024) {
    throw new Error('Native media envelope exceeds its wire limit');
  }
}

/** Validate host-private device traffic before it reaches the signed helper. */
export function validateMediaDeviceRequest(value: unknown): MediaDeviceRequestEnvelopeDto {
  const envelope = object(value, ['request_id', 'request']);
  identifier(envelope.request_id);
  const request = object(envelope.request, ['type', 'request', 'lease_id']);
  switch (request.type) {
    case 'open': {
      object(request, ['type', 'request']);
      const open = object(request.request, ['identity', 'owner', 'initiator', 'timeout_budget_ms', 'max_payload_bytes', 'request']);
      identity(open.identity); nativeMediaOwnerKey(open.owner);
      if (open.initiator != null) {
        const initiator = object(open.initiator, ['agent_id', 'tool_use_id', 'request_id']);
        for (const id of Object.values(initiator)) if (id != null) identifier(id);
      }
      if (open.timeout_budget_ms != null) integer(open.timeout_budget_ms, 1, 3_600_000);
      integer(open.max_payload_bytes, 1, MAX_MEDIA_DEVICE_FRAME_BYTES);
      const session = object(open.request, ['mode', 'intent', 'audio_source', 'video_source']);
      oneOf(session.mode, ['audio', 'video', 'audio_video']);
      oneOf(session.intent, ['observation', 'conversation']);
      if (session.audio_source != null) source(session.audio_source);
      if (session.video_source != null) source(session.video_source);
      if ((session.mode === 'audio' && session.video_source != null)
        || (session.mode === 'video' && session.audio_source != null)) throw new Error('Media source contradicts session mode');
      break;
    }
    case 'next_input': case 'stop':
      object(request, ['type', 'lease_id']); identifier(request.lease_id); break;
    case 'write_audio': case 'write_video': {
      object(request, ['type', 'request']);
      const write = object(request.request, ['lease_id', 'frame']); identifier(write.lease_id);
      if (request.type === 'write_audio') pcm(write.frame); else video(write.frame);
      break;
    }
    case 'flush_audio': case 'interrupt_audio': {
      object(request, ['type', 'request']);
      const control = object(request.request, ['lease_id']); identifier(control.lease_id); break;
    }
    case 'update': {
      object(request, ['type', 'request']);
      const change = object(request.request, ['lease_id', 'update']);
      identifier(change.lease_id); update(change.update); break;
    }
    default: throw new Error('Invalid native media operation');
  }
  wire(value);
  return value as MediaDeviceRequestEnvelopeDto;
}

/** Validate callback-confirmed data, including byte and playback-counter bounds. */
export function validateMediaDeviceResponse(value: unknown): MediaDeviceResponseDto {
  const envelope = object(value, ['request_id', 'result']); identifier(envelope.request_id);
  const result = object(envelope.result, ['type', 'lease', 'frame', 'receipt', 'status', 'error']);
  switch (result.type) {
    case 'opened': {
      object(result, ['type', 'lease']);
      const lease = object(result.lease, ['lease_id']); identifier(lease.lease_id); break;
    }
    case 'input': {
      object(result, ['type', 'frame']);
      if (result.frame === null) break;
      const frame = object(result.frame, ['source', 'sequence', 'captured_at_unix_ms', 'payload']);
      source(frame.source); integer(frame.sequence); integer(frame.captured_at_unix_ms, 1);
      const payload = object(frame.payload, ['type', 'frame']);
      if (payload.type === 'audio') pcm(payload.frame);
      else if (payload.type === 'video') video(payload.frame);
      else throw new Error('Invalid native captured frame kind');
      break;
    }
    case 'audio_written': case 'audio_flushed': case 'audio_interrupted': {
      object(result, ['type', 'receipt']);
      const receipt = object(result.receipt, ['accepted_sample_frames', 'played_sample_frames', 'rendered_at_unix_ms']);
      integer(receipt.accepted_sample_frames, 0, U32_MAX); integer(receipt.played_sample_frames, 0, U32_MAX);
      if (result.type !== 'audio_written' && receipt.accepted_sample_frames !== 0) {
        throw new Error('Native audio barrier cannot accept sample frames');
      }
      if (receipt.rendered_at_unix_ms != null) integer(receipt.rendered_at_unix_ms, 1);
      break;
    }
    case 'video_written': case 'stopped': object(result, ['type']); break;
    case 'updated': {
      object(result, ['type', 'status']);
      const status = object(result.status, ['state', 'audio_enabled', 'video_enabled']);
      oneOf(status.state, ['preparing', 'ready', 'reconnecting', 'ended', 'failed']);
      boolean(status.audio_enabled); boolean(status.video_enabled); break;
    }
    case 'error': {
      object(result, ['type', 'error']);
      const error = object(result.error, ['kind', 'message']);
      if (typeof error.kind !== 'string' || !ERROR_KINDS.has(error.kind)
        || typeof error.message !== 'string' || Buffer.byteLength(error.message, 'utf8') > 1024) {
        throw new Error('Invalid native media failure');
      }
      break;
    }
    default: throw new Error('Invalid native media response');
  }
  wire(value);
  return value as MediaDeviceResponseDto;
}

export function sameMediaOwner(left: MediaOwnerDto, right: MediaOwnerDto): boolean {
  return nativeMediaOwnerKey(left) === nativeMediaOwnerKey(right);
}
