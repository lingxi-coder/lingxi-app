import type { AccountUsageRowDto } from '@lingxi/bridge-client';
import { logicalProviderId } from './providerAccounts';

/** Tokens and cost for one account generation, summed across its models. */
export interface AccountUsageSlice {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheReadTokens: number;
  readonly cacheCreationTokens: number;
  readonly reasoningTokens: number;
  readonly webSearchRequests: number;
  readonly nanoUsd: number;
  readonly requestCount: number;
  readonly models: readonly AccountModelUsage[];
}

export interface AccountModelUsage {
  readonly model: string;
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheReadTokens: number;
  readonly cacheCreationTokens: number;
  readonly nanoUsd: number;
  readonly requestCount: number;
}

export interface AccountSessionUsage {
  /** Usage for the account's current credential generation. */
  readonly current: AccountUsageSlice;
  /** Earlier credential generations. They are not mixed into `current`. */
  readonly earlier: readonly { readonly generation: number; readonly usage: AccountUsageSlice }[];
}

export interface AccountUsageTarget {
  readonly accountId: string;
  readonly identityGeneration: number;
}

/** Compact token count. Values under 1,000 stay exact. */
export function formatSessionTokens(count: number): string {
  const safe = Number.isFinite(count) ? Math.max(0, Math.floor(count)) : 0;
  const units: readonly (readonly [number, string])[] = [
    [1_000_000_000_000, 't'],
    [1_000_000_000, 'b'],
    [1_000_000, 'm'],
    [1_000, 'k'],
  ];
  for (const [threshold, suffix] of units) {
    if (safe >= threshold) {
      const rounded = Math.round((safe / threshold) * 10) / 10;
      return `${rounded.toFixed(1).replace(/\.0$/, '')}${suffix}`;
    }
  }
  return String(safe);
}

/** Session cost from nano-USD. Amounts above $0.50 show cents. */
export function formatSessionUsd(nanoUsd: number): string {
  const usd = (Number.isFinite(nanoUsd) ? nanoUsd : 0) / 1_000_000_000;
  if (usd > 0.5) return `$${(Math.round(usd * 100) / 100).toFixed(2)}`;
  return `$${usd.toFixed(4)}`;
}

function emptySlice(): AccountUsageSlice {
  return {
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0,
    reasoningTokens: 0, webSearchRequests: 0, nanoUsd: 0, requestCount: 0, models: [],
  };
}

function addRow(slice: AccountUsageSlice, row: AccountUsageRowDto): AccountUsageSlice {
  const model = slice.models.find(item => item.model === row.model);
  const nextModel: AccountModelUsage = {
    model: row.model,
    inputTokens: (model?.inputTokens ?? 0) + row.input_tokens,
    outputTokens: (model?.outputTokens ?? 0) + row.output_tokens,
    cacheReadTokens: (model?.cacheReadTokens ?? 0) + row.cache_read_input_tokens,
    cacheCreationTokens: (model?.cacheCreationTokens ?? 0) + row.cache_creation_input_tokens,
    nanoUsd: (model?.nanoUsd ?? 0) + row.total_nano_usd,
    requestCount: (model?.requestCount ?? 0) + row.request_count,
  };
  return {
    inputTokens: slice.inputTokens + row.input_tokens,
    outputTokens: slice.outputTokens + row.output_tokens,
    cacheReadTokens: slice.cacheReadTokens + row.cache_read_input_tokens,
    cacheCreationTokens: slice.cacheCreationTokens + row.cache_creation_input_tokens,
    reasoningTokens: slice.reasoningTokens + row.reasoning_tokens,
    webSearchRequests: slice.webSearchRequests + row.web_search_requests,
    nanoUsd: slice.nanoUsd + row.total_nano_usd,
    requestCount: slice.requestCount + row.request_count,
    models: [...slice.models.filter(item => item.model !== row.model), nextModel].sort((a, b) => a.model.localeCompare(b.model)),
  };
}

/**
 * Session usage for one live account.
 *
 * Rows without an account stay out of every provider list. A matching account
 * id on an older `identity_generation` is a previous key. `openai-chatgpt`
 * folds to `openai` before the provider comparison.
 */
export function accountSessionUsage(
  rows: readonly AccountUsageRowDto[] | undefined,
  providerId: string,
  account: AccountUsageTarget,
): AccountSessionUsage {
  const provider = logicalProviderId(providerId);
  let current = emptySlice();
  const earlier = new Map<number, AccountUsageSlice>();
  for (const row of rows ?? []) {
    const attribution = row.attribution;
    if (!attribution || logicalProviderId(attribution.provider_id) !== provider || attribution.account_id !== account.accountId) continue;
    if (attribution.identity_generation === account.identityGeneration) current = addRow(current, row);
    else earlier.set(attribution.identity_generation, addRow(earlier.get(attribution.identity_generation) ?? emptySlice(), row));
  }
  return {
    current,
    earlier: [...earlier.entries()].sort((a, b) => a[0] - b[0]).map(([generation, usage]) => ({ generation, usage })),
  };
}
