import type { ImageRefDto } from '@lingxi/bridge-client';

/** This channel carries app-scoped events; it never exposes chat or approval APIs. */
export const CH_LOCAL_APP_AGENT_EVENT = 'lingxi:local-app:agent:event';
export const LOCAL_APP_AGENT_DEFAULT_MAX_TURNS = 20;
export const LOCAL_APP_AGENT_DEFAULT_TIMEOUT_MS = 600_000;

export interface LocalAppAgentRunInput {
  requestId: string;
  prompt: string;
  images?: ImageRefDto[];
  outputSchema?: Record<string, unknown>;
  /** Opaque IDs from the host's tool catalog, already declared and granted. */
  tools?: string[];
  maxTurns?: number;
  timeoutMs?: number;
}
export interface LocalAppAgentModel {
  providerId: string;
  modelId: string;
}
export interface LocalAppAgentModelChoice extends LocalAppAgentModel {
  label: string;
  canSelect: boolean;
  supportsImages: boolean;
  /** This release produces structured results through a Harness tool; derived from model tool support. */
  supportsStructuredOutput: boolean;
}
export type LocalAppAgentToolGrant =
  | { id: string; kind: 'builtin'; name: string }
  | { id: string; kind: 'mcp'; serverConfigId: string; configRevision: string; rawToolName: string; definitionDigest: string };
export type LocalAppAgentToolChoice = LocalAppAgentToolGrant & { label: string; description?: string };
export interface LocalAppAgentCatalog {
  models: LocalAppAgentModelChoice[];
  tools: LocalAppAgentToolChoice[];
}
/** Only the trusted host library UI may modify these settings. */
export interface LocalAppAgentSettings {
  enabled: boolean;
  model: LocalAppAgentModel;
  tools: LocalAppAgentToolGrant[];
  background: boolean;
  maxTurns: number;
  timeoutMs: number;
}
export type LocalAppAgentRunState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';
export interface LocalAppAgentUsage {
  inputTokens?: number;
  outputTokens?: number;
  costUsd?: number;
  turns?: number;
}
export interface LocalAppAgentResult {
  text?: string;
  /** Present only after the executor validates the requested JSON Schema. */
  output?: unknown;
}
export interface LocalAppAgentRun {
  runId: string;
  requestId: string;
  state: LocalAppAgentRunState;
  sequence: number;
  createdAt: number;
  updatedAt: number;
  model: LocalAppAgentModel;
  limits: { maxTurns: number; timeoutMs: number };
  result?: LocalAppAgentResult;
  usage?: LocalAppAgentUsage;
  error?: { code: string; message: string };
}
export type LocalAppAgentProgressPhase = 'starting' | 'thinking' | 'tool' | 'waiting_for_permission' | 'waiting_for_answer';
export type LocalAppAgentEvent =
  | { type: 'state'; runId: string; sequence: number; time: number; run: LocalAppAgentRun }
  | { type: 'progress'; runId: string; sequence: number; time: number; phase: LocalAppAgentProgressPhase; text?: string };
export interface LocalAppAgentApi {
  run(input: LocalAppAgentRunInput): Promise<{ runId: string }>;
  get(runId: string): Promise<LocalAppAgentRun>;
  /** Replays the current state before live events. The returned function unsubscribes. */
  subscribe(runId: string, listener: (event: LocalAppAgentEvent) => void): () => void;
  cancel(runId: string): Promise<void>;
}
