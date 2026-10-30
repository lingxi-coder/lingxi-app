import { CH_LOCAL_APPS, CH_LOCAL_APPS_CHANGED, type LocalAppsApi, type LocalAppEditRequest } from '../shared/localApps.js';
import { DESKTOP_BRIDGE_GLOBAL } from '../shared/desktopBridge.js';
import { CH_APPLICATION_ICON, type ApplicationIconApi } from '../shared/applicationIcon.js';
import { CH_DESKTOP_UPDATES, CH_DESKTOP_UPDATES_CHANGED, type DesktopUpdatesApi } from '../shared/desktopUpdates.js';
import { CH_INTERFACE_LOCALE } from '../shared/interfaceLocale.js';
import { CH_COMPUTER_SETTINGS, CH_COMPUTER_SETTINGS_CHANGED, type ComputerSettingsApi } from '../shared/computerSettings.js';
import { ModUiIpcQueue } from '../shared/modUiIpcQueue';
import type { AccountApi, AccountState } from '../shared/account.js';
import { CH_APPLICATION_MENU, type ApplicationMenuAction } from '../shared/applicationMenu.js';
import { CH_PROJECT_APPS_DEFAULT, CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK, type ProjectCreationPurpose, type ProjectFolderSelection } from '../shared/projectCreation.js';
import { CH_REALTIME_AUDIO_COMMAND, CH_REALTIME_AUDIO_STATE, type NativeRealtimeAudioCommand, type NativeRealtimeAudioState } from '../shared/realtimeAudio.js';
import { CH_SCHEDULED, type ScheduledApi } from '../shared/scheduled.js';
import {
  CH_REMOTE_AT_STARTUP_GET,
  CH_REMOTE_AT_STARTUP_SET,
  CH_REMOTE_DEVICE_REVOKE,
  CH_REMOTE_DEVICES_LIST,
  CH_REMOTE_ENABLED_SET,
  CH_REMOTE_PROMPT_ECHO,
  CH_REMOTE_SESSION_SET,
  CH_REMOTE_SESSIONS_EVENT,
  CH_REMOTE_SESSIONS_GET,
  CH_REMOTE_STATUS_EVENT,
  CH_REMOTE_STATUS_GET,
  type RemoteControlApi,
} from '../shared/remoteControl.js';
import type { VisualizationGuestEvent, VisualizationMount, VisualizationReference, VisualizationStateWrite, VisualizationTheme } from '../shared/visualization.js';
import { CH_GIT_REQUEST, CH_GIT_EVENT } from '../shared/git.js';
import type { GitApi } from '../shared/git.js';
import { CH_TERMINAL_REQUEST, CH_TERMINAL_EVENT } from '../shared/terminal.js';
import type { TerminalApi } from '../shared/terminal.js';
import type { CronJobDto } from '@lingxi/bridge-client';
import { contextBridge, ipcRenderer, webUtils, type IpcRendererEvent } from 'electron';
import type {
  AskUserQuestionRequestDto,
  AudioOperationDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  ImageRefDto,
  NativeUiControlRequest,
  NativeUiControlResponseFor,
  PermissionResponseDto,
  UiClientFrameEventDto,
  UiClientOperation,
  UiClientOperationResponseFor,
  UiControlCallResultDto,
  UiInvalidateEventDto,
} from '@lingxi/bridge-client';
import type { HostPermissionRequest } from '../shared/permission.js';
import { createRuntimeEventReplayBuffer, type SequencedRuntimeEventEnvelope } from './event-replay.js';
import type { AllowedClientCommand } from '../shared/clientCommands.js';
import {
  CH_NATIVE_AUDIO_CANCEL,
  CH_NATIVE_AUDIO_EVENT,
  CH_NATIVE_AUDIO_FINISH_LISTEN,
  CH_NATIVE_AUDIO_OPERATION,
  CH_NATIVE_AUDIO_REQUEST,
} from '../shared/nativeAudio.js';
import type {
  NativeAudioCommand,
  NativeAudioCommandResult,
  NativeAudioEvent,
  NativeAudioOperationResponse,
  NativeAudioResponse,
} from '../shared/nativeAudio.js';
import type { PublicSettings, SessionPinInput, SessionRef } from '../shared/settings.js';
import type { MicrophonePermissionStatus } from '../shared/microphoneAccess.js';
import type { AudioConfigurationV4 } from '../shared/generatedAudioConfiguration.js';
import { CH_SESSION_WORKSPACE, type SessionCatalogRow, type SessionCatalogScope, type SessionOpenSelection, type SessionWorkspaceOperation } from '../shared/sessionWorkspace.js';
import type { PluginSecretCleanupResult, PluginSecretMetadata, WorkspaceFilePreview } from '../main/host.js';

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
const CH_MOD_UI_CONTROL = 'lingxi:modUi:control';
const CH_MOD_UI_OPERATION = 'lingxi:modUi:operation';
const CH_MOD_UI_FRAME = 'lingxi:modUi:frame';
const CH_MOD_UI_INVALIDATE = 'lingxi:modUi:invalidate';
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
const CH_PLUGIN_SECRET_CLEANUP = 'lingxi:plugin-secret:cleanup-after-uninstall';
const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
const modUiIpcQueue = new ModUiIpcQueue();
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
const CH_SESSION_TOUCH = 'lingxi:session:touch';
const CH_SESSION_RENAME = 'lingxi:session:rename';
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
  sessions: SessionCatalogRow[];
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
export interface CredentialMetadata { configured: boolean; encryptionAvailable: boolean; credentialPreview?: string; runtimeOnly?: true; environmentOverride?: true }
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
  scheduledWorkspace?: string;
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
export interface WorkspaceFileSearchResult { files: string[]; directories?: string[]; truncated: boolean }

export type Unsubscribe = () => void;
export interface NativeAudioApi {
  realtime(command: NativeRealtimeAudioCommand): Promise<void>;
  onRealtimeState(callback: (state: NativeRealtimeAudioState) => void): () => void;
  request(command: NativeAudioCommand): Promise<NativeAudioCommandResult>;
  execute(operation: AudioOperationDto, configurationRevision?: number, configurationOverride?: AudioConfigurationV4): Promise<NativeAudioOperationResponse>;
  cancel(): Promise<void>;
  finishListen(): Promise<void>;
  onEvent(cb: (event: NativeAudioEvent) => void): Unsubscribe;
}

export interface ModUiApi {
  control<T extends NativeUiControlRequest>(
    sessionId: string,
    request: T,
  ): Promise<UiControlCallResultDto<NativeUiControlResponseFor<T>>>;
  operation<T extends UiClientOperation>(
    sessionId: string,
    operation: T,
  ): Promise<UiClientOperationResponseFor<T>>;
  onFrame(callback: (event: UiClientFrameEventDto) => void): Unsubscribe;
  onInvalidate(callback: (event: UiInvalidateEventDto) => void): Unsubscribe;
}

/** The macOS System Settings deep links this app opens: the computer-access TCC panel's two panes, plus the voice settings page's `microphone` row. */
export type SystemSettingsPane = 'accessibility' | 'screen_recording' | 'microphone' | 'speech_recognition';

export interface VisualizationApi {
  mount(sessionId: string, reference: VisualizationReference, theme: VisualizationTheme, locale: string, expanded: boolean): Promise<VisualizationMount | null>;
  writeState(sessionId: string, token: string, generation: number, baseVersion: number, modelContent: string, privateContent: string): Promise<VisualizationStateWrite>;
  unmount(sessionId: string, token: string): Promise<void>;
  onGuestEvent(callback: (event: VisualizationGuestEvent) => void): () => void;
}

export interface LingxiApi {
  computerSettings: ComputerSettingsApi;
  applicationIcon: ApplicationIconApi;
  desktopUpdates: DesktopUpdatesApi;
  localApps: LocalAppsApi;
  onLocalAppEdit(listener: (app: LocalAppEditRequest) => void): () => void;
  scheduled: ScheduledApi;
  remoteControl: RemoteControlApi;
  git?: GitApi;
  terminal: TerminalApi;
  getPathForFile(file: File): string;
  platform: NodeJS.Platform;
  isElectron: true;
  account: AccountApi;
  bootstrap(): Promise<BootstrapState>;
  settings(): Promise<PublicSettings>;
  openSettingsFile(path: string): Promise<void>;
  updateSettings(patch: {
    telemetryEnabled?: boolean;
    theme?: 'dark' | 'light' | 'system';
    collapseThoughtsByDefault?: boolean;
    model?: string | null;
    apiBaseUrl?: string | null;
    voice?: unknown;
    voiceRevision?: number;
    notifications?: unknown;
    modelPickerVisibility?: unknown;
    sidebar?: unknown;
  }): Promise<PublicSettings>;
  pickWorkspace(): Promise<WorkspaceMetadata | null>;
  pickProjectFolder(purpose?: ProjectCreationPurpose): Promise<ProjectFolderSelection | null>;
  defaultAppProjectFolder(): Promise<ProjectFolderSelection | null>;
  createProject(selectionId: string, directoryName?: string): Promise<WorkspaceMetadata>;
  setWorkspace(path: string): Promise<WorkspaceMetadata>;
  removeProject(path: string): Promise<BootstrapState>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<PublicSettings>;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  previewWorkspaceFile(sessionId: string, path: string): Promise<WorkspaceFilePreview>;
  providerCredentials(providerId?: string): Promise<ProviderCredentialMetadata[]>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  loginCodex(): Promise<ProviderCredentialUpdate>;
  cancelCodexLogin(): Promise<void>;
  testProviderConnection(providerId: string, credentialOverride?: string): Promise<ProviderConnectionTestResult>;
  pluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretMetadata>;
  clearPluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  cleanupPluginSecretsAfterUninstall?(pluginId: string, lastScopeRemoved: true): Promise<PluginSecretCleanupResult>;
  restartBridge(sessionId: string): Promise<void>;
  diagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  setInterfaceLocale(locale: string, followSystem: boolean): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  listProjectSessions(projectPath: string, scope?: SessionCatalogScope): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  updateSessionWorkspace(sessionId: string, operation: SessionWorkspaceOperation): Promise<BootstrapState | null>;
  newSession(projectPath: string, model?: string): Promise<BootstrapState>;
  openSession(projectPath: string, sessionId: string, selection?: SessionOpenSelection): Promise<BootstrapState>;
  preflightSessionArchive(projectPath: string, sessionId: string): Promise<CronJobDto[]>;
  archiveSession(projectPath: string, sessionId: string): Promise<BootstrapState>;
  touchSession(projectPath: string, sessionId: string): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  renameSession(projectPath: string, sessionId: string, title: string): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  clearSession(sessionId: string, name?: string): Promise<void>;
  sendPrompt(sessionId: string, text: string, images?: ImageRefDto[], turnId?: number, visualizationContext?: VisualizationReference): Promise<void>;
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
  onPermission(cb: (request: RuntimeEventEnvelope<HostPermissionRequest>) => void): Unsubscribe;
  onComputerAccess(cb: (request: RuntimeEventEnvelope<ComputerAccessRequestDto>) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: RuntimeEventEnvelope<ConnectionState>) => void): Unsubscribe;
  deepLinks: {
    copySessionLink(projectPath: string, sessionId: string, messageUuid?: string): Promise<void>;
    next(): Promise<{ id: string; target: import('../../../../packages/deep-links/index').DeepLinkTarget; projectPath?: string } | null>;
    acknowledge(id: string, error?: string): Promise<void>;
    onAvailable(callback: () => void): Unsubscribe;
  };
  onApplicationMenu(cb: (action: ApplicationMenuAction) => void): Unsubscribe;
  audio: NativeAudioApi;
  visualization: VisualizationApi;
  modUi: ModUiApi;
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
  computerSettings: {
    execute: command => ipcRenderer.invoke(CH_COMPUTER_SETTINGS, command),
    onChanged: listener => {
      const callback = (_event: IpcRendererEvent, snapshot: Parameters<typeof listener>[0]) => listener(snapshot);
      ipcRenderer.on(CH_COMPUTER_SETTINGS_CHANGED, callback);
      return () => ipcRenderer.removeListener(CH_COMPUTER_SETTINGS_CHANGED, callback);
    },
  },
  applicationIcon: { get: bundleId => ipcRenderer.invoke(CH_APPLICATION_ICON, bundleId) },
  desktopUpdates: {
    getState: () => ipcRenderer.invoke(CH_DESKTOP_UPDATES, 'get'),
    download: () => ipcRenderer.invoke(CH_DESKTOP_UPDATES, 'download'),
    remindLater: () => ipcRenderer.invoke(CH_DESKTOP_UPDATES, 'later'),
    onChanged: listener => subscribe(CH_DESKTOP_UPDATES_CHANGED, listener),
  },
  localApps: {
    list: async () => (await ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'list' })).apps,
    remoteSettings: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'remote-settings' }),
    remoteConfiguration: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'remote-configuration' }),
    setRemoteAccess: (id, enabled) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'remote-access', id, enabled }),
    open: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'open', id }),
    stop: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'stop', id }),
    restart: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'restart', id }),
    repair: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'repair', id }),
    logs: async id => (await ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'logs', id })).logs,
    versions: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'versions', id }),
    useVersion: (id, revision, expectedCurrentRevision) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'useVersion', id, revision, expectedCurrentRevision }),
    openData: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'openData', id }),
    remove: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'remove', id }),
    setBackground: (id, enabled) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'background', id, enabled }),
    revoke: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'revoke', id }),
    edit: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'edit', id }),
    bindAuthoring: (id, session) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'bindAuthoring', id, session }),
    prepareAuthoring: session => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'prepareAuthoring', session }),
    marketBrowse: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'marketBrowse' }),
    marketInstallLink: remoteId => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'marketInstallLink', remoteId }),
    marketInstall: (remoteId, id, expectedRevision) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'marketInstall', remoteId, id, expectedRevision }),
    marketPublish: (id, metadata, expectedRevision) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'marketPublish', id, metadata, expectedRevision }),
    marketManage: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'marketManage' }),
    restoreSource: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'restoreSource', id }),
    remix: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'remix', id }),
    agentCatalog: id => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'agent-catalog', id }),
    setAgentSettings: (id, settings) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'agent-settings', id, settings }),
    agentQuestions: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'agent-questions' }),
    answerAgentQuestion: (id, answers) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'agent-answer', questionId: id, answers }),
    onChanged: callback => subscribe(CH_LOCAL_APPS_CHANGED, callback),
  },
  onLocalAppEdit: callback => subscribe('lingxi:local-app:edit', callback),
  git: {
    request: (scope, request) => ipcRenderer.invoke(CH_GIT_REQUEST, scope, request),
    onChanged: (callback) => subscribe(CH_GIT_EVENT, callback),
  },
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
  account: {
    request: command => ipcRenderer.invoke('lingxi:account', command) as Promise<AccountState>,
    onChanged: callback => {
      const listener = (_event: Electron.IpcRendererEvent, state: AccountState) => callback(state);
      ipcRenderer.on('lingxi:account:changed', listener);
      return () => { ipcRenderer.removeListener('lingxi:account:changed', listener); };
    },
  },
  scheduled: {
    context: (scopeId) => ipcRenderer.invoke(CH_SCHEDULED, { action: 'context', scopeId }),
    scopes: () => ipcRenderer.invoke(CH_SCHEDULED, { action: 'scopes' }),
    manage: (scopeId, request) => ipcRenderer.invoke(CH_SCHEDULED, { action: 'manage', scopeId, request }),
    openSession: (scopeId, sessionId) => ipcRenderer.invoke(CH_SCHEDULED, { action: 'open', scopeId, sessionId }),
  } satisfies ScheduledApi,
  remoteControl: {
    status: () => ipcRenderer.invoke(CH_REMOTE_STATUS_GET),
    onStatus: callback => subscribe(CH_REMOTE_STATUS_EVENT, callback),
    setEnabled: enabled => ipcRenderer.invoke(CH_REMOTE_ENABLED_SET, enabled),
    atStartup: () => ipcRenderer.invoke(CH_REMOTE_AT_STARTUP_GET),
    setAtStartup: enabled => ipcRenderer.invoke(CH_REMOTE_AT_STARTUP_SET, enabled),
    devices: () => ipcRenderer.invoke(CH_REMOTE_DEVICES_LIST),
    revokeDevice: deviceId => ipcRenderer.invoke(CH_REMOTE_DEVICE_REVOKE, deviceId),
    sessions: () => ipcRenderer.invoke(CH_REMOTE_SESSIONS_GET),
    setSession: (sessionId, enabled) => ipcRenderer.invoke(CH_REMOTE_SESSION_SET, sessionId, enabled),
    onSessions: callback => subscribe(CH_REMOTE_SESSIONS_EVENT, callback),
    onPromptEcho: callback => subscribe(CH_REMOTE_PROMPT_ECHO, callback),
  } satisfies RemoteControlApi,
  bootstrap: () => ipcRenderer.invoke(CH_BOOTSTRAP) as Promise<BootstrapState>,
  settings: () => ipcRenderer.invoke(CH_SETTINGS_GET) as Promise<PublicSettings>,
  openSettingsFile: (path) => ipcRenderer.invoke(CH_SETTINGS_FILE_OPEN, path) as Promise<void>,
  updateSettings: (patch) => ipcRenderer.invoke(CH_SETTINGS_UPDATE, patch) as Promise<PublicSettings>,
  pickWorkspace: () => ipcRenderer.invoke(CH_WORKSPACE_PICK) as Promise<WorkspaceMetadata | null>,
  pickProjectFolder: (purpose) => ipcRenderer.invoke(CH_PROJECT_FOLDER_PICK, purpose) as Promise<ProjectFolderSelection | null>,
  defaultAppProjectFolder: () => ipcRenderer.invoke(CH_PROJECT_APPS_DEFAULT) as Promise<ProjectFolderSelection | null>,
  createProject: (selectionId, directoryName) => ipcRenderer.invoke(CH_PROJECT_CREATE, selectionId, directoryName) as Promise<WorkspaceMetadata>,
  setWorkspace: (path) => ipcRenderer.invoke(CH_WORKSPACE_SET, path) as Promise<WorkspaceMetadata>,
  removeProject: (path) => ipcRenderer.invoke(CH_PROJECT_REMOVE, path) as Promise<BootstrapState>,
  setSessionPinned: (session, pinned) => ipcRenderer.invoke(CH_SESSION_PIN_SET, session, pinned) as Promise<PublicSettings>,
  searchWorkspaceFiles: (query) => ipcRenderer.invoke(CH_WORKSPACE_FILES_SEARCH, query) as Promise<WorkspaceFileSearchResult>,
  previewWorkspaceFile: (sessionId, path) => ipcRenderer.invoke(CH_WORKSPACE_FILE_PREVIEW, sessionId, path) as Promise<WorkspaceFilePreview>,
  loginCodex: () => ipcRenderer.invoke('lingxi:codex:login') as Promise<ProviderCredentialUpdate>,
  cancelCodexLogin: () => ipcRenderer.invoke('lingxi:codex:cancel') as Promise<void>,
  providerCredentials: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIALS_GET, providerId) as Promise<ProviderCredentialMetadata[]>,
  setProviderCredential: (providerId, credential) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_SET, providerId, credential) as Promise<ProviderCredentialUpdate>,
  clearProviderCredential: (providerId) => ipcRenderer.invoke(CH_PROVIDER_CREDENTIAL_CLEAR, providerId) as Promise<ProviderCredentialMetadata>,
  testProviderConnection: (providerId, credentialOverride) => ipcRenderer.invoke(CH_PROVIDER_CONNECTION_TEST, providerId, credentialOverride) as Promise<ProviderConnectionTestResult>,
  pluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_GET, pluginId, key) as Promise<PluginSecretMetadata>,
  setPluginSecret: (pluginId, key, secret) => ipcRenderer.invoke(CH_PLUGIN_SECRET_SET, pluginId, key, secret) as Promise<PluginSecretMetadata>,
  clearPluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_CLEAR, pluginId, key) as Promise<PluginSecretMetadata>,
  cleanupPluginSecretsAfterUninstall: (pluginId, lastScopeRemoved) => ipcRenderer.invoke(CH_PLUGIN_SECRET_CLEANUP, pluginId, lastScopeRemoved) as Promise<PluginSecretCleanupResult>,
  restartBridge: (sessionId) => ipcRenderer.invoke(CH_BRIDGE_RESTART, sessionId) as Promise<void>,
  diagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_GET) as Promise<DiagnosticEntry[]>,
  copyDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_COPY) as Promise<void>,
  copyText: (text) => ipcRenderer.invoke(CH_CLIPBOARD_WRITE_TEXT, text) as Promise<void>,
  setInterfaceLocale: (locale, followSystem) => ipcRenderer.invoke(CH_INTERFACE_LOCALE, locale, followSystem) as Promise<void>,
  exportDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_EXPORT) as Promise<string | null>,
  listProjectSessions: (projectPath, scope) => ipcRenderer.invoke(CH_PROJECT_SESSIONS_LIST, projectPath, scope) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  updateSessionWorkspace: (sessionId, operation) => ipcRenderer.invoke(CH_SESSION_WORKSPACE, sessionId, operation) as Promise<BootstrapState | null>,
  newSession: (projectPath, model) => ipcRenderer.invoke(CH_SESSION_NEW, projectPath, model) as Promise<BootstrapState>,
  openSession: (projectPath, sessionId, selection) => (selection === undefined
    ? ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId)
    : ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId, selection)) as Promise<BootstrapState>,
  preflightSessionArchive: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_ARCHIVE_PREFLIGHT, projectPath, sessionId) as Promise<CronJobDto[]>,
  archiveSession: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_ARCHIVE, projectPath, sessionId) as Promise<BootstrapState>,
  touchSession: (projectPath, sessionId) => ipcRenderer.invoke(CH_SESSION_TOUCH, projectPath, sessionId) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  renameSession: (projectPath, sessionId, title) => ipcRenderer.invoke(CH_SESSION_RENAME, projectPath, sessionId, title) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  clearSession: (sessionId, name) => ipcRenderer.invoke(CH_SESSION_CLEAR, sessionId, name) as Promise<void>,
  sendPrompt: (sessionId, text, images, turnId, visualizationContext) => ipcRenderer.invoke(CH_SEND_PROMPT, sessionId, text, images ?? [], turnId, visualizationContext) as Promise<void>,
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
  deepLinks: {
    copySessionLink: (projectPath, sessionId, messageUuid) => ipcRenderer.invoke('lingxi:deep-link:session-url', projectPath, sessionId, messageUuid),
    next: () => ipcRenderer.invoke('lingxi:deep-link:next'),
    acknowledge: (id, error) => ipcRenderer.invoke('lingxi:deep-link:ack', id, error),
    onAvailable: callback => subscribe('lingxi:deep-link:available', callback),
  },
  onApplicationMenu: (callback) => subscribe(CH_APPLICATION_MENU, callback),
  modUi: {
    control: <T extends NativeUiControlRequest>(sessionId: string, request: T) =>
      modUiIpcQueue.request(
        request.subtype === 'ui_render'
          ? JSON.stringify([sessionId, request.surface, request.component, request.instance_id, request.client_id])
          : undefined,
        request,
        () => ipcRenderer.invoke(CH_MOD_UI_CONTROL, sessionId, request),
      ) as Promise<UiControlCallResultDto<NativeUiControlResponseFor<T>>>,
    operation: <T extends UiClientOperation>(sessionId: string, operation: T) =>
      modUiIpcQueue.request(undefined, operation, () => ipcRenderer.invoke(CH_MOD_UI_OPERATION, sessionId, operation)) as Promise<UiClientOperationResponseFor<T>>,
    onFrame: (callback) => subscribe(CH_MOD_UI_FRAME, (envelope: RuntimeEventEnvelope<Omit<UiClientFrameEventDto, 'sessionId'>>) => {
      callback({ sessionId: envelope.sessionId, ...envelope.event });
    }),
    onInvalidate: (callback) => subscribe(CH_MOD_UI_INVALIDATE, (envelope: RuntimeEventEnvelope<Omit<UiInvalidateEventDto, 'sessionId'>>) => {
      callback({ sessionId: envelope.sessionId, ...envelope.event });
    }),
  },
  visualization: {
    mount: (sessionId, reference, theme, locale, expanded) => ipcRenderer.invoke('lingxi:visualization:mount', sessionId, reference, theme, locale, expanded) as Promise<VisualizationMount | null>,
    writeState: (sessionId, token, generation, baseVersion, modelContent, privateContent) =>
      ipcRenderer.invoke('lingxi:visualization:writeState', sessionId, token, generation, baseVersion, modelContent, privateContent) as Promise<VisualizationStateWrite>,
    unmount: (sessionId, token) => ipcRenderer.invoke('lingxi:visualization:unmount', sessionId, token) as Promise<void>,
    onGuestEvent: (callback) => subscribe('lingxi:visualization:guestEvent', callback),
  },
  audio: {
    realtime: (command) => ipcRenderer.invoke(CH_REALTIME_AUDIO_COMMAND, command) as Promise<void>,
    onRealtimeState: (callback) => subscribe(CH_REALTIME_AUDIO_STATE, callback),    request: (command) => ipcRenderer.invoke(CH_NATIVE_AUDIO_REQUEST, command) as Promise<NativeAudioCommandResult>,
    execute: (operation, configurationRevision, configurationOverride) => ipcRenderer.invoke(CH_NATIVE_AUDIO_OPERATION, operation, configurationRevision, configurationOverride) as Promise<NativeAudioOperationResponse>,
    cancel: () => ipcRenderer.invoke(CH_NATIVE_AUDIO_CANCEL) as Promise<void>,
    finishListen: () => ipcRenderer.invoke(CH_NATIVE_AUDIO_FINISH_LISTEN) as Promise<void>,
    onEvent: (callback) => subscribe(CH_NATIVE_AUDIO_EVENT, callback),
  },
};

contextBridge.exposeInMainWorld(DESKTOP_BRIDGE_GLOBAL, api);
