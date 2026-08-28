import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import type {
  AskUserQuestionRequestDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  ImageRefDto,
  PermissionRequest,
  PermissionResponseDto,
  SessionRowDto,
} from '@lingxi/bridge-client';
import { createRuntimeEventReplayBuffer, type SequencedRuntimeEventEnvelope } from './event-replay.js';
import type { AllowedClientCommand } from '../shared/clientCommands.js';

export type { AllowedClientCommand } from '../shared/clientCommands.js';

const CH_SEND_PROMPT = 'lingxi:sendPrompt';
const CH_APPROVE = 'lingxi:approve';
const CH_DENY = 'lingxi:deny';
const CH_APPROVE_COMPUTER_ACCESS = 'lingxi:approveComputerAccess';
const CH_DENY_COMPUTER_ACCESS = 'lingxi:denyComputerAccess';
const CH_ANSWER_ASK_USER_QUESTION = 'lingxi:answerAskUserQuestion';
const CH_CANCEL_ASK_USER_QUESTION = 'lingxi:cancelAskUserQuestion';
const CH_CANCEL = 'lingxi:cancel';
const CH_COMMAND = 'lingxi:command';
const CH_CONNECTION_STATE = 'lingxi:connectionState';
const CH_EVENT = 'lingxi:event';
const CH_EVENT_REPLAY = 'lingxi:event:replay';
const CH_PERMISSION = 'lingxi:permission';
const CH_COMPUTER_ACCESS = 'lingxi:computerAccess';
const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';
const CH_OPEN_SYSTEM_SETTINGS = 'lingxi:openSystemSettings';
const CH_BOOTSTRAP = 'lingxi:bootstrap';
const CH_SETTINGS_GET = 'lingxi:settings:get';
const CH_SETTINGS_UPDATE = 'lingxi:settings:update';
const CH_WORKSPACE_PICK = 'lingxi:workspace:pick';
const CH_WORKSPACE_SET = 'lingxi:workspace:set';
const CH_PROJECT_REMOVE = 'lingxi:project:remove';
const CH_SESSION_PIN_SET = 'lingxi:session-pin:set';
const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
const CH_DIAGNOSTICS_GET = 'lingxi:diagnostics:get';
const CH_DIAGNOSTICS_COPY = 'lingxi:diagnostics:copy';
const CH_DIAGNOSTICS_EXPORT = 'lingxi:diagnostics:export';
const CH_CLIPBOARD_WRITE_TEXT = 'lingxi:clipboard:writeText';
const CH_PROJECT_SESSIONS_LIST = 'lingxi:project-sessions:list';
const CH_SESSION_NEW = 'lingxi:session:new';
const CH_SESSION_OPEN = 'lingxi:session:open';

export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'restarting' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

export interface PublicSettings {
  version: 1;
  theme?: 'dark' | 'light';
  model?: string;
  apiBaseUrl?: string;
  activeProject?: string;
  activeSession?: SessionRef;
  projects: string[];
  pinnedSessions: PinnedSessionRecord[];
}

export interface SessionRef {
  projectPath: string;
  sessionId: string;
}

export interface RuntimeEventEnvelope<T = unknown> {
  sessionId: string;
  event: T;
}

export interface SessionRuntimeSummary {
  projectPath: string;
  sessionId: string;
  connection: ConnectionState;
  turnActive: boolean;
  pendingInteractions: number;
  pendingAskUserQuestions: number;
}

export interface ProjectSessionCatalogState {
  sessions: SessionRowDto[];
  error?: string;
}

export interface PinnedSessionRecord {
  projectPath: string;
  sessionId: string;
  title: string;
  pinnedAt: string;
}
export type SessionPinInput = Omit<PinnedSessionRecord, 'pinnedAt'>;

export interface WorkspaceMetadata {
  path?: string;
  trusted: boolean;
  fingerprint?: string;
  recovery?: {
    state: 'missing';
    message: string;
  };
}
export interface CredentialMetadata { configured: boolean; encryptionAvailable: boolean; runtimeOnly?: true }
export interface ProviderCredentialMetadata extends CredentialMetadata { providerId: string }
export interface ProviderCredentialUpdate { credential: ProviderCredentialMetadata; settings: PublicSettings }
export interface DiagnosticEntry {
  timestamp: string;
  level: 'info' | 'warn' | 'error';
  source: 'host' | 'bridge';
  message: string;
}
export interface BootstrapState {
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  activeSession?: SessionRef;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  providerCredentials?: ProviderCredentialMetadata[];
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
}
export interface WorkspaceFileSearchResult { files: string[]; truncated: boolean }

export type Unsubscribe = () => void;

/** The two macOS System Settings deep links the computer-access TCC panel opens. */
export type SystemSettingsPane = 'accessibility' | 'screen_recording';

export interface LingxiApi {
  platform: NodeJS.Platform;
  isElectron: true;
  bootstrap(): Promise<BootstrapState>;
  settings(): Promise<PublicSettings>;
  updateSettings(patch: { theme?: 'dark' | 'light'; model?: string | null; apiBaseUrl?: string | null }): Promise<PublicSettings>;
  pickWorkspace(): Promise<WorkspaceMetadata | null>;
  setWorkspace(path: string): Promise<WorkspaceMetadata>;
  removeProject(path: string): Promise<BootstrapState>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<PublicSettings>;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  providerCredentials(): Promise<ProviderCredentialMetadata[]>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  restartBridge(sessionId: string): Promise<void>;
  diagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  listProjectSessions(projectPath: string): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  newSession(projectPath: string, model?: string): Promise<BootstrapState>;
  openSession(projectPath: string, sessionId: string): Promise<BootstrapState>;
  sendPrompt(sessionId: string, text: string, images?: ImageRefDto[]): Promise<void>;
  approve(sessionId: string, requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(sessionId: string, requestId: number): Promise<void>;
  approveComputerAccess(sessionId: string, requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(sessionId: string, requestId: number): Promise<void>;
  answerAskUserQuestion(sessionId: string, requestId: number, answers: Record<string, string>): Promise<void>;
  cancelAskUserQuestion(sessionId: string, requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  cancel(sessionId: string, turnId?: number): Promise<void>;
  /** Only the bounded Desktop model/session/task/slash surface is accepted by the main process. */
  command(sessionId: string, command: AllowedClientCommand): Promise<void>;
  connectionState(sessionId: string): Promise<ConnectionState>;
  onEvent(cb: (event: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe;
  onPermission(cb: (request: RuntimeEventEnvelope<PermissionRequest>) => void): Unsubscribe;
  onComputerAccess(cb: (request: RuntimeEventEnvelope<ComputerAccessRequestDto>) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: RuntimeEventEnvelope<ConnectionState>) => void): Unsubscribe;
}

function subscribe<T>(channel: string, callback: (payload: T) => void): Unsubscribe {
  const listener = (_event: IpcRendererEvent, payload: T): void => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

function subscribeRuntimeEvents(callback: (payload: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe {
  const replayBuffer = createRuntimeEventReplayBuffer(callback);
  const listener = (_event: IpcRendererEvent, payload: SequencedRuntimeEventEnvelope<ClientEvent>): void => {
    replayBuffer.push(payload);
  };
  ipcRenderer.on(CH_EVENT, listener);
  void (ipcRenderer.invoke(CH_EVENT_REPLAY) as Promise<SequencedRuntimeEventEnvelope<ClientEvent>[]>)
    .then((replay) => replayBuffer.resolve(replay))
    .catch(() => replayBuffer.reject());
  return () => {
    replayBuffer.dispose();
    ipcRenderer.removeListener(CH_EVENT, listener);
  };
}

const api: LingxiApi = {
  platform: process.platform,
  isElectron: true,
  bootstrap: () => ipcRenderer.invoke(CH_BOOTSTRAP) as Promise<BootstrapState>,
  settings: () => ipcRenderer.invoke(CH_SETTINGS_GET) as Promise<PublicSettings>,
  updateSettings: (patch) => ipcRenderer.invoke(CH_SETTINGS_UPDATE, patch) as Promise<PublicSettings>,
  pickWorkspace: () => ipcRenderer.invoke(CH_WORKSPACE_PICK) as Promise<WorkspaceMetadata | null>,
  setWorkspace: (path) => ipcRenderer.invoke(CH_WORKSPACE_SET, path) as Promise<WorkspaceMetadata>,
  removeProject: (path) => ipcRenderer.invoke(CH_PROJECT_REMOVE, path) as Promise<BootstrapState>,
  setSessionPinned: (session, pinned) => ipcRenderer.invoke(CH_SESSION_PIN_SET, session, pinned) as Promise<PublicSettings>,
  searchWorkspaceFiles: (query) => ipcRenderer.invoke(CH_WORKSPACE_FILES_SEARCH, query) as Promise<WorkspaceFileSearchResult>,
  providerCredentials: () => ipcRenderer.invoke(CH_PROVIDER_CREDENTIALS_GET) as Promise<ProviderCredentialMetadata[]>,
  setProviderCredential: (providerId, credential) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_SET, providerId, credential) as Promise<ProviderCredentialUpdate>,
  clearProviderCredential: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_CLEAR, providerId) as Promise<ProviderCredentialMetadata>,
  restartBridge: (sessionId) => ipcRenderer.invoke(CH_BRIDGE_RESTART, sessionId) as Promise<void>,
  diagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_GET) as Promise<DiagnosticEntry[]>,
  copyDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_COPY) as Promise<void>,
  copyText: (text) => ipcRenderer.invoke(CH_CLIPBOARD_WRITE_TEXT, text) as Promise<void>,
  exportDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_EXPORT) as Promise<string | null>,
  listProjectSessions: (projectPath) => ipcRenderer.invoke(CH_PROJECT_SESSIONS_LIST, projectPath) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  newSession: (projectPath, model) => ipcRenderer.invoke(CH_SESSION_NEW, projectPath, model) as Promise<BootstrapState>,
  openSession: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId) as Promise<BootstrapState>,
  sendPrompt: (sessionId, text, images) => ipcRenderer.invoke(CH_SEND_PROMPT, sessionId, text, images ?? []) as Promise<void>,
  approve: (sessionId, requestId, response) => ipcRenderer.invoke(CH_APPROVE, sessionId, requestId, response) as Promise<void>,
  deny: (sessionId, requestId) => ipcRenderer.invoke(CH_DENY, sessionId, requestId) as Promise<void>,
  approveComputerAccess: (sessionId, requestId, response) => ipcRenderer.invoke(CH_APPROVE_COMPUTER_ACCESS, sessionId, requestId, response) as Promise<void>,
  denyComputerAccess: (sessionId, requestId) => ipcRenderer.invoke(CH_DENY_COMPUTER_ACCESS, sessionId, requestId) as Promise<void>,
  answerAskUserQuestion: (sessionId, requestId, answers) => ipcRenderer.invoke(CH_ANSWER_ASK_USER_QUESTION, sessionId, requestId, answers) as Promise<void>,
  cancelAskUserQuestion: (sessionId, requestId) => ipcRenderer.invoke(CH_CANCEL_ASK_USER_QUESTION, sessionId, requestId) as Promise<void>,
  openSystemSettings: (pane) => ipcRenderer.invoke(CH_OPEN_SYSTEM_SETTINGS, pane) as Promise<void>,
  cancel: (sessionId, turnId) => ipcRenderer.invoke(CH_CANCEL, sessionId, turnId) as Promise<void>,
  command: (sessionId, command) => ipcRenderer.invoke(CH_COMMAND, sessionId, command) as Promise<void>,
  connectionState: (sessionId) => ipcRenderer.invoke(CH_CONNECTION_STATE, sessionId) as Promise<ConnectionState>,
  onEvent: (callback) => subscribeRuntimeEvents(callback),
  onPermission: (callback) => subscribe(CH_PERMISSION, callback),
  onComputerAccess: (callback) => subscribe(CH_COMPUTER_ACCESS, callback),
  onConnectionStateChanged: (callback) => subscribe(CH_STATE_CHANGED, callback),
};

contextBridge.exposeInMainWorld('lingxi', api);
