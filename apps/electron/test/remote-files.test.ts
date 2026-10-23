import { gatewayFixture, FakeRuntime, SESSION_A, SESSION_B, RecordingChannel, viewer } from './remote-session-helpers.js';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { FileChannel, FILE_CHUNK_BYTES, decodeFileFrame, encodeFileFrame, hashFile, validateFileOffer, type FragmentLink, type FileOffer, type FileSink } from '@lingxi/bridge-client/remote';
import { RemoteFileStore } from '../src/main/remoteControl/fileStore.js';
class Link implements FragmentLink {
  maxFragmentBytes = 16384;
  peer!: Link; handler?: (bytes: Uint8Array) => void; closers: Array<(reason: string) => void> = []; closed = false; max = 0;
  sendFragment(bytes: Uint8Array): void { if (this.closed) throw new Error('closed'); this.max = Math.max(this.max, bytes.length); this.peer.handler?.(bytes.slice()); }
  onFragment(callback: (bytes: Uint8Array) => void): void { this.handler = callback; }
  onClose(callback: (reason: string) => void): void { this.closers.push(callback); }
  async whenWritable(): Promise<void> { if (this.closed) throw new Error('closed'); }
  close(reason: string): void { if (this.closed) return; this.closed = true; for (const callback of this.closers) callback(reason); this.peer.close(reason); }
}
function pair() { const al = new Link(); const bl = new Link(); al.peer = bl; bl.peer = al; return { a: new FileChannel(al), b: new FileChannel(bl), al, bl }; }
const pattern = (size: number) => ({ size, read: async (at: number, n: number) => Uint8Array.from({ length: Math.min(size - at, n) }, (_, i) => (at + i) % 251) });
async function offer(size: number, id = '12'.repeat(16)): Promise<FileOffer> { return { transfer_id: id, session_id: 'session', name: 'arbitrary-文件.zip', size, sha256: await hashFile(pattern(size)) }; }
async function withStore(run: (store: RemoteFileStore, path: string) => Promise<void>): Promise<void> { const dir = await mkdtemp(join(tmpdir(), 'lingxi-files-')); try { await run(new RemoteFileStore(dir), dir); } finally { await rm(dir, { recursive: true, force: true }); } }
const authorize = () => undefined;
test('binary wire supports offsets above 4 GiB and rejects malformed frames/names', () => {
  const value = decodeFileFrame(encodeFileFrame(0, 'ab'.repeat(16), 5 * 1024 ** 3, new Uint8Array([0, 255])));
  assert.equal(value.offset, 5 * 1024 ** 3); assert.deepEqual(value.payload, new Uint8Array([0, 255]));
  assert.throws(() => decodeFileFrame(new Uint8Array(3)));
  assert.throws(() => encodeFileFrame(0, value.id, 0, new Uint8Array(FILE_CHUNK_BYTES + 1)));
  for (const name of ['../x', 'x/y', 'x\\y', '.', '\0']) assert.throws(() => validateFileOffer({ transfer_id: value.id, session_id: 's', size: 0, sha256: '0'.repeat(64), name }));
});
test('17 MiB original file exceeds JSON cap; reverse transfer uses same channel', async () => withStore(async store => {
  const metadata = await offer(17 * 1024 * 1024); const { a, b, al, bl } = pair();
  try {
    const receipt = await a.send(metadata, pattern(metadata.size), {
      offer: async info => { const sink = await store.accept('owner', info, authorize); b.receive(info.transfer_id, sink); return { offset: sink.offset }; }, commit: id => b.commit(id), cancel: id => b.cancel(id),
    });
    assert.equal(receipt.sha256, metadata.sha256); assert(al.max <= 16384); assert(bl.max <= 16384);
    const { source } = await store.source('owner', 'session', metadata.transfer_id); assert.equal(await hashFile(source), metadata.sha256);
    const second = await offer(31, '34'.repeat(16));
    const reverse = await b.send(second, pattern(31), { offer: async info => { const sink = await store.accept('reverse', info, authorize); a.receive(info.transfer_id, sink); return { offset: 0 }; }, commit: id => a.commit(id), cancel: id => a.cancel(id) });
    assert.equal(reverse.size, 31);
  } finally { a.close(); b.close(); }
}));
test('empty file publishes and lost commit response can be replayed', async () => withStore(async store => {
  const info = await offer(0); const sink = await store.accept('owner', info, authorize); const receipt = await sink.finish();
  assert.deepEqual(await store.receipt('owner', 'session', info.transfer_id), receipt);
  const replay = await store.accept('owner', info, authorize); assert.equal(replay.offset, 0); assert.deepEqual(await replay.finish(), receipt);
}));
test('durable resume after store recreation; account and session isolation', async () => withStore(async (store, path) => {
  const info = await offer(FILE_CHUNK_BYTES * 2 + 7); const first = await store.accept('owner1', info, authorize);
  await first.write(0, await pattern(info.size).read(0, FILE_CHUNK_BYTES)); await first.close();
  const recovered = new RemoteFileStore(path); const sink = await recovered.accept('owner1', info, authorize); assert.equal(sink.offset, FILE_CHUNK_BYTES);
  await sink.write(sink.offset, await pattern(info.size).read(sink.offset, info.size - sink.offset)); await sink.finish();
  await assert.rejects(recovered.receipt('owner2', 'session', info.transfer_id)); await assert.rejects(recovered.receipt('owner1', 'other', info.transfer_id)); assert.equal((await recovered.list('owner1', 'session')).length, 1);
}));
test('corruption never publishes; cancellation discards incomplete checkpoint', async () => withStore(async store => {
  const info = await offer(2); const sink = await store.accept('owner', info, authorize); await sink.write(0, new Uint8Array([7, 8])); await assert.rejects(sink.finish(), /checksum/);
  await assert.rejects(store.receipt('owner', 'session', info.transfer_id));
  const retry = await store.accept('owner', info, authorize); assert.equal(retry.offset, 0); await retry.cancel();
  const fresh = await store.accept('owner', info, authorize); assert.equal(fresh.offset, 0); await fresh.close();
}));
test('receiver ACK waits for durable storage', async () => {
  const { a, b, al, bl } = pair(); const info = await offer(FILE_CHUNK_BYTES); let release!: () => void; let offset = 0;
  const persisted = new Promise<void>(resolve => { release = resolve; });
  const sink: FileSink = { get offset() { return offset; }, write: async (_, data) => { await persisted; offset += data.length; }, finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => undefined, cancel: async () => undefined };
  al.handler = () => undefined;
  b.receive(info.transfer_id, sink); al.sendFragment(encodeFileFrame(0, info.transfer_id, 0, await pattern(info.size).read(0, info.size)));
  await new Promise(resolve => setImmediate(resolve)); assert.equal(bl.max, 0); release(); await b.commit(info.transfer_id); assert(bl.max > 0); a.close(); b.close();
});
test('disk failure promptly rejects transfer', async () => {
  const { a, b } = pair(); const info = await offer(100);
  await assert.rejects(a.send(info, pattern(info.size), {
    offer: async () => { b.receive(info.transfer_id, { offset: 0, write: async () => { throw new Error('ENOSPC'); }, finish: async () => { throw new Error('unreachable'); }, close: async () => undefined, cancel: async () => undefined }); return { offset: 0 }; }, commit: id => b.commit(id), cancel: id => b.cancel(id),
  }), /closed/);
});
test('bad sequence and revoked session cannot overwrite/publish', async () => withStore(async store => {
  let allowed = true; const info = await offer(2); const sink = await store.accept('owner', info, () => { if (!allowed) throw new Error('revoked'); });
  await assert.rejects(sink.write(1, new Uint8Array([1])), /invalid/); allowed = false; await assert.rejects(sink.write(0, new Uint8Array([0, 1])), /revoked/); await assert.rejects(sink.finish(), /revoked/); await sink.close();
}));

test('gateway requires approval scope, shared attached session and original-file ownership', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A); f.enable(SESSION_B);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume(); f.runtimes.add(new FakeRuntime(SESSION_B)).resume();
  const { a, b } = pair(); const channel = Object.assign(new RecordingChannel(), { files: b });
  const phone = viewer({ account_uid: 'account' }); const who = { viewer: phone, channel };
  const info = { ...await offer(31), session_id: SESSION_A };
  await assert.rejects(f.call('file.offer', info, who), /attach/);
  await f.call('session.attach', { session_id: SESSION_A }, who);
  const receipt = await a.send(info, pattern(info.size), {
    offer: o => f.call('file.offer', o, who), commit: id => f.call('file.commit', { session_id: SESSION_A, transfer_id: id }, who), cancel: async id => { await f.call('file.cancel', { session_id: SESSION_A, transfer_id: id }, who); },
  });
  assert.equal(receipt.sha256, info.sha256);
  assert.equal((await f.call('file.list', { session_id: SESSION_A }, who)).files.length, 1);
  await f.call('session.attach', { session_id: SESSION_B }, who);
  await assert.rejects(f.call('prompt.send', { session_id: SESSION_B, text: 'file', file_refs: [receipt.ref] }, who));
  const other = { viewer: viewer({ account_uid: 'other-account', conn_id: 'other' }), channel: new RecordingChannel() };
  await f.call('session.attach', { session_id: SESSION_A }, other);
  assert.equal((await f.call('file.list', { session_id: SESSION_A }, other)).files.length, 0);
  await f.call('prompt.send', { session_id: SESSION_A, text: 'Read this file', client_request_id: 'file-prompt-1', file_refs: [receipt.ref] }, who);
  assert.match(f.echoes.at(-1)!.text, /Local path:/);
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  await assert.rejects(f.call('file.list', { session_id: SESSION_A }, who));
  a.close(); b.close(); f.gateway.dispose();
}));

test('negotiated SCTP size below 16 KiB reduces chunk size and keeps the credit window bounded', async () => withStore(async store => {
  const { a, b, al } = pair(); al.maxFragmentBytes = 1024; const info = await offer(35_000);
  await a.send(info, pattern(info.size), {
    offer: async o => { const sink = await store.accept('small-message-peer', o, authorize); b.receive(o.transfer_id, sink); return { offset: 0 }; }, commit: id => b.commit(id), cancel: id => b.cancel(id),
  });
  assert(al.max <= 1024); a.close(); b.close();
}));
test('cancel while waiting for channel backpressure removes the incomplete receiver', async () => withStore(async store => {
  const { a, b, al } = pair(); const info = await offer(40_000); const abort = new AbortController();
  let waiting!: () => void; const blocked = new Promise<void>(resolve => { waiting = resolve; });
  al.whenWritable = () => { waiting(); return new Promise(() => undefined); };
  const transfer = a.send(info, pattern(info.size), {
    offer: async o => { const sink = await store.accept('cancel', o, authorize); b.receive(o.transfer_id, sink); return { offset: 0 }; }, commit: id => b.commit(id), cancel: id => b.cancel(id),
  }, undefined, abort.signal);
  await blocked; abort.abort(); await assert.rejects(transfer, /cancel/);
  const retry = await store.accept('cancel', info, authorize); assert.equal(retry.offset, 0); await retry.close(); a.close(); b.close();
}));

test('mid-window cancellation drains file frames before control cleanup and permits retry', async () => withStore(async store => {
  const { a, b, al, bl } = pair();
  const info = await offer(FILE_CHUNK_BYTES * 3);
  const controller = new AbortController();
  const send = al.sendFragment.bind(al);
  let stopped = false;
  al.sendFragment = bytes => {
    send(bytes);
    if (!stopped && decodeFileFrame(bytes).kind === 0) { stopped = true; controller.abort(); }
  };
  const control = {
    offer: async (metadata: FileOffer) => { const sink = await store.accept('owner', metadata, authorize); b.receive(metadata.transfer_id, sink); return { offset: sink.offset }; },
    commit: (id: string) => b.commit(id), cancel: (id: string) => b.cancel(id),
  };
  try {
    await assert.rejects(a.send(info, pattern(info.size), control, undefined, controller.signal), /cancel|abort/i);
    assert.equal(al.closed, false); assert.equal(bl.closed, false);
    assert.equal((await a.send(info, pattern(info.size), control)).sha256, info.sha256);
  } finally { a.close(); b.close(); }
}));

test('gateway download cancellation drains the file lane and keeps the original receipt', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b, al, bl } = pair();
  const channel = Object.assign(new RecordingChannel(), { files: b });
  const who = { viewer: viewer({ account_uid: 'account' }), channel };
  await f.call('session.attach', { session_id: SESSION_A }, who);
  const info = { ...await offer(FILE_CHUNK_BYTES * 3), session_id: SESSION_A };
  const params = { session_id: SESSION_A, transfer_id: info.transfer_id };
  try {
    await a.send(info, pattern(info.size), {
      offer: o => f.call('file.offer', o, who), commit: () => f.call('file.commit', params, who), cancel: async () => { await f.call('file.cancel', params, who); },
    });
    let received = 0; let durable = 0;
    a.receive(info.transfer_id, {
      get offset() { return durable; },
      write: async (at, bytes) => { assert.equal(at, received); received += bytes.length; },
      checkpoint: async () => { durable = received; },
      finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => undefined, cancel: async () => undefined,
    });
    const send = bl.sendFragment.bind(bl);
    let cancellation: Promise<unknown> | undefined;
    bl.sendFragment = bytes => {
      send(bytes);
      if (!cancellation && decodeFileFrame(bytes).kind === 0) cancellation = f.call('file.cancel', params, who);
    };
    await assert.rejects(f.call('file.download', { ...params, offset: 0 }, who), /abort|cancel/i);
    await cancellation;
    await a.cancel(info.transfer_id);
    assert.equal(al.closed, false); assert.equal(bl.closed, false);
    assert.equal((await f.call('file.list', { session_id: SESSION_A }, who)).files.length, 1);
  } finally { a.close(); b.close(); f.gateway.dispose(); }
}));

test('checksum rejection releases gateway registration so the same transfer can retry', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A); f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b } = pair();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
  await f.call('session.attach', { session_id: SESSION_A }, who);
  const info = { ...await offer(31), session_id: SESSION_A };
  const params = { session_id: SESSION_A, transfer_id: info.transfer_id };
  const control = {
    offer: (o: FileOffer) => f.call('file.offer', o, who), commit: () => f.call('file.commit', params, who), cancel: async () => { await f.call('file.cancel', params, who); },
  };
  const corrupt = { size: info.size, read: async (at: number, length: number) => { const bytes = await pattern(info.size).read(at, length); bytes[0] ^= 1; return bytes; } };
  try {
    await assert.rejects(a.send(info, corrupt, control), /checksum/);
    assert.equal((await a.send(info, pattern(info.size), control)).sha256, info.sha256);
  } finally { a.close(); b.close(); f.gateway.dispose(); }
}));
