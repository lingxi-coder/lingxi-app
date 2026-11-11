import assert from 'node:assert/strict';
import { mock, test } from 'node:test';

import { RemoteProtocolError } from '../src/remote/errors.js';
import { encodeMessage } from '../src/remote/framing.js';
import { RemoteRpcError, RpcPeer } from '../src/remote/rpc.js';
import { MessageTransport, type FragmentLink, type RemoteTransport } from '../src/remote/transport.js';
import { MESSAGE_MAX_BYTES } from '../src/remote/version.js';
import { settle } from './remote-helpers.js';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

/** No network or automatic progress: a test explicitly opens or rejects the link's gate. */
class ControlledLink implements FragmentLink {
  readonly sent: Uint8Array[] = [];
  readonly writable = deferred<void>();
  blocked = true;
  maxFragmentBytes = 16_384;
  closedWith: string | undefined;
  writeFailure: Error | undefined;
  private readonly closeHandlers: Array<(reason: string) => void> = [];

  sendFragment(fragment: Uint8Array): void {
    if (this.closedWith !== undefined) throw new Error('link closed');
    if (this.writeFailure !== undefined) throw this.writeFailure;
    this.sent.push(fragment.slice());
  }
  onFragment(): void {}
  onClose(callback: (reason: string) => void): void { this.closeHandlers.push(callback); }
  close(reason: string): void {
    if (this.closedWith !== undefined) return;
    this.closedWith = reason;
    for (const callback of this.closeHandlers.splice(0)) callback(reason);
  }
  whenWritable(): Promise<void> { return this.blocked ? this.writable.promise : Promise.resolve(); }
  open(): void { this.blocked = false; this.writable.resolve(); }
}

class ControlledTransport implements RemoteTransport {
  readonly sent: string[] = [];
  readonly writes: ReturnType<typeof deferred<void>>[] = [];
  blocked = false;
  closedWith: string | undefined;
  private messageHandler: ((text: string) => void) | undefined;
  private readonly closeHandlers: Array<(reason: string) => void> = [];

  send(text: string): Promise<void> {
    this.sent.push(text);
    if (!this.blocked) return Promise.resolve();
    const write = deferred<void>();
    this.writes.push(write);
    return write.promise;
  }
  onMessage(callback: (text: string) => void): void { this.messageHandler = callback; }
  onClose(callback: (reason: string) => void): void { this.closeHandlers.push(callback); }
  close(reason: string): void {
    if (this.closedWith !== undefined) return;
    this.closedWith = reason;
    for (const callback of this.closeHandlers.splice(0)) callback(reason);
  }
  receive(value: unknown): void { this.messageHandler?.(JSON.stringify(value)); }
}

test('outbound message count includes the blocked active send and closes before any fragment allocation', async (t) => {
  const link = new ControlledLink();
  const transport = new MessageTransport(link, { maxOutboundQueueMessages: 2 });
  const encoding = mock.method(TextEncoder.prototype, 'encode');
  t.after(() => encoding.mock.restore());
  const first = assert.rejects(transport.send('first'), /closed: capacity/);
  const second = assert.rejects(transport.send('second'), /closed: capacity/);
  const overflow = assert.rejects(transport.send('third'), /closed: capacity/);
  await Promise.all([first, second, overflow]);
  assert.equal(link.closedWith, 'capacity');
  assert.equal(link.sent.length, 0);
  assert.equal(encoding.mock.callCount(), 0, 'neither queued nor active messages were encoded while blocked');
  // The underlying gate deliberately never resolves: close must cancel its wait.
});

test('outbound byte admission counts UTF-8, unpaired surrogates, fragment headers and empty messages exactly', async (t) => {
  const text = 'Aé灵😀\ud800X\udc00';
  const link = new ControlledLink();
  link.maxFragmentBytes = 7;
  const wireBytes = encodeMessage(text, 7).reduce((sum, fragment) => sum + fragment.length, 0);
  const transport = new MessageTransport(link, { maxOutboundQueueBytes: wireBytes + 1 });
  const encoding = mock.method(TextEncoder.prototype, 'encode');
  t.after(() => encoding.mock.restore());
  const first = assert.rejects(transport.send(text), /closed: capacity/);
  const empty = assert.rejects(transport.send(''), /closed: capacity/);
  assert.equal(link.closedWith, undefined, 'the exact aggregate wire-byte limit is admitted');
  const overflow = assert.rejects(transport.send(''), /closed: capacity/);
  await Promise.all([first, empty, overflow]);
  assert.equal(encoding.mock.callCount(), 0, 'byte admission did not allocate encoded payloads');
});

test('admitted sends encode only as the link progresses, preserving bytes and message order', async (t) => {
  const link = new ControlledLink();
  link.maxFragmentBytes = 7;
  const messages = ['Aé灵😀\ud800X\udc00', '', 'next'];
  const expected = messages.flatMap(text => encodeMessage(text, 7));
  const transport = new MessageTransport(link);
  const encoding = mock.method(TextEncoder.prototype, 'encode');
  t.after(() => encoding.mock.restore());
  const sends = messages.map(text => transport.send(text));
  assert.equal(encoding.mock.callCount(), 0);
  link.open();
  await Promise.all(sends);
  assert.deepEqual(link.sent, expected);
  assert.equal(encoding.mock.callCount(), messages.length);
});

test('completed sends release both outbound budgets for the next message', async () => {
  const link = new ControlledLink();
  link.open();
  const transport = new MessageTransport(link, { maxOutboundQueueBytes: 5, maxOutboundQueueMessages: 1 });
  await transport.send('four');
  await transport.send('next');
  assert.equal(link.closedWith, undefined);
  assert.deepEqual(link.sent, [...encodeMessage('four'), ...encodeMessage('next')]);
});

test('close rejects active and queued sends without resolving a permanently blocked link', async (t) => {
  const link = new ControlledLink();
  const transport = new MessageTransport(link);
  const encoding = mock.method(TextEncoder.prototype, 'encode');
  t.after(() => encoding.mock.restore());
  const outcomes = [
    assert.rejects(transport.send('active'), /closed: gone/),
    assert.rejects(transport.send('queued'), /closed: gone/),
  ];
  link.close('gone');
  await Promise.all(outcomes);
  assert.equal(encoding.mock.callCount(), 0);
  await assert.rejects(transport.send('later'), /closed: gone/);
});

test('outbound reservations release after a writability failure, including an undefined rejection', async () => {
  const link = new ControlledLink();
  const transport = new MessageTransport(link, { maxOutboundQueueBytes: 5, maxOutboundQueueMessages: 1 });
  const outcome = transport.send('four').then(() => assert.fail('the send must reject'), error => assert.equal(error, undefined));
  link.writable.reject(undefined);
  await outcome;
  link.open();
  await transport.send('next');
  assert.equal(link.closedWith, undefined);
});

test('outbound reservations release after encoding and link-write failures', async (t) => {
  const link = new ControlledLink();
  link.open();
  const transport = new MessageTransport(link, { maxOutboundQueueBytes: 5, maxOutboundQueueMessages: 1 });
  const encoding = mock.method(TextEncoder.prototype, 'encode', () => { throw new Error('encode failed'); });
  t.after(() => encoding.mock.restore());
  await assert.rejects(transport.send('four'), /encode failed/);
  encoding.mock.restore();
  link.writeFailure = new Error('write failed');
  await assert.rejects(transport.send('four'), /write failed/);
  link.writeFailure = undefined;
  await transport.send('next');
  assert.equal(link.closedWith, undefined);
});

test('a message above the wire limit is rejected before encoding and does not reserve the queue', async (t) => {
  const link = new ControlledLink();
  link.open();
  const transport = new MessageTransport(link, { maxOutboundQueueBytes: 5, maxOutboundQueueMessages: 1 });
  const encoding = mock.method(TextEncoder.prototype, 'encode');
  t.after(() => encoding.mock.restore());
  await assert.rejects(transport.send('x'.repeat(MESSAGE_MAX_BYTES + 1)), RemoteProtocolError);
  assert.equal(encoding.mock.callCount(), 0);
  await transport.send('next');
  assert.equal(link.closedWith, undefined);
});

test('invalid outbound budgets and request budgets are rejected at construction', () => {
  for (const limit of [0, -1, 0.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.throws(() => new MessageTransport(new ControlledLink(), { maxOutboundQueueBytes: limit }), RangeError);
    assert.throws(() => new MessageTransport(new ControlledLink(), { maxOutboundQueueMessages: limit }), RangeError);
    assert.throws(() => new RpcPeer(new ControlledTransport(), { maxConcurrentRequests: limit }), RangeError);
    assert.throws(() => new RpcPeer(new ControlledTransport(), { maxControlRequests: limit }), RangeError);
  }
});

test('RPC admission bounds concurrent handlers before invoking excess work', async () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 2 });
  const pending = deferred<unknown>();
  const handled: string[] = [];
  peer.onRequest(method => { handled.push(method); return pending.promise; });
  transport.receive({ id: '1', method: 'session.history' });
  transport.receive({ id: '2', method: 'session.history' });
  transport.receive({ id: '3', method: 'session.history' });
  assert.deepEqual(handled, ['session.history', 'session.history']);
  assert.equal(peer.closed, true);
  assert.equal(transport.closedWith, 'capacity');
  pending.resolve('late');
  await settle();
  assert.deepEqual(transport.sent, []);
});

test('RPC request reservations include blocked response sends and release after the send completes', async () => {
  const transport = new ControlledTransport();
  transport.blocked = true;
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
  let handled = 0;
  peer.onRequest(() => ++handled);
  transport.receive({ id: '1', method: 'session.history' });
  await settle();
  assert.equal(transport.writes.length, 1);
  transport.writes[0]!.resolve();
  await settle();
  transport.receive({ id: '2', method: 'session.history' });
  await settle();
  assert.equal(handled, 2, 'the first completed send released its request slot');
  transport.receive({ id: '3', method: 'session.history' });
  assert.equal(handled, 2, 'a returned handler result still occupies its slot while its reply is blocked');
  assert.equal(transport.closedWith, 'capacity');
  transport.writes[1]!.resolve();
});

test('RPC reservations release after handler errors and failed response sends', async () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
  let handled = 0;
  peer.onRequest(() => { handled++; throw new Error('handler failed'); });
  transport.receive({ id: '1', method: 'session.history' });
  await settle();
  transport.blocked = true;
  transport.receive({ id: '2', method: 'session.history' });
  await settle();
  transport.writes[0]!.reject(new Error('send failed'));
  await settle();
  transport.blocked = false;
  transport.receive({ id: '3', method: 'session.history' });
  await settle();
  assert.equal(handled, 3);
  assert.equal(peer.closed, false);
});

test('existing cancellation, detach and approval methods can use bounded reserved RPC slots', async () => {
  for (const control of ['turn.cancel', 'session.detach', 'permission.respond', 'question.answer', 'question.cancel']) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1, maxControlRequests: 1 });
    const pending = deferred<unknown>();
    const handled: string[] = [];
    peer.onRequest(method => { handled.push(method); return method === 'session.history' ? pending.promise : {}; });
    transport.receive({ id: 'history', method: 'session.history' });
    transport.receive({ id: 'control', method: control });
    await settle();
    assert.deepEqual(handled, ['session.history', control]);
    assert.equal(peer.closed, false);
    assert.deepEqual(JSON.parse(transport.sent[0]!), { id: 'control', result: {} });
    // The completed control response frees the reserved slot for another cancellation.
    transport.receive({ id: 'again', method: control });
    await settle();
    assert.equal(handled.length, 3);
    assert.equal(peer.closed, false);
    peer.close();
    pending.resolve(undefined);
  }
});

test('reserved RPC control capacity is finite and unknown methods do not bypass admission', () => {
  for (const overflow of ['turn.cancel', 'unrecognized.cancel']) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1, maxControlRequests: 1 });
    let handled = 0;
    peer.onRequest(() => { handled++; return new Promise(() => undefined); });
    transport.receive({ id: 'history', method: 'session.history' });
    transport.receive({ id: 'control', method: 'turn.cancel' });
    transport.receive({ id: 'overflow', method: overflow });
    assert.equal(handled, 2);
    assert.equal(transport.closedWith, 'capacity');
  }
});

test('RPC skips result and error serialization when a handler finishes after peer close', async () => {
  for (const failed of [false, true]) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
    const pending = deferred<unknown>();
    peer.onRequest(() => pending.promise);
    transport.receive({ id: '1', method: 'session.history' });
    peer.close('gone');
    let serialized = 0;
    if (failed) {
      const error = new RemoteRpcError('internal');
      Object.defineProperty(error, 'message', { get() { serialized++; throw new Error('must not inspect'); } });
      pending.reject(error);
    } else pending.resolve({ toJSON() { serialized++; throw new Error('must not serialize'); } });
    await settle();
    assert.equal(serialized, 0);
    assert.deepEqual(transport.sent, []);
  }
});

test('requests and async notifications share the default 32-slot work budget in either wire form', async () => {
  for (const overflowIsNotification of [false, true]) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport);
    const pending = deferred<unknown>();
    let handled = 0;
    const handler = () => { handled++; return pending.promise; };
    peer.onRequest(handler);
    peer.onNotification(handler);
    transport.receive({ id: 'request', method: 'session.history' });
    for (let i = 0; i < 31; i++) transport.receive({ method: 'session.history' });
    assert.equal(handled, 32);
    assert.equal(peer.closed, false);
    transport.receive({ ...(overflowIsNotification ? {} : { id: 'overflow' }), method: 'session.history' });
    assert.equal(handled, 32, 'the overload never invokes another application handler');
    assert.equal(transport.closedWith, 'capacity');
    pending.resolve({ toJSON() { assert.fail('closed work must not be serialized'); } });
    await settle();
    assert.deepEqual(transport.sent, [], 'notifications and late closed requests produce no replies');
  }
});

test('synchronous notification results and throws release their work slot inline for event bursts', () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
  let handled = 0;
  peer.onNotification(() => {
    handled++;
    if (handled % 7 === 0) throw new Error('event callback failed');
    return handled; // Existing void callbacks may return Array.push/boolean results.
  });
  for (let i = 0; i < 1_000; i++) transport.receive({ method: 'session.event' });
  assert.equal(handled, 1_000);
  assert.equal(peer.closed, false);
  assert.deepEqual(transport.sent, [], 'no notification gets a response');
});

test('async notification resolution and rejection(undefined) release shared slots without replies', async () => {
  for (const failed of [false, true]) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
    const pending = deferred<unknown>();
    let handled = 0;
    peer.onNotification(() => { handled++; return pending.promise; });
    peer.onRequest(() => { handled++; return 'next'; });
    transport.receive({ method: 'session.history' });
    if (failed) pending.reject(undefined);
    else pending.resolve('ignored');
    await settle();
    assert.deepEqual(transport.sent, []);
    transport.receive({ id: 'request', method: 'session.history' });
    await settle();
    assert.equal(handled, 2);
    assert.equal(peer.closed, false);
    assert.deepEqual(transport.sent.map(text => JSON.parse(text)), [{ id: 'request', result: 'next' }]);
  }
});

test('mixed notification/request work shares finite control reserves without invoking excess work', async () => {
  for (const control of ['turn.cancel', 'session.detach', 'permission.respond', 'question.answer', 'question.cancel']) {
    for (const controlIsNotification of [false, true]) {
      const transport = new ControlledTransport();
      const peer = new RpcPeer(transport, { maxConcurrentRequests: 1, maxControlRequests: 1 });
      const pending = deferred<unknown>();
      const handled: string[] = [];
      const handler = (method: string) => { handled.push(method); return pending.promise; };
      peer.onRequest(handler);
      peer.onNotification(handler);
      transport.receive({ method: 'session.history' });
      transport.receive({ ...(controlIsNotification ? {} : { id: 'control' }), method: control });
      assert.deepEqual(handled, ['session.history', control]);
      assert.equal(peer.closed, false);
      transport.receive({ method: 'unrecognized.cancel' });
      assert.deepEqual(handled, ['session.history', control]);
      assert.equal(transport.closedWith, 'capacity');
      pending.reject(undefined);
      await settle();
      assert.deepEqual(transport.sent, []);
    }
  }
});

test('unknown cancel-like requests and notifications cannot consume unused control reserves', async () => {
  for (const notification of [false, true]) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1, maxControlRequests: 1 });
    const pending = deferred<unknown>();
    let handled = 0;
    const handler = () => { handled++; return pending.promise; };
    peer.onRequest(handler);
    peer.onNotification(handler);
    transport.receive({ method: 'session.history' });
    transport.receive({ ...(notification ? {} : { id: 'unknown' }), method: 'unrecognized.cancel' });
    assert.equal(handled, 1);
    assert.equal(transport.closedWith, 'capacity');
    pending.resolve(undefined);
    await settle();
  }
});

test('synchronous control notifications release reserved slots inline while ordinary request work is blocked', async () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1, maxControlRequests: 1 });
  const pending = deferred<unknown>();
  peer.onRequest(() => pending.promise);
  let handled = 0;
  peer.onNotification(() => { handled++; if (handled % 3 === 0) throw new Error('control callback failed'); });
  transport.receive({ id: 'history', method: 'session.history' });
  for (let i = 0; i < 100; i++) transport.receive({ method: 'turn.cancel' });
  assert.equal(handled, 100);
  assert.equal(peer.closed, false);
  assert.deepEqual(transport.sent, []);
  peer.close('gone');
  pending.resolve(undefined);
  await settle();
});

test('closing async notification work ignores late resolve/reject and later messages without replies', async () => {
  for (const failed of [false, true]) {
    const transport = new ControlledTransport();
    const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
    const pending = deferred<unknown>();
    let handled = 0;
    peer.onNotification(() => { handled++; return pending.promise; });
    transport.receive({ method: 'session.history' });
    peer.close('gone');
    if (failed) pending.reject(undefined);
    else pending.resolve({ toJSON() { assert.fail('notification results are never serialized'); } });
    transport.receive({ method: 'session.history' });
    await settle();
    assert.equal(handled, 1);
    assert.deepEqual(transport.sent, []);
    assert.equal(transport.closedWith, 'gone');
  }
});

test('notifications without a registered handler remain ignored while request capacity is full', async () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
  const pending = deferred<unknown>();
  peer.onRequest(() => pending.promise);
  transport.receive({ id: 'history', method: 'session.history' });
  for (let i = 0; i < 100; i++) transport.receive({ method: 'unregistered.event' });
  assert.equal(peer.closed, false);
  peer.close();
  pending.resolve(undefined);
  await settle();
});

test('a notification that closes its peer still has its returned rejection observed', async () => {
  const transport = new ControlledTransport();
  const peer = new RpcPeer(transport, { maxConcurrentRequests: 1 });
  peer.onNotification(() => {
    peer.close('gone');
    return Promise.reject(undefined);
  });
  transport.receive({ method: 'session.history' });
  await settle();
  assert.equal(transport.closedWith, 'gone');
  assert.deepEqual(transport.sent, []);
});
