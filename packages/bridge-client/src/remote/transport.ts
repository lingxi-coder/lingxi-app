/**
 * The seam between byte-level links (a DataChannel, the RTDB tunnel) and the
 * message-level RPC: {@link FragmentLink} moves fragments, {@link MessageTransport}
 * turns them into whole JSON messages and back.
 */

import { encodeMessage, Reassembler } from './framing.js';

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
   * Concurrent calls are serialized, so messages never interleave.
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
  private messageHandler: ((text: string) => void) | undefined;
  private activityHandler: (() => void) | undefined;
  private tail: Promise<void> = Promise.resolve();
  private closedReason: string | undefined;

  constructor(private readonly link: FragmentLink) {
    link.onFragment((fragment) => this.receive(fragment));
    link.onClose((reason) => this.ended(reason));
  }

  send(text: string): Promise<void> {
    if (this.closedReason !== undefined) return Promise.reject(new Error(`transport closed: ${this.closedReason}`));
    let fragments: Uint8Array[];
    try {
      fragments = encodeMessage(text, this.link.maxFragmentBytes);
    } catch (error) {
      return Promise.reject(error);
    }
    const run = async (): Promise<void> => {
      for (const fragment of fragments) {
        await this.link.whenWritable?.();
        if (this.closedReason !== undefined) throw new Error(`transport closed: ${this.closedReason}`);
        this.link.sendFragment(fragment);
      }
    };
    const result = this.tail.then(run);
    this.tail = result.catch(() => undefined);
    return result;
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
    for (const handler of this.closeHandlers.splice(0)) {
      try {
        handler(reason);
      } catch {
        // Close handlers are independent of each other.
      }
    }
  }
}
