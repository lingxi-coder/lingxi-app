import { fileTransferId, hashFile, type FileOffer, type FileReceipt, type FileSink, type FileProgress, type FileSource } from '@lingxi/bridge-client/remote';
import type { ViewerConnection } from './viewerConnection';
export const browserFileSource = (file: Blob): FileSource => ({ size: file.size, read: async (offset, length) => new Uint8Array(await file.slice(offset, offset + length).arrayBuffer()) });
export async function uploadFile(connection: ViewerConnection, session: string, file: File, progress?: FileProgress, signal?: AbortSignal): Promise<FileReceipt> {
  const source = browserFileSource(file);
  const hash = await hashFile(source, signal);
  const scope = await hashFile(browserFileSource(new Blob([connection.fileScope, '\0', session, '\0', file.name, '\0', hash])));
  const key = `rc-file-upload-${scope}`;
  const id = localStorage.getItem(key) ?? fileTransferId();
  localStorage.setItem(key, id);
  const offer: FileOffer = { transfer_id: id, session_id: session, name: file.name, size: file.size, sha256: hash };
  const channel = connection.files;
  if (!channel) throw new Error('Files require a WebRTC connection. Reconnect and retry.');
  return channel.send(offer, source, {
    offer: o => connection.request('file.offer', o),
    commit: transfer_id => connection.request('file.commit', { session_id: session, transfer_id }, { timeoutMs: 300_000 }),
    cancel: async transfer_id => { await connection.request('file.cancel', { session_id: session, transfer_id }); localStorage.removeItem(key); },
  }, progress, signal);
}
export async function downloadFile(connection: ViewerConnection, offer: FileReceipt, progress?: FileProgress, signal?: AbortSignal): Promise<File> {
  const channel = connection.files;
  if (!channel) throw new Error('Files require a WebRTC connection.');
  const worker = new Worker(new URL('./fileWorker.ts', import.meta.url), { type: 'module' });
  let sequence = 0;
  const pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
  worker.onmessage = event => {
    const { seq, result, error } = event.data;
    const request = pending.get(seq); pending.delete(seq);
    error ? request?.reject(new Error(error)) : request?.resolve(result);
  };
  worker.onerror = () => { for (const p of pending.values()) p.reject(new Error('File storage worker failed')); pending.clear(); };
  const call = <T>(t: string, extra: Record<string, unknown> = {}): Promise<T> => new Promise((resolve, reject) => {
    const seq = sequence++; pending.set(seq, { resolve: value => resolve(value as T), reject });
    worker.postMessage({ seq, t, ...extra });
  });
  let finished = false;
  const cancel = (): void => { void connection.request('file.cancel', { session_id: offer.session_id, transfer_id: offer.transfer_id }, { timeoutMs: 75_000 }).catch(() => undefined); };
  try {
    signal?.throwIfAborted();
    const scope = await hashFile(browserFileSource(new Blob([connection.fileScope, '\0', offer.session_id, '\0', offer.sha256])));
    let offset = await call<number>('open', { scope, offer });
    let result: File | undefined;
    const sink: FileSink = {
      get offset() { return offset; },
      write: async (at, bytes) => { await call<number>('write', { offset: at, bytes }); },
      checkpoint: async () => { offset = await call<number>('checkpoint'); progress?.(offset, offer.size); },
      finish: async () => { result = await call<File>('finish'); return offer; },
      close: async () => { await call('close'); },
      cancel: async () => { await call('cancel'); },
    };
    channel.receive(offer.transfer_id, sink);
    const receiving = connection.request('file.download', { session_id: offer.session_id, transfer_id: offer.transfer_id, offset }, { timeoutMs: 24 * 60 * 60_000 });
    signal?.addEventListener('abort', cancel, { once: true });
    if (signal?.aborted) cancel();
    await receiving;
    signal?.throwIfAborted();
    await channel.commit(offer.transfer_id);
    finished = true;
    return result!;
  } finally {
    signal?.removeEventListener('abort', cancel);
    if (!finished) {
      if (signal?.aborted) await channel.cancel(offer.transfer_id).catch(() => undefined);
      else await channel.detach(offer.transfer_id).catch(() => undefined);
    }
    worker.terminate();
  }
}
