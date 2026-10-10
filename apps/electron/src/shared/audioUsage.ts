/** Provider-reported audio usage; absent fields remain unknown and no chat pricing is applied. */
export interface AudioReportedUsage {
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  audioSeconds?: number;
  characters?: number;
  costUsd?: number;
}
export interface AudioUsageRecord {
  sequence: number;
  operationId: string;
  configurationRevision: number;
  kind: 'recognition' | 'speech' | 'realtime';
  profileId: string;
  providerId: string;
  accountScope?: string;
  sessionId?: string;
  modelId: string | null;
  turnId?: string;
  usage: AudioReportedUsage;
}
export function normalizeAudioUsage(value: unknown): AudioReportedUsage {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const raw = value as Record<string, unknown>;
  const fields = [['inputTokens', 'input_tokens'], ['outputTokens', 'output_tokens'], ['totalTokens', 'total_tokens'], ['audioSeconds', 'audio_seconds'], ['characters', 'characters'], ['costUsd', 'cost_usd']] as const;
  const result: AudioReportedUsage = {};
  for (const [field, native] of fields) {
    const numeric = raw[field] ?? raw[native];
    if (typeof numeric === 'number' && Number.isFinite(numeric) && numeric >= 0 && numeric <= Number.MAX_SAFE_INTEGER) result[field] = numeric;
  }
  return result;
}

export interface AudioUsageContext {
  operationId: string;
  profileId: string;
  providerId: string;
  accountScope?: string;
  modelId: string | null;
}
/** Only the native provider host can attest the executed model and account. */
export function parseAudioUsageContext(value: unknown): AudioUsageContext | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const raw = value as Record<string, unknown>;
  for (const field of ['operationId', 'profileId', 'providerId'] as const) {
    if (typeof raw[field] !== 'string' || !raw[field].trim() || raw[field].length > 512) return undefined;
  }
  if (raw['modelId'] !== undefined && raw['modelId'] !== null && (typeof raw['modelId'] !== 'string' || raw['modelId'].length > 512)) return undefined;
  if (raw['accountScope'] !== undefined && (typeof raw['accountScope'] !== 'string' || raw['accountScope'].length > 512)) return undefined;
  return { operationId: raw['operationId'] as string, profileId: raw['profileId'] as string, providerId: raw['providerId'] as string,
    modelId: typeof raw['modelId'] === 'string' && raw['modelId'].trim() ? raw['modelId'] : null, ...(typeof raw['accountScope'] === 'string' ? { accountScope: raw['accountScope'] } : {}) };
}
