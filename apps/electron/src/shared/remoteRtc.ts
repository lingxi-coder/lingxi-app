/**
 * The messages between the main process and the hidden WebRTC page
 * (`renderer/remoteRtc/main.ts`) over one MessagePort. The page only moves
 * bytes; authentication and every decision stay in main. Both ends validate
 * whatever they receive with the parsers below and drop the rest.
 */

import { FRAGMENT_MAX_BYTES, MAX_HOST_CONNECTIONS, SDP_MAX_CHARS } from '@lingxi/bridge-client/remote';

export type RtcCandidateType = 'host' | 'srflx' | 'prflx' | 'relay';
export interface RtcIceServer { urls: string[]; username?: string; credential?: string }

export type RtcPeerState = 'connected' | 'failed' | 'closed';

/** main → page. `ice` is the STUN list the peer connection is built with. */
export type MainToPage =
  | { t: 'answer'; conn: string; sdp: string; ice: (string | RtcIceServer)[] }
  | { t: 'send'; conn: string; data: ArrayBuffer; lane?: 'files' }
  | { t: 'close'; conn: string }
  | { t: 'close_files'; conn: string };

/** page → main. `buffered`/`drain` are the DataChannel's backpressure edges. */
export type PageToMain =
  | { t: 'answered'; conn: string; sdp: string }
  | { t: 'error'; conn: string; message: string }
  | { t: 'state'; conn: string; lane?: 'files'; state: RtcPeerState; candidate?: RtcCandidateType; max_message_bytes?: number }
  | { t: 'data'; conn: string; lane?: 'files'; data: ArrayBuffer }
  | { t: 'buffered'; conn: string; lane?: 'files' }
  | { t: 'drain'; conn: string; lane?: 'files' };

/** Peer connections the page keeps at once. */
export const MAX_RTC_PEERS = MAX_HOST_CONNECTIONS;

const CONN = /^[A-Za-z0-9_-]{22}$/;
const ICE_URL = /^(?:stun|stuns|turn|turns):(?:[A-Za-z0-9.-]+|\[[0-9A-Fa-f:]+\])(?::[0-9]{1,5})?(?:\?transport=(?:udp|tcp))?$/;
const MAX_ICE_URLS = 8;
const MAX_ERROR_CHARS = 300;
const CANDIDATE_TYPES: readonly string[] = ['host', 'srflx', 'prflx', 'relay'];
const PEER_STATES: readonly string[] = ['connected', 'failed', 'closed'];

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);

/** `true` for a `conn_id` as the protocol mints them: 22 characters of base64url. */
export function isRtcConnId(value: unknown): value is string {
  return typeof value === 'string' && CONN.test(value);
}

/**
 * Bytes of an ArrayBuffer or a view of one, as an exact-size ArrayBuffer (a
 * view is copied; a Node `Buffer` is a view onto a shared pool). Works across
 * realms, which `instanceof` does not.
 */
export function rtcBytes(value: unknown): ArrayBuffer | undefined {
  if (Object.prototype.toString.call(value) === '[object ArrayBuffer]') return value as ArrayBuffer;
  if (ArrayBuffer.isView(value)) {
    const copy = new Uint8Array(value.byteLength);
    copy.set(new Uint8Array(value.buffer as ArrayBuffer, value.byteOffset, value.byteLength));
    return copy.buffer;
  }
  return undefined;
}

function fragment(value: unknown): ArrayBuffer | undefined {
  const bytes = rtcBytes(value);
  return bytes !== undefined && bytes.byteLength >= 1 && bytes.byteLength <= FRAGMENT_MAX_BYTES ? bytes : undefined;
}

function sdp(value: unknown): value is string {
  return typeof value === 'string' && value.length >= 1 && value.length <= SDP_MAX_CHARS;
}

/** Validate explicitly configured ICE servers. Legacy STUN URL arrays remain accepted. */
export function parseRtcIceServers(value: unknown): RtcIceServer[] | undefined {
  if (!Array.isArray(value) || value.length > MAX_ICE_URLS) return undefined;
  const result: RtcIceServer[] = [];
  let count = 0;
  for (const entry of value) {
    const server = typeof entry === 'string' ? { urls: [entry] } : entry;
    if (!isObject(server)) return undefined;
    const urls = typeof server['urls'] === 'string' ? [server['urls']] : server['urls'];
    if (!Array.isArray(urls) || urls.length === 0 || (count += urls.length) > MAX_ICE_URLS) return undefined;
    if (!urls.every(url => {
      if (typeof url !== 'string' || url.length > 280 || !ICE_URL.test(url)) return false;
      if (/^stuns?:/.test(url) && url.includes('?')) return false;
      if (url.startsWith('turns:') && url.endsWith('transport=udp')) return false;
      const port = /:(\d+)(?:\?|$)/.exec(url)?.[1];
      return port === undefined || (Number(port) >= 1 && Number(port) <= 65535);
    })) return undefined;
    const username = server['username'], credential = server['credential'];
    if ((username !== undefined || credential !== undefined) && (typeof username !== 'string' || !username || username.length > 512 || typeof credential !== 'string' || !credential || credential.length > 1024)) return undefined;
    if (urls.some(url => /^turns?:/.test(url)) && (typeof username !== 'string' || typeof credential !== 'string')) return undefined;
    result.push({ urls: [...urls] as string[], ...(username === undefined ? {} : { username: username as string, credential: credential as string }) });
  }
  return result;
}

/** Validate a message the page received; `undefined` for anything that is not one. */
export function parseMainToPage(value: unknown): MainToPage | undefined {
  if (!isObject(value) || !isRtcConnId(value['conn'])) return undefined;
  const conn = value['conn'];
  if (value['lane'] !== undefined && value['lane'] !== 'files') return undefined;
  const lane = value['lane'] === 'files' ? { lane: 'files' as const } : {};
  switch (value['t']) {
    case 'answer': {
      const ice = parseRtcIceServers(value['ice']);
      if (!sdp(value['sdp']) || !ice) return undefined;
      return { t: 'answer', conn, sdp: value['sdp'], ice: (value['ice'] as unknown[]).every(item => typeof item === 'string') ? [...value['ice'] as string[]] : ice };
    }
    case 'send': {
      const data = fragment(value['data']);
      return data === undefined ? undefined : { t: 'send', conn, data, ...lane };
    }
    case 'close_files':
      return { t: 'close_files', conn };
    case 'close':
      return { t: 'close', conn };
    default:
      return undefined;
  }
}

/** Validate a message main received from the page; `undefined` for anything that is not one. */
export function parsePageToMain(value: unknown): PageToMain | undefined {
  if (!isObject(value) || !isRtcConnId(value['conn'])) return undefined;
  const conn = value['conn'];
  if (value['lane'] !== undefined && value['lane'] !== 'files') return undefined;
  const lane = value['lane'] === 'files' ? { lane: 'files' as const } : {};
  switch (value['t']) {
    case 'answered':
      return sdp(value['sdp']) ? { t: 'answered', conn, sdp: value['sdp'] } : undefined;
    case 'error':
      return typeof value['message'] === 'string' ? { t: 'error', conn, message: value['message'].slice(0, MAX_ERROR_CHARS) } : undefined;
    case 'state': {
      const state = value['state'];
      const candidate = value['candidate'];
      if (typeof state !== 'string' || !PEER_STATES.includes(state)) return undefined;
      if (candidate !== undefined && (typeof candidate !== 'string' || !CANDIDATE_TYPES.includes(candidate))) return undefined;
      const message: PageToMain = { t: 'state', conn, state: state as RtcPeerState, ...lane };
      if (candidate !== undefined) message.candidate = candidate as RtcCandidateType;
      const limit = value['max_message_bytes'];
      if (limit !== undefined) {
        if (typeof limit !== 'number' || !Number.isSafeInteger(limit) || limit < 1 || limit > FRAGMENT_MAX_BYTES) return undefined;
        message.max_message_bytes = limit;
      }
      return message;
    }
    case 'data': {
      const data = fragment(value['data']);
      return data === undefined ? undefined : { t: 'data', conn, data, ...lane };
    }
    case 'buffered':
      return { t: 'buffered', conn, ...lane };
    case 'drain':
      return { t: 'drain', conn, ...lane };
    default:
      return undefined;
  }
}
