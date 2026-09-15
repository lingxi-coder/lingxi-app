import type { CronJobDto } from '@lingxi/bridge-client';
import { contextBridge, ipcRenderer, webUtils, type IpcRendererEvent } from 'electron';
import { CH_TERMINAL_REQUEST, CH_TERMINAL_EVENT } from '../shared/terminal.js';
import type { TerminalApi } from '../shared/terminal.js';
import type {
  AskUserQuestionRequestDto,
  AudioOpDto,
  AudioResultDto,
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
import {
  CH_NATIVE_AUDIO_ENGINE_REQUEST,
  CH_NATIVE_AUDIO_EVENT,
  CH_NATIVE_AUDIO_REQUEST,
} from '../shared/nativeAudio.js';
import type {
  NativeAudioCommand,
  NativeAudioCommandResult,
  NativeAudioEvent,
  NativeAudioResponse,
} from '../shared/nativeAudio.js';
import type { PublicSettings, SessionPinInput, SessionRef } from '../shared/settings.js';
import type { MicrophonePermissionStatus } from '../shared/microphoneAccess.js';
import type { PluginSecretMetadata, WorkspaceFilePreview } from '../main/host.js';

export type { AllowedClientCommand } from '../shared/clientCommands.js';
export type {
  PinnedSessionRecord,
  PublicSettings,
  SessionPinInput,
  SessionRef,
} from '../shared/settings.js';

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
const CH_MICROPHONE_ACCESS_GET = 'lingxi:microphone-access:get';
const CH_BOOTSTRAP = 'lingxi:bootstrap';
const CH_SETTINGS_GET = 'lingxi:settings:get';
const CH_SETTINGS_FILE_OPEN = 'lingxi:settings:file:open';
const CH_SETTINGS_UPDATE = 'lingxi:settings:update';
const CH_WORKSPACE_PICK = 'lingxi:workspace:pick';
const CH_WORKSPACE_SET = 'lingxi:workspace:set';
const CH_PROJECT_REMOVE = 'lingxi:project:remove';
const CH_SESSION_PIN_SET = 'lingxi:session-pin:set';
const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
const CH_PROVIDER_CONNECTION_TEST = 'lingxi:provider-connection:test';
const CH_PLUGIN_SECRET_GET = 'lingxi:plugin-secret:get';
const CH_PLUGIN_SECRET_SET = 'lingxi:plugin-secret:set';
const CH_PLUGIN_SECRET_CLEAR = 'lingxi:plugin-secret:clear';
const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
const CH_DIAGNOSTICS_GET = 'lingxi:diagnostics:get';
const CH_DIAGNOSTICS_COPY = 'lingxi:diagnostics:copy';
const CH_DIAGNOSTICS_EXPORT = 'lingxi:diagnostics:export';
const CH_CLIPBOARD_WRITE_TEXT = 'lingxi:clipboard:writeText';
const CH_PROJECT_SESSIONS_LIST = 'lingxi:project-sessions:list';
const CH_SESSION_NEW = 'lingxi:session:new';
const CH_SESSION_OPEN = 'lingxi:session:open';
const CH_SESSION_ARCHIVE = 'lingxi:session:archive';
const CH_SESSION_ARCHIVE_PREFLIGHT = 'lingxi:session:archive-preflight';
const CH_SESSION_CLEAR = 'lingxi:session:clear';
const CH_WORKSPACE_FILE_PREVIEW = 'lingxi:workspace-file:preview';

export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'restarting' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

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

export interface WorkspaceMetadata {
  path?: string;
  trusted: boolean;
  fingerprint?: string;
  recovery?: {
    state: 'missing';
    message: string;
  };
}
export interface CredentialMetadata { configured: boolean; encryptionAvailable: boolean; credentialPreview?: string; runtimeOnly?: true }
export interface ProviderCredentialMetadata extends CredentialMetadata { providerId: string }
export interface ProviderCredentialUpdate { credential: ProviderCredentialMetadata; settings: PublicSettings }
export type ProviderConnectionTestResult = Extract<ClientEvent, { type: 'provider_connection_tested' }>;
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
export interface NativeAudioApi {
  request(command: NativeAudioCommand): Promise<NativeAudioCommandResult>;
  onEvent(cb: (event: NativeAudioEvent) => void): Unsubscribe;
  executeEngineRequest(sessionId: string, op: AudioOpDto): Promise<AudioResultDto>;
}

/** The macOS System Settings deep links this app opens: the computer-access TCC panel's two panes, plus the voice settings page's `microphone` row. */
export type SystemSettingsPane = 'accessibility' | 'screen_recording' | 'microphone' | 'speech_recognition';

export interface LingxiApi {
  terminal: TerminalApi;
  getPathForFile(file: File): string;
  platform: NodeJS.Platform;
  isElectron: true;
  bootstrap(): Promise<BootstrapState>;
  settings(): Promise<PublicSettings>;
  openSettingsFile(path: string): Promise<void>;
  updateSettings(patch: {
    theme?: 'dark' | 'light' | 'system';
    collapseThoughtsByDefault?: boolean;
    model?: string | null;
    apiBaseUrl?: string | null;
    voice?: unknown;
    notifications?: unknown;
    modelPickerVisibility?: unknown;
  }): Promise<PublicSettings>;
  pickWorkspace(): Promise<WorkspaceMetadata | null>;
  setWorkspace(path: string): Promise<WorkspaceMetadata>;
  removeProject(path: string): Promise<BootstrapState>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<PublicSettings>;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  previewWorkspaceFile(sessionId: string, path: string): Promise<WorkspaceFilePreview>;
  providerCredentials(providerId?: string): Promise<ProviderCredentialMetadata[]>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  testProviderConnection(providerId: string, credentialOverride?: string): Promise<ProviderConnectionTestResult>;
  pluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretMetadata>;
  clearPluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  restartBridge(sessionId: string): Promise<void>;
  diagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  listProjectSessions(projectPath: string): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  newSession(projectPath: string, model?: string): Promise<BootstrapState>;
  openSession(projectPath: string, sessionId: string): Promise<BootstrapState>;
  preflightSessionArchive(projectPath: string, sessionId: string): Promise<CronJobDto[]>;
  archiveSession(projectPath: string, sessionId: string): Promise<BootstrapState>;
  clearSession(sessionId: string): Promise<void>;
  sendPrompt(sessionId: string, text: string, images?: ImageRefDto[]): Promise<void>;
  approve(sessionId: string, requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(sessionId: string, requestId: number): Promise<void>;
  approveComputerAccess(sessionId: string, requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(sessionId: string, requestId: number): Promise<void>;
  answerAskUserQuestion(sessionId: string, requestId: number, answers: Record<string, string>): Promise<void>;
  cancelAskUserQuestion(sessionId: string, requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  /**
   * The OS microphone grant, read in the main process
   * (`systemPreferences.getMediaAccessStatus`). The renderer has no
   * equivalent: `navigator.permissions.query({name:'microphone'})` reports
   * the page permission this app grants itself, which can say `granted`
   * while macOS denies the device — see `shared/microphoneAccess.ts`.
   */
  microphoneAccess(): Promise<MicrophonePermissionStatus>;
  cancel(sessionId: string, turnId?: number): Promise<void>;
  /** Only the bounded Desktop model/session/task/slash surface is accepted by the main process. */
  command(sessionId: string, command: AllowedClientCommand): Promise<void>;
  connectionState(sessionId: string): Promise<ConnectionState>;
  onEvent(cb: (event: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe;
  onPermission(cb: (request: RuntimeEventEnvelope<PermissionRequest>) => void): Unsubscribe;
  onComputerAccess(cb: (request: RuntimeEventEnvelope<ComputerAccessRequestDto>) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: RuntimeEventEnvelope<ConnectionState>) => void): Unsubscribe;
  audio: NativeAudioApi;
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
  terminal: {
    list: (scope) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'list', scope }),
    create: (scope) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'create', scope }),
    input: (terminalId, data) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'input', terminalId, data }),
    resize: (terminalId, cols, rows) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'resize', terminalId, cols, rows }),
    close: (terminalId) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'close', terminalId }),
    acknowledge: (terminalId, sequence) => ipcRenderer.invoke(CH_TERMINAL_REQUEST, { kind: 'acknowledge', terminalId, sequence }),
    onEvent: (callback) => subscribe(CH_TERMINAL_EVENT, callback),
  },
  getPathForFile: (file) => webUtils.getPathForFile(file),
  platform: process.platform,
  isElectron: true,
  bootstrap: () => ipcRenderer.invoke(CH_BOOTSTRAP) as Promise<BootstrapState>,
  settings: () => ipcRenderer.invoke(CH_SETTINGS_GET) as Promise<PublicSettings>,
  openSettingsFile: (path) => ipcRenderer.invoke(CH_SETTINGS_FILE_OPEN, path) as Promise<void>,
  updateSettings: (patch) => ipcRenderer.invoke(CH_SETTINGS_UPDATE, patch) as Promise<PublicSettings>,
  pickWorkspace: () => ipcRenderer.invoke(CH_WORKSPACE_PICK) as Promise<WorkspaceMetadata | null>,
  setWorkspace: (path) => ipcRenderer.invoke(CH_WORKSPACE_SET, path) as Promise<WorkspaceMetadata>,
  removeProject: (path) => ipcRenderer.invoke(CH_PROJECT_REMOVE, path) as Promise<BootstrapState>,
  setSessionPinned: (session, pinned) => ipcRenderer.invoke(CH_SESSION_PIN_SET, session, pinned) as Promise<PublicSettings>,
  searchWorkspaceFiles: (query) => ipcRenderer.invoke(CH_WORKSPACE_FILES_SEARCH, query) as Promise<WorkspaceFileSearchResult>,
  previewWorkspaceFile: (sessionId, path) => ipcRenderer.invoke(CH_WORKSPACE_FILE_PREVIEW, sessionId, path) as Promise<WorkspaceFilePreview>,
  providerCredentials: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIALS_GET, providerId) as Promise<ProviderCredentialMetadata[]>,
  setProviderCredential: (providerId, credential) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_SET, providerId, credential) as Promise<ProviderCredentialUpdate>,
  clearProviderCredential: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_CLEAR, providerId) as Promise<ProviderCredentialMetadata>,
  testProviderConnection: (providerId, credentialOverride) => ipcRenderer.invoke(CH_PROVIDER_CONNECTION_TEST, providerId, credentialOverride) as Promise<ProviderConnectionTestResult>,
  pluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_GET, pluginId, key) as Promise<PluginSecretMetadata>,
  setPluginSecret: (pluginId, key, secret) => ipcRenderer.invoke(CH_PLUGIN_SECRET_SET, pluginId, key, secret) as Promise<PluginSecretMetadata>,
  clearPluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_CLEAR, pluginId, key) as Promise<PluginSecretMetadata>,
  restartBridge: (sessionId) => ipcRenderer.invoke(CH_BRIDGE_RESTART, sessionId) as Promise<void>,
  diagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_GET) as Promise<DiagnosticEntry[]>,
  copyDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_COPY) as Promise<void>,
  copyText: (text) => ipcRenderer.invoke(CH_CLIPBOARD_WRITE_TEXT, text) as Promise<void>,
  exportDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_EXPORT) as Promise<string | null>,
  listProjectSessions: (projectPath) => ipcRenderer.invoke(CH_PROJECT_SESSIONS_LIST, projectPath) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  newSession: (projectPath, model) => ipcRenderer.invoke(CH_SESSION_NEW, projectPath, model) as Promise<BootstrapState>,
  openSession: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId) as Promise<BootstrapState>,
  preflightSessionArchive: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_ARCHIVE_PREFLIGHT, projectPath, sessionId) as Promise<CronJobDto[]>,
  archiveSession: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_ARCHIVE, projectPath, sessionId) as Promise<BootstrapState>,
  clearSession: (sessionId) => ipcRenderer.invoke(CH_SESSION_CLEAR, sessionId) as Promise<void>,
  sendPrompt: (sessionId, text, images) => ipcRenderer.invoke(CH_SEND_PROMPT, sessionId, text, images ?? []) as Promise<void>,
  approve: (sessionId, requestId, response) => ipcRenderer.invoke(CH_APPROVE, sessionId, requestId, response) as Promise<void>,
  deny: (sessionId, requestId) => ipcRenderer.invoke(CH_DENY, sessionId, requestId) as Promise<void>,
  approveComputerAccess: (sessionId, requestId, response) => ipcRenderer.invoke(CH_APPROVE_COMPUTER_ACCESS, sessionId, requestId, response) as Promise<void>,
  denyComputerAccess: (sessionId, requestId) => ipcRenderer.invoke(CH_DENY_COMPUTER_ACCESS, sessionId, requestId) as Promise<void>,
  answerAskUserQuestion: (sessionId, requestId, answers) => ipcRenderer.invoke(CH_ANSWER_ASK_USER_QUESTION, sessionId, requestId, answers) as Promise<void>,
  cancelAskUserQuestion: (sessionId, requestId) => ipcRenderer.invoke(CH_CANCEL_ASK_USER_QUESTION, sessionId, requestId) as Promise<void>,
  openSystemSettings: (pane) => ipcRenderer.invoke(CH_OPEN_SYSTEM_SETTINGS, pane) as Promise<void>,
  microphoneAccess: () => ipcRenderer.invoke(CH_MICROPHONE_ACCESS_GET) as Promise<MicrophonePermissionStatus>,
  cancel: (sessionId, turnId) => ipcRenderer.invoke(CH_CANCEL, sessionId, turnId) as Promise<void>,
  command: (sessionId, command) => ipcRenderer.invoke(CH_COMMAND, sessionId, command) as Promise<void>,
  connectionState: (sessionId) => ipcRenderer.invoke(CH_CONNECTION_STATE, sessionId) as Promise<ConnectionState>,
  onEvent: (callback) => subscribeRuntimeEvents(callback),
  onPermission: (callback) => subscribe(CH_PERMISSION, callback),
  onComputerAccess: (callback) => subscribe(CH_COMPUTER_ACCESS, callback),
  onConnectionStateChanged: (callback) => subscribe(CH_STATE_CHANGED, callback),
  audio: {
    request: (command) => ipcRenderer.invoke(CH_NATIVE_AUDIO_REQUEST, command) as Promise<NativeAudioCommandResult>,
    onEvent: (callback) => subscribe(CH_NATIVE_AUDIO_EVENT, callback),
    executeEngineRequest: (sessionId, op) => (
      ipcRenderer.invoke(CH_NATIVE_AUDIO_ENGINE_REQUEST, sessionId, op) as Promise<AudioResultDto>
    ),
  },
};

contextBridge.exposeInMainWorld('lingxi', api);
