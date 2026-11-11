/**
 * The seam between byte-level links (a DataChannel, the RTDB tunnel) and the
 * message-level RPC: {@link FragmentLink} moves fragments, {@link MessageTransport}
 * turns them into whole JSON messages and back.
 */

import { encodeMessage, FRAGMENT_KIND, Reassembler } from './framing.js';
import { RemoteProtocolError } from './errors.js';
import { FRAGMENT_MAX_BYTES, MESSAGE_MAX_BYTES } from './version.js';

/** Local budgets, including the active send; they do not change the wire limits. */
const OUTBOUND_QUEUE_MAX_BYTES = 32 * 1024 * 1024;
const OUTBOUND_QUEUE_MAX_MESSAGES = 128;

export interface MessageTransportOptions {
  maxInboundMessageBytes?: number;
  /** Total queued fragment bytes, including each fragment's kind byte; defaults to 32 MiB. */
  maxOutboundQueueBytes?: number;
  /** Queued messages, including the active send; defaults to 128. */
  maxOutboundQueueMessages?: number;
}

interface OutboundMessage {
  text: string;
  maxFragmentBytes: number;
  bytes: number;
  settled: boolean;
  resolve(): void;
  reject(error: unknown): void;
}

/** TextEncoder's UTF-8 size, including replacement of unpaired surrogates, without allocating bytes. */
function encodedSize(text: string, maxFragmentBytes: number): number {
  if (!Number.isInteger(maxFragmentBytes) || maxFragmentBytes < 2 || maxFragmentBytes > FRAGMENT_MAX_BYTES) {
    throw new RangeError('maxFragmentBytes must be an integer from 2 to 16384');
  }
  if (text.length > MESSAGE_MAX_BYTES) throw new RemoteProtocolError('message exceeds the size limit');
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x80) bytes++;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff && text.charCodeAt(i + 1) >= 0xdc00 && text.charCodeAt(i + 1) <= 0xdfff) {
      bytes += 4;
      i++;
    } else bytes += 3;
    if (bytes > MESSAGE_MAX_BYTES) throw new RemoteProtocolError('message exceeds the size limit');
  }
  return bytes + Math.max(1, Math.ceil(bytes / (maxFragmentBytes - 1)));
}

/** An ordered, reliable stream of fragments (see `framing.ts`). */
export interface FragmentLink {
  /** Queue one fragment for the peer. Throws if the link is closed. */
  sendFragment(fragment: Uint8Array): void;
  /**
   * Receive the peer's fragments, in order. Single consumer: a later call
   * replaces the handler, and fragments arriving before one is set are lost,
   * so register it right after creating the link.
   */
  onFragment(callback: (fragment: Uint8Array) => void): void;
  /** Be told once when the link ends, whoever ended it; `reason` is a short machine-readable code. */
  onClose(callback: (reason: string) => void): void;
  /** End the link. Idempotent. */
  close(reason: string): void;
  /**
   * Backpressure: resolves when the link can take more fragments. Senders
   * `await` it before every fragment; absent means "always writable".
   */
  whenWritable?(): Promise<void>;
  /** Largest fragment this link wants, at most 16384 (the tunnel prefers 16374 to fit three per frame). */
  readonly maxFragmentBytes?: number;
}

/** Whole-message transport: what {@link RpcPeer} runs over. */
export interface RemoteTransport {
  /**
   * Send one message. Resolves once all its fragments are handed to the link
   * (after any backpressure wait); rejects if the transport is closed.
   * Concurrent calls are serialized, so messages never interleave. Exceeding
   * the local outbound queue budget closes the transport with `capacity`.
   */
  send(text: string): Promise<void>;
  /** Receive whole messages. Single consumer: a later call replaces the handler. */
  onMessage(callback: (text: string) => void): void;
  /**
   * Optional. Be told of every inbound fragment, including each piece of a message that is still arriving, so a
   * keepalive does not take a long transfer for silence. Single consumer: a later call replaces the handler.
   */
  onActivity?(callback: () => void): void;
  /** Be told once when the transport ends. */
  onClose(callback: (reason: string) => void): void;
  /** End the transport. Idempotent. */
  close(reason: string): void;
}

/**
 * Wraps a {@link FragmentLink} with message fragmentation and reassembly. A
 * peer protocol violation (bad fragment, oversize or invalid message) closes
 * the link with reason `protocol_error`.
 */
export class MessageTransport implements RemoteTransport {
  private readonly reassembler = new Reassembler();
  private readonly closeHandlers: Array<(reason: string) => void> = [];
  private readonly outbound: OutboundMessage[] = [];
  private readonly maxOutboundQueueBytes: number;
  private readonly maxOutboundQueueMessages: number;
  private messageHandler: ((text: string) => void) | undefined;
  private activityHandler: (() => void) | undefined;
  private queuedBytes = 0;
  private queuedMessages = 0;
  private active: OutboundMessage | undefined;
  private draining = false;
  private rejectWritable: ((error: Error) => void) | undefined;
  private closedReason: string | undefined;

  constructor(private readonly link: FragmentLink, private readonly options: MessageTransportOptions = {}) {
    const limit = options.maxInboundMessageBytes;
    if (limit !== undefined && (!Number.isSafeInteger(limit) || limit < 1 || limit > MESSAGE_MAX_BYTES)) throw new RangeError('invalid inbound message limit');
    this.maxOutboundQueueBytes = options.maxOutboundQueueBytes ?? OUTBOUND_QUEUE_MAX_BYTES;
    this.maxOutboundQueueMessages = options.maxOutboundQueueMessages ?? OUTBOUND_QUEUE_MAX_MESSAGES;
    if (!Number.isSafeInteger(this.maxOutboundQueueBytes) || this.maxOutboundQueueBytes < 1) throw new RangeError('invalid outbound byte limit');
    if (!Number.isSafeInteger(this.maxOutboundQueueMessages) || this.maxOutboundQueueMessages < 1) throw new RangeError('invalid outbound message limit');
    link.onFragment((fragment) => this.receive(fragment));
    link.onClose((reason) => this.ended(reason));
  }

  send(text: string): Promise<void> {
    if (this.closedReason !== undefined) return Promise.reject(new Error(`transport closed: ${this.closedReason}`));
    const maxFragmentBytes = this.link.maxFragmentBytes ?? FRAGMENT_MAX_BYTES;
    let bytes: number;
    try {
      bytes = encodedSize(text, maxFragmentBytes);
    } catch (error) {
      return Promise.reject(error);
    }
    if (this.queuedMessages >= this.maxOutboundQueueMessages || bytes > this.maxOutboundQueueBytes - this.queuedBytes) {
      // Do not queue a capacity response behind the work that exhausted capacity.
      this.close('capacity');
      return Promise.reject(new Error('transport closed: capacity'));
    }
    this.queuedBytes += bytes;
    this.queuedMessages++;
    return new Promise<void>((resolve, reject) => {
      this.outbound.push({ text, maxFragmentBytes, bytes, settled: false, resolve, reject });
      if (!this.draining) void this.drain();
    });
  }

  private async drain(): Promise<void> {
    this.draining = true;
    while (this.closedReason === undefined) {
      const message = this.outbound.shift();
      if (message === undefined) break;
      this.active = message;
      try {
        // The queue holds bounded strings. Only the send that can progress
        // allocates UTF-8 bytes and fragments.
        await this.whenWritable();
        if (this.closedReason !== undefined) throw new Error(`transport closed: ${this.closedReason}`);
        const fragments = encodeMessage(message.text, message.maxFragmentBytes);
        message.text = '';
        for (let i = 0; i < fragments.length; i++) {
          if (i > 0) await this.whenWritable();
          if (this.closedReason !== undefined) throw new Error(`transport closed: ${this.closedReason}`);
          this.link.sendFragment(fragments[i]!);
        }
        this.finish(message);
      } catch (error) {
        this.finish(message, { error });
      }
      this.active = undefined;
    }
    this.draining = false;
  }

  private whenWritable(): Promise<void> {
    const writable = this.link.whenWritable?.();
    if (this.closedReason !== undefined) return Promise.reject(new Error(`transport closed: ${this.closedReason}`));
    if (writable === undefined) return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      this.rejectWritable = reject;
      writable.then(() => {
        if (this.rejectWritable === reject) this.rejectWritable = undefined;
        resolve();
      }, (error: unknown) => {
        if (this.rejectWritable === reject) this.rejectWritable = undefined;
        reject(error);
      });
    });
  }

  private finish(message: OutboundMessage, failure?: { error: unknown }): void {
    if (message.settled) return;
    message.settled = true;
    message.text = '';
    this.queuedBytes -= message.bytes;
    this.queuedMessages--;
    if (failure === undefined) message.resolve();
    else message.reject(failure.error);
  }

  onMessage(callback: (text: string) => void): void {
    this.messageHandler = callback;
  }

  onActivity(callback: () => void): void {
    this.activityHandler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    const reason = this.closedReason;
    if (reason === undefined) this.closeHandlers.push(callback);
    else queueMicrotask(() => callback(reason));
  }

  close(reason: string): void {
    this.link.close(reason);
    this.ended(reason);
  }

  private receive(fragment: Uint8Array): void {
    if (this.closedReason !== undefined) return;
    this.activityHandler?.();
    let message: string | null;
    try {
      // A continuation without payload never advances the byte budget while
      // Reassembler retains another chunk. Keep bounded consumers bounded.
      if (this.options.maxInboundMessageBytes !== undefined && fragment.byteLength === 1 && fragment[0] !== FRAGMENT_KIND.whole) throw new RangeError('empty continuation');
      if (this.reassembler.pendingBytes + Math.max(0, fragment.byteLength - 1) > (this.options.maxInboundMessageBytes ?? MESSAGE_MAX_BYTES)) throw new RangeError('inbound message exceeds the size limit');
      message = this.reassembler.push(fragment);
    } catch {
      this.close('protocol_error');
      return;
    }
    if (message === null) return;
    try {
      this.messageHandler?.(message);
    } catch {
      // A throwing handler must not break reassembly of the messages behind it.
    }
  }

  private ended(reason: string): void {
    if (this.closedReason !== undefined) return;
    this.closedReason = reason;
    this.reassembler.reset();
    const error = new Error(`transport closed: ${reason}`);
    this.rejectWritable?.(error);
    this.rejectWritable = undefined;
    if (this.active !== undefined) this.finish(this.active, { error });
    for (const message of this.outbound.splice(0)) this.finish(message, { error });
    for (const handler of this.closeHandlers.splice(0)) {
      try {
        handler(reason);
      } catch {
        // Close handlers are independent of each other.
      }
    }
  }
}
