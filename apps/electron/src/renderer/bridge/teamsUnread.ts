/** The most a badge counts before it says "99+". */
const BADGE_LIMIT = 99;

/**
 * What the rail's Teams badge shows for this many unread messages: nothing for none, the count up to 99, and
 * "99+" beyond. The accessible name of the button states the exact count; the digits are only the sighted summary.
 */
export function unreadBadgeText(count: number): string {
  const whole = Math.floor(count);
  if (!(whole >= 1)) return '';
  return whole > BADGE_LIMIT ? `${BADGE_LIMIT}+` : String(whole);
}
