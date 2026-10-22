/**
 * A minimal Firebase Realtime Database client over REST and Server-Sent
 * Events, written against `fetch` so it runs unchanged in Node, the Electron
 * main process (inject `net.fetch`), the Electron renderer and browsers. No
 * Firebase SDK is involved: the caller supplies ID tokens.
 *
 * Everything else in the remote stack talks to the {@link Rtdb} interface, so
 * tests and other packages substitute {@link MemoryRtdb}.
 */

import { systemClock, type Clock } from './clock.js';
import { RemoteProtocolError } from './errors.js';
import { splitPath } from './rtdbTree.js';
import {
  SSE_BACKOFF_MAX_MS,
  SSE_BACKOFF_MIN_MS,
  SSE_BACKOFF_RESET_MS,
  SSE_IDLE_TIMEOUT_MS,
} from './version.js';

// ─────────────────────────────────────────────────────────────────────────────
// Interface
// ─────────────────────────────────────────────────────────────────────────────

/**
 * The write-time placeholder RTDB replaces with its own clock (ms since the
 * epoch). Use it for `created_at` and `last_seen`.
 */
export const SERVER_TIMESTAMP = Object.freeze({ '.sv': 'timestamp' } as const);

/** The type of {@link SERVER_TIMESTAMP}. */
export type ServerTimestamp = typeof SERVER_TIMESTAMP;

/**
 * One change notification, as in the RTDB streaming protocol. `path` is
 * relative to the listened location (`/` is the location itself).
 *
 * - `put`: the value at `path` is now `data` (`null` means deleted).
 * - `patch`: `data` is an object whose keys are child paths below `path`
 *   (possibly multi-segment) and whose values replace those children.
 */
export interface RtdbEvent {
  type: 'put' | 'patch';
  path: string;
  data: unknown;
}

/**
 * Why a subscription ended for good: `onError` is called once and nothing is retried.
 *
 * Two ways of being refused read access are told apart. A stream that cannot
 * be opened is an HTTP 401 `Permission denied`, but a 401 may only mean the
 * token expired, so it is tried once more at once with `getIdToken(true)`; a
 * second 401 is reported as `http`. A stream that was open and then lost its
 * read access gets `event: cancel` from the server, which is final as it
 * stands (a new token cannot change the rules) and is reported as `cancel`.
 */
export type RtdbListenError =
  /** The server cancelled the stream (the security rules no longer allow the read). */
  | { kind: 'cancel' }
  /** A request failed in a way retrying cannot fix (400, 401 after a fresh token, 403, 404). */
  | { kind: 'http'; status: number; message: string };

/** A live `listen`. */
export interface RtdbSubscription {
  /** Stop listening. Idempotent; nothing is delivered afterwards. */
  close(): void;
  /**
   * Drop the current stream and open a new one immediately, resetting the
   * backoff (after a network change or wake from sleep). The new stream starts
   * with a `put` at `/` carrying the whole subtree, exactly like any reconnect.
   */
  reconnect(): void;
}

/** The slice of the Realtime Database the remote stack uses. */
export interface Rtdb {
  /** Read the value at `path`; `null` when nothing is there. */
  get(path: string): Promise<unknown>;
  /** Replace the value at `path` (`null` deletes it). */
  put(path: string, value: unknown): Promise<void>;
  /** Replace the listed children of `path`, leaving its other children alone (`null` deletes one). */
  patch(path: string, value: Record<string, unknown>): Promise<void>;
  /** Delete `path`. */
  remove(path: string): Promise<void>;
  /**
   * Stream changes below `path`. The first event is always a `put` at `/` with
   * the current subtree (or `null`). The stream reconnects on its own, and
   * **every reconnect delivers that full-subtree `put` again**, so handlers
   * must be idempotent; {@link RtdbMirror} turns the repeat into a diff.
   * Handlers must not throw: an exception is swallowed so one bad event cannot
   * wedge the stream.
   */
  listen(path: string, onEvent: (event: RtdbEvent) => void, onError?: (error: RtdbListenError) => void): RtdbSubscription;
}

/** A failed RTDB request. `status` is the HTTP status, or 0 when no response arrived. */
export class RtdbError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'RtdbError';
    this.status = status;
  }
}

/**
 * The entries of an RTDB value as `[key, child]` pairs, whether RTDB returned
 * an object or an array. RTDB turns integer-like keys into arrays and renders
 * the gaps as `null`; those holes (and any other `null`) are skipped.
 */
export function childEntries(value: unknown): Array<[string, unknown]> {
  const entries: Array<[string, unknown]> = [];
  if (Array.isArray(value)) {
    value.forEach((child, index) => {
      if (child !== null && child !== undefined) entries.push([String(index), child]);
    });
  } else if (typeof value === 'object' && value !== null) {
    for (const [key, child] of Object.entries(value)) {
      if (child !== null && child !== undefined) entries.push([key, child]);
    }
  }
  return entries;
}

// ─────────────────────────────────────────────────────────────────────────────
// Server-Sent Events
// ─────────────────────────────────────────────────────────────────────────────

/** One dispatched SSE message. */
export interface SseMessage {
  /** The `event:` field (`message` when absent). */
  event: string;
  /** The `data:` lines joined with `\n`. */
  data: string;
}

const MAX_SSE_MESSAGE_CHARS = 32 * 1024 * 1024;

/**
 * Incremental SSE parser (WHATWG "Server-sent events" line rules). Feed it
 * text in chunks cut anywhere; `\n`, `\r\n` and a lone `\r` all end a line, and
 * a `\r\n` split across chunks counts once.
 */
export class SseParser {
  private partial = '';
  private skipLineFeed = false;
  private eventName = '';
  private dataLines: string[] = [];
  private dataChars = 0;

  /** Consume `chunk`; returns the messages it completed. */
  push(chunk: string): SseMessage[] {
    const messages: SseMessage[] = [];
    let text = chunk;
    if (this.skipLineFeed && text.length > 0) {
      if (text.charCodeAt(0) === 10) text = text.slice(1);
      this.skipLineFeed = false;
    }
    text = this.partial + text;
    let start = 0;
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      if (code !== 10 && code !== 13) continue;
      this.line(text.slice(start, i), messages);
      if (code === 13) {
        if (i + 1 < text.length) {
          if (text.charCodeAt(i + 1) === 10) i++;
        } else {
          this.skipLineFeed = true;
        }
      }
      start = i + 1;
    }
    this.partial = text.slice(start);
    if (this.partial.length + this.dataChars > MAX_SSE_MESSAGE_CHARS) {
      throw new RemoteProtocolError('SSE message is too large');
    }
    return messages;
  }

  private line(line: string, messages: SseMessage[]): void {
    if (line === '') {
      if (this.dataLines.length > 0) {
        messages.push({ event: this.eventName === '' ? 'message' : this.eventName, data: this.dataLines.join('\n') });
      }
      this.eventName = '';
      this.dataLines = [];
      this.dataChars = 0;
      return;
    }
    if (line.charCodeAt(0) === 58) return; // ':' comment
    const colon = line.indexOf(':');
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? '' : line.slice(colon + 1);
    if (value.charCodeAt(0) === 32) value = value.slice(1);
    if (field === 'event') {
      this.eventName = value;
    } else if (field === 'data') {
      this.dataLines.push(value);
      this.dataChars += value.length;
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// REST + SSE client
// ─────────────────────────────────────────────────────────────────────────────

/**
 * The `fetch` shape the client needs. Both the global `fetch` and Electron's
 * `net.fetch` fit, so the main process can route RTDB through Chromium's
 * network stack and the system proxy.
 */
export type FetchFn = (input: string, init?: RequestInit) => Promise<Response>;

/** Options of {@link RtdbClient}. */
export interface RtdbClientOptions {
  /** Instance URL, e.g. {@link DEFAULT_RTDB_URL}, or `http://127.0.0.1:9000` for the emulator. */
  databaseUrl: string;
  /**
   * Supplies the Firebase ID token (the `auth` query value). The emulator
   * accepts an unsigned token (`alg: none`) whose `sub` is the uid; its `owner`
   * admin token works only as an `Authorization: Bearer` header, not here.
   * Called for every request, so it should cache. `forceRefresh` is `true` on
   * the single retry after a 401 and after `auth_revoked`.
   */
  getIdToken: (forceRefresh?: boolean) => Promise<string>;
  /** Defaults to the global `fetch`. */
  fetch?: FetchFn;
  /** The emulator's `ns` query parameter. */
  namespace?: string;
  /** Defaults to the system clock. */
  clock?: Clock;
  /** Jitter source in [0, 1); defaults to `Math.random`. */
  random?: () => number;
  /** Give up on a REST request (not on the stream) after this long; default 30 s. */
  requestTimeoutMs?: number;
}

/** Fatal for a stream: retrying the same request cannot succeed. */
const FATAL_STATUS = new Set([400, 401, 403, 404]);

// An `auth_revoked` sooner than this after connecting is retried with backoff, not at once, so a server that
// revokes every new stream cannot make us spin.
const AUTH_REVOKED_MIN_UPTIME_MS = 5_000;

function redact(text: string): string {
  return text.replace(/(auth=)[^&\s"'<>]+/gi, '$1***');
}

async function discardBody(response: Response): Promise<void> {
  try {
    await response.body?.cancel();
  } catch {
    // The body is being thrown away; failing to cancel it changes nothing.
  }
}

async function httpError(response: Response): Promise<RtdbError> {
  let message = response.statusText || `HTTP ${response.status}`;
  try {
    const body = (await response.text()).slice(0, 2048);
    try {
      const parsed: unknown = JSON.parse(body);
      if (typeof parsed === 'object' && parsed !== null && typeof (parsed as { error?: unknown }).error === 'string') {
        message = (parsed as { error: string }).error;
      } else if (body) {
        message = body;
      }
    } catch {
      if (body) message = body;
    }
  } catch {
    // Keep the status text.
  }
  return new RtdbError(response.status, redact(message).slice(0, 300));
}

function networkError(error: unknown): RtdbError {
  const message = error instanceof Error ? error.message : String(error);
  return new RtdbError(0, redact(`network error: ${message}`).slice(0, 300));
}

/**
 * A {@link Rtdb} over the REST API: `GET/PUT/PATCH/DELETE {databaseUrl}/{path}.json?auth={token}`
 * and, for `listen`, the same URL with `Accept: text/event-stream`.
 *
 * Writes carry `print=silent`, so the server does not echo what was written
 * (tunnel frames are up to 64 KiB each, and RTDB bills download). The body of
 * a write is sent without a `Content-Type`, which a browser fills in as
 * `text/plain`, a safelisted type, so the CORS preflight that PUT, PATCH and
 * DELETE always trigger has to allow the method and no request headers; RTDB
 * parses the body as JSON regardless.
 */
export class RtdbClient implements Rtdb {
  private readonly base: string;
  private readonly getIdToken: RtdbClientOptions['getIdToken'];
  private readonly fetchFn: FetchFn;
  private readonly namespace: string | undefined;
  private readonly clock: Clock;
  private readonly random: () => number;
  private readonly requestTimeoutMs: number;

  constructor(options: RtdbClientOptions) {
    this.base = options.databaseUrl.replace(/\/+$/, '');
    this.getIdToken = options.getIdToken;
    this.fetchFn = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.namespace = options.namespace;
    this.clock = options.clock ?? systemClock;
    this.random = options.random ?? Math.random;
    this.requestTimeoutMs = options.requestTimeoutMs ?? 30_000;
  }

  async get(path: string): Promise<unknown> {
    return this.request('GET', path, undefined, undefined, async (response) => {
      try {
        return (await response.json()) as unknown;
      } catch (error) {
        throw networkError(error);
      }
    });
  }

  async put(path: string, value: unknown): Promise<void> {
    await this.write('PUT', path, body(value));
  }

  async patch(path: string, value: Record<string, unknown>): Promise<void> {
    await this.write('PATCH', path, body(value));
  }

  async remove(path: string): Promise<void> {
    await this.write('DELETE', path);
  }

  listen(path: string, onEvent: (event: RtdbEvent) => void, onError?: (error: RtdbListenError) => void): RtdbSubscription {
    const subscription = new SseSubscription(
      {
        open: (forceRefresh, signal) => this.open(path, forceRefresh, signal),
        clock: this.clock,
        random: this.random,
      },
      onEvent,
      onError,
    );
    subscription.start();
    return subscription;
  }

  private url(path: string, token: string, extra?: string): string {
    const segments = splitPath(path).map(encodeURIComponent).join('/');
    let url = `${this.base}/${segments}.json?auth=${encodeURIComponent(token)}`;
    if (this.namespace !== undefined) url += `&ns=${encodeURIComponent(this.namespace)}`;
    if (extra !== undefined) url += `&${extra}`;
    return url;
  }

  private async write(method: 'PUT' | 'PATCH' | 'DELETE', path: string, payload?: string): Promise<void> {
    await this.request(method, path, payload, 'print=silent', discardBody);
  }

  /**
   * One REST request, bounded by the request timeout. A 401 is retried once
   * with a freshly minted token; other failures surface as {@link RtdbError}.
   * `read` consumes the successful response while the timeout still applies.
   */
  private async request<T>(
    method: string,
    path: string,
    payload: string | undefined,
    extra: string | undefined,
    read: (response: Response) => Promise<T>,
  ): Promise<T> {
    for (let attempt = 0; ; attempt++) {
      const token = await this.getIdToken(attempt > 0);
      const controller = new AbortController();
      const timer = this.clock.setTimeout(() => controller.abort(), this.requestTimeoutMs);
      try {
        let response: Response;
        try {
          const init: RequestInit = { method, signal: controller.signal };
          if (payload !== undefined) init.body = payload;
          response = await this.fetchFn(this.url(path, token, extra), init);
        } catch (error) {
          throw networkError(error);
        }
        if (response.status === 401 && attempt === 0) {
          await discardBody(response);
          continue;
        }
        if (!response.ok) throw await httpError(response);
        return await read(response);
      } finally {
        this.clock.clearTimeout(timer);
      }
    }
  }

  private async open(path: string, forceRefresh: boolean, signal: AbortSignal): Promise<Response> {
    const token = await this.getIdToken(forceRefresh);
    return this.fetchFn(this.url(path, token), { method: 'GET', headers: { Accept: 'text/event-stream' }, signal });
  }
}

function body(value: unknown): string {
  const text = JSON.stringify(value);
  if (text === undefined) throw new TypeError('RTDB value is not JSON-serializable');
  return text;
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming subscription
// ─────────────────────────────────────────────────────────────────────────────

interface StreamHost {
  open(forceRefresh: boolean, signal: AbortSignal): Promise<Response>;
  clock: Clock;
  random: () => number;
}

type Next = 'backoff' | 'now' | 'stop';

/**
 * One SSE subscription with its reconnect loop. A stream that ends, errors or
 * stays silent for 90 s is reopened after 1 s doubling to 30 s (each delay
 * scaled by a random 0.5–1.0); the delay restarts at 1 s once a stream has
 * stayed up for 60 s. `auth_revoked` and a first 401 reopen at once with a
 * fresh token.
 */
class SseSubscription implements RtdbSubscription {
  private closed = false;
  private running = false;
  private attempt = 0;
  private forceRefresh = false;
  private reconnectNow = false;
  private controller: AbortController | undefined;
  private retryTimer: unknown;
  private idleTimer: unknown;

  constructor(
    private readonly host: StreamHost,
    private readonly onEvent: (event: RtdbEvent) => void,
    private readonly onError: ((error: RtdbListenError) => void) | undefined,
  ) {}

  start(): void {
    void this.loop();
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.host.clock.clearTimeout(this.retryTimer);
    this.host.clock.clearTimeout(this.idleTimer);
    this.controller?.abort();
  }

  reconnect(): void {
    if (this.closed) return;
    this.attempt = 0;
    if (this.running) {
      this.reconnectNow = true;
      this.controller?.abort();
    } else {
      this.host.clock.clearTimeout(this.retryTimer);
      void this.loop();
    }
  }

  private async loop(): Promise<void> {
    if (this.running || this.closed) return;
    this.running = true;
    try {
      for (;;) {
        const next = await this.connectOnce();
        if (this.closed || next === 'stop') return;
        if (this.reconnectNow) {
          this.reconnectNow = false;
          continue;
        }
        if (next === 'now') continue;
        const delay = this.backoffDelay();
        this.running = false;
        this.retryTimer = this.host.clock.setTimeout(() => void this.loop(), delay);
        return;
      }
    } finally {
      if (this.closed) this.running = false;
    }
  }

  private backoffDelay(): number {
    const base = Math.min(SSE_BACKOFF_MAX_MS, SSE_BACKOFF_MIN_MS * 2 ** this.attempt);
    this.attempt++;
    return base * (0.5 + 0.5 * this.host.random());
  }

  private armIdleTimer(controller: AbortController): void {
    this.host.clock.clearTimeout(this.idleTimer);
    this.idleTimer = this.host.clock.setTimeout(() => controller.abort(), SSE_IDLE_TIMEOUT_MS);
  }

  private fail(error: RtdbListenError): Next {
    this.closed = true;
    try {
      this.onError?.(error);
    } catch {
      // A throwing error handler must not escape the reconnect loop.
    }
    return 'stop';
  }

  /** Open one stream and read it to its end; says what to do next. */
  private async connectOnce(): Promise<Next> {
    const controller = new AbortController();
    this.controller = controller;
    const refreshed = this.forceRefresh;
    this.forceRefresh = false;
    let connectedAt: number | undefined;
    let next: Next = 'backoff';
    try {
      const response = await this.host.open(refreshed, controller.signal);
      if (this.closed) return 'stop';
      if (response.status === 401 && !refreshed) {
        await discardBody(response);
        this.forceRefresh = true;
        return 'now';
      }
      if (!response.ok) {
        const error = await httpError(response);
        if (FATAL_STATUS.has(error.status)) return this.fail({ kind: 'http', status: error.status, message: error.message });
        return 'backoff';
      }
      if (response.body === null) return 'backoff';
      connectedAt = this.host.clock.now();
      next = await this.pump(response.body, controller, connectedAt);
    } catch {
      // Network failure, token failure, our own abort: reconnect unless closed.
    } finally {
      this.host.clock.clearTimeout(this.idleTimer);
      controller.abort();
      if (connectedAt !== undefined && this.host.clock.now() - connectedAt >= SSE_BACKOFF_RESET_MS) this.attempt = 0;
    }
    return next;
  }

  private async pump(stream: ReadableStream<Uint8Array>, controller: AbortController, connectedAt: number): Promise<Next> {
    const reader = stream.getReader();
    // An abort (idle timeout, close, reconnect) must end the pending read even if the body is not wired to the signal.
    const stop = (): void => void reader.cancel().catch(() => undefined);
    if (controller.signal.aborted) stop();
    else controller.signal.addEventListener('abort', stop, { once: true });
    const decoder = new TextDecoder('utf-8');
    const parser = new SseParser();
    this.armIdleTimer(controller);
    for (;;) {
      const { done, value } = await reader.read();
      if (this.closed) return 'stop';
      if (done) return 'backoff';
      this.armIdleTimer(controller);
      for (const message of parser.push(decoder.decode(value, { stream: true }))) {
        if (this.closed) return 'stop'; // a handler may have closed us mid-chunk
        const next = this.dispatch(message, connectedAt);
        if (next !== undefined) return next;
      }
    }
  }

  /** Handle one message; returns what to do when it ends the stream. */
  private dispatch(message: SseMessage, connectedAt: number): Next | undefined {
    switch (message.event) {
      case 'put':
      case 'patch': {
        const event = parseEvent(message);
        if (event === null) return 'backoff'; // a stream that sends garbage is not trusted further
        try {
          this.onEvent(event);
        } catch {
          // See Rtdb.listen: handlers must not throw, and one that does is ignored.
        }
        return undefined;
      }
      case 'cancel':
        return this.fail({ kind: 'cancel' });
      case 'auth_revoked':
        this.forceRefresh = true;
        return this.host.clock.now() - connectedAt < AUTH_REVOKED_MIN_UPTIME_MS ? 'backoff' : 'now';
      default:
        return undefined; // keep-alive and anything unknown only prove liveness
    }
  }
}

function parseEvent(message: SseMessage): RtdbEvent | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(message.data);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const { path, data } = parsed as { path?: unknown; data?: unknown };
  if (typeof path !== 'string' || !path.startsWith('/')) return null;
  return { type: message.event === 'patch' ? 'patch' : 'put', path, data: data ?? null };
}
