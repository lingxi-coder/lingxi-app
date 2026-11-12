import { CH_BOTS_MARKET } from '../shared/botMarket.js';
import { CH_DECISION, CH_DECISION_CHANGED, type DecisionApi } from '../shared/decision.js';
import { DESKTOP_BRIDGE_GLOBAL } from '../shared/desktopBridge.js';
import { CH_SESSION_RECOVERY, type SessionRecoveryDiagnostic } from '../shared/sessionRecovery.js';
import type { ProviderAccountSnapshot } from '../shared/providerAccounts.js';
import { CH_SESSION_HISTORY, type TranscriptHistoryCursor, type TranscriptHistoryPage } from '../shared/sessionHistory.js';
import type { ProviderAccountsApi } from '../shared/providerAccounts.js';
import { CH_LOCAL_APPS, CH_LOCAL_APPS_CHANGED, type LocalAppsApi, type LocalAppEditRequest } from '../shared/localApps.js';
import { CH_IMAGE_SAVE, type ImageSaveRequest } from '../shared/imageSave.js';
import { CH_APPLICATION_ICON, type ApplicationIconApi } from '../shared/applicationIcon.js';
import { CH_DESKTOP_UPDATES, CH_DESKTOP_UPDATES_CHANGED, type DesktopUpdatesApi } from '../shared/desktopUpdates.js';
import { CH_INTERFACE_LOCALE } from '../shared/interfaceLocale.js';
import { CH_COMPUTER_SETTINGS, CH_COMPUTER_SETTINGS_CHANGED, type ComputerSettingsApi } from '../shared/computerSettings.js';
import { ModUiIpcQueue } from '../shared/modUiIpcQueue';
import type { AccountApi, AccountState } from '../shared/account.js';
import type { DesktopTeamsApi } from '../../../../packages/teams';
import type { DesktopFriendsApi } from '../../../../packages/teams/friends';
import { CH_APPLICATION_MENU, type ApplicationMenuAction } from '../shared/applicationMenu.js';
import { CH_PROJECT_APPS_DEFAULT, CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK, type ProjectCreationPurpose, type ProjectFolderSelection } from '../shared/projectCreation.js';
import { CH_REALTIME_AUDIO_COMMAND, CH_REALTIME_AUDIO_STATE, type NativeRealtimeAudioCommand, type NativeRealtimeAudioState } from '../shared/realtimeAudio.js';
import { CH_SCHEDULED, type ScheduledApi } from '../shared/scheduled.js';
import {
  CH_BOTS_AVAILABILITY,
  CH_BOTS_CREATE,
  CH_BOTS_DELETE,
  CH_BOTS_GET,
  CH_BOTS_LIST,
  CH_BOTS_LOGIN_ITEM_GET,
  CH_BOTS_LOGIN_ITEM_SET,
  CH_BOTS_LOGIN_ITEMS_OPEN,
  CH_BOTS_MEMORY_DELETE,
  CH_BOTS_MEMORY_LIST,
  CH_BOTS_MEMORY_READ,
  CH_BOTS_MEMORY_WRITE,
  CH_BOTS_PERSONA_GET,
  CH_BOTS_PERSONA_SET,
  CH_BOTS_PROMPT_SENT,
  CH_BOTS_SESSION_OPEN,
  CH_BOTS_SESSION_SHOWN,
  CH_BOTS_SESSIONS_LIST,
  CH_BOTS_SKILL_DELETE,
  CH_BOTS_SKILL_IMPORT,
  CH_BOTS_SKILLS_IMPORTABLE,
  CH_BOTS_SKILLS_LIST,
  CH_BOTS_TASK_CANCEL,
  CH_BOTS_TASKS_CHANGED,
  CH_BOTS_TASKS_LIST,
  CH_BOTS_UPDATE,
  type BotsApi,
} from '../shared/bots.js';
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
import { createRuntimeEventReplayBuffer, requestRuntimeEventReplay, type SequencedRuntimeEventEnvelope } from './event-replay.js';
import type { AllowedClientCommand } from '../shared/clientCommands.js';
import {
  CH_NATIVE_AUDIO_CANCEL,
  CH_NATIVE_AUDIO_EVENT,
  CH_NATIVE_AUDIO_FINISH_LISTEN,
  CH_NATIVE_AUDIO_FLOW_LEASE,
  CH_NATIVE_AUDIO_OPERATION,
  CH_NATIVE_AUDIO_REQUEST,
} from '../shared/nativeAudio.js';
import type {
  NativeAudioCommand,
  NativeAudioCommandResult,
  NativeAudioEvent,
  NativeAudioFlowLease,
  NativeAudioOperationResponse,
  NativeAudioResponse,
} from '../shared/nativeAudio.js';
import type { PublicSettings, SessionPinInput, SessionRef } from '../shared/settings.js';
import type { MicrophonePermissionStatus } from '../shared/microphoneAccess.js';
import type { AudioConfigurationV4 } from '../shared/generatedAudioConfiguration.js';
import { CH_SESSION_WORKSPACE, type SessionCatalogRow, type SessionCatalogScope, type SessionOpenSelection, type SessionMetadataSelection, type SessionWorkspaceOperation } from '../shared/sessionWorkspace.js';
import { CH_CONVERSATION_SHOWN, CH_CONVERSATION_SPACE, CH_SESSION_HANDOFF, CH_SESSION_HANDOFF_CANCEL, type ConversationSpace, type ConversationSpacesSnapshot } from '../shared/conversationSpaces.js';
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
const CH_SESSION_METADATA_CAPTURE = 'lingxi:session:metadata-capture';
const CH_SESSION_METADATA_RELEASE = 'lingxi:session:metadata-release';
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
  /** Set when the session is a Bot's. It is not a project's session and is listed with its Bot, not with the projects. */
  botId?: string;
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
export interface DiagnosticEntry {
  timestamp: string;
  level: 'info' | 'warn' | 'error';
  source: 'host' | 'bridge';
  message: string;
}
export interface BootstrapState {
  scheduledWorkspace?: string;
  conversationSpaces: ConversationSpacesSnapshot;
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  activeSession?: SessionRef;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  providerAccountSnapshot?: ProviderAccountSnapshot;
  credentialBrokerAvailable?: boolean;
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
}
export interface WorkspaceFileSearchResult { files: string[]; directories?: string[]; truncated: boolean }

export type Unsubscribe = () => void;
export interface NativeAudioApi {
  retainFlow(sessionId: string, leaseId: string, retain: boolean): Promise<void>;
  realtime(command: NativeRealtimeAudioCommand): Promise<void>;
  onRealtimeState(callback: (state: NativeRealtimeAudioState) => void): () => void;
  request(command: NativeAudioCommand): Promise<NativeAudioCommandResult>;
  execute(operation: AudioOperationDto, configurationRevision?: number, configurationOverride?: AudioConfigurationV4, flow?: NativeAudioFlowLease): Promise<NativeAudioOperationResponse>;
  cancel(flow?: NativeAudioFlowLease): Promise<void>;
  finishListen(flow?: NativeAudioFlowLease): Promise<void>;
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
  saveImage(request: ImageSaveRequest): Promise<boolean>;
  applicationIcon: ApplicationIconApi;
  desktopUpdates: DesktopUpdatesApi;
  localApps: LocalAppsApi;
  onLocalAppEdit(listener: (app: LocalAppEditRequest) => void): () => void;
  scheduled: ScheduledApi;
  remoteControl: RemoteControlApi;
  bots: BotsApi;
  git?: GitApi;
  terminal: TerminalApi;
  getPathForFile(file: File): string;
  platform: NodeJS.Platform;
  isElectron: true;
  account: AccountApi;
  teams: DesktopTeamsApi;
  friends: DesktopFriendsApi;
  bootstrap(): Promise<BootstrapState>;
  settings(): Promise<PublicSettings>;
  openSettingsFile(path: string): Promise<void>;
  updateSettings(patch: {
    telemetryEnabled?: boolean;
    theme?: 'dark' | 'light' | 'system';
    collapseThoughtsByDefault?: boolean;
    botsKeepAwake?: boolean;
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
  decision: DecisionApi;
  providerAccounts: ProviderAccountsApi;
  providerSettings(patch?: Record<string, unknown>, expected?: Record<string, unknown>): Promise<import('../shared/providerAccounts.js').HostProviderSettings>;
  pluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretMetadata>;
  clearPluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  cleanupPluginSecretsAfterUninstall?(pluginId: string, lastScopeRemoved: true): Promise<PluginSecretCleanupResult>;
  restartBridge(sessionId: string): Promise<void>;
  diagnostics(): Promise<DiagnosticEntry[]>;
  reportSessionRecovery(sample: SessionRecoveryDiagnostic): void;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  setInterfaceLocale(locale: string, followSystem: boolean): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  listProjectSessions(projectPath: string, scope?: SessionCatalogScope): Promise<ProjectSessionCatalogState & { projectPath: string; conversationSpaces: ConversationSpacesSnapshot }>;
  updateSessionWorkspace(sessionId: string, operation: SessionWorkspaceOperation): Promise<BootstrapState | null>;
  newSession(projectPath: string | null, model?: string, purpose?: import('../shared/settings.js').NewSessionPurpose): Promise<BootstrapState>;
  switchConversationSpace(mode: ConversationSpace): Promise<BootstrapState>;
  prepareSessionHandoff(sourceId: string, target: ConversationSpace, requestId: string, retry?: boolean): Promise<{ snapshot: BootstrapState; summary: string; requestId: string } | null>;
  cancelSessionHandoff(requestId: string): Promise<void>;
  onConversationShown(callback: (snapshot: BootstrapState) => void): Unsubscribe;
  openSession(projectPath: string, sessionId: string, selection?: SessionOpenSelection, linkedMode?: ConversationSpace, authoringAppId?: string, authoringVersion?: number): Promise<BootstrapState>;
  loadSessionHistory(sessionId: string, before: TranscriptHistoryCursor): Promise<TranscriptHistoryPage>;
  prepareSessionMetadata(projectPath: string, sessionId: string, selection?: SessionOpenSelection): Promise<SessionMetadataSelection>;
  releaseSessionMetadata(selection: SessionMetadataSelection): Promise<void>;
  preflightSessionArchive(projectPath: string, sessionId: string, selection: SessionMetadataSelection): Promise<CronJobDto[]>;
  archiveSession(projectPath: string, sessionId: string, selection: SessionMetadataSelection): Promise<BootstrapState>;
  touchSession(projectPath: string, sessionId: string, selection?: SessionOpenSelection): Promise<ProjectSessionCatalogState & { projectPath: string }>;
  renameSession(projectPath: string, sessionId: string, title: string, selection: SessionMetadataSelection): Promise<ProjectSessionCatalogState & { projectPath: string }>;
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

async function teamsRequest<T>(command: unknown): Promise<T> {
  const result = await ipcRenderer.invoke('lingxi:teams', command) as { value: T; error?: string };
  if (result.error) throw new Error(result.error);
  return result.value;
}
async function friendsRequest<T>(command: unknown): Promise<T> {
  const result = await ipcRenderer.invoke('lingxi:friends', command) as { value: T; error?: string };
  if (result.error) throw new Error(result.error);
  return result.value;
}
function subscribe<T>(channel: string, callback: (payload: T) => void): Unsubscribe {
  const listener = (_event: IpcRendererEvent, payload: T): void => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

/** One of the account's three Friends collections, as plain rows on every change; a failed watch is reported once and is over. */
function watchFriends<T>(uid: string, collection: 'friends' | 'blocks' | 'teamOffers', next: (rows: T[]) => void, failed: (code: string) => void): Unsubscribe {
  const leaseId = crypto.randomUUID();
  let closed = false;
  const stopRows = subscribe('lingxi:friends:rows', (event: { leaseId: string; rows: T[] }) => { if (event.leaseId === leaseId) next(event.rows); });
  const stopFailed = subscribe('lingxi:friends:failed', (event: { leaseId: string; code: string }) => { if (event.leaseId === leaseId) failed(event.code); });
  void friendsRequest({ type: 'watch', uid, collection, leaseId }).catch(error => { if (!closed) failed(error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE'); });
  return () => { closed = true; stopRows(); stopFailed(); void friendsRequest({ type: 'unwatch', uid, leaseId }).catch(() => undefined); };
}

const recoveryDocumentId = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
let recoverySubscriptionId = 0;
function reportSessionRecovery(sample: SessionRecoveryDiagnostic): void {
  const fields = Object.fromEntries(Object.entries(sample).filter(([, value]) => value !== undefined));
  // No reply is needed, and late shutdown samples must not invoke an IPC
  // handler that the main process has already removed.
  try { ipcRenderer.send(CH_SESSION_RECOVERY, { ...fields, documentId: recoveryDocumentId }); }
  catch { /* The renderer frame may already be closing. */ }
}

function subscribeRuntimeEvents(callback: (payload: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe {
  const subscriptionId = ++recoverySubscriptionId;
  reportSessionRecovery({ phase: 'preload_subscribe', subscriptionId });
  const replayBuffer = createRuntimeEventReplayBuffer(callback);
  const listener = (_event: IpcRendererEvent, payload: SequencedRuntimeEventEnvelope<ClientEvent>): void => {
    replayBuffer.push(payload);
  };
  ipcRenderer.on(CH_EVENT, listener);
  const cancelReplay = requestRuntimeEventReplay(
    () => ipcRenderer.invoke(CH_EVENT_REPLAY) as Promise<SequencedRuntimeEventEnvelope<ClientEvent>[]>,
    replayBuffer,
    undefined,
    sample => reportSessionRecovery({ ...sample, subscriptionId }),
  );
  return () => {
    reportSessionRecovery({ phase: 'preload_dispose', subscriptionId });
    cancelReplay();
    replayBuffer.dispose();
    ipcRenderer.removeListener(CH_EVENT, listener);
  };
}

const api: LingxiApi = {
  decision: {
    snapshot: () => ipcRenderer.invoke(CH_DECISION, { action: 'get' }),
    save: (revision, configuration, credential) => ipcRenderer.invoke(CH_DECISION, { action: 'save', revision, configuration, ...(credential === undefined ? {} : { credential }) }),
    evaluate: (operationId, input) => ipcRenderer.invoke(CH_DECISION, { action: 'evaluate', operationId, input }),
    cancel: operationId => ipcRenderer.invoke(CH_DECISION, { action: 'cancel', operationId }),
    onChanged: listener => {
      const handler = (_event: IpcRendererEvent, snapshot: Parameters<typeof listener>[0]) => listener(snapshot);
      ipcRenderer.on(CH_DECISION_CHANGED, handler);
      return () => ipcRenderer.removeListener(CH_DECISION_CHANGED, handler);
    },
  },
  computerSettings: {
    execute: command => ipcRenderer.invoke(CH_COMPUTER_SETTINGS, command),
    onChanged: listener => {
      const callback = (_event: IpcRendererEvent, snapshot: Parameters<typeof listener>[0]) => listener(snapshot);
      ipcRenderer.on(CH_COMPUTER_SETTINGS_CHANGED, callback);
      return () => ipcRenderer.removeListener(CH_COMPUTER_SETTINGS_CHANGED, callback);
    },
  },
  saveImage: request => ipcRenderer.invoke(CH_IMAGE_SAVE, request),
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
    bindAuthoring: (id, session, setCurrent, sourceRequestId) => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'bindAuthoring', id, session, setCurrent, sourceRequestId }),
    prepareAuthoring: session => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'prepareAuthoring', session }),
    pendingPublications: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'pendingPublications' }),
    resumePublication: requestId => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'resumePublication', requestId }),
    pendingSourceCopies: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'pendingSourceCopies' }),
    resumeSourceCopy: requestId => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'resumeSourceCopy', requestId }),
    pendingAuthoringSessions: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'pendingAuthoringSessions' }),
    authoringSessions: () => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'authoringSessions' }),
    resumeAuthoringSession: requestId => ipcRenderer.invoke(CH_LOCAL_APPS, { action: 'resumeAuthoringSession', requestId }),
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
  teams: {
    openBotMarket: () => ipcRenderer.invoke(CH_BOTS_MARKET, {action: 'browse'}),
    localBots: uid => teamsRequest({ type: 'localBots', uid }),
    publishLocalBot: (uid, input) => teamsRequest({ type: 'publishLocalBot', uid, input }),
    selectSharedFiles: (uid, instanceId) => teamsRequest({ type: 'selectSharedFiles', uid, instanceId }),
    list: uid => teamsRequest({ type: 'list', uid }),
    detail: (uid, teamId) => teamsRequest({ type: 'detail', uid, teamId }),
    messages: (uid, command) => teamsRequest({ type: 'messages', uid, command }),
    onMessagesChanged: callback => subscribe('lingxi:teams:messages', callback),
    watchMessages: (uid, scope, conversationId, next) => {
      const leaseId = crypto.randomUUID();
      let closed = false;
      const stop = subscribe('lingxi:teams:snapshot', (event: { leaseId: string; snapshot: import('../../../../packages/teams/messages').MessageSnapshot }) => { if (event.leaseId === leaseId && event.snapshot.uid === uid) next(event.snapshot); });
      void teamsRequest({ type: 'watchMessages', uid, scope, conversationId, leaseId }).catch(error => { if (!closed) next({ uid, ...scope, kind: 'error', fromCache: true, error: error instanceof Error ? error.message : 'MESSAGE_LISTENER_FAILED' }); });
      return () => { closed = true; stop(); void teamsRequest({ type: 'unwatchMessages', uid, leaseId }).catch(() => undefined); };
    },
    execute: (uid, command) => teamsRequest({ type: 'execute', uid, command }),
    onChanged: callback => subscribe('lingxi:teams:changed', callback),
  },
  friends: {
    profile: uid => friendsRequest({ type: 'profile', uid }),
    execute: (uid, command) => friendsRequest({ type: 'execute', uid, command }),
    watchCards: (uid, next, failed) => watchFriends(uid, 'friends', next, failed),
    watchBlocks: (uid, next, failed) => watchFriends(uid, 'blocks', next, failed),
    watchOffers: (uid, next, failed) => watchFriends(uid, 'teamOffers', next, failed),
    readProfile: (uid, friendUid) => friendsRequest({ type: 'readProfile', uid, friendUid }),
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
  bots: {
    market: request => ipcRenderer.invoke(CH_BOTS_MARKET, request),
    availability: () => ipcRenderer.invoke(CH_BOTS_AVAILABILITY),
    list: () => ipcRenderer.invoke(CH_BOTS_LIST),
    get: botId => ipcRenderer.invoke(CH_BOTS_GET, botId),
    create: request => ipcRenderer.invoke(CH_BOTS_CREATE, request),
    update: (botId, expectedRevision, patch) => ipcRenderer.invoke(CH_BOTS_UPDATE, botId, expectedRevision, patch),
    delete: botId => ipcRenderer.invoke(CH_BOTS_DELETE, botId),
    openSession: (botId, sessionId, transcriptPath) => (sessionId === undefined
      ? ipcRenderer.invoke(CH_BOTS_SESSION_OPEN, botId)
      : ipcRenderer.invoke(CH_BOTS_SESSION_OPEN, botId, sessionId, transcriptPath)),
    onSessionShown: callback => subscribe(CH_BOTS_SESSION_SHOWN, callback),
    listSessions: botId => ipcRenderer.invoke(CH_BOTS_SESSIONS_LIST, botId),
    persona: {
      get: botId => ipcRenderer.invoke(CH_BOTS_PERSONA_GET, botId),
      set: (botId, text) => ipcRenderer.invoke(CH_BOTS_PERSONA_SET, botId, text),
    },
    memory: {
      list: botId => ipcRenderer.invoke(CH_BOTS_MEMORY_LIST, botId),
      read: (botId, name) => ipcRenderer.invoke(CH_BOTS_MEMORY_READ, botId, name),
      write: (botId, name, content) => ipcRenderer.invoke(CH_BOTS_MEMORY_WRITE, botId, name, content),
      delete: (botId, name) => ipcRenderer.invoke(CH_BOTS_MEMORY_DELETE, botId, name),
    },
    skills: {
      list: botId => ipcRenderer.invoke(CH_BOTS_SKILLS_LIST, botId),
      delete: (botId, name) => ipcRenderer.invoke(CH_BOTS_SKILL_DELETE, botId, name),
      importFromUser: (botId, name) => ipcRenderer.invoke(CH_BOTS_SKILL_IMPORT, botId, name),
      listImportable: () => ipcRenderer.invoke(CH_BOTS_SKILLS_IMPORTABLE),
    },
    tasks: {
      list: botId => ipcRenderer.invoke(CH_BOTS_TASKS_LIST, botId),
      cancel: (botId, taskId) => ipcRenderer.invoke(CH_BOTS_TASK_CANCEL, botId, taskId),
      onChanged: callback => subscribe(CH_BOTS_TASKS_CHANGED, callback),
    },
    onPromptSent: callback => subscribe(CH_BOTS_PROMPT_SENT, callback),
    loginItem: {
      status: () => ipcRenderer.invoke(CH_BOTS_LOGIN_ITEM_GET),
      set: enabled => ipcRenderer.invoke(CH_BOTS_LOGIN_ITEM_SET, enabled),
      openSettings: () => ipcRenderer.invoke(CH_BOTS_LOGIN_ITEMS_OPEN),
    },
  } satisfies BotsApi,
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
  providerAccounts: {
    routing: () => ipcRenderer.invoke('lingxi:provider-accounts:routing:get'),
    onRouting: callback => subscribe('lingxi:provider-accounts:routing', callback),
    snapshot: () => ipcRenderer.invoke('lingxi:provider-accounts'),
    command: (request, credential) => ipcRenderer.invoke('lingxi:provider-accounts', request, credential),
    cancel: operationId => ipcRenderer.invoke('lingxi:provider-accounts:cancel', operationId),
  },
  providerSettings: (patch, expected) => ipcRenderer.invoke('lingxi:provider-settings', patch, expected),
  pluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_GET, pluginId, key) as Promise<PluginSecretMetadata>,
  setPluginSecret: (pluginId, key, secret) => ipcRenderer.invoke(CH_PLUGIN_SECRET_SET, pluginId, key, secret) as Promise<PluginSecretMetadata>,
  clearPluginSecret: (pluginId, key) => ipcRenderer.invoke(CH_PLUGIN_SECRET_CLEAR, pluginId, key) as Promise<PluginSecretMetadata>,
  cleanupPluginSecretsAfterUninstall: (pluginId, lastScopeRemoved) => ipcRenderer.invoke(CH_PLUGIN_SECRET_CLEANUP, pluginId, lastScopeRemoved) as Promise<PluginSecretCleanupResult>,
  restartBridge: (sessionId) => ipcRenderer.invoke(CH_BRIDGE_RESTART, sessionId) as Promise<void>,
  diagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_GET) as Promise<DiagnosticEntry[]>,
  reportSessionRecovery,
  copyDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_COPY) as Promise<void>,
  copyText: (text) => ipcRenderer.invoke(CH_CLIPBOARD_WRITE_TEXT, text) as Promise<void>,
  setInterfaceLocale: (locale, followSystem) => ipcRenderer.invoke(CH_INTERFACE_LOCALE, locale, followSystem) as Promise<void>,
  exportDiagnostics: () => ipcRenderer.invoke(CH_DIAGNOSTICS_EXPORT) as Promise<string | null>,
  listProjectSessions: (projectPath, scope) => ipcRenderer.invoke(CH_PROJECT_SESSIONS_LIST, projectPath, scope) as Promise<ProjectSessionCatalogState & { projectPath: string; conversationSpaces: ConversationSpacesSnapshot }>,
  updateSessionWorkspace: (sessionId, operation) => ipcRenderer.invoke(CH_SESSION_WORKSPACE, sessionId, operation) as Promise<BootstrapState | null>,
  newSession: (projectPath, model, purpose) => (purpose === undefined
    ? ipcRenderer.invoke(CH_SESSION_NEW, projectPath, model)
    : ipcRenderer.invoke(CH_SESSION_NEW, projectPath, model, purpose)) as Promise<BootstrapState>,
  switchConversationSpace: (mode) => ipcRenderer.invoke(CH_CONVERSATION_SPACE, mode) as Promise<BootstrapState>,
  prepareSessionHandoff: (sourceId, target, requestId, retry) => ipcRenderer.invoke(CH_SESSION_HANDOFF, sourceId, target, requestId, retry) as Promise<{ snapshot: BootstrapState; summary: string; requestId: string } | null>,
  cancelSessionHandoff: (requestId) => ipcRenderer.invoke(CH_SESSION_HANDOFF_CANCEL, requestId) as Promise<void>,
  onConversationShown: (callback) => subscribe(CH_CONVERSATION_SHOWN, callback),
  loadSessionHistory: (sessionId, before) => ipcRenderer.invoke(CH_SESSION_HISTORY, sessionId, before),
  openSession: (projectPath, sessionId, selection, linkedMode, authoringAppId, authoringVersion) => (authoringAppId !== undefined
    ? ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId, selection, linkedMode, authoringAppId, authoringVersion)
    : linkedMode !== undefined
    ? ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId, selection, linkedMode)
    : selection === undefined
    ? ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId)
    : ipcRenderer.invoke(CH_SESSION_OPEN, projectPath, sessionId, selection)) as Promise<BootstrapState>,
  prepareSessionMetadata: (projectPath, sessionId, selection) => ipcRenderer.invoke(CH_SESSION_METADATA_CAPTURE, projectPath, sessionId, selection) as Promise<SessionMetadataSelection>,
  releaseSessionMetadata: (selection) => ipcRenderer.invoke(CH_SESSION_METADATA_RELEASE, selection) as Promise<void>,
  preflightSessionArchive: (projectPath, sessionId, selection) => ipcRenderer.invoke(CH_SESSION_ARCHIVE_PREFLIGHT, projectPath, sessionId, selection) as Promise<CronJobDto[]>,
  archiveSession: (projectPath, sessionId, selection) => ipcRenderer.invoke(CH_SESSION_ARCHIVE, projectPath, sessionId, selection) as Promise<BootstrapState>,
  touchSession: (projectPath, sessionId, selection) => ipcRenderer.invoke(CH_SESSION_TOUCH, projectPath, sessionId, selection) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
  renameSession: (projectPath, sessionId, title, selection) => ipcRenderer.invoke(CH_SESSION_RENAME, projectPath, sessionId, title, selection) as Promise<ProjectSessionCatalogState & { projectPath: string }>,
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
    retainFlow: (sessionId, leaseId, retain) => ipcRenderer.invoke(CH_NATIVE_AUDIO_FLOW_LEASE, sessionId, leaseId, retain) as Promise<void>,
    realtime: (command) => ipcRenderer.invoke(CH_REALTIME_AUDIO_COMMAND, command) as Promise<void>,
    onRealtimeState: (callback) => subscribe(CH_REALTIME_AUDIO_STATE, callback),    request: (command) => ipcRenderer.invoke(CH_NATIVE_AUDIO_REQUEST, command) as Promise<NativeAudioCommandResult>,
    execute: (operation, configurationRevision, configurationOverride, flow) => ipcRenderer.invoke(CH_NATIVE_AUDIO_OPERATION, operation, configurationRevision, configurationOverride, flow) as Promise<NativeAudioOperationResponse>,
    cancel: (flow) => ipcRenderer.invoke(CH_NATIVE_AUDIO_CANCEL, flow) as Promise<void>,
    finishListen: (flow) => ipcRenderer.invoke(CH_NATIVE_AUDIO_FINISH_LISTEN, flow) as Promise<void>,
    onEvent: (callback) => subscribe(CH_NATIVE_AUDIO_EVENT, callback),
  },
};

contextBridge.exposeInMainWorld(DESKTOP_BRIDGE_GLOBAL, api);
