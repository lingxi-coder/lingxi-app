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
  if (reservedProfileIds.has(name)) return 'builtin 和 claude 为内置路由保留名称，请使用其他 Profile ID。';
  return typeof name === 'string' && /^[a-z0-9][a-z0-9._-]{0,63}$/.test(name) && !forbidden.has(name)
    ? null : 'Profile ID 需为 1–64 位小写字母、数字、点、下划线或连字符，并以字母或数字开头。';
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
  if (!Array.isArray(value) || !value.length) return `${label}credentialIds 必须是非空数组。`;
  if (value.some((key) => !nonempty(key))) return `${label}credentialIds 的每一项必须是非空字符串。`;
  if (new Set(value.map((key) => (key as string).trim())).size !== value.length) return `${label}credentialIds 不能重复。`;
  return null;
}

/**
 * A provider reachable several ways: validate each connection as if it were the
 * flat provider it desugars to, so one endpoint cannot pass rules another fails.
 */
function validateConnections(draft: CustomProviderDraft): string | null {
  const connections = draft.connections;
  if (!Array.isArray(connections) || !connections.length) return 'connections 必须是非空数组；单一连接请直接省略该字段。';
  const seen = new Set<string>();
  for (const [index, connection] of connections.entries()) {
    if (!record(connection)) return `connections[${index}] 必须是对象。`;
    const id = connection.id;
    if (!nonempty(id)) return `connections[${index}] 需要非空的 id。`;
    const trimmed = (id as string).trim();
    // The id becomes part of a qualified model reference (`provider:conn/model`),
    // so a separator in it would produce a reference that cannot be routed.
    if (/[/:#]/.test(trimmed)) return `连接 id ${JSON.stringify(trimmed)} 不能包含 '/'、':' 或 '#'。`;
    if (seen.has(trimmed)) return `连接 id ${JSON.stringify(trimmed)} 重复。`;
    seen.add(trimmed);
    const keysError = validateCredentialIds(connection.credentialIds, `连接 ${JSON.stringify(trimmed)} 的 `);
    if (keysError) return keysError;
    const merged = mergeConnection(draft, connection as CustomProviderConnectionDraft);
    const error = validateCustomProvider(merged);
    if (error) return `连接 ${JSON.stringify(trimmed)}：${error}`;
  }
  if (draft.fallback !== undefined) {
    if (!record(draft.fallback)) return 'fallback 必须为对象。';
    const on = (draft.fallback as Record<string, unknown>).on;
    if (on !== undefined) {
      if (!Array.isArray(on)) return 'fallback.on 必须是数组。';
      for (const trigger of on) {
        if (!nonempty(trigger) || !FALLBACK_TRIGGERS.includes((trigger as string).trim())) return `fallback.on 取值无效，支持：${FALLBACK_TRIGGERS.join(', ')}。`;
      }
    }
  }
  return null;
}

export function validateCustomProvider(draft: CustomProviderDraft): string | null {
  // A provider with `connections` is validated CONNECTION BY CONNECTION below;
  // the provider entry itself only supplies defaults, so it need not satisfy
  // baseUrl/models on its own.
  if (record(draft) && draft.connections !== undefined) return validateConnections(draft);
  const keysError = validateCredentialIds(draft.credentialIds, '');
  if (keysError) return keysError;
  if (!record(draft) || !SUPPORTED_PROVIDER_TYPES.includes(draft.type as SupportedProviderType)) return `请选择支持的协议：${SUPPORTED_PROVIDER_TYPES.join(', ')}`;
  if (!Array.isArray(draft.models) || !draft.models.length) return 'models 至少需要一个模型。';
  if (draft.models.some(m => !record(m) || !nonempty(m.id))) return '每个 models 条目必须有非空 id。';
  if (new Set(draft.models.map(m => m.id.trim())).size !== draft.models.length) return '模型 id 不能重复。';
  if (draft.models.some(m => m.aliases !== undefined && (!Array.isArray(m.aliases) || m.aliases.some(a => !nonempty(a))))) return '模型 aliases 必须为非空字符串数组。';
  for (const model of draft.models) {
    if (model.capabilities !== undefined && (!record(model.capabilities) || Object.values(model.capabilities).some(v => typeof v !== 'boolean'))) return '模型 capabilities 必须为布尔值对象。';
    if (model.metadata !== undefined && !validMetadata(model.metadata)) return '模型 metadata 格式无效。';
  }
  if (draft.visionDelegate !== undefined && (typeof draft.visionDelegate !== 'string' || draft.visionDelegate.length === 0)) return 'visionDelegate 必须为非空字符串。';
  if (draft.billingMode !== undefined && (typeof draft.billingMode !== 'string' || !['perToken', 'subscription', 'free', 'unknown'].includes(draft.billingMode))) return 'billingMode 无效。';
  if (draft.pricing !== undefined) {
    if (!record(draft.pricing)) return 'pricing 必须为对象。';
    for (const [model, prices] of Object.entries(draft.pricing)) {
      if (record(prices) && (!hasOwn(prices, 'inputPerMtok') || !hasOwn(prices, 'outputPerMtok'))) return 'pricing 每个模型必须同时填写 inputPerMtok 和 outputPerMtok。';
      if (!draft.models.some(m => m.id === model) || !record(prices) || Object.entries(prices).some(([key, value]) => !['inputPerMtok', 'outputPerMtok', 'cacheWritePerMtok', 'cacheReadPerMtok', 'reasoningPerMtok'].includes(key) || !finitePrice(value))) return 'pricing 需要已配置的模型及有效的非负价格。';
    }
  }
  if (draft.type === 'bedrock-claude' && !nonempty(draft.region)) return 'Bedrock 需要 region。';
  if (draft.type !== 'bedrock-claude' && !nonempty(draft.baseUrl)) return '请填写 baseUrl。';
  if (draft.baseUrl !== undefined) {
    if (!nonempty(draft.baseUrl)) return 'baseUrl 必须是 HTTP(S) 地址。';
    try { const url = new URL(draft.baseUrl); if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash || /\{(?:env|file):/.test(draft.baseUrl)) return 'baseUrl 必须是无内嵌凭据的 HTTP(S) 地址。'; }
    catch { return 'baseUrl 必须是有效的 HTTP(S) 地址。'; }
  }
  if (draft.apiKeyEnv !== undefined && (typeof draft.apiKeyEnv !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(draft.apiKeyEnv))) return 'apiKeyEnv 必须是有效的环境变量名称。';
  if (draft.type === 'azure-openai' && !nonempty(draft.apiVersion)) return 'Azure OpenAI 需要 apiVersion。';
  if (draft.supportsWebsockets !== undefined && typeof draft.supportsWebsockets !== 'boolean') return 'supportsWebsockets 必须为布尔值。';
  if (draft.supportsWebsockets && draft.type !== 'openai-responses') return 'WebSocket 仅适用于 openai-responses。';
  if (draft.supportsWebsocketCompression !== undefined && draft.supportsWebsocketCompression !== false) return '当前不支持 WebSocket 压缩。';
  if (draft.websocketConnectTimeoutMs !== undefined && (!Number.isSafeInteger(draft.websocketConnectTimeoutMs) || Number(draft.websocketConnectTimeoutMs) < 0)) return 'websocketConnectTimeoutMs 必须为非负整数。';
  return null;
}
const finitePrice = (value: unknown): boolean => typeof value === 'number' && Number.isFinite(value) && value >= 0;
const integer = (value: unknown): boolean => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
function validMetadata(value: unknown): boolean {
  if (!record(value)) return false;
  for (const [key, v] of Object.entries(value)) {
    if (['family','status','releaseDate','lastUpdated','knowledgeCutoff'].includes(key) && v !== null && typeof v !== 'string') return false;
    if (['contextWindowTokens','maxInputTokens','maxOutputTokens'].includes(key) && v !== null && !integer(v)) return false;
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
const providerFields = new Set(['type','baseUrl','apiKeyEnv','models','region','apiVersion','supportsWebsockets','supportsWebsocketCompression','websocketConnectTimeoutMs','visionDelegate','pricing','billingMode']);
const modelFields = new Set(['id','aliases','capabilities','metadata']);
const sdkTypes: Record<string, string> = { '@ai-sdk/openai-compatible': 'openai', '@ai-sdk/openai': 'openai-responses', '@ai-sdk/anthropic': 'anthropic', '@ai-sdk/google': 'gemini' };
const secretField = /(?:api.?key|secret|password|authorization|credential|headers)|^(?:access|refresh|auth|bearer)?[_-]?token$/i;
const diagnosticFields = new Set([...providerFields, ...modelFields, 'apiKey', 'accessToken', 'refreshToken', 'authorization', 'headers', 'timeout', 'fetch', 'includeUsage', 'compatibility', 'name', 'cost', 'limit', 'modalities', 'release_date', 'attachment', 'reasoning', 'temperature', 'tool_call', 'knowledge', 'open_weights', 'status', 'variants', 'options', 'provider', 'npm', 'whitelist', 'blacklist', 'plugin', 'plugins', 'routing', 'model', 'small_model', 'instructions', 'tools', 'mcp', 'agent']);
const fieldLabel = (key: string): string => diagnosticFields.has(key) ? key : '（未知字段名已隐藏）';
/** Advanced native JSON is retained except unsafe keys and credential-shaped properties. */
function safeValue(value: unknown, diagnostics: ImportDiagnostic[], depth = 0): unknown {
  if (depth > 32) { diagnostics.push({ severity: 'error', message: 'JSON 嵌套过深，请简化高级配置。' }); return null; }
  if (Array.isArray(value)) return value.map(v => safeValue(v, diagnostics, depth + 1));
  if (!record(value)) return value;
  return Object.fromEntries(Object.entries(value).filter(([key]) => {
    if (forbidden.has(key) || secretField.test(key)) { diagnostics.push({ severity: 'error', message: `高级字段 ${fieldLabel(key)} 包含凭据或不安全属性，已移除；请修改原始 JSON。` }); return false; }
    return true;
  }).map(([key,v]) => [key, safeValue(v, diagnostics, depth + 1)]));
}
function readKey(value: unknown, entry: ProviderImportEntry): void {
  if (value === undefined || value === null || value === '') return;
  if (typeof value !== 'string') { entry.diagnostics.push({ severity: 'error', message: 'apiKey 必须为字符串。' }); return; }
  const env = /^\{env:([A-Za-z_][A-Za-z0-9_]*)\}$/.exec(value);
  if (env) { entry.draft.apiKeyEnv = env[1]; return; }
  if (/\{(?:env|file):/.test(value)) { entry.diagnostics.push({ severity: 'error', message: '无法导入此凭据引用；请在原始 JSON 中移除引用，再填写 API Key 或环境变量。' }); return; }
  entry.apiKey = value;
}
function nativeDraft(raw: Record<string, unknown>, entry: ProviderImportEntry): void {
  const { diagnostics } = entry;
  for (const [key, value] of Object.entries(raw)) {
    if (key === 'apiKey') continue;
    if (!providerFields.has(key)) { diagnostics.push({ severity: secretField.test(key) || forbidden.has(key) ? 'error' : 'warning', message: `未导入 Provider 字段 ${fieldLabel(key)}（值已隐藏）；请核对原始 JSON。` }); continue; }
    if (key === 'models') {
      if (!Array.isArray(value)) { diagnostics.push({ severity: 'error', message: 'LingXi models 必须为数组。' }); continue; }
      entry.draft.models = value.map(model => {
        if (typeof model === 'string') return { id: model };
        if (!record(model)) { diagnostics.push({ severity: 'error', message: 'models 条目必须为字符串或对象。' }); return { id: '' }; }
        const result: Record<string, unknown> = {};
        for (const [field, data] of Object.entries(model)) {
          if (modelFields.has(field)) result[field] = safeValue(data, diagnostics);
          else diagnostics.push({ severity: 'warning', message: `未导入模型字段 ${fieldLabel(field)}（值已隐藏）。` });
        }
        return { ...result, id: typeof result.id === 'string' ? result.id : '' } as CustomProviderModelDraft;
      });
    } else entry.draft[key] = safeValue(value, diagnostics);
  }
  // Explicit key/reference wins over its env fallback, regardless of JSON member order.
  if (hasOwn(raw, 'apiKey')) {
    const fallback = entry.draft.apiKeyEnv;
    readKey(raw.apiKey, entry);
    if (fallback && entry.draft.apiKeyEnv !== fallback) diagnostics.push({ severity: 'warning', message: 'apiKey 的环境变量引用优先于 apiKeyEnv。' });
  }
}
function openCodeDraft(raw: Record<string, unknown>, entry: ProviderImportEntry): void {
  entry.draft.type = typeof raw.npm === 'string' && hasOwn(sdkTypes, raw.npm) ? sdkTypes[raw.npm] : '';
  if (!entry.draft.type) entry.diagnostics.push({ severity: 'warning', message: '无法识别 SDK，请手动选择协议。' });
  const options = raw.options === undefined ? {} : raw.options;
  if (!record(options)) entry.diagnostics.push({ severity: 'error', message: 'OpenCode options 必须为对象。' });
  else for (const [key, value] of Object.entries(options)) {
    if (key === 'baseURL') entry.draft.baseUrl = typeof value === 'string' ? value : '';
    else if (key === 'apiKey') readKey(value, entry);
    else entry.diagnostics.push({ severity: 'error', message: `无法转换请求选项 ${fieldLabel(key)}；请修改原始 JSON 或取消选择此项。` });
  }
  if (raw.models === undefined) entry.draft.models = [];
  else if (!record(raw.models)) entry.diagnostics.push({ severity: 'error', message: 'OpenCode models 必须为对象。' });
  else entry.draft.models = Object.entries(raw.models).map(([key, model]) => {
    if (!record(model)) { entry.diagnostics.push({ severity: 'error', message: 'OpenCode 模型条目必须为对象。' }); return { id: '' }; }
    const id = model.id === undefined ? key : typeof model.id === 'string' ? model.id : '';
    for (const field of Object.keys(model)) if (field !== 'id') {
      const metadata = ['name','cost','limit','modalities','release_date','attachment','reasoning','temperature','tool_call','knowledge','open_weights','status'].includes(field);
      entry.diagnostics.push({ severity: metadata ? 'warning' : 'error', message: metadata ? `OpenCode 模型字段 ${fieldLabel(field)} 未转换，请核对模型能力。` : `无法转换模型请求字段 ${fieldLabel(field)}；请修改原始 JSON 或取消选择此项。` });
    }
    return id !== key ? { id, aliases: [key] } : { id };
  });
  for (const key of Object.keys(raw)) if (!['npm','options','models'].includes(key)) entry.diagnostics.push({ severity: ['name','whitelist','blacklist'].includes(key) ? 'warning' : 'error', message: `未导入 OpenCode Provider 字段 ${fieldLabel(key)}；请核对原始 JSON。` });
}
export function parseProviderImport(text: string, existing: Record<string, CustomProviderDraft> = {}): ProviderImportResult {
  const result: ProviderImportResult = { entries: [], diagnostics: [] };
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { result.diagnostics.push({ severity: 'error', message: 'JSON 无效，请检查引号、逗号与括号；仅支持标准 JSON。' }); return result; }
  if (!record(parsed)) { result.diagnostics.push({ severity: 'error', message: 'JSON 顶层必须为对象。' }); return result; }
  if (hasOwn(parsed, 'provider') && hasOwn(parsed, 'providers')) { result.diagnostics.push({ severity: 'error', message: '请仅提供 provider 或 providers 中的一种格式。' }); return result; }
  const opencode = hasOwn(parsed, 'provider');
  const wrapped = opencode || hasOwn(parsed, 'providers');
  const map = opencode ? parsed.provider : wrapped ? parsed.providers : parsed;
  if (!record(map) || !Object.keys(map).length) { result.diagnostics.push({ severity: 'error', message: '未找到 Provider 对象。' }); return result; }
  if (wrapped) for (const key of Object.keys(parsed)) {
    if (!['provider','providers','$schema'].includes(key)) result.diagnostics.push({ severity: 'warning', message: `顶层字段 ${fieldLabel(key)} 不会执行或导入；仅导入 Provider。` });
  }
  for (const [name, raw] of Object.entries(map)) {
    const conflict = hasOwn(existing, name);
    const entry: ProviderImportEntry = { name, draft: { type: '', models: [] }, selected: !conflict, conflict, diagnostics: [] };
    if (!record(raw)) entry.diagnostics.push({ severity: 'error', message: 'Provider 必须为对象。' });
    else if (opencode) openCodeDraft(raw, entry);
    else nativeDraft(raw, entry);
    result.entries.push(entry);
  }
  return result;
}
export function validateImportEntry(entry: ProviderImportEntry, options: { credentialConfigured?: boolean } = {}): string | null {
  const issue = validateProfileName(entry.name) ?? entry.diagnostics.find(d => d.severity === 'error')?.message ?? validateCustomProvider(trimProviderDraft(entry.draft));
  if (issue) return issue;
  if (entry.draft.type !== 'bedrock-claude' && !nonempty(entry.apiKey) && !nonempty(entry.draft.apiKeyEnv) && !options.credentialConfigured) return '请填写 API Key 或环境变量名称。';
  return null;
}
export function mergeProviderImport(current: Record<string, CustomProviderDraft>, entries: ProviderImportEntry[], options: { credentialConfigured?: (name: string) => boolean } = {}): { providers: Record<string, CustomProviderDraft>; credentials: Record<string, string> } {
  const selected = entries.filter(entry => entry.selected);
  if (!selected.length) throw new Error('请至少选择一个 Provider。');
  const names = new Set<string>();
  for (const entry of selected) {
    const error = validateImportEntry(entry, { credentialConfigured: options.credentialConfigured?.(entry.name) });
    if (error) throw new Error(error);
    if (names.has(entry.name)) throw new Error('导入 Profile ID 重复。');
    names.add(entry.name);
  }
  const providers = { ...current };
  const credentials: Record<string, string> = {};
  for (const entry of selected) {
    // Re-sanitize edited drafts at the persistence boundary as well.
    const sanitized: ProviderImportEntry = { ...entry, draft: { type: '', models: [] }, diagnostics: [] };
    nativeDraft(trimProviderDraft(entry.draft), sanitized);
    if (sanitized.diagnostics.some(d => d.severity === 'error')) throw new Error('配置含不安全或无效字段。');
    providers[entry.name] = sanitized.draft;
    if (nonempty(entry.apiKey)) credentials[entry.name] = entry.apiKey.trim();
  }
  return { providers, credentials };
}
