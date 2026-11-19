import presetCatalog from '../../../firebase/src/llm-presets.json';
import type { VaultProvider } from '../../../../packages/llm-vault/contracts';

export type { VaultProvider } from '../../../../packages/llm-vault/contracts';

/** The cloud model-provider vault, reached through the main process only. A key never crosses this channel. */
export const CH_PROVIDER_VAULT = 'lingxi:provider-vault';

/** The presets the vault knows, which are also the built-in Desktop providers whose key can be synced to it. */
export const VAULT_PRESET_IDS: ReadonlySet<string> = new Set(presetCatalog.presets.map(preset => preset.id));

export type ProviderVaultCommand =
  | { action: 'list' }
  /** Copies these vault providers, keys included, into this Mac's provider accounts. */
  | { action: 'pull'; ids: string[] }
  | { action: 'remove'; id: string; expectedRevision?: number };

export interface ProviderVaultPull { id: string; ok: boolean; code?: string; localProviderId?: string }
export type ProviderVaultReply =
  | { ok: true; providers: VaultProvider[]; pulled?: ProviderVaultPull[] }
  /** `SIGN_IN_REQUIRED`, `REAUTH_REQUIRED` (pull only: sign in again with the same account first) or a vault error code. */
  | { ok: false; code: string };

/** What an add or reconnect with `cloudSync` reports beside the local result, which a cloud failure never undoes. */
export interface ProviderVaultSyncResult { ok: boolean; code?: string }

export interface ProviderVaultApi {
  command(command: ProviderVaultCommand): Promise<ProviderVaultReply>;
}

export function parseProviderVaultCommand(value: unknown): ProviderVaultCommand {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid provider vault command.');
  const input = value as Record<string, unknown>;
  const id = (item: unknown): item is string => typeof item === 'string' && /^[a-z0-9][a-z0-9._-]{0,63}$/u.test(item);
  if (input.action === 'list') return { action: 'list' };
  if (input.action === 'pull' && Array.isArray(input.ids) && input.ids.length >= 1 && input.ids.length <= 20 && input.ids.every(id))
    return { action: 'pull', ids: [...new Set(input.ids as string[])] };
  if (input.action === 'remove' && id(input.id) && (input.expectedRevision === undefined || (Number.isSafeInteger(input.expectedRevision) && (input.expectedRevision as number) >= 0)))
    return { action: 'remove', id: input.id, ...(input.expectedRevision === undefined ? {} : { expectedRevision: input.expectedRevision as number }) };
  throw new Error('Invalid provider vault command.');
}
