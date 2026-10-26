/** Binary file protocol, independent of JSON framing and never sent through RTDB.
 * Offsets are safe JS integers (up to 8 PiB). Every frame fits a 16 KiB SCTP
 * message. ACKs represent durable writes, not arrival in a memory queue.
 */
import { sha256 } from '@noble/hashes/sha256';
import type { FragmentLink } from './transport.js';

export const FILE_CHANNEL_LABEL = 'lingxi-files';
export const FILE_FRAME_BYTES = 16 * 1024;
export const FILE_HEADER_BYTES = 25; // kind:u8, id:16 bytes, offset:u64 big endian
export const FILE_CHUNK_BYTES = FILE_FRAME_BYTES - FILE_HEADER_BYTES;
export const FILE_WINDOW_BYTES = FILE_CHUNK_BYTES * 16;
export const FILE_ACK_TIMEOUT_MS = 60_000;
export const FILE_MAX_ACTIVE = 4;
export interface FileOffer {
  transfer_id: string; // 16 random bytes, lower-case hex
  session_id: string;
  name: string;
  size: number;
  sha256: string;
}
export interface FileReceipt extends FileOffer { ref: string }
export interface FileCheckpoint { offset: number }
export interface FileSource {
  readonly size: number;
  /** Must return exactly min(length, size-offset) bytes. No whole-file buffering. */
  read(offset: number, length: number): Promise<Uint8Array>;
}
export interface FileSink {
  readonly offset: number;
  /** Append sequential bytes. offset must report only durable progress. */
  write(offset: number, bytes: Uint8Array): Promise<void>;
  /** Flush all appended bytes and durably publish their checkpoint. */
  checkpoint?(): Promise<void>;
  /** Verify size/hash and atomically publish; may be retried after a lost reply. */
  finish(): Promise<FileReceipt>;
  /** Release handles; keep the durable checkpoint for reconnect. */
  close(): Promise<void>;
  /** Explicit cancellation removes incomplete data. */
  cancel(): Promise<void>;
}
export interface FileControl {
  offer(offer: FileOffer): Promise<FileCheckpoint>;
  commit(id: string): Promise<FileReceipt>;
  cancel(id: string): Promise<void>;
}
export type FileProgress = (offset: number, size: number) => void;
const ID = /^[0-9a-f]{32}$/;
const HASH = /^[0-9a-f]{64}$/;
export function validateFileOffer(value: unknown): FileOffer {
  if (!value || typeof value !== 'object') throw new Error('invalid file offer');
  const v = value as Record<string, unknown>;
  if (typeof v.transfer_id !== 'string' || !ID.test(v.transfer_id)
    || typeof v.session_id !== 'string' || v.session_id.length < 1 || v.session_id.length > 128
    || typeof v.name !== 'string' || v.name.length < 1 || v.name.length > 255
    || new TextEncoder().encode(v.name).length > 255 || /[\p{Cc}/\\]/u.test(v.name) || v.name === '.' || v.name === '..'
    || typeof v.size !== 'number' || !Number.isSafeInteger(v.size) || v.size < 0
    || typeof v.sha256 !== 'string' || !HASH.test(v.sha256)) throw new Error('invalid file offer');
  return { transfer_id: v.transfer_id, session_id: v.session_id, name: v.name, size: v.size, sha256: v.sha256 };
}
export function fileTransferId(): string {
  return Array.from(globalThis.crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2, '0')).join('');
}
export function encodeFileFrame(kind: 0 | 1 | 2 | 3 | 4, id: string, offset: number, payload: Uint8Array = new Uint8Array()): Uint8Array {
  if ((kind !== 0 && kind !== 1 && kind !== 2 && kind !== 3 && kind !== 4) || !ID.test(id) || !Number.isSafeInteger(offset) || offset < 0
    || payload.length > FILE_CHUNK_BYTES || ((kind === 1 || kind === 3 || kind === 4) && payload.length !== 0)
    || ((kind === 0 || kind === 2) && payload.length === 0)) throw new Error('invalid file frame');
  const frame = new Uint8Array(FILE_HEADER_BYTES + payload.length);
  frame[0] = kind;
  for (let i = 0; i < 16; i++) frame[i + 1] = parseInt(id.slice(i * 2, i * 2 + 2), 16);
  new DataView(frame.buffer).setBigUint64(17, BigInt(offset));
  frame.set(payload, FILE_HEADER_BYTES);
  return frame;
}
export function decodeFileFrame(frame: Uint8Array): { kind: 0 | 1 | 2 | 3 | 4; id: string; offset: number; payload: Uint8Array } {
  if (frame.length < FILE_HEADER_BYTES || frame.length > FILE_FRAME_BYTES || (frame[0] !== 0 && frame[0] !== 1 && frame[0] !== 2 && frame[0] !== 3 && frame[0] !== 4)
    || ((frame[0] === 1 || frame[0] === 3 || frame[0] === 4) && frame.length !== FILE_HEADER_BYTES) || ((frame[0] === 0 || frame[0] === 2) && frame.length === FILE_HEADER_BYTES)) throw new Error('invalid file frame');
  const offset = Number(new DataView(frame.buffer, frame.byteOffset, frame.byteLength).getBigUint64(17));
  if (!Number.isSafeInteger(offset)) throw new Error('invalid file offset');
  return { kind: frame[0], id: Array.from(frame.subarray(1, 17), b => b.toString(16).padStart(2, '0')).join(''), offset, payload: frame.subarray(FILE_HEADER_BYTES) };
}
export async function hashFile(source: FileSource, signal?: AbortSignal): Promise<string> {
  const hash = sha256.create();
  for (let offset = 0; offset < source.size;) {
    signal?.throwIfAborted();
    const size = Math.min(FILE_WINDOW_BYTES, source.size - offset);
    const bytes = await source.read(offset, size);
    if (bytes.length !== size) throw new Error('file changed while reading');
    hash.update(bytes);
    offset += size;
  }
  return Array.from(hash.digest(), b => b.toString(16).padStart(2, '0')).join('');
}
interface Sending {
  ack: number;
  sent: number;
  wake?: () => void;
  fail?: (error: Error) => void;
  drain?: () => void;
}
/** One connection, bidirectional, bounded to four active sends and 1 MiB of
 * queued ingress. Applications authorize an offer before calling receive(). */
export class FileChannel {
  private readonly sending = new Map<string, Sending>();
  private readonly sinks = new Map<string, FileSink>();
  private readonly commits = new Map<string, Promise<FileReceipt>>();
  private incoming = Promise.resolve();
  private queued = 0;
  private closed: Error | undefined;
  private readonly shutdown = new AbortController();
  get closedSignal(): AbortSignal { return this.shutdown.signal; }
  constructor(private readonly link: FragmentLink) {
    link.onFragment(frame => this.ingress(frame));
    link.onClose(reason => this.close(new Error(`file channel closed: ${reason}`)));
  }
  receive(id: string, sink: FileSink): void {
    if (this.closed) throw this.closed;
    if (!ID.test(id) || this.sinks.has(id) || this.sending.has(id) || this.commits.has(id) || this.sinks.size >= FILE_MAX_ACTIVE) throw new Error('file receiver busy');
    this.sinks.set(id, sink);
  }
  commit(id: string): Promise<FileReceipt> {
    const pending = this.commits.get(id);
    if (pending) return pending;
    const commit = this.incoming.then(async () => {
      if (this.closed) throw this.closed;
      const sink = this.sinks.get(id);
      if (!sink) throw new Error('unknown transfer');
      try { return await sink.finish(); }
      finally {
        this.sinks.delete(id);
        await sink.close().catch(() => undefined);
      }
    });
    // Only this sink waits for its hash/rename. Other transfers must continue
    // receiving and acknowledging while a large file is being verified.
    this.commits.set(id, commit);
    void commit.finally(() => this.commits.delete(id)).catch(() => undefined);
    return commit;
  }
  /** Network interruption releases the handle while retaining durable progress. */
  async detach(id: string): Promise<void> {
    const committing = this.commits.get(id);
    const detach = this.incoming.then(async () => {
      await committing?.catch(() => undefined);
      const sink = this.sinks.get(id);
      this.sinks.delete(id);
      await sink?.close();
    });
    this.incoming = detach.catch(error => this.close(asError(error)));
    await detach;
  }
  async cancel(id: string): Promise<void> {
    // Queue cancellation behind writes already accepted from this channel.
    const committing = this.commits.get(id);
    const cancel = this.incoming.then(async () => {
      await committing?.catch(() => undefined);
      const sink = this.sinks.get(id);
      this.sinks.delete(id);
      await sink?.cancel();
    });
    this.incoming = cancel.catch(error => this.close(asError(error)));
    await cancel;
  }
  async send(offer: FileOffer, source: FileSource, control: FileControl, progress?: FileProgress, signal?: AbortSignal): Promise<FileReceipt> {
    validateFileOffer(offer);
    if (source.size !== offer.size) throw new Error('file size changed');
    if (this.closed) throw this.closed;
    if (this.sending.has(offer.transfer_id) || this.sinks.has(offer.transfer_id) || this.commits.has(offer.transfer_id) || this.sending.size >= FILE_MAX_ACTIVE) throw new Error('file sender busy');
    const s: Sending = { ack: 0, sent: 0 };
    this.sending.set(offer.transfer_id, s);
    let offered = false;
    const wait = <T>(promise: Promise<T>): Promise<T> => withAbort(withAbort(promise, this.closedSignal), signal);
    try {
      signal?.throwIfAborted();
      const chunkBytes = Math.min(FILE_CHUNK_BYTES, (this.link.maxFragmentBytes ?? FILE_FRAME_BYTES) - FILE_HEADER_BYTES);
      if (chunkBytes < 1) throw new Error('negotiated file message size is too small');
      offered = true;
      const { offset } = await wait(control.offer(offer));
      if (this.closed) throw this.closed;
      if (!Number.isSafeInteger(offset) || offset < 0 || offset > offer.size) throw new Error('invalid checkpoint');
      s.ack = offset; s.sent = offset;
      progress?.(offset, offer.size);
      while (s.ack < offer.size) {
        const end = Math.min(offer.size, s.ack + chunkBytes * 16);
        while (s.sent < end) {
          signal?.throwIfAborted();
          if (this.closed) throw this.closed;
          const length = Math.min(chunkBytes, end - s.sent);
          const bytes = await wait(source.read(s.sent, length));
          if (bytes.length !== length) throw new Error('file changed while reading');
          await wait(this.link.whenWritable?.() ?? Promise.resolve());
          signal?.throwIfAborted();
          if (this.closed) throw this.closed;
          const start = s.sent;
          s.sent += length; // ACK may arrive synchronously from an in-memory link.
          try { this.link.sendFragment(encodeFileFrame(s.sent === end ? 2 : 0, offer.transfer_id, start, bytes)); }
          catch (error) { s.sent = start; throw error; }
        }
        await this.waitAck(s, end, signal);
        progress?.(s.ack, offer.size);
      }
      signal?.throwIfAborted();
      const receipt = await wait(control.commit(offer.transfer_id));
      if (JSON.stringify(validateFileOffer(receipt)) !== JSON.stringify(validateFileOffer(offer)) || receipt.ref !== offer.transfer_id) throw new Error('invalid file receipt');
      return receipt;
    } catch (error) {
      if (offered && !this.closed) {
        // Any local failure must release its remote registration before retry.
        // Drain already-sent frames on the ordered file lane before removing
        // the receiver over the independent control lane.
        if (s.sent > s.ack && !this.closed) {
          try {
            await new Promise<void>((resolve, reject) => {
              const timer = setTimeout(() => done(new Error('file cancellation drain timed out')), FILE_ACK_TIMEOUT_MS);
              const done = (error?: Error): void => { clearTimeout(timer); s.drain = undefined; s.fail = undefined; error ? reject(error) : resolve(); };
              s.drain = () => done(); s.fail = done;
              try { this.link.sendFragment(encodeFileFrame(3, offer.transfer_id, s.sent)); }
              catch (error) { done(asError(error)); }
            });
          } catch { this.close(new Error('file cancellation drain failed')); }
        }
        if (!this.closed) await control.cancel(offer.transfer_id).catch(() => this.close(new Error('file cleanup failed')));
      }
      throw error;
    } finally {
      this.sending.delete(offer.transfer_id);
    }
  }
  private waitAck(s: Sending, end: number, signal?: AbortSignal): Promise<void> {
    if (this.closed) return Promise.reject(this.closed);
    if (s.ack >= end) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const done = (error?: Error): void => {
        clearTimeout(timer); signal?.removeEventListener('abort', abort);
        s.wake = undefined; s.fail = undefined;
        error ? reject(error) : resolve();
      };
      const abort = (): void => done(new Error('file transfer cancelled'));
      const timer = setTimeout(() => done(new Error('file acknowledgement timed out')), FILE_ACK_TIMEOUT_MS);
      s.wake = () => { if (s.ack >= end) done(); };
      s.fail = error => done(error);
      signal?.addEventListener('abort', abort, { once: true });
      if (signal?.aborted) abort();
      else if (s.ack >= end) done();
    });
  }
  private ingress(frame: Uint8Array): void {
    if (this.closed) return;
    try {
      const message = decodeFileFrame(frame);
      if (message.kind === 4) {
        const s = this.sending.get(message.id);
        if (!s?.drain || message.offset !== s.sent) throw new Error('invalid file flush acknowledgement');
        s.drain();
        return;
      }
      if (message.kind === 1) {
        const s = this.sending.get(message.id);
        if (!s || message.offset < s.ack || message.offset > s.sent) throw new Error('invalid file acknowledgement');
        s.ack = message.offset; s.wake?.();
        return;
      }
      this.queued += frame.length;
      // Each full window can be followed by one cancellation barrier before
      // storage catches up. Reserve that header so valid concurrent cancels do
      // not exceed the receive bound when all four windows are queued.
      if (this.queued > (FILE_WINDOW_BYTES + FILE_HEADER_BYTES * 17) * FILE_MAX_ACTIVE) throw new Error('file receive window exceeded');
      this.incoming = this.incoming.then(async () => {
        if (this.closed) return;
        const sink = this.sinks.get(message.id);
        if (!sink) throw new Error('file transfer not accepted');
        if (message.kind !== 3) await sink.write(message.offset, message.payload);
        if (message.kind === 2 || message.kind === 3) await sink.checkpoint?.();
        if (message.kind === 3 && sink.offset !== message.offset) throw new Error('invalid file flush barrier');
        this.queued -= frame.length;
        if (this.closed) return;
        await this.link.whenWritable?.();
        this.link.sendFragment(encodeFileFrame(message.kind === 3 ? 4 : 1, message.id, sink.offset));
      }).catch(error => this.close(asError(error)));
    } catch (error) { this.close(asError(error)); }
  }
  close(error = new Error('file channel closed')): void {
    if (this.closed) return;
    this.closed = error;
    this.shutdown.abort(error);
    this.link.close('file_channel_closed');
    for (const s of this.sending.values()) s.fail?.(error);
    // In-flight writes finish before handles are closed and checkpoints retained.
    void this.incoming.then(async () => {
      await Promise.allSettled([...this.commits.values()]);
      for (const sink of this.sinks.values()) await sink.close().catch(() => undefined);
      this.sinks.clear();
    });
  }
}
function asError(error: unknown): Error { return error instanceof Error ? error : new Error(String(error)); }

function withAbort<T>(promise: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return promise;
  if (signal.aborted) {
    void promise.catch(() => undefined);
    return Promise.reject(signal.reason ?? new Error('file transfer cancelled'));
  }
  return new Promise((resolve, reject) => {
    const abort = (): void => { signal.removeEventListener('abort', abort); reject(new Error('file transfer cancelled')); };
    signal.addEventListener('abort', abort, { once: true });
    promise.then(value => { signal.removeEventListener('abort', abort); resolve(value); }, error => { signal.removeEventListener('abort', abort); reject(error); });
  });
}
