/** Host-owned redacted account contract. Secrets never occur in snapshots. */
export interface ProviderAccountMetadata {
  accountId: string;
  providerId: string;
  displayName: string;
  authKind: 'api_key' | 'oauth';
  executionBindingId: string;
  credentialRef: { storeScope: string; secretRef: string };
  enabled: boolean;
  userPriority: number;
  configRevision: number;
  identityGeneration: number;
}
export interface ProviderAccountHealth {
  accountId: string;
  identityGeneration: number;
  status: 'ready' | 'cooling_down' | 'needs_auth' | 'quota_exhausted' | 'storage_unavailable' | 'disabled';
  reason?: string;
  cooldownUntilMs?: number;
  healthRevision: number;
}
export interface ProviderAccountModelEntitlement {
  accountId: string;
  identityGeneration: number;
  modelId: string;
  status: 'unverified' | 'confirmed' | 'forbidden';
  observedAtMs: number;
  retryAtMs?: number;
  reason?: string;
}
export interface ProviderAccountModelAvailability {
  providerId: string;
  executionBindingId: string;
  modelId: string;
  eligibleAccountIds: string[];
  confirmedAccountIds: string[];
  unverifiedAccountIds: string[];
  forbiddenAccountIds: string[];
}
export interface ProviderAccountScopedHealth {
  accountId: string;
  identityGeneration: number;
  scopeKind: 'model' | 'endpoint';
  scopeId: string;
  unavailableUntilMs?: number;
  reason: string;
}
export interface ProviderAccountSnapshot {
  storeScope: string;
  configRevision: number;
  accounts: ProviderAccountMetadata[];
  health: ProviderAccountHealth[];
  scopedHealth?: ProviderAccountScopedHealth[];
  modelEntitlements?: ProviderAccountModelEntitlement[];
  modelAvailability?: ProviderAccountModelAvailability[];
  providerPolicies?: Record<string, { allowSubscriptionToMetered: boolean }>;
}
export interface ProviderAccountCommand {
  action: 'list' | 'add' | 'update' | 'reorder' | 'enable' | 'remove' | 'test' | 'logout' | 'reconnect' | 'set_credential' | 'set_policy';
  expectedRevision: number;
  operationId?: number;
  providerId?: string;
  accountId?: string;
  displayName?: string;
  executionBindingId?: string;
  authKind?: 'api_key' | 'oauth';
  accountIds?: string[];
  enabled?: boolean;
  allowSubscriptionToMetered?: boolean;
}
export interface ProviderAccountRouting {
  logicalCallId: string;
  model: string;
  attempt: number;
  status: 'selected' | 'all_unavailable';
  selectedAccountId: string | null;
  identityGeneration: number | null;
  configRevision: number | null;
}
export function parseProviderAccountRouting(input: unknown): ProviderAccountRouting {
  if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('Invalid account routing event.');
  const row = input as Record<string, unknown>;
  const text = (value: unknown, max: number): value is string => typeof value === 'string' && value.length > 0 && value.length <= max && !/[\u0000-\u001f\u007f]/.test(value);
  if (!text(row.logicalCallId, 128) || !text(row.model, 256) || !Number.isSafeInteger(row.attempt) || (row.attempt as number) < 0
    || !['selected', 'all_unavailable'].includes(String(row.status))
    || (row.selectedAccountId !== null && !text(row.selectedAccountId, 128))
    || (row.identityGeneration !== null && !Number.isSafeInteger(row.identityGeneration))
    || (row.configRevision !== null && !Number.isSafeInteger(row.configRevision))) throw new Error('Invalid account routing event.');
  return { logicalCallId: row.logicalCallId, model: row.model, attempt: row.attempt as number, status: row.status as ProviderAccountRouting['status'],
    selectedAccountId: row.selectedAccountId as string | null, identityGeneration: row.identityGeneration as number | null, configRevision: row.configRevision as number | null };
}
export interface ProviderAccountsApi {
  routing?(): Promise<ProviderAccountRouting | undefined>;
  onRouting?(callback: (event: ProviderAccountRouting) => void): () => void;
  snapshot(): Promise<ProviderAccountSnapshot>;
  command(request: ProviderAccountCommand, credential?: string): Promise<ProviderAccountSnapshot>;
  cancel(operationId: number): Promise<void>;
}
export interface HostProviderSettings { raw: Record<string, unknown>; effective: Record<string, unknown>; locked: string[] }

export const CH_PROVIDER_ACCOUNTS = 'lingxi:provider-accounts';
export const CH_PROVIDER_ACCOUNTS_CANCEL = 'lingxi:provider-accounts:cancel';
export function logicalProviderId(providerId: string): string { return providerId === 'openai-chatgpt' ? 'openai' : providerId; }
export function orderedProviderAccounts(snapshot: ProviderAccountSnapshot, providerId: string): ProviderAccountMetadata[] {
  return snapshot.accounts.filter(account => account.providerId === logicalProviderId(providerId))
    .sort((a, b) => a.userPriority - b.userPriority || a.accountId.localeCompare(b.accountId));
}

/** Configuration health is independent from model-specific permissions. */
export function providerAccountSummary(snapshot: ProviderAccountSnapshot, providerId: string): {
  status: 'unconfigured' | 'configured' | 'unverified' | 'disabled' | 'needs_auth' | 'temporarily_unavailable' | 'storage_unavailable';
  total: number;
  enabled: number;
} {
  const accounts = orderedProviderAccounts(snapshot, providerId);
  const enabled = accounts.filter(account => account.enabled);
  const counts = { total: accounts.length, enabled: enabled.length };
  if (!accounts.length) return { ...counts, status: 'unconfigured' };
  if (!enabled.length) return { ...counts, status: 'disabled' };
  const health = enabled.map(account => snapshot.health.find(row => row.accountId === account.accountId && row.identityGeneration === account.identityGeneration)?.status);
  if (health.includes('ready')) return { ...counts, status: 'configured' };
  if (health.some(status => !status || status === 'disabled')) return { ...counts, status: 'unverified' };
  if (health.every(status => status === 'needs_auth')) return { ...counts, status: 'needs_auth' };
  if (health.includes('storage_unavailable')) return { ...counts, status: 'storage_unavailable' };
  return { ...counts, status: 'temporarily_unavailable' };
}
