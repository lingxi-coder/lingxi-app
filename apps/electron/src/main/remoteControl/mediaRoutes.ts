/**
 * The Electron side of the product media route. Bridge-server supplies offers
 * from the shared MediaConnector; this registry binds each one to the exact
 * approved Remote Control channel that owns its route and media session.
 */

import { randomUUID } from 'node:crypto';

import {
  RemoteRpcError,
  parseRemoteMediaBridgeOfferRequest,
  parseRemoteMediaBridgeControlRequest,
  parseRemoteMediaAnswer,
  parseRemoteMediaControlResult,
  parseRemoteMediaPlaybackReceipt,
  type RemoteMediaBridgeOfferRequest,
  type RemoteMediaBridgeOfferResponse,
  type RemoteMediaBridgeControlRequest,
  type RemoteMediaControlResult,
  type RemoteMediaPlaybackReceipt,
  type RemoteMediaCapabilities,
  isRemoteMediaSourceForRoute,
} from '@lingxi/bridge-client/remote';

import {
  validateMediaConnectionRequest,
  validateMediaControlRequest,
  type MediaConnectionRequestDto,
  type MediaConnectionResponseDto,
  type MediaControlRequestDto,
  type MediaControlResponseDto,
  type MediaErrorDto,
} from '@lingxi/bridge-client';

import type { RemoteViewer, RemoteViewerChannel } from './gateway.js';
import { isSessionId } from '../sessionIdentity.js';

const MAX_PENDING_OFFERS = 32;
const MAX_COMPLETED_OFFERS = 32;
const MAX_COMPLETED_CONTROLS = 64;
const MAX_SESSIONS_PER_ROUTE = 16;

export interface RemoteMediaRouteRecord {
  /** Host-issued product route token; equals generic MediaConnectionRequest.connection_id. */
  route_id: string;
  account_uid: string;
  host_id: string;
  device_id: string;
  /** Signed Remote Control connection id; never used as the media route token. */
  remote_control_connection_id: string;
}

/**
 * App bridge boundary implemented by the BridgeClient/bridge-server adapter.
 * Registration is explicit so the Rust connector cannot select an arbitrary
 * first viewer when multiple devices are connected.
 */
export interface RemoteMediaRouteError {
  code: 'not_approved' | 'forbidden' | 'invalid_params' | 'closed' | 'timeout' | 'internal';
  message: string;
}

interface SessionBinding {
  sessionId: string;
  generation?: number;
  lastReceipt?: RemoteMediaPlaybackReceipt;
  /** Native cumulative count observed at the previous barrier response. */
  lastControlSequence: number;
  lastControlPlayed: number;
  lastCumulativePlayed: number;
  /** Each interrupt/close supersedes any older pending flush operation. */
  controlEpoch: number;
  pendingFlushAborters: Set<() => void>;
  aborters: Set<() => void>;
  closing: boolean;
}

interface Route {
  record: RemoteMediaRouteRecord;
  viewer: RemoteViewer;
  channel: RemoteViewerChannel;
  sessions: Map<string, SessionBinding>;
  authorizedSessions: Set<string>;
  closed: boolean;
}

interface InFlight {
  fingerprint: string;
  routeId: string;
  sessionRef: string;
  promise: Promise<RemoteMediaBridgeOfferResponse>;
}

interface Completed {
  fingerprint: string;
  routeId: string;
  sessionRef: string;
  response: RemoteMediaBridgeOfferResponse;
}

interface InFlightControl {
  fingerprint: string;
  routeId: string;
  sessionRef: string;
  promise: Promise<MediaControlResponseDto>;
}

interface CompletedControl {
  fingerprint: string;
  routeId: string;
  sessionRef: string;
  response: MediaControlResponseDto;
}

function routeKey(viewer: RemoteViewer): string {
  return `${viewer.account_uid ?? ''}\0${viewer.host_id}\0${viewer.conn_id}`;
}

function validCapabilities(value: RemoteMediaCapabilities | undefined): value is RemoteMediaCapabilities {
  if (value === undefined || value.version !== 1 || value.modes.length < 1 || value.modes.length > 3
    || value.source_kinds.length < 1 || value.source_kinds.length > 3
    || new Set(value.modes).size !== value.modes.length
    || new Set(value.source_kinds).size !== value.source_kinds.length) return false;
  const audio = value.modes.some(mode => mode === 'audio' || mode === 'audio_video');
  const video = value.modes.some(mode => mode === 'video' || mode === 'audio_video');
  return audio === value.source_kinds.includes('microphone')
    && video === value.source_kinds.some(kind => kind === 'camera' || kind === 'screen');
}

function rpcError(error: unknown): RemoteMediaRouteError {
  if (error instanceof RemoteRpcError) {
    const allowed = ['not_approved', 'forbidden', 'invalid_params', 'closed', 'timeout'] as const;
    const code = (allowed as readonly string[]).includes(error.code) ? error.code as RemoteMediaRouteError['code'] : 'internal';
    return { code, message: code === 'internal' ? 'media negotiation failed' : code };
  }
  return { code: 'internal', message: 'media negotiation failed' };
}

function failure(request: Pick<RemoteMediaBridgeOfferRequest, 'request_id' | 'route_id'>, error: RemoteMediaRouteError): RemoteMediaBridgeOfferResponse {
  return { request_id: request.request_id, route_id: request.route_id, error };
}

function fingerprint(request: RemoteMediaBridgeOfferRequest): string {
  return JSON.stringify(request.offer);
}

function controlFingerprint(request: RemoteMediaBridgeControlRequest): string {
  return JSON.stringify(request.control);
}

function controlError(code: RemoteMediaRouteError['code'], message: string): MediaErrorDto {
  const kinds: Record<RemoteMediaRouteError['code'], MediaErrorDto['kind']> = {
    not_approved: 'permission_denied',
    forbidden: 'permission_denied',
    invalid_params: 'invalid_request',
    closed: 'cancelled',
    timeout: 'timeout',
    internal: 'native_failure',
  };
  return { kind: kinds[code], message: message.slice(0, 256) };
}

function controlFailure(request: MediaControlRequestDto, error: RemoteMediaRouteError): MediaControlResponseDto {
  return {
    request_id: request.request_id,
    connection_id: request.connection_id,
    media_session_ref: request.media_session_ref,
    generation: request.generation,
    error: controlError(error.code, error.message),
  };
}

function connectionFailure(request: MediaConnectionRequestDto, error: RemoteMediaRouteError): MediaConnectionResponseDto {
  return {
    request_id: request.request_id,
    connection_id: request.connection_id,
    media_session_ref: request.media_session_ref,
    error: controlError(error.code, error.message),
  };
}

/**
 * Owns route and host-issued session bindings only; MediaService and MediaSession
 * registries stay in the shared host runtime. Every offer is forwarded over the
 * already approved authenticated control link.
 */
export class RemoteMediaRoutes {
  private readonly routes = new Map<string, Route>();
  private readonly byConnection = new Map<string, string>();
  private readonly registering = new Map<string, Promise<string>>();
  private readonly inFlight = new Map<string, InFlight>();
  private readonly completed = new Map<string, Completed>();
  private readonly controlInFlight = new Map<string, InFlightControl>();
  private readonly controlCompleted = new Map<string, CompletedControl>();

  constructor() {}

  /** Register a viewer only after HostConnection has completed approval. */
  async registerViewer(viewer: RemoteViewer, channel: RemoteViewerChannel): Promise<string> {
    const accountUid = viewer.account_uid;
    if (!accountUid || !viewer.device_id || !viewer.conn_id) throw new Error('invalid approved media viewer');
    if (!validCapabilities(viewer.media_capabilities)) throw new Error('viewer did not declare supported remote media');
    const connectionKey = routeKey(viewer);
    const existingRegistration = this.registering.get(connectionKey);
    if (existingRegistration !== undefined) return existingRegistration;
    const registration = this.registerViewerRoute(viewer, channel, connectionKey, accountUid);
    this.registering.set(connectionKey, registration);
    try {
      return await registration;
    } finally {
      if (this.registering.get(connectionKey) === registration) this.registering.delete(connectionKey);
    }
  }

  private async registerViewerRoute(
    viewer: RemoteViewer,
    channel: RemoteViewerChannel,
    connectionKey: string,
    accountUid: string,
  ): Promise<string> {
    const existingId = this.byConnection.get(connectionKey);
    const existing = existingId === undefined ? undefined : this.routes.get(existingId);
    if (existing && !existing.closed) {
      if (existing.viewer.account_uid !== viewer.account_uid || existing.viewer.device_id !== viewer.device_id) {
        throw new Error('remote media route owner changed');
      }
      return existing.record.route_id;
    }

    const record: RemoteMediaRouteRecord = {
      route_id: randomUUID(),
      account_uid: accountUid,
      host_id: viewer.host_id,
      device_id: viewer.device_id,
      remote_control_connection_id: viewer.conn_id,
    };
    const route: Route = { record, viewer: { ...viewer }, channel, sessions: new Map(), authorizedSessions: new Set(), closed: false };
    this.routes.set(record.route_id, route);
    this.byConnection.set(connectionKey, record.route_id);
    return record.route_id;
  }

  /** Bind a session ref returned by MediaService to this viewer and owner session. */
  allowSession(routeId: string, sessionId: string): void {
    const route = this.routes.get(routeId);
    if (route === undefined || route.closed) throw new RemoteRpcError('closed', 'the media route is closed');
    if (!isSessionId(sessionId)) throw new RemoteRpcError('invalid_params', 'invalid media owner session');
    route.authorizedSessions.add(sessionId);
  }

  disallowSession(routeId: string, sessionId: string): void {
    const route = this.routes.get(routeId);
    if (route === undefined) return;
    route.authorizedSessions.delete(sessionId);
    for (const [reference, binding] of route.sessions) {
      if (binding.sessionId === sessionId) this.unbindSession(routeId, reference);
    }
  }

  bindSession(routeId: string, sessionId: string, mediaSessionRef: string, generation?: number): void {
    const route = this.routes.get(routeId);
    if (route === undefined || route.closed) throw new RemoteRpcError('closed', 'the media route is closed');
    if (!isSessionId(sessionId) || mediaSessionRef.length === 0 || mediaSessionRef.length > 128) {
      throw new RemoteRpcError('invalid_params', 'invalid media session reference');
    }
    const existing = route.sessions.get(mediaSessionRef);
    if (existing && existing.sessionId !== sessionId) throw new RemoteRpcError('forbidden', 'media session owner changed');
    if (!existing && route.sessions.size >= MAX_SESSIONS_PER_ROUTE) {
      throw new RemoteRpcError('capacity', 'this device has too many active media sessions');
    }
    if (generation !== undefined && (!Number.isSafeInteger(generation) || generation < 1)) {
      throw new RemoteRpcError('invalid_params', 'invalid media session generation');
    }
    route.sessions.set(mediaSessionRef, existing ?? {
      sessionId,
      ...(generation === undefined ? {} : { generation }),
      lastControlSequence: 0,
      lastControlPlayed: 0,
      lastCumulativePlayed: 0,
      controlEpoch: 0,
      pendingFlushAborters: new Set(),
      aborters: new Set(),
      closing: false,
    });
    if (existing && generation !== undefined) {
      if (existing.generation !== undefined && existing.generation !== generation) {
        throw new RemoteRpcError('forbidden', 'media session generation changed');
      }
      existing.generation = generation;
    }
  }

  /** Remove one binding after `media.stop` or session detach has completed. */
  unbindSession(routeId: string, mediaSessionRef: string): void {
    const route = this.routes.get(routeId);
    if (route === undefined) return;
    const session = route.sessions.get(mediaSessionRef);
    if (session === undefined) return;
    route.sessions.delete(mediaSessionRef);
    for (const abort of session.aborters) abort();
    this.dropCompletedFor(routeId, mediaSessionRef);
    this.dropCompletedControlsFor(routeId, mediaSessionRef);
  }

  /** Unregister one approved device route; outstanding control requests fail closed. */
  async unregisterViewer(routeId: string): Promise<void> {
    const route = this.routes.get(routeId);
    if (route === undefined || route.closed) return;
    route.closed = true;
    this.routes.delete(routeId);
    this.byConnection.delete(routeKey(route.viewer));
    for (const session of route.sessions.values()) for (const abort of session.aborters) abort();
    route.sessions.clear();
    for (const [requestId, entry] of this.inFlight) if (entry.routeId === routeId) this.inFlight.delete(requestId);
    for (const [requestId, entry] of this.controlInFlight) if (entry.routeId === routeId) this.controlInFlight.delete(requestId);
    this.dropCompletedFor(routeId);
    this.dropCompletedControlsFor(routeId);
  }

  /**
   * Handle one host-only bridge event. Its correlation token and route are
   * validated before it can reach a viewer's control channel.
   */
  async handleOfferEvent(raw: unknown): Promise<RemoteMediaBridgeOfferResponse> {
    let request: RemoteMediaBridgeOfferRequest;
    try {
      request = parseRemoteMediaBridgeOfferRequest(raw);
    } catch {
      // No trusted correlation fields are available for a response.
      throw new RemoteRpcError('invalid_params', 'invalid remote media bridge request');
    }
    const key = `${request.route_id}/${request.request_id}`;
    const body = fingerprint(request);
    const completed = this.completed.get(key);
    if (completed !== undefined) {
      return completed.fingerprint === body
        ? completed.response
        : failure(request, { code: 'invalid_params', message: 'media request id was reused' });
    }
    const pending = this.inFlight.get(key);
    if (pending !== undefined) {
      return pending.fingerprint === body
        ? pending.promise
        : failure(request, { code: 'invalid_params', message: 'media request id was reused' });
    }
    if (this.inFlight.size >= MAX_PENDING_OFFERS) {
      return failure(request, { code: 'timeout', message: 'too many media negotiations are pending' });
    }

    const route = this.routes.get(request.route_id);
    if (route === undefined || route.closed) return failure(request, { code: 'closed', message: 'media route is closed' });
    const binding = route.sessions.get(request.offer.media_session_ref);
    if (binding === undefined || binding.sessionId !== request.offer.session_id) {
      return failure(request, { code: 'forbidden', message: 'media session is not owned by this route' });
    }
    if (binding.generation !== undefined && binding.generation !== request.offer.generation) {
      return failure(request, { code: 'forbidden', message: 'media session generation changed' });
    }
    const capabilities = route.viewer.media_capabilities!;
    if (!capabilities.modes.includes(request.offer.mode)
      || request.offer.mode !== 'video' && request.offer.mode !== 'audio' && request.offer.mode !== 'audio_video') {
      return failure(request, { code: 'forbidden', message: 'viewer did not declare this media mode' });
    }
    if ((request.offer.mode === 'audio' || request.offer.mode === 'audio_video')
      && (!isRemoteMediaSourceForRoute(request.route_id, request.offer.audio_source_ref, 'microphone')
        || !capabilities.source_kinds.includes('microphone'))) {
      return failure(request, { code: 'forbidden', message: 'audio source is not declared for this route' });
    }
    if ((request.offer.mode === 'video' || request.offer.mode === 'audio_video')
      && (!request.offer.video_source_ref || !isRemoteMediaSourceForRoute(request.route_id, request.offer.video_source_ref)
        || !capabilities.source_kinds.includes(request.offer.video_source_ref.endsWith('_screen') ? 'screen' : 'camera'))) {
      return failure(request, { code: 'forbidden', message: 'video source is not declared for this route' });
    }
    binding.generation = request.offer.generation;

    let abortRequest!: () => void;
    const aborted = new Promise<never>((_, reject) => {
      abortRequest = () => reject(new RemoteRpcError('closed', 'media route closed'));
    });
    binding.aborters.add(abortRequest);
    const operation = (async (): Promise<RemoteMediaBridgeOfferResponse> => {
      try {
        const rawAnswer = await Promise.race([
          route.channel.request('media.offer', request.offer),
          aborted,
        ]);
        if (route.closed || this.routes.get(request.route_id) !== route
          || route.sessions.get(request.offer.media_session_ref) !== binding) {
          return failure(request, { code: 'closed', message: 'media route closed' });
        }
        try {
          const answer = parseRemoteMediaAnswer(rawAnswer, request.offer);
          return { request_id: request.request_id, route_id: request.route_id, result: answer };
        } catch {
          return failure(request, { code: 'invalid_params', message: 'viewer returned an invalid media answer' });
        }
      } catch (error) {
        return failure(request, rpcError(error));
      } finally {
        binding.aborters.delete(abortRequest);
      }
    })();
    this.inFlight.set(key, { fingerprint: body, routeId: request.route_id, sessionRef: request.offer.media_session_ref, promise: operation });
    const response = await operation;
    this.inFlight.delete(key);
    if (!route.closed && this.routes.get(request.route_id) === route
      && route.sessions.get(request.offer.media_session_ref) === binding) {
      this.rememberCompleted(key, {
        fingerprint: body,
        routeId: request.route_id,
        sessionRef: request.offer.media_session_ref,
        response,
      });
    }
    return response;
  }

  /** Convert the generic host-control offer into the product RPC for one approved viewer. */
  async handleConnectionRequest(raw: unknown): Promise<MediaConnectionResponseDto> {
    let request: MediaConnectionRequestDto;
    try {
      request = validateMediaConnectionRequest(raw);
    } catch {
      throw new RemoteRpcError('invalid_params', 'invalid media connection request');
    }
    if (request.owner.type !== 'session' || request.owner.session_id !== request.session_id) {
      return connectionFailure(request, { code: 'forbidden', message: 'media owner is not the attached session' });
    }
    const route = this.routes.get(request.connection_id);
    if (route === undefined || route.closed || !route.authorizedSessions.has(request.session_id)) {
      return connectionFailure(request, { code: 'forbidden', message: 'media route is not attached to this session' });
    }
    const offer = {
      v: 1 as const,
      media_session_ref: request.media_session_ref.id,
      session_id: request.session_id,
      generation: request.operation_id.generation,
      mode: request.mode,
      intent: request.intent,
      ...(request.audio_source?.id ? { audio_source_ref: request.audio_source.id } : {}),
      ...(request.video_source?.id ? { video_source_ref: request.video_source.id } : {}),
      offer_sdp: request.description.sdp,
    };
    try {
      this.bindSession(request.connection_id, request.session_id, request.media_session_ref.id, request.operation_id.generation);
    } catch {
      return connectionFailure(request, { code: 'forbidden', message: 'media session is already bound to another request' });
    }
    const productResponse = await this.handleOfferEvent({
      request_id: request.request_id,
      route_id: request.connection_id,
      offer,
    });
    if ('result' in productResponse) {
      return {
        request_id: request.request_id,
        connection_id: request.connection_id,
        media_session_ref: request.media_session_ref,
        description: { kind: 'answer', sdp: productResponse.result.answer_sdp },
      };
    }
    this.unbindSession(request.connection_id, request.media_session_ref.id);
    return connectionFailure(request, productResponse.error);
  }

  /** Handle one correlated peer output barrier from the shared media connector. */
  async handleControlEvent(raw: MediaControlRequestDto): Promise<MediaControlResponseDto> {
    try {
      raw = validateMediaControlRequest(raw);
    } catch {
      return controlFailure(raw, { code: 'invalid_params', message: 'invalid media control request' });
    }
    const control = {
      v: 1,
      media_session_ref: raw.media_session_ref?.id,
      generation: raw.generation,
      action: raw.action?.type,
      target_audio_sequence: raw.action?.type === 'close' ? 0 : raw.action?.target_sequence,
      target_sample_frames: raw.action?.type === 'close' ? 0 : raw.action?.target_sample_frames,
    };
    let request: RemoteMediaBridgeControlRequest;
    try {
      request = parseRemoteMediaBridgeControlRequest({
        request_id: raw.request_id,
        route_id: raw.connection_id,
        control,
      });
    } catch {
      return controlFailure(raw, { code: 'invalid_params', message: 'invalid media control request' });
    }
    const key = `${request.route_id}/${request.request_id}`;
    const body = controlFingerprint(request);
    const completed = this.controlCompleted.get(key);
    if (completed !== undefined) {
      return completed.fingerprint === body
        ? completed.response
        : controlFailure(raw, { code: 'invalid_params', message: 'media control request id was reused' });
    }
    const pending = this.controlInFlight.get(key);
    if (pending !== undefined) {
      return pending.fingerprint === body
        ? pending.promise
        : controlFailure(raw, { code: 'invalid_params', message: 'media control request id was reused' });
    }
    const route = this.routes.get(request.route_id);
    if (route === undefined || route.closed) {
      return controlFailure(raw, { code: 'closed', message: 'media route is closed' });
    }
    const binding = route.sessions.get(request.control.media_session_ref);
    if (binding === undefined || binding.generation !== request.control.generation) {
      return controlFailure(raw, { code: 'forbidden', message: 'media session is not owned by this route' });
    }

    const action = request.control.action;
    if (binding.closing && action !== 'close') {
      return controlFailure(raw, { code: 'closed', message: 'media session is closing' });
    }
    if (action === 'interrupt_audio' || action === 'close') {
      binding.controlEpoch += 1;
      if (action === 'close') binding.closing = true;
      for (const abort of binding.pendingFlushAborters) abort();
    }
    const controlEpoch = binding.controlEpoch;

    let abortRequest!: () => void;
    const aborted = new Promise<never>((_, reject) => {
      abortRequest = () => reject(new RemoteRpcError('closed', 'media route closed'));
    });
    binding.aborters.add(abortRequest);
    let abortSupersededFlush!: () => void;
    const supersededFlush = new Promise<never>((_, reject) => {
      abortSupersededFlush = () => reject(new RemoteRpcError('closed', 'media flush was superseded'));
    });
    if (action === 'flush_audio') binding.pendingFlushAborters.add(abortSupersededFlush);
    const operation = (async (): Promise<MediaControlResponseDto> => {
      try {
        const pendingRequests: Promise<unknown>[] = [route.channel.request('media.control', request.control), aborted];
        if (action === 'flush_audio') pendingRequests.push(supersededFlush);
        const rawResult = await Promise.race(pendingRequests);
        if (route.closed || this.routes.get(request.route_id) !== route
          || route.sessions.get(request.control.media_session_ref) !== binding) {
          return controlFailure(raw, { code: 'closed', message: 'media route closed' });
        }
        if (binding.controlEpoch !== controlEpoch) {
          return controlFailure(raw, { code: 'closed', message: 'media control was superseded' });
        }
        let result: RemoteMediaControlResult;
        try {
          result = parseRemoteMediaControlResult(rawResult, request.control);
        } catch {
          return controlFailure(raw, { code: 'invalid_params', message: 'viewer returned an invalid playback receipt' });
        }
        if (result.audio_sequence < binding.lastControlSequence
          || result.accepted_sample_frames < (binding.lastReceipt?.accepted_sample_frames ?? 0)
          || result.played_sample_frames < binding.lastControlPlayed) {
          return controlFailure(raw, { code: 'invalid_params', message: 'viewer playback progress moved backwards' });
        }
        const playedDelta = result.played_sample_frames - binding.lastControlPlayed;
        binding.lastControlSequence = result.audio_sequence;
        binding.lastControlPlayed = result.played_sample_frames;
        binding.lastCumulativePlayed = Math.max(binding.lastCumulativePlayed, result.played_sample_frames);
        if (result.played_sample_frames >= (binding.lastReceipt?.played_sample_frames ?? 0)) {
          binding.lastReceipt = {
            v: 1,
            media_session_ref: result.media_session_ref,
            generation: result.generation,
            audio_sequence: result.audio_sequence,
            accepted_sample_frames: result.accepted_sample_frames,
            played_sample_frames: result.played_sample_frames,
            ...(result.rendered_at_unix_ms === undefined ? {} : { rendered_at_unix_ms: result.rendered_at_unix_ms }),
          };
        }
        return {
          request_id: raw.request_id,
          connection_id: raw.connection_id,
          media_session_ref: raw.media_session_ref,
          generation: raw.generation,
          receipt: {
            accepted_sample_frames: 0,
            played_sample_frames: playedDelta,
            ...(result.rendered_at_unix_ms === undefined ? {} : { rendered_at_unix_ms: result.rendered_at_unix_ms }),
          },
        };
      } catch (error) {
        return controlFailure(raw, rpcError(error));
      } finally {
        binding.aborters.delete(abortRequest);
        if (action === 'flush_audio') binding.pendingFlushAborters.delete(abortSupersededFlush);
      }
    })();
    this.controlInFlight.set(key, {
      fingerprint: body,
      routeId: request.route_id,
      sessionRef: request.control.media_session_ref,
      promise: operation,
    });
    const response = await operation;
    this.controlInFlight.delete(key);
    if (!route.closed && this.routes.get(request.route_id) === route
      && route.sessions.get(request.control.media_session_ref) === binding) {
      this.rememberCompletedControl(key, {
        fingerprint: body,
        routeId: request.route_id,
        sessionRef: request.control.media_session_ref,
        response,
      });
    }
    return response;
  }

  /** Validate a viewer playback notification and update route-local progress. */
  recordPlaybackReceipt(viewer: RemoteViewer, raw: unknown): boolean {
    const receipt = parseRemoteMediaPlaybackReceipt(raw);
    const routeId = this.byConnection.get(routeKey(viewer));
    const route = routeId === undefined ? undefined : this.routes.get(routeId);
    const binding = route?.sessions.get(receipt.media_session_ref);
    if (route === undefined || route.closed || binding === undefined
      || binding.generation !== receipt.generation) {
      throw new RemoteRpcError('forbidden', 'media receipt is not owned by this route');
    }
    const previous = binding.lastReceipt;
    if (previous !== undefined) {
      if (receipt.audio_sequence < previous.audio_sequence
        || receipt.accepted_sample_frames < previous.accepted_sample_frames
        || receipt.played_sample_frames < previous.played_sample_frames) {
        throw new RemoteRpcError('invalid_params', 'media playback progress moved backwards');
      }
      if (receipt.audio_sequence === previous.audio_sequence
        && receipt.accepted_sample_frames === previous.accepted_sample_frames
        && receipt.played_sample_frames === previous.played_sample_frames) return false;
    }
    binding.lastReceipt = receipt;
    binding.lastCumulativePlayed = Math.max(binding.lastCumulativePlayed, receipt.played_sample_frames);
    return true;
  }

  private rememberCompleted(key: string, entry: Completed): void {
    this.completed.delete(key);
    this.completed.set(key, entry);
    while (this.completed.size > MAX_COMPLETED_OFFERS) {
      const oldest = this.completed.keys().next().value;
      if (oldest === undefined) break;
      this.completed.delete(oldest);
    }
  }

  private dropCompletedFor(routeId: string, sessionRef?: string): void {
    for (const [key, entry] of this.completed) {
      if (entry.routeId === routeId
        && (sessionRef === undefined || entry.sessionRef === sessionRef)) {
        this.completed.delete(key);
      }
    }
  }

  private rememberCompletedControl(key: string, entry: CompletedControl): void {
    this.controlCompleted.delete(key);
    this.controlCompleted.set(key, entry);
    while (this.controlCompleted.size > MAX_COMPLETED_CONTROLS) {
      const oldest = this.controlCompleted.keys().next().value;
      if (oldest === undefined) break;
      this.controlCompleted.delete(oldest);
    }
  }

  private dropCompletedControlsFor(routeId: string, sessionRef?: string): void {
    for (const [key, entry] of this.controlCompleted) {
      if (entry.routeId === routeId
        && (sessionRef === undefined || entry.sessionRef === sessionRef)) {
        this.controlCompleted.delete(key);
      }
    }
  }
}
