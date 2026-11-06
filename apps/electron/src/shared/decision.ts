/** Decision host contract. Provider-native formats stay behind the Rust service. */
export const CH_DECISION = 'lingxi:decision';
export const CH_DECISION_CHANGED = 'lingxi:decision:changed';
export type DecisionProtocol = 'openai_decisions' | 'typesafe_systemone';
export interface DecisionConfiguration {
  name?: string;
  source: 'openai' | 'jev' | 'custom';
  protocol: DecisionProtocol;
  endpoint: string;
  model: string;
  accountId?: string;
}
export interface DecisionSnapshot { revision: string; configuration: DecisionConfiguration | null; credentialConfigured: boolean }
export type { DecisionQuestion } from './generatedDecision.js';
export type DecisionResult = import('./generatedDecision.js').DecisionResponse;
export interface DecisionInput { images?: import('./generatedDecision.js').DecisionImage[]; state: unknown; questions: Record<string, import('./generatedDecision.js').DecisionQuestion> }
export type DecisionCommand = { action: 'get' } | { action: 'save'; revision: string; configuration: DecisionConfiguration | null; credential?: string } | { action: 'evaluate'; operationId: string; input: DecisionInput } | { action: 'cancel'; operationId: string };
export interface DecisionApi {
  snapshot(): Promise<DecisionSnapshot>;
  save(revision: string, configuration: DecisionConfiguration | null, credential?: string): Promise<DecisionSnapshot>;
  evaluate(operationId: string, input: DecisionInput): Promise<DecisionResult>;
  cancel(operationId: string): Promise<void>;
  onChanged(listener: (value: DecisionSnapshot) => void): () => void;
}
export function parseDecisionConfiguration(value: unknown): DecisionConfiguration | null {
  if (value === null) return null;
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Decision configuration');
  const v = value as Record<string, unknown>;
  if (!['openai', 'jev', 'custom'].includes(String(v.source)) || !['openai_decisions', 'typesafe_systemone'].includes(String(v.protocol))
      || typeof v.endpoint !== 'string' || v.endpoint.length > 2048 || typeof v.model !== 'string' || !v.model.trim() || v.model.length > 256
      || (v.accountId !== undefined && (typeof v.accountId !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(v.accountId)))) throw new Error('Invalid Decision configuration');
  if (v.accountId && v.source !== 'openai') throw new Error('Saved provider accounts can only be used with the OpenAI service');
  if (v.name !== undefined && (typeof v.name !== 'string' || v.name.length > 128 || /[\u0000-\u001f]/.test(v.name))) throw new Error('Invalid Decision service name');
  const url = new URL(v.endpoint);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Use an HTTP(S) resource URL without credentials, query or fragment');
  if (v.source === 'openai' && (url.href !== 'https://api.openai.com/v1/decisions' || v.protocol !== 'openai_decisions')) throw new Error('Invalid OpenAI Decisions endpoint');
  if (v.source === 'jev' && (url.href !== 'https://api.typesafe.ai/v1/systemone' || v.protocol !== 'typesafe_systemone')) throw new Error('Invalid Jev endpoint');
  return { source: v.source as DecisionConfiguration['source'], protocol: v.protocol as DecisionProtocol, endpoint: url.href, model: v.model.trim(), ...(v.accountId ? { accountId: v.accountId as string } : {}), ...(typeof v.name === 'string' && v.name.trim() ? { name: v.name.trim() } : {}) };
}
