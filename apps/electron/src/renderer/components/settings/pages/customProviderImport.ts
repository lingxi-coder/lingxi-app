import { tr } from '../../../i18n/locale';
import { trimProviderDraft } from './customProviderDraft';
/** Provider import stays pure: secrets never enter the settings payload or diagnostics. */
export const SUPPORTED_PROVIDER_TYPES = [
  'openai', 'openai-responses', 'anthropic', 'gemini', 'azure-openai',
  'bedrock-claude', 'vertex-claude', 'vertex-gemini', 'foundry-claude',
] as const;
export type SupportedProviderType = (typeof SUPPORTED_PROVIDER_TYPES)[number];
export interface CustomProviderModelDraft { id: string; aliases?: string[]; [key: string]: unknown }
export interface CustomProviderConnectionDraft { id: string; [key: string]: unknown }
export interface CustomProviderDraft { type: string; baseUrl?: string; apiKeyEnv?: string; models: CustomProviderModelDraft[]; connections?: CustomProviderConnectionDraft[]; credentialIds?: string[]; [key: string]: unknown }
export interface ImportDiagnostic { severity: 'error' | 'warning'; message: string }
export interface ProviderImportEntry {
  name: string;
  draft: CustomProviderDraft;
  apiKey?: string;
  selected: boolean;
  conflict: boolean;
  diagnostics: ImportDiagnostic[];
}
export interface ProviderImportResult { entries: ProviderImportEntry[]; diagnostics: ImportDiagnostic[] }
const record = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v);
const nonempty = (v: unknown): v is string => typeof v === 'string' && v.trim().length > 0;
const hasOwn = (value: object, key: string): boolean => Object.prototype.hasOwnProperty.call(value, key);
const forbidden = new Set(['__proto__', 'prototype', 'constructor']);
const reservedProfileIds = new Set(['builtin', 'claude']);
export function validateProfileName(name: string): string | null {
  if (reservedProfileIds.has(name)) return tr('desktop_settings_30835417d3');
  return typeof name === 'string' && /^[a-z0-9][a-z0-9._-]{0,63}$/.test(name) && !forbidden.has(name)
    ? null : tr('desktop_settings_144eb627dd');
}
/**
 * Merge one connection over the provider-level defaults.
 *
 * Shallow, exactly like the engine's desugaring: a connection that redeclares
 * `models` means "this endpoint serves exactly these", not "add to the list".
 */
function mergeConnection(draft: CustomProviderDraft, connection: CustomProviderConnectionDraft): CustomProviderDraft {
  const base = { ...draft };
  delete base.connections;
  delete base.fallback;
  delete base.credentialIds;
  const merged = { ...base } as Record<string, unknown>;
  for (const [key, value] of Object.entries(connection)) {
    if (key === 'id' || key === 'credentialIds') continue;
    merged[key] = value;
  }
  return merged as CustomProviderDraft;
}

const FALLBACK_TRIGGERS = ['rate_limit', 'overloaded', 'server_error', 'network', 'auth'];

/**
 * `credentialIds` must be a non-empty list of distinct non-empty credential ids.
 *
 * These name secrets already in the keychain. The field is deliberately not
 * `apiKeys`: a settings key spelled that way invites pasting a real key into
 * `settings.json`, which the clients' secret-free guards reject outright.
 */
function validateCredentialIds(value: unknown, label: string): string | null {
  if (value === undefined) return null;
  if (!Array.isArray(value) || !value.length) return tr('desktop_settings_186903c5f4', label);
  if (value.some((key) => !nonempty(key))) return tr('desktop_settings_ba3db89e32', label);
  if (new Set(value.map((key) => (key as string).trim())).size !== value.length) return tr('desktop_settings_d453a799e5', label);
  return null;
}

/**
 * A provider reachable several ways: validate each connection as if it were the
 * flat provider it desugars to, so one endpoint cannot pass rules another fails.
 */
function validateConnections(draft: CustomProviderDraft): string | null {
  const connections = draft.connections;
  if (!Array.isArray(connections) || !connections.length) return tr('desktop_settings_1d1042756a');
  const seen = new Set<string>();
  for (const [index, connection] of connections.entries()) {
    if (!record(connection)) return tr('desktop_settings_2696d785f7', index);
    const id = connection.id;
    if (!nonempty(id)) return tr('desktop_settings_071521f356', index);
    const trimmed = (id as string).trim();
    // The id becomes part of a qualified model reference (`provider:conn/model`),
    // so a separator in it would produce a reference that cannot be routed.
    if (/[/:#]/.test(trimmed)) return tr('desktop_settings_2e7458cb01', JSON.stringify(trimmed));
    if (seen.has(trimmed)) return tr('desktop_settings_078850f795', JSON.stringify(trimmed));
    seen.add(trimmed);
    const keysError = validateCredentialIds(connection.credentialIds, tr('desktop_settings_40a568cf54', JSON.stringify(trimmed)));
    if (keysError) return keysError;
    const merged = mergeConnection(draft, connection as CustomProviderConnectionDraft);
    const error = validateCustomProvider(merged);
    if (error) return tr('desktop_settings_38ef74e728', JSON.stringify(trimmed), error);
  }
  if (draft.fallback !== undefined) {
    if (!record(draft.fallback)) return tr('desktop_settings_7ba28be21b');
    const on = (draft.fallback as Record<string, unknown>).on;
    if (on !== undefined) {
      if (!Array.isArray(on)) return tr('desktop_settings_080c4ed554');
      for (const trigger of on) {
        if (!nonempty(trigger) || !FALLBACK_TRIGGERS.includes((trigger as string).trim())) return tr('desktop_settings_7e716339b6', FALLBACK_TRIGGERS.join(', '));
      }
    }
  }
  return null;
}

/**
 * Whether authentication is satisfied at the CONNECTION level.
 *
 * `addProviderConnection` moves `apiKeyEnv` down onto the connections, so a
 * multi-connection provider legitimately has none at provider level. Demanding
 * one there rejected a provider whose every connection is authenticated.
 */
export function connectionsCarryAuth(draft: CustomProviderDraft): boolean {
  const connections = Array.isArray(draft.connections) ? draft.connections : [];
  if (!connections.length) return false;
  return connections.every((connection) =>
    nonempty((connection as Record<string, unknown>).apiKeyEnv) ||
    (Array.isArray((connection as Record<string, unknown>).credentialIds) &&
      ((connection as Record<string, unknown>).credentialIds as unknown[]).length > 0));
}

export function validateCustomProvider(draft: CustomProviderDraft): string | null {
  // A provider with `connections` is validated CONNECTION BY CONNECTION below;
  // the provider entry itself only supplies defaults, so it need not satisfy
  // baseUrl/models on its own.
  // The provider-level list is checked FIRST either way: Rust, Swift and Kotlin
  // all reject a bad one, so returning early on `connections` let a config pass
  // here and fail in the engine.
  const keysError = validateCredentialIds(draft.credentialIds, '');
  if (keysError) return keysError;
  if (record(draft) && draft.connections !== undefined) return validateConnections(draft);
  if (!record(draft) || !SUPPORTED_PROVIDER_TYPES.includes(draft.type as SupportedProviderType)) return tr('desktop_settings_9221cf14be', SUPPORTED_PROVIDER_TYPES.join(', '));
  if (!Array.isArray(draft.models) || !draft.models.length) return tr('desktop_settings_a58fc33a23');
  if (draft.models.some(m => !record(m) || !nonempty(m.id))) return tr('desktop_settings_b75df669db');
  if (new Set(draft.models.map(m => m.id.trim())).size !== draft.models.length) return tr('desktop_settings_740f825ba2');
  if (draft.models.some(m => m.aliases !== undefined && (!Array.isArray(m.aliases) || m.aliases.some(a => !nonempty(a))))) return tr('desktop_settings_e912598ff7');
  for (const model of draft.models) {
    if (model.capabilities !== undefined && (!record(model.capabilities) || Object.values(model.capabilities).some(v => typeof v !== 'boolean'))) return tr('desktop_settings_47a44a3b47');
    if (model.metadata !== undefined && !validMetadata(model.metadata)) return tr('desktop_settings_11cc3f633b');
  }
  if (draft.visionDelegate !== undefined && (typeof draft.visionDelegate !== 'string' || draft.visionDelegate.length === 0)) return tr('desktop_settings_00b87f3a56');
  if (draft.billingMode !== undefined && (typeof draft.billingMode !== 'string' || !['perToken', 'subscription', 'free', 'unknown'].includes(draft.billingMode))) return tr('desktop_settings_469964577d');
  if (draft.pricing !== undefined) {
    if (!record(draft.pricing)) return tr('desktop_settings_e99fa65871');
    for (const [model, prices] of Object.entries(draft.pricing)) {
      if (record(prices) && (!hasOwn(prices, 'inputPerMtok') || !hasOwn(prices, 'outputPerMtok'))) return tr('desktop_settings_cbf188f726');
      if (!draft.models.some(m => m.id === model) || !record(prices) || Object.entries(prices).some(([key, value]) => !['inputPerMtok', 'outputPerMtok', 'cacheWritePerMtok', 'cacheReadPerMtok', 'reasoningPerMtok'].includes(key) || !finitePrice(value))) return tr('desktop_settings_12cb209e26');
    }
  }
  if (draft.type === 'bedrock-claude' && !nonempty(draft.region)) return tr('desktop_settings_d05aa4b859');
  if (draft.type !== 'bedrock-claude' && !nonempty(draft.baseUrl)) return tr('desktop_settings_f51404fdc8');
  if (draft.baseUrl !== undefined) {
    if (!nonempty(draft.baseUrl)) return tr('desktop_settings_8aa7abc7da');
    try { const url = new URL(draft.baseUrl); if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash || /\{(?:env|file):/.test(draft.baseUrl)) return tr('desktop_settings_47430852db'); }
    catch { return tr('desktop_settings_d88f4b0b09'); }
  }
  if (draft.apiKeyEnv !== undefined && (typeof draft.apiKeyEnv !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(draft.apiKeyEnv))) return tr('desktop_settings_d69e84ee92');
  if (draft.type === 'azure-openai' && !nonempty(draft.apiVersion)) return tr('desktop_settings_0a1fcb84d0');
  if (draft.supportsWebsockets !== undefined && typeof draft.supportsWebsockets !== 'boolean') return tr('desktop_settings_2e39a79b9c');
  if (draft.supportsWebsockets && draft.type !== 'openai-responses') return tr('desktop_settings_207d871d1f');
  if (draft.supportsWebsocketCompression !== undefined && draft.supportsWebsocketCompression !== false) return tr('desktop_settings_79da4e1a4e');
  if (draft.websocketConnectTimeoutMs !== undefined && (!Number.isSafeInteger(draft.websocketConnectTimeoutMs) || Number(draft.websocketConnectTimeoutMs) < 0)) return tr('desktop_settings_5d383d8765');
  return null;
}
const finitePrice = (value: unknown): boolean => typeof value === 'number' && Number.isFinite(value) && value >= 0;
const integer = (value: unknown): boolean => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
function validMetadata(value: unknown): boolean {
  if (!record(value)) return false;
  for (const [key, v] of Object.entries(value)) {
    if (['family','status','releaseDate','lastUpdated','knowledgeCutoff'].includes(key) && v !== null && typeof v !== 'string') return false;
    if (['contextWindowTokens','maxInputTokens','maxOutputTokens'].includes(key) && v !== null && !integer(v)) return false;
    if (key === 'context' && v !== null && !validContextMetadata(v)) return false;
    if (key === 'applicationOutputTokens' && v !== null && (!integer(v) || Number(v) === 0 || Number(v) > 0xffffffff)) return false;
    if (['openWeights','attachments','temperatureControl'].includes(key) && v !== null && typeof v !== 'boolean') return false;
    if (['inputModalities','outputModalities'].includes(key) && (!Array.isArray(v) || v.some(item => typeof item !== 'string'))) return false;
    if (key === 'pricing' && v !== null && !validModelPricing(v)) return false;
  }
  return true;
}
function validModelPricing(value: unknown): boolean {
  if (!record(value)) return false;
  for (const [key, v] of Object.entries(value)) {
    if (['inputPerMillion','outputPerMillion','cacheReadPerMillion','cacheWritePerMillion','reasoningPerMillion'].includes(key) && v !== null && !finitePrice(v)) return false;
    if (key === 'billingMode' && (typeof v !== 'string' || !['perToken','subscription','free','unknown'].includes(v))) return false;
    if (key === 'source' && v !== null && typeof v !== 'string') return false;
    if (key === 'tiers' && (!Array.isArray(v) || v.some(tier => !record(tier) || !integer(tier.contextThresholdTokens) || !validModelPricing(tier)))) return false;
  }
  return true;
}
const providerFields = new Set(['type','baseUrl','apiKeyEnv','models','region','regions','apiVersion','supportsWebsockets','supportsWebsocketCompression','websocketConnectTimeoutMs','visionDelegate','pricing','connections','credentialIds','fallback','billingMode']);
const modelFields = new Set(['id','aliases','capabilities','metadata']);
const sdkTypes: Record<string, string> = { '@ai-sdk/openai-compatible': 'openai', '@ai-sdk/openai': 'openai-responses', '@ai-sdk/anthropic': 'anthropic', '@ai-sdk/google': 'gemini' };
const referenceFields = new Set(['apiKeyEnv', 'credentialIds']);
const secretField = /(?:api.?key|secret|password|authorization|credential|headers)|^(?:access|refresh|auth|bearer)?[_-]?token$/i;
const diagnosticFields = new Set([...providerFields, ...modelFields, 'apiKey', 'accessToken', 'refreshToken', 'authorization', 'headers', 'timeout', 'fetch', 'includeUsage', 'compatibility', 'name', 'cost', 'limit', 'modalities', 'release_date', 'attachment', 'reasoning', 'temperature', 'tool_call', 'knowledge', 'open_weights', 'status', 'variants', 'options', 'provider', 'npm', 'whitelist', 'blacklist', 'plugin', 'plugins', 'routing', 'model', 'small_model', 'instructions', 'tools', 'mcp', 'agent']);
const fieldLabel = (key: string): string => diagnosticFields.has(key) ? key : tr('desktop_settings_e90a969e23');
/** Advanced native JSON is retained except unsafe keys and credential-shaped properties. */
function safeValue(value: unknown, diagnostics: ImportDiagnostic[], depth = 0): unknown {
  if (depth > 32) { diagnostics.push({ severity: 'error', message: tr('desktop_settings_1f9121d617') }); return null; }
  if (Array.isArray(value)) return value.map(v => safeValue(v, diagnostics, depth + 1));
  if (!record(value)) return value;
  return Object.fromEntries(Object.entries(value).filter(([key]) => {
    // `apiKeyEnv` (an environment variable NAME) and `credentialIds` (keychain
    // ids) are references, never secrets. They are allowed at the top level by
    // `providerFields`, but this walker recurses, so a connection's own
    // `apiKeyEnv` was being stripped as if it were a pasted key. The pattern
    // itself is unchanged: every other credential-shaped key is still removed.
    if (forbidden.has(key) || (!referenceFields.has(key) && secretField.test(key))) { diagnostics.push({ severity: 'error', message: tr('desktop_settings_d720114573', fieldLabel(key)) }); return false; }
    return true;
  }).map(([key,v]) => [key, safeValue(v, diagnostics, depth + 1)]));
}
function readKey(value: unknown, entry: ProviderImportEntry): void {
  if (value === undefined || value === null || value === '') return;
  if (typeof value !== 'string') { entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_018380f837') }); return; }
  const env = /^\{env:([A-Za-z_][A-Za-z0-9_]*)\}$/.exec(value);
  if (env) { entry.draft.apiKeyEnv = env[1]; return; }
  if (/\{(?:env|file):/.test(value)) { entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_00aab73b47') }); return; }
  entry.apiKey = value;
}
function nativeDraft(raw: Record<string, unknown>, entry: ProviderImportEntry): void {
  const { diagnostics } = entry;
  for (const [key, value] of Object.entries(raw)) {
    if (key === 'apiKey') continue;
    if (!providerFields.has(key)) { diagnostics.push({ severity: secretField.test(key) || forbidden.has(key) ? 'error' : 'warning', message: tr('desktop_settings_b36487f7a4', fieldLabel(key)) }); continue; }
    if (key === 'models') {
      if (!Array.isArray(value)) { diagnostics.push({ severity: 'error', message: tr('desktop_settings_0cd669354a') }); continue; }
      entry.draft.models = value.map(model => {
        if (typeof model === 'string') return { id: model };
        if (!record(model)) { diagnostics.push({ severity: 'error', message: tr('desktop_settings_cab3bef8fa') }); return { id: '' }; }
        const result: Record<string, unknown> = {};
        for (const [field, data] of Object.entries(model)) {
          if (modelFields.has(field)) result[field] = safeValue(data, diagnostics);
          else diagnostics.push({ severity: 'warning', message: tr('desktop_settings_eb75ed21f8', fieldLabel(field)) });
        }
        return { ...result, id: typeof result.id === 'string' ? result.id : '' } as CustomProviderModelDraft;
      });
    } else entry.draft[key] = safeValue(value, diagnostics);
  }
  // Explicit key/reference wins over its env fallback, regardless of JSON member order.
  if (hasOwn(raw, 'apiKey')) {
    const fallback = entry.draft.apiKeyEnv;
    readKey(raw.apiKey, entry);
    if (fallback && entry.draft.apiKeyEnv !== fallback) diagnostics.push({ severity: 'warning', message: tr('desktop_settings_376783887e') });
  }
}
function openCodeDraft(raw: Record<string, unknown>, entry: ProviderImportEntry): void {
  entry.draft.type = typeof raw.npm === 'string' && hasOwn(sdkTypes, raw.npm) ? sdkTypes[raw.npm] : '';
  if (!entry.draft.type) entry.diagnostics.push({ severity: 'warning', message: tr('desktop_settings_6b335cb413') });
  const options = raw.options === undefined ? {} : raw.options;
  if (!record(options)) entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_6e28f89e0b') });
  else for (const [key, value] of Object.entries(options)) {
    if (key === 'baseURL') entry.draft.baseUrl = typeof value === 'string' ? value : '';
    else if (key === 'apiKey') readKey(value, entry);
    else entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_60b03bbd82', fieldLabel(key)) });
  }
  if (raw.models === undefined) entry.draft.models = [];
  else if (!record(raw.models)) entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_5b54290056') });
  else entry.draft.models = Object.entries(raw.models).map(([key, model]) => {
    if (!record(model)) { entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_c60a133a6f') }); return { id: '' }; }
    const id = model.id === undefined ? key : typeof model.id === 'string' ? model.id : '';
    for (const field of Object.keys(model)) if (field !== 'id') {
      const metadata = ['name','cost','limit','modalities','release_date','attachment','reasoning','temperature','tool_call','knowledge','open_weights','status'].includes(field);
      entry.diagnostics.push({ severity: metadata ? 'warning' : 'error', message: metadata ? tr('desktop_settings_a1070ce69f', fieldLabel(field)) : tr('desktop_settings_4ac15a7702', fieldLabel(field)) });
    }
    return id !== key ? { id, aliases: [key] } : { id };
  });
  for (const key of Object.keys(raw)) if (!['npm','options','models'].includes(key)) entry.diagnostics.push({ severity: ['name','whitelist','blacklist'].includes(key) ? 'warning' : 'error', message: tr('desktop_settings_bb65ecdc0c', fieldLabel(key)) });
}
export function parseProviderImport(text: string, existing: Record<string, CustomProviderDraft> = {}): ProviderImportResult {
  const result: ProviderImportResult = { entries: [], diagnostics: [] };
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { result.diagnostics.push({ severity: 'error', message: tr('desktop_settings_1f501a2d52') }); return result; }
  if (!record(parsed)) { result.diagnostics.push({ severity: 'error', message: tr('desktop_settings_45f67fcc43') }); return result; }
  if (hasOwn(parsed, 'provider') && hasOwn(parsed, 'providers')) { result.diagnostics.push({ severity: 'error', message: tr('desktop_settings_c403b52fbb') }); return result; }
  const opencode = hasOwn(parsed, 'provider');
  const wrapped = opencode || hasOwn(parsed, 'providers');
  const map = opencode ? parsed.provider : wrapped ? parsed.providers : parsed;
  if (!record(map) || !Object.keys(map).length) { result.diagnostics.push({ severity: 'error', message: tr('desktop_settings_69367391b0') }); return result; }
  if (wrapped) for (const key of Object.keys(parsed)) {
    if (!['provider','providers','$schema'].includes(key)) result.diagnostics.push({ severity: 'warning', message: tr('desktop_settings_2f17473a57', fieldLabel(key)) });
  }
  for (const [name, raw] of Object.entries(map)) {
    const conflict = hasOwn(existing, name);
    const entry: ProviderImportEntry = { name, draft: { type: '', models: [] }, selected: !conflict, conflict, diagnostics: [] };
    if (!record(raw)) entry.diagnostics.push({ severity: 'error', message: tr('desktop_settings_534e0531e5') });
    else if (opencode) openCodeDraft(raw, entry);
    else nativeDraft(raw, entry);
    result.entries.push(entry);
  }
  return result;
}
export function validateImportEntry(entry: ProviderImportEntry, options: { credentialConfigured?: boolean } = {}): string | null {
  const issue = validateProfileName(entry.name) ?? entry.diagnostics.find(d => d.severity === 'error')?.message ?? validateCustomProvider(trimProviderDraft(entry.draft));
  if (issue) return issue;
  if (entry.draft.type !== 'bedrock-claude' && !nonempty(entry.apiKey) && !nonempty(entry.draft.apiKeyEnv) && !connectionsCarryAuth(entry.draft) && !options.credentialConfigured) return tr('desktop_settings_fc019e5232');
  return null;
}
export function mergeProviderImport(current: Record<string, CustomProviderDraft>, entries: ProviderImportEntry[], options: { credentialConfigured?: (name: string) => boolean } = {}): { providers: Record<string, CustomProviderDraft>; credentials: Record<string, string> } {
  const selected = entries.filter(entry => entry.selected);
  if (!selected.length) throw new Error(tr('desktop_settings_9d9db82aa8'));
  const names = new Set<string>();
  for (const entry of selected) {
    const error = validateImportEntry(entry, { credentialConfigured: options.credentialConfigured?.(entry.name) });
    if (error) throw new Error(error);
    if (names.has(entry.name)) throw new Error(tr('desktop_settings_6d23eca32d'));
    names.add(entry.name);
  }
  const providers = { ...current };
  const credentials: Record<string, string> = {};
  for (const entry of selected) {
    // Re-sanitize edited drafts at the persistence boundary as well.
    const sanitized: ProviderImportEntry = { ...entry, draft: { type: '', models: [] }, diagnostics: [] };
    nativeDraft(trimProviderDraft(entry.draft), sanitized);
    if (sanitized.diagnostics.some(d => d.severity === 'error')) throw new Error(tr('desktop_settings_d725aafe25'));
    providers[entry.name] = sanitized.draft;
    if (nonempty(entry.apiKey)) credentials[entry.name] = entry.apiKey.trim();
  }
  return { providers, credentials };
}

function validContextMetadata(value: unknown): boolean {
  if (!record(value)) return false;
  if (Object.keys(value).some(key => !['sharedWindow', 'thinkingMaxInputTokens', 'recommendation', 'application'].includes(key))) return false;
  if (value.sharedWindow != null && typeof value.sharedWindow !== 'boolean') return false;
  if (value.thinkingMaxInputTokens != null && (!integer(value.thinkingMaxInputTokens) || value.thinkingMaxInputTokens === 0)) return false;
  if (value.recommendation != null) {
    const r = value.recommendation;
    if (!record(r) || !integer(r.inputTokens) || r.inputTokens === 0 || !nonempty(r.source) || !nonempty(r.verifiedAt)) return false;
    if (Object.keys(r).some(key => !['inputTokens', 'source', 'verifiedAt'].includes(key))) return false;
  }
  if (value.application != null) {
    const p = value.application;
    if (!record(p) || !['auto', 'long', 'custom'].includes(String(p.mode))) return false;
    if (Object.keys(p).some(key => !['mode', ...(p.mode === 'custom' ? ['input_tokens'] : [])].includes(key))) return false;
    if (p.mode === 'custom' && (!integer(p.input_tokens) || p.input_tokens === 0)) return false;
  }
  return true;
}
