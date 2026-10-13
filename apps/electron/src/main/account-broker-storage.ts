import type { AccountSessionStore } from './account.js';
import type { ProviderCredentialBroker } from './credential-broker.js';

const ACCOUNT_SESSION_ID = 'lingxi-account-session';

/** Use the packaged macOS broker rather than probing Electron's own Keychain item at startup. */
export function brokerAccountSessionStore(
  broker: Pick<ProviderCredentialBroker, 'resolve' | 'set' | 'delete'>,
): AccountSessionStore {
  return {
    persistent: true,
    async read() { return await broker.resolve(ACCOUNT_SESSION_ID) ?? null; },
    async write(value) { await broker.set(ACCOUNT_SESSION_ID, value); },
    async remove() { await broker.delete(ACCOUNT_SESSION_ID); },
  };
}
