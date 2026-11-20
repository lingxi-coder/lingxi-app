import { Agent, request as httpsRequest } from 'node:https';
import { lookup as dnsLookup } from 'node:dns';
import { BlockList, isIP } from 'node:net';

/**
 * The proxy's only way out. It calls https:443 on a host name, and connects only to the public address its own lookup
 * returned: the check and the connection use the same answer, so a host that resolves to a private address on a second
 * lookup (DNS rebinding) never reaches it. Redirects are not followed.
 */
const blocked = new BlockList();
for (const [network, prefix] of [['0.0.0.0', 8], ['10.0.0.0', 8], ['100.64.0.0', 10], ['127.0.0.0', 8], ['169.254.0.0', 16],
  ['172.16.0.0', 12], ['192.0.0.0', 24], ['192.0.2.0', 24], ['192.88.99.0', 24], ['192.168.0.0', 16], ['198.18.0.0', 15],
  ['198.51.100.0', 24], ['203.0.113.0', 24], ['224.0.0.0', 4], ['240.0.0.0', 4]]) blocked.addSubnet(network, prefix, 'ipv4');
for (const [network, prefix] of [['::', 128], ['::1', 128], ['::', 96], ['100::', 64], ['2001::', 32], ['2001:db8::', 32], ['2002::', 16],
  ['fc00::', 7], ['fe80::', 10], ['fec0::', 10], ['ff00::', 8]]) blocked.addSubnet(network, prefix, 'ipv6');

/** Whether `address` is a public unicast address. IPv4-mapped and NAT64 addresses are judged by the IPv4 inside. */
export function isPublicAddress(address) {
  const family = isIP(address);
  if (family === 4) return !blocked.check(address, 'ipv4');
  if (family !== 6) return false;
  const lower = address.toLowerCase();
  const embedded = /^(?:::ffff:|64:ff9b::)(\d+\.\d+\.\d+\.\d+)$/u.exec(lower)?.[1]
    ?? (/^(?:::ffff:|64:ff9b::)([0-9a-f]{1,4}):([0-9a-f]{1,4})$/u.exec(lower) ? hexV4(lower) : undefined);
  if (embedded) return isPublicAddress(embedded);
  return !blocked.check(address, 'ipv6');
}
const hexV4 = text => {
  const [, high, low] = /:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/u.exec(text);
  const value = (parseInt(high, 16) << 16 >>> 0) + parseInt(low, 16);
  return [value >>> 24, (value >>> 16) & 255, (value >>> 8) & 255, value & 255].join('.');
};

export class EgressError extends Error {
  constructor(code) { super(code); this.code = code; }
}

/** A `lookup` for https.request that answers only with a public address. */
export function publicLookup(lookup = dnsLookup) {
  return (hostname, options, callback) => {
    lookup(hostname, { all: true, verbatim: true }, (error, addresses) => {
      if (error) { callback(new EgressError('UNAVAILABLE')); return; }
      const list = Array.isArray(addresses) ? addresses : [{ address: addresses, family: isIP(addresses) }];
      if (!list.length || list.some(entry => !isPublicAddress(entry.address))) { callback(new EgressError('EGRESS_DENIED')); return; }
      // Every address passed the check, so connection racing (autoSelectFamily) may try any of them.
      const family = options?.family === 4 || options?.family === 6 ? options.family : 0;
      const usable = family ? list.filter(entry => entry.family === family) : list;
      if (!usable.length) { callback(new EgressError('UNAVAILABLE')); return; }
      if (options?.all) callback(null, usable); else callback(null, usable[0].address, usable[0].family);
    });
  };
}

/** Whether a URL may be called at all, before any lookup. */
export function checkUrl(value) {
  let url;
  try { url = new URL(value); } catch { throw new EgressError('EGRESS_DENIED'); }
  if (url.protocol !== 'https:' || (url.port && url.port !== '443') || url.username || url.password || isIP(url.hostname.replace(/^\[|\]$/gu, ''))
    || !url.hostname.includes('.')) throw new EgressError('EGRESS_DENIED');
  return url;
}

/**
 * The total is below llmApi's 300 s function timeout, so a slow answer ends with a TIMEOUT event rather than a cut
 * connection; reasoning models can think for minutes before their first byte, hence the long idle limit.
 */
export const LIMITS = { connectMs: 15_000, idleMs: 180_000, totalMs: 280_000, maxBytes: 32 * 1024 * 1024 };

/** Pooled connections: a reused socket is one this module already connected to a checked public address. */
const pool = new Agent({ keepAlive: true, maxSockets: 64, maxFreeSockets: 16, timeout: 60_000 });

/**
 * POSTs `body` to `url` and resolves with the status and an async iterable of the response body. `signal` aborts it;
 * the connect, idle and total limits end it with TIMEOUT. Never reads more than `maxBytes`.
 */
export function upstreamTransport({ lookup, request = httpsRequest, limits = LIMITS, agent = pool } = {}) {
  const resolve = publicLookup(lookup);
  return ({ url, headers, body, signal }) => new Promise((done, reject) => {
    let target;
    try { target = checkUrl(url); } catch (error) { reject(error); return; }
    if (signal?.aborted) { reject(new EgressError('ABORTED')); return; }
    let settled = false, connected = false, reason;
    const kill = code => { reason ??= code; req.destroy(new EgressError(reason)); };
    const fail = error => { if (!settled) { settled = true; reject(error); } };
    const req = request(target, { method: 'POST', headers: { ...headers, 'Content-Length': Buffer.byteLength(body) }, lookup: resolve, agent });
    const total = setTimeout(() => kill('TIMEOUT'), limits.totalMs);
    const connect = setTimeout(() => { if (!connected) kill('TIMEOUT'); }, limits.connectMs);
    const onAbort = () => kill('ABORTED');
    const cleanup = () => { clearTimeout(total); clearTimeout(connect); signal?.removeEventListener('abort', onAbort); };
    signal?.addEventListener('abort', onAbort, { once: true });
    // The request's own timeout follows the socket only while this request holds it, so a pooled socket carries no stale timer.
    req.setTimeout(limits.idleMs, () => kill('TIMEOUT'));
    req.on('socket', socket => {
      if (!socket.connecting) { connected = true; clearTimeout(connect); return; }
      socket.once('secureConnect', () => { connected = true; clearTimeout(connect); });
    });
    req.on('error', error => { cleanup(); fail(new EgressError(reason ?? (error instanceof EgressError ? error.code : 'UNAVAILABLE'))); });
    req.on('response', res => {
      connected = true; clearTimeout(connect); settled = true;
      res.on('close', cleanup);
      done({ status: res.statusCode ?? 0, headers: res.headers, body: limited(res, limits.maxBytes, kill, () => reason) });
    });
    req.end(body);
    // The body and the key-bearing headers are not needed past here; the closures above outlive the request.
    body = headers = undefined;
  });
}

async function* limited(stream, maxBytes, kill, reason) {
  let bytes = 0;
  try {
    for await (const chunk of stream) {
      bytes += chunk.length;
      if (bytes > maxBytes) { kill('TOO_LARGE'); break; }
      yield chunk;
    }
  } catch { throw new EgressError(reason() ?? 'UNAVAILABLE'); }
  if (reason()) throw new EgressError(reason());
}
