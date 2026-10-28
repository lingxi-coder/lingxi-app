import { randomBytes } from 'node:crypto';
import { MessageTransport } from '@lingxi/bridge-client/remote';
import { parseRtcIceServers, type RtcIceServer } from '../../shared/remoteRtc.js';
import type { PeerHost } from '../remoteControl/peerHost.js';
import type { RemoteRun } from './remoteGateway.js';
import { LOCAL_APP_CREDENTIALS_HEADER } from './serviceCookies.js';

const MAX_RESPONSE = 256 * 1024;
const MAX_IN_FLIGHT = 8;
const MAX_TOTAL = 1024 * 1024;
const TIMEOUT = 15_000;
const REQUEST_HEADERS = new Set(['accept', 'accept-language', 'if-none-match', 'if-modified-since']);
const RESPONSE_EXCLUDE = new Set(['set-cookie', 'set-cookie2', 'connection', 'transfer-encoding']);

export interface LocalAppRtcOptions {
  peerHost: PeerHost;
  resource(run: RemoteRun, request: Request): Promise<Response>;
  current(run: RemoteRun): Promise<boolean>;
  /** Trusted host configuration, never taken from the app or offer. */
  iceServers?: readonly RtcIceServer[];
  /** Overrides the request deadline, for hosts and deterministic tests. */
  requestTimeoutMs?: number;
}

/** One authenticated browser session owns at most one peer. */
export class LocalAppRtc {
  private peers = new Map<string, { close(): void }>();
  private ice: RtcIceServer[];
  constructor(private options: LocalAppRtcOptions) {
    const parsed = parseRtcIceServers(options.iceServers ?? []);
    if (!parsed) throw new Error('Invalid configured ICE servers');
    this.ice = parsed;
    if (options.requestTimeoutMs !== undefined && (!Number.isInteger(options.requestTimeoutMs) || options.requestTimeoutMs < 1 || options.requestTimeoutMs > TIMEOUT)) throw new Error('Invalid RTC request deadline');
  }
  async rtc(run: RemoteRun, request: Request, sessionId: string, signal: AbortSignal): Promise<Response> {
    if (signal.aborted || !await this.options.current(run)) return new Response(null, { status: 403 });
    let offer: unknown;
    try {
      const text = await request.text();
      if (text.length > 128_000) throw new Error();
      offer = JSON.parse(text);
    } catch { return new Response(null, { status: 400 }); }
    if (signal.aborted || !await this.options.current(run) || signal.aborted) return new Response(null, { status: 403 });
    if (offer && typeof offer === 'object' && (offer as { type?: unknown }).type === 'config') return Response.json({ iceServers: this.ice }, { headers: { 'cache-control': 'no-store' } });
    if (!offer || typeof offer !== 'object' || typeof (offer as { sdp?: unknown }).sdp !== 'string') return new Response(null, { status: 400 });
    this.peers.get(sessionId)?.close();
    const conn = randomBytes(16).toString('base64url');
    let link: ReturnType<PeerHost['link']> | undefined;
    const pending = new Map<number, AbortController>();
    // Undici's Request follows its input signal through a weak reference to
    // the Request's controller. Keep the Request alive until cancellation or
    // completion so a stalled upstream is still aborted after garbage collection.
    const pendingRequests = new Set<Request>();
    let active = 0, total = 0, closed = false;
    let resolveEnded!: () => void;
    const ended = new Promise<void>(resolve => { resolveEnded = resolve; });
    const close = () => {
      if (closed) return;
      closed = true;
      resolveEnded();
      for (const controller of pending.values()) controller.abort();
      link?.close('session_ended');
      signal.removeEventListener('abort', close);
      if (this.peers.get(sessionId)?.close === close) this.peers.delete(sessionId);
    };
    this.peers.set(sessionId, { close });
    signal.addEventListener('abort', close, { once: true });
    try {
      const answerPromise = this.options.peerHost.answer(conn, (offer as { sdp: string }).sdp, this.ice);
      void answerPromise.catch(() => undefined);
      link = this.options.peerHost.link(conn);
      if (closed || signal.aborted) { link.close('session_ended'); return new Response(null, { status: 403 }); }
      const transport = new MessageTransport(link, { maxInboundMessageBytes: 64 * 1024 });
      transport.onClose(close);
      transport.onMessage(text => { void (async () => {
        let data: { id?: unknown; cancel?: unknown; method?: unknown; path?: unknown; headers?: unknown };
        try { data = JSON.parse(text); } catch { transport.close('protocol_error'); return; }
        if (data && Number.isSafeInteger(data.id) && data.cancel === true) { pending.get(data.id as number)?.abort(); return; }
        if (!data || !Number.isSafeInteger(data.id) || pending.has(data.id as number) || typeof data.path !== 'string' || data.path.length > 4096 || !['GET', 'HEAD'].includes(String(data.method)) || active >= MAX_IN_FLIGHT) { transport.close('protocol_error'); return; }
        // URL parsing alone normalizes dot segments, so validate the original form too.
        const path = data.path;
        if (!path.startsWith('/') || path.startsWith('//') || /[\\\r\n\0#]/.test(path) || /%(?:2f|5c|00)/i.test(path) || /(?:^|\/)(?:\.|%2e){1,2}(?:\/|\?|$)/i.test(path) || /^\/__lingxi_remote(?:_|\/|\?|$)/i.test(path)) { transport.close('protocol_error'); return; }
        const headers = new Headers();
        if (!data.headers || typeof data.headers !== 'object' || Array.isArray(data.headers)) { transport.close('protocol_error'); return; }
        try {
          for (const [key, value] of Object.entries(data.headers)) {
            if (!REQUEST_HEADERS.has(key.toLowerCase()) || typeof value !== 'string' || value.length > 2048) throw new Error();
            headers.set(key, value);
          }
        } catch { transport.close('protocol_error'); return; }
        headers.set(LOCAL_APP_CREDENTIALS_HEADER, 'omit');
        active++;
        const controller = new AbortController(); pending.set(data.id as number, controller);
        const timer = setTimeout(() => { controller.abort(); close(); }, this.options.requestTimeoutMs ?? TIMEOUT);
        let size = 0;
        let resourceRequest: Request | undefined;
        let response: Response | undefined;
        try {
          const abort = new Promise<never>((_, reject) => controller.signal.addEventListener('abort', () => reject(new Error('aborted')), { once: true }));
          if (closed || controller.signal.aborted || !await Promise.race([this.options.current(run), abort]) || closed || controller.signal.aborted) throw new Error('expired');
          const url = new URL(path, request.url);
          if (url.origin !== new URL(request.url).origin) throw new Error('invalid origin');
          resourceRequest = new Request(url, { method: data.method as string, headers, signal: controller.signal });
          pendingRequests.add(resourceRequest);
          // The resource can resolve after cancellation wins the race. Its body
          // still needs an owner, even when the provider ignored the signal.
          const resource = this.options.resource(run, resourceRequest).then(result => {
            if ((closed || controller.signal.aborted) && result.body) void result.body.cancel().catch(() => undefined);
            return result;
          });
          response = await Promise.race([resource, abort]);
          if (response.status >= 300 && response.status < 400 && response.status !== 304) throw new Error('redirect');
          const type = response.headers.get('content-type')?.toLowerCase() ?? '';
          const length = Number(response.headers.get('content-length'));
          if (type.startsWith('text/event-stream') || type.startsWith('application/octet-stream') || /attachment/i.test(response.headers.get('content-disposition') ?? '') || length > MAX_RESPONSE) {
            void response.body?.cancel().catch(() => undefined);
            throw new Error('stream or download');
          }
          const output: Record<string, string> = {};
          let headerBytes = 0;
          response.headers.forEach((value, key) => { if (!RESPONSE_EXCLUDE.has(key.toLowerCase()) && (key.toLowerCase() !== 'content-length' || data.method === 'HEAD')) { headerBytes += key.length + value.length; if (headerBytes > 32_768) throw new Error('headers too large'); output[key] = value; } });
          const parts: Uint8Array[] = [];
          if (response.body && data.method !== 'HEAD') {
            const reader = response.body.getReader();
            try { for (;;) { const next = await Promise.race([reader.read(), abort]); if (next.done) break; size += next.value.byteLength; total += next.value.byteLength; if (size > MAX_RESPONSE || total > MAX_TOTAL) throw new Error('too large'); parts.push(next.value); } }
            finally { if (size > MAX_RESPONSE || total > MAX_TOTAL || controller.signal.aborted) void reader.cancel().catch(() => undefined); }
          }
          if (closed || controller.signal.aborted || !await Promise.race([this.options.current(run), abort]) || closed || controller.signal.aborted) throw new Error('expired');
          const body = Buffer.concat(parts.map(part => Buffer.from(part)), size).toString('base64');
          await transport.send(JSON.stringify({ id: data.id, status: response.status, statusText: response.statusText, headers: output, body }));
        } catch {
          const cancelled = controller.signal.aborted;
          controller.abort();
          if (response?.body && !response.body.locked) void response.body.cancel().catch(() => undefined);
          if (!closed && !cancelled) await transport.send(JSON.stringify({ id: data.id, fallback: true })).catch(() => close());
        } finally { total -= size; clearTimeout(timer); pending.delete(data.id as number); if (resourceRequest) pendingRequests.delete(resourceRequest); active--; }
      })(); });
      const result = await Promise.race([answerPromise.then(answer => ({ answer })), ended.then(() => undefined)]);
      if (!result) return new Response(null, { status: 403 });
      const answer = result.answer;
      if (closed || signal.aborted || !await this.options.current(run) || closed || signal.aborted) { close(); return new Response(null, { status: 403 }); }
      return Response.json({ sdp: answer, iceServers: this.ice }, { headers: { 'cache-control': 'no-store' } });
    } catch { close(); return new Response(null, { status: signal.aborted ? 403 : 503 }); }
  }
  /** Release the dedicated hidden window when no browser session has a peer. */
  releaseIdle(): void { if (this.peers.size === 0) this.options.peerHost.shutdown(); }

  /** The RTC adapter owns its dedicated peer host and hidden window. */
  dispose(): void { for (const peer of [...this.peers.values()]) peer.close(); this.options.peerHost.dispose(); }
}
