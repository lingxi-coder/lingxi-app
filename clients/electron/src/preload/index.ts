import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import type {
  AskUserQuestionRequestDto,
  ClientCommand,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';

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
const CH_PERMISSION = 'lingxi:permission';
const CH_COMPUTER_ACCESS = 'lingxi:computerAccess';
const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';
const CH_OPEN_SYSTEM_SETTINGS = 'lingxi:openSystemSettings';
const CH_BOOTSTRAP = 'lingxi:bootstrap';
const CH_SETTINGS_GET = 'lingxi:settings:get';
const CH_SETTINGS_UPDATE = 'lingxi:settings:update';
const CH_WORKSPACE_PICK = 'lingxi:workspace:pick';
const CH_WORKSPACE_SET = 'lingxi:workspace:set';
const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
const CH_TRUST_SET = 'lingxi:trust:set';
const CH_CREDENTIAL_GET = 'lingxi:credential:get';
const CH_CREDENTIAL_SET = 'lingxi:credential:set';
const CH_CREDENTIAL_CLEAR = 'lingxi:credential:clear';
const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
const CH_DIAGNOSTICS_GET = 'lingxi:diagnostics:get';
const CH_DIAGNOSTICS_COPY = 'lingxi:diagnostics:copy';
const CH_DIAGNOSTICS_EXPORT = 'lingxi:diagnostics:export';

export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'restarting' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

export type AllowedClientCommand = Extract<ClientCommand, {
  type: 'set_model' | 'list_models' | 'new_session' | 'resume_session' | 'list_sessions' |
    'task_list' | 'task_output' | 'task_stop' | 'set_permission_mode' | 'run_slash_command';
}> | { type: 'refresh_listings'; which: Array<{ type: 'status' | 'doctor' | 'slash_commands' }> };

export interface PublicSettings {
  version: 1;
  theme?: 'dark' | 'light';
  model?: string;
  apiBaseUrl?: string;
  lastWorkspace?: string;
  recentWorkspaces: string[];
}

export interface WorkspaceMetadata {
  path?: string;
  trusted: boolean;
  fingerprint?: string;
  recovery?: {
    state: 'missing';
    message: string;
  };
}
export interface CredentialMetadata { configured: boolean; encryptionAvailable: boolean; sessionOnly?: true; runtimeOnly?: true }
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
  credential: CredentialMetadata;
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
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  setWorkspaceTrusted(trusted: boolean): Promise<WorkspaceMetadata>;
  credential(): Promise<CredentialMetadata>;
  setCredential(credential: string): Promise<CredentialMetadata>;
  clearCredential(): Promise<CredentialMetadata>;
  providerCredentials(): Promise<ProviderCredentialMetadata[]>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  restartBridge(): Promise<void>;
  diagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  sendPrompt(text: string): Promise<void>;
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(requestId: number): Promise<void>;
  approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(requestId: number): Promise<void>;
  answerAskUserQuestion(requestId: number, answers: Record<string, string>): Promise<void>;
  cancelAskUserQuestion(requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  cancel(turnId?: number): Promise<void>;
  /** Only the bounded Desktop model/session/task/slash surface is accepted by the main process. */
  command(command: AllowedClientCommand): Promise<void>;
  connectionState(): Promise<ConnectionState>;
  onEvent(cb: (event: ClientEvent) => void): Unsubscribe;
  onPermission(cb: (request: PermissionRequest) => void): Unsubscribe;
  onComputerAccess(cb: (request: ComputerAccessRequestDto) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: ConnectionState) => void): Unsubscribe;
}

function subscribe<T>(channel: string, callback: (payload: T) => void): Unsubscribe {
  const listener = (_event: IpcRendererEvent, payload: T): void => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

const api: LingxiApi = {
  platform: process.platform,
  isElectron: true,
  bootstrap: () => ipcRenderer.invoke(CH_BOOTSTRAP) as Promise<BootstrapState>,
  settings: () => ipcRenderer.invoke(CH_SETTINGS_GET) as Promise<PublicSettings>,
  updateSettings: (patch) => ipcRenderer.invoke(CH_SETTINGS_UPDATE, patch) as Promise<PublicSettings>,
  pickWorkspace: () => ipcRenderer.invoke(CH_WORKSPACE_PICK) as Promise<WorkspaceMetadata | null>,
  setWorkspace: (path) => ipcRenderer.invoke(CH_WORKSPACE_SET, path) as Promise<WorkspaceMetadata>,
  searchWorkspaceFiles: (query) => ipcRenderer.invoke(CH_WORKSPACE_FILES_SEARCH, query) as Promise<WorkspaceFileSearchResult>,
  setWorkspaceTrusted: (trusted) => ipcRenderer.invoke(CH_TRUST_SET, trusted) as Promise<WorkspaceMetadata>,
  credential: () => ipcRenderer.invoke(CH_CREDENTIAL_GET) as Promise<CredentialMetadata>,
  setCredential: (credential) => ipcRenderer.invoke(CH_CREDENTIAL_SET, credential) as Promise<CredentialMetadata>,
  clearCredential: () => ipcRenderer.invoke(CH_CREDENTIAL_CLEAR) as Promise<CredentialMetadata>,
  providerCredentials: () => ipcRenderer.invoke(CH_PROVIDER_CREDENTIALS_GET) as Promise<ProviderCredentialMetadata[]>,
  setProviderCredential: (providerId, credential) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_SET, providerId, credential) as Promise<ProviderCredentialUpdate>,
  clearProviderCredential: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_CLEAR, providerId) as Promise<ProviderCredentialMetadata>,
  restartBridge: () => ipcRenderer.invoke(CH_BRIDGE_RESTART) as Promise<void>,
  diagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_GET) as Promise<DiagnosticEntry[]>,
  copyDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_COPY) as Promise<void>,
  exportDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_EXPORT) as Promise<string | null>,
  sendPrompt: (text) => ipcRenderer.invoke(CH_SEND_PROMPT, text) as Promise<void>,
  approve: (requestId, response) => ipcRenderer.invoke(CH_APPROVE, requestId, response) as Promise<void>,
  deny: (requestId) => ipcRenderer.invoke(CH_DENY, requestId) as Promise<void>,
  approveComputerAccess: (requestId, response) => ipcRenderer.invoke(CH_APPROVE_COMPUTER_ACCESS, requestId, response) as Promise<void>,
  denyComputerAccess: (requestId) => ipcRenderer.invoke(CH_DENY_COMPUTER_ACCESS, requestId) as Promise<void>,
  answerAskUserQuestion: (requestId, answers) => ipcRenderer.invoke(CH_ANSWER_ASK_USER_QUESTION, requestId, answers) as Promise<void>,
  cancelAskUserQuestion: (requestId) => ipcRenderer.invoke(CH_CANCEL_ASK_USER_QUESTION, requestId) as Promise<void>,
  openSystemSettings: (pane) => ipcRenderer.invoke(CH_OPEN_SYSTEM_SETTINGS, pane) as Promise<void>,
  cancel: (turnId) => ipcRenderer.invoke(CH_CANCEL, turnId) as Promise<void>,
  command: (command) => ipcRenderer.invoke(CH_COMMAND, command) as Promise<void>,
  connectionState: () => ipcRenderer.invoke(CH_CONNECTION_STATE) as Promise<ConnectionState>,
  onEvent: (callback) => subscribe(CH_EVENT, callback),
  onPermission: (callback) => subscribe(CH_PERMISSION, callback),
  onComputerAccess: (callback) => subscribe(CH_COMPUTER_ACCESS, callback),
  onConnectionStateChanged: (callback) => subscribe(CH_STATE_CHANGED, callback),
};

contextBridge.exposeInMainWorld('lingxi', api);
