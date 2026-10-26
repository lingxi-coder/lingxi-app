import {
  DATA_CHANNEL_BUFFER_HIGH_BYTES,
  FRAGMENT_MAX_BYTES,
  type Bytes,
  type Clock,
  type FragmentLink,
} from '@lingxi/bridge-client/remote';

/**
 * The parts of `RTCDataChannel` the viewer uses. Written so the real object
 * satisfies it (handlers take DOM events), which lets tests substitute a fake
 * without a WebRTC stack.
 */
export interface DataChannelLike {
  readonly readyState: RTCDataChannelState;
  readonly bufferedAmount: number;
  bufferedAmountLowThreshold: number;
  binaryType: BinaryType;
  onopen: ((event: Event) => void) | null;
  onclose: ((event: Event) => void) | null;
  onerror: ((event: RTCErrorEvent) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onbufferedamountlow: ((event: Event) => void) | null;
  send(data: Bytes): void;
  close(): void;
}

/** The parts of `RTCPeerConnection` the viewer uses; see {@link DataChannelLike}. */
export interface PeerConnectionLike {
  readonly iceGatheringState: RTCIceGatheringState;
  readonly connectionState: RTCPeerConnectionState;
  readonly sctp?: { readonly maxMessageSize: number } | null;
  readonly localDescription: { readonly sdp: string } | null;
  onicegatheringstatechange: ((event: Event) => void) | null;
  onconnectionstatechange: ((event: Event) => void) | null;
  createDataChannel(label: string, options: RTCDataChannelInit): DataChannelLike;
  createOffer(): Promise<RTCSessionDescriptionInit>;
  setLocalDescription(description: RTCSessionDescriptionInit): Promise<void>;
  setRemoteDescription(description: RTCSessionDescriptionInit): Promise<void>;
  close(): void;
}

export type PeerConnectionFactory = (configuration: RTCConfiguration) => PeerConnectionLike;

/** `new RTCPeerConnection`, or `undefined` in a browser (or test runtime) without WebRTC. */
export function browserPeerConnectionFactory(): PeerConnectionFactory | undefined {
  if (typeof RTCPeerConnection === 'undefined') return undefined;
  return (configuration) => new RTCPeerConnection(configuration);
}

/**
 * Resolves when ICE gathering is `complete` or after `timeoutMs`, whichever is
 * first: the offer SDP must carry the candidates, since signaling is a single
 * write rather than trickled candidates.
 */
export function waitForIceGathering(peer: PeerConnectionLike, clock: Clock, timeoutMs: number): Promise<void> {
  return new Promise<void>((resolve) => {
    if (peer.iceGatheringState === 'complete') {
      resolve();
      return;
    }
    const timer = clock.setTimeout(finish, timeoutMs);
    peer.onicegatheringstatechange = () => {
      if (peer.iceGatheringState === 'complete') finish();
    };
    function finish(): void {
      clock.clearTimeout(timer);
      peer.onicegatheringstatechange = null;
      resolve();
    }
  });
}

/**
 * A DataChannel as a {@link FragmentLink}: one fragment per binary message. The
 * sender waits while more than `DATA_CHANNEL_BUFFER_HIGH_BYTES` are queued in
 * the browser and resumes on `bufferedamountlow`, so a large snapshot cannot
 * pile up in memory.
 */
export class DataChannelLink implements FragmentLink {
  get maxFragmentBytes(): number { return Math.min(FRAGMENT_MAX_BYTES, this.messageLimit()); }

  private handler: ((fragment: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  private readonly closeHandlers: Array<(reason: string) => void> = [];
  private readonly waiters: Array<() => void> = [];
  private closedReason: string | undefined;

  constructor(private readonly channel: DataChannelLike, private readonly messageLimit: () => number = () => FRAGMENT_MAX_BYTES) {
    channel.binaryType = 'arraybuffer';
    channel.bufferedAmountLowThreshold = Math.floor(DATA_CHANNEL_BUFFER_HIGH_BYTES / 2);
    channel.onmessage = (event) => this.receive(event.data);
    channel.onbufferedamountlow = () => this.release();
    channel.onclose = () => this.ended('channel_closed');
    channel.onerror = () => this.ended('channel_error');
  }

  sendFragment(fragment: Uint8Array): void {
    if (this.closedReason !== undefined) throw new Error(`data channel is closed: ${this.closedReason}`);
    this.channel.send(fragment as Bytes); // fragments are made on plain ArrayBuffers, which is what `send` takes
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.handler = callback;
    // Fragments that arrived before a consumer was set are handed over now, in order.
    for (const fragment of this.early.splice(0)) callback(fragment);
  }

  onClose(callback: (reason: string) => void): void {
    const reason = this.closedReason;
    if (reason === undefined) this.closeHandlers.push(callback);
    else queueMicrotask(() => callback(reason));
  }

  /** Be told once when the channel is open; at once if it already is. */
  onOpen(callback: () => void): void {
    if (this.channel.readyState === 'open') callback();
    else this.channel.onopen = () => callback();
  }

  whenWritable(): Promise<void> {
    if (this.closedReason !== undefined || this.channel.bufferedAmount < DATA_CHANNEL_BUFFER_HIGH_BYTES) return Promise.resolve();
    return new Promise<void>((resolve) => this.waiters.push(resolve));
  }

  close(reason: string): void {
    if (this.closedReason !== undefined) return;
    try {
      this.channel.close();
    } catch {
      // Closing a channel that is already gone changes nothing.
    }
    this.ended(reason);
  }

  private receive(data: unknown): void {
    if (this.closedReason !== undefined) return;
    let fragment: Uint8Array;
    if (data instanceof ArrayBuffer) fragment = new Uint8Array(data);
    else if (ArrayBuffer.isView(data)) fragment = new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    else {
      this.close('protocol_error'); // text and Blob messages are not part of the protocol
      return;
    }
    if (this.handler === undefined) this.early.push(fragment);
    else this.handler(fragment);
  }

  private release(): void {
    for (const resolve of this.waiters.splice(0)) resolve();
  }

  private ended(reason: string): void {
    if (this.closedReason !== undefined) return;
    this.closedReason = reason;
    this.channel.onmessage = null;
    this.channel.onbufferedamountlow = null;
    this.release();
    for (const handler of this.closeHandlers.splice(0)) {
      try {
        handler(reason);
      } catch {
        // Close handlers are independent of each other.
      }
    }
  }
}
