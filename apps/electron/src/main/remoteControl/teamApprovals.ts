import { randomBytes } from 'node:crypto';
import { RemoteRpcError, type RemotePending, type RemoteSessionSummary } from '@lingxi/bridge-client/remote';
import type { TeamBotTarget } from '../teams/process.js';
import type { RemoteSessionGateway, RemoteViewer, RemoteViewerChannel } from './gateway.js';

/** A reserved approval capability, never a SessionRuntime or an executable chat. */
export const TEAM_APPROVAL_PREFIX = 'team-approval:';
const MAX_PENDING = 32;
const MAX_INPUT_BYTES = 64 * 1024;
const MAX_WAIT_MS = 5 * 60 * 1000;
type Decision = 'allow_once' | 'deny';
type Json = Record<string, unknown>;
const object = (value: unknown): value is Json => value !== null && typeof value === 'object' && !Array.isArray(value);
const exact = (value: unknown, keys: string[]): value is Json => object(value) && Object.keys(value).every(key => keys.includes(key));

export interface TeamOwnerApprovalRequest {
  target: TeamBotTarget;
  accountEpoch: number;
  toolName: string;
  input: unknown;
  /** Host-owned check of the captured account, invocation and exact binding. */
  assertCurrent(): void;
  /** A native owner dialog competes for the very same single-use request. */
  showLocal?(signal: AbortSignal): Promise<boolean>;
}
export interface PendingTeamOwnerApproval {
  readonly nonce: string;
  readonly target: Readonly<TeamBotTarget>;
  readonly accountEpoch: number;
  readonly toolName: string;
  readonly inputJson: string;
  readonly expiresAt: number;
}
interface Entry {
  view: PendingTeamOwnerApproval;
  request: TeamOwnerApprovalRequest;
  signal: AbortSignal;
  controller: AbortController;
  resolve(value: boolean): void;
  timer: ReturnType<typeof setTimeout>;
  aborted(): void;
}

/** Host memory only. No credentials, execution methods, question answers or storage. */
export class TeamOwnerApprovalBroker {
  private readonly pending = new Map<string, Entry>();
  private readonly listeners = new Set<() => void>();
  private closed = false;
  constructor(private readonly options: { botRemoteEnabled(botId: string): boolean; now?: () => number }) {}
  private now(): number { return (this.options.now ?? Date.now)(); }
  private remote(entry: Entry): boolean { try { return this.options.botRemoteEnabled(entry.view.target.botId) === true; } catch { return false; } }

  request(input: TeamOwnerApprovalRequest, signal: AbortSignal): Promise<boolean> {
    if (this.closed || signal.aborted || this.pending.size >= MAX_PENDING) return Promise.resolve(false);
    let inputJson: string;
    try {
      input.assertCurrent();
      inputJson = JSON.stringify(input.input);
      if (typeof inputJson !== 'string' || Buffer.byteLength(inputJson) > MAX_INPUT_BYTES || !input.toolName || input.toolName.length > 200) return Promise.resolve(false);
    } catch { return Promise.resolve(false); }
    const nonce = randomBytes(24).toString('base64url');
    const view = Object.freeze({ nonce, target: Object.freeze({ ...input.target }), accountEpoch: input.accountEpoch,
      toolName: input.toolName, inputJson, expiresAt: this.now() + MAX_WAIT_MS });
    return new Promise<boolean>(resolve => {
      const controller = new AbortController();
      const entry: Entry = { view, request: input, signal, controller, resolve,
        timer: setTimeout(() => this.settle(nonce, false), MAX_WAIT_MS), aborted: () => { this.settle(nonce, false); } };
      entry.timer.unref(); this.pending.set(nonce, entry); signal.addEventListener('abort', entry.aborted, { once: true });
      if (signal.aborted) { this.settle(nonce, false); return; }
      this.emit();
      if (input.showLocal) void Promise.resolve().then(() => {
        if (controller.signal.aborted) return false;
        return input.showLocal!(controller.signal);
      }).then(allow => this.settle(nonce, allow === true), () => this.settle(nonce, false));
    });
  }
  onChanged(listener: () => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  list(ownerUid: string, accountEpoch: number): PendingTeamOwnerApproval[] {
    const result: PendingTeamOwnerApproval[] = [];
    for (const [nonce, entry] of this.pending) {
      if (!this.current(entry)) { this.settle(nonce, false); continue; }
      if (entry.view.target.ownerUid === ownerUid && entry.view.accountEpoch === accountEpoch && this.remote(entry)) result.push(entry.view);
    }
    return result;
  }
  respond(nonce: string, decision: Decision, ownerUid: string, accountEpoch: number): void {
    const entry = this.pending.get(nonce);
    if (!entry || entry.view.target.ownerUid !== ownerUid || entry.view.accountEpoch !== accountEpoch || !this.current(entry) || !this.remote(entry)) {
      if (entry && !this.current(entry)) this.settle(nonce, false);
      throw new RemoteRpcError('not_pending', 'the Team approval is no longer pending');
    }
    if (decision !== 'allow_once' && decision !== 'deny') throw new RemoteRpcError('forbidden', 'Team tools allow only allow once or deny');
    this.settle(nonce, decision === 'allow_once');
  }
  close(): void { this.closed = true; for (const nonce of [...this.pending.keys()]) this.settle(nonce, false); this.listeners.clear(); }
  private current(entry: Entry): boolean {
    if (this.closed || entry.signal.aborted || entry.view.expiresAt <= this.now()) return false;
    try { entry.request.assertCurrent(); return true; } catch { return false; }
  }
  private settle(nonce: string, allow: boolean): void {
    const entry = this.pending.get(nonce); if (!entry) return;
    const accepted = allow && this.current(entry);
    this.pending.delete(nonce); clearTimeout(entry.timer); entry.signal.removeEventListener('abort', entry.aborted);
    entry.controller.abort(); entry.resolve(accepted); this.emit();
  }
  private emit(): void { for (const listener of this.listeners) { try { listener(); } catch { /* A viewer cannot change resolution. */ } } }
}

export interface TeamApprovalsBindings {
  ownerContext(): { uid: string; epoch: number } | undefined;
  hostId(): string | undefined;
  active(): boolean;
  /** Check an approved, currently live conn_id/device_id, not only a remembered device. */
  approvedViewer(viewer: RemoteViewer): boolean;
  /** Changes on RC stop, approval revocation and account change. */
  generation(viewer: RemoteViewer): string | number;
  /** Current local Bot.remote switch; no ordinary Bot runtime is consulted. */
  remoteEnabled(target: Readonly<TeamBotTarget>): boolean;
}
interface Viewer {
  identity: RemoteViewer;
  channel: RemoteViewerChannel;
  epoch: string | number;
  watching: boolean;
  attached: Set<string>;
  revision: number;
}

/**
 * Existing session/pending DTOs carry owner-only cards over the encrypted RC
 * channel. The reserved namespace is intercepted BEFORE any ordinary gateway;
 * even an unknown nonce cannot become a prompt, file, tool, media or question API.
 */
export class TeamApprovalsGateway implements RemoteSessionGateway {
  private readonly viewers = new Map<string, Viewer>();
  private readonly stop: () => void;
  private closed = false;
  constructor(private readonly base: RemoteSessionGateway, private readonly broker: TeamOwnerApprovalBroker, private readonly bindings: TeamApprovalsBindings) {
    this.stop = broker.onChanged(() => { for (const state of this.viewers.values()) this.changed(state); });
  }
  async handle(viewer: RemoteViewer, method: string, params: unknown, channel: RemoteViewerChannel): Promise<unknown> {
    const id = object(params) ? params['session_id'] : undefined;
    if (typeof id === 'string' && id.startsWith(TEAM_APPROVAL_PREFIX)) return this.approval(viewer, method, params, channel, id);
    if (method !== 'sessions.list' && method !== 'sessions.watch') return this.base.handle(viewer, method, params, this.channel(viewer, channel));
    const state = this.state(viewer, channel); this.scope(state);
    if (method === 'sessions.watch') state.watching = true;
    const result = await this.base.handle(viewer, method, params, this.channel(viewer, channel));
    const rows = this.rows(state);
    if (!object(result) || !Array.isArray(result['sessions'])) throw new RemoteRpcError('internal', 'invalid session catalog');
    return { ...result, sessions: [...result['sessions'], ...rows.map(row => this.summary(row))] };
  }
  viewerClosed(viewer: RemoteViewer): void { this.viewers.delete(viewer.conn_id); this.base.viewerClosed(viewer); }
  close(): void { this.closed = true; this.stop(); this.viewers.clear(); }
  private scope(state: Viewer): { uid: string; epoch: number } {
    const owner = this.bindings.ownerContext(), viewer = state.identity;
    if (this.closed || !owner || !this.bindings.active() || !viewer.account_uid || owner.uid !== viewer.account_uid ||
      !viewer.host_id || viewer.host_id !== this.bindings.hostId() || !this.bindings.approvedViewer(viewer) || state.epoch !== this.bindings.generation(viewer)) {
      throw new RemoteRpcError('not_approved', 'Team owner approval access changed');
    }
    return owner;
  }
  private rows(state: Viewer): PendingTeamOwnerApproval[] {
    const owner = this.scope(state);
    const rows = this.broker.list(owner.uid, owner.epoch).filter(row => this.bindings.remoteEnabled(row.target));
    this.scope(state); return rows;
  }
  private summary(row: PendingTeamOwnerApproval): RemoteSessionSummary {
    return { session_id: TEAM_APPROVAL_PREFIX + row.nonce, title: `Team tool approval: ${row.toolName}`, project_label: 'Allow once / Deny only', status: 'requires_action', pending_action: 'permission', viewers: 0 };
  }
  private pending(row: PendingTeamOwnerApproval): RemotePending {
    return { permissions: [{ request_id: 1, suppress_always_allow_rule: true,
      kind: { type: 'tool_use_confirm', default_allow: false, tool_name: row.toolName, tool_input_json: JSON.stringify({ approval: "Allow once / Deny only", team: row.target.teamId, bot: row.target.botId, binding_revision: row.target.botRevision, conversation: row.target.conversationId, input: JSON.parse(row.inputJson) }) } }], questions: [], computer_access: [] };
  }
  private state(viewer: RemoteViewer, channel: RemoteViewerChannel): Viewer {
    let state = this.viewers.get(viewer.conn_id);
    if (state && (state.identity.account_uid !== viewer.account_uid || state.identity.device_id !== viewer.device_id || state.identity.host_id !== viewer.host_id)) throw new RemoteRpcError('not_approved', 'connection identity changed');
    if (!state) { state = { identity: { ...viewer }, channel, epoch: this.bindings.generation(viewer), watching: false, attached: new Set(), revision: 0 }; this.viewers.set(viewer.conn_id, state); }
    return state;
  }
  private channel(viewer: RemoteViewer, channel: RemoteViewerChannel): RemoteViewerChannel {
    return { ...(channel.files ? { files: channel.files } : {}), request: (method, params) => channel.request(method, params), notify: (method, params) => {
      if (method !== 'sessions.changed') { channel.notify(method, params); return; }
      try {
        const state = this.state(viewer, channel), rows = this.rows(state);
        if (object(params) && Array.isArray(params['sessions'])) channel.notify(method, { ...params, sessions: [...params['sessions'], ...rows.map(row => this.summary(row))] });
      } catch { /* Revoked viewer gets no Team payload. */ }
    } };
  }
  private approval(viewer: RemoteViewer, method: string, params: unknown, channel: RemoteViewerChannel, id: string): unknown {
    const state = this.state(viewer, channel); const owner = this.scope(state);
    if (method === 'session.detach') { state.attached.delete(id); return {}; }
    if (method === 'session.presence') return undefined;
    if (method !== 'session.attach' && method !== 'permission.respond') throw new RemoteRpcError('forbidden', 'Team approval capabilities allow only allow once or deny');
    const row = this.rows(state).find(row => TEAM_APPROVAL_PREFIX + row.nonce === id);
    if (!row) throw new RemoteRpcError('not_pending', 'the Team approval is no longer pending');
    if (method === 'session.attach') {
      if (!exact(params, ['session_id', 'resume'])) throw new RemoteRpcError('invalid_params', 'invalid Team approval attachment');
      state.attached.add(id);
      channel.notify('session.snapshot', { phase: 'begin', session_id: id, epoch: row.nonce, first_message_index: 0 });
      channel.notify('session.snapshot', { phase: 'end', session_id: id, cursor: { epoch: row.nonce, seq: 0 }, pending: this.pending(row), turn: { running: false } });
      return { mode: 'snapshot', epoch: row.nonce };
    }
    if (!state.attached.has(id) || !exact(params, ['session_id', 'request_id', 'decision']) || params['request_id'] !== 1 ||
      !exact(params['decision'], ['type']) || !['allow_once', 'deny'].includes(String(params['decision']['type']))) throw new RemoteRpcError('forbidden', 'Team tools allow only allow once or deny');
    this.scope(state);
    if (!this.bindings.remoteEnabled(row.target)) throw new RemoteRpcError('not_approved', 'Bot Remote Control is off');
    this.broker.respond(row.nonce, params['decision']['type'] as Decision, owner.uid, owner.epoch);
    return {};
  }
  private changed(state: Viewer): void {
    const revision = ++state.revision;
    try {
      const rows = this.rows(state), present = new Set(rows.map(row => TEAM_APPROVAL_PREFIX + row.nonce));
      for (const id of state.attached) if (!present.has(id)) { state.attached.delete(id); state.channel.notify('session.pending', { session_id: id, pending: { permissions: [], questions: [], computer_access: [] } }); state.channel.notify('session.closed', { session_id: id, reason: 'ended' }); }
      if (state.watching) void this.base.handle(state.identity, 'sessions.list', {}, this.channel(state.identity, state.channel)).then(result => {
        if (this.viewers.get(state.identity.conn_id) !== state || state.revision !== revision) return;
        const current = this.rows(state);
        if (object(result) && Array.isArray(result['sessions'])) state.channel.notify('sessions.changed', { sessions: [...result['sessions'], ...current.map(row => this.summary(row))] });
      }).catch(() => {});
    } catch { /* Authorization changes do not expose request details. */ }
  }
}
