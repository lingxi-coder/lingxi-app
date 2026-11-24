/**
 * What a Bot asks of its owner: a folder to work in. A Bot calls `request_workspace` from its
 * session; the call only records the request. The owner answers it on this computer, in the
 * window or from the notification, and nothing is shared before they approve: the engine's own
 * permission prompt is not the owner's decision here, because a Bot that runs with full access
 * would approve itself. Approving binds the folder to the Bot like any folder the owner adds, and
 * the Bot's open chats are given it as soon as they are not working.
 *
 * Requests are kept in memory. A restart forgets them, and a Bot that still wants the folder asks
 * again. Free of Electron.
 */

import { randomBytes } from 'node:crypto';
import { isAbsolute, resolve } from 'node:path';

import { DelegationError, type Caller } from './routerTools.js';
import type { BotRecord, BotStore, BotWorkspaceInput, BotWorkspaceMode } from './store.js';

export const MAX_PENDING_PER_BOT = 8;
export const MAX_REASON_CHARS = 300;
/** A request nobody answered is forgotten after this long. */
export const REQUEST_TTL_MS = 24 * 60 * 60 * 1000;
/** After the owner declined a folder, the same Bot is not let ask for it again for this long. */
export const DECLINE_COOLDOWN_MS = 10 * 60 * 1000;
const MAX_DECIDED_KEPT = 64;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/g;

export type WorkspaceRequestState = 'pending' | 'approved' | 'denied' | 'expired';

export interface WorkspaceRequest {
  id: string;
  botId: string;
  /** The session that asked. */
  sessionId: string;
  /** A real path the Bot may be given. */
  path: string;
  mode: BotWorkspaceMode;
  reason: string;
  createdAt: number;
  state: WorkspaceRequestState;
  decidedAt?: number;
}

/** What a call to `request_workspace` is answered with. */
export interface WorkspaceRequestView {
  request_id: string;
  path: string;
  mode: BotWorkspaceMode;
  status: { state: WorkspaceRequestState; message?: string };
}

/** What the router asks of the requests. */
export interface WorkspaceBackend {
  request(caller: Caller, args: { path: string; mode: BotWorkspaceMode; reason: string; waitSeconds: number }, signal: AbortSignal): Promise<WorkspaceRequestView>;
}

export interface WorkspaceRequestsOptions {
  store: BotStore;
  /** Give the Bot the folder, or the mode, as the owner would have: settings written, open chats brought along, everyone told. */
  grant(botId: string, input: BotWorkspaceInput): Promise<BotRecord>;
  /** Whether Bots can run, and the Bot is not being deleted. */
  usable(botId: string): boolean;
  /** These Bots' requests changed: the window reads them again. */
  changed(botIds: readonly string[]): void;
  /** A request is waiting for the owner. Called once for each. */
  notify?(request: WorkspaceRequest, botName: string): void;
  now?: () => number;
  newId?: () => string;
  warn?: (message: string) => void;
}

function oneLine(value: string, max: number): string {
  return value.replace(CONTROL, ' ').replace(/\s+/g, ' ').trim().slice(0, max).trim();
}

export class WorkspaceRequests implements WorkspaceBackend {
  private readonly requests = new Map<string, WorkspaceRequest>();
  private readonly waiters = new Map<string, Set<() => void>>();
  private readonly now: () => number;
  private readonly newId: () => string;

  constructor(private readonly options: WorkspaceRequestsOptions) {
    this.now = options.now ?? Date.now;
    this.newId = options.newId ?? (() => randomBytes(12).toString('base64url'));
  }

  /** What the owner has yet to answer, oldest first: of one Bot, or of all. */
  pending(botId?: string): WorkspaceRequest[] {
    this.expire();
    return [...this.requests.values()]
      .filter((request) => request.state === 'pending' && (botId === undefined || request.botId === botId))
      .sort((left, right) => left.createdAt - right.createdAt)
      .map((request) => ({ ...request }));
  }

  /** A Bot asks for a folder. The call is answered when the owner has, or when it has waited `waitSeconds`. */
  async request(caller: Caller, args: { path: string; mode: BotWorkspaceMode; reason: string; waitSeconds: number }, signal: AbortSignal): Promise<WorkspaceRequestView> {
    this.expire();
    const { store } = this.options;
    if (!this.options.usable(caller.botId)) throw new DelegationError('unavailable', 'Bots cannot ask for folders right now.');
    let record: BotRecord;
    try { record = store.get(caller.botId); } catch { throw new DelegationError('unavailable', 'You are not a Bot of this computer any more.'); }
    const reason = oneLine(args.reason, MAX_REASON_CHARS);
    if (reason === '') throw new DelegationError('invalid', 'Say what you need the folder for: the owner reads it.');
    // The Bot's own folder is in the app's data, which a Bot may not be given: say what the Bot needs to hear.
    if (isAbsolute(args.path) && resolve(args.path) === store.workspaceDir(record.bot_id)) throw new DelegationError('invalid', 'That is your own folder; you have it already.');
    let checked: { path: string; mode: BotWorkspaceMode };
    try { checked = store.workspace(args.path, args.mode); } catch (error) {
      throw new DelegationError('invalid', error instanceof Error ? error.message : 'That folder cannot be used.');
    }
    const held = record.workspaces.find((workspace) => workspace.path === checked.path);
    if (held !== undefined && (held.mode === 'write' || checked.mode === 'read')) {
      return { request_id: '', path: checked.path, mode: held.mode, status: { state: 'approved', message: 'You have this folder already.' } };
    }
    const same = [...this.requests.values()].filter((request) => request.botId === record.bot_id && request.path === checked.path);
    const waiting = same.find((request) => request.state === 'pending' && (request.mode === 'write' || checked.mode === 'read'));
    if (waiting !== undefined) return this.waitFor(waiting, args.waitSeconds, signal);
    const declined = same.find((request) => request.state === 'denied' && this.now() - (request.decidedAt ?? 0) < DECLINE_COOLDOWN_MS
      && (request.mode === 'read' || checked.mode === 'write'));
    if (declined !== undefined) return this.view(declined, 'The owner declined this folder a moment ago. Do not ask again; work with what you have.');
    // A pending request of the same folder is replaced below, so it does not count.
    if (this.pending(record.bot_id).filter((request) => request.path !== checked.path).length >= MAX_PENDING_PER_BOT) throw new DelegationError('busy', 'Your owner has several requests of yours to answer already. Wait for them.');
    // A request for more than an earlier one of the same folder takes its place.
    for (const request of same) if (request.state === 'pending') this.settle(request, 'expired');
    const request: WorkspaceRequest = {
      id: this.newId(), botId: record.bot_id, sessionId: caller.sessionId, path: checked.path, mode: checked.mode, reason, createdAt: this.now(), state: 'pending',
    };
    this.requests.set(request.id, request);
    this.options.changed([record.bot_id]);
    try { this.options.notify?.({ ...request }, record.name); } catch (error) {
      this.options.warn?.(`a folder request could not be announced: ${error instanceof Error ? error.message : String(error)}`);
    }
    return this.waitFor(request, args.waitSeconds, signal);
  }

  /**
   * The owner answers. Approving gives the Bot the folder (or the mode) and rejects, leaving the
   * request to be answered again, if that cannot be done: the folder has gone, say.
   */
  async decide(botId: string, requestId: string, decision: 'approve' | 'deny'): Promise<WorkspaceRequest> {
    this.expire();
    const request = this.requests.get(requestId);
    if (request === undefined || request.botId !== botId || request.state !== 'pending') throw new Error('This request was answered already, or has expired.');
    if (decision === 'approve') {
      await this.options.grant(botId, { path: request.path, mode: request.mode });
      // Another answer may have come while the folder was being given.
      if (request.state !== 'pending') return { ...request };
      this.settle(request, 'approved');
    } else if (decision === 'deny') {
      this.settle(request, 'denied');
    } else {
      throw new Error('invalid decision');
    }
    return { ...request };
  }

  /** A Bot is gone: what it asked for is not asked any more. */
  dropBot(botId: string): void {
    let touched = false;
    for (const request of [...this.requests.values()]) {
      if (request.botId !== botId) continue;
      touched = touched || request.state === 'pending';
      // A call still waiting on it hears that it ended, not that it is still pending.
      if (request.state === 'pending') { request.state = 'expired'; request.decidedAt = this.now(); }
      this.requests.delete(request.id);
      this.wake(request.id);
    }
    if (touched) this.options.changed([botId]);
  }

  private settle(request: WorkspaceRequest, state: Exclude<WorkspaceRequestState, 'pending'>): void {
    request.state = state;
    request.decidedAt = this.now();
    this.options.changed([request.botId]);
    this.wake(request.id);
    this.prune();
  }

  private expire(): void {
    const cutoff = this.now() - REQUEST_TTL_MS;
    for (const request of this.requests.values()) {
      if (request.state === 'pending' && request.createdAt < cutoff) this.settle(request, 'expired');
    }
  }

  /** Answered requests are kept for a while, so that the Bot that waits for one hears how it ended. */
  private prune(): void {
    const decided = [...this.requests.values()].filter((request) => request.state !== 'pending').sort((left, right) => (left.decidedAt ?? 0) - (right.decidedAt ?? 0));
    for (const request of decided.slice(0, Math.max(0, decided.length - MAX_DECIDED_KEPT))) this.requests.delete(request.id);
  }

  private wake(requestId: string): void {
    const waiting = this.waiters.get(requestId);
    this.waiters.delete(requestId);
    for (const resume of waiting ?? []) resume();
  }

  private view(request: WorkspaceRequest, message?: string): WorkspaceRequestView {
    const said = message ?? {
      pending: 'Your owner has been asked on this computer. Carry on with what you can do; the folder is yours from your next turn once they approve.',
      approved: 'Approved. The folder is yours from your next turn.',
      denied: 'The owner declined. Work with the folders you have.',
      expired: 'The request ran out without an answer. Ask again if you still need the folder.',
    }[request.state];
    return { request_id: request.id, path: request.path, mode: request.mode, status: { state: request.state, message: said } };
  }

  private async waitFor(request: WorkspaceRequest, waitSeconds: number, signal: AbortSignal): Promise<WorkspaceRequestView> {
    if (request.state === 'pending' && waitSeconds > 0 && !signal.aborted) {
      await new Promise<void>((resolve) => {
        const finish = (): void => { clearTimeout(timer); signal.removeEventListener('abort', finish); this.waiters.get(request.id)?.delete(finish); resolve(); };
        const timer = setTimeout(finish, waitSeconds * 1000);
        timer.unref?.();
        signal.addEventListener('abort', finish, { once: true });
        const set = this.waiters.get(request.id) ?? new Set<() => void>();
        set.add(finish);
        this.waiters.set(request.id, set);
      });
    }
    return this.view(request);
  }
}
