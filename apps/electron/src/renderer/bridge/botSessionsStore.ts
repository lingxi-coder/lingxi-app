/**
 * The chats of the Bots whose groups the sidebar shows open, as the window
 * holds them. Listing a Bot's chats makes the host read all of its transcripts,
 * so a list is read only for a Bot on show, never twice at once, and read again
 * only when something has happened that adds to a chat or makes one.
 */

import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BotPromptSentUi, BotUi, BotsApi } from '../../shared/bots';
import { ipcErrorMessage, sortBots } from './botForm';
import type { BotRuntimeActivity } from './botSession';
import { ObservableState } from './observableState';
import { sidebarSessionTimestamp } from './sessionPresentation';

export interface BotSessionsEntry {
  /** Newest first. Absent until a read has worked. */
  sessions: readonly SessionRowDto[] | undefined;
  /** A read is under way. While there are `sessions`, they are those of the read before. */
  loading: boolean;
  /** Why the last read failed. */
  error: string | undefined;
}

export interface BotSessionsState {
  entries: Readonly<Record<string, BotSessionsEntry>>;
}

type EventSource = Pick<EventTarget, 'addEventListener' | 'removeEventListener'>;

export class BotSessionsController extends ObservableState<BotSessionsState> {
  /** The Bots whose chats are on show. */
  private shown: ReadonlySet<string> = new Set();
  /** The read under way for a Bot. */
  private readonly reads = new Map<string, Promise<void>>();
  /** Counts the times a Bot was dropped, so that a read still under way for it can tell. */
  private readonly generations = new Map<string, number>();
  /** When the last read of a Bot began, in epoch milliseconds. */
  private readonly readStarted = new Map<string, number>();
  /** Whether each running chat of a Bot had a turn going when it was last looked at. */
  private turns = new Map<string, { botId: string; active: boolean }>();

  constructor(
    private readonly api: Pick<BotsApi, 'listSessions' | 'onPromptSent' | 'onSessionShown'>,
    private readonly now: () => number = Date.now,
  ) {
    super({ entries: {} });
  }

  /**
   * Listen for what makes a list stale that the window is not told otherwise:
   * the host sending a prompt or showing a chat on its own, and the owner coming
   * back to the window. Returns the way to stop listening.
   */
  watch(target: EventSource | undefined = typeof globalThis.window === 'undefined' ? undefined : globalThis.window): () => void {
    const stops = [
      this.api.onPromptSent?.((prompt) => this.promptSent(prompt)),
      this.api.onSessionShown?.((session) => { void this.load(session.botId); }),
    ];
    const focused = () => this.loadShown();
    target?.addEventListener('focus', focused);
    return () => {
      for (const stop of stops) stop?.();
      target?.removeEventListener('focus', focused);
    };
  }

  /** Say which Bots' chats are on show. A Bot newly on show is read, with what was read of it before staying up meanwhile. */
  show(botIds: readonly string[]): void {
    const added = botIds.filter((botId) => !this.shown.has(botId));
    this.shown = new Set(botIds);
    for (const botId of new Set(added)) void this.load(botId);
  }

  /** Read a Bot's chats now. Nothing happens for a Bot that is not on show, and a read already under way is the one that answers. */
  load(botId: string): Promise<void> {
    if (!this.shown.has(botId)) return Promise.resolve();
    const running = this.reads.get(botId);
    if (running) return running;
    const read: Promise<void> = this.read(botId).finally(() => { if (this.reads.get(botId) === read) this.reads.delete(botId); });
    this.reads.set(botId, read);
    return read;
  }

  /** Read again the Bots whose chat just finished a turn: it has added to the chat, or made one. */
  noteRuntimes(runtimes: ReadonlyArray<Pick<BotRuntimeActivity, 'sessionId' | 'botId' | 'turnActive'>>): void {
    const now = new Map(runtimes.map((runtime) => [runtime.sessionId, { botId: runtime.botId, active: runtime.turnActive }] as const));
    const finished = new Set<string>();
    // A chat that is gone while its turn was going has ended the turn as much as one that says so.
    for (const [sessionId, before] of this.turns) if (before.active && !now.get(sessionId)?.active) finished.add(before.botId);
    this.turns = now;
    for (const botId of finished) void this.load(botId);
  }

  /**
   * Take in the Bots as the list has them now: a Bot that is gone loses its
   * chats, and one used since its chats were read is read again. Use before the
   * read began is in it already, so that is no reason to read again.
   */
  noteBots(bots: ReadonlyArray<Pick<BotUi, 'botId' | 'lastActivityAt'>>): void {
    const listed = new Set(bots.map((bot) => bot.botId));
    const known = new Set([...Object.keys(this.getSnapshot().entries), ...this.shown]);
    for (const botId of known) if (!listed.has(botId)) this.drop(botId);
    for (const bot of bots) {
      const started = this.readStarted.get(bot.botId);
      if (bot.lastActivityAt !== undefined && started !== undefined && bot.lastActivityAt > started) void this.load(bot.botId);
    }
  }

  private async read(botId: string): Promise<void> {
    const generation = this.generations.get(botId) ?? 0;
    this.readStarted.set(botId, this.now());
    this.put(botId, { loading: true, error: undefined });
    let change: Partial<BotSessionsEntry>;
    try {
      const { sessions } = await this.api.listSessions(botId);
      change = { sessions: [...sessions].sort((left, right) => sidebarSessionTimestamp(right) - sidebarSessionTimestamp(left)), loading: false, error: undefined };
    } catch (cause) {
      change = { loading: false, error: ipcErrorMessage(cause) };
    }
    // The Bot was deleted while its chats were being read: they are not put back.
    if ((this.generations.get(botId) ?? 0) === generation) this.put(botId, change);
  }

  private put(botId: string, change: Partial<BotSessionsEntry>): void {
    const { entries } = this.getSnapshot();
    const before: BotSessionsEntry = entries[botId] ?? { sessions: undefined, loading: false, error: undefined };
    this.update({ entries: { ...entries, [botId]: { ...before, ...change } } });
  }

  private drop(botId: string): void {
    this.shown = new Set([...this.shown].filter((shown) => shown !== botId));
    this.generations.set(botId, (this.generations.get(botId) ?? 0) + 1);
    this.reads.delete(botId);
    this.readStarted.delete(botId);
    for (const [sessionId, turn] of this.turns) if (turn.botId === botId) this.turns.delete(sessionId);
    this.update({ entries: Object.fromEntries(Object.entries(this.getSnapshot().entries).filter(([id]) => id !== botId)) });
  }

  private loadShown(): void {
    for (const botId of this.shown) void this.load(botId);
  }

  /** The host sent a prompt to a chat. It names the chat and not the Bot: the Bot is the one that lists it, or any on show when the chat is new. */
  private promptSent(prompt: BotPromptSentUi): void {
    const owner = Object.entries(this.getSnapshot().entries).find(([, entry]) => entry.sessions?.some((row) => row.uuid === prompt.sessionId))?.[0];
    if (owner === undefined) this.loadShown();
    else void this.load(owner);
  }
}

/** The chat of a Bot that was used last, in epoch milliseconds, if its chats have been read. */
function newestChat(entry: BotSessionsEntry | undefined): number | undefined {
  return entry?.sessions?.reduce<number | undefined>((newest, row) => Math.max(newest ?? 0, sidebarSessionTimestamp(row)), undefined);
}

/**
 * The Bots most recently used first. A Bot's chats, once read, can know of use
 * more recent than the list does, which is only read now and then.
 */
export function recentBots(bots: readonly BotUi[], entries: BotSessionsState['entries']): BotUi[] {
  return sortBots(bots.map((bot) => {
    const newest = newestChat(entries[bot.botId]);
    return newest !== undefined && newest > (bot.lastActivityAt ?? 0) ? { ...bot, lastActivityAt: newest } : bot;
  }));
}
