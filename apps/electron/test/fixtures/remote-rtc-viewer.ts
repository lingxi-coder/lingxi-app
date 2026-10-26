/**
 * The pretend phone of the real-Electron Remote Control test: a page with a
 * raw RTCPeerConnection offerer and the protocol's own MessageTransport over its
 * DataChannel. The test's main process drives it through `window.viewer`.
 */

import { DATA_CHANNEL_BUFFER_HIGH_BYTES, DATA_CHANNEL_LABEL, FILE_CHANNEL_LABEL, FileChannel, hashFile, MessageTransport, type FileOffer, type FragmentLink } from '@lingxi/bridge-client/remote';

const pattern = (seed: string, length: number): string => seed.repeat(Math.ceil(length / seed.length)).slice(0, length);

class ChannelLink implements FragmentLink {
  readonly maxFragmentBytes = 16384;
  private handler: ((fragment: Uint8Array) => void) | undefined;
  private readonly closers: Array<(reason: string) => void> = [];
  private reason: string | undefined;

  constructor(private readonly channel: RTCDataChannel) {
    channel.binaryType = 'arraybuffer';
    channel.bufferedAmountLowThreshold = 256 * 1024;
    channel.onmessage = (event) => this.handler?.(new Uint8Array(event.data as ArrayBuffer));
    channel.onclose = () => this.close('channel_closed');
  }

  sendFragment(fragment: Uint8Array): void {
    this.channel.send(fragment.slice());
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.handler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    if (this.reason === undefined) this.closers.push(callback);
    else callback(this.reason);
  }

  close(reason: string): void {
    if (this.reason !== undefined) return;
    this.reason = reason;
    for (const closer of this.closers.splice(0)) closer(reason);
    if (this.channel.readyState !== 'closed') this.channel.close();
  }

  whenWritable(): Promise<void> {
    if (this.channel.bufferedAmount <= DATA_CHANNEL_BUFFER_HIGH_BYTES) return Promise.resolve();
    return new Promise((resolve) => {
      this.channel.addEventListener('bufferedamountlow', () => resolve(), { once: true });
    });
  }
}

interface Session {
  pc: RTCPeerConnection;
  channel: RTCDataChannel;
  transport?: MessageTransport;
  bulk?: FileChannel;
  bulkReady?: boolean;
  fileOffset?: number;
  opened: boolean;
  closed: boolean;
  received: Array<{ length: number; ok: boolean }>;
}

const sessions = new Map<string, Session>();
const get = (name: string): Session => {
  const session = sessions.get(name);
  if (!session) throw new Error(`no session ${name}`);
  return session;
};

function gathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === 'complete') return Promise.resolve();
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, 3000);
    pc.addEventListener('icegatheringstatechange', () => {
      if (pc.iceGatheringState === 'complete') { clearTimeout(timer); resolve(); }
    });
  });
}

(window as unknown as { viewer: unknown }).viewer = {
  pattern,
  async offer(name: string, options: { ordered?: boolean; label?: string } = {}): Promise<string> {
    const pc = new RTCPeerConnection({ iceServers: [] });
    const channel = pc.createDataChannel(options.label ?? DATA_CHANNEL_LABEL, { ordered: options.ordered ?? true });
    const session: Session = { pc, channel, opened: false, closed: false, received: [] };
    channel.onopen = () => { session.opened = true; };
    if (options.ordered !== false && !options.label) {
      const bulk = pc.createDataChannel(FILE_CHANNEL_LABEL, { ordered: true });
      session.bulk = new FileChannel(new ChannelLink(bulk));
      bulk.onopen = () => { session.bulkReady = true; };
    }
    channel.addEventListener('close', () => { session.closed = true; });
    sessions.set(name, session);
    await pc.setLocalDescription(await pc.createOffer());
    await gathered(pc);
    return pc.localDescription!.sdp;
  },
  async accept(name: string, sdp: string): Promise<void> {
    await get(name).pc.setRemoteDescription({ type: 'answer', sdp });
  },
  status(name: string): { opened: boolean; closed: boolean; bulkReady?: boolean; fileOffset?: number; received: Array<{ length: number; ok: boolean }> } {
    const { opened, closed, received, bulkReady, fileOffset } = get(name);
    return { opened, closed, received, bulkReady, fileOffset };
  },
  async prepareFile(size: number, id: string): Promise<FileOffer> {
    const source = { size, read: async (at: number, n: number) => Uint8Array.from({ length: Math.min(size - at, n) }, (_, i) => (at+i)%251) };
    return { transfer_id: id, session_id: 'fixture', name: 'original.zip', size, sha256: await hashFile(source) };
  },
  async uploadFile(name: string, offer: FileOffer): Promise<unknown> {
    return get(name).bulk!.send(offer, { size: offer.size, read: async (at,n) => Uint8Array.from({ length: Math.min(offer.size-at,n) },(_,i)=>(at+i)%251) }, {
      offer: async () => ({ offset: 0 }), commit: async () => ({ ...offer, ref: offer.transfer_id }), cancel: async () => undefined,
    });
  },
  receiveFile(name: string, offer: FileOffer): void {
    const session = get(name); let offset = 0;
    session.bulk!.receive(offer.transfer_id, {
      get offset() { return offset; },
      write: async (at,bytes) => { if (at !== offset || bytes.some((b,i)=>b!==(at+i)%251)) throw new Error('corrupt original bytes'); offset += bytes.length; session.fileOffset = offset; },
      finish: async () => { if (offset !== offer.size) throw new Error('incomplete original'); return { ...offer, ref: offer.transfer_id }; },
      close: async () => undefined, cancel: async () => undefined,
    });
  },
  async finishFile(name: string, id: string): Promise<unknown> { return get(name).bulk!.commit(id); },
  startTransport(name: string, expectSeed: string, expectLength: number): void {
    const session = get(name);
    const transport = new MessageTransport(new ChannelLink(session.channel));
    transport.onMessage((text) => {
      session.received.push({ length: text.length, ok: text === pattern(expectSeed, expectLength) });
    });
    session.transport = transport;
  },
  async send(name: string, seed: string, length: number): Promise<void> {
    await get(name).transport!.send(pattern(seed, length));
  },
};
