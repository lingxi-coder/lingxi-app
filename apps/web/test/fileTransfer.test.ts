import assert from 'node:assert/strict';
import test from 'node:test';
import { FileChannel, type FileReceipt, type FileSink, type FragmentLink } from '@lingxi/bridge-client/remote';
import { downloadFile } from '../src/remote/fileTransfer';
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
function setup(t: test.TestContext, request: (method: string) => Promise<unknown>) {
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
