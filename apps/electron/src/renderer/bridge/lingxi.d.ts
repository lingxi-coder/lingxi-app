import type { ScheduledApi } from '../../shared/scheduled';
import type { VisualizationGuestEvent, VisualizationMount, VisualizationReference, VisualizationStateWrite, VisualizationTheme } from '../../shared/visualization';
import type { GitApi } from '../../shared/git.js';
import type { TerminalApi } from '../../shared/terminal.js';
import type { CronJobDto } from '@lingxi/bridge-client';
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
  SessionRowDto,
  UiClientFrameEventDto,
  UiClientOperation,
  UiClientOperationResponseFor,
  UiControlCallResultDto,
  UiInvalidateEventDto,
} from '@lingxi/bridge-client';
import type { HostPermissionRequest } from '../../shared/permission.js';
import type { AllowedClientCommand } from '../../shared/clientCommands.js';
import type {
  NativeAudioCommand,
  NativeAudioOperationResponse,
  NativeAudioResponse,
  NativeAudioEvent,
} from '../../shared/nativeAudio.js';
import type { PinnedSessionRecord, PublicSettings, SessionRef } from '../../shared/settings.js';
import type { MicrophonePermissionStatus } from '../../shared/microphoneAccess.js';
import type { AudioConfigurationV3 } from '../../shared/generatedAudioConfiguration.js';

export interface WorkspaceFilePreview {
  kind: 'text' | 'binary';
  path: string;
  size: number;
  content?: string;
  truncated: boolean;
}

export type { AllowedClientCommand } from '../../shared/clientCommands.js';
export type {
  PinnedSessionRecord,
  PublicSettings,
  SessionPinInput,
  SessionRef,
} from '../../shared/settings.js';

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
export interface SequencedRuntimeEventEnvelope<T = unknown> extends RuntimeEventEnvelope<T> { sequence: number }
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
export interface CredentialMetadata { configured: boolean; encryptionAvailable: boolean; credentialPreview?: string; runtimeOnly?: true; storageError?: string; codexAccountEmail?: string }
export interface ProviderCredentialMetadata extends CredentialMetadata { providerId: string }
export interface PluginSecretMetadata { pluginId: string; key: string; configured: boolean; maskedValue?: string; storageError?: string; restartRequired?: boolean }
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
  revision: number;
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  activeSession?: SessionRef;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  providerCredentials?: ProviderCredentialMetadata[];
  /** Whether credentials can be queried without a connected engine. */
  credentialBrokerAvailable?: boolean;
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
  /** The three numbers the About page shows. `engine` is absent until a bridge runtime has connected at least once. */
  versions: { app: string; electron: string; engine?: { serverName: string; serverProtocol: string; clientProtocol: string } };
}
export interface WorkspaceFileSearchResult { files: string[]; directories?: string[]; truncated: boolean }
export type Unsubscribe = () => void;
export interface NativeAudioApi {
  request(command: NativeAudioCommand): Promise<NativeAudioResponse>;
  execute(operation: AudioOperationDto, configurationRevision?: number, configurationOverride?: AudioConfigurationV3): Promise<NativeAudioOperationResponse>;
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
  scheduled?: ScheduledApi;
  git?: GitApi;
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
    voiceRevision?: number;
    notifications?: unknown;
    modelPickerVisibility?: unknown;
    sidebar?: unknown;
    sidebar?: unknown;
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
  loginCodex(): Promise<ProviderCredentialUpdate>;
  cancelCodexLogin(): Promise<void>;
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
  /** The OS microphone grant, read in the main process — see `shared/microphoneAccess.ts` for why the renderer cannot read it itself. */
  microphoneAccess(): Promise<MicrophonePermissionStatus>;
  cancel(sessionId: string, turnId?: number): Promise<void>;
  command(sessionId: string, command: AllowedClientCommand): Promise<void>;
  connectionState(sessionId: string): Promise<ConnectionState>;
  onEvent(cb: (event: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe;
  onPermission(cb: (request: RuntimeEventEnvelope<HostPermissionRequest>) => void): Unsubscribe;
  onComputerAccess(cb: (request: RuntimeEventEnvelope<ComputerAccessRequestDto>) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: RuntimeEventEnvelope<ConnectionState>) => void): Unsubscribe;
  audio: NativeAudioApi;
  visualization: VisualizationApi;
  modUi: ModUiApi;
}

declare global {
  interface Window {
    lingxi?: LingxiApi;
  }
}
