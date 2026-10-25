/**
 * The Remote Control side of the session hub: the `RemoteSessionGateway` that
 * `connection.ts` hands every request of an approved viewer to.
 *
 * It keeps three things straight:
 *
 * - which sessions a viewer may see (only those with Remote Control on, see
 *   `policy.ts`) and what state each is in;
 * - who is attached to which session, with one attachment per viewer and
 *   session (`sessionAttachment.ts`), and the one lease per session that keeps
 *   its runtime open while anyone is attached and for two minutes after;
 * - what each viewer's requests are allowed to do (`remoteCommands.ts`).
 *
 * Errors are `RemoteRpcError`s: `invalid_params` for a request that is
 * malformed or that the session refused, `unknown_session` and
 * `not_remote_enabled` for a session the viewer may not use, `not_pending`
 * and `forbidden` from the commands, `capacity` when too many sessions are
 * already open for viewers.
 */

import { basename } from 'node:path';

import {
  RemoteRpcError,
  type Clock,
  type RemotePendingAction,
  type RemoteSessionSummary,
  type SessionAttachParams,
} from '@lingxi/bridge-client/remote';

import type { RemoteObservation, SessionRef } from '../bridgeTypes.js';
import { isSessionId } from '../sessionIdentity.js';
import { RemoteCapacityError } from '../sessionRuntimeManager.js';
import { describeError, type RemoteDiagnostics } from './diagnostics.js';
import type { RemoteSessionGateway, RemoteViewer, RemoteViewerChannel } from './gateway.js';
import type { RemotePolicy } from './policy.js';
import type { PushPresence } from './pushRequests.js';
import type { RemoteCommands } from './remoteCommands.js';
import type { RemoteRuntime, RemoteRuntimes, RuntimeLease } from './remoteRuntime.js';
import { SessionAttachment } from './sessionAttachment.js';
import { parseImageRef } from './snapshot.js';

/** A session stays open this long after its last viewer detached. */
export const LEASE_GRACE_MS = 2 * 60_000;
/** Changes to the session list are sent at most this often. */
export const LIST_DEBOUNCE_MS = 100;
/** Session titles are looked up again after this long. */
const TITLE_TTL_MS = 60_000;
const MAX_ATTACHMENTS_PER_VIEWER = 8;
const UNTITLED = 'Untitled session';

export interface SessionGatewayDeps {
  runtimes: RemoteRuntimes;
  policy: Pick<RemotePolicy, 'enabledSessions' | 'find' | 'wasEnabled' | 'onChange'>;
  commands: RemoteCommands;
  /** The session's title, from the persisted transcript. */
  titleOf(ref: SessionRef): Promise<string | undefined>;
  /** The model a session that is not open will use. */
  modelOf?(ref: SessionRef): string | undefined;
  clock: Clock;
  diagnostics: RemoteDiagnostics;
}

interface SessionLease {
  refs: number;
  opening?: Promise<RuntimeLease>;
  lease?: RuntimeLease;
  timer?: unknown;
}

interface Attached {
  attachment: SessionAttachment;
  lease: SessionLease;
}

interface ViewerState {
  viewer: RemoteViewer;
  channel: RemoteViewerChannel;
  attached: Map<string, Attached>;
  /** A pending attach reserves capacity and is invalidated by detach/replacement. */
  attaching: Map<string, symbol>;
  watching: boolean;
  lastList?: string;
  closed: boolean;
}

type Json = Record<string, unknown>;

export class SessionGateway implements RemoteSessionGateway, PushPresence {
  private readonly viewers = new Map<string, ViewerState>();
  private readonly leases = new Map<string, SessionLease>();
  private readonly titles = new Map<string, { title: string; at: number }>();
  private readonly watchers = new Set<() => void>();
  private readonly viewerListeners = new Set<() => void>();
  private listTimer: unknown;
  private disposed = false;

  constructor(private readonly deps: SessionGatewayDeps) {
    this.watchers.add(deps.policy.onChange(() => this.policyChanged()));
    this.watchers.add(deps.runtimes.onRuntimeCreated((runtime) => this.watchRuntime(runtime)));
  }

  // ───────────────────────────────────────────────────────────────────────────
  // RemoteSessionGateway
  // ───────────────────────────────────────────────────────────────────────────

  async handle(viewer: RemoteViewer, method: string, rawParams: unknown, channel: RemoteViewerChannel): Promise<unknown> {
    if (this.disposed) throw new RemoteRpcError('internal', 'remote sessions are not available');
    const state = this.stateOf(viewer, channel);
    const params = rawParams === undefined ? {} : rawParams;
    switch (method) {
      case 'sessions.list':
        return { sessions: await this.summaries() };
      case 'sessions.watch': {
        state.watching = true;
        const sessions = await this.summaries();
        state.lastList = JSON.stringify(sessions);
        return { sessions };
      }
      case 'session.attach':
        return this.attach(state, params);
      case 'session.detach':
        this.detach(state, this.sessionIdOf(params));
        return {};
      case 'session.presence':
        this.presence(state, params);
        return undefined;
      case 'session.history':
        return this.deps.commands.history(this.attachedRuntime(state, params), params);
      case 'image.get':
        return this.deps.commands.imageGet(this.imageRuntime(params), params);
      case 'prompt.send':
        return this.deps.commands.promptSend(this.attachedRuntime(state, params), viewer, params);
      case 'turn.cancel':
        return this.deps.commands.turnCancel(this.attachedRuntime(state, params), params);
      case 'permission.respond':
        return this.deps.commands.permissionRespond(this.attachedRuntime(state, params), params);
      case 'question.answer':
        return this.deps.commands.questionAnswer(this.attachedRuntime(state, params), params);
      case 'question.cancel':
        return this.deps.commands.questionCancel(this.attachedRuntime(state, params), params);
      case 'controls.set_model':
        return this.deps.commands.setModel(this.attachedRuntime(state, params), viewer, params);
      case 'controls.set_reasoning':
        return this.deps.commands.setReasoning(this.attachedRuntime(state, params), viewer, params);
      case 'controls.set_permission_mode':
        return this.deps.commands.setPermissionMode(this.attachedRuntime(state, params), viewer, params);
      case 'models.list':
        return this.deps.commands.modelsList(this.modelsRuntime(state, params));
      default:
        throw new RemoteRpcError('invalid_params', `unknown method ${method.slice(0, 64)}`);
    }
  }

  viewerClosed(viewer: RemoteViewer): void {
    const state = this.viewers.get(viewer.conn_id);
    if (state === undefined) return;
    state.closed = true;
    state.attaching.clear();
    this.viewers.delete(viewer.conn_id);
    for (const sessionId of [...state.attached.keys()]) this.dropAttachment(state, sessionId, true);
    this.viewersChanged();
  }

  /** Stop watching, close every attachment and give up the leases. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const stop of this.watchers) stop();
    this.watchers.clear();
    this.deps.clock.clearTimeout(this.listTimer);
    for (const state of [...this.viewers.values()]) {
      state.closed = true;
      state.attaching.clear();
      for (const sessionId of [...state.attached.keys()]) this.dropAttachment(state, sessionId, true);
    }
    this.viewers.clear();
    for (const entry of [...this.leases.values()]) this.releaseNow(entry);
    this.leases.clear();
  }

  private stateOf(viewer: RemoteViewer, channel: RemoteViewerChannel): ViewerState {
    let state = this.viewers.get(viewer.conn_id);
    if (state === undefined) {
      state = { viewer, channel, attached: new Map(), attaching: new Map(), watching: false, closed: false };
      this.viewers.set(viewer.conn_id, state);
    }
    return state;
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Which session
  // ───────────────────────────────────────────────────────────────────────────

  private sessionIdOf(params: unknown): string {
    const sessionId = (params as Json | null)?.['session_id'];
    if (!isSessionId(sessionId)) throw new RemoteRpcError('invalid_params', 'invalid session_id');
    return sessionId;
  }

  /** The reference of a session the viewer may use, or the error that says why not. */
  private requireEnabled(sessionId: string): SessionRef {
    const ref = this.deps.policy.find(sessionId);
    if (ref !== undefined) return ref;
    throw this.deps.runtimes.get(sessionId) !== undefined || this.deps.policy.wasEnabled(sessionId)
      ? new RemoteRpcError('not_remote_enabled', 'Remote Control is off for this session')
      : new RemoteRpcError('unknown_session', 'no such session');
  }

  /** The runtime of a session this viewer is attached to. */
  private attachedRuntime(state: ViewerState, params: unknown): RemoteRuntime {
    const sessionId = this.sessionIdOf(params);
    this.requireEnabled(sessionId);
    const attached = state.attached.get(sessionId);
    const runtime = attached === undefined ? undefined : this.deps.runtimes.get(sessionId);
    if (attached === undefined || runtime === undefined) throw new RemoteRpcError('invalid_params', 'attach to the session first');
    return runtime;
  }

  /** The runtime a reference points into, if its session may be used. */
  private imageRuntime(params: unknown): RemoteRuntime | undefined {
    const ref = (params as Json | null)?.['ref'];
    const parsed = typeof ref === 'string' ? parseImageRef(ref) : undefined;
    if (parsed === undefined) throw new RemoteRpcError('invalid_params', 'invalid ref');
    this.requireEnabled(parsed.sessionId);
    return this.deps.runtimes.get(parsed.sessionId);
  }

  private modelsRuntime(state: ViewerState, params: unknown): RemoteRuntime {
    const sessionId = (params as Json | null)?.['session_id'];
    if (sessionId !== undefined) {
      const id = this.sessionIdOf(params);
      this.requireEnabled(id);
      const runtime = this.deps.runtimes.get(id);
      if (runtime === undefined) throw new RemoteRpcError('invalid_params', 'attach to the session first');
      return runtime;
    }
    for (const id of state.attached.keys()) {
      const runtime = this.deps.runtimes.get(id);
      if (runtime !== undefined) return runtime;
    }
    for (const ref of this.deps.policy.enabledSessions()) {
      const runtime = this.deps.runtimes.get(ref.sessionId);
      if (runtime?.connectionState.status === 'connected') return runtime;
    }
    throw new RemoteRpcError('unknown_session', 'no session is open to list models for');
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Attaching
  // ───────────────────────────────────────────────────────────────────────────

  private async attach(state: ViewerState, rawParams: unknown): Promise<unknown> {
    const sessionId = this.sessionIdOf(rawParams);
    const resume = this.resumeOf(rawParams);
    const ref = this.requireEnabled(sessionId);
    const reserved = new Set([...state.attached.keys(), ...state.attaching.keys()]);
    if (!reserved.has(sessionId) && reserved.size >= MAX_ATTACHMENTS_PER_VIEWER) {
      throw new RemoteRpcError('capacity', 'this device is attached to too many sessions');
    }
    const attempt = Symbol();
    state.attaching.set(sessionId, attempt);
    try {
      const { entry, runtime } = await this.acquire(ref);
      // Detach, an unshare, or a newer attach may have superseded this request
      // while the runtime opened. A re-enabled session must not revive it.
      if (state.closed || this.disposed || this.deps.policy.find(sessionId) === undefined
        || state.attaching.get(sessionId) !== attempt) {
        this.release(entry);
        if (state.closed || this.disposed) throw new RemoteRpcError('internal', 'the connection closed');
        if (this.deps.policy.find(sessionId) === undefined) throw this.requireEnabledError(sessionId);
        throw new RemoteRpcError('invalid_params', 'the session attachment was cancelled');
      }
      // Attaching again replaces the attachment; the session stays leased throughout.
      this.dropAttachment(state, sessionId, true);
      const attachment: SessionAttachment = new SessionAttachment({
        runtime,
        channel: state.channel,
        clock: this.deps.clock,
        onEnded: () => this.dropEnded(state, sessionId, attachment),
      });
      state.attached.set(sessionId, { attachment, lease: entry });
      let result;
      try {
        result = attachment.attach(resume);
      } catch (error) {
        this.dropAttachment(state, sessionId, true);
        throw new RemoteRpcError('internal', `could not attach (${describeError(error)})`);
      }
      this.viewersChanged();
      return result;
    } finally {
      if (state.attaching.get(sessionId) === attempt) state.attaching.delete(sessionId);
    }
  }

  private requireEnabledError(sessionId: string): RemoteRpcError {
    try {
      this.requireEnabled(sessionId);
    } catch (error) {
      if (error instanceof RemoteRpcError) return error;
    }
    return new RemoteRpcError('not_remote_enabled', 'Remote Control is off for this session');
  }

  private resumeOf(params: unknown): SessionAttachParams['resume'] {
    const resume = (params as Json | null)?.['resume'];
    if (resume === undefined) return undefined;
    if (typeof resume !== 'object' || resume === null || Array.isArray(resume)) throw new RemoteRpcError('invalid_params', 'invalid resume');
    const { epoch, after_seq } = resume as Json;
    if (typeof epoch !== 'string' || epoch.length === 0 || epoch.length > 64
      || typeof after_seq !== 'number' || !Number.isSafeInteger(after_seq) || after_seq < 0) {
      throw new RemoteRpcError('invalid_params', 'invalid resume');
    }
    return { epoch, after_seq };
  }

  private detach(state: ViewerState, sessionId: string): void {
    state.attaching.delete(sessionId);
    this.dropAttachment(state, sessionId, true);
    this.viewersChanged();
  }

  private presence(state: ViewerState, params: unknown): void {
    const sessionId = (params as Json | null)?.['session_id'];
    const foreground = (params as Json | null)?.['foreground'];
    if (typeof sessionId !== 'string' || typeof foreground !== 'boolean') return;
    state.attached.get(sessionId)?.attachment.setPresence(foreground);
  }

  /** Remove a viewer's attachment, close it, and give back its share of the session lease. */
  private dropAttachment(state: ViewerState, sessionId: string, release: boolean): void {
    const attached = state.attached.get(sessionId);
    if (attached === undefined) return;
    state.attached.delete(sessionId);
    attached.attachment.close();
    if (release) this.release(attached.lease);
  }

  /** The attachment's runtime was disposed; the viewer has been told. */
  private dropEnded(state: ViewerState, sessionId: string, attachment: SessionAttachment): void {
    const attached = state.attached.get(sessionId);
    if (attached?.attachment !== attachment) return;
    state.attached.delete(sessionId);
    this.release(attached.lease);
    this.viewersChanged();
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Leases
  // ───────────────────────────────────────────────────────────────────────────

  /** Take a share of the session's lease, opening the session first if it is asleep. */
  private async acquire(ref: SessionRef): Promise<{ entry: SessionLease; runtime: RemoteRuntime }> {
    let entry = this.leases.get(ref.sessionId);
    if (entry === undefined) {
      entry = { refs: 0 };
      this.leases.set(ref.sessionId, entry);
    }
    entry.refs += 1;
    this.deps.clock.clearTimeout(entry.timer);
    entry.timer = undefined;
    try {
      entry.opening ??= this.deps.runtimes.openForRemote(ref).then((lease) => { entry.lease = lease; return lease; });
      const lease = await entry.opening;
      return { entry, runtime: lease.runtime };
    } catch (error) {
      entry.opening = undefined;
      entry.refs -= 1;
      if (entry.refs <= 0 && entry.lease === undefined && this.leases.get(ref.sessionId) === entry) this.leases.delete(ref.sessionId);
      if (error instanceof RemoteCapacityError) throw new RemoteRpcError('capacity', 'too many sessions are open for remote devices');
      this.deps.diagnostics('warn', `could not open session ${ref.sessionId} for a remote device (${describeError(error)})`);
      throw new RemoteRpcError('internal', 'the session could not be opened');
    }
  }

  /** Give back a share; the session is released two minutes after the last one. */
  private release(entry: SessionLease): void {
    entry.refs -= 1;
    if (entry.refs > 0) return;
    if (this.disposed) { this.releaseNow(entry); return; }
    this.deps.clock.clearTimeout(entry.timer);
    entry.timer = this.deps.clock.setTimeout(() => {
      entry.timer = undefined;
      if (entry.refs <= 0) this.releaseNow(entry);
    }, LEASE_GRACE_MS);
  }

  private releaseNow(entry: SessionLease): void {
    this.deps.clock.clearTimeout(entry.timer);
    entry.timer = undefined;
    entry.refs = 0;
    const lease = entry.lease;
    entry.lease = undefined;
    entry.opening = undefined;
    for (const [sessionId, other] of this.leases) if (other === entry) this.leases.delete(sessionId);
    try { lease?.release(); } catch (error) {
      this.deps.diagnostics('warn', `could not release a session lease (${describeError(error)})`);
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // The session list
  // ───────────────────────────────────────────────────────────────────────────

  private async summaries(): Promise<RemoteSessionSummary[]> {
    const entries = await Promise.all(this.deps.policy.enabledSessions().map(async (ref) => ({
      ref,
      title: await this.titleFor(ref),
    })));
    // Title I/O may outlive sharing withdrawal. Recheck all references after
    // every title has settled, before exposing the list to a viewer.
    return entries.flatMap(({ ref, title }) => {
      const current = this.deps.policy.find(ref.sessionId);
      return current === undefined ? [] : [this.summary(current, title)];
    });
  }

  private async titleFor(ref: SessionRef): Promise<string> {
    const cached = this.titles.get(ref.sessionId);
    const now = this.deps.clock.now();
    if (cached !== undefined && now - cached.at < TITLE_TTL_MS) return cached.title;
    let title = cached?.title ?? UNTITLED;
    try {
      title = (await this.deps.titleOf(ref)) || title;
    } catch { /* Keep the last known title. */ }
    this.titles.set(ref.sessionId, { title, at: now });
    return title;
  }

  private summary(ref: SessionRef, title: string): RemoteSessionSummary {
    const runtime = this.deps.runtimes.get(ref.sessionId);
    const viewers = [...this.viewers.values()].filter((state) => state.attached.has(ref.sessionId)).length;
    const summary: RemoteSessionSummary = {
      session_id: ref.sessionId,
      title,
      project_label: basename(ref.projectPath),
      status: 'dormant',
      viewers,
    };
    if (runtime === undefined || runtime.connectionState.status !== 'connected') {
      const model = runtime?.remoteModel ?? this.deps.modelOf?.(ref);
      if (model !== undefined) summary.model = model;
      return summary;
    }
    const pending = runtime.remotePending();
    const action: RemotePendingAction | undefined = pending.permissions.length > 0 ? 'permission'
      : pending.questions.length > 0 ? 'question'
        : pending.computer_access.length > 0 ? 'computer_access' : undefined;
    if (action !== undefined) {
      summary.status = 'requires_action';
      summary.pending_action = action;
    } else {
      summary.status = runtime.turnActive ? 'running' : 'idle';
    }
    const model = runtime.remoteModel;
    if (model !== undefined) summary.model = model;
    return summary;
  }

  private policyChanged(): void {
    // A session switched off: its viewers are told and let go at once.
    for (const state of this.viewers.values()) {
      for (const sessionId of state.attaching.keys()) {
        if (this.deps.policy.find(sessionId) === undefined) state.attaching.delete(sessionId);
      }
      for (const [sessionId, attached] of [...state.attached]) {
        if (this.deps.policy.find(sessionId) !== undefined) continue;
        state.attached.delete(sessionId);
        attached.attachment.close('remote_disabled');
        this.releaseNow(attached.lease);
      }
    }
    this.viewersChanged();
  }

  private watchRuntime(runtime: RemoteRuntime): void {
    const stop = runtime.observeRemote((observation) => {
      this.runtimeChanged(runtime, observation);
      if (observation.type === 'disposed') {
        stop();
        this.watchers.delete(stop);
      }
    });
    this.watchers.add(stop);
  }

  private runtimeChanged(runtime: RemoteRuntime, observation: RemoteObservation): void {
    switch (observation.type) {
      case 'event': {
        const type = observation.envelope.event.type;
        if (type === 'turn_started' || type === 'turn_ended' || type === 'model_changed'
          || type === 'session_started' || type === 'session_resumed' || type === 'session_ended') this.scheduleList();
        return;
      }
      case 'pending':
      case 'state':
        this.scheduleList();
        return;
      case 'disposed':
        // The runtime is gone whatever the lease says; the next attach opens it again.
        for (const [sessionId, entry] of this.leases) {
          if (sessionId === runtime.sessionId && entry.lease?.runtime === runtime) this.releaseNow(entry);
        }
        this.scheduleList();
        return;
      default:
        return;
    }
  }

  private viewersChanged(): void {
    this.scheduleList();
    for (const listener of [...this.viewerListeners]) {
      try { listener(); } catch { /* One listener cannot break the others. */ }
    }
  }

  private scheduleList(): void {
    if (this.listTimer !== undefined || this.disposed) return;
    if (![...this.viewers.values()].some((state) => state.watching)) return;
    this.listTimer = this.deps.clock.setTimeout(() => {
      this.listTimer = undefined;
      void this.publishList();
    }, LIST_DEBOUNCE_MS);
  }

  private async publishList(): Promise<void> {
    const watching = [...this.viewers.values()].filter((state) => state.watching && !state.closed);
    if (watching.length === 0) return;
    let sessions: RemoteSessionSummary[];
    try {
      sessions = await this.summaries();
    } catch (error) {
      this.deps.diagnostics('warn', `could not build the session list (${describeError(error)})`);
      return;
    }
    const serialized = JSON.stringify(sessions);
    for (const state of watching) {
      if (state.closed || state.lastList === serialized) continue;
      state.lastList = serialized;
      state.channel.notify('sessions.changed', { sessions });
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // PushPresence
  // ───────────────────────────────────────────────────────────────────────────

  isEnabled(sessionId: string): boolean {
    return this.deps.policy.find(sessionId) !== undefined;
  }

  foregroundViewers(sessionId: string): number {
    return [...this.viewers.values()].filter((state) => state.attached.get(sessionId)?.attachment.foreground === true).length;
  }

  attachedInstallationIds(sessionId: string): string[] {
    const ids = new Set<string>();
    for (const state of this.viewers.values()) {
      const id = state.viewer.push_installation_id;
      if (id !== undefined && state.attached.has(sessionId)) ids.add(id);
    }
    return [...ids];
  }

  /** How many viewers are attached to this session, or to any session. */
  attachedViewers(sessionId?: string): number {
    return [...this.viewers.values()].filter((state) => (sessionId === undefined ? state.attached.size > 0 : state.attached.has(sessionId))).length;
  }

  /** Be told whenever viewers attach, detach or leave (for the desktop's per-session indicator). */
  onViewersChanged(listener: () => void): () => void {
    this.viewerListeners.add(listener);
    return () => { this.viewerListeners.delete(listener); };
  }
}
