/**
 * The hidden WebRTC page of Remote Control: the host's peer connections. It
 * only moves bytes (SDP in, SDP out, DataChannel messages both ways) over the
 * one MessagePort main hands it. It holds no keys, makes no requests of its own
 * (the CSP is `default-src 'none'`) and decides nothing; main validates every
 * message it receives from here, and this page validates every one it gets.
 */

import {
  DATA_CHANNEL_BUFFER_HIGH_BYTES,
  DATA_CHANNEL_LABEL,
  FILE_CHANNEL_LABEL,
  FRAGMENT_MAX_BYTES,
  ICE_GATHER_TIMEOUT_MS,
  SDP_MAX_CHARS,
} from '@lingxi/bridge-client/remote';

import { MAX_RTC_PEERS, parseMainToPage, type PageToMain, type RtcCandidateType } from '../../shared/remoteRtc';
import { REMOTE_RTC_PORT_MESSAGE } from '../../shared/remoteRtcPort';

// A paused sender is told to continue once the channel's queue is below this.
const BUFFER_LOW_BYTES = 256 * 1024;
// How long a channel main closed gets to deliver what it still holds.
const CLOSE_FLUSH_MS = 1_500;

interface Peer {
  conn: string;
  pc: RTCPeerConnection;
  channel: RTCDataChannel | undefined;
  paused: boolean;
  files?: RTCDataChannel;
  filePaused?: boolean;
}

const peers = new Map<string, Peer>();
let port: MessagePort | undefined;

// Only the port itself can be transferred to the main process; an ArrayBuffer is copied.
function post(message: PageToMain): void {
  try {
    port?.postMessage(message);
  } catch {
    // The port is gone, and so is main.
  }
}

const describe = (error: unknown): string => error instanceof Error ? error.message : String(error);

function release(peer: Peer): void {
  const { pc, channel } = peer;
  pc.onconnectionstatechange = null;
  pc.ondatachannel = null;
  pc.ontrack = null;
  if (channel) {
    channel.onmessage = null;
    channel.onclose = null;
    channel.onbufferedamountlow = null;
  }
  peer.files?.close();
  pc.close();
}

/** The peer failed on this side: forget it and tell main. */
function failPeer(peer: Peer, message: string): void {
  if (peers.get(peer.conn) !== peer) return;
  peers.delete(peer.conn);
  release(peer);
  post({ t: 'error', conn: peer.conn, message });
}

/** The peer ended without main asking: forget it and tell main, once. */
function lost(peer: Peer, state: 'failed' | 'closed'): void {
  if (peers.get(peer.conn) !== peer) return;
  peers.delete(peer.conn);
  release(peer);
  post({ t: 'state', conn: peer.conn, state });
}

/** Main closed the connection: let the channel deliver what it holds, then close the peer connection. */
function closeFromMain(conn: string): void {
  const peer = peers.get(conn);
  if (!peer) return;
  peers.delete(conn);
  const { pc, channel } = peer;
  pc.onconnectionstatechange = null;
  if (!channel || channel.readyState !== 'open') {
    release(peer);
    return;
  }
  channel.onmessage = null;
  channel.onbufferedamountlow = null;
  const finish = (): void => {
    clearTimeout(timer);
    channel.onclose = null;
    pc.close();
  };
  const timer = setTimeout(finish, CLOSE_FLUSH_MS);
  channel.onclose = finish;
  channel.close();
}

function gathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === 'complete') return Promise.resolve();
  return new Promise((resolve) => {
    const finish = (): void => {
      clearTimeout(timer);
      pc.removeEventListener('icegatheringstatechange', check);
      resolve();
    };
    const check = (): void => {
      if (pc.iceGatheringState === 'complete') finish();
    };
    const timer = setTimeout(finish, ICE_GATHER_TIMEOUT_MS);
    pc.addEventListener('icegatheringstatechange', check);
  });
}

/** The local candidate type of the selected pair, as `getStats` reports it. */
async function candidateType(pc: RTCPeerConnection): Promise<RtcCandidateType | undefined> {
  try {
    const reports = new Map<string, Record<string, unknown>>();
    (await pc.getStats()).forEach((report: Record<string, unknown>) => { reports.set(String(report['id']), report); });
    let pair: Record<string, unknown> | undefined;
    for (const report of reports.values()) {
      if (report['type'] === 'transport' && typeof report['selectedCandidatePairId'] === 'string') pair = reports.get(report['selectedCandidatePairId']);
    }
    if (!pair) {
      for (const report of reports.values()) {
        if (report['type'] === 'candidate-pair' && report['nominated'] === true && report['state'] === 'succeeded') pair = report;
      }
    }
    const local = typeof pair?.['localCandidateId'] === 'string' ? reports.get(pair['localCandidateId']) : undefined;
    const type = local?.['candidateType'];
    return type === 'host' || type === 'srflx' || type === 'prflx' || type === 'relay' ? type : undefined;
  } catch {
    return undefined;
  }
}

/** Take the viewer's DataChannel; anything but the one reliable, ordered channel of the protocol is closed. */
function adopt(peer: Peer, channel: RTCDataChannel): void {
  const bulk = channel.label === FILE_CHANNEL_LABEL;
  const lane = bulk ? { lane: 'files' as const } : {};
  const protocol = (channel.label === DATA_CHANNEL_LABEL || bulk) && channel.ordered
    && channel.maxRetransmits === null && channel.maxPacketLifeTime === null;
  if ((bulk ? peer.files !== undefined : peer.channel !== undefined) || !protocol) {
    channel.close();
    return;
  }
  if (bulk) peer.files = channel;
  else peer.channel = channel;
  channel.binaryType = 'arraybuffer';
  channel.bufferedAmountLowThreshold = BUFFER_LOW_BYTES;
  channel.onmessage = (event: MessageEvent) => {
    const data: unknown = event.data;
    // The protocol is binary fragments of 1 to 16 KiB; a peer that sends anything else is not a LingXi viewer.
    if (!(data instanceof ArrayBuffer) || data.byteLength < 1 || data.byteLength > FRAGMENT_MAX_BYTES) {
      if (bulk) channel.close();
      else failPeer(peer, 'the viewer sent a message that is not a fragment');
      return;
    }
    post({ t: 'data', conn: peer.conn, data, ...lane });
  };
  channel.onbufferedamountlow = () => {
    if (!(bulk ? peer.filePaused : peer.paused)) return;
    if (bulk) peer.filePaused = false; else peer.paused = false;
    post({ t: 'drain', conn: peer.conn, ...lane });
  };
  channel.onclose = () => {
    if (bulk) post({ t: 'state', conn: peer.conn, state: 'closed', ...lane });
    else lost(peer, 'closed');
  };
  const opened = (): void => {
    if (bulk) { post({ t: 'state', conn: peer.conn, state: 'connected', ...lane, max_message_bytes: Math.min(FRAGMENT_MAX_BYTES, peer.pc.sctp?.maxMessageSize || FRAGMENT_MAX_BYTES) }); return; }
    void candidateType(peer.pc).then((candidate) => {
      if (peers.get(peer.conn) !== peer) return;
      post(candidate === undefined
        ? { t: 'state', conn: peer.conn, state: 'connected' }
        : { t: 'state', conn: peer.conn, state: 'connected', candidate });
    });
  };
  if (channel.readyState === 'open') opened();
  else channel.onopen = opened;
}

async function answer(conn: string, offer: string, ice: string[]): Promise<void> {
  if (peers.has(conn) || peers.size >= MAX_RTC_PEERS) {
    post({ t: 'error', conn, message: 'no room for another peer connection' });
    return;
  }
  const pc = new RTCPeerConnection({ iceServers: ice.length > 0 ? [{ urls: ice }] : [] });
  const peer: Peer = { conn, pc, channel: undefined, paused: false };
  peers.set(conn, peer);
  pc.ondatachannel = (event) => adopt(peer, event.channel);
  // The protocol is one DataChannel; a viewer that negotiates media is not one of ours.
  pc.ontrack = () => failPeer(peer, 'the viewer offered media');
  pc.onconnectionstatechange = () => {
    if (pc.connectionState === 'failed') lost(peer, 'failed');
    else if (pc.connectionState === 'closed') lost(peer, 'closed');
  };
  try {
    await pc.setRemoteDescription({ type: 'offer', sdp: offer });
    await pc.setLocalDescription(await pc.createAnswer());
    await gathered(pc);
    if (peers.get(conn) !== peer) return;
    const sdp = pc.localDescription?.sdp;
    if (!sdp || sdp.length > SDP_MAX_CHARS) throw new Error('the answer is empty or too large');
    post({ t: 'answered', conn, sdp });
  } catch (error) {
    failPeer(peer, describe(error));
  }
}

function send(conn: string, data: ArrayBuffer, lane?: 'files'): void {
  const peer = peers.get(conn);
  const channel = lane === 'files' ? peer?.files : peer?.channel;
  if (!peer || !channel || channel.readyState !== 'open') return;
  try {
    channel.send(data);
  } catch (error) {
    failPeer(peer, `send failed: ${describe(error)}`);
    return;
  }
  if (!(lane === 'files' ? peer.filePaused : peer.paused) && channel.bufferedAmount > DATA_CHANNEL_BUFFER_HIGH_BYTES) {
    if (lane === 'files') peer.filePaused = true; else peer.paused = true;
    post({ t: 'buffered', conn, ...(lane ? { lane } : {}) });
  }
}

function receive(value: unknown): void {
  const message = parseMainToPage(value);
  if (message === undefined) return;
  switch (message.t) {
    case 'answer':
      void answer(message.conn, message.sdp, message.ice);
      return;
    case 'send':
      send(message.conn, message.data, message.lane);
      return;
    case 'close_files':
      peers.get(message.conn)?.files?.close();
      return;
    case 'close':
      closeFromMain(message.conn);
      return;
  }
}

// The preload posts the port into this world once; `event.source === window`
// says it came from the preload and not from a frame.
window.addEventListener('message', (event) => {
  if (event.source !== window || event.data !== REMOTE_RTC_PORT_MESSAGE || port !== undefined) return;
  const [received] = event.ports;
  if (!received) return;
  port = received;
  port.onmessage = (portEvent) => receive(portEvent.data);
});
