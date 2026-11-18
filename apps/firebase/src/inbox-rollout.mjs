import { createHash } from 'node:crypto';

// Delivery rollout selects one path per account. Summaries, canonical writes
// and membership checks are independent of this operator-owned switch.
export function inboxNotificationEnabled(uid, env = process.env) {
  const allow = (env.INBOX_NOTIFY_ALLOW_SHA256 ?? '')
    .split(',')
    .map(s => s.trim())
    .filter(Boolean);
  const value = env.INBOX_NOTIFY_ROLLOUT_PERCENT ?? (allow.length ? '0' : '100');
  const percent = /^(?:100|[0-9]{1,2})$/u.test(value) ? Number(value) : 0;
  if (percent === 100) return true;
  const hash = createHash('sha256').update(uid).digest('hex');
  return allow.includes(hash) || parseInt(hash.slice(0, 8), 16) % 100 < percent;
}
export const hasNotificationRollout = () =>
  process.env.INBOX_NOTIFY_ROLLOUT_PERCENT !== undefined || !!process.env.INBOX_NOTIFY_ALLOW_SHA256;
