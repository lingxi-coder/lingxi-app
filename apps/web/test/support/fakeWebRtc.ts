import type { Bytes } from '@lingxi/bridge-client/remote';
import type { DataChannelLike, PeerConnectionFactory, PeerConnectionLike } from '../../src/remote/peerLink';

/**
 * A stand-in for the WebRTC stack. The viewer's fake connection and the fake
 * host's channel meet through the token the viewer puts in its offer SDP, so a
 * `PeerConnectionFactory` built here connects to whichever `FakeHost` is
 * listening, with the knobs a test needs to make the direct path fail.
 */

export const FAKE_SDP_ANSWER = 'v=0\r\na=fake-answer\r\n';

/** One end of a channel pair. */
export class FakeDataChannel implements DataChannelLike {
  readyState: RTCDataChannelState = 'connecting';
  bufferedAmount = 0;
  bufferedAmountLowThreshold = 0;
  binaryType: BinaryType = 'blob';
  onopen: ((event: Event) => void) | null = null;
  onclose: ((event: Event) => void) | null = null;
  onerror: ((event: RTCErrorEvent) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onbufferedamountlow: ((event: Event) => void) | null = null;

  peer: FakeDataChannel | undefined;
  /** Messages sent while `holding`, delivered by `release()`. */
  private readonly held: Bytes[] = [];
  /** When set, sent bytes stay "in the browser's buffer" until `release()`. */
  holding = false;
  readonly sent: Bytes[] = [];

  send(data: Bytes): void {
    if (this.readyState !== 'open') throw new Error(`data channel is ${this.readyState}`);
    const copy = data.slice();
    this.sent.push(copy);
    if (this.holding) {
      this.held.push(copy);
      this.bufferedAmount += copy.length;
      return;
    }
    this.deliver(copy);
  }

  /** Let held messages through and tell the sender the buffer drained. */
  release(): void {
    this.holding = false;
    for (const message of this.held.splice(0)) this.deliver(message);
    this.bufferedAmount = 0;
    queueMicrotask(() => this.onbufferedamountlow?.(new Event('bufferedamountlow')));
  }

  private deliver(message: Bytes): void {
    const peer = this.peer;
    if (peer === undefined) return;
    queueMicrotask(() => {
      if (peer.readyState === 'open') peer.onmessage?.({ data: message.buffer.slice(message.byteOffset, message.byteOffset + message.byteLength) } as MessageEvent);
    });
  }

  open(): void {
    if (this.readyState !== 'connecting') return;
    this.readyState = 'open';
    this.onopen?.(new Event('open'));
  }

  close(): void {
    if (this.readyState === 'closed') return;
    this.readyState = 'closed';
    const peer = this.peer;
    queueMicrotask(() => {
      this.onclose?.(new Event('close'));
      peer?.remoteClosed();
    });
  }

  /** The other end closed. */
  remoteClosed(): void {
    if (this.readyState === 'closed') return;
    this.readyState = 'closed';
    queueMicrotask(() => this.onclose?.(new Event('close')));
  }
}

export class FakePeerConnection implements PeerConnectionLike {
  iceGatheringState: RTCIceGatheringState = 'new';
  connectionState: RTCPeerConnectionState = 'new';
  localDescription: { readonly sdp: string } | null = null;
  onicegatheringstatechange: ((event: Event) => void) | null = null;
  onconnectionstatechange: ((event: Event) => void) | null = null;
  readonly channels: FakeDataChannel[] = [];
  closed = false;

  constructor(
    private readonly network: FakeWebRtcNetwork,
    readonly token: string,
  ) {}

  createDataChannel(label: string): DataChannelLike {
    const channel = new FakeDataChannel();
    this.channels.push(channel);
    this.network.labels.push(label);
    return channel;
  }

  async createOffer(): Promise<RTCSessionDescriptionInit> {
    return { type: 'offer', sdp: `v=0\r\na=fake-token:${this.token}\r\na=fingerprint:sha-256 AA:BB:CC\r\n` };
  }

  async setLocalDescription(description: RTCSessionDescriptionInit): Promise<void> {
    this.localDescription = { sdp: `${description.sdp ?? ''}a=candidate:1 1 udp 1 192.0.2.1 9 typ host\r\n` };
    if (this.network.neverFinishGathering) {
      this.iceGatheringState = 'gathering';
      return;
    }
    this.iceGatheringState = 'gathering';
    setImmediate(() => {
      this.iceGatheringState = 'complete';
      this.onicegatheringstatechange?.(new Event('icegatheringstatechange'));
    });
  }

  async setRemoteDescription(description: RTCSessionDescriptionInit): Promise<void> {
    if (this.network.rejectsAnswer) throw new Error('bad answer');
    if (description.sdp !== FAKE_SDP_ANSWER) throw new Error('not the fake answer');
    this.network.connect(this);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const channel of this.channels) channel.close();
  }

  fail(): void {
    this.connectionState = 'failed';
    this.onconnectionstatechange?.(new Event('connectionstatechange'));
  }
}

/** Creates fake connections and lets the fake host pick up their channels. */
export class FakeWebRtcNetwork {
  /** `false`: the direct path never comes up (the channel stays `connecting`). */
  opensChannel = true;
  /** `true`: the SDP never completes gathering, so the 3 s cap decides. */
  neverFinishGathering = false;
  /** `true`: `setRemoteDescription` throws. */
  rejectsAnswer = false;
  readonly labels: string[] = [];
  readonly connections: FakePeerConnection[] = [];
  private nextToken = 1;
  private readonly hostSides = new Map<string, FakeDataChannel>();

  /** The factory the viewer is given. */
  factory(): PeerConnectionFactory {
    return () => {
      const connection = new FakePeerConnection(this, `t${this.nextToken++}`);
      this.connections.push(connection);
      return connection;
    };
  }

  /** The token a viewer put in its offer SDP, if any. */
  static tokenOf(sdp: string | undefined): string | undefined {
    return /a=fake-token:(\S+)/u.exec(sdp ?? '')?.[1];
  }

  /** The host's end of the channel for an offer, ready to be wrapped in a link. */
  acceptOffer(sdp: string | undefined): FakeDataChannel | undefined {
    const token = FakeWebRtcNetwork.tokenOf(sdp);
    const connection = this.connections.find((candidate) => candidate.token === token);
    const viewerSide = connection?.channels[0];
    if (token === undefined || viewerSide === undefined) return undefined;
    const hostSide = new FakeDataChannel();
    hostSide.peer = viewerSide;
    viewerSide.peer = hostSide;
    this.hostSides.set(token, hostSide);
    return hostSide;
  }

  /** The viewer applied the answer: bring the channel up unless the test says not to. */
  connect(connection: FakePeerConnection): void {
    const hostSide = this.hostSides.get(connection.token);
    const viewerSide = connection.channels[0];
    if (hostSide === undefined || viewerSide === undefined || !this.opensChannel) return;
    setImmediate(() => {
      connection.connectionState = 'connected';
      hostSide.open();
      viewerSide.open();
    });
  }
}
