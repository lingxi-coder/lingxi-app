import type { BaseEvent, ContentPart, Message, RunAgentInput } from '@ag-ui/core';
import type { RunAgentResult } from '@ag-ui/client';
import type { LocalAppAgent } from '@lingxi/local-app-agui/client';

/** The host extension is carried only through AG-UI's forwardedProps field. */
export interface LocalAppAgentRunOptions {
  version: 1;
  outputSchema?: Record<string, unknown>;
  tools?: string[];
  skills?: string[];
  maxTurns?: number;
  timeoutMs?: number;
}

export const LOCAL_APP_AGENT_DEFAULT_MAX_TURNS = 20;
export const LOCAL_APP_AGENT_DEFAULT_TIMEOUT_MS = 600_000;
export const LOCAL_APP_AGENT_MAX_TURNS = 100;
export const LOCAL_APP_AGENT_MAX_TIMEOUT_MS = 3_600_000;
export const LOCAL_APP_AGENT_MAX_FILES = 8;
export const LOCAL_APP_AGENT_MAX_FILE_BYTES = 25 * 1024 * 1024;
export const LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES = 50 * 1024 * 1024;

export interface LocalAppAgentModel { providerId: string; modelId: string }
export interface LocalAppAgentModelChoice extends LocalAppAgentModel {
  label: string;
  canSelect: boolean;
  /** Generic text/PDF/image file transport is handled by the host. */
  supportsAttachments: boolean;
  /** Only image bytes require this provider capability. */
  supportsVision: boolean;
  supportsStructuredOutput: boolean;
}
export type LocalAppAgentToolGrant =
  | { id: string; kind: 'builtin'; name: string }
  | { id: string; kind: 'mcp'; serverConfigId: string; configRevision: string; rawToolName: string; definitionDigest: string };
export type LocalAppAgentToolChoice = LocalAppAgentToolGrant & { label: string; description?: string };
export interface LocalAppAgentSkillChoice { id: string; label: string; description?: string }
export interface LocalAppAgentCatalog {
  models: LocalAppAgentModelChoice[];
  tools: LocalAppAgentToolChoice[];
  skills: LocalAppAgentSkillChoice[];
}
/** Only trusted native Apps settings can change these selections. */
export interface LocalAppAgentSettings {
  enabled: boolean;
  model: LocalAppAgentModel;
  tools: LocalAppAgentToolGrant[];
  skills: string[];
  background: boolean;
  maxTurns: number;
  timeoutMs: number;
}

export type LocalAppAgentTaskState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';
/** Host-owned task resource metadata; result messages and events use AG-UI. */
export interface LocalAppAgentTask {
  runId: string;
  threadId: string;
  state: LocalAppAgentTaskState;
  sequence: number;
  phase?: string;
  createdAt: number;
  updatedAt: number;
  limits: { maxTurns: number; timeoutMs: number };
  usage?: LocalAppAgentUsage;
  progress?: { completedModelTurns: number; turnLimit: number };
  error?: { code: string; message: string };
}
export interface LocalAppAgentUsage {
  /** Omitted when the provider did not report the value. */
  inputTokens?: number;
  outputTokens?: number;
  costUsd?: number;
  costEstimated?: boolean;
  turns?: number;
}
export interface LocalAppAgentTaskEventPage { events: BaseEvent[]; sequence: number }
export interface LocalAppAgentTaskListOptions { cursor?: string; limit?: number }
export interface LocalAppAgentTasksApi {
  start(input: RunAgentInput): Promise<{ runId: string }>;
  get(runId: string): Promise<LocalAppAgentTask>;
  list(options?: LocalAppAgentTaskListOptions): Promise<{ tasks: LocalAppAgentTask[]; nextCursor?: string }>;
  events(input: { runId: string; afterSequence?: number }): Promise<LocalAppAgentTaskEventPage>;
  subscribe(runId: string, listener: (event: BaseEvent) => void): () => void;
  cancel(runId: string): Promise<void>;
}

export interface LocalAppAgentChatSummary {
  /** Chat resource identifier and AG-UI threadId are the same value. */
  chatId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  archived: boolean;
  state: 'idle' | 'running' | 'interrupted';
}
export interface LocalAppAgentChatHistory { messages: Message[]; nextCursor?: string }
export interface LocalAppAgentChatHistoryOptions { cursor?: string; limit?: number }
export interface LocalAppAgentChatListOptions extends LocalAppAgentTaskListOptions { includeArchived?: boolean }
export interface LocalAppAgentChatListPage {
  chats: LocalAppAgentChatSummary[];
  /** Unavailable IDs in this page; their original Chat data is preserved. */
  unavailableChats?: Array<{ chatId: string; code: 'chat_storage_uncertain' }>;
  /** Signals that at least one Chat across all pages needs recovery. */
  hasUncertainChats?: boolean;
  nextCursor?: string;
}
export interface LocalAppAgentChatHandle {
  /** Host-owned chat ID; also the stable AG-UI threadId. */
  id: string;
  send(input: RunAgentInput): Promise<RunAgentResult>;
  history(options?: LocalAppAgentChatHistoryOptions): Promise<LocalAppAgentChatHistory>;
  subscribe(listener: (message: Message) => void): () => void;
  cancel(): Promise<void>;
}
export interface LocalAppAgentChatsApi {
  create(input: { requestId: string; title?: string }): Promise<LocalAppAgentChatHandle>;
  creationStatus(input: { requestId: string; title?: string }): Promise<LocalAppAgentChatHandle | null>;
  pendingCreations(): Promise<Array<{ requestId: string; title?: string; state: 'pending' | 'accepted' | 'deleted' | 'rejected' }>>;
  forgetCreation(requestId: string): Promise<void>;
  open(chatId: string): Promise<LocalAppAgentChatHandle>;
  list(options?: LocalAppAgentChatListOptions): Promise<LocalAppAgentChatListPage>;
  archive(chatId: string): Promise<void>;
  delete(chatId: string): Promise<void>;
}
export interface LocalAppAgentClientOptions { threadId?: string; initialMessages?: Message[] }
export interface LocalAppAgentApi {
  run(input: RunAgentInput): Promise<RunAgentResult>;
  createClient(options?: LocalAppAgentClientOptions): LocalAppAgent;
  file(file: File): Promise<ContentPart>;
  tasks: LocalAppAgentTasksApi;
  chats: LocalAppAgentChatsApi;
}

/** Trusted native file-picker options. Returned values are standard File objects. */
export interface LocalAppFilePickOptions { multiple?: boolean; accept?: string | string[] }
export interface LocalAppFilesApi {
  pick(options?: LocalAppFilePickOptions): Promise<File[]>;
  pickDirectory(): Promise<{ token: string; name: string } | null>;
  read(root: string, path: string): Promise<string>;
  write(root: string, path: string, text: string): Promise<void>;
  list(root: string, path?: string): Promise<Array<{ name: string; directory: boolean }>>;
}
