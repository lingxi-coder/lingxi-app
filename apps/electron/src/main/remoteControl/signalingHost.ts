/**
 * The host's side of the signaling in the Realtime Database: the presence
 * record with its heartbeat, and the single stream on this host's inbox
 * (`to_host/{host_id}`) that carries every viewer's offers and tunnel frames.
 *
 * The stream replays the whole inbox after every reconnect, so everything
 * dispatched from it is derived from a diff against what was seen before, and
 * each offer is handed on once per `conn_id` however often it reappears.
 */

import {
  HEARTBEAT_INTERVAL_MS,
  RtdbChanges,
  RtdbError,
  RtdbMirror,
  SERVER_TIMESTAMP,
  answerPath,
  hostPath,
  offerPath,
  parseFrameKey,
  systemClock,
  toHostConnPath,
  toHostPath,
  toViewerConnPath,
  toViewerPath,
  type AnswerWrite,
  type Clock,
  type HostRecordWrite,
  type Rtdb,
  type RtdbEvent,
  type RtdbListenError,
  type RtdbSubscription,
} from '@lingxi/bridge-client/remote';

import { isRtcConnId } from '../../shared/remoteRtc.js';
import { describeError, noDiagnostics, type RemoteDiagnostics } from './diagnostics.js';
import type { HostIdentity } from './hostIdentity.js';

/** What the computer tells viewers about itself in `hosts/{host_id}`. */
export interface HostInfo {
  /** At most 128 characters. */
  name: string;
  /** At most 32 characters; `process.platform`. */
  platform: string;
  /** At most 64 characters. */
  app_version: string;
  /** `CLIENT_PROTOCOL_MAJOR`. */
  client_protocol_version: number;
}

export type SignalingState = 'stopped' | 'starting' | 'online' | 'error';

export interface HostSignalingOptions {
  rtdb: Rtdb;
  uid: string;
  identity: HostIdentity;
  info: HostInfo;
  clock?: Clock;
  diagnostics?: RemoteDiagnostics;
  /** A new `conn_id` appeared with an offer; `record` is whatever was written there. Once per `conn_id`. */
  onOffer(connId: string, record: unknown): void;
  /** A tunnel frame from a viewer appeared at `up/f{seq}`. */
  onFrame(connId: string, seq: number, value: unknown): void;
  /** The state changed; `error` explains `'error'`. */
  onState?(state: SignalingState, error?: string): void;
}

// Heartbeat failures in a row before the host reports itself in error (three beats: viewers have by then given up on it).
const BEAT_FAILURES_BEFORE_ERROR = 3;
// How long `stop()` may spend telling the database goodbye.
const STOP_BUDGET_MS = 2_000;
// `conn_id`s remembered for the once-only rule.
const MAX_REMEMBERED_OFFERS = 4096;

export class HostSignaling {
  private readonly rtdb: Rtdb;
  private readonly clock: Clock;
  private readonly diagnostics: RemoteDiagnostics;
  private readonly hostRecord: string;
  private readonly inbox: string;
  private readonly outbox: string;
  private readonly offered = new Set<string>();
  private mirror = new RtdbMirror();
  private subscription: RtdbSubscription | undefined;
  private heartbeat: unknown;
  private running = false;
  // Whether the database may hold this run's host record, inbox and outbox.
  private published = false;
  // Whether the inbox stream has been opened by `start()`.
  private listening = false;
  private generation = 0;
  private beating = false;
  private beatFailures = 0;
  private listenError: string | undefined;
  private beatError: string | undefined;
  private state: SignalingState = 'stopped';
  private stateError: string | undefined;

  constructor(private readonly options: HostSignalingOptions) {
    this.rtdb = options.rtdb;
    this.clock = options.clock ?? systemClock;
    this.diagnostics = options.diagnostics ?? noDiagnostics;
    this.hostRecord = hostPath(options.uid, options.identity.hostId);
    this.inbox = toHostPath(options.uid, options.identity.hostId);
    this.outbox = toViewerPath(options.uid, options.identity.hostId);
  }

  /**
   * Clear the inbox and outbox (connections of an earlier run are dead), write
   * the full host record, then start the heartbeat and the stream. Rejects when
   * the database cannot be reached or refuses; the caller decides about retrying.
   */
  async start(): Promise<void> {
    if (this.running) return;
    this.running = true;
    this.published = true;
    const generation = ++this.generation;
    this.beatFailures = 0;
    this.listenError = undefined;
    this.beatError = undefined;
    this.setState('starting');
    try {
      await Promise.all([this.rtdb.remove(this.inbox), this.rtdb.remove(this.outbox)]);
      await this.rtdb.put(this.hostRecord, this.record());
    } catch (error) {
      if (generation === this.generation) {
        this.running = false;
        this.setState('error', describeError(error));
      }
      throw error;
    }
    if (generation !== this.generation) {
      // Stopped while starting: the record written above must not be left saying online.
      this.rtdb.patch(this.hostRecord, { online: false, last_seen: SERVER_TIMESTAMP }).catch(() => undefined);
      return;
    }
    this.listening = true;
    this.listen();
    this.scheduleBeat();
    this.refreshState();
  }

  /** Stop at once, then spend up to two seconds marking the host offline and emptying its inbox and outbox. */
  async stop(): Promise<void> {
    this.running = false;
    this.listening = false;
    this.generation++;
    this.clock.clearTimeout(this.heartbeat);
    this.heartbeat = undefined;
    this.subscription?.close();
    this.subscription = undefined;
    this.setState('stopped');
    if (!this.published) return;
    this.published = false;
    const goodbye = Promise.allSettled([
      this.rtdb.patch(this.hostRecord, { online: false, last_seen: SERVER_TIMESTAMP }),
      this.rtdb.remove(this.inbox),
      this.rtdb.remove(this.outbox),
    ]);
    let timer: unknown;
    const budget = new Promise<void>((resolve) => { timer = this.clock.setTimeout(resolve, STOP_BUDGET_MS); });
    try {
      await Promise.race([goodbye, budget]);
    } finally {
      this.clock.clearTimeout(timer);
    }
  }

  /**
   * After a network change or a wake from sleep: reopen the stream, or open it
   * again if it had ended for good, and refresh `last_seen` right away.
   */
  reconnect(): void {
    if (!this.running || !this.listening) return;
    if (this.subscription !== undefined && this.listenError === undefined) this.subscription.reconnect();
    else this.listen();
    this.clock.clearTimeout(this.heartbeat);
    void this.beat();
  }

  writeAnswer(connId: string, answer: AnswerWrite): Promise<void> {
    return this.rtdb.put(answerPath(this.options.uid, this.options.identity.hostId, connId), answer);
  }

  removeOffer(connId: string): Promise<void> {
    return this.rtdb.remove(offerPath(this.options.uid, this.options.identity.hostId, connId));
  }

  /** Remove everything this connection left on both sides of the inbox. */
  async removeConnection(connId: string): Promise<void> {
    const { uid, identity } = this.options;
    await Promise.all([
      this.rtdb.remove(toHostConnPath(uid, identity.hostId, connId)),
      this.rtdb.remove(toViewerConnPath(uid, identity.hostId, connId)),
    ]);
  }

  private record(): HostRecordWrite {
    const { identity, info } = this.options;
    return {
      v: 1,
      name: info.name,
      platform: info.platform,
      app_version: info.app_version,
      client_protocol_version: info.client_protocol_version,
      host_key: identity.hostKey,
      online: true,
      last_seen: SERVER_TIMESTAMP,
      started_at: SERVER_TIMESTAMP,
    };
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Heartbeat
  // ───────────────────────────────────────────────────────────────────────────

  private scheduleBeat(): void {
    if (!this.running) return;
    this.heartbeat = this.clock.setTimeout(() => void this.beat(), HEARTBEAT_INTERVAL_MS);
  }

  /**
   * PATCH `last_seen`. The rules validate the merged record, so a PATCH on a
   * record that no longer exists is refused as a permission failure (401): in
   * that case the full record is written again, once.
   */
  private async beat(): Promise<void> {
    if (!this.running || this.beating) return;
    this.beating = true;
    const generation = this.generation;
    try {
      try {
        await this.rtdb.patch(this.hostRecord, { last_seen: SERVER_TIMESTAMP });
      } catch (error) {
        if (!(error instanceof RtdbError) || error.status !== 401) throw error;
        await this.rtdb.put(this.hostRecord, this.record());
        this.diagnostics('info', 'the host record was written again after a refused heartbeat');
      }
      this.beatFailures = 0;
      this.beatError = undefined;
    } catch (error) {
      this.beatFailures++;
      this.diagnostics('warn', `heartbeat failed (${describeError(error)})`);
      if (this.beatFailures >= BEAT_FAILURES_BEFORE_ERROR) this.beatError = `heartbeat failed: ${describeError(error)}`;
    } finally {
      this.beating = false;
      if (generation === this.generation) {
        this.refreshState();
        this.scheduleBeat();
      }
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Inbox stream
  // ───────────────────────────────────────────────────────────────────────────

  private listen(): void {
    this.subscription?.close();
    this.listenError = undefined;
    // A new stream starts with the whole inbox, which is diffed against a fresh mirror.
    this.mirror = new RtdbMirror();
    this.subscription = this.rtdb.listen(
      this.inbox,
      (event) => this.onEvent(event),
      (error) => this.onListenError(error),
    );
  }

  private onListenError(error: RtdbListenError): void {
    if (!this.running) return;
    this.listenError = error.kind === 'cancel'
      ? 'the Realtime Database cancelled the inbox stream (permission denied or sign-in expired)'
      : `the Realtime Database refused the inbox stream (HTTP ${error.status}: ${error.message})`;
    this.diagnostics('error', this.listenError);
    this.refreshState();
  }

  private onEvent(event: RtdbEvent): void {
    if (!this.running) return;
    const changes: RtdbChanges = this.mirror.apply(event);
    for (const change of changes.at(2)) {
      const [connId, node] = change.path;
      if (node === 'offer' && change.kind !== 'removed') this.offer(connId, change.value);
    }
    for (const change of changes.at(3)) {
      const [connId, node, key] = change.path;
      if (node !== 'up' || change.kind === 'removed' || !isRtcConnId(connId)) continue;
      const seq = parseFrameKey(key);
      if (seq === null) continue;
      try {
        this.options.onFrame(connId, seq, change.value);
      } catch (error) {
        this.diagnostics('warn', `a tunnel frame handler failed (${describeError(error)})`);
      }
    }
  }

  private offer(connId: string, record: unknown): void {
    if (!isRtcConnId(connId) || this.offered.has(connId)) return;
    if (this.offered.size >= MAX_REMEMBERED_OFFERS) this.offered.delete(this.offered.values().next().value!);
    this.offered.add(connId);
    try {
      this.options.onOffer(connId, record);
    } catch (error) {
      this.diagnostics('warn', `an offer handler failed (${describeError(error)})`);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // State
  // ───────────────────────────────────────────────────────────────────────────

  private refreshState(): void {
    if (!this.running) return;
    const error = this.listenError ?? this.beatError;
    if (error === undefined) this.setState('online');
    else this.setState('error', error);
  }

  private setState(state: SignalingState, error?: string): void {
    if (state === this.state && error === this.stateError) return;
    this.state = state;
    this.stateError = error;
    try {
      this.options.onState?.(state, error);
    } catch {
      // A throwing observer must not break the host.
    }
  }
}
