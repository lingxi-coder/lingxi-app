import type { AccountSessionStore } from './account.js';
import type { ProviderCredentialBroker } from './credential-broker.js';

const ACCOUNT_SESSION_ID = 'lingxi-account-session';
const DIAGNOSTIC_ACCOUNT_PREFIX = 'lingxi-account-diagnostic-';
const HOST_DIAGNOSTICS_FLAG = '--lingxi-host-diagnostics';
const ACCOUNT_SCOPE_FLAG = '--lingxi-account-session-scope';

function normalizedDiagnosticScope(scope: string): string {
  if (typeof scope !== 'string' || (scope.length !== 32 && scope.length !== 36)
    || !/^(?:[a-f0-9]{32}|[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})$/i.test(scope)) {
    throw new Error('Invalid host diagnostic account scope');
  }
  return scope.replaceAll('-', '').toLowerCase();
}

/** Only the host's explicit development diagnostic launch can select a separate account. */
export function accountSessionDiagnosticScope(
  argv: readonly string[],
  credentialChannel: ProviderCredentialBroker['credentialChannel'],
): string | undefined {
  const separator = argv.indexOf('--');
  const startup = separator < 0 ? argv : argv.slice(0, separator);
  const modes = startup.filter(arg => arg === HOST_DIAGNOSTICS_FLAG || arg.startsWith(`${HOST_DIAGNOSTICS_FLAG}=`));
  const scopes = startup.filter(arg => arg === ACCOUNT_SCOPE_FLAG || arg.startsWith(`${ACCOUNT_SCOPE_FLAG}=`));
  if (modes.length === 0 && scopes.length === 0) return undefined;
  if (credentialChannel !== 'development') throw new Error('Host account diagnostics require the development credential channel');
  if (modes.length !== 1 || modes[0] !== HOST_DIAGNOSTICS_FLAG || scopes.length !== 1
    || !scopes[0]!.startsWith(`${ACCOUNT_SCOPE_FLAG}=`)) {
    throw new Error('Host account diagnostics require one explicit mode and account scope');
  }
  return normalizedDiagnosticScope(scopes[0]!.slice(ACCOUNT_SCOPE_FLAG.length + 1));
}

/** Use the packaged macOS broker rather than probing Electron's own Keychain item at startup. */
export function brokerAccountSessionStore(
  broker: Pick<ProviderCredentialBroker, 'resolve' | 'set' | 'delete'> & Partial<Pick<ProviderCredentialBroker, 'credentialChannel'>>,
  diagnosticScope?: string,
): AccountSessionStore {
  if (diagnosticScope !== undefined && broker.credentialChannel !== 'development') {
    throw new Error('Host account diagnostics require the development credential channel');
  }
  const accountId = diagnosticScope === undefined
    ? ACCOUNT_SESSION_ID
    : `${DIAGNOSTIC_ACCOUNT_PREFIX}${normalizedDiagnosticScope(diagnosticScope)}`;
  return {
    persistent: true,
    async read() { return await broker.resolve(accountId) ?? null; },
    async write(value) { await broker.set(accountId, value); },
    async remove() { await broker.delete(accountId); },
  };
}
