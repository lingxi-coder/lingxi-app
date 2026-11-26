import { executionCommand, type ExecutionScope, type HeldReply, type HeldReplyDecision, type SharedBotCard, type TaskState, type TeamDocument } from './execution.js';
import { HELD_REPLY_BYTES } from './execution-validation.js';
import type { TeamsApi } from './index.js';
import type { TeamConversation } from './messages.js';
import { maintainsBot } from './triage-log.js';

/**
 * The replies of a Bot that wait for its maintainer to let them go (Bot v3, B6). The service keeps a held reply where only the Bot's maintainer can read it,
 * so these three reads and writes are the one way a client ever sees one; every other member sees no more than that the task awaits approval. A reply lives ten
 * minutes: it is posted by an approval, and dropped by a rejection, by the time running out, and, when nobody asked the Bot, by a newer message of the Chat.
 */

const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const word = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
const moment = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= 8.64e15;
const generation = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

function teamDocument(value: unknown): TeamDocument | undefined {
  return record(value) && word(value.title) && typeof value.body === 'string' ? { title: value.title, body: value.body } : undefined;
}

/** One held reply as the service sent it, or nothing when this version cannot read it: a reply is never half shown, and one that is not understood does not hide the others. */
export function heldReply(value: unknown): HeldReply | undefined {
  if (!record(value) || !record(value.requester)) return undefined;
  const { requester } = value, shown = value.document === undefined ? undefined : teamDocument(value.document);
  if (!word(value.taskId) || !word(value.conversationId) || !word(value.messageId) || !word(requester.uid) || !generation(requester.generation)) return undefined;
  if (typeof value.unasked !== 'boolean' || !moment(value.heldAt) || !moment(value.expiresAt) || !word(value.text)) return undefined;
  // A document that cannot be read is no reply to approve: the maintainer would be letting go of something they did not see.
  if (value.document !== undefined && !shown) return undefined;
  return {
    taskId: value.taskId, conversationId: value.conversationId, messageId: value.messageId, requester: { uid: requester.uid, generation: requester.generation },
    unasked: value.unasked, heldAt: value.heldAt, expiresAt: value.expiresAt, text: value.text,
    ...(shown ? { document: shown } : {}), ...(word(value.conversationTitle) ? { conversationTitle: value.conversationTitle } : {}),
  };
}
/** The answer to `listHeldReplies`, newest first. An answer that is not a list at all is `INVALID_DATA`, never an empty one. */
export function heldReplies(value: unknown): HeldReply[] {
  if (!record(value) || !Array.isArray(value.replies)) throw new Error('INVALID_DATA');
  return value.replies.map(heldReply).filter((reply): reply is HeldReply => reply !== undefined);
}
/** What a decision came to; an answer that says nothing of the task is `INVALID_DATA`. */
export function heldReplyDecision(value: unknown): HeldReplyDecision {
  if (!record(value) || !word(value.taskId) || !word(value.state)) throw new Error('INVALID_DATA');
  const { declineReason, replyId, artifactId } = value;
  return {
    taskId: value.taskId, state: value.state as TaskState,
    ...(declineReason === 'approval_rejected' || declineReason === 'revoked' ? { declineReason } : {}),
    ...(word(replyId) || replyId === null ? { replyId } : {}), ...(word(artifactId) || artifactId === null ? { artifactId } : {}),
  };
}

/** The replies of one Bot that wait for this person, optionally in one Chat. `MAINTAINER_REQUIRED` when they do not maintain the Bot. */
export async function readHeldReplies(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, request: { sharedBotId: string; conversationId?: string }): Promise<HeldReply[]> {
  return heldReplies(await api.messages<unknown>(executionCommand(scope, 'listHeldReplies', {
    sharedBotId: request.sharedBotId, ...(request.conversationId ? { conversationId: request.conversationId } : {}),
  })));
}
/**
 * Let a held reply go: as the Bot made it, or with `text` as the maintainer changed it to. `commandId` is that of the first attempt when this one retries it, so
 * that an approval whose answer was lost is answered again and not decided twice. A reply that is no longer held is refused with `REPLY_APPROVED`, `REPLY_REJECTED`,
 * `REPLY_EXPIRED` or `REPLY_SUPERSEDED`, for whichever came first.
 */
export async function approveHeldReply(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, taskId: string, options: { text?: string; commandId?: string } = {}): Promise<HeldReplyDecision> {
  return heldReplyDecision(await api.messages<unknown>(executionCommand(scope, 'approveReply', { taskId, ...(options.text === undefined ? {} : { text: options.text }) }, options.commandId)));
}
/** Drop a held reply: nothing is posted. Refused like an approval when the reply is no longer held. */
export async function rejectHeldReply(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, taskId: string, options: { commandId?: string } = {}): Promise<HeldReplyDecision> {
  return heldReplyDecision(await api.messages<unknown>(executionCommand(scope, 'rejectReply', { taskId }, options.commandId)));
}

// ── What the maintainer may send ──

/** What sending the maintainer's own text comes to: the text to send (none when it says what the Bot said), or why it cannot be sent. */
export type HeldEdit = { ok: true; text: string | undefined } | { ok: false; problem: 'empty' | 'too_long' };
/**
 * The text of a held reply as the maintainer left it in the editor. The service takes it trimmed, at least a character and at most `HELD_REPLY_BYTES` of UTF-8; text
 * that is the Bot's own once trimmed is not sent at all, so that the reply is posted as the Bot made it and is not recorded as changed.
 */
export function heldEdit(draft: string, edited: string): HeldEdit {
  const text = edited.trim();
  if (!text) return { ok: false, problem: 'empty' };
  if (new TextEncoder().encode(text).length > HELD_REPLY_BYTES) return { ok: false, problem: 'too_long' };
  return { ok: true, text: text === draft.trim() ? undefined : text };
}

/**
 * How the service says a reply is no longer there to decide: it was decided, ran out of time or was dropped for a newer message, it is gone, or it is not this
 * person's to decide. A decision refused with one of these is not tried again.
 */
export const HELD_REPLY_ENDED: ReadonlySet<string> = new Set(['REPLY_APPROVED', 'REPLY_REJECTED', 'REPLY_NOT_HELD', 'REPLY_EXPIRED', 'REPLY_SUPERSEDED', 'TASK_REQUIRED', 'MAINTAINER_REQUIRED']);
/** What a client says of an approval the service took that posted nothing, because the reply could no longer be posted (`declined`, `revoked`). The service never sends it. */
export const REPLY_NOT_POSTED = 'REPLY_NOT_POSTED';

// ── Whose replies, and where they are shown ──

/**
 * The Bots of a Chat whose held replies this person decides on: the ones they maintain that are in it, answering when asked or on their own. A maintainer need not
 * take part in a Chat for the service to show them its replies, so the list does not depend on that.
 */
export function heldReplyBots(conversation: Pick<TeamConversation, 'participants'>, bots: readonly SharedBotCard[], uid: string, generation: number): SharedBotCard[] {
  return bots.filter(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`) && maintainsBot(bot, uid, generation));
}

/**
 * Where the held replies of a Chat are shown. A reply of a task the Chat lists sits with that task. A reply nobody asked for has no task in the list (the service keeps
 * such a task out of every member's), so it is a row of its own, oldest first as the stream is; an asked reply whose task is not listed yet waits for it.
 */
export function placeHeld(rows: readonly HeldRow[], listed: ReadonlySet<string>): { byTask: Map<string, HeldRow>; orphans: HeldRow[] } {
  const byTask = new Map<string, HeldRow>(), orphans: HeldRow[] = [];
  for (const row of rows) {
    if (listed.has(row.reply.taskId)) byTask.set(row.reply.taskId, row);
    else if (row.reply.unasked) orphans.unshift(row);
  }
  return { byTask, orphans };
}

// ── The list ──

/** A held reply and the Bot that made it: the service's answer says whose list it is, not whose each reply is, so the client keeps it from the list it asked for. */
export interface HeldBotReply extends HeldReply { sharedBotId: string }

/** One held reply as a screen shows it, and what is going on with it. */
export interface HeldRow {
  readonly reply: HeldBotReply;
  /** A decision on it is on its way. */
  readonly busy: 'approve' | 'reject' | undefined;
  /** The last decision failed in a way that can be tried again: how (a code). */
  readonly error: string | undefined;
  /** It is not there to be decided any more, and why (a code of the service; `REPLY_EXPIRED` for time that ran out while it was shown, `REPLY_NOT_POSTED` for an approval that posted nothing). It stays on screen, saying so, until it is closed. */
  readonly ended: string | undefined;
}
export interface HeldView {
  /** `loading` until the first answer; `failed`: the last read failed (`error` says how) and the rows are what was read before; `denied`: the service says this person maintains none of the Bots. */
  readonly status: 'loading' | 'ready' | 'failed' | 'denied';
  /** Newest first. */
  readonly rows: readonly HeldRow[];
  readonly error: string | undefined;
  /** The time the rows are shown at: it moves on every `HELD_TICK_MS` and at each reply's end, so that the time left is never staler than that. */
  readonly now: number;
}
/** How often the time left is brought up to date. */
export const HELD_TICK_MS = 30_000;
/** How long after a reply's end it is read again: the service has dropped it by then, whatever its clock says. */
const EXPIRY_SLACK_MS = 500;

export type HeldRead = () => Promise<HeldBotReply[]>;
export interface HeldDecide {
  approve(taskId: string, options: { text?: string; commandId: string }): Promise<HeldReplyDecision>;
  reject(taskId: string, options: { commandId: string }): Promise<HeldReplyDecision>;
}

const errorCode = (error: unknown): string => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const newestFirst = (a: Pick<HeldReply, 'heldAt' | 'taskId'>, b: Pick<HeldReply, 'heldAt' | 'taskId'>): number => b.heldAt - a.heldAt || a.taskId.localeCompare(b.taskId);

/**
 * The replies of the given Bots that wait for this person, in one Chat or in every one, newest first. A Bot the service says is not theirs after all adds nothing; if that
 * is every Bot the answer is `MAINTAINER_REQUIRED`, and any other failure is the failure of the read.
 */
export async function readHeldRepliesOf(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, bots: readonly string[], conversationId?: string): Promise<HeldBotReply[]> {
  const answers = await Promise.allSettled(bots.map(async sharedBotId => (await readHeldReplies(api, scope, { sharedBotId, ...(conversationId ? { conversationId } : {}) })).map((reply): HeldBotReply => ({ ...reply, sharedBotId }))));
  const failure = answers.find((answer): answer is PromiseRejectedResult => answer.status === 'rejected' && errorCode(answer.reason) !== 'MAINTAINER_REQUIRED');
  if (failure) throw failure.reason;
  const read = answers.flatMap(answer => answer.status === 'fulfilled' ? [answer.value] : []);
  if (!read.length) throw new Error('MAINTAINER_REQUIRED');
  return read.flat().sort(newestFirst);
}

interface Entry { reply: HeldBotReply; busy: HeldRow['busy']; error: string | undefined; ended: string | undefined }

/**
 * The held replies a screen shows and the decisions on them. It reads the list when asked to (`load`, one read at a time; a request that comes while one is going on
 * is answered by one more read after it) and keeps the time: the time left moves on every `HELD_TICK_MS`, and the list is read again when the first reply runs out.
 *
 * A reply that leaves the list stays on screen when someone is told something about it: while a decision on it is on its way, after the service refused one because
 * it was decided or ran out of time, and after its time ran out while it was shown. It is told about once (`dismiss`), and then it is gone. One that leaves for no
 * such reason (it was decided on another screen, or a newer message dropped it) simply goes.
 */
export class HeldRepliesBoard {
  private readonly entries = new Map<string, Entry>();
  /** The command id of the latest attempt at each reply, for the decision it was: a retry of the same decision is the same command, a different decision is a new one. */
  private readonly attempts = new Map<string, { decision: string; commandId: string }>();
  private status: HeldView['status'] = 'loading';
  private error: string | undefined;
  private now: number;
  private view: HeldView;
  private pending: Promise<void> | undefined;
  private again = false;
  private closed = false;
  private ticker: ReturnType<typeof setInterval> | undefined;
  private expiry: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly read: HeldRead, private readonly decide: HeldDecide, private readonly changed: (view: HeldView) => void,
    private readonly clock: () => number = Date.now, private readonly newId: () => string = () => crypto.randomUUID(),
  ) {
    this.now = clock();
    this.view = this.build();
  }

  get current(): HeldView { return this.view; }

  /** Start keeping the time. */
  start(): void {
    if (this.ticker !== undefined || this.closed) return;
    this.ticker = setInterval(() => this.tick(), HELD_TICK_MS);
    this.schedule();
  }
  /** Nothing that is still on its way changes the list after this. */
  close(): void {
    this.closed = true;
    if (this.ticker !== undefined) clearInterval(this.ticker);
    if (this.expiry !== undefined) clearTimeout(this.expiry);
    this.ticker = this.expiry = undefined;
  }

  /** Read the list (again). */
  load(): Promise<void> {
    if (this.closed) return Promise.resolve();
    if (this.pending) { this.again = true; return this.pending; }
    const work = (async () => {
      do { this.again = false; await this.pass(); } while (this.again && !this.closed);
    })().finally(() => { this.pending = undefined; });
    this.pending = work;
    return work;
  }

  /** Let the reply go, with the maintainer's text when they changed it. Whether the service took the decision. */
  approve(taskId: string, text?: string): Promise<boolean> {
    return this.decision(taskId, 'approve', `approve:${text ?? ''}`, async commandId => {
      const decision = await this.decide.approve(taskId, { ...(text === undefined ? {} : { text }), commandId });
      // Accepted, and posted nothing: the reply could not be posted any more. It says so.
      return decision.state === 'declined' && decision.declineReason === 'revoked' ? REPLY_NOT_POSTED : undefined;
    });
  }
  /** Drop the reply. Whether the service took the decision. */
  reject(taskId: string): Promise<boolean> {
    return this.decision(taskId, 'reject', 'reject', async commandId => { await this.decide.reject(taskId, { commandId }); return undefined; });
  }
  /** The person has read what became of a reply that was shown: it goes. */
  dismiss(taskId: string): void {
    if (this.entries.get(taskId)?.ended === undefined) return;
    this.entries.delete(taskId);
    this.publish();
  }

  private async decision(taskId: string, kind: 'approve' | 'reject', decision: string, send: (commandId: string) => Promise<string | undefined>): Promise<boolean> {
    const entry = this.entries.get(taskId);
    if (!entry || entry.busy || entry.ended !== undefined || this.closed) return false;
    entry.busy = kind; entry.error = undefined; this.publish();
    const known = this.attempts.get(taskId);
    const attempt = known?.decision === decision ? known : { decision, commandId: this.newId() };
    this.attempts.set(taskId, attempt);
    try {
      const ended = await send(attempt.commandId);
      this.attempts.delete(taskId);
      if (this.closed) return true;
      if (ended === undefined) this.entries.delete(taskId); else { entry.busy = undefined; entry.ended = ended; }
      this.publish();
      void this.load();
      return true;
    } catch (error) {
      if (this.closed) return false;
      const code = errorCode(error);
      entry.busy = undefined;
      // The reply is not there to be decided: say so, and read the list, which no longer has it. Anything else may be tried again, as the same command.
      if (HELD_REPLY_ENDED.has(code)) { entry.ended = code; this.attempts.delete(taskId); } else entry.error = code;
      this.publish();
      if (HELD_REPLY_ENDED.has(code)) void this.load();
      return false;
    }
  }

  private async pass(): Promise<void> {
    try {
      const replies = await this.read();
      if (this.closed) return;
      this.merge(replies);
      this.status = 'ready'; this.error = undefined;
    } catch (error) {
      if (this.closed) return;
      const code = errorCode(error);
      if (code === 'MAINTAINER_REQUIRED') { this.entries.clear(); this.status = 'denied'; this.error = code; } else { this.status = 'failed'; this.error = code; }
    }
    this.now = this.clock();
    this.publish();
  }

  private merge(replies: readonly HeldBotReply[]): void {
    const now = this.clock();
    const listed = new Set(replies.map(reply => reply.taskId));
    for (const [taskId, entry] of this.entries) {
      if (listed.has(taskId) || entry.busy || entry.ended !== undefined) continue;
      if (entry.reply.expiresAt <= now) entry.ended = 'REPLY_EXPIRED'; else this.entries.delete(taskId);
    }
    for (const reply of replies) {
      const entry = this.entries.get(reply.taskId);
      if (entry) entry.reply = reply; else this.entries.set(reply.taskId, { reply, busy: undefined, error: undefined, ended: undefined });
    }
  }

  private tick(): void {
    if (this.closed) return;
    this.now = this.clock();
    this.publish();
  }
  /** The list is read again, and the time brought up to date, when the first of the replies that wait runs out. */
  private schedule(): void {
    if (this.expiry !== undefined) clearTimeout(this.expiry);
    this.expiry = undefined;
    if (this.closed || this.ticker === undefined) return;
    const next = Math.min(...[...this.entries.values()].filter(entry => entry.ended === undefined && entry.reply.expiresAt > this.clock()).map(entry => entry.reply.expiresAt));
    if (!Number.isFinite(next)) return;
    this.expiry = setTimeout(() => { this.expiry = undefined; this.tick(); void this.load(); }, Math.max(0, next - this.clock()) + EXPIRY_SLACK_MS);
  }
  private build(): HeldView {
    const rows = [...this.entries.values()].sort((a, b) => newestFirst(a.reply, b.reply)).map((entry): HeldRow => ({ reply: entry.reply, busy: entry.busy, error: entry.error, ended: entry.ended }));
    return { status: this.status, rows, error: this.error, now: this.now };
  }
  private publish(): void {
    this.view = this.build();
    this.schedule();
    this.changed(this.view);
  }
}
