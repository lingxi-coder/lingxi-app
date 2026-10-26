import assert from 'node:assert/strict';
import test from 'node:test';
import { FileChannel, type FileReceipt, type FileSink, type FragmentLink } from '@lingxi/bridge-client/remote';
import { downloadFile, uploadFile } from '../src/remote/fileTransfer';
import type { ViewerConnection } from '../src/remote/viewerConnection';
const receipt: FileReceipt = { transfer_id: 'ab'.repeat(16), ref: 'ab'.repeat(16), session_id: 'session', name: 'empty.bin', size: 0, sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855' };
class StorageWorker {
  static instances: StorageWorker[] = [];
  static failOpen = false;
  static onFinish: (() => void) | undefined;
  onmessage?: (e: { data: unknown }) => void;
  onerror?: () => void;
  onmessageerror?: () => void;
  terminated = false;
  constructor() { StorageWorker.instances.push(this); }
  postMessage({ seq, t }: { seq: number; t: string }) {
    queueMicrotask(() => {
      if (t === 'finish' && StorageWorker.onFinish) { StorageWorker.onFinish(); return; }
      if (t === 'open' && StorageWorker.failOpen) this.onerror?.();
      else this.onmessage?.({ data: { seq, result: t === 'open' ? 0 : undefined } });
    });
  }
  terminate() { this.terminated = true; }
}
Object.defineProperty(globalThis, 'Worker', { value: StorageWorker, configurable: true, writable: true });
function setup(t: test.TestContext, request: (method: string, params?: any) => Promise<unknown>) {
  StorageWorker.instances = []; StorageWorker.failOpen = false; StorageWorker.onFinish = undefined;
  const link: FragmentLink = { sendFragment() {}, onFragment: () => () => {}, onClose: () => () => {}, close() {} };
  const channel = new FileChannel(link);
  const connection = { fileSession: () => ({ channel, scope: 'owner\0host', request }) } as unknown as ViewerConnection;
  t.after(() => channel.close());
  return { channel, connection };
}
test('worker crash during open rejects and terminates without waiting on dead cleanup', { timeout: 2000 }, async t => {
  const { connection } = setup(t, async () => { throw new Error('unexpected RPC'); });
  StorageWorker.failOpen = true;
  await assert.rejects(downloadFile(connection, receipt), /worker failed/);
  assert.equal(StorageWorker.instances[0]?.terminated, true);
});
test('worker crash while receiving cancels the pinned sender and releases the lane', { timeout: 2000 }, async t => {
  const calls: string[] = [];
  const { connection, channel } = setup(t, async method => {
    calls.push(method);
    if (method === 'file.download') {
      queueMicrotask(() => StorageWorker.instances[0]!.onerror?.());
      return new Promise(() => {});
    }
    return {};
  });
  await assert.rejects(downloadFile(connection, receipt), /worker failed/);
  assert.deepEqual(calls, ['file.download', 'file.cancel']);
  assert.equal(StorageWorker.instances[0]?.terminated, true);
  assert.equal(channel.closedSignal.aborted, true);
});
test('rejected duplicate download cannot detach the original receiver', { timeout: 2000 }, async t => {
  const { connection, channel } = setup(t, async () => { throw new Error('unexpected RPC'); });
  let released = false;
  const original: FileSink = { offset: 0, write: async () => {}, finish: async () => receipt, close: async () => { released = true; }, cancel: async () => {} };
  channel.receive(receipt.transfer_id, original);
  await assert.rejects(downloadFile(connection, receipt), /receiver busy/);
  assert.equal(released, false);
  assert.deepEqual(await channel.commit(receipt.transfer_id), receipt);
});
test('closing only the file lane interrupts a pending download control reply', { timeout: 2000 }, async t => {
  const { connection, channel } = setup(t, async method => {
    assert.equal(method, 'file.download');
    queueMicrotask(() => channel.close());
    return new Promise(() => {});
  });
  await assert.rejects(downloadFile(connection, receipt), /channel closed/);
  assert.equal(StorageWorker.instances[0]?.terminated, true);
});


test('cancellation during a long verification terminates the worker promptly', { timeout: 2000 }, async t => {
  const cancel = new AbortController();
  const { connection } = setup(t, async () => ({}));
  StorageWorker.onFinish = () => cancel.abort();
  await assert.rejects(downloadFile(connection, receipt, undefined, cancel.signal), /storage closed/);
  assert.equal(StorageWorker.instances[0]?.terminated, true);
});

test('a rejected duplicate upload preserves another tab resume identity', { timeout: 2000 }, async t => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const saved = new Map<string, string>();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value),
    removeItem: (key: string) => saved.delete(key),
  } });
  t.after(() => { if (original) Object.defineProperty(globalThis, 'localStorage', original); else delete (globalThis as any).localStorage; });
  let active!: FileReceipt;
  let started!: () => void; const ready = new Promise<void>(resolve => { started = resolve; });
  let finish!: (value: FileReceipt) => void; const completion = new Promise<FileReceipt>(resolve => { finish = resolve; });
  const first = setup(t, async (method, params) => {
    if (method === 'file.offer') { active = { ...params, ref: params.transfer_id }; return { offset: 0 }; }
    if (method === 'file.commit') { started(); return completion; }
    return {};
  });
  const second = setup(t, async method => {
    if (method === 'file.offer') throw new Error('transfer already active');
    return {};
  });
  const file = new File([], 'empty.bin');
  const running = uploadFile(first.connection, 'session', file);
  await ready;
  try {
    const before = [...saved.entries()];
    await assert.rejects(uploadFile(second.connection, 'session', file), /already active/);
    assert.deepEqual([...saved.entries()], before);
  } finally { finish(active); await running; }
});
