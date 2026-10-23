/**
 * The RTDB tunnel: a {@link FragmentLink} that carries fragments as
 * AES-GCM-sealed frames written under `…/up/f{seq}` (viewer to host) or
 * `…/down/f{seq}` (host to viewer) and deleted by the reader once processed.
 * It is the fallback when no direct WebRTC connection can be made.
 *
 * Liveness pings are not part of this class; they are RPC notifications.
 */

import { systemClock, type Clock } from './clock.js';
import { RemoteProtocolError } from './errors.js';
import type { WebCryptoKey } from './keys.js';
import { childEntries, RtdbError, type Rtdb, type RtdbEvent, type RtdbSubscription } from './rtdb.js';
import { splitPath } from './rtdbTree.js';
import { frameKey, parseFrameKey } from './signaling.js';
import type { FragmentLink } from './transport.js';
import {
  openFrame,
  packFragments,
  sealFrame,
  TunnelCryptoError,
  unpackFragments,
  type TunnelDir,
} from './tunnelCrypto.js';
import {
  FRAGMENT_MAX_BYTES,
  TUNNEL_DELETE_BATCH,
  TUNNEL_DELETE_INTERVAL_MS,
  TUNNEL_FLUSH_DELAY_MS,
  TUNNEL_FRAGMENT_MAX_BYTES,
  TUNNEL_FRAME_PLAINTEXT_MAX,
  TUNNEL_GAP_TIMEOUT_MS,
  TUNNEL_MAX_INFLIGHT_WRITES,
  TUNNEL_WRITE_HIGH_WATER_BYTES,
} from './version.js';

/** Options of {@link TunnelLink}. */
export interface TunnelLinkOptions {
  rtdb: Rtdb;
  connId: string;
  /** Where this side writes frames: `…/up` for the viewer, `…/down` for the host. */
  sendPath: string;
  sendDir: TunnelDir;
  /** `keys.up` for the viewer, `keys.down` for the host. */
  sendKey: WebCryptoKey;
  /** Where the peer's frames appear: `…/down` for the viewer, `…/up` for the host. */
  recvPath: string;
  recvDir: TunnelDir;
  recvKey: WebCryptoKey;
  /**
   * `true` (viewer): listen on `recvPath` here. `false` (host): the owner has
   * one stream on `to_host/{host_id}` for every connection and feeds the
   * frames in through {@link TunnelLink.deliverFrame}.
   */
  subscribe: boolean;
  /** Defaults to the system clock. */
  clock?: Clock;
}

// Frames that may wait for a missing predecessor (or for decryption) at once.
const MAX_BUFFERED_FRAMES = 1024;
// Writes are retried this many times in all, backing off from 200 ms.
const WRITE_ATTEMPTS = 5;
const WRITE_RETRY_BASE_MS = 200;
// close() waits at most this long for queued frames to reach RTDB.
const CLOSE_FLUSH_TIMEOUT_MS = 10_000;
// The reason a link closes with when the peer's close notice arrives.
const PEER_CLOSED = 'peer_closed';
// A failing delete batch is given up on after this many consecutive failures.
const DELETE_FAILURES_MAX = 3;
const MAX_FRAME_SEQ = 9_999_999_999;

interface OutFrame {
  seq: number;
  fragments: Uint8Array[];
  bytes: number;
}

function retryable(error: unknown): boolean {
  if (error instanceof RtdbError) return error.status === 0 || error.status === 429 || error.status >= 500;
  return true;
}

/**
 * One connection's tunnel endpoint.
 *
 * **Writing.** Fragments queue up and are packed greedily into frames of at
 * most 49136 plaintext bytes. A frame is written when it is full or 25 ms after
 * its first fragment, as `PUT {sendPath}/f{seq}` with `seq` counting from 0,
 * at most four writes in flight. A failed write is retried with backoff and
 * ends the link (`tunnel_write`) when retries run out.
 *
 * **Reading.** Frames are decrypted on arrival, reordered by `seq` and
 * delivered in order; a frame below the expected `seq` (the repeat after an
 * SSE reconnect) is ignored. Processed frames are deleted with one PATCH every
 * 500 ms or 32 frames. A missing frame that stays missing for 30 s ends the
 * link (`tunnel_gap`); a frame that fails authentication ends it
 * (`tunnel_crypto`).
 *
 * **Closing.** `close(reason)` stops reading at once, flushes what is queued,
 * writes the close notice (one more frame, sealed like any other, whose
 * plaintext holds no fragments) and then deletes its own `sendPath` subtree
 * (best effort). The notice is skipped when the link cannot write any more, and
 * when the close was itself the answer to the peer's notice. The reader closes
 * with `peer_closed` once it has processed every frame before the notice.
 * {@link whenClosed} resolves when the cleanup is done. A peer that vanishes
 * without a notice is caught by the connection-level pings or the gap timeout.
 *
 * Reasons the link closes with by itself: `peer_closed`, `tunnel_crypto`,
 * `protocol_error`, `tunnel_gap`, `tunnel_overflow`, `tunnel_write`,
 * `tunnel_listen` and `tunnel_exhausted`.
 */
export class TunnelLink implements FragmentLink {
  /** Fragment size the tunnel wants, so that three fill a frame. */
  readonly maxFragmentBytes = TUNNEL_FRAGMENT_MAX_BYTES;

  private readonly rtdb: Rtdb;
  private readonly clock: Clock;
  private readonly connId: string;
  private readonly sendPath: string;
  private readonly sendDir: TunnelDir;
  private readonly sendKey: WebCryptoKey;
  private readonly recvPath: string;
  private readonly recvDir: TunnelDir;
  private readonly recvKey: WebCryptoKey;

  // lifecycle
  private closed = false;
  private closeReason: string | undefined;
  private settled: Promise<void> = Promise.resolve();
  private fragmentHandler: ((fragment: Uint8Array) => void) | undefined;
  private readonly closeHandlers: Array<(reason: string) => void> = [];
  private subscription: RtdbSubscription | undefined;

  // writer
  private sendSeq = 0;
  private pending: Uint8Array[] = [];
  private pendingBytes = 0;
  private readonly outbox: OutFrame[] = [];
  private inFlight = 0;
  private unsent = 0;
  private flushTimer: unknown;
  private abortWrites = false;
  private waiters: Array<() => void> = [];
  private onDrained: (() => void) | undefined;

  // reader
  private recvNext = 0;
  private readonly decrypting = new Set<number>();
  private readonly ready = new Map<number, Uint8Array[]>();
  private gapTimer: unknown;
  private toDelete: string[] = [];
  private deleteTimer: unknown;
  private deleting = false;
  private deleteFailures = 0;

  constructor(options: TunnelLinkOptions) {
    this.rtdb = options.rtdb;
    this.clock = options.clock ?? systemClock;
    this.connId = options.connId;
    this.sendPath = options.sendPath;
    this.sendDir = options.sendDir;
    this.sendKey = options.sendKey;
    this.recvPath = options.recvPath;
    this.recvDir = options.recvDir;
    this.recvKey = options.recvKey;
    if (options.subscribe) {
      this.subscription = this.rtdb.listen(
        this.recvPath,
        (event) => this.onRtdbEvent(event),
        () => this.close('tunnel_listen'),
      );
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // FragmentLink
  // ───────────────────────────────────────────────────────────────────────────

  sendFragment(fragment: Uint8Array): void {
    if (this.closed) throw new Error(`tunnel link is closed: ${this.closeReason}`);
    if (fragment.length === 0 || fragment.length > FRAGMENT_MAX_BYTES) {
      throw new RemoteProtocolError('fragment size out of range');
    }
    const cost = 4 + fragment.length;
    if (this.pendingBytes + cost > TUNNEL_FRAME_PLAINTEXT_MAX) this.sealPending();
    this.pending.push(fragment);
    this.pendingBytes += cost;
    this.unsent += fragment.length;
    if (TUNNEL_FRAME_PLAINTEXT_MAX - this.pendingBytes < 5) {
      this.sealPending(); // no room left for even a one-byte fragment
    } else if (this.flushTimer === undefined) {
      this.flushTimer = this.clock.setTimeout(() => this.sealPending(), TUNNEL_FLUSH_DELAY_MS);
    }
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.fragmentHandler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    const reason = this.closeReason;
    if (reason === undefined) this.closeHandlers.push(callback);
    else queueMicrotask(() => callback(reason));
  }

  whenWritable(): Promise<void> {
    if (this.closed || this.unsent < TUNNEL_WRITE_HIGH_WATER_BYTES) return Promise.resolve();
    return new Promise<void>((resolve) => this.waiters.push(resolve));
  }

  close(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    this.closeReason = reason;
    this.subscription?.close();
    this.clock.clearTimeout(this.gapTimer);
    this.gapTimer = undefined;
    this.ready.clear();
    this.decrypting.clear();
    this.sealPending();
    if (reason !== PEER_CLOSED) this.queueCloseNotice();
    this.releaseWaiters();
    for (const handler of this.closeHandlers.splice(0)) {
      try {
        handler(reason);
      } catch {
        // Close handlers are independent of each other.
      }
    }
    this.settled = this.finishClose();
  }

  /** Resolves once {@link close} has finished flushing and deleting; immediately if never closed. */
  whenClosed(): Promise<void> {
    return this.settled;
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Incoming frames
  // ───────────────────────────────────────────────────────────────────────────

  /**
   * Hand in the value stored at `{recvPath}/f{seq}`. For links created with
   * `subscribe: false`; harmless (and ignored) for deletions, duplicates and
   * anything that is not a string.
   */
  deliverFrame(seq: number, value: unknown): void {
    if (this.closed || typeof value !== 'string') return;
    if (!Number.isSafeInteger(seq) || seq < this.recvNext || seq > MAX_FRAME_SEQ) return;
    if (this.ready.has(seq) || this.decrypting.has(seq)) return;
    if (this.ready.size + this.decrypting.size >= MAX_BUFFERED_FRAMES) {
      this.close('tunnel_overflow');
      return;
    }
    this.decrypting.add(seq);
    void this.decode(seq, value);
    this.updateGapTimer();
  }

  private onRtdbEvent(event: RtdbEvent): void {
    if (this.closed) return;
    const path = splitPath(event.path);
    if (path.length === 0) {
      for (const [key, value] of childEntries(event.data)) this.deliverKey(key, value);
    } else if (path.length === 1 && event.type === 'put') {
      this.deliverKey(path[0]!, event.data);
    }
  }

  private deliverKey(key: string, value: unknown): void {
    const seq = parseFrameKey(key);
    if (seq !== null) this.deliverFrame(seq, value);
  }

  private async decode(seq: number, stored: string): Promise<void> {
    let fragments: Uint8Array[];
    try {
      fragments = unpackFragments(await openFrame(this.recvKey, this.connId, this.recvDir, seq, stored));
    } catch (error) {
      this.decrypting.delete(seq);
      this.close(error instanceof TunnelCryptoError ? 'tunnel_crypto' : 'protocol_error');
      return;
    }
    this.decrypting.delete(seq);
    if (this.closed) return;
    this.ready.set(seq, fragments);
    this.drain();
  }

  /** Deliver every frame that is now next in line. */
  private drain(): void {
    while (!this.closed) {
      const fragments = this.ready.get(this.recvNext);
      if (fragments === undefined) break;
      this.ready.delete(this.recvNext);
      this.processed(this.recvNext++);
      if (fragments.length === 0) {
        this.close(PEER_CLOSED); // a frame without fragments is the peer's close notice
        break;
      }
      for (const fragment of fragments) {
        if (this.closed) break;
        try {
          this.fragmentHandler?.(fragment);
        } catch {
          // A throwing consumer must not stall the frames behind this one.
        }
      }
    }
    this.updateGapTimer();
  }

  /**
   * The 30 s clock on a missing frame: armed when later frames are waiting for
   * one that has not arrived, left running while more frames pile up behind
   * it, and stopped as soon as it shows up. A second hole behind it gets a
   * fresh 30 s, since the timer was stopped when the first one was filled.
   */
  private updateGapTimer(): void {
    const waiting = this.ready.size + this.decrypting.size > 0;
    const arrived = this.ready.has(this.recvNext) || this.decrypting.has(this.recvNext);
    if (!waiting || arrived) {
      this.clock.clearTimeout(this.gapTimer);
      this.gapTimer = undefined;
    } else if (this.gapTimer === undefined) {
      this.gapTimer = this.clock.setTimeout(() => {
        this.gapTimer = undefined;
        if (!this.closed) this.close('tunnel_gap');
      }, TUNNEL_GAP_TIMEOUT_MS);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Deleting processed frames
  // ───────────────────────────────────────────────────────────────────────────

  private processed(seq: number): void {
    this.toDelete.push(frameKey(seq));
    if (this.toDelete.length >= TUNNEL_DELETE_BATCH) {
      void this.flushDeletes();
    } else if (this.deleteTimer === undefined) {
      this.deleteTimer = this.clock.setTimeout(() => {
        this.deleteTimer = undefined;
        void this.flushDeletes();
      }, TUNNEL_DELETE_INTERVAL_MS);
    }
  }

  private async flushDeletes(): Promise<void> {
    this.clock.clearTimeout(this.deleteTimer);
    this.deleteTimer = undefined;
    if (this.deleting || this.toDelete.length === 0) return;
    this.deleting = true;
    const keys = this.toDelete.splice(0, 256);
    try {
      await this.rtdb.patch(this.recvPath, Object.fromEntries(keys.map((key): [string, null] => [key, null])));
      this.deleteFailures = 0;
    } catch {
      // Deleting is housekeeping: try again with the next batch, then give up.
      if (++this.deleteFailures < DELETE_FAILURES_MAX) this.toDelete.unshift(...keys);
    } finally {
      this.deleting = false;
    }
    if (this.closed) return;
    if (this.toDelete.length >= TUNNEL_DELETE_BATCH) void this.flushDeletes();
    else if (this.toDelete.length > 0 && this.deleteTimer === undefined) {
      this.deleteTimer = this.clock.setTimeout(() => {
        this.deleteTimer = undefined;
        void this.flushDeletes();
      }, TUNNEL_DELETE_INTERVAL_MS);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Outgoing frames
  // ───────────────────────────────────────────────────────────────────────────

  /** Close the frame being filled and queue it for writing. */
  private sealPending(): void {
    this.clock.clearTimeout(this.flushTimer);
    this.flushTimer = undefined;
    if (this.pending.length === 0) return;
    if (this.sendSeq > MAX_FRAME_SEQ) {
      this.abortWrites = true;
      this.close('tunnel_exhausted');
      return;
    }
    const fragments = this.pending;
    this.pending = [];
    this.pendingBytes = 0;
    this.outbox.push({ seq: this.sendSeq++, fragments, bytes: fragments.reduce((sum, f) => sum + f.length, 0) });
    this.pump();
  }

  /** Queue the close notice: a frame with no fragments, after everything written so far. */
  private queueCloseNotice(): void {
    if (this.sendSeq > MAX_FRAME_SEQ) return;
    this.outbox.push({ seq: this.sendSeq++, fragments: [], bytes: 0 });
    this.pump();
  }

  private pump(): void {
    while (!this.abortWrites && this.inFlight < TUNNEL_MAX_INFLIGHT_WRITES && this.outbox.length > 0) {
      const frame = this.outbox.shift()!;
      this.inFlight++;
      void this.writeFrame(frame).finally(() => {
        this.inFlight--;
        this.unsent -= frame.bytes;
        this.releaseWaiters();
        this.pump();
        this.checkDrained();
      });
    }
    this.checkDrained();
  }

  private async writeFrame(frame: OutFrame): Promise<void> {
    try {
      const stored = await sealFrame(this.sendKey, this.connId, this.sendDir, frame.seq, packFragments(frame.fragments));
      const path = `${this.sendPath}/${frameKey(frame.seq)}`;
      for (let attempt = 0; ; attempt++) {
        try {
          await this.rtdb.put(path, stored);
          return;
        } catch (error) {
          if (attempt + 1 >= WRITE_ATTEMPTS || !retryable(error) || this.abortWrites) throw error;
          await new Promise<void>((resolve) => this.clock.setTimeout(resolve, WRITE_RETRY_BASE_MS * 2 ** attempt));
          if (this.abortWrites) throw error;
        }
      }
    } catch {
      this.abortWrites = true;
      this.close('tunnel_write');
    }
  }

  private releaseWaiters(): void {
    if (!this.closed && this.unsent >= TUNNEL_WRITE_HIGH_WATER_BYTES) return;
    for (const resolve of this.waiters.splice(0)) resolve();
  }

  private checkDrained(): void {
    if (this.onDrained !== undefined && this.inFlight === 0 && (this.outbox.length === 0 || this.abortWrites)) {
      const done = this.onDrained;
      this.onDrained = undefined;
      done();
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Shutdown
  // ───────────────────────────────────────────────────────────────────────────

  /** After `close`: let queued frames land, drop received-frame bookkeeping, remove our subtree. */
  private async finishClose(): Promise<void> {
    try {
      await new Promise<void>((resolve) => {
        const timer = this.clock.setTimeout(resolve, CLOSE_FLUSH_TIMEOUT_MS);
        this.onDrained = () => {
          this.clock.clearTimeout(timer);
          resolve();
        };
        this.checkDrained();
      });
      this.onDrained = undefined;
      this.clock.clearTimeout(this.deleteTimer);
      this.deleteTimer = undefined;
      if (this.toDelete.length > 0 && !this.deleting) await this.flushDeletes();
      await this.rtdb.remove(this.sendPath);
    } catch {
      // Best effort: the host also deletes the whole connection node when it ends.
    }
  }
}
