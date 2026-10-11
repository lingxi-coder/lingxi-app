import type { NativeRealtimeAudioCommand, NativeRealtimeAudioState } from '../../shared/realtimeAudio.js';
import type {
  AgentDto,
  AskUserQuestionRequestDto,
  AudioOperationDto,
  AuthStateDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  ConfigurationDomainDto,
  CronJobDto,
  CronRequestDto,
  CronRunDto,
  HookAdminCommandDto,
  HookDto,
  ImageRefDto,
  McpAdminCommandDto,
  PermissionBehaviorDto,
  PermissionModeId,
  PermissionResponseDto,
  PluginAdminCommandDto,
  ReasoningSelectionDto,
  SkillAdminCommandDto,
  WritableScopeDto,
} from '@lingxi/bridge-client';
import type { AudioConfigurationV4 } from '../../shared/generatedAudioConfiguration.js';
import type {
  MicrophonePermissionStatus as VoicePermissionStatus,
} from '../../shared/microphoneAccess.js';
import type {
  NativeAudioCommand,
  NativeAudioCommandResult,
  NativeAudioOperationResponse,
  NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';
import type { NotificationPreferences } from '../../shared/notificationPreferences.js';
import type { HostPermissionRequest } from '../../shared/permission.js';
import type { ScheduledContext, ScheduledScope } from '../../shared/scheduled.js';
import type { ModelPickerVisibilitySettings } from '../../shared/settings.js';
import type { VoicePreferences } from '../../shared/voicePreferences.js';
import type { VisualizationContextChip, VisualizationFollowup } from '../model/runItem.js';
import type { ConversationState, UsageSnapshot } from './conversation.js';
import type { DesktopState } from './desktopState.js';
import type {
  BootstrapState,
  ConnectionState,
  DiagnosticEntry,
  PluginSecretMetadata,
  ProjectSessionCatalogState,
  ProviderConnectionTestResult,
  ProviderCredentialMetadata,
  ProviderCredentialUpdate,
  SessionPinInput,
  SessionRef,
  SystemSettingsPane,
  WorkspaceFilePreview,
  WorkspaceFileSearchResult,
  WorkspaceMetadata,
} from './lingxi.js';
import type {
  RuntimeCenterItemRef,
  RuntimeCenterSection,
  RuntimeCenterState,
} from './runtimeCenterState.js';

/**
 * The wire shape of `ClientEvent::SettingsSnapshot`, unparsed. The settings
 * shell (not this hook) turns its JSON-string fields into structured data —
 * this hook's job stops at "the latest one the engine sent", the same way
 * `bootstrap` stops at the raw `BootstrapState` DTO without interpreting it.
 */
export type SettingsSnapshotEvent = Extract<ClientEvent, { type: 'settings_snapshot' }>;

/** The wire shape of the MCP server listing (`ClientEvent::McpServers`), unparsed — same "latest one wins, one per app not per session" treatment as {@link SettingsSnapshotEvent}. */
export type McpServersEvent = Extract<ClientEvent, { type: 'mcp_servers' }>;

/** The wire shape of the discovered-skills listing (`ClientEvent::Skills`), unparsed. */
export type SkillsEvent = Extract<ClientEvent, { type: 'skills' }>;

export type ConfigurationOperationEvent = Extract<ClientEvent, { type: 'configuration_operation' }>;

export type SkillCatalogEvent = Extract<ClientEvent, { type: 'skill_catalog' }>;

export type SkillDocumentEvent = Extract<ClientEvent, { type: 'skill_document' }>;

export type McpConfigurationSnapshotEvent = Extract<ClientEvent, { type: 'mcp_configuration_snapshot' }>;

export type PluginCatalogEvent = Extract<ClientEvent, { type: 'plugin_catalog' }>;

export type TrackedPromptPurpose = 'composer' | 'flow';

export interface DesktopTurnToken {
  readonly sessionId: string;
  readonly clientTurnId: string;
  readonly purpose: TrackedPromptPurpose;
  /** Correlates admission and cancellation before the first engine event. */
  readonly turnId?: number;
}

export type TrackedSpeechTerminal = 'turn_ended' | 'stale';

export interface TrackedSpeechEvent {
  readonly type: 'delta' | 'message' | 'completion';
  readonly token: DesktopTurnToken;
  readonly text: string;
  readonly sequence: number;
  readonly turnId?: number;
  readonly terminal?: TrackedSpeechTerminal;
}

export interface UseBridge {
  /** Stable per-renderer identity shared by ui_attach and parent UI requests. */
  readonly uiSurfaceClientId: string;
  readonly hosted: boolean;
  readonly loading: boolean;
  readonly bootstrap: BootstrapState | null;
  readonly settingsSnapshotEvent: SettingsSnapshotEvent | null;
  readonly mcpServersEvent: McpServersEvent | null;
  readonly skillsEvent: SkillsEvent | null;
  readonly skillCatalogEvent: SkillCatalogEvent | null;
  readonly skillDocumentEvent: SkillDocumentEvent | null;
  readonly mcpConfigurationSnapshotEvent: McpConfigurationSnapshotEvent | null;
  readonly pluginCatalogEvent: PluginCatalogEvent | null;
  readonly configurationOperations: Readonly<Partial<Record<ConfigurationDomainDto, ConfigurationOperationEvent>>>;
  readonly activeSession: SessionRef | undefined;
  readonly sessionLoading: boolean;
  readonly connection: ConnectionState;
  readonly connected: boolean;
  readonly conversation: ConversationState;
  readonly desktop: DesktopState;
  readonly authState: AuthStateDto | null;
  readonly hooksCatalog: readonly HookDto[];
  readonly agentCatalog: readonly AgentDto[];
  readonly cost: DesktopState['lastCost'];
  readonly lastCompaction: DesktopState['lastCompaction'];
  readonly retryState: DesktopState['lastApiRetry'];
  readonly runtimeCenter: RuntimeCenterState;
  readonly usage: UsageSnapshot | null;
  readonly running: boolean;
  readonly isCancelling: boolean;
  readonly pendingPermission: HostPermissionRequest | null;
  readonly pendingComputerAccess: ComputerAccessRequestDto | null;
  readonly pendingAskUserQuestion: AskUserQuestionRequestDto | null;
  readonly error: string | null;
  readonly audioSnapshot: NativeAudioSnapshot;
  readonly audioRealtimeState: NativeRealtimeAudioState;
  audioRealtimeCommand(command: NativeRealtimeAudioCommand): Promise<void>;
  clearError(): void;
  dismissCommandResult(): void;
  sendTrackedPrompt(
    text: string,
    images?: ImageRefDto[],
    imageNames?: string[],
    filePaths?: string[],
    options?: { purpose?: TrackedPromptPurpose; visualizationContext?: VisualizationContextChip },
  ): { token: DesktopTurnToken; queued: Promise<void> } | null;
  /** A follow-up a widget in the active session drafted, until the composer takes it. */
  readonly visualizationFollowup: VisualizationFollowup | null;
  offerVisualizationFollowup(followup: VisualizationFollowup): void;
  clearVisualizationFollowup(): void;
  subscribeTrackedSpeech(token: DesktopTurnToken, listener: (event: TrackedSpeechEvent) => void): () => void;
  sendPrompt(text: string, images?: ImageRefDto[], imageNames?: string[], filePaths?: string[]): Promise<void>;
  runSlashCommand(raw: string): Promise<void>;
  /** Echo a slash line the desktop is handling locally; makes no `running` claim. */
  beginLocalCommand(raw: string): void;
  /** Push a locally-produced command's own output into the transcript. */
  emitCommandOutput(output: string, isError: boolean): void;
  cancel(turnId?: number): Promise<void>;
  cancelTrackedPrompt(token: DesktopTurnToken): Promise<void>;
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(requestId: number): Promise<void>;
  approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(requestId: number): Promise<void>;
  answerAskUserQuestion(requestId: number, answers: Record<string, string>): Promise<void>;
  cancelAskUserQuestion(requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  /**
   * The OS microphone grant, read by the MAIN process
   * (`systemPreferences.getMediaAccessStatus`). The renderer has no honest
   * equivalent — `navigator.permissions.query({name:'microphone'})` reports
   * the page permission this app grants itself — so the voice settings page
   * asks through here. Never rejects: an unreachable host, a failed IPC call
   * or an answer this renderer cannot interpret are all `'unavailable'`
   * ("cannot determine"), which the 麦克风权限 row renders as 无法确定. It is
   * deliberately NOT routed through `capture`: a permission probe that the
   * page already renders as an honest state has nothing to say in the
   * shell's global error banner.
   */
  microphonePermission(): Promise<VoicePermissionStatus>;
  addProject(): Promise<WorkspaceMetadata | null>;
  activateProject(path: string): Promise<WorkspaceMetadata | null>;
  removeProject(path: string): Promise<void>;
  updateSidebarPreferences(preferences: import('../../shared/settings').SidebarPreferences): Promise<void>;
  preflightSessionArchive(projectPath: string, sessionId: string): Promise<CronJobDto[]>;
  archiveSession(projectPath: string, sessionId: string): Promise<void>;
  touchSession(projectPath: string, sessionId: string): Promise<void>;
  renameSession(projectPath: string, sessionId: string, title: string): Promise<void>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<void>;
  openSession(projectPath: string, sessionId: string): Promise<void>;
  listProjectSessions(projectPath: string): Promise<ProjectSessionCatalogState | undefined>;
  sessionRuntimeStatus(sessionId: string): SessionRuntimeStatus | undefined;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  loginCodex(): Promise<ProviderCredentialUpdate>;
  cancelCodexLogin(): Promise<void>;
  testProviderConnection(providerId: string, credentialOverride?: string): Promise<ProviderConnectionTestResult>;
  refreshProviderCredential(providerId: string): Promise<void>;
  pluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  setPluginSecret(pluginId: string, key: string, secret: string): Promise<PluginSecretMetadata>;
  clearPluginSecret(pluginId: string, key: string): Promise<PluginSecretMetadata>;
  setThemePreference(theme: 'dark' | 'light' | 'system'): Promise<void>;
  setCollapseThoughtsByDefault(collapseThoughtsByDefault: boolean): Promise<void>;
  /**
   * Writes the WHOLE notification-preferences object at once, like
   * `setVoicePreferences`. The main process pushes the result straight at
   * `HostNotifier`, which holds armed timers and so cannot notice a change it
   * is not told about.
   */
  setNotificationPreferences(notifications: NotificationPreferences): Promise<void>;
  /** The device-level (Electron store) custom API base URL override — `null` clears it. Distinct from `updateEngineSettings` below, which writes to an engine settings FILE layer. */
  setApiBaseUrl(apiBaseUrl: string | null): Promise<void>;
  /**
   * Writes the WHOLE voice-preferences object at once, through the same
   * device-settings path `setThemePreference`/`setApiBaseUrl` already use
   * (`host.updateSettings({ voice })` → `SettingsStore.update()` →
   * `parseVoicePreferences`, Task 4). The voice settings page is the only
   * caller and always supplies a complete `VoicePreferences`, matching how
   * both phones persist voice settings (whole-snapshot writes, never a
   * partial per-field merge) — see `shared/voicePreferences.ts`'s own doc.
   * Never restarts the bridge: unlike `model`/`apiBaseUrl`, nothing here
   * changes what the running engine talks to.
   */
  setVoicePreferences(voice: VoicePreferences, expectedRevision: number): Promise<void>;
  setModelPickerVisibility(modelPickerVisibility: ModelPickerVisibilitySettings): Promise<void>;
  /**
   * Writes a JSON-object patch into one engine settings file layer via the
   * `update_settings` wire command (`patch_json`; a `null` value in the patch
   * deletes that key at this layer). This is the ONLY write path a `layered`
   * settings page (`nav.ts`'s `layered: true`) has — there is no per-key
   * command for `settings.providers` / `settings.routing`, unlike
   * `permissions`/`workspace directories`, which get their own typed
   * commands. Refetches the snapshot afterward so the page's own `snapshot`
   * prop reflects the write without a separate caller-side refresh call.
   */
  updateEngineSettings(destination: 'user' | 'project' | 'local', patch: Record<string, unknown>): Promise<void>;
  /** Save provider settings and wait for confirmation from the persisted file layer. */
  updateProviderSettings(destination: 'user' | 'project' | 'local', patch: Record<string, unknown>): Promise<void>;
  /**
   * `update_permission_rules` — the ONLY write path for `permissions.{allow,deny,ask}`.
   * `apply_patch` refuses the `permissions` key outright, so there is no
   * generic-patch alternative to fall back to. `add`/`remove` are rule
   * strings; parsing is infallible on the engine side
   * (`PermissionRuleValue::from_rule_string` degrades malformed input to a
   * bare tool name, matching claude-code) — this wrapper does not validate
   * or reject anything either. Refetches the snapshot afterward so the
   * page renders the ACTUALLY persisted (possibly normalised) rule text,
   * never an optimistic echo of the raw input.
   */
  updatePermissionRules(
    destination: WritableScopeDto, behavior: PermissionBehaviorDto, add: string[], remove: string[],
  ): Promise<void>;
  /**
   * `set_default_permission_mode` — persists `permissions.defaultMode`.
   * The engine deliberately refuses `"bypassPermissions"` here (returns
   * `Ok(false)` and emits a `Rejected` error event) as a security property,
   * not a bug — see `permission::persist_permission_mode`. This wrapper
   * does not special-case that mode; it always refetches the snapshot
   * afterward so a caller can tell a refusal apart from a success by
   * comparing the requested mode against what the snapshot actually shows.
   */
  setDefaultPermissionMode(destination: WritableScopeDto, mode: string): Promise<void>;
  /** `update_workspace_directories` — the dedicated writer for `permissions.additionalDirectories`, same add/remove-delta shape as {@link updatePermissionRules}. */
  updateWorkspaceDirectories(destination: WritableScopeDto, add: string[], remove: string[]): Promise<void>;
  /** Re-pulls the MCP server listing (`refresh_listings{mcp}` → `ClientEvent::McpServers`). This is a RUNNING/merged view (name, status, transport) with no per-scope provenance — see `McpServers.tsx`'s own doc comment for why the page cannot decompose it by scope. */
  refreshMcpServers(): Promise<void>;
  /** Re-pulls the discovered-skills listing (`refresh_listings{skills}` → `ClientEvent::Skills`). Directory-discovered, NOT layered — `Skills.tsx` reads this the same way regardless of `editingLayer`. */
  refreshSkills(): Promise<void>;
  /** `upsert_mcp_server` — writes one server definition into exactly the named scope's own storage location (`~/.lingxi.json` for User/Local, `<project>/.mcp.json` for Project). Refetches the MCP listing afterward. `config` is a plain JS object; this wrapper owns the `JSON.stringify` the wire's `config_json: String` field requires. */
  upsertMcpServer(scope: WritableScopeDto, name: string, config: Record<string, unknown>): Promise<void>;
  /** `remove_mcp_server` — idempotent removal from exactly the named scope. Refetches the MCP listing afterward. */
  removeMcpServer(scope: WritableScopeDto, name: string): Promise<void>;
  /** Native Desktop skill administration. Write commands resolve only after the correlated terminal operation event. */
  scheduledScopes: ScheduledScope[];
  scheduledContext(scopeId: string): Promise<ScheduledContext>;
  manageScheduled(scopeId: string, request: CronRequestDto): Promise<CronJobDto[]>;
  readScheduledHistory(scopeId: string, jobId: string): Promise<CronRunDto[]>;
  openScheduledSession(scopeId: string, sessionId: string): Promise<void>;
  manageCron(request: CronRequestDto): Promise<CronJobDto[]>;
  skillAdmin(command: SkillAdminCommandDto): Promise<ConfigurationOperationEvent | void>;
  /** Native Desktop MCP administration with strict validation, revision/CAS, approval, and live reconcile. */
  mcpAdmin(command: McpAdminCommandDto): Promise<ConfigurationOperationEvent | void>;
  /** Native Desktop plugin catalog/package/config administration. */
  pluginAdmin(command: PluginAdminCommandDto): Promise<ConfigurationOperationEvent | void>;
  /** Native Desktop hook document validation and persistence. */
  hookAdmin(command: HookAdminCommandDto): Promise<ConfigurationOperationEvent | void>;
  restartBridge(sessionId?: string): Promise<void>;
  refreshDiagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  audioRequest(command: NativeAudioCommand): Promise<NativeAudioCommandResult>;
  audioExecute(operation: AudioOperationDto, configurationRevision?: number, configurationOverride?: AudioConfigurationV4): Promise<NativeAudioOperationResponse>;
  audioCancel(): Promise<void>;
  audioFinishListen(): Promise<void>;
  refresh(): Promise<void>;
  newSession(projectPath?: string): Promise<void>;
  resumeSession(sessionId: string): Promise<void>;
  setModel(model: string): Promise<void>;
  setReasoningSelection(selection: ReasoningSelectionDto): Promise<void>;
  setFastMode(enabled: boolean): Promise<void>;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  login(): Promise<void>;
  logout(): Promise<void>;
  forceCompact(instructions?: string): Promise<void>;
  clearSession(name?: string): Promise<void>;
  refreshTasks(options?: { preserve?: boolean }): Promise<void>;
  refreshAuth(): Promise<void>;
  refreshHooks(): Promise<void>;
  refreshAgents(): Promise<void>;
  refreshStatus(): Promise<void>;
  refreshDoctor(): Promise<void>;
  /** Re-pulls the live engine slash-command registry for completion and the command palette. */
  refreshSlashCommands(): Promise<void>;
  /**
   * Re-pulls everything the model picker shows: the catalog (`ModelList` +
   * `ProviderModelCatalog`) and the reasoning controls.
   *
   * The once-per-connection listing batch is the only other thing that asks for
   * either, so without this a single lost reply left the picker with nothing to
   * offer — and its Effort row dead — until the app was restarted.
   */
  refreshModelPicker(): Promise<void>;
  refreshSessionAgents(): Promise<void>;
  loadSessionAgentTranscript(agentId: string): Promise<void>;
  openRuntimeItem(item: RuntimeCenterItemRef): void;
  closeRuntimeItem(item: RuntimeCenterItemRef): void;
  setRuntimeCenterOverviewOpen(open: boolean): void;
  setRuntimeInspectorOpen(open: boolean): void;
  toggleRuntimeCenterSection(section: RuntimeCenterSection): void;
  previewWorkspaceFile(path: string): Promise<WorkspaceFilePreview>;
  taskOutput(taskId: string): Promise<void>;
  stopTask(taskId: string): Promise<void>;
  refreshSettingsSnapshot(): Promise<void>;
}

export interface SessionRuntimeStatus {
  readonly backgroundAgentsRunning?: boolean;
  readonly connection: ConnectionState;
  readonly turnActive: boolean;
  readonly pendingInteractions: number;
  readonly pendingAskUserQuestions: number;
  readonly error?: string;
}
