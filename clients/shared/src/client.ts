/**
 * {@link BridgeClient} — the Node-side WebSocket client that drives the
 * bridge-server (`apps/bridge-server`) over the wire protocol in
 * `./protocol.ts`.
 *
 * Connection lifecycle (mirrors `bridge/src/mcp_endpoint.rs` +
 * `apps/bridge-server/src/server.rs`):
 *
 *   1. {@link BridgeClient.connect} opens `ws://127.0.0.1:<port>/mcp` with the
 *      `mcp` subprotocol and the lockfile `authToken` in the
 *      `X-LingXi-Ide-Authorization` header. The endpoint rejects a
 *      missing/mismatched token with HTTP 401 BEFORE the upgrade completes — so
 *      the token IS the auth (there is no separate post-handshake auth frame).
 *   2. It sends a `Frame::Request { method: "hello", params: ClientHello }` and
 *      awaits the matching `Frame::Response` carrying a {@link ServerHello}. A
 *      MAJOR-version mismatch in EITHER the bridge-envelope or the
 *      client-protocol version refuses the connection (the server replies with
 *      a `BridgeWireError` and bars subsequent commands), so the client
 *      verifies compatibility before proceeding.
 *   3. Subsequent commands ride as `Frame::Request { params: ClientCommand }`.
 *      Inbound `Frame::Event` / `Frame::PermissionRequest` / `Frame::ComputerAccessRequest`
 *      are surfaced through {@link BridgeClient.events} (an `AsyncIterable`) and via the
 *      `'event'` / `'permission'` / `'computerAccess'` listener callbacks.
 */

import { EventEmitter } from 'node:events';
import WebSocket from 'ws';

import {
  BRIDGE_PROTOCOL_VERSION,
  CLIENT_PROTOCOL_VERSION,
  type ClientCommand,
  type ClientEvent,
  type ComputerAccessRequestDto,
  type ComputerAccessResponseDto,
  type Frame,
  type ImageRefDto,
  type PermissionRequest,
  type PermissionResponseDto,
  type ServerHello,
} from './protocol.js';
import {
  discoverLatestLockfile,
  readLockfile,
  type Lockfile,
} from './lockfile.js';
import { versionCompatible } from './version.js';

/** Literal WS subprotocol the endpoint echoes back on a successful upgrade. */
const WS_SUBPROTOCOL = 'mcp';
/** Auth header the endpoint validates against the lockfile `authToken`. */
const AUTH_HEADER_NAME = 'X-LingXi-Ide-Authorization';

/** Options for constructing a {@link BridgeClient}. */
export interface BridgeClientOptions {
  /**
   * Path to a specific `<port>.lock` discovery file. If omitted, the newest
   * lockfile in `lockfileDir` (or `~/.lingxi/bridge`) is auto-discovered.
   */
  lockfilePath?: string;
  /** Directory to scan for lockfiles when `lockfilePath` is omitted. */
  lockfileDir?: string;
  /** Override the discovered host (defaults to `127.0.0.1`). */
  host?: string;
  /** Human-readable client identifier sent in the {@link ClientHello}. */
  clientName?: string;
  /** Handshake timeout in milliseconds (default 10_000). */
  handshakeTimeoutMs?: number;
}

/** Typed listener surface for a {@link BridgeClient}. */
export interface BridgeClientEvents {
  event: (event: ClientEvent) => void;
  permission: (request: PermissionRequest) => void;
  computerAccess: (request: ComputerAccessRequestDto) => void;
  close: (code: number, reason: string) => void;
  error: (err: Error) => void;
}

interface QueueWaiter {
  resolve: (result: IteratorResult<ClientEvent>) => void;
  reject: (err: Error) => void;
}

/**
 * A Node-side client for one bridge-server connection.
 *
 * Construct, then `await connect()`. After a successful connect, drive turns
 * with {@link sendPrompt} and consume the live feed via
 * `for await (const ev of client.events()) { … }` or the `'event'` listener.
 */
export class BridgeClient extends EventEmitter {
  private readonly opts: BridgeClientOptions;
  private ws: WebSocket | null = null;
  private lockfile: Lockfile | null = null;
  private serverHello: ServerHello | null = null;

  /** Monotonic correlation id for outbound `Frame::Request`s. */
  private nextRequestId = 1;

  /** Buffered inbound events + parked async-iterator waiters. */
  private readonly eventQueue: ClientEvent[] = [];
  private readonly eventWaiters: QueueWaiter[] = [];
  private streamClosed = false;

  /** Pending `Frame::Response` correlators keyed by request id. */
  private readonly pendingResponses = new Map<
    number,
    { resolve: (result: unknown) => void; reject: (err: Error) => void }
  >();

  constructor(opts: BridgeClientOptions = {}) {
    super();
    this.opts = opts;
  }

  // ── Typed event emitter overrides ──────────────────────────────────────────

  override on<E extends keyof BridgeClientEvents>(event: E, listener: BridgeClientEvents[E]): this {
    return super.on(event, listener as (...args: unknown[]) => void);
  }

  override once<E extends keyof BridgeClientEvents>(
    event: E,
    listener: BridgeClientEvents[E],
  ): this {
    return super.once(event, listener as (...args: unknown[]) => void);
  }

  override emit<E extends keyof BridgeClientEvents>(
    event: E,
    ...args: Parameters<BridgeClientEvents[E]>
  ): boolean {
    return super.emit(event, ...args);
  }

  // ── Lockfile discovery ──────────────────────────────────────────────────────

  /**
   * Resolve the lockfile this client will connect through — either the explicit
   * `lockfilePath`, or the newest lockfile under `lockfileDir`/`~/.lingxi/bridge`.
   * Throws if none is found.
   */
  resolveLockfile(): Lockfile {
    if (this.opts.lockfilePath) {
      return readLockfile(this.opts.lockfilePath);
    }
    const found = discoverLatestLockfile(this.opts.lockfileDir);
    if (!found) {
      const where = this.opts.lockfileDir ?? '~/.lingxi/bridge';
      throw new Error(`no bridge lockfile found under ${where}`);
    }
    return found;
  }

  /** The discovered/parsed lockfile (available after {@link connect}). */
  get discoveredLockfile(): Lockfile | null {
    return this.lockfile;
  }

  /** The verified {@link ServerHello} (available after {@link connect}). */
  get hello(): ServerHello | null {
    return this.serverHello;
  }

  // ── Connect + handshake ─────────────────────────────────────────────────────

  /**
   * Open the WebSocket, present the lockfile auth token, exchange hellos, and
   * verify version compatibility. Resolves once the {@link ServerHello} is
   * accepted; rejects on auth failure, transport error, or version mismatch.
   */
  async connect(): Promise<ServerHello> {
    const lockfile = this.resolveLockfile();
    this.lockfile = lockfile;

    const host = this.opts.host ?? '127.0.0.1';
    const url = `ws://${host}:${lockfile.port}/mcp`;

    const ws = new WebSocket(url, [WS_SUBPROTOCOL], {
      headers: { [AUTH_HEADER_NAME]: lockfile.body.authToken },
    });
    this.ws = ws;

    await new Promise<void>((resolve, reject) => {
      const onOpen = () => {
        ws.off('error', onError);
        resolve();
      };
      const onError = (err: Error) => {
        ws.off('open', onOpen);
        reject(new Error(`bridge connect failed: ${err.message}`));
      };
      ws.once('open', onOpen);
      ws.once('error', onError);
    });

    // Wire the steady-state message/close/error handlers now that we are open.
    ws.on('message', (data: WebSocket.RawData) => this.onMessage(data));
    ws.on('close', (code: number, reason: Buffer) => this.onClose(code, reason.toString()));
    ws.on('error', (err: Error) => this.emit('error', err));

    const hello = await this.handshake();
    this.serverHello = hello;
    return hello;
  }

  /** Send the {@link ClientHello} and verify the {@link ServerHello} reply. */
  private async handshake(): Promise<ServerHello> {
    const clientHello = {
      protocol_version: BRIDGE_PROTOCOL_VERSION,
      client_name: this.opts.clientName ?? 'lingxi-bridge-client/0.1.0',
      capabilities: {
        supports_streaming: true,
        supports_tools: true,
        supports_skills: true,
        supports_commands: true,
        client_protocol_version: CLIENT_PROTOCOL_VERSION,
      },
    };

    const result = await this.request('hello', clientHello, this.opts.handshakeTimeoutMs ?? 10_000);
    const hello = result as ServerHello;
    if (!hello || typeof hello.protocol_version !== 'string') {
      throw new Error('bridge handshake: malformed ServerHello');
    }

    if (!versionCompatible(BRIDGE_PROTOCOL_VERSION, hello.protocol_version)) {
      throw new Error(
        `bridge handshake: incompatible bridge protocol version (client ${BRIDGE_PROTOCOL_VERSION}, server ${hello.protocol_version})`,
      );
    }
    const serverClientProto = hello.capabilities?.client_protocol_version ?? '';
    if (!versionCompatible(CLIENT_PROTOCOL_VERSION, serverClientProto)) {
      throw new Error(
        `bridge handshake: incompatible client-protocol version (client ${CLIENT_PROTOCOL_VERSION}, server ${serverClientProto})`,
      );
    }
    return hello;
  }

  // ── Outbound frames ─────────────────────────────────────────────────────────

  /**
   * Send a `Frame::Request` and await the matching `Frame::Response`. The server
   * matches `method === "hello"` specially; for every other method it decodes
   * `params` as a {@link ClientCommand}, so the `method` string is informational
   * for non-hello requests.
   */
  private request(method: string, params: unknown, timeoutMs = 30_000): Promise<unknown> {
    const id = this.nextRequestId++;
    const frame: Frame = { type: 'request', payload: { id, method, params } };

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingResponses.delete(id);
        reject(new Error(`bridge request "${method}" (id=${id}) timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      this.pendingResponses.set(id, {
        resolve: (r) => {
          clearTimeout(timer);
          resolve(r);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });

      try {
        this.sendFrame(frame);
      } catch (err) {
        clearTimeout(timer);
        this.pendingResponses.delete(id);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }

  /** Fire-and-forget a {@link ClientCommand} as a `Frame::Request` (no reply awaited). */
  sendCommand(command: ClientCommand): void {
    const id = this.nextRequestId++;
    this.sendFrame({ type: 'request', payload: { id, method: command.type, params: command } });
  }

  private sendFrame(frame: Frame): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error('bridge client not connected');
    }
    this.ws.send(JSON.stringify(frame));
  }

  // ── High-level command helpers ──────────────────────────────────────────────

  /** Submit a user prompt to drive a turn ({@link ClientCommand} `send_prompt`). */
  sendPrompt(
    text: string,
    opts: { images?: ImageRefDto[]; turnId?: number } = {},
  ): void {
    const command: ClientCommand = {
      type: 'send_prompt',
      text,
      images: opts.images ?? [],
    };
    if (opts.turnId !== undefined) {
      command.turn_id = opts.turnId;
    }
    this.sendCommand(command);
  }

  /** Cancel the in-flight turn (optionally a specific `turnId`). */
  cancel(turnId?: number): void {
    const command: ClientCommand = { type: 'cancel' };
    if (turnId !== undefined) {
      command.turn_id = turnId;
    }
    this.sendCommand(command);
  }

  /** Approve a parked permission request, correlated by `request_id`. */
  approvePermission(
    requestId: number,
    response: PermissionResponseDto = { type: 'allow_once' },
  ): void {
    this.sendCommand({ type: 'approve_permission', request_id: requestId, response });
  }

  /** Deny a parked permission request, correlated by `request_id`. */
  denyPermission(requestId: number): void {
    this.sendCommand({ type: 'deny_permission', request_id: requestId });
  }

  /** Approve a parked `computer` tool `request_access` request, correlated by `request_id`. */
  approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): void {
    this.sendCommand({ type: 'approve_computer_access', request_id: requestId, response });
  }

  /** Deny a parked `computer` tool `request_access` request, correlated by `request_id`. */
  denyComputerAccess(requestId: number): void {
    this.sendCommand({ type: 'deny_computer_access', request_id: requestId });
  }

  // ── Inbound frames ──────────────────────────────────────────────────────────

  private onMessage(data: WebSocket.RawData): void {
    let frame: Frame;
    try {
      frame = JSON.parse(data.toString()) as Frame;
    } catch (err) {
      this.emit('error', new Error(`bridge: undecodable inbound frame: ${String(err)}`));
      return;
    }

    switch (frame.type) {
      case 'event':
        this.pushEvent(frame.payload);
        this.emit('event', frame.payload);
        break;
      case 'permission_request':
        this.emit('permission', frame.payload);
        break;
      case 'computer_access_request':
        this.emit('computerAccess', frame.payload);
        break;
      case 'response': {
        const { id, result, error } = frame.payload;
        const pending = this.pendingResponses.get(id);
        if (!pending) {
          break;
        }
        this.pendingResponses.delete(id);
        if (error) {
          pending.reject(new Error(`bridge response error ${error.code}: ${error.message}`));
        } else {
          pending.resolve(result ?? null);
        }
        break;
      }
      // `Frame::Request` is a server-direction violation when received inbound;
      // the server never sends one. Ignore (matches the Rust read loop).
      default:
        break;
    }
  }

  private onClose(code: number, reason: string): void {
    this.emit('close', code, reason);
    this.closeStream();
    for (const pending of this.pendingResponses.values()) {
      pending.reject(new Error(`bridge connection closed (code=${code})`));
    }
    this.pendingResponses.clear();
  }

  // ── Async event stream ──────────────────────────────────────────────────────

  private pushEvent(event: ClientEvent): void {
    const waiter = this.eventWaiters.shift();
    if (waiter) {
      waiter.resolve({ value: event, done: false });
    } else {
      this.eventQueue.push(event);
    }
  }

  private closeStream(): void {
    this.streamClosed = true;
    for (const waiter of this.eventWaiters) {
      waiter.resolve({ value: undefined, done: true });
    }
    this.eventWaiters.length = 0;
  }

  /**
   * The live inbound {@link ClientEvent} feed as an `AsyncIterable`. Iteration
   * ends when the connection closes. Buffered events are delivered in order;
   * one stream is shared across all consumers (an event goes to whoever is
   * waiting first).
   */
  events(): AsyncIterableIterator<ClientEvent> {
    const self = this;
    const iterator: AsyncIterableIterator<ClientEvent> = {
      next(): Promise<IteratorResult<ClientEvent>> {
        const buffered = self.eventQueue.shift();
        if (buffered !== undefined) {
          return Promise.resolve({ value: buffered, done: false });
        }
        if (self.streamClosed) {
          return Promise.resolve({ value: undefined, done: true });
        }
        return new Promise<IteratorResult<ClientEvent>>((resolve, reject) => {
          self.eventWaiters.push({ resolve, reject });
        });
      },
      return(): Promise<IteratorResult<ClientEvent>> {
        return Promise.resolve({ value: undefined, done: true });
      },
      [Symbol.asyncIterator]() {
        return this;
      },
    };
    return iterator;
  }

  // ── Teardown ────────────────────────────────────────────────────────────────

  /** Close the WebSocket connection. */
  close(): void {
    this.ws?.close();
  }
}
