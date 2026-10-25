/**
 * JSON RPC over a {@link RemoteTransport}, independent of how the bytes travel.
 *
 * - request: `{id, method, params}` with a string `id`;
 * - response: `{id, result}` or `{id, error: {code, message}}`;
 * - notification: `{method, params}` without an `id`.
 */

import { systemClock, type Clock } from './clock.js';
import { RemoteProtocolError } from './errors.js';
import type { RemoteTransport } from './transport.js';
import {
  DATA_CHANNEL_DEAD_MS,
  DATA_CHANNEL_PING_INTERVAL_MS,
  RPC_DEFAULT_TIMEOUT_MS,
  TUNNEL_DEAD_MS,
  TUNNEL_PING_INTERVAL_MS,
} from './version.js';

/** The protocol's error codes, plus two that only ever arise locally. */
export type RemoteErrorCode =
  | 'not_approved'
  | 'not_pending'
  | 'capacity'
  | 'invalid_params'
  | 'unknown_session'
  | 'not_remote_enabled'
  | 'forbidden'
  | 'upgrade_required'
  | 'internal'
  /** Local: no response arrived within the request's timeout. */
  | 'timeout'
  /** Local: the peer or transport closed before the response arrived. */
  | 'closed';

/** Every {@link RemoteErrorCode}. */
export const REMOTE_ERROR_CODES: readonly RemoteErrorCode[] = [
  'not_approved',
  'not_pending',
  'capacity',
  'invalid_params',
  'unknown_session',
  'not_remote_enabled',
  'forbidden',
  'upgrade_required',
  'internal',
  'timeout',
  'closed',
];

/**
 * An RPC failure. Throw it from a request handler to answer with that `code`
 * and `message`; a rejected `request()` carries what the peer sent.
 */
export class RemoteRpcError extends Error {
  readonly code: RemoteErrorCode;

  constructor(code: RemoteErrorCode, message: string = code) {
    super(message);
    this.name = 'RemoteRpcError';
    this.code = code;
  }
}

/** Answers an inbound request: resolve with the result, or throw {@link RemoteRpcError}. */
export type RpcRequestHandler = (method: string, params: unknown) => Promise<unknown> | unknown;

/** Receives an inbound notification. */
export type RpcNotificationHandler = (method: string, params: unknown) => void;

/** Which transport's liveness timings {@link RpcPeer.startKeepalive} uses. */
export type KeepaliveKind = 'datachannel' | 'tunnel';

/** Options of {@link RpcPeer}. */
export interface RpcPeerOptions {
  /** Defaults to the system clock. */
  clock?: Clock;
  /** Default request timeout; 30 s if omitted. */
  timeoutMs?: number;
}

interface Pending {
  resolve(value: unknown): void;
  reject(error: Error): void;
  timer: unknown;
}

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function toCode(code: string): RemoteErrorCode {
  return (REMOTE_ERROR_CODES as readonly string[]).includes(code) ? (code as RemoteErrorCode) : 'internal';
}

/**
 * One end of an RPC conversation. Both ends can send requests and
 * notifications. Inbound requests are handled concurrently.
 *
 * Failure model: a thrown {@link RemoteRpcError} becomes an error response with
 * its code; any other exception becomes `internal` with a fixed message, so
 * nothing about the host leaks. Inbound text that is not valid JSON, or not a
 * request, response or notification, closes the peer with `protocol_error`.
 * When the transport closes, every pending request rejects with `closed`.
 *
 * Liveness: the `ping` notification is answered with `pong` here, and neither
 * reaches the application's notification handler. {@link RpcPeer.startKeepalive}
 * sends pings and gives up on a peer that has been silent too long.
 */
export class RpcPeer {
  private readonly clock: Clock;
  private readonly defaultTimeoutMs: number;
  private readonly pending = new Map<string, Pending>();
  private requestHandler: RpcRequestHandler | undefined;
  private notificationHandler: RpcNotificationHandler | undefined;
  private nextId = 0;
  private ended = false;
  private endReason = 'closed';
  private lastReceivedAt: number;
  private keepaliveTimer: unknown;

  constructor(
    private readonly transport: RemoteTransport,
    options: RpcPeerOptions = {},
  ) {
    this.clock = options.clock ?? systemClock;
    this.defaultTimeoutMs = options.timeoutMs ?? RPC_DEFAULT_TIMEOUT_MS;
    this.lastReceivedAt = this.clock.now();
    transport.onActivity?.(() => {
      this.lastReceivedAt = this.clock.now();
    });
    transport.onMessage((text) => this.receive(text));
    transport.onClose((reason) => this.end(reason));
  }

  /** `true` once the peer or its transport has closed. */
  get closed(): boolean {
    return this.ended;
  }

  /** Set the handler for inbound requests (replaces any earlier one). */
  onRequest(handler: RpcRequestHandler): void {
    this.requestHandler = handler;
  }

  /** Set the handler for inbound notifications (replaces any earlier one). */
  onNotification(handler: RpcNotificationHandler): void {
    this.notificationHandler = handler;
  }

  /**
   * Send a request and wait for its result. Rejects with {@link RemoteRpcError}:
   * the peer's code, `timeout` after `timeoutMs`, or `closed`.
   */
  request<T = unknown>(method: string, params?: unknown, options: { timeoutMs?: number } = {}): Promise<T> {
    if (this.ended) return Promise.reject(new RemoteRpcError('closed', this.endReason));
    let text: string;
    try {
      const id = String(++this.nextId);
      text = JSON.stringify(params === undefined ? { id, method } : { id, method, params });
      return new Promise<T>((resolve, reject) => {
        const timer = this.clock.setTimeout(() => {
          this.pending.delete(id);
          reject(new RemoteRpcError('timeout', `${method} timed out`));
        }, options.timeoutMs ?? this.defaultTimeoutMs);
        this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
        this.transport.send(text).catch((error: unknown) => {
          const entry = this.pending.get(id);
          if (entry === undefined) return;
          this.pending.delete(id);
          this.clock.clearTimeout(entry.timer);
          // A request the framing refuses (above 16 MiB) is the caller's fault; anything else means the transport is gone.
          entry.reject(
            !this.ended && error instanceof RemoteProtocolError
              ? new RemoteRpcError('invalid_params', error.message)
              : new RemoteRpcError('closed', this.endReason),
          );
        });
      });
    } catch (error) {
      return Promise.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }

  /**
   * Send a notification. Resolves once the transport has taken it; it also
   * resolves, without sending, if the peer is closed, since a notification
   * promises no delivery anyway. It rejects only for a message the framing
   * refuses (above 16 MiB), which the caller must handle rather than lose.
   */
  async notify(method: string, params?: unknown): Promise<void> {
    if (this.ended) return;
    const text = JSON.stringify(params === undefined ? { method } : { method, params });
    try {
      await this.transport.send(text);
    } catch (error) {
      if (!this.ended && error instanceof RemoteProtocolError) throw error;
      // Otherwise the transport is gone; see above.
    }
  }

  /**
   * Keep the link alive and notice when it is dead: send a `ping` notification
   * every 15 s (data channel) or 25 s (tunnel), and close the peer with
   * `keepalive_timeout` once nothing, not just a `pong`, has arrived for 45 s
   * or 75 s. Silence is counted from this call. Idempotent; stops with the peer.
   *
   * Call it on both ends. A long message holds back the pings queued behind
   * it, and the other end's pings are what show the link is alive meanwhile;
   * a transport that reports inbound fragments (see
   * {@link RemoteTransport.onActivity}) also keeps a long download from
   * looking like silence.
   */
  startKeepalive(kind: KeepaliveKind): void {
    if (this.ended || this.keepaliveTimer !== undefined) return;
    const intervalMs = kind === 'tunnel' ? TUNNEL_PING_INTERVAL_MS : DATA_CHANNEL_PING_INTERVAL_MS;
    const deadMs = kind === 'tunnel' ? TUNNEL_DEAD_MS : DATA_CHANNEL_DEAD_MS;
    this.lastReceivedAt = this.clock.now();
    const tick = (): void => {
      if (this.ended) return;
      const silentMs = this.clock.now() - this.lastReceivedAt;
      if (silentMs >= deadMs) {
        this.close('keepalive_timeout');
        return;
      }
      void this.notify('ping');
      // Pings keep their cadence; only the last wait is cut short, so the verdict falls exactly at the dead time.
      this.keepaliveTimer = this.clock.setTimeout(tick, Math.min(intervalMs, deadMs - silentMs));
    };
    this.keepaliveTimer = this.clock.setTimeout(tick, intervalMs);
  }

  /** Close the peer and its transport. Idempotent. */
  close(reason = 'closed'): void {
    this.transport.close(reason);
    this.end(reason);
  }

  private end(reason: string): void {
    if (this.ended) return;
    this.ended = true;
    this.endReason = reason;
    this.clock.clearTimeout(this.keepaliveTimer);
    this.keepaliveTimer = undefined;
    for (const entry of this.pending.values()) {
      this.clock.clearTimeout(entry.timer);
      entry.reject(new RemoteRpcError('closed', reason));
    }
    this.pending.clear();
  }

  private receive(text: string): void {
    if (this.ended) return;
    this.lastReceivedAt = this.clock.now();
    let message: unknown;
    try {
      message = JSON.parse(text);
    } catch {
      return this.close('protocol_error');
    }
    if (!isObject(message)) return this.close('protocol_error');
    const { id, method } = message;
    if (typeof method === 'string' && method.length > 0) {
      if (id === undefined) {
        if (method === 'ping') {
          void this.notify('pong');
          return;
        }
        if (method === 'pong') return;
        try {
          this.notificationHandler?.(method, message['params']);
        } catch {
          // A failing notification handler is the handler's problem, not the peer's.
        }
      } else if (typeof id === 'string') {
        void this.answer(id, method, message['params']);
      } else {
        this.close('protocol_error');
      }
    } else if (typeof id === 'string' && ('result' in message || 'error' in message)) {
      this.settle(id, message);
    } else {
      this.close('protocol_error');
    }
  }

  private settle(id: string, message: Json): void {
    const entry = this.pending.get(id);
    if (entry === undefined) return; // answered too late: already timed out
    this.pending.delete(id);
    this.clock.clearTimeout(entry.timer);
    const { error } = message;
    if (error === undefined || error === null) {
      entry.resolve(message['result']);
    } else if (isObject(error) && typeof error['code'] === 'string' && typeof error['message'] === 'string') {
      entry.reject(new RemoteRpcError(toCode(error['code']), error['message']));
    } else {
      entry.reject(new RemoteRpcError('internal', 'malformed error response'));
    }
  }

  private async answer(id: string, method: string, params: unknown): Promise<void> {
    let text: string;
    try {
      if (this.requestHandler === undefined) throw new RemoteRpcError('internal', 'no request handler');
      const result = await this.requestHandler(method, params);
      text = JSON.stringify({ id, result: result === undefined ? null : result });
    } catch (error) {
      const failure =
        error instanceof RemoteRpcError
          ? { code: error.code, message: error.message }
          : { code: 'internal', message: 'internal error' };
      text = JSON.stringify({ id, error: failure });
    }
    if (this.ended) return;
    try {
      await this.transport.send(text);
    } catch (error) {
      // Usually the transport closed while the handler ran, and the peer will see that itself. A result the
      // framing refuses (above 16 MiB) would leave the peer waiting for its timeout, so tell it instead.
      if (this.ended || !(error instanceof RemoteProtocolError)) return;
      await this.transport.send(JSON.stringify({ id, error: { code: 'internal', message: 'response too large' } })).catch(() => undefined);
    }
  }
}
