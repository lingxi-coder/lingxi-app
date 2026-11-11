import { friendCodeOf } from '../../../../packages/teams/friends';

const key = 'lingxi.friends.addIntent.1';
const lifetime = 7 * 24 * 60 * 60 * 1000;
type StoragePort = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
/**
 * A friend link opened while signed out survives registration, closed tabs and refresh (D8). Opening it never redeems:
 * the signed-in page looks the code up and asks first, and the backend checks that it still works.
 */
export function readAddIntent(url: string, storage: StoragePort, now = Date.now()): string {
  const incoming = friendCodeOf(url);
  if (incoming) return incoming;
  try {
    const raw: unknown = JSON.parse(storage.getItem(key) ?? 'null');
    if (raw && typeof raw === 'object' && 'code' in raw && 'expiresAt' in raw
      && typeof raw.code === 'string' && friendCodeOf(raw.code) === raw.code
      && typeof raw.expiresAt === 'number' && Number.isSafeInteger(raw.expiresAt)
      && raw.expiresAt > now && raw.expiresAt <= now + lifetime) return raw.code;
    storage.removeItem(key);
  } catch { /* A denied browser store does not prevent an in-page friend link. */ }
  return '';
}
export function retainAddIntent(code: string, storage: StoragePort, now = Date.now()): boolean {
  if (!code || friendCodeOf(code) !== code) return false;
  try {
    // Refreshing must not extend the local lifetime of the same link.
    const old = JSON.parse(storage.getItem(key) ?? 'null') as { code?: unknown; expiresAt?: unknown } | null;
    if (old?.code === code && typeof old.expiresAt === 'number' && old.expiresAt > now && old.expiresAt <= now + lifetime) return true;
    storage.setItem(key, JSON.stringify({ code, expiresAt: now + lifetime }));
    return true;
  } catch { return false; }
}
export function clearAddIntent(storage: StoragePort, expectedCode: string): void {
  if (friendCodeOf(expectedCode) !== expectedCode) return;
  try {
    const current: unknown = JSON.parse(storage.getItem(key) ?? 'null');
    if (current && typeof current === 'object' && 'code' in current && current.code === expectedCode) storage.removeItem(key);
  } catch { /* The visible intent is cleared by its owner. */ }
}
