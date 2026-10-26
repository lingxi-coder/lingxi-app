import { fileTransferId, hashFile, validateFileOffer, type FileOffer, type FileReceipt, type FileSink, type FileProgress, type FileSource } from '@lingxi/bridge-client/remote';
import type { ViewerConnection } from './viewerConnection';

export const browserFileSource = (file: Blob): FileSource => ({ size: file.size, read: async (offset, length) => new Uint8Array(await file.slice(offset, offset + length).arrayBuffer()) });

function untilAborted<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = (): void => { signal.removeEventListener('abort', abort); reject(signal.reason ?? new Error('File transfer cancelled')); };
    signal.addEventListener('abort', abort, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
    if (signal.aborted) abort();
  });
}

/** A crashed worker stays failed: cleanup must not post to a dead worker and hang. */
class FileStorage {
  readonly failed = new AbortController();
  private sequence = 0;
  private pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
  constructor(private readonly worker: Worker) {
    worker.onmessage = event => {
      const { seq, result, error } = event.data;
      if (error) { this.fail(new Error(error)); return; }
      const request = this.pending.get(seq); this.pending.delete(seq);
      request?.resolve(result);
    };
    worker.onerror = () => this.fail(new Error('File storage worker failed'));
    worker.onmessageerror = () => this.fail(new Error('Invalid file storage response'));
  }
  private fail(error: Error): void {
    if (this.failed.signal.aborted) return;
    this.failed.abort(error);
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
  }
  call<T>(t: string, extra: Record<string, unknown> = {}): Promise<T> {
    if (this.failed.signal.aborted) return Promise.reject(this.failed.signal.reason);
    return new Promise((resolve, reject) => {
      const seq = this.sequence++; this.pending.set(seq, { resolve: value => resolve(value as T), reject });
      try { this.worker.postMessage({ seq, t, ...extra }); }
      catch (error) { this.fail(error instanceof Error ? error : new Error(String(error))); }
    });
  }
  dispose(): void { this.fail(new Error('File storage closed')); this.worker.terminate(); }
}

export async function uploadFile(connection: ViewerConnection, session: string, file: File, progress?: FileProgress, signal?: AbortSignal): Promise<FileReceipt> {
  const transport = connection.fileSession();
  const running = AbortSignal.any([transport.channel.closedSignal, ...(signal ? [signal] : [])]);
  running.throwIfAborted();
  const source = browserFileSource(file);
  const hash = await hashFile(source, running);
  const scope = await hashFile(browserFileSource(new Blob([transport.scope, '\0', session, '\0', file.name, '\0', hash])), running);
  running.throwIfAborted();
  const key = `rc-file-upload-${scope}`;
  const id = localStorage.getItem(key) ?? fileTransferId();
  localStorage.setItem(key, id);
  const offer: FileOffer = { transfer_id: id, session_id: session, name: file.name, size: file.size, sha256: hash };
  return transport.channel.send(offer, source, {
    offer: o => transport.request('file.offer', o),
    commit: transfer_id => transport.request('file.commit', { session_id: session, transfer_id }, { timeoutMs: 300_000 }),
    // Another tab may be using this persisted identity. Cancellation discards
    // remote partial data, but the same ID can safely restart from offset zero.
    cancel: async transfer_id => { await transport.request('file.cancel', { session_id: session, transfer_id }); },
  }, progress, running);
}

export async function downloadFile(connection: ViewerConnection, receipt: FileReceipt, progress?: FileProgress, signal?: AbortSignal): Promise<File> {
  const offer = { ...validateFileOffer(receipt), ref: receipt.ref };
  if (offer.ref !== offer.transfer_id) throw new Error('Invalid file receipt');
  const transport = connection.fileSession();
  const channel = transport.channel;
  const storage = new FileStorage(new Worker(new URL('./fileWorker.ts', import.meta.url), { type: 'module' }));
  const running = AbortSignal.any([channel.closedSignal, storage.failed.signal, ...(signal ? [signal] : [])]);
  let registered = false;
  let requested = false;
  let finished = false;
  try {
    running.throwIfAborted();
    const scope = await hashFile(browserFileSource(new Blob([transport.scope, '\0', offer.session_id, '\0', offer.sha256])), running);
    let offset = 0;
    let result: File | undefined;
    const sink: FileSink = {
      get offset() { return offset; },
      write: async (at, bytes) => { await storage.call<number>('write', { offset: at, bytes }); },
      checkpoint: async () => { offset = await storage.call<number>('checkpoint'); progress?.(offset, offer.size); },
      finish: async () => {
        // Verification may read gigabytes. No network frames remain at this
        // point, so cancellation can safely terminate the worker immediately.
        const stop = () => storage.dispose();
        running.addEventListener('abort', stop, { once: true });
        try {
          if (running.aborted) stop();
          result = await storage.call<File>('finish');
          return offer;
        } finally { running.removeEventListener('abort', stop); }
      },
      close: async () => { await storage.call('close'); },
      cancel: async () => { await storage.call('cancel'); },
    };
    running.throwIfAborted();
    channel.receive(offer.transfer_id, sink);
    registered = true;
    // Reserve the id before opening/truncating its partial file in OPFS.
    offset = await untilAborted(storage.call<number>('open', { scope, offer }), running);
    running.throwIfAborted();
    requested = true;
    await untilAborted(transport.request('file.download', { session_id: offer.session_id, transfer_id: offer.transfer_id, offset }, { timeoutMs: 24 * 60 * 60_000 }), running);
    running.throwIfAborted();
    await channel.commit(offer.transfer_id);
    finished = true;
    return result!;
  } finally {
    try {
      if (!finished && requested && !channel.closedSignal.aborted) {
        // Keep accepting frames until the remote sender has drained. Cancellation
        // cannot detach the sink while its final ordered frames are in flight.
        await transport.request('file.cancel', { session_id: offer.session_id, transfer_id: offer.transfer_id }, { timeoutMs: 75_000 })
          .catch(() => channel.close(new Error('File cancellation failed')));
      }
      if (!finished && registered) {
        if (signal?.aborted && !channel.closedSignal.aborted) await channel.cancel(offer.transfer_id).catch(() => undefined);
        else await channel.detach(offer.transfer_id).catch(() => undefined);
      }
    } finally { storage.dispose(); }
  }
}
