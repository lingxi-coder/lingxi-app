import { inviteCode } from '../../../../packages/teams';

const key = 'lingxi.teams.inviteIntent.1';
const lifetime = 7 * 24 * 60 * 60 * 1000;
type StoragePort = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
/** A same-tab invitation survives registration, auth popups and refresh. The
 * backend still checks expiry/revocation and joining always needs confirmation. */
export function readInviteIntent(url: string, storage: StoragePort, now = Date.now()): string {
  const incoming = inviteCode(url);
  if (incoming) return incoming;
  try {
    const raw: unknown = JSON.parse(storage.getItem(key) ?? 'null');
    if (raw && typeof raw === 'object' && 'code' in raw && 'expiresAt' in raw
      && typeof raw.code === 'string' && inviteCode(raw.code) === raw.code
      && typeof raw.expiresAt === 'number' && Number.isSafeInteger(raw.expiresAt)
      && raw.expiresAt > now && raw.expiresAt <= now + lifetime) return raw.code;
    storage.removeItem(key);
  } catch { /* A denied browser store does not prevent an in-page invitation. */ }
  return '';
}
export function retainInviteIntent(code: string, storage: StoragePort, now = Date.now()): boolean {
  if (!code || inviteCode(code) !== code) return false;
  try {
    // Refreshing must not extend the local lifetime of the same invitation.
    const old = JSON.parse(storage.getItem(key) ?? 'null') as { code?: unknown; expiresAt?: unknown } | null;
    if (old?.code === code && typeof old.expiresAt === 'number' && old.expiresAt > now && old.expiresAt <= now + lifetime) return true;
    storage.setItem(key, JSON.stringify({ code, expiresAt: now + lifetime }));
    return true;
  } catch { return false; }
}
export function clearInviteIntent(storage: StoragePort): void {
  try { storage.removeItem(key); } catch { /* The visible intent is cleared by its owner. */ }
}
