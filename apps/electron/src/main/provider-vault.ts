import { desktopTypeOfProtocol, protocolOfDesktopType, type DesktopProviderType, type VaultExport, type VaultProvider, type VaultUpsert } from '../../../../packages/llm-vault/contracts';
import { providerById } from '../shared/providers.js';
import { VAULT_PRESET_IDS } from '../shared/providerVault.js';

/** The account the vault is called as. The ID token stays in the main process. */
export interface VaultAccount {
  snapshot(): { user: { uid: string } | null };
  sessionEpoch?(): number;
  getIdToken(forceRefresh?: boolean): Promise<string>;
}

export class ProviderVaultError extends Error {
  constructor(readonly code: string, readonly status = 0) { super(code); this.name = 'ProviderVaultError'; }
}

const DESKTOP_TYPES: readonly DesktopProviderType[] = ['openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai',
  'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude'];

/**
 * The model-provider vault for this Mac (`apps/firebase/src/llm-api.mjs`). Calls come from the main process with no Origin
 * header and `X-LingXi-Client: desktop`, which is what lets `exportKeys` return keys: a browser always sends Origin.
 */
export class ProviderVaultClient {
  constructor(private readonly account: VaultAccount, private readonly base = 'https://lingxios.net/v1/llm',
    private readonly fetcher: typeof fetch = fetch) {}

  list(expectedUid: string): Promise<VaultProvider[]> {
    return this.call<{ providers: VaultProvider[] }>(expectedUid, 'GET', '/providers').then(value => value.providers);
  }
  upsert(expectedUid: string, input: VaultUpsert): Promise<VaultProvider> {
    return this.call<{ provider: VaultProvider }>(expectedUid, 'POST', '/providers', input).then(value => value.provider);
  }
  async remove(expectedUid: string, id: string, expectedRevision?: number): Promise<void> {
    await this.call(expectedUid, 'POST', '/providers', { action: 'delete', id, ...(expectedRevision === undefined ? {} : { expectedRevision }) });
  }
  /** Keys in plaintext, for this Mac's credential store only. The vault refuses unless the account signed in within five minutes. */
  exportKeys(expectedUid: string, ids: readonly string[]): Promise<VaultExport[]> {
    return this.call<{ providers: VaultExport[] }>(expectedUid, 'POST', '/providers/export', { ids }).then(value => value.providers);
  }

  private async call<T>(expectedUid: string, method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
    const epoch = this.account.sessionEpoch?.();
    const assertAccount = () => {
      if (this.account.snapshot().user?.uid !== expectedUid || this.account.sessionEpoch?.() !== epoch) throw new ProviderVaultError('INVALID_SESSION', 401);
    };
    assertAccount();
    for (let attempt = 0; ; attempt++) {
      let token: string;
      try { token = await this.account.getIdToken(attempt > 0); } catch { throw new ProviderVaultError('SIGN_IN_REQUIRED', 401); }
      assertAccount();
      let response: Response;
      try {
        response = await this.fetcher(`${this.base}${path}`, {
          method, redirect: 'error', signal: AbortSignal.timeout(30_000),
          headers: { Authorization: `Bearer ${token}`, 'X-LingXi-Client': 'desktop', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        });
      } catch { throw new ProviderVaultError('NETWORK'); }
      const value = await response.json().catch(() => null) as (T & { error?: { code?: unknown } }) | null;
      assertAccount();
      if (response.ok && value) return value;
      const code = typeof value?.error?.code === 'string' ? value.error.code : response.ok ? 'INVALID_RESPONSE' : `HTTP_${response.status}`;
      // A cached token the vault no longer accepts gets one fresh try.
      if (code === 'INVALID_SESSION' && attempt === 0) continue;
      throw new ProviderVaultError(code, response.status);
    }
  }
}

const modelIds = (models: unknown): string[] => Array.isArray(models)
  ? [...new Set(models.map(model => typeof model === 'string' ? model : model && typeof model === 'object' ? (model as { id?: unknown }).id : undefined)
    .filter((id): id is string => typeof id === 'string' && id.trim().length > 0).map(id => id.trim()))].slice(0, 100)
  : [];

/**
 * The vault record for a key just saved on this Mac, or `null` when the vault cannot hold it: a built-in provider the vault
 * has no preset for (GitHub Copilot, a ChatGPT login), or a custom provider without an https base URL. `profile` is the
 * custom provider's settings entry, absent for a built-in provider.
 */
export function vaultUpsertFor(providerId: string, profile: unknown, apiKey: string): VaultUpsert | null {
  const builtin = providerById(providerId);
  if (builtin) return VAULT_PRESET_IDS.has(providerId) ? { action: 'upsert', id: providerId, presetId: providerId, label: builtin.label, apiKey } : null;
  if (!profile || typeof profile !== 'object' || Array.isArray(profile)) return null;
  const { type, baseUrl, models } = profile as { type?: unknown; baseUrl?: unknown; models?: unknown };
  if (!DESKTOP_TYPES.includes(type as DesktopProviderType) || typeof baseUrl !== 'string' || !/^https:\/\//iu.test(baseUrl.trim())) return null;
  const ids = modelIds(models);
  return { action: 'upsert', id: providerId, label: providerId, protocol: protocolOfDesktopType(type as DesktopProviderType),
    desktopType: type as DesktopProviderType, baseUrl: baseUrl.trim(), models: ids.map(id => ({ id })), ...(ids[0] ? { defaultModel: ids[0] } : {}), apiKey };
}

/** How a vault provider lands on this Mac: as an account of a built-in provider, or of a custom provider (added when missing). */
export type VaultInstallPlan =
  | { kind: 'builtin'; providerId: string; displayName: string }
  | { kind: 'custom'; providerId: string; displayName: string; profile: { type: DesktopProviderType; baseUrl: string; models: Array<{ id: string }> } | null }
  | { kind: 'unsupported'; code: 'ID_CONFLICT' | 'UNSUPPORTED_PROVIDER' | 'NO_MODELS' };

export function vaultInstallPlan(provider: VaultProvider, profiles: Readonly<Record<string, unknown>>): VaultInstallPlan {
  const displayName = provider.label.slice(0, 128);
  if (provider.presetId && providerById(provider.presetId) && VAULT_PRESET_IDS.has(provider.presetId)) return { kind: 'builtin', providerId: provider.presetId, displayName };
  // A custom vault provider never becomes an account of the built-in provider that happens to share its id.
  if (providerById(provider.id)) return { kind: 'unsupported', code: 'ID_CONFLICT' };
  const type = provider.desktopType ?? desktopTypeOfProtocol(provider.protocol);
  // Nor of a custom provider of this Mac that only shares its name: the key would go to that provider's endpoint.
  if (Object.hasOwn(profiles, provider.id)) return sameEndpoint(profiles[provider.id], type, provider.baseUrl)
    ? { kind: 'custom', providerId: provider.id, displayName, profile: null } : { kind: 'unsupported', code: 'ID_CONFLICT' };
  if (!type) return { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' };
  const ids = modelIds(provider.models);
  // The provider's default model leads, so it is also the first model Desktop offers.
  if (provider.defaultModel) { const at = ids.indexOf(provider.defaultModel); if (at > 0) ids.splice(at, 1); if (at !== 0) ids.unshift(provider.defaultModel); }
  if (!ids.length) return { kind: 'unsupported', code: 'NO_MODELS' };
  return { kind: 'custom', providerId: provider.id, displayName, profile: { type, baseUrl: provider.baseUrl, models: ids.map(id => ({ id })) } };
}

const withoutTrailingSlash = (url: string) => url.trim().replace(/\/+$/u, '');
/** Whether this Mac's custom provider settings call the same kind of endpoint at the same base URL as the vault record. */
function sameEndpoint(profile: unknown, type: DesktopProviderType | null, baseUrl: string): boolean {
  if (!type || !profile || typeof profile !== 'object' || Array.isArray(profile)) return false;
  const local = profile as { type?: unknown; baseUrl?: unknown };
  return local.type === type && typeof local.baseUrl === 'string' && withoutTrailingSlash(local.baseUrl) === withoutTrailingSlash(baseUrl);
}
