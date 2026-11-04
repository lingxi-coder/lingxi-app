/**
 * A Bot's chats for a viewer that understands Bots: the Bots whose Remote
 * Control switch is on, a Bot's chats, and starting a new one. The chats
 * themselves are ordinary sessions; a viewer uses them through the gateway's
 * session methods with their `session_id`, and `policy.ts` decides whether it
 * may (the Bot's switch, on top of the computer's and the administrator's).
 *
 * v1 has no principal on the wire: whoever asks is the approved owner device of
 * the connection. A member principal, when there is one, belongs next to
 * `bot_id` in the parameters here and in the policy's `botChat`.
 */

import type { SessionRowDto } from '@lingxi/bridge-client';
import {
  RemoteRpcError,
  type BotThreadCreateResult,
  type BotThreadsListResult,
  type Clock,
  type RemoteBotStatus,
  type RemoteBotSummary,
  type RemotePending,
  type RemoteSessionSummary,
} from '@lingxi/bridge-client/remote';

import type { SessionRef } from '../bridgeTypes.js';
import { describeError, type RemoteDiagnostics } from './diagnostics.js';
import type { RemoteViewer } from './gateway.js';
import { IDEMPOTENCY_MAX_ENTRIES, IDEMPOTENCY_WINDOW_MS } from './remoteCommands.js';
import type { RemoteRuntime } from './remoteRuntime.js';

/** A Bot's chats are listed newest first, this many at most. */
export const MAX_BOT_THREADS = 50;
const MAX_ID_CHARS = 128;

/** A Bot a remote device may see. */
export interface RemoteBot {
  botId: string;
  name: string;
  description: string;
  /** The working directory of the Bot's sessions. */
  workspace: string;
}

/** Where a Bot stands for remote devices. */
export type RemoteBotLookup =
  | { state: 'on'; bot: RemoteBot }
  /** Its Remote Control switch is off. */
  | { state: 'off' }
  | { state: 'deleting' }
  /** There is no such Bot, or Bots cannot run on this computer. */
  | { state: 'unknown' };

/** A chat of a Bot, found without reading the disk to the end. */
export interface BotChat {
  botId: string;
  /** `projectPath` is the Bot's workspace. */
  ref: SessionRef;
  /** Whether the Bot's switch is on and the Bot is not being deleted. */
  remote: boolean;
}

/** What of the Bots Remote Control uses. `BotService` provides it. */
export interface RemoteBotDirectory {
  /** The Bots whose switch is on, oldest first; none where Bots cannot run, and not one that is being deleted. */
  remoteBots(): RemoteBot[];
  remoteBot(botId: string): RemoteBotLookup;
  /**
   * The Bot a chat belongs to, if it belongs to one. Synchronous, because every
   * request asks: a chat open in a Bot's workspace, or a transcript in a Bot's
   * home. The hidden session that watches the clock is no chat.
   */
  chat(sessionId: string): BotChat | undefined;
  /** Whether a Bot whose switch is on works in this folder. */
  remoteEnabled(workspace: string): boolean;
  /** Whether the folder is shaped like a Bot's workspace, whether or not the Bot exists. */
  isBotWorkspace(path: string): boolean;
  /** The Bot's chats, read from its home. */
  listSessions(botId: string): Promise<SessionRowDto[]>;
  /** Epoch milliseconds of the newest chat file, if there is one. */
  lastActivityAt(botId: string): Promise<number | undefined>;
  /** A new chat of the Bot, as the Bots page starts one. */
  openSession(botId: string): Promise<{ sessionId: string }>;
}

export interface RemoteBotsDeps {
  directory(): RemoteBotDirectory | null | undefined;
  /** Every runtime that is open now. */
  runtimes(): Iterable<RemoteRuntime>;
  /** A chat as `sessions.list` would describe it. */
  summary(ref: SessionRef, title: string | undefined): RemoteSessionSummary;
  clock: Clock;
  diagnostics: RemoteDiagnostics;
}

interface CreatedThread {
  at: number;
  result: Promise<BotThreadCreateResult>;
}

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);

function invalid(message: string): RemoteRpcError {
  return new RemoteRpcError('invalid_params', message);
}

function params(value: unknown): Json {
  if (!isObject(value)) throw invalid('params must be an object');
  return value;
}

function boundedString(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_ID_CHARS || value.includes('\0')) throw invalid(`invalid ${name}`);
  return value;
}

const waitingOn = (pending: RemotePending): boolean => pending.permissions.length > 0 || pending.questions.length > 0 || pending.computer_access.length > 0;

/** Newest first; a row without a readable time comes last. */
function newestFirst(left: SessionRowDto, right: SessionRowDto): number {
  const time = (row: SessionRowDto): number => { const ms = Date.parse(row.modified_rfc3339); return Number.isNaN(ms) ? 0 : ms; };
  return time(right) - time(left) || left.uuid.localeCompare(right.uuid);
}

export class RemoteBots {
  private readonly created = new Map<string, CreatedThread>();

  constructor(private readonly deps: RemoteBotsDeps) {}

  /** Whether a runtime works in a folder shaped like a Bot's: its changes can change what `bots.list` says. */
  concerns(runtime: RemoteRuntime): boolean {
    return this.deps.directory()?.isBotWorkspace(runtime.projectPath) === true;
  }

  /** `bots.list` and `bots.watch`. */
  async list(): Promise<RemoteBotSummary[]> {
    const directory = this.deps.directory();
    if (!directory) return [];
    const listed = directory.remoteBots();
    const activity = await Promise.all(listed.map((bot) => directory.lastActivityAt(bot.botId).catch(() => undefined)));
    const states = this.states(directory);
    // The disk was read meanwhile: a switch that went off since must not be shown.
    const current = new Map(directory.remoteBots().map((bot) => [bot.botId, bot]));
    return listed.flatMap((first, index) => {
      const bot = current.get(first.botId);
      if (bot === undefined) return [];
      const { waiting, running } = states.get(bot.botId) ?? { waiting: 0, running: false };
      const status: RemoteBotStatus = waiting > 0 ? 'requires_action' : running ? 'running' : 'idle';
      const last = activity[index];
      return [{
        bot_id: bot.botId,
        name: bot.name,
        description: bot.description,
        status,
        waiting_threads: waiting,
        ...(last !== undefined ? { last_activity_at: last } : {}),
      }];
    });
  }

  /** What the open runtimes say about each Bot's chats: how many wait for an answer, and whether one runs. */
  private states(directory: RemoteBotDirectory): Map<string, { waiting: number; running: boolean }> {
    const states = new Map<string, { waiting: number; running: boolean }>();
    for (const runtime of this.deps.runtimes()) {
      if (runtime.connectionState.status !== 'connected') continue;
      const botId = directory.chat(runtime.sessionId)?.botId;
      if (botId === undefined) continue;
      const state = states.get(botId) ?? { waiting: 0, running: false };
      if (waitingOn(runtime.remotePending())) state.waiting += 1;
      else if (runtime.turnActive) state.running = true;
      states.set(botId, state);
    }
    return states;
  }

  /** `bot.threads.list`. */
  async threads(value: unknown): Promise<BotThreadsListResult> {
    const botId = boundedString(params(value)['bot_id'], 'bot_id');
    const rows = await this.reachable(botId).directory.listSessions(botId).catch((error: unknown) => {
      this.deps.diagnostics('warn', `could not list the chats of a Bot (${describeError(error)})`);
      throw new RemoteRpcError('internal', 'the chats could not be listed');
    });
    // The disk was read meanwhile: a Bot switched off since shows nothing.
    const { bot } = this.reachable(botId);
    const sessions = [...rows].sort(newestFirst).slice(0, MAX_BOT_THREADS).map((row) => ({
      ...this.deps.summary({ projectPath: bot.workspace, sessionId: row.uuid }, row.title),
      project_label: bot.name,
      bot_id: botId,
    }));
    return { sessions };
  }

  /**
   * `bot.thread.create`. A retry with the same `client_request_id` from the same
   * device gets the first answer instead of a second chat, like `prompt.send`.
   */
  create(viewer: RemoteViewer, value: unknown): Promise<BotThreadCreateResult> {
    const input = params(value);
    const botId = boundedString(input['bot_id'], 'bot_id');
    const clientRequestId = boundedString(input['client_request_id'], 'client_request_id');
    const { directory } = this.reachable(botId);
    const key = `${viewer.device_id}\0${botId}\0${clientRequestId}`;
    const now = this.deps.clock.now();
    const earlier = this.created.get(key);
    if (earlier !== undefined && now - earlier.at < IDEMPOTENCY_WINDOW_MS) return earlier.result;
    this.prune(now);
    const result = directory.openSession(botId).then(({ sessionId }) => ({ session_id: sessionId }), (error: unknown) => {
      this.deps.diagnostics('warn', `could not start a chat of a Bot (${describeError(error)})`);
      throw new RemoteRpcError('internal', 'the chat could not be started');
    });
    this.created.set(key, { at: now, result });
    // A chat that could not be started may be asked for again.
    result.catch(() => { if (this.created.get(key)?.result === result) this.created.delete(key); });
    return result;
  }

  private prune(now: number): void {
    for (const [key, entry] of this.created) {
      if (now - entry.at >= IDEMPOTENCY_WINDOW_MS || this.created.size >= IDEMPOTENCY_MAX_ENTRIES) this.created.delete(key);
    }
  }

  /** The Bot, if a remote device may use it; otherwise the error that says why not. */
  private reachable(botId: string): { directory: RemoteBotDirectory; bot: RemoteBot } {
    const directory = this.deps.directory();
    if (!directory) throw new RemoteRpcError('unknown_session', 'no such Bot');
    const lookup = directory.remoteBot(botId);
    switch (lookup.state) {
      case 'on':
        return { directory, bot: lookup.bot };
      case 'off':
        throw new RemoteRpcError('not_remote_enabled', 'Remote Control is off for this Bot');
      case 'deleting':
        throw new RemoteRpcError('unknown_session', 'this Bot is being deleted');
      case 'unknown':
        throw new RemoteRpcError('unknown_session', 'no such Bot');
    }
  }
}
