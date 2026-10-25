import assert from 'node:assert/strict';
import vm from 'node:vm';
import { test } from 'node:test';
import { FRAGMENT_MAX_BYTES, ManualClock, SDP_MAX_CHARS } from '@lingxi/bridge-client/remote';

import { PeerHost, type RtcPort, type RtcWindow } from '../src/main/remoteControl/peerHost.js';
import {
  MAX_RTC_PEERS,
  isRtcConnId,
  parseMainToPage,
  parsePageToMain,
  rtcBytes,
  type MainToPage,
  type RtcCandidateType,
  type RtcPeerState,
} from '../src/shared/remoteRtc.js';
import { START, diagnosticsLog, settle } from './remote-helpers.js';

const conn = (n: number): string => String(n).padStart(22, 'A');
const A = conn(1);
const B = conn(2);

class FakePort implements RtcPort {
  readonly posted: MainToPage[] = [];
  private handler: ((data: unknown) => void) | undefined;
  post(message: MainToPage): void { this.posted.push(message); }
  onMessage(handler: (data: unknown) => void): void { this.handler = handler; }
  /** What the page says. */
  say(data: unknown): void { this.handler!(data); }
  of(type: MainToPage['t']): MainToPage[] { return this.posted.filter((message) => message.t === type); }
}

class FakeWindow implements RtcWindow {
  readonly port = new FakePort();
  opened = 0;
  closed = 0;
  failOpen: Error | undefined;
  private goneHandler: ((reason: string) => void) | undefined;
  async open(): Promise<RtcPort> {
    this.opened++;
    if (this.failOpen) throw this.failOpen;
    return this.port;
  }
  onGone(handler: (reason: string) => void): void { this.goneHandler = handler; }
  close(): void { this.closed++; }
  crash(reason = 'crashed'): void { this.goneHandler!(reason); }
}

function setup() {
  const clock = new ManualClock(START);
  const log = diagnosticsLog();
  const windows: FakeWindow[] = [];
  const host = new PeerHost({ createWindow: () => { const window = new FakeWindow(); windows.push(window); return window; }, clock, diagnostics: log.diagnostics });
  return { clock, log, host, windows };
}

/** Ask for an answer and let the page give it. */
async function answered(t: ReturnType<typeof setup>, id = A): Promise<string> {
  const pending = t.host.answer(id, 'v=0 offer', ['stun:s.example']);
  await settle();
  t.windows.at(-1)!.port.say({ t: 'answered', conn: id, sdp: `v=0 answer ${id}` });
  return pending;
}

test('the window opens on the first offer, and the page is asked for the answer', async () => {
  const t = setup();
  assert.equal(t.host.windowOpen, false);
  const pending = t.host.answer(A, 'v=0 offer', ['stun:s.example:3478']);
  await settle();
  assert.equal(t.windows.length, 1);
  assert.deepEqual(t.windows[0]!.port.posted, [{ t: 'answer', conn: A, sdp: 'v=0 offer', ice: ['stun:s.example:3478'] }]);
  t.windows[0]!.port.say({ t: 'answered', conn: A, sdp: 'v=0 answer' });
  assert.equal(await pending, 'v=0 answer');
  assert.equal(t.host.connectionCount, 1);
  assert.equal(t.host.windowOpen, true);

  // The second connection reuses the window.
  await answered(t, B);
  assert.equal(t.windows.length, 1);
  assert.equal(t.host.connectionCount, 2);
});

test('bad requests are refused before anything happens', async () => {
  const t = setup();
  await assert.rejects(t.host.answer('short', 'v=0', []), /invalid/);
  await assert.rejects(t.host.answer(A, '', []), /invalid/);
  await assert.rejects(t.host.answer(A, 'x'.repeat(SDP_MAX_CHARS + 1), []), /invalid/);
  await assert.rejects(t.host.answer(A, 'v=0', ['turn:t.example']), /invalid/);
  await assert.rejects(t.host.answer(A, 'v=0', Array.from({ length: 9 }, () => 'stun:s.example')), /invalid/);
  assert.equal(t.windows.length, 0, 'nothing reached the page');
});

test('a connection id can have one peer, and there are at most eight', async () => {
  const t = setup();
  void t.host.answer(A, 'v=0', []).catch(() => undefined);
  await assert.rejects(t.host.answer(A, 'v=0', []), /already has a peer/);
  for (let i = 2; i <= MAX_RTC_PEERS; i++) void t.host.answer(conn(i), 'v=0', []).catch(() => undefined);
  assert.equal(t.host.connectionCount, MAX_RTC_PEERS);
  await assert.rejects(t.host.answer(conn(99), 'v=0', []), /too many/);
  t.host.dispose();
});

test('an answer that does not come in ten seconds fails, and the connection is released', async () => {
  const t = setup();
  const pending = t.host.answer(A, 'v=0', []);
  const settled = assert.rejects(pending, /timed out/);
  await settle();
  t.clock.advance(10_000);
  await settled;
  assert.equal(t.host.connectionCount, 0);
  assert.throws(() => t.host.link(A), /unknown/);
});

test('the page reporting an error fails the answer and closes the link', async () => {
  const t = setup();
  const pending = t.host.answer(A, 'v=0', []);
  const link = t.host.link(A);
  const closed: string[] = [];
  link.onClose((reason) => closed.push(reason));
  const settled = assert.rejects(pending, /WebRTC page failed: bad sdp/);
  await settle();
  t.windows[0]!.port.say({ t: 'error', conn: A, message: 'bad sdp' });
  await settled;
  assert.deepEqual(closed, ['answer_failed']);
  assert.equal(t.host.connectionCount, 0);
});

test('a window that cannot be opened fails the answer and is closed', async () => {
  const t = setup();
  const original = t.host;
  const failing = new PeerHost({
    createWindow: () => { const window = new FakeWindow(); window.failOpen = new Error('no display'); t.windows.push(window); return window; },
    clock: t.clock,
    diagnostics: t.log.diagnostics,
  });
  await assert.rejects(failing.answer(A, 'v=0', []), /no display/);
  assert.equal(failing.windowOpen, false);
  assert.equal(t.windows[0]!.closed, 1);
  assert.equal(failing.connectionCount, 0);
  assert.ok(t.log.messages('warn').some((message) => /could not be opened/.test(message)));
  assert.equal(original.windowOpen, false);
});

test('the link reports the DataChannel opening, with how the two ends met', async () => {
  const t = setup();
  await answered(t);
  const link = t.host.link(A);
  const states: Array<[RtcPeerState, RtcCandidateType | undefined]> = [];
  link.onState((state, candidate) => states.push([state, candidate]));
  assert.equal(link.connected, false);
  t.windows[0]!.port.say({ t: 'state', conn: A, state: 'connected', candidate: 'srflx' });
  assert.equal(link.connected, true);
  assert.equal(link.candidate, 'srflx');
  assert.deepEqual(states, [['connected', 'srflx']]);

  // A late subscriber hears the current state.
  const late: RtcPeerState[] = [];
  link.onState((state) => late.push(state));
  await settle();
  assert.deepEqual(late, ['connected']);
});

test('a failed or closed connection closes the link with that reason, without echoing it to the page', async () => {
  const t = setup();
  await answered(t);
  const link = t.host.link(A);
  const closed: string[] = [];
  link.onClose((reason) => closed.push(reason));
  t.windows[0]!.port.say({ t: 'state', conn: A, state: 'failed' });
  assert.deepEqual(closed, ['failed']);
  assert.equal(t.windows[0]!.port.of('close').length, 0);
  assert.equal(t.host.connectionCount, 0);
  assert.throws(() => link.sendFragment(new Uint8Array(1)), /closed/);
});

test('fragments from the page reach the link as they are', async () => {
  const t = setup();
  await answered(t);
  const received: number[][] = [];
  t.host.link(A).onFragment((fragment) => received.push([...fragment]));
  t.windows[0]!.port.say({ t: 'data', conn: A, data: new Uint8Array([1, 2, 3]).buffer });
  t.windows[0]!.port.say({ t: 'data', conn: A, data: new Uint8Array([4]) });
  assert.deepEqual(received, [[1, 2, 3], [4]]);
  // Fragments for a connection that does not exist are dropped quietly.
  t.windows[0]!.port.say({ t: 'data', conn: B, data: new Uint8Array([9]).buffer });
  assert.equal(received.length, 2);
});

test('whatever the page sends that is not a valid message is dropped and counted', async () => {
  const t = setup();
  await answered(t);
  const received: unknown[] = [];
  t.host.link(A).onFragment((fragment) => received.push(fragment));
  const port = t.windows[0]!.port;
  const bad: unknown[] = [
    null, 'text', 42, [], {},
    { t: 'data' },
    { t: 'data', conn: 'short', data: new ArrayBuffer(1) },
    { t: 'data', conn: A, data: new ArrayBuffer(0) },
    { t: 'data', conn: A, data: new ArrayBuffer(FRAGMENT_MAX_BYTES + 1) },
    { t: 'data', conn: A, data: 'not bytes' },
    { t: 'state', conn: A, state: 'exploded' },
    { t: 'state', conn: A, state: 'connected', candidate: 'teleport' },
    { t: 'answered', conn: A, sdp: '' },
    { t: 'answered', conn: A, sdp: 'x'.repeat(SDP_MAX_CHARS + 1) },
    { t: 'error', conn: A, message: 5 },
    { t: 'unknown', conn: A },
  ];
  for (const message of bad) port.say(message);
  assert.equal(t.host.discardedMessages, bad.length);
  assert.deepEqual(received, []);
  assert.equal(t.host.link(A).connected, false);
});

test('sending copies the fragment and posts it to the page', async () => {
  const t = setup();
  await answered(t);
  const link = t.host.link(A);
  const fragment = new Uint8Array([7, 8, 9]);
  link.sendFragment(fragment);
  await settle();
  fragment.fill(0);
  const sent = t.windows[0]!.port.of('send') as Array<Extract<MainToPage, { t: 'send' }>>;
  assert.equal(sent.length, 1);
  assert.equal(sent[0]!.conn, A);
  assert.deepEqual([...new Uint8Array(sent[0]!.data)], [7, 8, 9], 'a later change to the caller\'s buffer does not show');
  assert.throws(() => link.sendFragment(new Uint8Array(0)), RangeError);
  assert.throws(() => link.sendFragment(new Uint8Array(FRAGMENT_MAX_BYTES + 1)), RangeError);
  assert.equal(link.maxFragmentBytes, FRAGMENT_MAX_BYTES);
});

test('a page that is buffering holds the sender back until it drains', async () => {
  const t = setup();
  await answered(t);
  const link = t.host.link(A);
  const port = t.windows[0]!.port;

  await link.whenWritable!();
  port.say({ t: 'buffered', conn: A });
  let released = false;
  const waiting = link.whenWritable!().then(() => { released = true; });
  await settle();
  assert.equal(released, false);
  port.say({ t: 'drain', conn: A });
  await waiting;
  assert.equal(released, true);

  // Closing also frees a sender that waits.
  port.say({ t: 'buffered', conn: A });
  let freed = false;
  const stuck = link.whenWritable!().then(() => { freed = true; });
  await settle();
  assert.equal(freed, false);
  link.close('test');
  await stuck;
  assert.equal(freed, true);
});

test('closing a link tells the page, once', async () => {
  const t = setup();
  await answered(t);
  const link = t.host.link(A);
  const reasons: string[] = [];
  link.onClose((reason) => reasons.push(reason));
  link.close('done');
  link.close('again');
  await settle();
  assert.deepEqual(reasons, ['done']);
  assert.deepEqual(t.windows[0]!.port.of('close'), [{ t: 'close', conn: A }]);
  // A handler added afterwards still hears it.
  const later: string[] = [];
  link.onClose((reason) => later.push(reason));
  await settle();
  assert.deepEqual(later, ['done']);
});

test('when the window dies every connection ends and the next offer gets a new window', async () => {
  const t = setup();
  await answered(t);
  const pending = t.host.answer(B, 'v=0', []);
  const failed = assert.rejects(pending, /window ended/);
  const closed: string[] = [];
  t.host.link(A).onClose((reason) => closed.push(reason));
  await settle();
  t.windows[0]!.crash('renderer crashed');
  await failed;
  assert.deepEqual(closed, ['window_gone']);
  assert.equal(t.host.connectionCount, 0);
  assert.equal(t.host.windowOpen, false);
  assert.ok(t.log.messages('warn').some((message) => /renderer crashed/.test(message)));

  await answered(t, B);
  assert.equal(t.windows.length, 2);
  // The old window cannot speak any more.
  t.windows[0]!.port.say({ t: 'data', conn: B, data: new ArrayBuffer(1) });
  assert.equal(t.host.discardedMessages, 0);
});

test('the window closes five minutes after the last connection, unless another one comes first', async () => {
  const t = setup();
  await answered(t);
  t.host.link(A).close('done');
  t.clock.advance(5 * 60_000 - 1);
  assert.equal(t.windows[0]!.closed, 0);
  await answered(t, B);
  t.clock.advance(10 * 60_000);
  assert.equal(t.windows[0]!.closed, 0, 'a connection is open');

  t.host.link(B).close('done');
  t.clock.advance(5 * 60_000);
  assert.equal(t.windows[0]!.closed, 1);
  assert.equal(t.host.windowOpen, false);
});

test('shutdown ends everything and the host can be used again; dispose is final', async () => {
  const t = setup();
  await answered(t);
  const closed: string[] = [];
  t.host.link(A).onClose((reason) => closed.push(reason));
  t.host.shutdown();
  assert.deepEqual(closed, ['host_shutdown']);
  assert.equal(t.windows[0]!.closed, 1);
  assert.equal(t.host.windowOpen, false);

  await answered(t, B);
  assert.equal(t.windows.length, 2);
  t.host.dispose();
  assert.equal(t.windows[1]!.closed, 1);
  await assert.rejects(t.host.answer(A, 'v=0', []), /shut down/);
});

// ── The messages themselves ──────────────────────────────────────────────────

test('connection ids are exactly 22 characters of base64url', () => {
  assert.equal(isRtcConnId(A), true);
  assert.equal(isRtcConnId('AAAAAAAAAAAAAAAAAAAAA'), false);
  assert.equal(isRtcConnId('AAAAAAAAAAAAAAAAAAAAA+'), false);
  assert.equal(isRtcConnId(5), false);
});

test('rtcBytes gives an exact ArrayBuffer for buffers, views and buffers of another realm', () => {
  const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
  assert.equal(rtcBytes(buffer), buffer);
  const view = new Uint8Array(buffer, 1, 2);
  assert.deepEqual([...new Uint8Array(rtcBytes(view)!)], [2, 3]);
  assert.equal(rtcBytes(view)!.byteLength, 2);
  const node = Buffer.from([5, 6, 7]);
  assert.deepEqual([...new Uint8Array(rtcBytes(node)!)], [5, 6, 7]);
  const foreign = vm.runInNewContext('new Uint8Array([9, 8]).buffer') as ArrayBuffer;
  assert.ok(!(foreign instanceof ArrayBuffer));
  assert.deepEqual([...new Uint8Array(rtcBytes(foreign)!)], [9, 8]);
  assert.equal(rtcBytes('text'), undefined);
  assert.equal(rtcBytes(null), undefined);
});

test('messages to the page: shapes, sizes and the STUN list are checked', () => {
  assert.deepEqual(parseMainToPage({ t: 'close', conn: A }), { t: 'close', conn: A });
  assert.deepEqual(parseMainToPage({ t: 'answer', conn: A, sdp: 'v=0', ice: ['stun:a.example', 'stuns:b.example:5349'] }), { t: 'answer', conn: A, sdp: 'v=0', ice: ['stun:a.example', 'stuns:b.example:5349'] });
  assert.equal(parseMainToPage({ t: 'answer', conn: A, sdp: 'v=0', ice: ['turn:a.example'] }), undefined);
  assert.equal(parseMainToPage({ t: 'answer', conn: A, sdp: 'v=0', ice: 'stun:a.example' }), undefined);
  assert.equal(parseMainToPage({ t: 'answer', conn: A, sdp: 'v=0' }), undefined);
  assert.equal(parseMainToPage({ t: 'send', conn: A, data: new ArrayBuffer(0) }), undefined);
  assert.equal(parseMainToPage({ t: 'send', conn: A, data: new ArrayBuffer(FRAGMENT_MAX_BYTES) })?.t, 'send');
  assert.equal(parseMainToPage({ t: 'send', conn: A, data: new ArrayBuffer(FRAGMENT_MAX_BYTES + 1) }), undefined);
  assert.equal(parseMainToPage({ t: 'close' }), undefined);
  assert.equal(parseMainToPage('close'), undefined);
});

test('messages from the page: errors are shortened, states and candidates are from a fixed list', () => {
  const long = parsePageToMain({ t: 'error', conn: A, message: 'x'.repeat(1000) });
  assert.equal(long?.t === 'error' && long.message.length, 300);
  assert.deepEqual(parsePageToMain({ t: 'state', conn: A, state: 'connected' }), { t: 'state', conn: A, state: 'connected' });
  assert.deepEqual(parsePageToMain({ t: 'state', conn: A, state: 'connected', candidate: 'relay' }), { t: 'state', conn: A, state: 'connected', candidate: 'relay' });
  assert.deepEqual(parsePageToMain({ t: 'buffered', conn: A }), { t: 'buffered', conn: A });
  assert.deepEqual(parsePageToMain({ t: 'drain', conn: A }), { t: 'drain', conn: A });
  assert.equal(parsePageToMain({ t: 'drain', conn: 'x' }), undefined);
});
