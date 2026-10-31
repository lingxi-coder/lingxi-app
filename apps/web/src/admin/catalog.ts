import type { User } from 'firebase/auth';
import { auth, catalogApiOrigin } from './firebase';

export type Platform = 'desktop' | 'android' | 'ios' | 'cli';
export interface Provider {
  id: string; name: string; description: string; baseUrl: string; protocol: string;
  enabled: boolean; order: number; regions: string[];
  defaultModelId: string; requiredCapabilities: string[];
  executionDefinition: { format: 'llm-client-preset-toml'; source: string; sha256: string };
}
export interface Model {
  providerId: string; id: string; name: string; description: string; status: 'active' | 'deprecated' | 'disabled'; requiredCapabilities: string[]; order: number;
  platforms: Platform[]; minAppVersion: string; maxAppVersion: string | null;
  contextWindow: number; maxOutputTokens: number;
  capabilities: { tools: boolean; vision: boolean; reasoning: boolean; audio: boolean };
  inputModalities: string[]; outputModalities: string[];
  pricing: { currency: 'USD' | 'CNY'; inputPerMillion: number; outputPerMillion: number; cacheReadPerMillion?: number; cacheWritePerMillion?: number } | null;
}
export interface Catalog { schemaVersion: 2; providers: Provider[]; models: Model[] }
export interface Draft { revision: number; updatedAt: number | null; catalog: Catalog }
export interface Release { id: string; label: string; createdAt: number; revision: number }
export async function catalogRequest<T>(user: User, path: string, body?: object, method = 'POST', signal?: AbortSignal): Promise<T> {
  if (auth.currentUser?.uid !== user.uid) throw new Error('Session changed. Sign in again.');
  const token = await user.getIdToken();
  const response = await fetch(`${catalogApiOrigin}${path}`, {
    method: body ? method : 'GET', signal,
    headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const data = await response.json();
  if (auth.currentUser?.uid !== user.uid) throw new Error('Session changed. Sign in again.');
  if (!response.ok) throw new Error(data.error?.message || data.error?.code || `HTTP ${response.status}`);
  return data as T;
}
