import { executionCommand, TRIAGE_ACTIONS, TRIAGE_REASONS, TRIAGE_SOURCES, type ExecutionScope, type SharedBotCard, type TaskState, type TriageAction, type TriageLogItem, type TriageLogPage, type TriageReason } from './execution.js';
import { TRIAGE_LOG_LIMIT } from './execution-validation.js';
import type { TeamsApi } from './index.js';
import type { TeamConversation } from './messages.js';
import { tk, type TeamsKey, type TeamsText } from './i18n.js';

/**
 * The auto-reply log of a Bot, for the person who maintains it. A Bot's host judges every message nobody asked the Bot about and tells the
 * service what it decided and why, in codes; the maintainer reads them here, newest first, a page at a time, for every Chat of the Team or for one.
 * Only the decision and the reason are kept, never what a model said. Desktop's Bots tab reads the same log through its own window bridge and
 * shares the words and the two small functions below with this one.
 */

/** How many rows a page of the log has: the service gives at most `TRIAGE_LOG_LIMIT`. */
export const TRIAGE_LOG_PAGE = 20;

/** Where a judged message ended up, in the words of the log: it was answered, the Bot stayed silent (it declined, or the task was canceled before it spoke), it failed, or it is still going on. */
export type TriageOutcome = 'replied' | 'silent' | 'failed' | 'waiting';

/** Every state a task can be in, and where it leaves the judged message. A state this table does not have is a row this version cannot read. */
const OUTCOMES: Record<TaskState, TriageOutcome> = {
  completed: 'replied',
  declined: 'silent', cancelled: 'silent',
  queued: 'waiting', running: 'waiting', waiting: 'waiting',
  failed: 'failed', expired: 'failed', interrupted: 'failed', authorization_required: 'failed', revoked: 'failed', needs_reconciliation: 'failed',
};
export const triageOutcome = (state: TaskState): TriageOutcome => OUTCOMES[state];

/** A probability the Decision service gave, or a threshold of the Bot's on the same scale, as the whole percent it is shown as. */
export const triagePercent = (probability: number): number => Math.round(probability * 100);

const ACTION_TEXT: Record<TriageAction, TeamsKey> = {
  decline: tk('teams_triage_action_decline'), run: tk('teams_triage_action_run'), run_may_decline: tk('teams_triage_action_maybe'),
};
const REASON_TEXT: Record<TriageReason, TeamsKey> = {
  quiet_hours: tk('teams_triage_reason_quiet_hours'), cooldown: tk('teams_triage_reason_cooldown'), keyword: tk('teams_triage_reason_keyword'),
  addressed: tk('teams_triage_reason_addressed'), below_threshold: tk('teams_triage_reason_below_threshold'), between_thresholds: tk('teams_triage_reason_between_thresholds'),
  above_threshold: tk('teams_triage_reason_above_threshold'), decision_unavailable: tk('teams_triage_reason_decision_unavailable'), message_unavailable: tk('teams_triage_reason_message_unavailable'),
};
const OUTCOME_TEXT: Record<TriageOutcome, TeamsKey> = {
  replied: tk('teams_triage_state_replied'), silent: tk('teams_triage_state_silent'), failed: tk('teams_triage_state_failed'), waiting: tk('teams_triage_state_waiting'),
};
export const triageActionText = (t: TeamsText, action: TriageAction): string => t(ACTION_TEXT[action]);
/** Why, in words; nothing for a reason this version does not know. */
export const triageReasonText = (t: TeamsText, reason: TriageReason | undefined): string => reason !== undefined && Object.prototype.hasOwnProperty.call(REASON_TEXT, reason) ? t(REASON_TEXT[reason]) : '';
export const triageOutcomeText = (t: TeamsText, outcome: TriageOutcome): string => t(OUTCOME_TEXT[outcome]);
/** "62% likely": what the Decision service gave. */
export const triageProbabilityText = (t: TeamsText, probability: number): string => t('teams_triage_probability_fmt', triagePercent(probability));

// ── What the service says ──

const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const word = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
/** A time in epoch milliseconds that a date can hold. */
const moment = (value: unknown): value is number => finite(value) && Math.abs(value) <= 8.64e15;
const oneOf = <T extends string>(values: readonly T[], value: unknown): value is T => typeof value === 'string' && (values as readonly string[]).includes(value);

/**
 * One row of the log as the service sent it, or nothing when this version cannot read it: a row is never half shown, and one row that is not
 * understood does not hide the page it came in. A reason or a probability it does not know is left out of the row; the decision is kept.
 */
export function triageItem(value: unknown): TriageLogItem | undefined {
  if (!record(value) || !record(value.triage)) return undefined;
  const { triage } = value;
  if (!word(value.taskId) || !word(value.conversationId) || !word(value.messageId) || !moment(value.createdAt)) return undefined;
  if (!oneOf(TRIAGE_ACTIONS, triage.action) || !oneOf(TRIAGE_SOURCES, triage.source) || !moment(triage.at)) return undefined;
  if (typeof value.state !== 'string' || !Object.prototype.hasOwnProperty.call(OUTCOMES, value.state)) return undefined;
  return {
    taskId: value.taskId, conversationId: value.conversationId, messageId: value.messageId, createdAt: value.createdAt, state: value.state as TaskState,
    triage: {
      action: triage.action, source: triage.source, at: triage.at,
      ...(oneOf(TRIAGE_REASONS, triage.reason) ? { reason: triage.reason } : {}),
      ...(finite(triage.probability) && triage.probability >= 0 && triage.probability <= 1 ? { probability: triage.probability } : {}),
    },
    ...(word(value.conversationTitle) ? { conversationTitle: value.conversationTitle } : {}),
  };
}
/** A page as the service sent it. An answer that is not a page at all is `INVALID_DATA`, never an empty log. */
export function triagePage(value: unknown): TriageLogPage {
  if (!record(value) || !Array.isArray(value.items)) throw new Error('INVALID_DATA');
  const items = value.items.map(triageItem).filter((item): item is TriageLogItem => item !== undefined);
  return { items, ...(word(value.nextCursor) ? { nextCursor: value.nextCursor } : {}) };
}

/** One read of the log: the rows after `cursor` (from the newest when there is none), at most `limit`. */
export type TriageLogRead = (cursor: string | undefined, limit: number) => Promise<TriageLogPage>;
/** The log of one Bot of the Team, or of its part in one Chat, as the member commands read it. */
export function triageLogRead(api: Pick<TeamsApi, 'messages'>, scope: ExecutionScope, request: { sharedBotId: string; conversationId?: string }): TriageLogRead {
  return async (cursor, limit) => triagePage(await api.messages<unknown>(executionCommand(scope, 'listTriage', {
    sharedBotId: request.sharedBotId, ...(request.conversationId ? { conversationId: request.conversationId } : {}), ...(cursor ? { cursor } : {}), limit,
  })));
}

// ── The list on screen ──

export interface TriageLogView {
  /**
   * `loading`: the first page is being read; `more`: the next one is; `failed`: the last read failed (`error` says how) and the rows are what was read before;
   * `denied`: the service says this person does not maintain the Bot, so there is no log to show and nothing to offer.
   */
  readonly status: 'loading' | 'ready' | 'more' | 'failed' | 'denied';
  /** Newest first, each task once. */
  readonly items: readonly TriageLogItem[];
  /** Pass it back to read the page after the last one; absent at the end of the log. */
  readonly nextCursor: string | undefined;
  readonly error: string | undefined;
}
/** Where a list starts: nothing read yet. */
export const TRIAGE_LOADING: TriageLogView = { status: 'loading', items: [], nextCursor: undefined, error: undefined };

/**
 * The paged list behind the dialog. A read that was overtaken by a newer one, or that answers after the list was closed, does not change what
 * the newer one showed.
 */
export class TriageLogReader {
  private run = 0;
  private current: TriageLogView = TRIAGE_LOADING;
  constructor(private readonly read: TriageLogRead, private readonly changed: (view: TriageLogView) => void, private readonly limit = TRIAGE_LOG_PAGE) {}

  get view(): TriageLogView { return this.current; }

  /** Read the first page. Also what "refresh" and "try again" do: the list starts over. */
  async load(): Promise<void> {
    const run = ++this.run;
    this.set(TRIAGE_LOADING);
    await this.page(run, undefined);
  }
  /** Read the page after the last one. Nothing happens while a read is going on or at the end of the log. */
  async more(): Promise<void> {
    const { status, nextCursor } = this.current;
    if (nextCursor === undefined || status === 'loading' || status === 'more' || status === 'denied') return;
    const run = ++this.run;
    this.set({ ...this.current, status: 'more', error: undefined });
    await this.page(run, nextCursor);
  }
  /** Nothing that is still being read changes the list after this. */
  close(): void { this.run++; }

  private async page(run: number, cursor: string | undefined): Promise<void> {
    try {
      const page = await this.read(cursor, this.limit);
      if (run !== this.run) return;
      // Each task once: a page that repeats a row of the one before (the log moved on between the two reads) adds nothing.
      const items = cursor === undefined ? [] : [...this.current.items], seen = new Set(items.map(item => item.taskId));
      for (const item of page.items) if (!seen.has(item.taskId)) { seen.add(item.taskId); items.push(item); }
      this.set({ status: 'ready', items, nextCursor: page.nextCursor, error: undefined });
    } catch (error) {
      if (run !== this.run) return;
      const code = error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
      this.set(code === 'MAINTAINER_REQUIRED' ? { status: 'denied', items: [], nextCursor: undefined, error: code } : { ...this.current, status: 'failed', error: code });
    }
  }
  private set(view: TriageLogView): void { this.current = view; this.changed(view); }
}

// ── Whose log, and why a Bot stayed silent ──

/** Whether this person maintains the Bot, by the membership they hold now. */
export const maintainsBot = (bot: Pick<SharedBotCard, 'maintainer'>, uid: string, generation: number): boolean => bot.maintainer.uid === uid && bot.maintainer.generation === generation;

/**
 * The Bots of a Chat whose log this person reads there: the ones they maintain that answer on their own in it, or are published to. A maintainer
 * need not take part in a Chat to be shown it, so the list does not depend on that.
 */
export function triageBots(conversation: Pick<TeamConversation, 'participants' | 'autoReply'>, bots: readonly SharedBotCard[], uid: string, generation: number): SharedBotCard[] {
  const on = conversation.autoReply?.targets ?? [];
  return bots.filter(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`) && maintainsBot(bot, uid, generation) && (!!bot.autoReply || on.includes(bot.sharedBotId)));
}

/** How many pages of 50 rows are read to find the verdict of a task before it is given up. */
export const TRIAGE_FIND_PAGES = 4;
/**
 * The rows of the log for these tasks, read from the newest verdict down until every task has its row, the log ends, or `pages` pages were read.
 * A task that has no row is not in the answer. A read that fails ends the search with what it has found: the reason of a silence is a courtesy, never an error.
 */
export async function findTriage(read: TriageLogRead, taskIds: readonly string[], pages = TRIAGE_FIND_PAGES): Promise<Map<string, TriageLogItem>> {
  const found = new Map<string, TriageLogItem>(), missing = new Set(taskIds);
  let cursor: string | undefined;
  try {
    for (let index = 0; index < pages && missing.size; index++) {
      const page = await read(cursor, TRIAGE_LOG_LIMIT);
      for (const item of page.items) if (missing.delete(item.taskId)) found.set(item.taskId, item);
      if (page.nextCursor === undefined || page.nextCursor === cursor) break;
      cursor = page.nextCursor;
    }
  } catch { /* what was found is what there is */ }
  return found;
}
