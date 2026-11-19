/**
 * The model-provider vault as clients see it (`apps/firebase/src/llm-api.mjs`). Shared by the Web and Desktop.
 * A provider record never carries its key, except in a native export.
 */
export type VaultProtocol = 'open_ai_chat' | 'open_ai_responses' | 'anthropic_messages' | 'gemini_generate_content' | 'other';
export const WEB_PROTOCOLS: readonly VaultProtocol[] = ['open_ai_chat', 'open_ai_responses', 'anthropic_messages', 'gemini_generate_content'];
export type VaultSource = 'web' | 'desktop' | 'android' | 'ios' | 'unknown';
/** The Desktop custom-provider types (`customProviderImport.ts`). */
export type DesktopProviderType = 'openai' | 'openai-responses' | 'anthropic' | 'gemini' | 'azure-openai'
  | 'bedrock-claude' | 'vertex-claude' | 'vertex-gemini' | 'foundry-claude';

export interface VaultModel { id: string; name?: string }
export interface VaultProvider {
  id: string;
  presetId: string | null;
  label: string;
  protocol: VaultProtocol;
  desktopType: DesktopProviderType | null;
  baseUrl: string;
  models: VaultModel[];
  defaultModel: string | null;
  keyHint: string;
  keyFingerprint: string;
  source: VaultSource;
  revision: number;
  /** Whether the Web proxy can call it; Desktop-only kinds (Azure, Bedrock, Vertex, Foundry) are stored but not proxied. */
  webUsable: boolean;
  createdAt: number | null;
  updatedAt: number | null;
}
export interface VaultExport extends VaultProvider { apiKey: string }

export interface VaultUpsert {
  action: 'upsert';
  id: string;
  presetId?: string;
  label?: string;
  protocol?: VaultProtocol;
  desktopType?: DesktopProviderType;
  baseUrl?: string;
  models?: VaultModel[];
  defaultModel?: string;
  /** Omitted on a change that keeps the stored key. */
  apiKey?: string;
  expectedRevision?: number;
}

export interface ChatTurnMessage { role: 'user' | 'assistant'; content: string }
export interface ChatTurn { providerId: string; model: string; system?: string; messages: ChatTurnMessage[] }

/** One event of a streamed turn. */
export type ChatEvent =
  | { type: 'start'; provider: string; model: string }
  | { type: 'delta'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'usage'; input: number | null; output: number | null }
  | { type: 'done'; reason: string }
  | { type: 'error'; code: string };

export interface ProviderTestResult { ok: boolean; code?: string; model?: string }

/** Desktop's provider id rule (`validateProfileName`), which vault ids share so one provider has one id everywhere. */
export const PROVIDER_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/u;
const RESERVED_IDS = new Set(['builtin', 'claude', '__proto__', 'prototype', 'constructor']);
export const isProviderId = (value: string) => PROVIDER_ID.test(value) && !RESERVED_IDS.has(value);

/** The vault protocol a Desktop custom provider speaks through the proxy, or `other` when only Desktop can call it. */
export function protocolOfDesktopType(type: DesktopProviderType): VaultProtocol {
  switch (type) {
    case 'openai': return 'open_ai_chat';
    case 'openai-responses': return 'open_ai_responses';
    case 'anthropic': return 'anthropic_messages';
    case 'gemini': return 'gemini_generate_content';
    default: return 'other';
  }
}
/** The Desktop custom-provider type for a vault protocol, for a provider first saved on the Web. */
export function desktopTypeOfProtocol(protocol: VaultProtocol): DesktopProviderType | null {
  switch (protocol) {
    case 'open_ai_chat': return 'openai';
    case 'open_ai_responses': return 'openai-responses';
    case 'anthropic_messages': return 'anthropic';
    case 'gemini_generate_content': return 'gemini';
    default: return null;
  }
}
