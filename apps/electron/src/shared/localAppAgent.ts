/** The public, page-scoped Local App Agent contract. Files stay binary until the host boundary. */
export const CH_LOCAL_APP_AGENT_EVENT = 'lingxi:local-app:agent:event';
export const LOCAL_APP_AGENT_DEFAULT_MAX_TURNS = 20;
export const LOCAL_APP_AGENT_DEFAULT_TIMEOUT_MS = 600_000;
export const LOCAL_APP_AGENT_MAX_TURNS = 100;
export const LOCAL_APP_AGENT_MAX_TIMEOUT_MS = 3_600_000;
export const LOCAL_APP_AGENT_MAX_FILES = 8;
export const LOCAL_APP_AGENT_MAX_FILE_BYTES = 25 * 1024 * 1024;
export const LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES = 50 * 1024 * 1024;

/** `File` is accepted directly; native paths and base64 are never page API fields. */
export interface LocalAppAgentInput {
  requestId?: string;
  prompt: string;
  attachments?: readonly File[];
  outputSchema?: Record<string, unknown>;
  /** Exact tool IDs declared by this release and granted by the host. */
  tools?: string[];
  /** Exact Skill IDs declared by this release and granted by the host. */
  skills?: string[];
  maxTurns?: number;
  timeoutMs?: number;
}

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
export interface LocalAppAgentUsage {
  /** Omitted when the provider did not report the value. */
  inputTokens?: number;
  outputTokens?: number;
  costUsd?: number;
  costEstimated?: boolean;
  turns?: number;
}
export interface LocalAppAgentTaskResult {
  text: string;
  output?: unknown;
  /** Result attachments are not emitted in this release. */
  attachments: [];
  /** Present for a chat turn so the SDK can form a ChatReply without guessing. */
  chatId?: string;
  messageId?: string;
}
export interface LocalAppAgentTask {
  runId: string;
  requestId: string;
  state: LocalAppAgentTaskState;
  sequence: number;
  phase?: 'waiting_for_permission' | 'waiting_for_answer';
  createdAt: number;
  updatedAt: number;
  model: LocalAppAgentModel;
  limits: { maxTurns: number; timeoutMs: number };
  result?: LocalAppAgentTaskResult;
  usage?: LocalAppAgentUsage;
  progress?: { completedModelTurns: number; turnLimit: number };
  error?: { code: string; message: string };
}
/** Host-normalized form passed only between trusted main and native execution. */
export interface LocalAppAgentNormalizedAttachment { name: string; mediaType: string; base64: string }
export interface LocalAppAgentRunInput extends Omit<LocalAppAgentInput, 'attachments'> {
  requestId: string;
  attachments?: LocalAppAgentNormalizedAttachment[];
}
export type LocalAppAgentRun = LocalAppAgentTask;
export type LocalAppAgentResult = LocalAppAgentTaskResult;
export type LocalAppAgentProgressPhase = 'starting' | 'thinking' | 'tool' | 'waiting_for_permission' | 'waiting_for_answer';
export type LocalAppAgentEvent =
  | { type: 'state'; runId: string; sequence: number; time: number; task: LocalAppAgentTask }
  | { type: 'connection'; runId: string; error: { code: string; message: string } };
export interface LocalAppAgentReply {
  runId: string;
  text: string;
  output?: unknown;
  attachments: [];
  usage: LocalAppAgentUsage;
}
export interface LocalAppAgentChatReply extends LocalAppAgentReply { chatId: string; messageId: string }
export interface LocalAppAgentTaskListOptions { cursor?: string; limit?: number }
export interface LocalAppAgentTasksApi {
  start(input: LocalAppAgentInput): Promise<{ runId: string }>;
  get(runId: string): Promise<LocalAppAgentTask>;
  list(options?: LocalAppAgentTaskListOptions): Promise<LocalAppAgentTask[]>;
  subscribe(runId: string, listener: (event: LocalAppAgentEvent) => void): () => void;
  cancel(runId: string): Promise<void>;
}

export interface LocalAppAgentChatSummary {
  chatId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  archived: boolean;
  state: 'idle' | 'running' | 'interrupted';
}
export interface LocalAppAgentChatMessage {
  messageId: string;
  runId: string;
  role: 'user' | 'assistant';
  text: string;
  createdAt: number;
  attachments: Array<{ name: string; mediaType: string; size: number }>;
}
export interface LocalAppAgentChatHistory { messages: LocalAppAgentChatMessage[]; nextCursor?: string }
export interface LocalAppAgentChatHistoryOptions { cursor?: string; limit?: number }
export interface LocalAppAgentChatListOptions extends LocalAppAgentTaskListOptions { includeArchived?: boolean }
export type LocalAppAgentChatEvent =
  | { type: 'state'; chatId: string; chat: LocalAppAgentChatSummary }
  | { type: 'messages'; chatId: string; messages: LocalAppAgentChatMessage[]; nextCursor?: string }
  | { type: 'connection'; chatId: string; error: { code: string; message: string } };
export interface LocalAppAgentChatHandle {
  id: string;
  send(input: LocalAppAgentInput): Promise<LocalAppAgentChatReply>;
  history(options?: LocalAppAgentChatHistoryOptions): Promise<LocalAppAgentChatHistory>;
  subscribe(listener: (event: LocalAppAgentChatEvent) => void): () => void;
  cancel(): Promise<void>;
}
export interface LocalAppAgentChatsApi {
  create(input?: { title?: string }): Promise<LocalAppAgentChatHandle>;
  open(chatId: string): Promise<LocalAppAgentChatHandle>;
  list(options?: LocalAppAgentChatListOptions): Promise<LocalAppAgentChatSummary[]>;
  archive(chatId: string): Promise<void>;
  delete(chatId: string): Promise<void>;
}
export interface LocalAppAgentApi {
  /** A one-shot independent request. The returned Promise lives only as long as this page. */
  run(input: LocalAppAgentInput): Promise<LocalAppAgentReply>;
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
