import { authorIdentity, type TeamMessage } from './messages.js';

/** Messages from one author within this long of each other read as one run. */
export const GROUP_WINDOW_MS = 5 * 60_000;

export interface StreamOptions {
  /** The caller's read watermark for this conversation. */
  lastReadSeq: number;
  /** The caller, whose own messages never anchor the "new messages" divider. */
  uid: string;
  /** BCP 47 tag used for day labels. Chinese tags (`zh…`) get Chinese words. */
  locale: string;
  now: Date | number;
  /** IANA zone for day boundaries and labels. Defaults to the runtime's zone. */
  timeZone?: string;
}
export type StreamRow =
  | { type: 'date'; key: string; label: string }
  | { type: 'unread'; key: string }
  | { type: 'message'; key: string; message: TeamMessage; grouped: boolean };

const dayFormatters = new Map<string, Intl.DateTimeFormat>();
/** `YYYY-MM-DD` of an instant in a time zone, comparable as a string. */
function dayKey(ms: number, timeZone?: string): string {
  const id = timeZone ?? '';
  let format = dayFormatters.get(id);
  if (!format) { format = new Intl.DateTimeFormat('en-CA', { ...(timeZone ? { timeZone } : {}), year: 'numeric', month: '2-digit', day: '2-digit' }); dayFormatters.set(id, format); }
  const parts = format.formatToParts(ms);
  const get = (type: string) => parts.find(part => part.type === type)?.value ?? '';
  return `${get('year')}-${get('month')}-${get('day')}`;
}
function previousDay(key: string): string {
  const [year, month, day] = key.split('-').map(Number) as [number, number, number];
  return new Date(Date.UTC(year, month - 1, day - 1)).toISOString().slice(0, 10);
}
const isChinese = (locale: string) => locale.toLowerCase().startsWith('zh');
function instant(value: string | null | undefined): number | undefined {
  if (!value) return undefined;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? undefined : ms;
}
const asMs = (now: Date | number) => typeof now === 'number' ? now : now.getTime();

/** "Today", "Yesterday", or a long weekday + date (with the year when it is not this one). */
export function dayLabel(ms: number, now: Date | number, locale: string, timeZone?: string): string {
  const nowMs = asMs(now), day = dayKey(ms, timeZone), today = dayKey(nowMs, timeZone);
  if (day === today) return isChinese(locale) ? '今天' : 'Today';
  if (day === previousDay(today)) return isChinese(locale) ? '昨天' : 'Yesterday';
  const sameYear = day.slice(0, 4) === today.slice(0, 4);
  return new Intl.DateTimeFormat(locale, { ...(timeZone ? { timeZone } : {}), weekday: 'long', month: 'long', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) }).format(ms);
}

/** Hour and minute of a message in the viewer's locale. Empty for a missing or invalid time. */
export function formatClock(value: string | null | undefined, locale: string, timeZone?: string): string {
  const ms = instant(value);
  return ms === undefined ? '' : new Intl.DateTimeFormat(locale, { ...(timeZone ? { timeZone } : {}), hour: 'numeric', minute: '2-digit' }).format(ms);
}

/** Compact conversation-list time: clock today, "Yesterday", weekday this week, then the date. */
export function formatListTime(value: string | null | undefined, now: Date | number, locale: string, timeZone?: string): string {
  const ms = instant(value);
  if (ms === undefined) return '';
  const nowMs = asMs(now), day = dayKey(ms, timeZone), today = dayKey(nowMs, timeZone);
  const zone = timeZone ? { timeZone } : {};
  if (day === today) return formatClock(value, locale, timeZone);
  if (day === previousDay(today)) return isChinese(locale) ? '昨天' : 'Yesterday';
  // Two to six days back a weekday name is unambiguous; seven days back would repeat today's.
  let sixDaysAgo = today;
  for (let i = 0; i < 6; i++) sixDaysAgo = previousDay(sixDaysAgo);
  if (day >= sixDaysAgo && day < today) return new Intl.DateTimeFormat(locale, { ...zone, weekday: 'short' }).format(ms);
  const sameYear = day.slice(0, 4) === today.slice(0, 4);
  return new Intl.DateTimeFormat(locale, { ...zone, month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) }).format(ms);
}

const sameAuthor = (a: TeamMessage['author'], b: TeamMessage['author']) => authorIdentity(a) === authorIdentity(b);

/** The first unread message written by someone else: where the "new messages" divider goes. */
export function firstUnreadMessage(messages: readonly TeamMessage[], lastReadSeq: number, uid: string): TeamMessage | undefined {
  return messages.find(message => message.seq > lastReadSeq && !(message.author.kind === 'user' && message.author.uid === uid));
}

/**
 * Rows of the message stream: day dividers, one "new messages" divider, and messages flagged
 * `grouped` when they continue the previous message's run (same author, at most five minutes
 * apart, same day, no divider between, previous not recalled). Messages without a time never
 * start a day and never group, so the author header is shown.
 */
export function buildStreamRows(messages: readonly TeamMessage[], options: StreamOptions): StreamRow[] {
  const anchor = firstUnreadMessage(messages, options.lastReadSeq, options.uid);
  const rows: StreamRow[] = [];
  let previous: TeamMessage | undefined, previousTime: number | undefined, lastDay: string | undefined;
  for (const message of messages) {
    const time = instant(message.createdAt), day = time === undefined ? undefined : dayKey(time, options.timeZone);
    const startsDay = day !== undefined && day !== lastDay;
    if (startsDay) rows.push({ type: 'date', key: `date:${day}`, label: dayLabel(time!, options.now, options.locale, options.timeZone) });
    const unread = message === anchor;
    if (unread) rows.push({ type: 'unread', key: 'unread' });
    const grouped = !!previous && !startsDay && !unread && !previous.recalled && sameAuthor(previous.author, message.author)
      && time !== undefined && previousTime !== undefined && Math.abs(time - previousTime) <= GROUP_WINDOW_MS;
    rows.push({ type: 'message', key: `message:${message.id}`, message, grouped });
    previous = message; previousTime = time;
    if (day !== undefined) lastDay = day;
  }
  return rows;
}

/** Messages rendered at once: the newest `WINDOW_SIZE`, more on request. */
export const WINDOW_SIZE = 100;
/** The most the window grows by itself (to include unread messages). A reader can always ask for more. */
export const WINDOW_MAX = 400;
/** Context shown above the first unread message when the window is widened to include it. */
export const UNREAD_CONTEXT = 3;

/**
 * Where the rendered window of a conversation starts. The window follows the newest message, so
 * arrivals never push older ones out of view or move the reader. By itself it is the newest `size`
 * messages, widened back to the first unread message (with a little context) but never past `max`;
 * `startId`, the oldest message the reader asked to see, extends it back without that limit.
 */
export function visibleStart(messages: readonly TeamMessage[], options: { startId?: string; lastReadSeq: number; uid: string; size?: number; max?: number }): number {
  const size = options.size ?? WINDOW_SIZE, max = options.max ?? WINDOW_MAX;
  let start = Math.max(0, messages.length - size);
  const unread = firstUnreadMessage(messages, options.lastReadSeq, options.uid);
  if (unread) start = Math.min(start, Math.max(0, messages.indexOf(unread) - UNREAD_CONTEXT));
  start = Math.max(start, messages.length - max, 0);
  const asked = options.startId ? messages.findIndex(message => message.id === options.startId) : -1;
  return asked >= 0 ? Math.min(start, asked) : start;
}
