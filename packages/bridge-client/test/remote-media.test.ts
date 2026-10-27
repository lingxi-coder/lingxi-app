import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  RemoteProtocolError,
  parseRemoteMediaAnswer,
  parseRemoteMediaBridgeOfferRequest,
  parseRemoteMediaControlRequest,
  parseRemoteMediaOffer,
  remoteMediaBridgeFailure,
  remoteMediaBridgeSuccess,
  remoteMediaSourceRef,
  isRemoteMediaSourceForRoute,
  type RemoteMediaMode,
} from '../src/remote.js';

function sdp(mode: RemoteMediaMode): string {
  const tracks = mode === 'audio' ? 'm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n'
    : mode === 'video' ? 'm=video 9 UDP/TLS/RTP/SAVPF 96\r\n'
      : 'm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n';
  return `v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n${tracks}`;
}

function offer(mode: RemoteMediaMode = 'audio_video') {
  return {
    v: 1,
    media_session_ref: 'session_opaque_0123456789',
    session_id: '12121212-3434-4567-8899-aaaaaaaaaaaa',
    generation: 1,
    mode,
    intent: 'conversation',
    offer_sdp: sdp(mode),
  };
}

test('media offer parser accepts each bounded mode and keeps host-issued refs opaque', () => {
  for (const mode of ['audio', 'video', 'audio_video'] as const) {
    const parsed = parseRemoteMediaOffer(offer(mode));
    assert.equal(parsed.mode, mode);
    assert.equal(parsed.media_session_ref, 'session_opaque_0123456789');
  }
});

test('media offer parser rejects extra owner fields, unsafe refs, and track/mode mismatch', () => {
  assert.throws(() => parseRemoteMediaOffer({ ...offer(), account_uid: 'attacker' }), RemoteProtocolError);
  assert.throws(() => parseRemoteMediaOffer({ ...offer(), media_session_ref: '../other' }), RemoteProtocolError);
  assert.throws(() => parseRemoteMediaOffer({ ...offer('audio'), offer_sdp: sdp('audio_video') }), RemoteProtocolError);
});

test('media answer is bounded and bound to the exact host-issued session ref', () => {
  const expected = parseRemoteMediaOffer(offer());
  assert.equal(parseRemoteMediaAnswer({ v: 1, media_session_ref: expected.media_session_ref, answer_sdp: sdp('audio_video') }, expected).v, 1);
  assert.throws(() => parseRemoteMediaAnswer({ v: 1, media_session_ref: 'other', answer_sdp: sdp('audio_video') }, expected), RemoteProtocolError);
  assert.throws(() => parseRemoteMediaAnswer({ v: 1, media_session_ref: expected.media_session_ref, answer_sdp: `${sdp('audio_video')}${'a=x\r\n'.repeat(20000)}` }, expected), RemoteProtocolError);
});

test('bridge offer correlation binds the answer and failure to the registered route', () => {
  const request = parseRemoteMediaBridgeOfferRequest({
    request_id: 'AAAAAAAAAAAAAAAAAAAAAA',
    route_id: 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff',
    offer: offer('audio'),
  });
  const answer = remoteMediaBridgeSuccess(request, {
    v: 1,
    media_session_ref: request.offer.media_session_ref,
    answer_sdp: sdp('audio'),
  });
  assert.deepEqual([answer.request_id, answer.route_id], [request.request_id, request.route_id]);
  const failure = remoteMediaBridgeFailure(request, 'closed', 'connection ended');
  assert.deepEqual([failure.request_id, failure.route_id], [request.request_id, request.route_id]);
  assert.throws(() => parseRemoteMediaBridgeOfferRequest({
    request_id: 'request', route_id: 'BBBBBBBBBBBBBBBBBBBBBB', offer: request.offer,
  }), RemoteProtocolError);
});

test('remote media close is a strict zero-target action for the exact generation', () => {
  const expected = parseRemoteMediaOffer(offer('audio'));
  assert.deepEqual(parseRemoteMediaControlRequest({
    v: 1,
    media_session_ref: expected.media_session_ref,
    generation: expected.generation,
    action: 'close',
    target_audio_sequence: 0,
    target_sample_frames: 0,
  }, expected), {
    v: 1,
    media_session_ref: expected.media_session_ref,
    generation: expected.generation,
    action: 'close',
    target_audio_sequence: 0,
    target_sample_frames: 0,
  });
  assert.throws(() => parseRemoteMediaControlRequest({
    v: 1,
    media_session_ref: expected.media_session_ref,
    generation: expected.generation,
    action: 'close',
    target_audio_sequence: 1,
    target_sample_frames: 960,
  }, expected), RemoteProtocolError);
});

test('remote source references are exact to a UUID route and kind', () => {
  const route = 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff';
  const microphone = remoteMediaSourceRef(route, 'microphone');
  const camera = remoteMediaSourceRef(route, 'camera');
  assert.equal(microphone, `remote_${route}_microphone`);
  assert.equal(isRemoteMediaSourceForRoute(route, microphone, 'microphone'), true);
  assert.equal(isRemoteMediaSourceForRoute(route, camera, 'microphone'), false);
  assert.equal(isRemoteMediaSourceForRoute('aaaaaaaa-cccc-4ddd-8eee-ffffffffffff', microphone), false);
  assert.throws(() => remoteMediaSourceRef('not-a-route', 'microphone'), RemoteProtocolError);
});
