import { currentLocale, tr, type DesktopLocale } from '../i18n/locale';
const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

const relativeFormatters = new Map<DesktopLocale, Intl.RelativeTimeFormat>();
const clockFormatters = new Map<DesktopLocale, Intl.DateTimeFormat>();
function relative(value: number, unit: Intl.RelativeTimeFormatUnit): string {
  const locale = currentLocale();
  let formatter = relativeFormatters.get(locale);
  if (!formatter) { formatter = new Intl.RelativeTimeFormat(locale, { numeric: 'always' }); relativeFormatters.set(locale, formatter); }
  return formatter.format(-value, unit);
}

/** Format catalog activity without depending on the wall clock in tests. */
export function formatRelativeSessionTime(modifiedRfc3339: string, now = Date.now()): string {
  const modified = Date.parse(modifiedRfc3339);
  if (!Number.isFinite(modified) || !Number.isFinite(now) || modified > now) return tr('desktop_session_just_now');
  const elapsed = Math.max(0, now - modified);
  if (elapsed < MINUTE) return tr('desktop_session_just_now');
  if (elapsed < HOUR) return relative(Math.floor(elapsed / MINUTE), 'minute');
  if (elapsed < DAY) return relative(Math.floor(elapsed / HOUR), 'hour');
  if (elapsed < WEEK) return relative(Math.floor(elapsed / DAY), 'day');
  if (elapsed < MONTH) return relative(Math.floor(elapsed / WEEK), 'week');
  if (elapsed < YEAR) return relative(Math.floor(elapsed / MONTH), 'month');
  return relative(Math.floor(elapsed / YEAR), 'year');
}

/** Wall-clock time in the selected interface language; invalid timestamps have no clock. */
export function formatClockTime(epochMs: number | undefined): string {
  if (epochMs === undefined || !Number.isFinite(epochMs)) return '';
  const at = new Date(epochMs);
  if (!Number.isFinite(at.getTime())) return '';
  const locale = currentLocale();
  let formatter = clockFormatters.get(locale);
  if (!formatter) { formatter = new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit' }); clockFormatters.set(locale, formatter); }
  return formatter.format(at);
}

export function formatSessionMetadata(modifiedRfc3339: string, messageCount: number, now = Date.now()): string {
  const count = Number.isSafeInteger(messageCount) && messageCount >= 0 ? messageCount : 0;
  return `${formatRelativeSessionTime(modifiedRfc3339, now)} · ${tr(count === 1 ? 'desktop_session_message_one' : 'desktop_session_message_count', count)}`;
}
