import { createHash, randomUUID } from 'node:crypto';
import type {
  MediaDeviceRequestEnvelopeDto,
  MediaDeviceResponseDto,
  MediaOwnerDto,
} from '@lingxi/bridge-client';
import {
  nativeMediaOwnerKey, validateMediaDeviceRequest, validateMediaDeviceResponse,
} from '../../shared/nativeMedia.js';

interface NativeMediaDeviceHost {
  executeMediaDevice(request: MediaDeviceRequestEnvelopeDto): Promise<MediaDeviceResponseDto>;
}

interface PendingRequest {
  fingerprint: string;
  promise: Promise<MediaDeviceResponseDto>;
}

const MAX_PENDING_REQUESTS = 32;
const MAX_REMEMBERED_REQUESTS = 256;

function failure(requestId: string, kind: 'invalid_request' | 'not_found' | 'cancelled' | 'busy' | 'native_failure', message: string): MediaDeviceResponseDto {
  return { request_id: requestId, result: { type: 'error', error: { kind, message } } };
}

function rejectedOpen(requestId: string, kind: 'not_found' | 'busy' | 'cancelled', message: string): MediaDeviceResponseDto {
  return { request_id: requestId, result: { type: 'open_failed_without_resource', error: { kind, message } } };
}

function leaseId(request: MediaDeviceRequestEnvelopeDto): string | undefined {
  switch (request.request.type) {
    case 'open': return undefined;
    case 'next_input': case 'stop': return request.request.lease_id;
    default: return request.request.request.lease_id;
  }
}

/** One authenticated bridge's view of the app's native media device service. */
export class NativeMediaDevices {
  private readonly leases = new Set<string>();
  // A bounded response history must not forget an open whose native resource
  // is still live or whose outcome is unknown.
  private readonly openReservations = new Set<string>();
  private readonly leaseOpenRequests = new Map<string, { requestId: string; onClose?: () => void }>();
  private readonly quarantined = new Set<string>();
  private readonly pending = new Map<string, PendingRequest>();
  private readonly completed = new Set<string>();
  private readonly reading = new Set<string>();
  private readonly sourceSequences = new Map<string, number>();
  private readonly ownerKey: string;
  private closed = false;
  private disposing?: Promise<void>;

  constructor(private readonly options: {
    audio: NativeMediaDeviceHost;
    owner: MediaOwnerDto;
  }) {
    // The caller supplies an already trusted binding. Requests never install it.
    this.ownerKey = nativeMediaOwnerKey(options.owner);
  }

  request(value: unknown, onLeaseClosed?: () => void): Promise<MediaDeviceResponseDto> {
    let request: MediaDeviceRequestEnvelopeDto;
    try { request = validateMediaDeviceRequest(value); }
    catch {
      const requestId = value && typeof value === 'object' && 'request_id' in value
        && typeof value.request_id === 'string' && value.request_id.length <= 256 ? value.request_id : 'invalid';
      return Promise.resolve(failure(requestId, 'invalid_request', 'Invalid native media request'));
    }
    if (this.closed) return Promise.resolve(failure(request.request_id, 'cancelled', 'The native media connection is closed'));
    const fingerprint = createHash('sha256').update(JSON.stringify(request)).digest('hex');
    const existing = this.pending.get(request.request_id);
    if (existing) return existing.fingerprint === fingerprint ? existing.promise
      : Promise.resolve(failure(request.request_id, 'invalid_request', 'The media request identity was reused'));
    if (this.completed.has(request.request_id) || this.openReservations.has(request.request_id)) {
      return Promise.resolve(failure(request.request_id, 'invalid_request', 'The media request already completed'));
    }
    if (this.pending.size >= MAX_PENDING_REQUESTS) return Promise.resolve(request.request.type === 'open'
      ? rejectedOpen(request.request_id, 'busy', 'The native media request queue is full')
      : failure(request.request_id, 'busy', 'The native media request queue is full'));
    if (request.request.type === 'open' && nativeMediaOwnerKey(request.request.request.owner) !== this.ownerKey) {
      return Promise.resolve(rejectedOpen(request.request_id, 'not_found', 'The native media owner is not attached'));
    }
    const lease = leaseId(request);
    if (lease && !this.leases.has(lease)) return Promise.resolve(failure(request.request_id, 'not_found', 'The native media lease was not found'));
    if (lease && this.quarantined.has(lease) && request.request.type !== 'stop') {
      return Promise.resolve(failure(request.request_id, 'native_failure', 'The native media lease is awaiting cleanup'));
    }
    if (request.request.type === 'next_input' && lease && this.reading.has(lease)) {
      return Promise.resolve(failure(request.request_id, 'busy', 'A native media reader is already waiting'));
    }
    if (request.request.type === 'next_input' && lease) this.reading.add(lease);
    if (request.request.type === 'open') this.openReservations.add(request.request_id);
    const promise = this.dispatch(request, onLeaseClosed).finally(() => {
      this.pending.delete(request.request_id);
      if (request.request.type === 'next_input' && lease) this.reading.delete(lease);
      this.completed.add(request.request_id);
      if (this.completed.size > MAX_REMEMBERED_REQUESTS) this.completed.delete(this.completed.values().next().value!);
    });
    this.pending.set(request.request_id, { fingerprint, promise });
    return promise;
  }

  private async dispatch(request: MediaDeviceRequestEnvelopeDto, onLeaseClosed?: () => void): Promise<MediaDeviceResponseDto> {
    try {
      const response = validateMediaDeviceResponse(await this.options.audio.executeMediaDevice(request));
      if (response.request_id !== request.request_id) throw new Error('Mismatched native media response');
      if (response.result.type === 'error') return response;
      if (response.result.type === 'open_failed_without_resource') {
        if (request.request.type !== 'open') throw new Error('Unexpected native media open failure receipt');
        this.openReservations.delete(request.request_id);
        return response;
      }
      const expected = {
        open: 'opened', next_input: 'input', write_audio: 'audio_written', write_video: 'video_written',
        flush_audio: 'audio_flushed', interrupt_audio: 'audio_interrupted', update: 'updated', stop: 'stopped',
      } as const;
      if (response.result.type !== expected[request.request.type]) throw new Error('Unexpected native media result');
      if (response.result.type === 'opened') {
        const id = response.result.lease.lease_id;
        if (this.leases.has(id)) throw new Error('Native media returned an existing lease');
        this.leases.add(id);
        this.leaseOpenRequests.set(id, { requestId: request.request_id, onClose: onLeaseClosed });
        if (this.closed) {
          this.quarantined.add(id);
          await this.stopNativeLease(id);
          this.forgetLease(id);
          return rejectedOpen(request.request_id, 'cancelled', 'The native media connection closed during capture setup');
        }
      }
      if (this.closed && response.result.type !== 'opened' && response.result.type !== 'stopped') {
        return failure(request.request_id, 'cancelled', 'The native media connection is closed');
      }
      if (response.result.type === 'audio_written' && request.request.type === 'write_audio') {
        const frame = request.request.request.frame;
        const sent = frame.pcm16le.length / (2 * frame.channels);
        if (response.result.receipt.accepted_sample_frames > sent
          || response.result.receipt.played_sample_frames > response.result.receipt.accepted_sample_frames) {
          throw new Error('Native playback receipt exceeds the submitted frame');
        }
      }
      if (response.result.type === 'input' && response.result.frame) {
        const frame = response.result.frame;
        const key = `${leaseId(request)}:${frame.source.id}`;
        const previous = this.sourceSequences.get(key);
        if (previous !== undefined && frame.sequence <= previous) throw new Error('Native capture sequence did not advance');
        if (previous === undefined && [...this.sourceSequences.keys()].filter((source) => source.startsWith(`${leaseId(request)}:`)).length >= 4) {
          throw new Error('Native capture produced too many sources for its lease');
        }
        this.sourceSequences.set(key, frame.sequence);
      }
      if (response.result.type === 'stopped') this.forgetLease(leaseId(request)!);
      return response;
    } catch {
      const lease = leaseId(request);
      if (lease) {
        this.quarantined.add(lease);
        await this.stopNativeLease(lease).then(() => this.forgetLease(lease), () => undefined);
      }
      return failure(request.request_id, 'native_failure', 'The native media adapter returned an invalid result or could not complete the request');
    }
  }

  private forgetLease(id: string): void {
    const openRequest = this.leaseOpenRequests.get(id);
    if (openRequest) {
      this.openReservations.delete(openRequest.requestId);
      openRequest.onClose?.();
    }
    this.leaseOpenRequests.delete(id);
    this.leases.delete(id);
    this.quarantined.delete(id);
    this.reading.delete(id);
    for (const key of this.sourceSequences.keys()) if (key.startsWith(`${id}:`)) this.sourceSequences.delete(key);
  }

  private async stopNativeLease(id: string): Promise<void> {
    const request = { request_id: randomUUID(), request: { type: 'stop' as const, lease_id: id } };
    const response = validateMediaDeviceResponse(await this.options.audio.executeMediaDevice(request));
    if (response.request_id !== request.request_id || response.result.type !== 'stopped') {
      throw new Error('Native media did not acknowledge lease cleanup');
    }
  }

  /** Close this bridge's leases; a late open is stopped by dispatch itself. */
  dispose(): Promise<void> {
    if (this.disposing) return this.disposing;
    this.closed = true;
    const cleanup = async () => {
      const pending = [...this.pending.values()].map((entry) => entry.promise);
      const results = await Promise.allSettled([...this.leases].map(async (id) => {
        await this.stopNativeLease(id);
        this.forgetLease(id);
      }));
      // An open already in flight can acquire a lease after the first snapshot.
      // Its dispatch path closes it, and disposal must observe that acknowledgement.
      await Promise.allSettled(pending);
      if (this.leases.size > 0 || results.some((result) => result.status === 'rejected')) {
        throw new Error('Native media cleanup was not acknowledged');
      }
    };
    this.disposing = cleanup().finally(() => { this.disposing = undefined; });
    return this.disposing;
  }
}
