/**
 * WebRTC for the host. Electron's main process has none, so the peer
 * connections live in a hidden Chromium window that only moves bytes: SDP in,
 * SDP out, DataChannel messages both ways. Every decision (who may connect,
 * what is said) stays in main; the window talks to it over one MessagePort,
 * and both ends validate what they receive (`shared/remoteRtc.ts`).
 *
 * The window is created on the first WebRTC offer and closed after five
 * minutes without a connection. `PeerHost` is Electron-free: the window itself
 * (`rtcWindow.ts`) is injected, so this logic runs in plain Node tests.
 */

import {
  FRAGMENT_MAX_BYTES,
  systemClock,
  type Clock,
  type FragmentLink,
} from '@lingxi/bridge-client/remote';

import {
  MAX_RTC_PEERS,
  parseMainToPage,
  parsePageToMain,
  type MainToPage,
  type PageToMain,
  type RtcCandidateType,
  type RtcPeerState,
  type RtcIceServer,
} from '../../shared/remoteRtc.js';
import { describeError, noDiagnostics, type RemoteDiagnostics } from './diagnostics.js';

/** Main's end of the channel to the page. */
export interface RtcPort {
  post(message: MainToPage): void;
  /** Receive whatever the page sends; unvalidated. */
  onMessage(handler: (data: unknown) => void): void;
}

/** The hidden window, as `PeerHost` needs it. */
export interface RtcWindow {
  /** Create and load the page, and resolve with this side's end of the channel once the page holds the other end. */
  open(): Promise<RtcPort>;
  /** Be told once when the window is gone, however it went (closed, crashed, the channel dropped). */
  onGone(handler: (reason: string) => void): void;
  /** Destroy the window. Idempotent. */
  close(): void;
}

/** One WebRTC connection as a fragment stream, with what the page reports about it. */
export interface PeerLink extends FragmentLink {
  readonly connId: string;
  readonly files?: PeerLink;
  /** Whether the DataChannel is open. */
  readonly connected: boolean;
  /** The selected pair's local candidate type, once the page has reported it. */
  readonly candidate: RtcCandidateType | undefined;
  /** Be told when the DataChannel opens (`connected`) or the connection ends; replays the latest state. */
  onState(handler: (state: RtcPeerState, candidate?: RtcCandidateType) => void): void;
}

export interface PeerHostOptions {
  createWindow: () => RtcWindow;
  clock?: Clock;
  diagnostics?: RemoteDiagnostics;
}

const ANSWER_TIMEOUT_MS = 10_000;
const IDLE_CLOSE_MS = 5 * 60_000;

interface PendingAnswer {
  resolve(sdp: string): void;
  reject(error: Error): void;
  timer: unknown;
}

export class PeerHost {
  private readonly createWindow: () => RtcWindow;
  private readonly clock: Clock;
  private readonly diagnostics: RemoteDiagnostics;
  private readonly links = new Map<string, LinkImpl>();
  private readonly pending = new Map<string, PendingAnswer>();
  private window: RtcWindow | undefined;
  private port: Promise<RtcPort> | undefined;
  private idleTimer: unknown;
  private disposed = false;
  private discarded = 0;

  constructor(options: PeerHostOptions) {
    this.createWindow = options.createWindow;
    this.clock = options.clock ?? systemClock;
    this.diagnostics = options.diagnostics ?? noDiagnostics;
  }

  /** Connections currently held, answered or not. */
  get connectionCount(): number {
    return this.links.size;
  }

  /** Whether the hidden window exists. */
  get windowOpen(): boolean {
    return this.window !== undefined;
  }

  /** Messages from the page that were not valid and so were dropped. */
  get discardedMessages(): number {
    return this.discarded;
  }

  /**
   * Hand a viewer's offer SDP to the page and get its answer SDP back (ICE
   * candidates included). Rejects after ten seconds, if the page fails, or if
   * the window cannot be created. The connection's link is available from
   * {@link link} as soon as this is called, and is released when it rejects.
   */
  answer(connId: string, offerSdp: string, iceUrls: readonly (string | RtcIceServer)[]): Promise<string> {
    const request = parseMainToPage({ t: 'answer', conn: connId, sdp: offerSdp, ice: [...iceUrls] });
    if (this.disposed) return Promise.reject(new Error('the peer host is shut down'));
    if (request?.t !== 'answer') return Promise.reject(new Error('invalid WebRTC answer request'));
    if (this.links.has(connId)) return Promise.reject(new Error('this connection already has a peer'));
    if (this.links.size >= MAX_RTC_PEERS) return Promise.reject(new Error('too many peer connections'));
    this.clock.clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    this.links.set(connId, new LinkImpl(connId, this));
    return new Promise<string>((resolve, reject) => {
      const timer = this.clock.setTimeout(() => this.failAnswer(connId, new Error('timed out waiting for the WebRTC answer')), ANSWER_TIMEOUT_MS);
      const pending = { resolve, reject, timer };
      this.pending.set(connId, pending);
      // Opening the window can finish after the peer was closed or replaced.
      // Never create a renderer peer for an answer that no longer belongs to us.
      Promise.resolve().then(() => this.pending.get(connId) === pending ? this.openPort() : undefined).then(
        (port) => {
          if (!port || this.pending.get(connId) !== pending) return;
          try { port.post(request); } catch (error) {
            this.failAnswer(connId, error instanceof Error ? error : new Error(describeError(error)));
          }
        },
        (error: unknown) => {
          if (this.pending.get(connId) === pending) this.failAnswer(connId, error instanceof Error ? error : new Error(describeError(error)));
        },
      );
    });
  }

  /** The link of a connection {@link answer} was called for. */
  link(connId: string): PeerLink {
    const link = this.links.get(connId);
    if (link === undefined) throw new Error('unknown peer connection');
    return link;
  }

  /** Close the window and every connection; the host can be used again afterwards. */
  shutdown(): void {
    this.endAll('host_shutdown');
    this.closeWindow();
  }

  dispose(): void {
    this.disposed = true;
    this.shutdown();
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Used by the links
  // ───────────────────────────────────────────────────────────────────────────

  /** @internal */
  post(message: MainToPage): void {
    void this.port?.then((port) => {
      try { port.post(message); } catch { /* The window went away first. */ }
    }, () => undefined);
  }

  /** @internal A link ended: forget it, tell the page, and start the idle clock when nothing is left. */
  release(connId: string, tellPage: boolean): void {
    if (!this.links.delete(connId)) return;
    const pending = this.pending.get(connId);
    if (pending) {
      this.pending.delete(connId);
      this.clock.clearTimeout(pending.timer);
      pending.reject(new Error('the peer connection closed before its answer'));
    }
    if (tellPage) this.post({ t: 'close', conn: connId });
    if (this.links.size === 0 && this.window !== undefined && this.idleTimer === undefined) {
      this.idleTimer = this.clock.setTimeout(() => {
        this.idleTimer = undefined;
        if (this.links.size === 0) this.closeWindow();
      }, IDLE_CLOSE_MS);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // The window
  // ───────────────────────────────────────────────────────────────────────────

  private openPort(): Promise<RtcPort> {
    if (this.port !== undefined) return this.port;
    const window = this.createWindow();
    this.window = window;
    window.onGone((reason) => this.windowGone(window, reason));
    const port = Promise.resolve().then(() => {
      // shutdown or a gone notification can revoke ownership before this microtask.
      if (this.window !== window) throw new Error('the WebRTC window was closed before opening');
      return window.open();
    }).then((opened) => {
      if (this.window !== window) {
        // An asynchronous open may finish after close; dispose any late resource.
        try { window.close(); } catch { /* Already gone. */ }
        throw new Error('the WebRTC window was closed while opening');
      }
      opened.onMessage((data) => this.receive(window, data));
      return opened;
    });
    if (this.window === window) this.port = port;
    port.catch((error: unknown) => {
      if (this.window !== window) return;
      this.diagnostics('warn', `the WebRTC window could not be opened (${describeError(error)})`);
      this.closeWindow();
    });
    return port;
  }

  private closeWindow(): void {
    this.clock.clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    const window = this.window;
    this.window = undefined;
    this.port = undefined;
    try { window?.close(); } catch { /* Already gone. */ }
  }

  private windowGone(window: RtcWindow, reason: string): void {
    if (this.window !== window) return;
    this.diagnostics('warn', `the WebRTC window is gone (${reason})`);
    this.window = undefined;
    this.port = undefined;
    this.clock.clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    this.endAll('window_gone');
  }

  private endAll(reason: string): void {
    for (const connId of [...this.pending.keys()]) this.failAnswer(connId, new Error(`the WebRTC window ended (${reason})`));
    for (const link of [...this.links.values()]) link.close(reason);
  }

  private failAnswer(connId: string, error: Error): void {
    const pending = this.pending.get(connId);
    if (pending === undefined) return;
    this.pending.delete(connId);
    this.clock.clearTimeout(pending.timer);
    this.links.get(connId)?.close('answer_failed');
    pending.reject(error);
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Messages from the page
  // ───────────────────────────────────────────────────────────────────────────

  private receive(window: RtcWindow, data: unknown): void {
    if (this.window !== window) return;
    const message = parsePageToMain(data);
    if (message === undefined) {
      this.discarded++;
      return;
    }
    this.apply(message);
  }

  private apply(message: PageToMain): void {
    const parent = this.links.get(message.conn);
    const link = 'lane' in message && message.lane === 'files' ? parent?.files : parent;
    switch (message.t) {
      case 'answered': {
        const pending = this.pending.get(message.conn);
        if (pending === undefined) return;
        this.pending.delete(message.conn);
        this.clock.clearTimeout(pending.timer);
        pending.resolve(message.sdp);
        return;
      }
      case 'error':
        if (this.pending.has(message.conn)) this.failAnswer(message.conn, new Error(`the WebRTC page failed: ${message.message}`));
        else link?.close('page_error');
        return;
      case 'state':
        link?.apply(message.state, message.candidate, message.max_message_bytes);
        return;
      case 'data':
        link?.receive(new Uint8Array(message.data));
        return;
      case 'buffered':
        link?.pause();
        return;
      case 'drain':
        link?.resume();
        return;
    }
  }
}

class LinkImpl implements PeerLink {
  maxFragmentBytes = FRAGMENT_MAX_BYTES;
  readonly files: LinkImpl | undefined;
  connected = false;
  candidate: RtcCandidateType | undefined;

  private closed = false;
  private paused = false;
  private waiters: Array<() => void> = [];
  private fragmentHandler: ((fragment: Uint8Array) => void) | undefined;
  private readonly closeHandlers: Array<(reason: string) => void> = [];
  private readonly stateHandlers: Array<(state: RtcPeerState, candidate?: RtcCandidateType) => void> = [];
  private closeReason: string | undefined;

  constructor(
    readonly connId: string,
    private readonly host: PeerHost,
    private readonly lane?: 'files',
  ) { if (!lane) this.files = new LinkImpl(connId, host, 'files'); }

  sendFragment(fragment: Uint8Array): void {
    if (this.closed) throw new Error(`peer link is closed: ${this.closeReason}`);
    if (fragment.length === 0 || fragment.length > FRAGMENT_MAX_BYTES) throw new RangeError('fragment size out of range');
    const data = new Uint8Array(fragment.length);
    data.set(fragment);
    this.host.post({ t: 'send', conn: this.connId, data: data.buffer, ...(this.lane ? { lane: this.lane } : {}) });
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.fragmentHandler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    const reason = this.closeReason;
    if (reason === undefined) this.closeHandlers.push(callback);
    else queueMicrotask(() => callback(reason));
  }

  onState(handler: (state: RtcPeerState, candidate?: RtcCandidateType) => void): void {
    this.stateHandlers.push(handler);
    const latest: RtcPeerState | undefined = this.closed ? 'closed' : this.connected ? 'connected' : undefined;
    if (latest !== undefined) queueMicrotask(() => handler(latest, this.candidate));
  }

  /**
   * Resolves when the page can take more. Always yields a turn of the event
   * loop first, so that a `buffered` the page has just sent is seen before the
   * next fragment goes out; otherwise a whole large message would be queued
   * into the page before the pause could take effect.
   */
  whenWritable(): Promise<void> {
    return new Promise<void>((resolve) => {
      setImmediate(() => {
        if (!this.paused || this.closed) resolve();
        else this.waiters.push(resolve);
      });
    });
  }

  close(reason: string, tellPage = true): void {
    if (this.closed) return;
    this.closed = true;
    this.connected = false;
    this.closeReason = reason;
    this.files?.close(reason, false);
    if (!this.lane) this.host.release(this.connId, tellPage);
    else if (tellPage) this.host.post({ t: 'close_files', conn: this.connId });
    this.wake();
    for (const handler of this.closeHandlers.splice(0)) {
      try { handler(reason); } catch { /* Close handlers are independent of each other. */ }
    }
    this.notifyState('closed');
  }

  /** @internal */
  apply(state: RtcPeerState, candidate: RtcCandidateType | undefined, limit?: number): void {
    if (this.closed) return;
    if (state === 'connected') {
      if (limit !== undefined) this.maxFragmentBytes = Math.min(FRAGMENT_MAX_BYTES, limit);
      this.connected = true;
      this.candidate = candidate;
      this.notifyState('connected');
    } else {
      this.notifyState(state);
      this.close(state, false);
    }
  }

  /** @internal */
  receive(fragment: Uint8Array): void {
    if (this.closed) return;
    try { this.fragmentHandler?.(fragment); } catch { /* A throwing consumer must not break the link. */ }
  }

  /** @internal */
  pause(): void {
    this.paused = true;
  }

  /** @internal */
  resume(): void {
    this.paused = false;
    this.wake();
  }

  private wake(): void {
    for (const resolve of this.waiters.splice(0)) resolve();
  }

  private notifyState(state: RtcPeerState): void {
    for (const handler of this.stateHandlers) {
      try { handler(state, this.candidate); } catch { /* Handlers are independent of each other. */ }
    }
  }
}
