import { gatewayFixture, FakeRuntime, SESSION_A, SESSION_B, RecordingChannel, viewer } from './remote-session-helpers.js';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
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
async function withStore(run: (store: RemoteFileStore, path: string) => Promise<void>): Promise<void> { const dir = await mkdtemp(join(tmpdir(), 'lingxi-files-')); try { await run(new RemoteFileStore(dir), dir); } finally { await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 20 }); } }
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
    const opened = await store.source('owner', 'session', metadata.transfer_id);
    try { assert.equal(await hashFile(opened.source), metadata.sha256); } finally { await opened.close(); }
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
test('explicit prompt creates an independent named working copy without trusting the private store', async () => withStore(async (store, root) => {
  const workspace = await mkdtemp(join(root, 'workspace-'));
  const info = await offer(31); const sink = await store.accept('owner', info, authorize);
  const bytes = await pattern(31).read(0, 31); await sink.write(0, bytes); await sink.finish();
  const ref = await store.promptReference('owner', 'session', info.transfer_id, workspace, authorize);
  const path = JSON.parse(ref.text.split('Local path: ')[1]!);
  assert.equal(dirname(dirname(path)), await realpath(workspace));
  assert.equal(path.split('/').at(-1), info.name);
  assert.deepEqual(new Uint8Array(await readFile(path)), bytes);
  assert.equal((await stat(dirname(path))).mode & 0o777, 0o700);
  assert.equal((await stat(path)).mode & 0o777, 0o600);
  await writeFile(path, 'model edit');
  const original = await store.source('owner', 'session', info.transfer_id);
  try { assert.equal(await hashFile(original.source), info.sha256, 'working copy cannot mutate the verified transfer'); }
  finally { await original.close(); }
  await ref.discard(); assert.deepEqual(await readdir(workspace), []);
}));
test('download source pins a regular file across concurrent reads and reopens a replacement', async () => withStore(async (store, root) => {
  const info = await offer(FILE_CHUNK_BYTES * 2 + 7);
  const sink = await store.accept('owner', info, authorize);
  const bytes = await pattern(info.size).read(0, info.size);
  await sink.write(0, bytes); await sink.finish();
  const [scope] = await readdir(root);
  const path = join(root, scope!, info.transfer_id, 'data');
  const opened = await store.source('owner', 'session', info.transfer_id);
  try {
    await rename(path, `${path}.original`);
    await writeFile(path, new Uint8Array(info.size).fill(0x7b));
    const ranges = [[0, FILE_CHUNK_BYTES], [FILE_CHUNK_BYTES, FILE_CHUNK_BYTES], [FILE_CHUNK_BYTES * 2, 7]] as const;
    const chunks = await Promise.all(ranges.map(([offset, length]) => opened.source.read(offset, length)));
    for (let i = 0; i < ranges.length; i++) {
      const [offset, length] = ranges[i]!;
      assert.deepEqual(chunks[i], bytes.subarray(offset, offset + length));
    }
  } finally { await opened.close(); }
  await opened.close(); // Idempotent close is safe for cleanup paths.
  await assert.rejects(opened.source.read(0, 1));
  const replacement = await store.source('owner', 'session', info.transfer_id);
  try { assert.deepEqual(await replacement.source.read(0, 1), Uint8Array.of(0x7b)); }
  finally { await replacement.close(); }
}));
test('download source rejects a symlink at the receipt path', { skip: process.platform === 'win32' ? 'Creating symlinks requires extra Windows privileges' : false }, async () => withStore(async (store, root) => {
  const info = await offer(2); const sink = await store.accept('owner', info, authorize);
  await sink.write(0, Uint8Array.of(0, 1)); await sink.finish();
  const [scope] = await readdir(root);
  const path = join(root, scope!, info.transfer_id, 'data');
  await rename(path, `${path}.original`); await symlink(`${path}.original`, path);
  await assert.rejects(store.source('owner', 'session', info.transfer_id), /invalid source file/);
  assert.deepEqual(new Uint8Array(await readFile(`${path}.original`)), Uint8Array.of(0, 1));
}));
test('revocation after receipt lookup never leaves an unpublished workspace attachment', async () => withStore(async (store, root) => {
  const workspace = await mkdtemp(join(root, 'workspace-')); const info = await offer(0);
  await (await store.accept('owner', info, authorize)).finish();
  let calls = 0;
  await assert.rejects(store.promptReference('owner', 'session', info.transfer_id, workspace, () => {
    if (++calls === 5) throw new Error('revoked');
  }), /revoked/);
  assert.deepEqual(await readdir(workspace), []);
}));
test('discard never deletes a replacement workspace directory', async () => withStore(async (store, root) => {
  const workspace = await mkdtemp(join(root, 'workspace-')); const info = await offer(0);
  await (await store.accept('owner', info, authorize)).finish();
  const ref = await store.promptReference('owner', 'session', info.transfer_id, workspace, authorize);
  const path = dirname(JSON.parse(ref.text.split('Local path: ')[1]!));
  await rename(path, `${path}-moved`); await mkdir(path); await writeFile(join(path, 'user-data'), 'preserve');
  await ref.discard(); assert.equal(await readFile(join(path, 'user-data'), 'utf8'), 'preserve');
}));
test('an empty attachment list cannot send after its viewer closes before preparation', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A); f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: new RecordingChannel() };
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const pending = f.call('prompt.send', { session_id: SESSION_A, text: 'do not send', client_request_id: 'closed-empty-files', file_refs: [] }, who);
    f.gateway.viewerClosed(who.viewer);
    await assert.rejects(pending, /attachment|attach|closed/);
    assert.equal(f.echoes.length, 0);
  } finally { f.gateway.dispose(); }
}));
for (const failure of ['second reference', 'prompt refused', 'attachment replaced', 'workspace changed']) {
  test(`unpublished working copies are removed when ${failure}`, async () => withStore(async (store, root) => {
    const workspace = await mkdtemp(join(root, 'workspace-'));
    const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
    const runtime = new FakeRuntime(SESSION_A); runtime.engineWorkspacePath = workspace;
    f.runtimes.add(runtime).resume();
    const who = { viewer: viewer({ account_uid: 'account' }), channel: new RecordingChannel() };
    const info = { ...await offer(0), session_id: SESSION_A };
    await (await store.accept('account\0device-1', info, authorize)).finish();
    try {
      await f.call('session.attach', { session_id: SESSION_A }, who);
      if (failure === 'prompt refused') runtime.failSend = new Error('prompt refused');
      if (failure === 'attachment replaced' || failure === 'workspace changed') {
        const original = store.promptReference.bind(store);
        store.promptReference = async (...args) => {
          const ref = await original(...args);
          if (failure === 'workspace changed') runtime.engineWorkspacePath = root;
          else {
            await f.call('session.detach', { session_id: SESSION_A }, who);
            await f.call('session.attach', { session_id: SESSION_A }, who);
          }
          return ref;
        };
      }
      await assert.rejects(f.call('prompt.send', { session_id: SESSION_A, text: 'Read file', client_request_id: 'copy-failure',
        file_refs: failure === 'second reference' ? [info.transfer_id, 'ab'.repeat(16)] : [info.transfer_id] }, who));
      assert.deepEqual(await readdir(workspace), []);
      assert.equal(f.echoes.length, 0);
    } finally { f.gateway.dispose(); }
  }));
}
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
  b.receive(info.transfer_id, sink); al.sendFragment(encodeFileFrame(2, info.transfer_id, 0, await pattern(info.size).read(0, info.size)));
  await new Promise(resolve => setImmediate(resolve)); assert.equal(bl.max, 0); release(); await b.commit(info.transfer_id); assert(bl.max > 0); a.close(); b.close();
});
test('ordinary file frames do not acknowledge an uncommitted offset', async () => {
  const { a, b, al, bl } = pair(); const info = await offer(2); let written = 0; let durable = 0;
  const replies: Array<{ kind: number; offset: number }> = [];
  al.handler = bytes => { const { kind, offset } = decodeFileFrame(bytes); replies.push({ kind, offset }); };
  b.receive(info.transfer_id, {
    get offset() { return durable; },
    write: async (at, bytes) => { assert.equal(at, written); written += bytes.length; },
    checkpoint: async () => { durable = written; },
    finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => {}, cancel: async () => {},
  });
  al.sendFragment(encodeFileFrame(0, info.transfer_id, 0, new Uint8Array([1])));
  await new Promise(resolve => setImmediate(resolve)); assert.deepEqual(replies, []);
  al.sendFragment(encodeFileFrame(2, info.transfer_id, 1, new Uint8Array([2])));
  await new Promise(resolve => setImmediate(resolve)); assert.deepEqual(replies, [{ kind: 1, offset: 2 }]);
  assert.equal(bl.closed, false); a.close(); b.close();
});
test('one transfer pipelines four windows but does not exceed the receiver bound', { timeout: 4000 }, async () => {
  const { a, b, al } = pair(); const info = await offer(FILE_CHUNK_BYTES * 16 * 5);
  let unblock!: () => void; let full!: () => void; let sent = 0; let written = 0; let durable = 0;
  const disk = new Promise<void>(resolve => { unblock = resolve; });
  const saturated = new Promise<void>(resolve => { full = resolve; });
  const send = al.sendFragment.bind(al);
  al.sendFragment = bytes => { send(bytes); if ((bytes[0] === 0 || bytes[0] === 2) && ++sent === 64) full(); };
  const transfer = a.send(info, pattern(info.size), {
    offer: async () => {
      b.receive(info.transfer_id, {
        get offset() { return durable; },
        write: async (at, bytes) => { assert.equal(at, written); written += bytes.length; },
        checkpoint: async () => { await disk; durable = written; },
        finish: async () => { assert.equal(durable, info.size); return { ...info, ref: info.transfer_id }; },
        close: async () => {}, cancel: async () => {},
      });
      return { offset: 0 };
    }, commit: id => b.commit(id), cancel: id => b.cancel(id),
  });
  try {
    await saturated; await new Promise(resolve => setImmediate(resolve));
    assert.equal(sent, 64, 'the fifth window must wait for a durable ACK');
  } finally { unblock(); }
  assert.equal((await transfer).size, info.size); assert.equal(sent, 80); a.close(); b.close();
});
test('cancel a four-window transfer while another waits for credit, then retry both', { timeout: 5000 }, async () => {
  const { a, b, al } = pair(); const first = await offer(FILE_CHUNK_BYTES * 16 * 5, '11'.repeat(16));
  const second = await offer(100, '22'.repeat(16)); const firstAbort = new AbortController();
  let unblock!: () => void; let full!: () => void; let sent = 0; let barrier = false; let removed = 0;
  const disk = new Promise<void>(resolve => { unblock = resolve; });
  const saturated = new Promise<void>(resolve => { full = resolve; });
  const send = al.sendFragment.bind(al);
  al.sendFragment = bytes => {
    send(bytes);
    if (bytes[0] === 3) barrier = true;
    if ((bytes[0] === 0 || bytes[0] === 2) && ++sent === 64) full();
  };
  const control = {
    offer: async (info: FileOffer) => {
      let written = 0; let durable = 0;
      b.receive(info.transfer_id, {
        get offset() { return durable; }, write: async (at, bytes) => { assert.equal(at, written); written += bytes.length; },
        checkpoint: async () => { await disk; durable = written; },
        finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => {}, cancel: async () => { removed++; },
      });
      return { offset: 0 };
    }, commit: (id: string) => b.commit(id), cancel: (id: string) => b.cancel(id),
  };
  const large = a.send(first, pattern(first.size), control, undefined, firstAbort.signal);
  try {
    await saturated;
    const small = a.send(second, pattern(second.size), control);
    firstAbort.abort();
    await new Promise(resolve => setImmediate(resolve));
    assert(barrier, 'all in-flight windows must be drained before cancellation');
    assert.equal(al.closed, false);
    unblock();
    await assert.rejects(large, /cancel|abort/i);
    assert.equal((await small).size, second.size);
    assert.equal((await a.send(first, pattern(first.size), control)).size, first.size);
    assert.equal(removed, 1);
  } finally { unblock(); a.close(); b.close(); }
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

test('gateway requires approval scope, shared attached session and original-file ownership', async () => withStore(async (store, path) => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A); f.enable(SESSION_B);
  const runtime = new FakeRuntime(SESSION_A); runtime.engineWorkspacePath = path;
  f.runtimes.add(runtime).resume(); f.runtimes.add(new FakeRuntime(SESSION_B)).resume();
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
  await f.call('prompt.send', { session_id: SESSION_A, text: 'Read this file', client_request_id: 'file-prompt-1', file_refs: [receipt.ref] }, who);
  assert.equal((await readdir(path)).filter(name => name.startsWith('.lingxi-remote-attachment-')).length, 1, 'replayed prompts do not publish extra working copies');
  f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
  await assert.rejects(f.call('file.list', { session_id: SESSION_A }, who));
  a.close(); b.close(); f.gateway.dispose();
}));

for (const reason of ['file lane revoked', 'attachment replaced']) {
  test(`an accepted file sink cannot survive ${reason}`, async () => withStore(async store => {
    const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
    f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
    const { a, b } = pair(); let allowed = true;
    const channel = Object.defineProperty(new RecordingChannel(), 'files', { get: () => allowed ? b : undefined });
    const who = { viewer: viewer({ account_uid: 'account' }), channel };
    let sink!: FileSink; const accept = store.accept.bind(store);
    store.accept = async (...args) => { sink = await accept(...args); return sink; };
    try {
      await f.call('session.attach', { session_id: SESSION_A }, who);
      await f.call('file.offer', { ...await offer(2), session_id: SESSION_A }, who);
      await sink.write(0, Uint8Array.of(0));
      if (reason === 'file lane revoked') allowed = false;
      else {
        await f.call('session.detach', { session_id: SESSION_A }, who);
        await f.call('session.attach', { session_id: SESSION_A }, who);
      }
      await assert.rejects(sink.write(1, Uint8Array.of(1)), /closed|authorization|attachment/);
      await assert.rejects(sink.finish(), /closed|authorization|attachment/);
    } finally { await sink?.close(); a.close(); b.close(); f.gateway.dispose(); }
  }));
}

test('an in-flight file listing cannot survive withdrawal and reattachment', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: new RecordingChannel() };
  let release!: () => void;
  store.list = () => new Promise(resolve => { release = () => resolve([]); });
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const pending = f.call('file.list', { session_id: SESSION_A }, who);
    await f.call('session.detach', { session_id: SESSION_A }, who);
    await f.call('session.attach', { session_id: SESSION_A }, who);
    release();
    await assert.rejects(pending, /closed|authorization|attachment/);
  } finally { f.gateway.dispose(); }
}));

test('withdrawal during a disk read cannot send its late bytes under a replacement attachment', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b, bl } = pair();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
  let release!: (bytes: Uint8Array) => void;
  let started!: () => void; const reading = new Promise<void>(resolve => { started = resolve; });
  const info = { ...await offer(2), session_id: SESSION_A };
  let closed = 0;
  store.source = async () => ({ offer: { ...info, ref: info.transfer_id }, source: { size: 2, read: () => { started(); return new Promise(resolve => { release = resolve; }); } }, close: async () => { closed++; } });
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const pending = f.call('file.download', { session_id: SESSION_A, transfer_id: info.transfer_id, offset: 0 }, who);
    await reading;
    await f.call('session.detach', { session_id: SESSION_A }, who);
    await f.call('session.attach', { session_id: SESSION_A }, who);
    release(Uint8Array.of(0, 1));
    await assert.rejects(pending, /attachment|abort|cancel/i);
    assert.equal(closed, 1);
    assert.equal(bl.max, 0, 'no file bytes leave after the attachment was withdrawn');
  } finally { a.close(); b.close(); f.gateway.dispose(); }
}));

for (const outcome of ['complete', 'read failure', 'cancel', 'peer close', 'detach', 'viewer close', 'dispose', 'withdraw while opening'] as const) {
  test(`gateway releases its download source on ${outcome}`, async () => withStore(async store => {
    const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
    f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
    const { a, b } = pair();
    const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
    const info = { ...await offer(2), session_id: SESSION_A };
    const params = { session_id: SESSION_A, transfer_id: info.transfer_id };
    const sink = await store.accept('account\0device-1', info, authorize);
    await sink.write(0, Uint8Array.of(0, 1)); await sink.finish();
    let released!: () => void; const held = new Promise<void>(resolve => { released = resolve; });
    let entered!: () => void; const started = new Promise<void>(resolve => { entered = resolve; });
    let closes = 0;
    const openSource = store.source.bind(store);
    store.source = async (...args) => {
      const opened = await openSource(...args);
      if (outcome === 'withdraw while opening') { entered(); await held; }
      return { ...opened, source: { size: opened.source.size, read: async (at, length) => {
        entered();
        if (outcome === 'read failure') throw new Error('injected read failure');
        if (outcome !== 'complete') await held;
        return opened.source.read(at, length);
      } }, close: async () => { closes++; await opened.close(); } };
    };
    let received = 0;
    a.receive(info.transfer_id, { get offset() { return received; },
      write: async (at, bytes) => { assert.equal(at, received); received += bytes.length; },
      finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => undefined, cancel: async () => undefined });
    try {
      await f.call('session.attach', { session_id: SESSION_A }, who);
      const result = f.call('file.download', { ...params, offset: 0 }, who).then(
        value => ({ ok: true as const, value }), error => ({ ok: false as const, error }),
      );
      await started;
      if (outcome === 'cancel') await f.call('file.cancel', params, who);
      if (outcome === 'peer close') a.close();
      if (outcome === 'detach' || outcome === 'withdraw while opening') await f.call('session.detach', { session_id: SESSION_A }, who);
      if (outcome === 'viewer close') f.gateway.viewerClosed(who.viewer);
      if (outcome === 'dispose') f.gateway.dispose();
      if (outcome === 'withdraw while opening') released();
      const settled = await result;
      assert.equal(settled.ok, outcome === 'complete');
      assert.equal(closes, 1, 'the acquired source is closed exactly once');
      if (outcome === 'complete') assert.equal(received, info.size);
    } finally { released(); a.close(); b.close(); f.gateway.dispose(); }
  }));
}

test('downloads reserve at most four source handles and can reopen an id after cancellation', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b } = pair();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
  const infos = await Promise.all(Array.from({ length: 5 }, (_, i) => offer(2, String(i + 1).repeat(32)).then(info => ({ ...info, session_id: SESSION_A }))));
  let unblock!: () => void; const held = new Promise<void>(resolve => { unblock = resolve; });
  let signalFour!: () => void; const fourStarted = new Promise<void>(resolve => { signalFour = resolve; });
  let started = 0, opened = 0, closed = 0, block = true;
  store.source = async (_owner, _session, id) => {
    const info = infos.find(item => item.transfer_id === id)!; opened++;
    return { offer: { ...info, ref: id }, source: { size: info.size, read: async () => {
      if (block) { if (++started === 4) signalFour(); await held; }
      return Uint8Array.of(0, 1);
    } }, close: async () => { closed++; } };
  };
  const params = (i: number) => ({ session_id: SESSION_A, transfer_id: infos[i]!.transfer_id });
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const downloads = infos.slice(0, 4).map((_, i) => f.call('file.download', { ...params(i), offset: 0 }, who).then(
      () => 'resolved', () => 'rejected',
    ));
    await fourStarted;
    await assert.rejects(f.call('file.download', { ...params(4), offset: 0 }, who), /busy/);
    await assert.rejects(f.call('file.download', { ...params(0), offset: 0 }, who), /busy/);
    assert.equal(opened, 4);
    await Promise.all(infos.slice(0, 4).map((_, i) => f.call('file.cancel', params(i), who)));
    assert.deepEqual(await Promise.all(downloads), ['rejected', 'rejected', 'rejected', 'rejected']);
    assert.equal(closed, 4);
    block = false; unblock();
    let received = 0;
    a.receive(infos[0]!.transfer_id, { get offset() { return received; }, write: async (_, bytes) => { received += bytes.length; },
      finish: async () => ({ ...infos[0]!, ref: infos[0]!.transfer_id }), close: async () => undefined, cancel: async () => undefined });
    await f.call('file.download', { ...params(0), offset: 0 }, who);
    assert.equal(received, 2); assert.equal(opened, 5); assert.equal(closed, 5);
  } finally { unblock(); a.close(); b.close(); f.gateway.dispose(); }
}));

for (const method of ['file.commit', 'prompt.send']) {
  test(`a delayed ${method} file lookup cannot revive under a new attachment`, async () => withStore(async store => {
    const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
    f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
    const { a, b } = pair();
    const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
    const info = { ...await offer(0), session_id: SESSION_A };
    let release!: () => void;
    if (method === 'file.commit') store.receipt = () => new Promise(resolve => { release = () => resolve({ ...info, ref: info.transfer_id }); });
    else store.promptReference = () => new Promise(resolve => { release = () => resolve({ text: 'Local path: test', discard: async () => undefined }); });
    try {
      await f.call('session.attach', { session_id: SESSION_A }, who);
      const pending = f.call(method, { session_id: SESSION_A, transfer_id: info.transfer_id, file_refs: [info.transfer_id], text: 'Read file', client_request_id: 'stale-file-prompt' }, who);
      while (!release) await new Promise(resolve => setImmediate(resolve));
      await f.call('session.detach', { session_id: SESSION_A }, who);
      await f.call('session.attach', { session_id: SESSION_A }, who);
      release();
      await assert.rejects(pending, /attachment|attach/);
      assert.equal(f.echoes.length, 0);
    } finally { a.close(); b.close(); f.gateway.dispose(); }
  }));
}

for (const withdrawal of ['detach', 'unshare']) {
test(`${withdrawal} releases an idle upload so reattachment can resume the same id`, async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b } = pair();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
  const info = { ...await offer(2), session_id: SESSION_A };
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    await f.call('file.offer', info, who);
    if (withdrawal === 'detach') await f.call('session.detach', { session_id: SESSION_A }, who);
    else {
      f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
      f.enable(SESSION_A);
    }
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const receipt = await a.send(info, pattern(info.size), {
      offer: offer => f.call('file.offer', offer, who),
      commit: transfer_id => f.call('file.commit', { session_id: SESSION_A, transfer_id }, who),
      cancel: transfer_id => f.call('file.cancel', { session_id: SESSION_A, transfer_id }, who),
    });
    assert.equal(receipt.sha256, info.sha256);
  } finally { a.close(); b.close(); f.gateway.dispose(); }
}));
}

test('queued old offers cannot acquire a replacement attachment', async () => withStore(async store => {
  const f = gatewayFixture({ files: store }); f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  const { a, b } = pair();
  const who = { viewer: viewer({ account_uid: 'account' }), channel: Object.assign(new RecordingChannel(), { files: b }) };
  const info = { ...await offer(2), session_id: SESSION_A };
  let release!: () => void;
  const accept = store.accept.bind(store); let calls = 0;
  store.accept = async (...args) => { calls++; await new Promise<void>(resolve => { release = resolve; }); return accept(...args); };
  try {
    await f.call('session.attach', { session_id: SESSION_A }, who);
    const first = f.call('file.offer', info, who);
    const second = f.call('file.offer', info, who);
    const results = Promise.allSettled([first, second]);
    while (!release) await new Promise(resolve => setImmediate(resolve));
    await f.call('session.detach', { session_id: SESSION_A }, who);
    await f.call('session.attach', { session_id: SESSION_A }, who);
    release();
    assert.deepEqual((await results).map(item => item.status), ['rejected', 'rejected']);
    assert.equal(calls, 1, 'the queued offer must fail before allocating storage');
  } finally { a.close(); b.close(); f.gateway.dispose(); }
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

test('source read failure drains and releases the upload so its id can retry', async () => withStore(async store => {
  const { a, b, al, bl } = pair(); const info = await offer(FILE_CHUNK_BYTES * 2);
  const control = {
    offer: async (o: FileOffer) => { const sink = await store.accept('owner', o, authorize); b.receive(o.transfer_id, sink); return { offset: sink.offset }; },
    commit: (id: string) => b.commit(id), cancel: (id: string) => b.cancel(id),
  };
  try {
    await assert.rejects(a.send(info, { size: info.size, read: async (at, length) => {
      if (at > 0) throw new Error('source became unreadable');
      return pattern(info.size).read(at, length);
    } }, control), /unreadable/);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(al.closed, false); assert.equal(bl.closed, false);
    assert.equal((await a.send(info, pattern(info.size), control)).sha256, info.sha256);
  } finally { a.close(); b.close(); }
}));

test('closing a file lane waits for an in-progress commit before releasing its sink', async () => {
  const { a, b } = pair(); const info = await offer(0);
  let finish!: () => void; let entered!: () => void; let released = false;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  const started = new Promise<void>(resolve => { entered = resolve; });
  b.receive(info.transfer_id, { offset: 0, write: async () => undefined,
    finish: async () => { entered(); await pending; assert.equal(released, false); return { ...info, ref: info.transfer_id }; },
    close: async () => { released = true; }, cancel: async () => undefined,
  });
  const committed = b.commit(info.transfer_id);
  await started; b.close(); await new Promise(resolve => setImmediate(resolve));
  const premature = released; finish();
  await committed; a.close();
  assert.equal(premature, false);
  assert.equal(released, true);
});

test('cancelling an already-published sink cannot delete its receipt', async () => withStore(async store => {
  const info = await offer(0); const sink = await store.accept('owner', info, authorize);
  const published = await sink.finish(); await sink.cancel();
  assert.deepEqual(await store.receipt('owner', info.session_id, info.transfer_id), published);
}));

test('verifying one large file does not block another file transfer', { timeout: 2000 }, async () => {
  const { a, b } = pair();
  const first = await offer(0);
  let release!: () => void;
  const hashing = new Promise<void>(resolve => { release = resolve; });
  b.receive(first.transfer_id, { offset: 0, write: async () => {}, finish: async () => { await hashing; return { ...first, ref: first.transfer_id }; }, close: async () => {}, cancel: async () => {} });
  const finishing = b.commit(first.transfer_id);
  try {
    const second = await offer(3, 'ee'.repeat(16));
    let offset = 0;
    const receipt = await a.send(second, pattern(3), {
      offer: async () => { b.receive(second.transfer_id, { get offset() { return offset; }, write: async (_, bytes) => { offset += bytes.length; }, finish: async () => ({ ...second, ref: second.transfer_id }), close: async () => {}, cancel: async () => {} }); return { offset: 0 }; },
      commit: id => b.commit(id), cancel: id => b.cancel(id),
    });
    assert.equal(receipt.size, 3);
  } finally { release(); await finishing; a.close(); b.close(); }
});

test('four saturated windows leave room for cancellation barriers', { timeout: 4000 }, async () => {
  const { a, b, al } = pair();
  const controllers = Array.from({ length: 4 }, () => new AbortController());
  let unblock!: () => void; let full!: () => void; let sent = 0;
  const disk = new Promise<void>(resolve => { unblock = resolve; });
  const saturated = new Promise<void>(resolve => { full = resolve; });
  const send = al.sendFragment.bind(al);
  al.sendFragment = bytes => { send(bytes); if ((bytes[0] === 0 || bytes[0] === 2) && ++sent === 64) full(); };
  const infos = await Promise.all(controllers.map((_, index) => offer(FILE_CHUNK_BYTES * 16, String(index + 1).repeat(32))));
  const transfers = infos.map((info, index) => a.send(info, pattern(info.size), {
    offer: async () => {
      let offset = 0;
      b.receive(info.transfer_id, { get offset() { return offset; }, write: async (_, bytes) => { await disk; offset += bytes.length; }, finish: async () => ({ ...info, ref: info.transfer_id }), close: async () => {}, cancel: async () => {} });
      return { offset: 0 };
    }, commit: id => b.commit(id), cancel: id => b.cancel(id),
  }, undefined, controllers[index]!.signal));
  const cancelled = Promise.all(transfers.map(transfer => assert.rejects(transfer, /cancel|abort/i)));
  try {
    await saturated;
    controllers.forEach(controller => controller.abort());
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(al.closed, false, 'flush barriers must fit alongside four complete windows');
  } finally { unblock(); await cancelled; a.close(); b.close(); }
});
