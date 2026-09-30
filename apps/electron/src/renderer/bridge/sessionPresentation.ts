const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

function plural(value: number, unit: string): string {
  return `${value} ${unit}${value === 1 ? '' : 's'} ago`;
}

/** Format catalog activity without depending on the wall clock in tests. */
export function formatRelativeSessionTime(modifiedRfc3339: string, now = Date.now()): string {
  const modified = Date.parse(modifiedRfc3339);
  if (!Number.isFinite(modified) || !Number.isFinite(now) || modified > now) return 'just now';
  const elapsed = Math.max(0, now - modified);
  if (elapsed < MINUTE) return 'just now';
  if (elapsed < HOUR) return plural(Math.floor(elapsed / MINUTE), 'minute');
  if (elapsed < DAY) return plural(Math.floor(elapsed / HOUR), 'hour');
  if (elapsed < WEEK) return plural(Math.floor(elapsed / DAY), 'day');
  if (elapsed < MONTH) return plural(Math.floor(elapsed / WEEK), 'week');
  if (elapsed < YEAR) return plural(Math.floor(elapsed / MONTH), 'month');
  return plural(Math.floor(elapsed / YEAR), 'year');
}

/**
 * Wall-clock time for one transcript message, in the composer's locale —
 * the `11:35 PM` shape. Hand-formatted rather than `toLocaleTimeString` so the
 * transcript reads identically in every locale and in tests; returns `''` for a
 * value that is not a usable timestamp, which the caller renders as no clock.
 */
export function formatClockTime(epochMs: number | undefined): string {
  if (epochMs === undefined || !Number.isFinite(epochMs)) return '';
  const at = new Date(epochMs);
  const hours = at.getHours();
  const hour = hours % 12 === 0 ? 12 : hours % 12;
  const minutes = at.getMinutes().toString().padStart(2, '0');
  return `${hour}:${minutes} ${hours < 12 ? 'AM' : 'PM'}`;
}

export function formatSessionMetadata(modifiedRfc3339: string, messageCount: number, now = Date.now()): string {
  const count = Number.isSafeInteger(messageCount) && messageCount >= 0 ? messageCount : 0;
  return `${formatRelativeSessionTime(modifiedRfc3339, now)} · ${count} ${count === 1 ? 'message' : 'messages'}`;
}
