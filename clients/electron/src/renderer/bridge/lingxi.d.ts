import type {
  AskUserQuestionRequestDto,
  ClientCommand,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';

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
  command(command: AllowedClientCommand): Promise<void>;
  connectionState(): Promise<ConnectionState>;
  onEvent(cb: (event: ClientEvent) => void): Unsubscribe;
  onPermission(cb: (request: PermissionRequest) => void): Unsubscribe;
  onComputerAccess(cb: (request: ComputerAccessRequestDto) => void): Unsubscribe;
  onConnectionStateChanged(cb: (state: ConnectionState) => void): Unsubscribe;
}

declare global {
  interface Window {
    lingxi?: LingxiApi;
  }
}
