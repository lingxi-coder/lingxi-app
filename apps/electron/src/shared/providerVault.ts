import presetCatalog from '../../../firebase/src/llm-presets.json';
import { isProviderId, type VaultProvider } from '../../../../packages/llm-vault/contracts';

export type { VaultProvider } from '../../../../packages/llm-vault/contracts';

/** The cloud model-provider vault, reached through the main process only. A key never crosses this channel. */
export const CH_PROVIDER_VAULT = 'lingxi:provider-vault';
/** Main → renderer: how a cloud sync that an add or reconnect started has ended (`ProviderVaultSynced`). */
export const CH_PROVIDER_VAULT_SYNCED = 'lingxi:provider-vault:synced';

/** The presets the vault knows, which are also the built-in Desktop providers whose key can be synced to it. */
export const VAULT_PRESET_IDS: ReadonlySet<string> = new Set(presetCatalog.presets.map(preset => preset.id));

export type ProviderVaultCommand =
  | { action: 'list' }
  /** Copies these vault providers, keys included, into this Mac's provider accounts. */
  | { action: 'pull'; ids: string[] }
  | { action: 'remove'; id: string; expectedRevision?: number };

export interface ProviderVaultPull { id: string; ok: boolean; code?: string; localProviderId?: string }
export type ProviderVaultReply =
  /** `providers` is the vault's list after the command; a pull whose keys are in but whose list could not be read leaves it out. */
  | { ok: true; providers?: VaultProvider[]; pulled?: ProviderVaultPull[] }
  /** `SIGN_IN_REQUIRED`, `REAUTH_REQUIRED` (pull only: sign in again with the same account first) or a vault error code. */
  | { ok: false; code: string };

/** How a cloud sync ended. A cloud failure never undoes the key saved on this Mac. */
export interface ProviderVaultSyncResult { ok: boolean; code?: string }
/**
 * An add or reconnect with `cloudSync` replies as soon as the key is saved on this Mac, with the `syncId` of the cloud sync it
 * started; that sync's result follows as this event.
 */
export interface ProviderVaultSynced extends ProviderVaultSyncResult { syncId: string }

export interface ProviderVaultApi {
  command(command: ProviderVaultCommand): Promise<ProviderVaultReply>;
  /** Results of cloud syncs started by adds and reconnects. */
  onSynced?(callback: (result: ProviderVaultSynced) => void): () => void;
}

export function parseProviderVaultCommand(value: unknown): ProviderVaultCommand {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid provider vault command.');
  const input = value as Record<string, unknown>;
  const id = (item: unknown): item is string => typeof item === 'string' && isProviderId(item);
  if (input.action === 'list') return { action: 'list' };
  if (input.action === 'pull' && Array.isArray(input.ids) && input.ids.length >= 1 && input.ids.length <= 20 && input.ids.every(id))
    return { action: 'pull', ids: [...new Set(input.ids as string[])] };
  if (input.action === 'remove' && id(input.id) && (input.expectedRevision === undefined || (Number.isSafeInteger(input.expectedRevision) && (input.expectedRevision as number) >= 0)))
    return { action: 'remove', id: input.id, ...(input.expectedRevision === undefined ? {} : { expectedRevision: input.expectedRevision as number }) };
  throw new Error('Invalid provider vault command.');
}
