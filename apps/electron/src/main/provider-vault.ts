import { desktopTypeOfProtocol, isDesktopProviderType, isProviderId, isVaultBaseUrl, protocolOfDesktopType, type DesktopProviderType, type VaultExport, type VaultProvider, type VaultUpsert } from '../../../../packages/llm-vault/contracts';
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
 * Whether a provider id is one of this Mac's built-in providers, by the runtime's official catalog (which has providers,
 * such as Qwen and MiniMax, that the static list does not). The host passes it in.
 */
export type BuiltinProviders = (id: string) => boolean;

const API_VERSION = /^[A-Za-z0-9._-]{1,40}$/u, REGION = /^[a-z]{2}(?:-[a-z0-9]+)+-\d{1,2}$/u;
const text = (value: unknown) => typeof value === 'string' ? value.trim() : '';
/** Bedrock is addressed by region; the vault keeps a base URL for every record, so it stores the region's endpoint. */
const bedrockBaseUrl = (region: string) => `https://bedrock-runtime.${region}.amazonaws.com`;

/**
 * The vault id for one provider account of this Mac: the record it is already linked to, else the provider's own id while no
 * other account of this Mac holds that one, else an id of its own, so a second account never replaces the first one's key.
 */
export function vaultIdForAccount(providerId: string, accountId: string, linked: string | undefined, heldByAnother: (vaultId: string) => boolean): string | null {
  if (linked) return linked;
  if (isProviderId(providerId) && !heldByAnother(providerId)) return providerId;
  const suffix = accountId.toLowerCase().replace(/[^a-z0-9]/gu, '').slice(0, 8);
  const id = `${providerId.slice(0, 55)}.${suffix}`;
  return suffix && isProviderId(id) ? id : null;
}

/**
 * The vault record for one provider account of this Mac, or `null` when the vault cannot hold it: a built-in provider the
 * vault has no preset for (GitHub Copilot, a ChatGPT login), or a custom provider whose endpoint the vault would refuse (not
 * https on 443, an IP or local host name) or that lacks what Desktop needs to call it again (Azure's API version, Bedrock's
 * region). `profile` is the custom provider's settings entry, absent for a built-in provider. Without `apiKey` it is a
 * change that keeps the stored key (a custom provider's models, say).
 */
export function vaultUpsertFor(account: { vaultId: string; providerId: string; displayName: string }, builtin: BuiltinProviders, profile: unknown, apiKey?: string): VaultUpsert | null {
  const label = account.displayName.trim().slice(0, 80) || account.providerId;
  const key = apiKey === undefined ? {} : { apiKey };
  if (builtin(account.providerId)) return VAULT_PRESET_IDS.has(account.providerId) ? { action: 'upsert', id: account.vaultId, presetId: account.providerId, label, ...key } : null;
  if (!profile || typeof profile !== 'object' || Array.isArray(profile)) return null;
  const { type, models } = profile as { type?: unknown; models?: unknown };
  if (!isDesktopProviderType(type)) return null;
  const region = text((profile as { region?: unknown }).region), apiVersion = text((profile as { apiVersion?: unknown }).apiVersion);
  if (type === 'bedrock-claude' && !REGION.test(region)) return null;
  if (type === 'azure-openai' && !API_VERSION.test(apiVersion)) return null;
  const baseUrl = text((profile as { baseUrl?: unknown }).baseUrl) || (type === 'bedrock-claude' ? bedrockBaseUrl(region) : '');
  if (!isVaultBaseUrl(baseUrl)) return null;
  const ids = modelIds(models);
  return { action: 'upsert', id: account.vaultId, label, protocol: protocolOfDesktopType(type), desktopType: type, baseUrl,
    models: ids.map(id => ({ id })), ...(ids[0] ? { defaultModel: ids[0] } : {}),
    ...(type === 'azure-openai' ? { apiVersion } : {}), ...(type === 'bedrock-claude' ? { region } : {}), ...key };
}

/** A custom provider's settings entry as a pull writes it. */
export interface VaultInstallProfile { type: DesktopProviderType; baseUrl: string; models: Array<{ id: string }>; apiVersion?: string; region?: string }
/** How a vault provider lands on this Mac: as an account of a built-in provider, or of a custom provider (added when missing). */
export type VaultInstallPlan =
  | { kind: 'builtin'; providerId: string; displayName: string }
  | { kind: 'custom'; providerId: string; displayName: string; profile: VaultInstallProfile | null }
  | { kind: 'unsupported'; code: 'ID_CONFLICT' | 'UNSUPPORTED_PROVIDER' | 'NO_MODELS' | 'PROVIDER_INCOMPLETE' };

export function vaultInstallPlan(provider: VaultProvider, profiles: Readonly<Record<string, unknown>>, builtin: BuiltinProviders): VaultInstallPlan {
  const displayName = provider.label.slice(0, 128);
  if (provider.presetId && VAULT_PRESET_IDS.has(provider.presetId)) {
    return builtin(provider.presetId) ? { kind: 'builtin', providerId: provider.presetId, displayName } : { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' };
  }
  // A custom vault provider never becomes an account of the built-in provider that happens to share its id.
  if (builtin(provider.id)) return { kind: 'unsupported', code: 'ID_CONFLICT' };
  const type = provider.desktopType ?? desktopTypeOfProtocol(provider.protocol);
  if (!type) return { kind: 'unsupported', code: 'UNSUPPORTED_PROVIDER' };
  const extra = type === 'azure-openai' ? (provider.apiVersion ? { apiVersion: provider.apiVersion } : null)
    : type === 'bedrock-claude' ? (provider.region ? { region: provider.region } : null) : {};
  if (!extra) return { kind: 'unsupported', code: 'PROVIDER_INCOMPLETE' };
  // Nor of a custom provider of this Mac that only shares its name: the key would go to that provider's endpoint.
  if (Object.hasOwn(profiles, provider.id)) return sameEndpoint(profiles[provider.id], type, provider.baseUrl, extra)
    ? { kind: 'custom', providerId: provider.id, displayName, profile: null } : { kind: 'unsupported', code: 'ID_CONFLICT' };
  const ids = modelIds(provider.models);
  // The provider's default model leads, so it is also the first model Desktop offers.
  if (provider.defaultModel) { const at = ids.indexOf(provider.defaultModel); if (at > 0) ids.splice(at, 1); if (at !== 0) ids.unshift(provider.defaultModel); }
  if (!ids.length) return { kind: 'unsupported', code: 'NO_MODELS' };
  return { kind: 'custom', providerId: provider.id, displayName, profile: { type, baseUrl: provider.baseUrl, models: ids.map(id => ({ id })), ...extra } };
}

const withoutTrailingSlash = (url: string) => url.trim().replace(/\/+$/u, '');
/** Whether this Mac's custom provider settings call the same endpoint as the vault record: kind, base URL, version, region. */
function sameEndpoint(profile: unknown, type: DesktopProviderType, baseUrl: string, extra: { apiVersion?: string; region?: string }): boolean {
  if (!profile || typeof profile !== 'object' || Array.isArray(profile)) return false;
  const local = profile as { type?: unknown; baseUrl?: unknown; apiVersion?: unknown; region?: unknown };
  if (local.type !== type || (extra.apiVersion !== undefined && text(local.apiVersion) !== extra.apiVersion) || (extra.region !== undefined && text(local.region) !== extra.region)) return false;
  // A Bedrock provider of this Mac may name only its region; the vault keeps that region's endpoint.
  const localBase = text(local.baseUrl) || (type === 'bedrock-claude' && extra.region ? bedrockBaseUrl(extra.region) : '');
  return !!localBase && withoutTrailingSlash(localBase) === withoutTrailingSlash(baseUrl);
}
