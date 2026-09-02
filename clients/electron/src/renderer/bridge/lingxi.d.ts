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
import type { AllowedClientCommand } from '../../shared/clientCommands.js';
import type { PinnedSessionRecord, PublicSettings, SessionRef } from '../../shared/settings.js';
import type { MicrophonePermissionStatus } from '../../shared/microphoneAccess.js';

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
  revision: number;
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  activeSession?: SessionRef;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  providerCredentials?: ProviderCredentialMetadata[];
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
  /** The three numbers the About page shows. `engine` is absent until a bridge runtime has connected at least once. */
  versions: { app: string; electron: string; engine?: { serverName: string; serverProtocol: string; clientProtocol: string } };
}
export interface WorkspaceFileSearchResult { files: string[]; truncated: boolean }
export type Unsubscribe = () => void;

/** The macOS System Settings deep links this app opens: the computer-access TCC panel's two panes, plus the voice settings page's `microphone` row. */
export type SystemSettingsPane = 'accessibility' | 'screen_recording' | 'microphone';

export interface LingxiApi {
  platform: NodeJS.Platform;
  isElectron: true;
  bootstrap(): Promise<BootstrapState>;
  settings(): Promise<PublicSettings>;
  updateSettings(patch: { theme?: 'dark' | 'light' | 'system'; model?: string | null; apiBaseUrl?: string | null; voice?: unknown }): Promise<PublicSettings>;
  pickWorkspace(): Promise<WorkspaceMetadata | null>;
  setWorkspace(path: string): Promise<WorkspaceMetadata>;
  removeProject(path: string): Promise<BootstrapState>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<PublicSettings>;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  previewWorkspaceFile(sessionId: string, path: string): Promise<WorkspaceFilePreview>;
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
  /** The OS microphone grant, read in the main process — see `shared/microphoneAccess.ts` for why the renderer cannot read it itself. */
  microphoneAccess(): Promise<MicrophonePermissionStatus>;
  cancel(sessionId: string, turnId?: number): Promise<void>;
  command(sessionId: string, command: AllowedClientCommand): Promise<void>;
  connectionState(sessionId: string): Promise<ConnectionState>;
  onEvent(cb: (event: SequencedRuntimeEventEnvelope<ClientEvent>) => void): Unsubscribe;
  onPermission(cb: (request: RuntimeEventEnvelope<PermissionRequest>) => void): Unsubscribe;
  onComputerAccess(cb: (request: RuntimeEventEnvelope<ComputerAccessRequestDto>) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: RuntimeEventEnvelope<ConnectionState>) => void): Unsubscribe;
}

declare global {
  interface Window {
    lingxi?: LingxiApi;
  }
}
