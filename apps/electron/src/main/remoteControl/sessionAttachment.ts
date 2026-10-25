/**
 * One viewer's attachment to one session: the first answer (a delta or a
 * snapshot), then the live stream, then whatever else the session wants the
 * viewer to know (`session.pending`, `session.reset`, `session.closed`).
 *
 * The snapshot and the live subscription are made in the same call, with no
 * await between reading the runtime and subscribing to it, so no event can
 * fall between them. Live records are batched for 50 ms and sent in order.
 */

import type { Clock, RemotePending, RemoteRecord, RemoteSeqRecord, SessionAttachParams, SessionAttachResult } from '@lingxi/bridge-client/remote';

import type { RemoteObservation, RemoteRuntimeView } from '../bridgeTypes.js';
import type { RemoteViewerChannel } from './gateway.js';
import type { RemoteRuntime } from './remoteRuntime.js';
import {
  buildSnapshot,
  coalesceRecords,
  recordForEvent,
  recordForInteraction,
  recordForPrompt,
  recordsAfter,
} from './snapshot.js';

/** How long live records are collected before they are sent. */
export const LIVE_BATCH_MS = 50;

export interface SessionAttachmentDeps {
  runtime: RemoteRuntime;
  channel: RemoteViewerChannel;
  clock: Clock;
  /** The runtime went away under the attachment (it told the viewer with `session.closed` first). */
  onEnded?: () => void;
}

type Resume = SessionAttachParams['resume'];

/**
 * Whether a viewer that holds everything up to `resume` can be given just what
 * came after: it is the same base, the position is one the runtime has really
 * reached, and nothing after it has been forgotten.
 */
export function canResume(view: RemoteRuntimeView, resume: Resume): resume is NonNullable<Resume> {
  return resume !== undefined
    && resume.epoch === view.epoch
    && Number.isSafeInteger(resume.after_seq)
    && resume.after_seq >= view.baseSequence
    && resume.after_seq <= view.lastSequence
    && resume.after_seq >= view.droppedThroughSequence;
}

const pendingSignature = (pending: RemotePending): string => JSON.stringify(pending);

export class SessionAttachment {
  private readonly runtime: RemoteRuntime;
  private readonly channel: RemoteViewerChannel;
  private readonly clock: Clock;
  private readonly onEnded: (() => void) | undefined;
  private unobserve: (() => void) | undefined;
  private batch: RemoteSeqRecord[] = [];
  private timer: unknown;
  private resnapshot = false;
  private lastPending = '';
  private closed = false;
  private foregroundNow = false;

  constructor(deps: SessionAttachmentDeps) {
    this.runtime = deps.runtime;
    this.channel = deps.channel;
    this.clock = deps.clock;
    this.onEnded = deps.onEnded;
  }

  get sessionId(): string { return this.runtime.sessionId; }
  /** Whether the viewer said it is looking at the session. */
  get foreground(): boolean { return this.foregroundNow; }
  get isClosed(): boolean { return this.closed; }

  setPresence(foreground: boolean): void {
    this.foregroundNow = foreground;
  }

  /**
   * Answer `session.attach`: send the delta or the snapshot, start the live
   * stream, and return the reply. Everything it sends is queued before the
   * reply itself.
   */
  attach(resume: Resume): SessionAttachResult {
    this.unobserve?.();
    const view = this.runtime.remoteView();
    this.unobserve = this.runtime.observeRemote((observation) => this.observe(observation));
    this.lastPending = pendingSignature(view.pending);
    if (canResume(view, resume)) {
      for (const { seq, record } of recordsAfter(view, resume.after_seq)) this.sendRecord(seq, record);
      return { mode: 'delta', epoch: view.epoch, pending: view.pending };
    }
    this.sendSnapshotFrames(view);
    return { mode: 'snapshot', epoch: view.epoch };
  }

  /** Stop. With a `reason` the viewer is told (`session.closed`); without, it is the viewer that asked. */
  close(reason?: string): void {
    if (this.closed) return;
    if (reason !== undefined) this.flush();
    this.closed = true;
    this.unobserve?.();
    this.unobserve = undefined;
    this.clock.clearTimeout(this.timer);
    this.timer = undefined;
    this.batch = [];
    if (reason !== undefined) this.channel.notify('session.closed', { session_id: this.sessionId, reason });
  }

  // ───────────────────────────────────────────────────────────────────────────
  // The live stream
  // ───────────────────────────────────────────────────────────────────────────

  private observe(observation: RemoteObservation): void {
    if (this.closed) return;
    switch (observation.type) {
      case 'event': {
        const type = observation.envelope.event.type;
        if (type === 'session_started' || type === 'session_resumed') { this.baseChanged(); return; }
        this.push(recordForEvent(observation.envelope));
        return;
      }
      case 'prompt':
        this.push(recordForPrompt(this.sessionId, observation.entry));
        return;
      case 'interaction':
        this.push(recordForInteraction(observation.interaction));
        return;
      case 'pending':
        this.pendingChanged();
        return;
      case 'disposed':
        this.close('ended');
        this.onEnded?.();
        return;
      case 'state':
        return;
    }
  }

  private push(item: RemoteSeqRecord | undefined): void {
    if (item === undefined || this.resnapshot) return;
    this.batch.push(item);
    this.schedule();
  }

  private schedule(): void {
    if (this.timer !== undefined) return;
    this.timer = this.clock.setTimeout(() => { this.timer = undefined; this.flush(); }, LIVE_BATCH_MS);
  }

  /** Send what has been collected, or the fresh snapshot a new base asked for. */
  private flush(): void {
    this.clock.clearTimeout(this.timer);
    this.timer = undefined;
    if (this.closed) return;
    if (this.resnapshot) {
      this.resnapshot = false;
      this.batch = [];
      const view = this.runtime.remoteView();
      this.lastPending = pendingSignature(view.pending);
      this.sendSnapshotFrames(view);
      return;
    }
    const records = coalesceRecords(this.batch);
    this.batch = [];
    for (const { seq, record } of records) this.sendRecord(seq, record);
  }

  /** The base changed (`/clear`, a restarted engine): what the viewer holds is stale. A fresh snapshot follows once things settle. */
  private baseChanged(): void {
    if (!this.resnapshot) this.channel.notify('session.reset', { session_id: this.sessionId });
    this.resnapshot = true;
    this.batch = [];
    this.schedule();
  }

  private pendingChanged(): void {
    if (this.resnapshot) return; // the snapshot will say
    const pending = this.runtime.remotePending();
    const signature = pendingSignature(pending);
    if (signature === this.lastPending) return;
    // The records that led to the change go first.
    this.flush();
    this.lastPending = signature;
    this.channel.notify('session.pending', { session_id: this.sessionId, pending });
  }

  private sendRecord(seq: number, record: RemoteRecord): void {
    this.channel.notify('session.event', { session_id: this.sessionId, seq, record });
  }

  private sendSnapshotFrames(view: RemoteRuntimeView): void {
    for (const frame of buildSnapshot(view)) this.channel.notify('session.snapshot', frame);
  }
}
