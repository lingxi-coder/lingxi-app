import type { ProviderAccountSnapshot } from '../../shared/providerAccounts';

export function createProviderAccountSnapshotRefresh<Source>(options: {
  read(): Promise<ProviderAccountSnapshot>;
  source(): Source;
  isCurrent(source: Source): boolean;
  apply(snapshot: ProviderAccountSnapshot, source: Source): void;
  pending(value: boolean): void;
}) {
  let requested = 0;
  let pending: Promise<ProviderAccountSnapshot> | undefined;
  const refresh = (): Promise<ProviderAccountSnapshot> => {
    requested++;
    if (pending) return pending;
    options.pending(true);
    pending = Promise.resolve().then(async () => {
      try {
        let snapshot: ProviderAccountSnapshot | undefined;
        let request: number;
        do {
          request = requested;
          const source = options.source();
          try {
            snapshot = await options.read();
            if (request === requested && options.isCurrent(source)) options.apply(snapshot, source);
          } catch (error) {
            if (request === requested) throw error;
          }
        } while (request !== requested);
        if (!snapshot) throw new Error('Provider account snapshot unavailable.');
        return snapshot;
      } finally {
        pending = undefined;
        options.pending(false);
      }
    });
    return pending;
  };
  return { refresh };
}

// Runtime health can recover without another model call or bridge event.
// These reads never refresh tokens, mutate accounts, or choose an account.
export function providerAccountRecoveryState(snapshot?: ProviderAccountSnapshot) {
  const accounts = new Map(snapshot?.accounts.filter(account => account.enabled).map(account => [account.accountId, account.identityGeneration]));
  const live = (row: { accountId: string; identityGeneration: number }) => accounts.get(row.accountId) === row.identityGeneration;
  const health = snapshot?.health.filter(live) ?? [];
  const scoped = snapshot?.scopedHealth?.filter(live) ?? [];
  const entitlements = snapshot?.modelEntitlements?.filter(live) ?? [];
  const recovering = health.some(row => row.status === 'cooling_down' || row.status === 'quota_exhausted' || row.status === 'storage_unavailable')
    || scoped.length > 0 || entitlements.some(row => row.status === 'forbidden' && row.retryAtMs !== undefined)
    || (accounts.size > 0 && !(snapshot?.modelAvailability?.some(row => row.eligibleAccountIds.length > 0) ?? false));
  const deadlines = [...health.map(row => row.cooldownUntilMs), ...scoped.map(row => row.unavailableUntilMs), ...entitlements.map(row => row.retryAtMs)]
    .filter((value): value is number => value !== undefined && Number.isFinite(value) && value > Date.now());
  return { recovering, retryAtMs: deadlines.length ? Math.min(...deadlines) : undefined };
}

export function providerAccountUnavailableReasons(snapshot?: ProviderAccountSnapshot): string[] {
  if (!snapshot?.accounts.length) return [];
  const accounts = new Map(snapshot.accounts.filter(account => account.enabled).map(account => [account.accountId, account.identityGeneration]));
  const live = (row: { accountId: string; identityGeneration: number }) => accounts.get(row.accountId) === row.identityGeneration;
  const reasons = snapshot.health.filter(row => live(row) && row.status !== 'ready' && row.status !== 'disabled')
    .map(row => `provider_accounts_health_${row.status}`);
  const scopedReasons: Record<string, string> = { rate_limited: 'provider_accounts_health_cooling_down', quota_exhausted: 'provider_accounts_health_quota_exhausted' };
  for (const row of snapshot.scopedHealth?.filter(live) ?? []) reasons.push(scopedReasons[row.reason] ?? 'provider_accounts_temporarily_unavailable');
  if (snapshot.modelEntitlements?.some(row => live(row) && row.status === 'forbidden')) reasons.push('provider_accounts_model_forbidden');
  return [...new Set(reasons.length ? reasons : ['provider_accounts_models_unverified'])];
}
export function watchCredentialRecovery(options: {
  windowTarget: EventTarget;
  documentTarget: EventTarget;
  isVisible: () => boolean;
  refresh: () => Promise<void>;
  retryAtMs?: number;
  pollIntervalMs?: number;
}): () => void {
  let disposed = false;
  let pending = false;
  const refresh = () => {
    if (disposed || pending || !options.isVisible()) return;
    pending = true;
    void options.refresh().catch(() => {
      // Keep the canonical unavailable state visible until a metadata read succeeds.
    }).finally(() => { pending = false; });
  };
  options.windowTarget.addEventListener('focus', refresh);
  options.documentTarget.addEventListener('visibilitychange', refresh);
  const timer = setInterval(refresh, options.pollIntervalMs ?? 5_000);
  const deadline = options.retryAtMs === undefined || options.retryAtMs <= Date.now() ? undefined : setTimeout(refresh, Math.min(2_147_483_647, Math.max(0, options.retryAtMs - Date.now() + 25)));
  return () => {
    disposed = true;
    clearInterval(timer);
    clearTimeout(deadline);
    options.windowTarget.removeEventListener('focus', refresh);
    options.documentTarget.removeEventListener('visibilitychange', refresh);
  };
}
