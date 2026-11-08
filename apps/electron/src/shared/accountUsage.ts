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

interface MutableModelUsage {
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  nanoUsd: number;
  requestCount: number;
}

interface MutableUsage {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  reasoningTokens: number;
  webSearchRequests: number;
  nanoUsd: number;
  requestCount: number;
  models: Map<string, MutableModelUsage>;
}

function emptyMutable(): MutableUsage {
  return {
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0,
    reasoningTokens: 0, webSearchRequests: 0, nanoUsd: 0, requestCount: 0, models: new Map(),
  };
}

function addRow(slice: MutableUsage, row: AccountUsageRowDto): void {
  slice.inputTokens += row.input_tokens;
  slice.outputTokens += row.output_tokens;
  slice.cacheReadTokens += row.cache_read_input_tokens;
  slice.cacheCreationTokens += row.cache_creation_input_tokens;
  slice.reasoningTokens += row.reasoning_tokens;
  slice.webSearchRequests += row.web_search_requests;
  slice.nanoUsd += row.total_nano_usd;
  slice.requestCount += row.request_count;
  const model = slice.models.get(row.model);
  if (model) {
    model.inputTokens += row.input_tokens;
    model.outputTokens += row.output_tokens;
    model.cacheReadTokens += row.cache_read_input_tokens;
    model.cacheCreationTokens += row.cache_creation_input_tokens;
    model.nanoUsd += row.total_nano_usd;
    model.requestCount += row.request_count;
    return;
  }
  slice.models.set(row.model, {
    model: row.model,
    inputTokens: row.input_tokens,
    outputTokens: row.output_tokens,
    cacheReadTokens: row.cache_read_input_tokens,
    cacheCreationTokens: row.cache_creation_input_tokens,
    nanoUsd: row.total_nano_usd,
    requestCount: row.request_count,
  });
}

function freeze(slice: MutableUsage): AccountUsageSlice {
  return {
    inputTokens: slice.inputTokens,
    outputTokens: slice.outputTokens,
    cacheReadTokens: slice.cacheReadTokens,
    cacheCreationTokens: slice.cacheCreationTokens,
    reasoningTokens: slice.reasoningTokens,
    webSearchRequests: slice.webSearchRequests,
    nanoUsd: slice.nanoUsd,
    requestCount: slice.requestCount,
    models: [...slice.models.values()].sort((a, b) => a.model.localeCompare(b.model)),
  };
}

/**
 * Session usage for one live account.
 *
 * Rows without an account stay out of every provider list. A matching account
 * id on an older `identity_generation` is a previous key. `openai-chatgpt`
 * folds to `openai` before the provider comparison.
 */
/** One pass over session rows, keyed by account id then credential generation. */
export function accountUsageByAccount(
  rows: readonly AccountUsageRowDto[] | undefined,
  providerId: string,
): ReadonlyMap<string, ReadonlyMap<number, AccountUsageSlice>> {
  const provider = logicalProviderId(providerId);
  const accounts = new Map<string, Map<number, MutableUsage>>();
  for (const row of rows ?? []) {
    const attribution = row.attribution;
    if (!attribution || logicalProviderId(attribution.provider_id) !== provider) continue;
    let generations = accounts.get(attribution.account_id);
    if (!generations) {
      generations = new Map();
      accounts.set(attribution.account_id, generations);
    }
    let slice = generations.get(attribution.identity_generation);
    if (!slice) {
      slice = emptyMutable();
      generations.set(attribution.identity_generation, slice);
    }
    addRow(slice, row);
  }
  const frozen = new Map<string, ReadonlyMap<number, AccountUsageSlice>>();
  for (const [accountId, generations] of accounts) {
    const slices = new Map<number, AccountUsageSlice>();
    for (const [generation, slice] of generations) slices.set(generation, freeze(slice));
    frozen.set(accountId, slices);
  }
  return frozen;
}

/** Current generation versus earlier keys for one live account. */
export function sessionUsageFromIndex(
  index: ReadonlyMap<string, ReadonlyMap<number, AccountUsageSlice>>,
  account: AccountUsageTarget,
): AccountSessionUsage {
  const generations = index.get(account.accountId);
  const current = generations?.get(account.identityGeneration) ?? emptySlice();
  const earlier: { generation: number; usage: AccountUsageSlice }[] = [];
  if (generations) {
    for (const [generation, usage] of generations) {
      if (generation !== account.identityGeneration) earlier.push({ generation, usage });
    }
    earlier.sort((a, b) => a.generation - b.generation);
  }
  return { current, earlier };
}

export function accountSessionUsage(
  rows: readonly AccountUsageRowDto[] | undefined,
  providerId: string,
  account: AccountUsageTarget,
): AccountSessionUsage {
  return sessionUsageFromIndex(accountUsageByAccount(rows, providerId), account);
}
