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
  MAX_BRIDGE_FRAME_BYTES,
  type AudioCapabilitySnapshotDto,
  type ClientCommand,
  type ClientEvent,
  type ComputerAccessRequestDto,
  type ComputerAccessResponseDto,
  type Frame,
  type ImageRefDto,
  type PermissionRequest,
  type PermissionResponseDto,
  type ServerHello,
  type VisualizationMountDto,
  type VisualizationRefDto,
  type VisualizationRequest,
  type VisualizationRevisionDto,
  type VisualizationServeDto,
  type VisualizationStateWriteDto,
  type VisualizationThemeDto,
} from './protocol.js';
import {
  validateClientEvent,
  validatePermissionScope,
  validateRuntimeSnapshot,
  validateServerHello,
  validateVisualizationList,
  validateVisualizationMount,
  validateVisualizationServe,
  validateVisualizationStateWrite,
} from './validation.js';
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
  /** Initial device capabilities; omitted until the device service is known. */
  audioCapabilities?: AudioCapabilitySnapshotDto;
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
  owner: symbol;
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
/** What an outbound frame is FOR, for the size error's message. */
function frameMethod(frame: Frame): string {
  const payload = (frame as { payload?: { method?: unknown } }).payload;
  return typeof payload?.method === 'string' ? payload.method : frame.type;
}

/**
 * The error an outbound frame too large for the engine's reader must produce
 * HERE, or `null` when it fits.
 *
 * Sending it anyway is not a rejected command: the engine's WebSocket read
 * yields `Err(Capacity(MessageTooLong))`, `run_frame_pump` breaks, and
 * `BridgeConnection::close_connection` aborts the active turn and drains every
 * broker — every session on the connection dies because one payload was
 * oversized. Refusing it here costs the one command instead.
 *
 * This is a backstop, not the place a caller should discover its limits: a
 * payload bound derived from {@link MAX_BRIDGE_FRAME_BYTES} (see
 * `apps/electron/src/shared/audioResponse.ts`) lets the caller answer with a
 * real, typed failure long before a frame gets here.
 */
export function frameSizeError(serialized: string, method: string): Error | null {
  const bytes = Buffer.byteLength(serialized, 'utf8');
  if (bytes <= MAX_BRIDGE_FRAME_BYTES) return null;
  return new Error(
    `bridge frame for "${method}" is too large to send: ${bytes} bytes, `
    + `over the engine's ${MAX_BRIDGE_FRAME_BYTES}-byte read limit`,
  );
}

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
  private readonly eventStreams = new Set<symbol>();
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
    try {
      return await this.connectInternal();
    } catch (error) {
      this.closeStream();
      // A rejected hello must not leave an incompatible open connection.
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.close();
      throw error;
    }
  }

  private async connectInternal(): Promise<ServerHello> {
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
        ...(this.opts.audioCapabilities === undefined ? {} : { audio: this.opts.audioCapabilities }),
      },
    };

    const result = await this.request('hello', clientHello, this.opts.handshakeTimeoutMs ?? 10_000);
    let hello: ServerHello;
    try {
      hello = validateServerHello(result);
    } catch (error) {
      throw new Error(`bridge handshake: malformed ServerHello (${error instanceof Error ? error.message : String(error)})`);
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
   * matches `hello` and product scope/snapshot requests specially;
   * other methods decode `params` as a {@link ClientCommand}.
   */
  private request(
    method: string,
    params: unknown,
    timeoutMs = 30_000,
    accept?: (result: unknown) => void,
  ): Promise<unknown> {
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
          try {
            // Apply authoritative snapshots inside the response boundary,
            // before later live frames can overtake a Promise continuation.
            accept?.(r);
            resolve(r);
          } catch (error) {
            reject(error instanceof Error ? error : new Error(String(error)));
          }
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

  /** Read the authenticated product runtime roster as one correlated response. */
  requestRuntimeSnapshot(accept?: (events: readonly ClientEvent[]) => void): Promise<ClientEvent[]> {
    let events: ClientEvent[] = [];
    return this.request('desktop_runtime_snapshot', { type: 'list_session_agents' }, 30_000, (result) => {
      events = validateRuntimeSnapshot(result);
      accept?.(events);
    }).then(() => events);
  }

  /** Read immutable SDK ownership while this permission ask is still pending. */
  async requestPermissionScope(requestId: number): Promise<{
    request_id: number; background_owned: boolean;
  } | null> {
    if (!Number.isSafeInteger(requestId) || requestId < 0) throw new Error('invalid permission request id');
    return validatePermissionScope(
      await this.request('permission_request_scope', { request_id: requestId }), requestId,
    );
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
    const serialized = JSON.stringify(frame);
    // Last line of defence, deliberately BEFORE the socket: see
    // `frameSizeError` for why an oversize frame costs the whole connection.
    const oversize = frameSizeError(serialized, frameMethod(frame));
    if (oversize) throw oversize;
    if (frameMethod(frame) === 'realtime_audio_input' && this.ws.bufferedAmount + Buffer.byteLength(serialized) > 512 * 1024) {
      throw new Error('Realtime audio input queue is full; restart the audio session');
    }
    this.ws.send(serialized);
  }

  // ── High-level command helpers ──────────────────────────────────────────────

  /** Submit a user prompt to drive a turn ({@link ClientCommand} `send_prompt`). */
  sendPrompt(
    text: string,
    opts: { images?: ImageRefDto[]; turnId?: number; visualizationContext?: VisualizationRefDto } = {},
  ): void {
    const command: ClientCommand = {
      type: 'send_prompt',
      text,
      images: opts.images ?? [],
    };
    if (opts.turnId !== undefined) {
      command.turn_id = opts.turnId;
    }
    if (opts.visualizationContext !== undefined) {
      command.visualization_context = opts.visualizationContext;
    }
    this.sendCommand(command);
  }

  // ── Inline visualization host (correlated `visualization` requests) ─────────

  private visualization(params: VisualizationRequest, timeoutMs = 15_000): Promise<unknown> {
    return this.request('visualization', params, timeoutMs);
  }

  /** Authorize a mount; `null` means the reference is unavailable here. */
  async visualizationMount(
    sessionId: string,
    reference: VisualizationRefDto,
    theme: VisualizationThemeDto,
    locale: string,
    expanded: boolean,
  ): Promise<VisualizationMountDto | null> {
    return validateVisualizationMount(await this.visualization({
      op: 'mount',
      session_id: sessionId,
      id: reference.id,
      revision: reference.revision,
      theme,
      locale,
      expanded,
    }));
  }

  /** Answer one request of the visualization origin (shell, asset or document). */
  async visualizationServe(path: string): Promise<VisualizationServeDto> {
    return validateVisualizationServe(await this.visualization({ op: 'serve', path }));
  }

  /** Compare-and-swap a state write from a live mount. */
  async visualizationWriteState(
    token: string,
    generation: number,
    baseVersion: number,
    modelContent: string,
    privateContent: string,
  ): Promise<VisualizationStateWriteDto> {
    return validateVisualizationStateWrite(await this.visualization({
      op: 'write_state',
      token,
      generation,
      base_version: baseVersion,
      model_content: modelContent,
      private_content: privateContent,
    }));
  }

  /** Retire a mount. */
  async visualizationUnmount(token: string): Promise<void> {
    await this.visualization({ op: 'unmount', token });
  }

  /** Retire every mount of a conversation. */
  async visualizationUnmountSession(sessionId: string): Promise<void> {
    await this.visualization({ op: 'unmount_session', session_id: sessionId });
  }

  /** Stored revisions of a conversation. */
  async visualizationList(sessionId: string): Promise<VisualizationRevisionDto[]> {
    return validateVisualizationList(await this.visualization({ op: 'list', session_id: sessionId }));
  }

  /** Third-party notices bundled with the visualization runtime. */
  async visualizationNotices(): Promise<string> {
    const notices = await this.visualization({ op: 'notices' });
    if (typeof notices !== 'string') throw new Error('invalid visualization notices');
    return notices;
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

  /** Answer a parked interactive questionnaire, correlated by `request_id`. */
  answerAskUserQuestion(requestId: number, answers: Record<string, string>): void {
    this.sendCommand({ type: 'answer_ask_user_question', request_id: requestId, answers });
  }

  /** Cancel a parked interactive questionnaire, correlated by `request_id`. */
  cancelAskUserQuestion(requestId: number): void {
    this.sendCommand({ type: 'cancel_ask_user_question', request_id: requestId });
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
        try {
          const event = validateClientEvent(frame.payload);
          // Refreshed credentials belong only to the host event handler, never the replay queue.
          if (event.type !== 'openai_oauth_updated') this.pushEvent(event);
          this.emit('event', event);
        } catch (error) {
          this.emit('error', new Error(`bridge: invalid inbound client event (${error instanceof Error ? error.message : String(error)})`));
        }
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
    // EventEmitter consumers do not request a second, retained copy of every
    // event. Buffer only while an async stream is explicitly subscribed.
    if (this.streamClosed || this.eventStreams.size === 0) return;
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
   * waiting first). Calling this method subscribes immediately; call it before
   * connect/send to retain events arriving before the first `next()`. Events
   * preceding the subscription are not replayed. Returning the last iterator
   * releases its buffered events and disables buffering.
   */
  events(): AsyncIterableIterator<ClientEvent> {
    const self = this;
    const owner = Symbol('bridge event stream');
    let returned = false;
    self.eventStreams.add(owner);
    const unsubscribe = () => {
      returned = true;
      self.eventStreams.delete(owner);
      for (let index = self.eventWaiters.length - 1; index >= 0; index -= 1) {
        if (self.eventWaiters[index].owner === owner) {
          const [waiter] = self.eventWaiters.splice(index, 1);
          waiter.resolve({ value: undefined, done: true });
        }
      }
      if (self.eventStreams.size === 0) self.eventQueue.length = 0;
    };
    const iterator: AsyncIterableIterator<ClientEvent> = {
      next(): Promise<IteratorResult<ClientEvent>> {
        if (returned) return Promise.resolve({ value: undefined, done: true });
        const buffered = self.eventQueue.shift();
        if (buffered !== undefined) {
          return Promise.resolve({ value: buffered, done: false });
        }
        if (self.streamClosed) {
          unsubscribe();
          return Promise.resolve({ value: undefined, done: true });
        }
        return new Promise<IteratorResult<ClientEvent>>((resolve, reject) => {
          self.eventWaiters.push({ owner, resolve, reject });
        });
      },
      return(): Promise<IteratorResult<ClientEvent>> {
        unsubscribe();
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
    if (!this.ws) this.closeStream();
    this.ws?.close();
  }
}
