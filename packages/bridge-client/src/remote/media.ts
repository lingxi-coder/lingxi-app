/** Product Remote Control media negotiation on the approved control link. */
import { RemoteProtocolError } from './errors.js';

/** Hard bound before SDP reaches RPC framing or a native peer implementation. */
export const REMOTE_MEDIA_SDP_MAX_CHARS = 64 * 1024;
export const REMOTE_MEDIA_REFERENCE_MAX_CHARS = 128;
export const REMOTE_MEDIA_AUDIO_SEQUENCE_MAX = 180_000;
export const REMOTE_MEDIA_AUDIO_SAMPLE_FRAMES_MAX = REMOTE_MEDIA_AUDIO_SEQUENCE_MAX * 960;
export const REMOTE_MEDIA_COUNTER_MAX = Number.MAX_SAFE_INTEGER;

export type RemoteMediaSourceKind = 'microphone' | 'camera' | 'screen';

/** Explicit viewer-side product support; approval alone does not imply media. */
export interface RemoteMediaCapabilities {
  version: 1;
  modes: RemoteMediaMode[];
  source_kinds: RemoteMediaSourceKind[];
}

/** Stable product-side reference syntax shared by connector and gateway. */
export function remoteMediaSourceRef(routeId: string, kind: RemoteMediaSourceKind): string {
  if (!validRouteId(routeId)) throw new RemoteProtocolError('invalid media route id');
  return `remote_${routeId}_${kind}`;
}

/** Check exact route ownership and source category before a host RPC. */
export function isRemoteMediaSourceForRoute(
  routeId: string,
  sourceRef: unknown,
  kind?: RemoteMediaSourceKind,
): sourceRef is string {
  if (!validRouteId(routeId) || !validReference(sourceRef)) return false;
  const prefix = `remote_${routeId}_`;
  if (!sourceRef.startsWith(prefix)) return false;
  const suffix = sourceRef.slice(prefix.length);
  return kind === undefined
    ? suffix === 'microphone' || suffix === 'camera' || suffix === 'screen'
    : suffix === kind;
}

export type RemoteMediaMode = 'audio' | 'video' | 'audio_video';
export type RemoteMediaIntent = 'observation' | 'conversation';

/** Host-to-viewer negotiation request; no AV frame data is carried here. */
export interface RemoteMediaOfferParams {
  v: 1;
  media_session_ref: string;
  session_id: string;
  generation: number;
  mode: RemoteMediaMode;
  intent: RemoteMediaIntent;
  audio_source_ref?: string;
  video_source_ref?: string;
  offer_sdp: string;
}

/** Viewer-to-host result for one host-issued media session. */
export interface RemoteMediaAnswerResult {
  v: 1;
  media_session_ref: string;
  answer_sdp: string;
}

export type RemoteMediaControlAction = 'flush_audio' | 'interrupt_audio' | 'close';

/** Bounded media control over the existing authenticated Remote Control RPC. */
export interface RemoteMediaControlRequestParams {
  v: 1;
  media_session_ref: string;
  generation: number;
  action: RemoteMediaControlAction;
  target_audio_sequence: number;
  target_sample_frames: number;
}

/** Actual native playback progress returned for one flush or interruption. */
export interface RemoteMediaControlResult {
  v: 1;
  media_session_ref: string;
  generation: number;
  audio_sequence: number;
  accepted_sample_frames: number;
  played_sample_frames: number;
  rendered_at_unix_ms?: number;
}

/** Viewer notification reporting callback-confirmed progress on the exact session. */
export interface RemoteMediaPlaybackReceipt {
  v: 1;
  media_session_ref: string;
  generation: number;
  audio_sequence: number;
  accepted_sample_frames: number;
  played_sample_frames: number;
  rendered_at_unix_ms?: number;
}

/** Bridge-server event routed to Electron main for one approved control peer. */
export interface RemoteMediaBridgeOfferRequest {
  request_id: string;
  route_id: string;
  offer: RemoteMediaOfferParams;
}

export type RemoteMediaBridgeErrorCode = 'not_approved' | 'forbidden' | 'invalid_params' | 'closed' | 'timeout' | 'internal';

/** Correlated Electron response for a bridge-server media offer event. */
export type RemoteMediaBridgeOfferResponse =
  | { request_id: string; route_id: string; result: RemoteMediaAnswerResult }
  | { request_id: string; route_id: string; error: { code: RemoteMediaBridgeErrorCode; message: string } };

/** Bridge-server control request to one exact approved viewer/session route. */
export interface RemoteMediaBridgeControlRequest {
  request_id: string;
  route_id: string;
  control: RemoteMediaControlRequestParams;
}

/** Correlated actual-progress response to a bridge-server media control request. */
export type RemoteMediaBridgeControlResponse =
  | { request_id: string; route_id: string; result: RemoteMediaControlResult }
  | { request_id: string; route_id: string; error: { code: RemoteMediaBridgeErrorCode; message: string } };

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function validReference(value: unknown): value is string {
  return typeof value === 'string'
    && value.length > 0
    && value.length <= REMOTE_MEDIA_REFERENCE_MAX_CHARS
    && /^[A-Za-z0-9_-]+$/u.test(value);
}

function validRequestId(value: unknown): value is string {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{22}$/u.test(value);
}

/** Host-issued, per-viewer UUID used only in the product media adapter. */
function validRouteId(value: unknown): value is string {
  return typeof value === 'string'
    && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(value);
}

function validSessionId(value: unknown): value is string {
  return typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(value);
}

function validCounter(value: unknown, maximum: number): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}

function validProgress(sequence: unknown, accepted: unknown, played: unknown): boolean {
  return validCounter(sequence, REMOTE_MEDIA_AUDIO_SEQUENCE_MAX)
    && validCounter(accepted, REMOTE_MEDIA_AUDIO_SAMPLE_FRAMES_MAX)
    && validCounter(played, REMOTE_MEDIA_AUDIO_SAMPLE_FRAMES_MAX)
    && played <= accepted;
}

function validSdp(sdp: unknown, mode: RemoteMediaMode): sdp is string {
  if (typeof sdp !== 'string' || sdp.length < 1 || sdp.length > REMOTE_MEDIA_SDP_MAX_CHARS || sdp.includes('\0') || !sdp.startsWith('v=0\r\n')) return false;
  const audio = /^m=audio /mu.test(sdp);
  const video = /^m=video /mu.test(sdp);
  return audio === (mode === 'audio' || mode === 'audio_video')
    && video === (mode === 'video' || mode === 'audio_video');
}

const MODES: readonly string[] = ['audio', 'video', 'audio_video'];
const INTENTS: readonly string[] = ['observation', 'conversation'];

/** Validate an untrusted host media offer before an adapter touches native devices. */
export function parseRemoteMediaOffer(raw: unknown): RemoteMediaOfferParams {
  const invalid = () => new RemoteProtocolError('invalid media offer');
  if (!isObject(raw)) throw invalid();
  const { v, media_session_ref, session_id, generation, mode, intent, audio_source_ref, video_source_ref, offer_sdp } = raw;
  const keys = Object.keys(raw).sort().join(',');
  const expectedKeys = [
    'audio_source_ref', 'generation', 'intent', 'media_session_ref', 'mode', 'offer_sdp', 'session_id', 'v', 'video_source_ref',
  ].filter((key) => raw[key] !== undefined).sort().join(',');
  if (keys !== expectedKeys || v !== 1 || !validReference(media_session_ref) || !validSessionId(session_id)
    || !validCounter(generation, REMOTE_MEDIA_COUNTER_MAX) || generation === 0
    || typeof mode !== 'string' || !MODES.includes(mode)
    || typeof intent !== 'string' || !INTENTS.includes(intent)
    || (audio_source_ref !== undefined && !validReference(audio_source_ref))
    || (video_source_ref !== undefined && !validReference(video_source_ref))
    || !validSdp(offer_sdp, mode as RemoteMediaMode)) throw invalid();
  return {
    v: 1,
    media_session_ref,
    session_id,
    generation,
    mode: mode as RemoteMediaMode,
    intent: intent as RemoteMediaIntent,
    ...(audio_source_ref === undefined ? {} : { audio_source_ref }),
    ...(video_source_ref === undefined ? {} : { video_source_ref }),
    offer_sdp,
  };
}

/** Validate a host control request against the negotiated media session. */
export function parseRemoteMediaControlRequest(
  raw: unknown,
  expected: Pick<RemoteMediaOfferParams, 'media_session_ref' | 'generation'>,
): RemoteMediaControlRequestParams {
  const invalid = () => new RemoteProtocolError('invalid media control request');
  if (!isObject(raw)) throw invalid();
  const { v, media_session_ref, generation, action, target_audio_sequence, target_sample_frames } = raw;
  if (Object.keys(raw).sort().join(',') !== 'action,generation,media_session_ref,target_audio_sequence,target_sample_frames,v'
    || v !== 1 || media_session_ref !== expected.media_session_ref || generation !== expected.generation
    || (action !== 'flush_audio' && action !== 'interrupt_audio' && action !== 'close')
    || !validCounter(target_audio_sequence, REMOTE_MEDIA_AUDIO_SEQUENCE_MAX)
    || !validCounter(target_sample_frames, REMOTE_MEDIA_AUDIO_SAMPLE_FRAMES_MAX)
    || ((target_audio_sequence === 0) !== (target_sample_frames === 0))
    || (action === 'close' && (target_audio_sequence !== 0 || target_sample_frames !== 0))) throw invalid();
  return { v: 1, media_session_ref, generation, action, target_audio_sequence, target_sample_frames };
}

/** Validate a control result and require actual played progress for flush. */
export function parseRemoteMediaControlResult(
  raw: unknown,
  expected: RemoteMediaControlRequestParams,
): RemoteMediaControlResult {
  const invalid = () => new RemoteProtocolError('invalid media control result');
  if (!isObject(raw)) throw invalid();
  const { v, media_session_ref, generation, audio_sequence, accepted_sample_frames, played_sample_frames, rendered_at_unix_ms } = raw;
  const keys = Object.keys(raw).sort().join(',');
  const expectedKeys = ['accepted_sample_frames', 'audio_sequence', 'generation', 'media_session_ref', 'played_sample_frames', 'rendered_at_unix_ms', 'v']
    .filter((key) => raw[key] !== undefined).sort().join(',');
  if (keys !== expectedKeys || v !== 1 || media_session_ref !== expected.media_session_ref
    || generation !== expected.generation || !validProgress(audio_sequence, accepted_sample_frames, played_sample_frames)) throw invalid();
  const audioSequence = audio_sequence as number;
  const acceptedFrames = accepted_sample_frames as number;
  const playedFrames = played_sample_frames as number;
  if ((expected.action === 'flush_audio' && playedFrames < expected.target_sample_frames)
    || (expected.action === 'interrupt_audio' && playedFrames > expected.target_sample_frames)
    || (rendered_at_unix_ms !== undefined && !validCounter(rendered_at_unix_ms, REMOTE_MEDIA_COUNTER_MAX))) throw invalid();
  return {
    v: 1,
    media_session_ref: expected.media_session_ref,
    generation: expected.generation,
    audio_sequence: audioSequence,
    accepted_sample_frames: acceptedFrames,
    played_sample_frames: playedFrames,
    ...(rendered_at_unix_ms === undefined ? {} : { rendered_at_unix_ms: rendered_at_unix_ms as number }),
  };
}

/** Validate an actual playback receipt before session-owner and ordering checks. */
export function parseRemoteMediaPlaybackReceipt(raw: unknown): RemoteMediaPlaybackReceipt {
  const invalid = () => new RemoteProtocolError('invalid media playback receipt');
  if (!isObject(raw)) throw invalid();
  const { v, media_session_ref, generation, audio_sequence, accepted_sample_frames, played_sample_frames, rendered_at_unix_ms } = raw;
  const keys = Object.keys(raw).sort().join(',');
  const expectedKeys = ['accepted_sample_frames', 'audio_sequence', 'generation', 'media_session_ref', 'played_sample_frames', 'rendered_at_unix_ms', 'v']
    .filter((key) => raw[key] !== undefined).sort().join(',');
  if (keys !== expectedKeys || v !== 1 || !validReference(media_session_ref)
    || !validCounter(generation, REMOTE_MEDIA_COUNTER_MAX) || generation === 0
    || !validProgress(audio_sequence, accepted_sample_frames, played_sample_frames)
    || audio_sequence === 0 || played_sample_frames === 0
    || (rendered_at_unix_ms !== undefined && !validCounter(rendered_at_unix_ms, REMOTE_MEDIA_COUNTER_MAX))) throw invalid();
  return {
    v: 1,
    media_session_ref: media_session_ref as string,
    generation: generation as number,
    audio_sequence: audio_sequence as number,
    accepted_sample_frames: accepted_sample_frames as number,
    played_sample_frames: played_sample_frames as number,
    ...(rendered_at_unix_ms === undefined ? {} : { rendered_at_unix_ms: rendered_at_unix_ms as number }),
  };
}

/** Validate an answer and bind it to the exact host-issued session and media mode. */
export function parseRemoteMediaAnswer(raw: unknown, expected: Pick<RemoteMediaOfferParams, 'media_session_ref' | 'mode'>): RemoteMediaAnswerResult {
  const invalid = () => new RemoteProtocolError('invalid media answer');
  if (!isObject(raw) || Object.keys(raw).sort().join(',') !== 'answer_sdp,media_session_ref,v'
    || raw['v'] !== 1 || raw['media_session_ref'] !== expected.media_session_ref
    || !validSdp(raw['answer_sdp'], expected.mode)) throw invalid();
  return {
    v: 1,
    media_session_ref: expected.media_session_ref,
    answer_sdp: raw['answer_sdp'] as string,
  };
}

/** Parse a bridge-server media request without trusting its route or session ids. */
export function parseRemoteMediaBridgeOfferRequest(raw: unknown): RemoteMediaBridgeOfferRequest {
  if (!isObject(raw) || Object.keys(raw).sort().join(',') !== 'offer,request_id,route_id'
    || !validRequestId(raw['request_id']) || !validRouteId(raw['route_id'])) {
    throw new RemoteProtocolError('invalid remote media bridge request');
  }
  return {
    request_id: raw['request_id'],
    route_id: raw['route_id'],
    offer: parseRemoteMediaOffer(raw['offer']),
  };
}

/** Build one bridge response; route and request ids must exactly match the received event. */
export function remoteMediaBridgeSuccess(
  request: Pick<RemoteMediaBridgeOfferRequest, 'request_id' | 'route_id' | 'offer'>,
  raw: unknown,
): RemoteMediaBridgeOfferResponse {
  return {
    request_id: request.request_id,
    route_id: request.route_id,
    result: parseRemoteMediaAnswer(raw, request.offer),
  };
}

/** Convert a safe Remote Control RPC error into the fixed bridge error union. */
export function remoteMediaBridgeFailure(
  request: Pick<RemoteMediaBridgeOfferRequest, 'request_id' | 'route_id'>,
  code: RemoteMediaBridgeErrorCode,
  message: string,
): RemoteMediaBridgeOfferResponse {
  return { request_id: request.request_id, route_id: request.route_id, error: { code, message: message.slice(0, 256) } };
}

/** Parse one correlated bridge control event. */
export function parseRemoteMediaBridgeControlRequest(raw: unknown): RemoteMediaBridgeControlRequest {
  if (!isObject(raw) || Object.keys(raw).sort().join(',') !== 'control,request_id,route_id'
    || !validRequestId(raw['request_id']) || !validRouteId(raw['route_id']) || !isObject(raw['control'])) {
    throw new RemoteProtocolError('invalid remote media control bridge request');
  }
  const control = raw['control'];
  const offer = { media_session_ref: control['media_session_ref'], generation: control['generation'] };
  const parsed = parseRemoteMediaControlRequest(control, offer as Pick<RemoteMediaOfferParams, 'media_session_ref' | 'generation'>);
  return { request_id: raw['request_id'], route_id: raw['route_id'], control: parsed };
}

/** Build a correlated bridge control response after validating actual progress. */
export function remoteMediaBridgeControlSuccess(
  request: RemoteMediaBridgeControlRequest,
  raw: unknown,
): RemoteMediaBridgeControlResponse {
  return {
    request_id: request.request_id,
    route_id: request.route_id,
    result: parseRemoteMediaControlResult(raw, request.control),
  };
}
