import assert from 'node:assert/strict';
import { test } from 'node:test';

import { RemoteMediaRoutes } from '../src/main/remoteControl/mediaRoutes.js';
import { remoteMediaSourceRef } from '@lingxi/bridge-client/remote';
import { RecordingChannel, viewer } from './remote-session-helpers.js';

const OFFER = {
  v: 1,
  media_session_ref: 'media_0123456789abcdef',
  session_id: '12121212-3434-4567-8899-aaaaaaaaaaaa',
  generation: 1,
  mode: 'audio',
  intent: 'conversation',
  audio_source_ref: '',
  offer_sdp: 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n',
};

const CAPABILITIES = { version: 1 as const, modes: ['audio', 'video', 'audio_video'] as const, source_kinds: ['microphone', 'camera'] as const };

test('bridge offer dispatch uses the exact approved route and host-issued session binding', async () => {
  const routes = new RemoteMediaRoutes();
  const channel = new RecordingChannel();
  const approvedViewer = viewer({ account_uid: 'account-1', media_capabilities: CAPABILITIES });
  const routeId = await routes.registerViewer(approvedViewer, channel);
  const offer = { ...OFFER, audio_source_ref: remoteMediaSourceRef(routeId, 'microphone') };
  routes.bindSession(routeId, OFFER.session_id, OFFER.media_session_ref);

  const raw = { request_id: 'AAAAAAAAAAAAAAAAAAAAAA', route_id: routeId, offer };
  const response = await routes.handleOfferEvent(raw);
  assert.deepEqual(channel.requests, [{ method: 'media.offer', params: offer }]);
  assert.deepEqual(response, {
    request_id: raw.request_id,
    route_id: routeId,
    result: { v: 1, media_session_ref: OFFER.media_session_ref, answer_sdp: offer.offer_sdp },
  });

  assert.deepEqual(await routes.handleOfferEvent(raw), response, 'a duplicate bridge delivery reuses the same answer');
  assert.equal(channel.requests.length, 1);

  const wrongOwner = await routes.handleOfferEvent({
    ...raw,
    request_id: 'BBBBBBBBBBBBBBBBBBBBBB',
    offer: { ...offer, session_id: '12121212-3434-4567-8899-bbbbbbbbbbbb' },
  });
  assert.equal('error' in wrongOwner ? wrongOwner.error.code : 'ok', 'forbidden');
  await routes.unregisterViewer(routeId);
  assert.match(routeId, /^[0-9a-f-]{36}$/iu);
  const closed = await routes.handleOfferEvent({ ...raw, request_id: 'CCCCCCCCCCCCCCCCCCCCCC' });
  assert.equal('error' in closed ? closed.error.code : 'ok', 'closed');
});

test('bridge routes are host-issued and requests from another route cannot borrow an answer', async () => {
  const routes = new RemoteMediaRoutes();
  const first = new RecordingChannel();
  const second = new RecordingChannel();
  const firstViewer = viewer({ account_uid: 'account-1', media_capabilities: CAPABILITIES });
  const firstRoute = await routes.registerViewer(firstViewer, first);
  const secondRoute = await routes.registerViewer(viewer({ account_uid: 'account-1', device_id: 'device-2', conn_id: 'conn-2', media_capabilities: CAPABILITIES }), second);
  routes.bindSession(firstRoute, OFFER.session_id, OFFER.media_session_ref);

  const response = await routes.handleOfferEvent({
    request_id: 'DDDDDDDDDDDDDDDDDDDDDD', route_id: secondRoute, offer: { ...OFFER, audio_source_ref: remoteMediaSourceRef(firstRoute, 'microphone') },
  });
  assert.equal('error' in response ? response.error.code : 'ok', 'forbidden');
  assert.equal(first.requests.length + second.requests.length, 0);
});

test('concurrent registration is single-flight and connection ids are scoped to the verified identity', async () => {
  const routes = new RemoteMediaRoutes();
  const firstViewer = viewer({ account_uid: 'account-1', media_capabilities: CAPABILITIES });
  const [first, duplicate] = await Promise.all([
    routes.registerViewer(firstViewer, new RecordingChannel()),
    routes.registerViewer(firstViewer, new RecordingChannel()),
  ]);
  assert.equal(first, duplicate);
  assert.match(first, /^[0-9a-f-]{36}$/iu);

  const otherIdentity = await routes.registerViewer(
    viewer({ account_uid: 'account-2', device_id: firstViewer.device_id, media_capabilities: CAPABILITIES }),
    new RecordingChannel(),
  );
  assert.notEqual(first, otherIdentity);
});

test('only viewers with explicit media handler declarations can register routes', async () => {
  const routes = new RemoteMediaRoutes();
  await assert.rejects(
    routes.registerViewer(viewer({ account_uid: 'account-1', platform: 'web' }), new RecordingChannel()),
    /did not declare supported remote media/u,
  );
});

test('control barriers return actual monotonic playback delta even when receipts arrive first', async () => {
  class BarrierChannel extends RecordingChannel {
    async request(method: 'media.offer' | 'media.control', params: unknown): Promise<unknown> {
      this.requests.push({ method, params });
      if (method === 'media.offer' && typeof params === 'object' && params !== null) {
        const offer = params as { media_session_ref?: unknown; offer_sdp?: unknown };
        return { v: 1, media_session_ref: offer.media_session_ref, answer_sdp: offer.offer_sdp };
      }
      const control = params as { media_session_ref: string; generation: number; target_audio_sequence: number; target_sample_frames: number };
      return {
        v: 1,
        media_session_ref: control.media_session_ref,
        generation: control.generation,
        audio_sequence: control.target_audio_sequence,
        accepted_sample_frames: control.target_sample_frames,
        played_sample_frames: control.target_sample_frames,
        rendered_at_unix_ms: 1_800_000_000_000,
      };
    }
  }

  const routes = new RemoteMediaRoutes();
  const approvedViewer = viewer({ account_uid: 'account-1', media_capabilities: CAPABILITIES });
  const channel = new BarrierChannel();
  const routeId = await routes.registerViewer(approvedViewer, channel);
  const sessionId = OFFER.session_id;
  routes.allowSession(routeId, sessionId);
  const genericOffer = {
    request_id: 'AAAAAAAAAAAAAAAAAAAAAA',
    connection_id: routeId,
    session_id: sessionId,
    media_session_ref: { id: OFFER.media_session_ref },
    operation_id: { id: 'operation-1', generation: 7, service_epoch: 1 },
    owner: { type: 'session', session_id: sessionId },
    mode: 'audio',
    intent: 'conversation',
    audio_source: { id: remoteMediaSourceRef(routeId, 'microphone') },
    video_source: null,
    description: { kind: 'offer', sdp: OFFER.offer_sdp },
  } as const;
  const answer = await routes.handleConnectionRequest(genericOffer);
  assert.equal(answer.description?.kind, 'answer');
  assert.equal(answer.error, undefined);

  assert.equal(routes.recordPlaybackReceipt(approvedViewer, {
    v: 1, media_session_ref: OFFER.media_session_ref, generation: 7,
    audio_sequence: 1, accepted_sample_frames: 960, played_sample_frames: 960,
  }), true);
  const first = await routes.handleControlEvent({
    request_id: 'BBBBBBBBBBBBBBBBBBBBBB', connection_id: routeId,
    media_session_ref: { id: OFFER.media_session_ref }, generation: 7,
    action: { type: 'flush_audio', target_sequence: 2, target_sample_frames: 1920 },
  });
  assert.equal(first.receipt?.accepted_sample_frames, 0);
  assert.equal(first.receipt?.played_sample_frames, 1920, 'receipt notification must not be subtracted from the first barrier');

  const second = await routes.handleControlEvent({
    request_id: 'CCCCCCCCCCCCCCCCCCCCCC', connection_id: routeId,
    media_session_ref: { id: OFFER.media_session_ref }, generation: 7,
    action: { type: 'flush_audio', target_sequence: 3, target_sample_frames: 2880 },
  });
  assert.equal(second.receipt?.played_sample_frames, 960, 'subsequent barriers report only newly confirmed playout');
});

test('interrupt acknowledges while an older flush is pending and suppresses the stale flush receipt', async () => {
  let markFlushStarted!: () => void;
  let resolveFlush!: (value: unknown) => void;
  const flushStarted = new Promise<void>(resolve => { markFlushStarted = resolve; });
  const delayedFlush = new Promise<unknown>(resolve => { resolveFlush = resolve; });

  class BargeInChannel extends RecordingChannel {
    async request(method: 'media.offer' | 'media.control', params: unknown): Promise<unknown> {
      this.requests.push({ method, params });
      if (method === 'media.offer') {
        const offer = params as { media_session_ref?: unknown; offer_sdp?: unknown };
        return { v: 1, media_session_ref: offer.media_session_ref, answer_sdp: offer.offer_sdp };
      }
      const control = params as {
        media_session_ref: string;
        generation: number;
        action: string;
        target_audio_sequence: number;
        target_sample_frames: number;
      };
      if (control.action === 'flush_audio') {
        markFlushStarted();
        return delayedFlush;
      }
      return {
        v: 1,
        media_session_ref: control.media_session_ref,
        generation: control.generation,
        audio_sequence: control.target_audio_sequence,
        accepted_sample_frames: control.target_sample_frames,
        played_sample_frames: 480,
        rendered_at_unix_ms: 1_800_000_000_000,
      };
    }
  }

  const routes = new RemoteMediaRoutes();
  const approvedViewer = viewer({ account_uid: 'account-1', media_capabilities: CAPABILITIES });
  const channel = new BargeInChannel();
  const routeId = await routes.registerViewer(approvedViewer, channel);
  const sessionId = OFFER.session_id;
  routes.allowSession(routeId, sessionId);
  const genericOffer = {
    request_id: 'AAAAAAAAAAAAAAAAAAAAAA',
    connection_id: routeId,
    session_id: sessionId,
    media_session_ref: { id: OFFER.media_session_ref },
    operation_id: { id: 'operation-1', generation: 7, service_epoch: 1 },
    owner: { type: 'session', session_id: sessionId },
    mode: 'audio',
    intent: 'conversation',
    audio_source: { id: remoteMediaSourceRef(routeId, 'microphone') },
    video_source: null,
    description: { kind: 'offer', sdp: OFFER.offer_sdp },
  } as const;
  await routes.handleConnectionRequest(genericOffer);

  const pendingFlush = routes.handleControlEvent({
    request_id: 'BBBBBBBBBBBBBBBBBBBBBB', connection_id: routeId,
    media_session_ref: { id: OFFER.media_session_ref }, generation: 7,
    action: { type: 'flush_audio', target_sequence: 2, target_sample_frames: 1920 },
  });
  await flushStarted;
  const interrupt = await routes.handleControlEvent({
    request_id: 'CCCCCCCCCCCCCCCCCCCCCC', connection_id: routeId,
    media_session_ref: { id: OFFER.media_session_ref }, generation: 7,
    action: { type: 'interrupt_audio', target_sequence: 2, target_sample_frames: 1920 },
  });
  assert.equal(interrupt.receipt?.played_sample_frames, 480, 'interrupt returns native-confirmed progress promptly');
  assert.equal(interrupt.error, undefined);

  resolveFlush({
    v: 1,
    media_session_ref: OFFER.media_session_ref,
    generation: 7,
    audio_sequence: 2,
    accepted_sample_frames: 1920,
    played_sample_frames: 1920,
    rendered_at_unix_ms: 1_800_000_000_001,
  });
  const stale = await pendingFlush;
  assert.equal(stale.receipt, undefined, 'superseded flush cannot report completed playback');
  assert.equal(stale.error?.kind, 'cancelled');
});
