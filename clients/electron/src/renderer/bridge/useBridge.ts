import { submittedSessionCatalogs, type SubmittedSession } from './submittedSessionCatalog';
import { saveProviderSettings } from './providerSettingsSave';
import { requestCronManagement } from './cronManagement';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  CronJobDto,
  CronRequestDto,
  AgentDto,
  AskUserQuestionRequestDto,
  AuthStateDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  ConfigurationDomainDto,
  HookAdminCommandDto,
  HookDto,
  ImageRefDto,
  McpAdminCommandDto,
  McpScopeDto,
  PermissionBehaviorDto,
  PermissionModeId,
  PermissionRequest,
  PermissionResponseDto,
  ReasoningSelectionDto,
  SettingsDestinationDto,
  PluginAdminCommandDto,
  SkillAdminCommandDto,
} from '@lingxi/bridge-client';

import type { AudioRequestDeps } from '../audio/requests';
import {
  hostMicrophonePermissionReader,
  type MicrophonePermissionStatus as VoicePermissionStatus,
} from '../../shared/microphoneAccess';
import {
  defaultNativeAudioSnapshot,
  type NativeAudioCommand,
  type NativeAudioCommandResult,
  type NativeAudioResponse,
  type NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';
import type { VoicePreferences } from '../../shared/voicePreferences';
import type { NotificationPreferences } from '../../shared/notificationPreferences';
import type { ModelPickerVisibilitySettings } from '../../shared/settings';
import {
  appendPendingUserPrompt,
  beginCompaction,
  beginLocalSlashCommand,
  beginSlashCommand,
  emptyConversation,
  reduceEvent,
  type ConversationState,
  type UsageSnapshot,
} from './conversation';
import {
  beginTaskRefresh,
  emptyDesktopState,
  reduceDesktopEvent,
  type DesktopState,
} from './desktopState';
import {
  addRuntimeResources,
  closeRuntimeCenterItem,
  commitRuntimeResources,
  emptyRuntimeCenterState,
  openRuntimeCenterItem,
  promptRuntimeResources,
  reduceRuntimeCenterEvent,
  reduceRuntimeCenterPermission,
  resetRuntimeCenterConnection,
  resourcesFromRestoredMessages,
  rollbackRuntimeResources,
  setRuntimeCenterOverviewOpen,
  setRuntimeInspectorOpen,
  toggleRuntimeCenterSection,
  type RuntimeCenterItemRef,
  type RuntimeCenterSection,
  type RuntimeCenterState,
} from './runtimeCenterState';
import type {
  BootstrapState,
  ConnectionState,
  DiagnosticEntry,
  PluginSecretMetadata,
  ProjectSessionCatalogState,
  ProviderCredentialMetadata,
  ProviderCredentialUpdate,
  ProviderConnectionTestResult,
  SequencedRuntimeEventEnvelope,
  SessionPinInput,
  SessionRef,
  SessionRuntimeSummary,
  SystemSettingsPane,
  WorkspaceFileSearchResult,
  WorkspaceFilePreview,
  WorkspaceMetadata,
} from './lingxi';

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
}

export type TrackedSpeechTerminal = 'message_complete' | 'turn_ended' | 'stale';

export interface TrackedSpeechEvent {
  readonly type: 'delta' | 'completion';
  readonly token: DesktopTurnToken;
  readonly text: string;
  readonly sequence: number;
  readonly turnId?: number;
  readonly terminal?: TrackedSpeechTerminal;
}

export interface UseBridge {
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
  readonly pendingPermission: PermissionRequest | null;
  readonly pendingComputerAccess: ComputerAccessRequestDto | null;
  readonly pendingAskUserQuestion: AskUserQuestionRequestDto | null;
  readonly error: string | null;
  readonly audioSnapshot: NativeAudioSnapshot;
  clearError(): void;
  sendTrackedPrompt(
    text: string,
    images?: ImageRefDto[],
    imageNames?: string[],
    filePaths?: string[],
    options?: { purpose?: TrackedPromptPurpose },
  ): { token: DesktopTurnToken; queued: Promise<void> } | null;
  subscribeTrackedSpeech(token: DesktopTurnToken, listener: (event: TrackedSpeechEvent) => void): () => void;
  sendPrompt(text: string, images?: ImageRefDto[], imageNames?: string[], filePaths?: string[]): Promise<void>;
  runSlashCommand(raw: string): Promise<void>;
  /** Echo a slash line the desktop is handling locally; makes no `running` claim. */
  beginLocalCommand(raw: string): void;
  /** Push a locally-produced command's own output into the transcript. */
  emitCommandOutput(output: string, isError: boolean): void;
  cancel(turnId?: number): Promise<void>;
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
  preflightSessionArchive(projectPath: string, sessionId: string): Promise<CronJobDto[]>;
  archiveSession(projectPath: string, sessionId: string): Promise<void>;
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
  setVoicePreferences(voice: VoicePreferences): Promise<void>;
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
    destination: SettingsDestinationDto, behavior: PermissionBehaviorDto, add: string[], remove: string[],
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
  setDefaultPermissionMode(destination: SettingsDestinationDto, mode: string): Promise<void>;
  /** `update_workspace_directories` — the dedicated writer for `permissions.additionalDirectories`, same add/remove-delta shape as {@link updatePermissionRules}. */
  updateWorkspaceDirectories(destination: SettingsDestinationDto, add: string[], remove: string[]): Promise<void>;
  /** Re-pulls the MCP server listing (`refresh_listings{mcp}` → `ClientEvent::McpServers`). This is a RUNNING/merged view (name, status, transport) with no per-scope provenance — see `McpServers.tsx`'s own doc comment for why the page cannot decompose it by scope. */
  refreshMcpServers(): Promise<void>;
  /** Re-pulls the discovered-skills listing (`refresh_listings{skills}` → `ClientEvent::Skills`). Directory-discovered, NOT layered — `Skills.tsx` reads this the same way regardless of `editingLayer`. */
  refreshSkills(): Promise<void>;
  /** `upsert_mcp_server` — writes one server definition into exactly the named scope's own storage location (`~/.lingxi.json` for User/Local, `<project>/.mcp.json` for Project). Refetches the MCP listing afterward. `config` is a plain JS object; this wrapper owns the `JSON.stringify` the wire's `config_json: String` field requires. */
  upsertMcpServer(scope: McpScopeDto, name: string, config: Record<string, unknown>): Promise<void>;
  /** `remove_mcp_server` — idempotent removal from exactly the named scope. Refetches the MCP listing afterward. */
  removeMcpServer(scope: McpScopeDto, name: string): Promise<void>;
  /** Native Desktop skill administration. Write commands resolve only after the correlated terminal operation event. */
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
  clearSession(): Promise<void>;
  refreshTasks(options?: { preserve?: boolean }): Promise<void>;
  refreshAuth(): Promise<void>;
  refreshHooks(): Promise<void>;
  refreshAgents(): Promise<void>;
  refreshStatus(): Promise<void>;
  refreshDoctor(): Promise<void>;
  /** Re-pulls the live engine slash-command registry for completion and the command palette. */
  refreshSlashCommands(): Promise<void>;
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

interface PendingConfigurationOperation {
  domain: ConfigurationDomainDto;
  resolve: (event: ConfigurationOperationEvent) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

const CONFIGURATION_OPERATION_TIMEOUT_MS = 5 * 60_000;
const MAX_TRACKED_SPEECH_SUBSCRIBERS = 64;

function configurationOperationId(
  command: SkillAdminCommandDto | McpAdminCommandDto | PluginAdminCommandDto | HookAdminCommandDto,
): number | undefined {
  return 'operation_id' in command && typeof command.operation_id === 'number' ? command.operation_id : undefined;
}

function getHost() {
  return typeof window !== 'undefined' ? window.lingxi : undefined;
}

function messageFrom(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return 'The desktop host could not complete that action.';
}

/** An interaction can race an engine-side expiry or another window's answer. */
export function isPermissionRequestGone(error: unknown): boolean {
  return /\bpermission request is not pending\b/.test(messageFrom(error));
}

export const BRIDGE_RESTART_TIMEOUT_MS = 20_000;

export function restartBridgePreconditionError(
  sessionLoading: boolean,
  hasHost: boolean,
  sessionId: string | null | undefined,
): Error | null {
  if (sessionLoading) {
    return new Error('Cannot restart the engine while a session is loading. Please wait for it to finish opening.');
  }
  if (!hasHost) return new Error('Desktop host unavailable.');
  if (!sessionId) return new Error('Open a session before restarting the engine.');
  return null;
}

/**
 * Keep renderer actions bounded even if an IPC handler never settles. The
 * underlying restart is intentionally not cancelled: the host owns that
 * lifecycle and may still finish after the renderer has entered recovery.
 */
export async function restartBridgeWithTimeout(
  restart: () => Promise<void>,
  timeoutMs = BRIDGE_RESTART_TIMEOUT_MS,
): Promise<void> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new Error('invalid bridge restart timeout');
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const operation = Promise.resolve().then(restart);
    await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Timed out waiting for the engine to restart.')), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

/**
 * Share one host restart per session. A renderer timeout must not release the
 * slot while the host is still stopping/starting that session.
 */
export function restartBridgeSingleFlight(
  inFlight: Map<string, Promise<void>>,
  sessionId: string,
  restart: () => Promise<void>,
): Promise<void> {
  const current = inFlight.get(sessionId);
  if (current) return current;

  const operation = Promise.resolve().then(restart);
  inFlight.set(sessionId, operation);
  const clear = (): void => {
    if (inFlight.get(sessionId) === operation) inFlight.delete(sessionId);
  };
  // Supplying both handlers prevents a rejected operation's cleanup promise
  // from becoming an unhandled rejection while preserving the original error
  // for every caller awaiting `operation`.
  void operation.then(clear, clear);
  return operation;
}

/** Choose the edge target when focus is outside or at a dialog boundary. */
export function dialogFocusTarget<T>(
  focusable: readonly T[],
  active: T | null | undefined,
  backwards: boolean,
): T | undefined {
  if (focusable.length === 0) return undefined;
  const activeIndex = active === null || active === undefined ? -1 : focusable.indexOf(active);
  if (activeIndex < 0) return backwards ? focusable.at(-1) : focusable[0];
  if (backwards && activeIndex === 0) return focusable.at(-1);
  if (!backwards && activeIndex === focusable.length - 1) return focusable[0];
  return undefined;
}

export function shouldResetBridgeRuntime(state: ConnectionState): boolean {
  return state.status === 'spawning';
}

export function shouldClearPendingPermissions(state: ConnectionState): boolean {
  return shouldResetBridgeRuntime(state)
    || state.status === 'disconnected'
    || state.status === 'error'
    || state.status === 'idle';
}

export function resetBridgeRuntimeState(): {
  conversation: ConversationState;
  desktop: DesktopState;
  permissionQueue: PermissionRequest[];
  computerAccessQueue: ComputerAccessRequestDto[];
  askUserQuestionQueue: AskUserQuestionRequestDto[];
} {
  return {
    conversation: emptyConversation(),
    desktop: emptyDesktopState(),
    permissionQueue: [],
    computerAccessQueue: [],
    askUserQuestionQueue: [],
  };
}

export function clearCancellationRuntime(
  cancelling: { current: boolean },
  task: { current: Promise<void> | null },
): void {
  cancelling.current = false;
  task.current = null;
}

/** Remove renderer state for runtimes no longer present in the host snapshot. */
interface SessionRuntimeMap {
  keys(): IterableIterator<string>;
  delete(sessionId: string): boolean;
}

export function pruneRuntimeMaps(runtimeIds: Iterable<string>, ...maps: SessionRuntimeMap[]): void {
  const authoritativeIds = new Set(runtimeIds);
  for (const map of maps) {
    for (const sessionId of map.keys()) {
      if (!authoritativeIds.has(sessionId)) map.delete(sessionId);
    }
  }
}

export function removeRuntimeFromMaps(sessionId: string, ...maps: SessionRuntimeMap[]): void {
  for (const map of maps) map.delete(sessionId);
}

/**
 * Turn ownership for slash dispatch.
 *
 * `sendPrompt` claims the turn the instant the command crosses the bridge
 * (`turn_started` may land a tick later; `bridge.ts:900` carries the same
 * pre-claim). Slash dispatch needs the same claim — but most slash commands
 * are display-only and never start a turn, so an unconditional claim would
 * lock the composer forever on `/status`. The claim is therefore released by
 * `slash_command_result`, and only while it is still outstanding: the engine
 * has a fallback arm that emits a display-only result for a prompt command
 * (`bridge-server/src/router.rs:938`), and releasing on that would unlock the
 * composer in the middle of a live turn.
 */
export function claimSlashTurn(pending: Map<string, boolean>, sessionId: string): void {
  pending.set(sessionId, true);
}

export function clearSlashTurnClaim(pending: Map<string, boolean>, sessionId: string): void {
  pending.delete(sessionId);
}

export function shouldReleaseSlashTurn(pending: Map<string, boolean>, sessionId: string): boolean {
  return pending.get(sessionId) === true;
}

/**
 * The audio bindings for ONE session, built on first use and then kept for
 * that session's lifetime.
 *
 * Per session, not per hook. A capture spans a `start_recording` /
 * `stop_recording` PAIR of engine requests, so the recorder has to outlive a
 * single request — but `SessionRuntimeManager` runs a Map of concurrent
 * runtimes, each with its own engine and its own `AudioBridge`, and each
 * registering the `voice` tool. One shared `MicrophoneCapture` across all of
 * them means session B's `is_recording` answers `true` for a capture session A
 * started, B's `stop_recording` finalizes A's clip into B's transcript, and A's
 * own stop then answers `not_recording` having lost its recording entirely.
 * Keying by session is what makes each answer describe the session that asked.
 *
 * `build` is a factory rather than a value because it touches
 * `navigator.mediaDevices` and `window.speechSynthesis`, which a
 * server-rendered probe of this hook has neither of — nothing is constructed
 * until an `audio_request` actually arrives for that session.
 */
export function sessionAudioBindings(
  bindings: Map<string, AudioRequestDeps>,
  sessionId: string,
  build: () => AudioRequestDeps,
): AudioRequestDeps {
  const existing = bindings.get(sessionId);
  if (existing) return existing;
  const built = build();
  bindings.set(sessionId, built);
  return built;
}

/**
 * Drops one session's audio bindings, stopping a capture that is still running.
 *
 * Nothing else holds that `MicrophoneCapture`: dropping the entry while it is
 * recording would leave the OS microphone (and its indicator) on for the life
 * of the app, with no object left that could release it.
 */
export function discardAudioBindings(bindings: Map<string, AudioRequestDeps>, sessionId: string): void {
  const deps = bindings.get(sessionId);
  if (!deps) return;
  bindings.delete(sessionId);
  try {
    if (deps.recorder.isRecording()) void deps.recorder.stop().catch(() => undefined);
  } catch {
    // Releasing a device on teardown must never take the caller down with it.
  }
}

/** `pruneRuntimeMaps` for the audio bindings, which need the release above. */
export function pruneAudioBindings(
  bindings: Map<string, AudioRequestDeps>,
  runtimeIds: Iterable<string>,
): void {
  const authoritativeIds = new Set(runtimeIds);
  for (const sessionId of [...bindings.keys()]) {
    if (!authoritativeIds.has(sessionId)) discardAudioBindings(bindings, sessionId);
  }
}

export function shouldApplyBootstrapSnapshot(
  latestRevision: number | null,
  snapshotRevision: number,
): boolean {
  const normalizedSnapshotRevision = Number.isFinite(snapshotRevision) ? snapshotRevision : 0;
  // Equal revisions cannot establish ordering. Treat the first snapshot as
  // the baseline, then require a strictly newer revision so a delayed copy
  // cannot undo a local runtime-disposal tombstone.
  return latestRevision === null || normalizedSnapshotRevision > latestRevision;
}

export function nextOperationId(current: number): number {
  return current + 1;
}

export function isLatestOperation(operationId: number, latestOperationId: number): boolean {
  return operationId === latestOperationId;
}

export function beginProjectCatalogRequest(generations: Map<string, number>, projectPath: string): number {
  const generation = (generations.get(projectPath) ?? 0) + 1;
  generations.set(projectPath, generation);
  return generation;
}

export function isLatestProjectCatalogRequest(
  generations: ReadonlyMap<string, number>,
  projectPath: string,
  generation: number,
): boolean {
  return generations.get(projectPath) === generation;
}

export async function recoverLatestNavigationFailure<T>(
  operationId: number,
  isCurrent: (operationId: number) => boolean,
  fetchAuthoritative: () => Promise<T>,
  applyAuthoritative: (snapshot: T) => void,
): Promise<void> {
  if (!isCurrent(operationId)) return;
  const snapshot = await fetchAuthoritative();
  if (isCurrent(operationId)) applyAuthoritative(snapshot);
}

export function pendingCountAfterResponse(current: number | undefined, fallback: number): number {
  return Math.max(0, (current ?? fallback) - 1);
}

export function reconcilePendingCount(localOverride: number | undefined, snapshotCount: number): number | undefined {
  return localOverride !== undefined && snapshotCount > localOverride ? localOverride : undefined;
}

const SESSION_RUNTIME_DISPOSED_REASON = 'session runtime disposed';

export function isRuntimeRemovedState(state: ConnectionState): boolean {
  return state.status === 'disconnected' && state.reason === SESSION_RUNTIME_DISPOSED_REASON;
}

function pendingAskQueueFromBootstrap(snapshot: BootstrapState): AskUserQuestionRequestDto[] {
  return snapshot.pendingAskUserQuestions ? [...snapshot.pendingAskUserQuestions] : [];
}

export function canResumePendingSession(
  pending: SessionRef | null,
  workspace: WorkspaceMetadata | undefined,
  connection: ConnectionState,
): boolean {
  return Boolean(
    pending
    && connection.status === 'connected'
    && workspace?.path === pending.projectPath
    && workspace.trusted,
  );
}

export function displayedSession(
  persisted: SessionRef | undefined,
  pending: SessionRef | null,
): SessionRef | undefined {
  return pending ?? persisted;
}

interface RuntimeState {
  submittedSession?: SubmittedSession;
  connection: ConnectionState;
  conversation: ConversationState;
  desktop: DesktopState;
  runtimeCenter: RuntimeCenterState;
  permissionQueue: PermissionRequest[];
  computerAccessQueue: ComputerAccessRequestDto[];
  askUserQuestionQueue: AskUserQuestionRequestDto[];
  pendingInteractionsOverride?: number;
  pendingAskUserQuestionsOverride?: number;
  resolvedPermissionIds: Set<number>;
  resolvedAskUserQuestionIds: Set<number>;
  isCancelling: boolean;
  error?: string;
}

interface ActiveTrackedTurn {
  token: DesktopTurnToken;
  text: string;
  sequence: number;
  turnId?: number;
  completed: boolean;
}

type TrackedSpeechListener = (event: TrackedSpeechEvent) => void;

export function createDesktopTurnToken(
  sessionId: string,
  sequence: number,
  purpose: TrackedPromptPurpose,
): DesktopTurnToken {
  return { sessionId, clientTurnId: `${sessionId}:tracked:${sequence}`, purpose };
}

export function enqueueTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  token: DesktopTurnToken,
): void {
  const queue = pending.get(token.sessionId);
  if (queue) queue.push(token);
  else pending.set(token.sessionId, [token]);
}

export function dequeueTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  token: DesktopTurnToken,
): void {
  const queue = pending.get(token.sessionId);
  if (!queue) return;
  const next = queue.filter((entry) => entry.clientTurnId !== token.clientTurnId);
  if (next.length === 0) pending.delete(token.sessionId);
  else pending.set(token.sessionId, next);
}

function eventClientTurnId(event: Extract<ClientEvent, { type: 'turn_started' }>): string | undefined {
  const clientTurnId = (event as { client_turn_id?: unknown }).client_turn_id;
  return typeof clientTurnId === 'string' && clientTurnId.length > 0 ? clientTurnId : undefined;
}

function trackedListenerKey(token: DesktopTurnToken): string {
  return token.clientTurnId;
}

function emitTrackedSpeech(
  listeners: Map<string, Set<TrackedSpeechListener>>,
  event: TrackedSpeechEvent,
): void {
  const subscribers = listeners.get(trackedListenerKey(event.token));
  if (!subscribers || subscribers.size === 0) return;
  for (const listener of [...subscribers]) listener(event);
}

export function bindTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
  event: Extract<ClientEvent, { type: 'turn_started' }>,
): ActiveTrackedTurn | null {
  const queue = pending.get(sessionId);
  if (!queue || queue.length === 0) return null;
  const explicitClientTurnId = eventClientTurnId(event);
  const index = explicitClientTurnId
    ? Math.max(0, queue.findIndex((entry) => entry.clientTurnId === explicitClientTurnId))
    : 0;
  const [token] = queue.splice(index, 1);
  if (!token) return null;
  if (queue.length === 0) pending.delete(sessionId);
  const tracked = { token, text: '', sequence: 0, turnId: event.turn_id, completed: false };
  active.set(sessionId, tracked);
  return tracked;
}

export function appendTrackedTurnDelta(
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
  text: string,
): ActiveTrackedTurn | null {
  if (!text) return null;
  const tracked = active.get(sessionId);
  if (!tracked || tracked.completed) return null;
  tracked.text += text;
  tracked.sequence += 1;
  return tracked;
}

export function completeTrackedTurn(
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
): ActiveTrackedTurn | null {
  const tracked = active.get(sessionId);
  if (!tracked || tracked.completed) return null;
  tracked.completed = true;
  active.delete(sessionId);
  return tracked;
}

export function clearTrackedTurnState(
  pending: Map<string, DesktopTurnToken[]>,
  active: Map<string, ActiveTrackedTurn>,
  listeners: Map<string, Set<TrackedSpeechListener>>,
  sessionId: string,
): DesktopTurnToken[] {
  const cleared: DesktopTurnToken[] = pending.get(sessionId) ?? [];
  pending.delete(sessionId);
  const tracked = active.get(sessionId);
  if (tracked) {
    active.delete(sessionId);
    cleared.push(tracked.token);
  }
  for (const token of cleared) listeners.delete(trackedListenerKey(token));
  return cleared;
}

function emptyRuntimeState(connection: ConnectionState = { status: 'idle' }): RuntimeState {
  return {
    connection,
    conversation: emptyConversation(),
    desktop: emptyDesktopState(),
    runtimeCenter: emptyRuntimeCenterState(),
    permissionQueue: [],
    computerAccessQueue: [],
    askUserQuestionQueue: [],
    resolvedPermissionIds: new Set(),
    resolvedAskUserQuestionIds: new Set(),
    isCancelling: false,
  };
}

export function useBridge(): UseBridge {
  const hostRef = useRef(getHost());
  const host = hostRef.current;
  const hosted = host !== undefined;
  const [loading, setLoading] = useState(hosted);
  const [bootstrap, setBootstrap] = useState<BootstrapState | null>(null);
  const [pendingSession, setPendingSession] = useState<SessionRef | null>(null);
  const [runtimeStates, setRuntimeStates] = useState<Map<string, RuntimeState>>(new Map());
  const [error, setError] = useState<string | null>(null);
  const clearError = useCallback(() => setError(null), []);
  const [audioSnapshot, setAudioSnapshot] = useState<NativeAudioSnapshot>(defaultNativeAudioSnapshot());
  // Settings are file-layer state, not per-conversation state, so this is
  // one value for the whole app rather than something keyed into `runtimeStates`.
  const [settingsSnapshotEvent, setSettingsSnapshotEvent] = useState<SettingsSnapshotEvent | null>(null);
  // Same "one value for the whole app" treatment as `settingsSnapshotEvent`
  // above: MCP server definitions and discovered skills are not per-session
  // state either.
  const [mcpServersEvent, setMcpServersEvent] = useState<McpServersEvent | null>(null);
  const [skillsEvent, setSkillsEvent] = useState<SkillsEvent | null>(null);
  const [skillCatalogEvent, setSkillCatalogEvent] = useState<SkillCatalogEvent | null>(null);
  const [skillDocumentEvent, setSkillDocumentEvent] = useState<SkillDocumentEvent | null>(null);
  const [mcpConfigurationSnapshotEvent, setMcpConfigurationSnapshotEvent] = useState<McpConfigurationSnapshotEvent | null>(null);
  const [pluginCatalogEvent, setPluginCatalogEvent] = useState<PluginCatalogEvent | null>(null);
  const [configurationOperations, setConfigurationOperations] = useState<Partial<Record<ConfigurationDomainDto, ConfigurationOperationEvent>>>({});
  const pendingConfigurationOperations = useRef(new Map<string, PendingConfigurationOperation>());
  const turnActiveRefs = useRef(new Map<string, boolean>());
  const slashPendingRefs = useRef(new Map<string, boolean>());
  const cancellingRefs = useRef(new Map<string, { current: boolean }>());
  const cancellationTasks = useRef(new Map<string, { current: Promise<void> | null }>());
  const removedRuntimeIds = useRef(new Set<string>());
  const bootstrapRef = useRef<BootstrapState | null>(null);
  const pendingSessionRef = useRef<SessionRef | null>(null);
  const sessionLoadingRef = useRef(false);
  const latestBootstrapRevisionRef = useRef<number | null>(null);
  const navigationOperationRef = useRef(0);
  const pinOperationRef = useRef(0);
  const restartOperationsRef = useRef(new Map<string, Promise<void>>());
  const catalogRefreshTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const catalogRequestGenerations = useRef(new Map<string, number>());
  const runtimeResourceSendSequence = useRef(0);
  const trackedTurnSequence = useRef(0);
  const pendingTrackedTurns = useRef(new Map<string, DesktopTurnToken[]>());
  const activeTrackedTurns = useRef(new Map<string, ActiveTrackedTurn>());
  const trackedSpeechListeners = useRef(new Map<string, Set<TrackedSpeechListener>>());

  useEffect(() => () => {
    for (const pending of pendingConfigurationOperations.current.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('configuration operation was interrupted'));
    }
    pendingConfigurationOperations.current.clear();
    for (const sessionId of new Set<string>([
      ...pendingTrackedTurns.current.keys(),
      ...activeTrackedTurns.current.keys(),
    ])) {
      const tracked = completeTrackedTurn(activeTrackedTurns.current, sessionId);
      if (tracked) {
        emitTrackedSpeech(trackedSpeechListeners.current, {
          type: 'completion',
          token: tracked.token,
          text: tracked.text,
          sequence: tracked.sequence,
          turnId: tracked.turnId,
          terminal: 'stale',
        });
      }
      clearTrackedTurnState(pendingTrackedTurns.current, activeTrackedTurns.current, trackedSpeechListeners.current, sessionId);
    }
  }, []);

  const persistedActiveSession = bootstrap?.activeSession ?? bootstrap?.settings.activeSession;
  const activeSession = displayedSession(persistedActiveSession, pendingSession);
  const sessionLoading = pendingSession !== null;
  const activeSessionId = activeSession?.sessionId ?? null;
  const providerSettingsSaves = useRef(new Set<AbortController>());
  useEffect(() => () => {
    for (const controller of providerSettingsSaves.current) controller.abort();
    providerSettingsSaves.current.clear();
  }, [activeSessionId]);
  const activeSessionIdRef = useRef<string | null>(null);
  activeSessionIdRef.current = activeSessionId;
  sessionLoadingRef.current = sessionLoading;
  bootstrapRef.current = bootstrap;

  useEffect(() => {
    let cancelled = false;
    if (!host?.audio) {
      setAudioSnapshot(defaultNativeAudioSnapshot());
      if (host) {
        void hostMicrophonePermissionReader(host)().then((microphone) => {
          if (!cancelled) setAudioSnapshot({
            ...defaultNativeAudioSnapshot(),
            permissions: { microphone, speech: 'unavailable' },
          });
        });
      }
      return () => { cancelled = true; };
    }
    void host.audio.request({ type: 'get_snapshot' }).then((response) => {
      if (!cancelled) setAudioSnapshot(response.snapshot);
    }).catch(() => {
      if (!cancelled) setAudioSnapshot(defaultNativeAudioSnapshot());
    });
    const offAudio = host.audio.onEvent((event) => {
      if (!cancelled && event.type !== 'input_level') setAudioSnapshot(event.snapshot);
    });
    return () => {
      cancelled = true;
      offAudio();
    };
  }, [host]);

  // Settings are captured per-CONNECTION on the engine side (`SettingsContext`
  // is built once per `assemble_with_provider_keys` call), so a snapshot from
  // a previous session/project is not a valid answer for a new one — without
  // this, switching sessions would leave the old session's `project`/`local`
  // values on screen until a fresh snapshot happened to arrive.
  useEffect(() => {
    setSettingsSnapshotEvent(null);
    setSkillCatalogEvent(null);
    setSkillDocumentEvent(null);
    setMcpConfigurationSnapshotEvent(null);
    setPluginCatalogEvent(null);
    setConfigurationOperations({});
  }, [activeSessionId]);

  const capture = useCallback((cause: unknown) => {
    const message = messageFrom(cause);
    setError(message);
    throw cause;
  }, []);

  const completeTrackedSpeech = useCallback((sessionId: string, terminal: TrackedSpeechTerminal): void => {
    const tracked = completeTrackedTurn(activeTrackedTurns.current, sessionId);
    if (!tracked) {
      if (terminal === 'stale') {
        clearTrackedTurnState(
          pendingTrackedTurns.current,
          activeTrackedTurns.current,
          trackedSpeechListeners.current,
          sessionId,
        );
      }
      return;
    }
    emitTrackedSpeech(trackedSpeechListeners.current, {
      type: 'completion',
      token: tracked.token,
      text: tracked.text,
      sequence: tracked.sequence,
      turnId: tracked.turnId,
      terminal,
    });
    trackedSpeechListeners.current.delete(trackedListenerKey(tracked.token));
    if (terminal === 'stale') {
      clearTrackedTurnState(
        pendingTrackedTurns.current,
        activeTrackedTurns.current,
        trackedSpeechListeners.current,
        sessionId,
      );
    }
  }, []);

  const runtime = activeSessionId ? runtimeStates.get(activeSessionId) : undefined;
  const connection: ConnectionState = sessionLoading
    ? { status: 'spawning' }
    : runtime?.connection ?? bootstrap?.connection ?? { status: 'idle' };
  const conversation = runtime?.conversation ?? emptyConversation();
  const desktop = runtime?.desktop ?? emptyDesktopState();
  const runtimeCenter = runtime?.runtimeCenter ?? emptyRuntimeCenterState();
  const permissionQueue = runtime?.permissionQueue ?? [];
  const computerAccessQueue = runtime?.computerAccessQueue ?? [];
  const askUserQuestionQueue = runtime?.askUserQuestionQueue ?? [];
  const isCancelling = runtime?.isCancelling ?? false;

  const updateRuntime = useCallback((sessionId: string, updater: (state: RuntimeState) => RuntimeState): void => {
    setRuntimeStates((previous) => {
      const current = previous.get(sessionId) ?? emptyRuntimeState();
      const next = updater(current);
      if (next === current) return previous;
      const copy = new Map(previous);
      copy.set(sessionId, next);
      return copy;
    });
  }, []);

  const acknowledgeInteraction = useCallback((
    sessionId: string,
    requestId: number,
    kind: 'permission' | 'computerAccess' | 'askUserQuestion',
  ): void => {
    const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
    updateRuntime(sessionId, (state) => {
      const next: RuntimeState = { ...state };
      if (kind === 'permission') {
        next.permissionQueue = state.permissionQueue.filter((entry) => entry.request_id !== requestId);
      } else if (kind === 'computerAccess') {
        next.computerAccessQueue = state.computerAccessQueue.filter((entry) => entry.request_id !== requestId);
      } else {
        next.askUserQuestionQueue = state.askUserQuestionQueue.filter((entry) => entry.request_id !== requestId);
        const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
        resolvedAskUserQuestionIds.add(requestId);
        next.resolvedAskUserQuestionIds = resolvedAskUserQuestionIds;
      }
      const localPendingInteractions = state.permissionQueue.length
        + state.computerAccessQueue.length
        + state.askUserQuestionQueue.length;
      const fallbackPendingInteractions = Math.max(summary?.pendingInteractions ?? 0, localPendingInteractions);
      next.pendingInteractionsOverride = pendingCountAfterResponse(
        state.pendingInteractionsOverride,
        fallbackPendingInteractions,
      );
      if (kind === 'askUserQuestion') {
        const fallbackPendingQuestions = Math.max(summary?.pendingAskUserQuestions ?? 0, state.askUserQuestionQueue.length);
        next.pendingAskUserQuestionsOverride = pendingCountAfterResponse(
          state.pendingAskUserQuestionsOverride,
          fallbackPendingQuestions,
        );
      }
      return next;
    });
  }, [updateRuntime]);

  const applyBootstrap = useCallback((snapshot: BootstrapState): void => {
    if (!shouldApplyBootstrapSnapshot(latestBootstrapRevisionRef.current, snapshot.revision)) return;
    latestBootstrapRevisionRef.current = Number.isFinite(snapshot.revision) ? snapshot.revision : 0;
    const pending = pendingSessionRef.current;
    setRuntimeStates((previous) => {
      const next = new Map(previous);
      const summaries = snapshot.runtimes ?? [];
      const runtimeIds = new Set(summaries.map((summary) => summary.sessionId));
      if (pending) runtimeIds.add(pending.sessionId);
      // The snapshot is authoritative, so disposal markers have been
      // reconciled once it arrives and must not accumulate across sessions.
      removedRuntimeIds.current.clear();
      pruneRuntimeMaps(runtimeIds, next, turnActiveRefs.current, slashPendingRefs.current, cancellingRefs.current, cancellationTasks.current);
      for (const sessionId of new Set<string>([
        ...pendingTrackedTurns.current.keys(),
        ...activeTrackedTurns.current.keys(),
      ])) {
        if (!runtimeIds.has(sessionId)) completeTrackedSpeech(sessionId, 'stale');
      }
      for (const summary of summaries) {
        const current = next.get(summary.sessionId) ?? emptyRuntimeState(summary.connection);
        const nextState: RuntimeState = {
          ...current,
          connection: summary.connection,
          ...(summary.connection.status === 'connected' ? { error: undefined } : {}),
        };
        const pendingInteractionsOverride = reconcilePendingCount(
          current.pendingInteractionsOverride,
          summary.pendingInteractions,
        );
        const pendingAskUserQuestionsOverride = reconcilePendingCount(
          current.pendingAskUserQuestionsOverride,
          summary.pendingAskUserQuestions,
        );
        if (pendingInteractionsOverride === undefined) delete nextState.pendingInteractionsOverride;
        else nextState.pendingInteractionsOverride = pendingInteractionsOverride;
        if (pendingAskUserQuestionsOverride === undefined) delete nextState.pendingAskUserQuestionsOverride;
        else nextState.pendingAskUserQuestionsOverride = pendingAskUserQuestionsOverride;
        next.set(summary.sessionId, nextState);
        turnActiveRefs.current.set(summary.sessionId, summary.turnActive);
      }
      const sessionId = pending?.sessionId ?? snapshot.activeSession?.sessionId ?? snapshot.settings.activeSession?.sessionId;
      if (!pending && sessionId && snapshot.pendingAskUserQuestions) {
        const current = next.get(sessionId) ?? emptyRuntimeState();
        next.set(sessionId, {
          ...current,
          askUserQuestionQueue: pendingAskQueueFromBootstrap(snapshot)
            .filter((request) => !current.resolvedAskUserQuestionIds.has(request.request_id)),
        });
      }
      return next;
    });
    setBootstrap(snapshot);
  }, [completeTrackedSpeech]);

  const beginNavigationOperation = useCallback((): number => {
    pendingSessionRef.current = null;
    sessionLoadingRef.current = false;
    setPendingSession(null);
    const operationId = nextOperationId(navigationOperationRef.current);
    navigationOperationRef.current = operationId;
    return operationId;
  }, []);

  const isCurrentNavigationOperation = useCallback((operationId: number): boolean => (
    isLatestOperation(operationId, navigationOperationRef.current)
  ), []);

  const patchBootstrap = useCallback((patch: Partial<BootstrapState>) => {
    setBootstrap((previous) => previous ? { ...previous, ...patch } : previous);
  }, []);

  // Returns the catalog it fetched as well as writing it into `bootstrap`. A
  // caller acting on a click cannot read the bootstrap it just triggered — the
  // `bridge` it closed over is that render's snapshot — so handing back the
  // value is what lets "switch to this project" open the newest session instead
  // of falling through to a stray new one.
  const listProjectSessions = useCallback(async (projectPath: string): Promise<ProjectSessionCatalogState | undefined> => {
    if (!host) return undefined;
    const generation = beginProjectCatalogRequest(catalogRequestGenerations.current, projectPath);
    const result = await host.listProjectSessions(projectPath);
    if (!isLatestProjectCatalogRequest(catalogRequestGenerations.current, projectPath, generation)) return undefined;
    const catalog: ProjectSessionCatalogState = {
      sessions: result.sessions.map((session) => ({ ...session })),
      ...(result.error ? { error: result.error } : {}),
    };
    setBootstrap((previous) => previous ? {
      ...previous,
      projectCatalogs: { ...previous.projectCatalogs, [result.projectPath]: catalog },
    } : previous);
    return catalog;
  }, [host]);

  const scheduleProjectCatalogRefresh = useCallback((sessionId: string): void => {
    const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
    const projectPath = summary?.projectPath
      ?? (bootstrapRef.current?.activeSession?.sessionId === sessionId ? bootstrapRef.current.activeSession.projectPath : undefined);
    if (!projectPath || !host) return;
    const previous = catalogRefreshTimers.current.get(projectPath);
    if (previous) clearTimeout(previous);
    const timer = setTimeout(() => {
      catalogRefreshTimers.current.delete(projectPath);
      void listProjectSessions(projectPath).catch((cause) => setError(messageFrom(cause)));
    }, 250);
    timer.unref?.();
    catalogRefreshTimers.current.set(projectPath, timer);
  }, [host, listProjectSessions]);

  const requestTaskList = useCallback(async (
    sessionId = activeSessionIdRef.current,
    options?: { preserve?: boolean },
  ) => {
    if (sessionLoadingRef.current || !host || !sessionId) return;
    // [Finding 5, rework round 2] A background poll (fired every 1.5s while
    // a task is in flight) must NOT wipe `tasks`/`taskOutput` before the
    // fresh `task_row` lands -- clearing here made `orderedTasks(...)` (and
    // therefore `hasActiveTask`/`hasInFlightTask`) flip false for the one
    // render between the wipe and the reply, which re-triggered any effect
    // keyed on that flag and produced a self-amplifying task_list storm.
    // Only the user-initiated, one-shot refreshes (session connect, the
    // manual refresh button, opening the Tasks pane) still wipe first, the
    // way every other `refresh_listings` call in this file already does.
    if (!options?.preserve) {
      updateRuntime(sessionId, (state) => {
        const desktopState = beginTaskRefresh(state.desktop);
        return desktopState === state.desktop ? state : { ...state, desktop: desktopState };
      });
    }
    try {
      await host.command(sessionId, { type: 'task_list' });
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host, updateRuntime]);

  useEffect(() => {
    if (!host) {
      setLoading(false);
      setError('LingXi Desktop must run inside the signed Electron application.');
      return;
    }

    const offEvent = host.onEvent((envelope: SequencedRuntimeEventEnvelope<ClientEvent>) => {
      const sessionId = envelope.sessionId;
      if (removedRuntimeIds.current.has(sessionId)) return;
      const event = envelope.event;
      if (event.type === 'turn_started') turnActiveRefs.current.set(sessionId, true);
      if (event.type === 'turn_ended' || event.type === 'session_ended') turnActiveRefs.current.set(sessionId, false);
      if (event.type === 'turn_started') {
        if (activeTrackedTurns.current.has(sessionId)) completeTrackedSpeech(sessionId, 'stale');
        bindTrackedTurn(pendingTrackedTurns.current, activeTrackedTurns.current, sessionId, event);
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
      }
      // The reducer resets `pendingSlashName` to null on every one of these
      // three events (a fresh `emptyConversation()`/`conversationFromMessages`
      // state). The refs half of the claim must reset in lockstep, or a stale
      // `slashPendingRefs` entry can survive a session reset and later arm the
      // `error` release branch below against an unrelated turn.
      if (event.type === 'session_started' || event.type === 'session_ended' || event.type === 'session_resumed') {
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
      }
      if (event.type === 'slash_command_result' && shouldReleaseSlashTurn(slashPendingRefs.current, sessionId)) {
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
        turnActiveRefs.current.set(sessionId, false);
        // This release is terminal for the session's cancellation bookkeeping
        // too: a display-only command (e.g. `/status`) makes Stop the user's
        // only affordance while the composer is locked, and pressing it sets
        // `cancelling.current`/`isCancelling` with only `turn_ended` wired to
        // clear them (`:587-597`) -- which a display-only command never
        // produces. Left set, `cancelling.current` never resets, so `cancel()`
        // (`:800`) early-returns forever after, and `isCancelling` renders the
        // Stop button `disabled` on the user's next real turn.
        const cancelling = cancellingRefs.current.get(sessionId);
        const task = cancellationTasks.current.get(sessionId);
        if (cancelling && task) clearCancellationRuntime(cancelling, task);
        updateRuntime(sessionId, (state) => ({ ...state, isCancelling: false }));
      }
      // A dispatch that never reaches the engine, or an engine with no
      // dispatcher wired (`bridge-server/src/router.rs:953` emits `error`
      // instead of `slash_command_result`), is this claim's only terminal
      // event -- no `turn_started`/`turn_ended` will ever arrive to release
      // it otherwise. Same guard as above: only while the claim is still
      // outstanding, so an ordinary turn's error is untouched.
      if (event.type === 'error' && shouldReleaseSlashTurn(slashPendingRefs.current, sessionId)) {
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
        turnActiveRefs.current.set(sessionId, false);
        // Same terminal-release reasoning as the slash_command_result branch
        // above: this error is the claim's only terminal event, so it must
        // also reset the cancellation bookkeeping it may have armed.
        const cancelling = cancellingRefs.current.get(sessionId);
        const task = cancellationTasks.current.get(sessionId);
        if (cancelling && task) clearCancellationRuntime(cancelling, task);
        updateRuntime(sessionId, (state) => ({ ...state, isCancelling: false }));
      }
      if (event.type === 'text_delta') {
        const tracked = appendTrackedTurnDelta(activeTrackedTurns.current, sessionId, event.text);
        if (tracked) {
          emitTrackedSpeech(trackedSpeechListeners.current, {
            type: 'delta',
            token: tracked.token,
            text: event.text,
            sequence: tracked.sequence,
            turnId: tracked.turnId,
          });
        }
      }
      if (event.type === 'message_complete') completeTrackedSpeech(sessionId, 'message_complete');
      if (event.type === 'turn_ended' || event.type === 'session_ended' || event.type === 'session_started' || event.type === 'session_resumed') {
        completeTrackedSpeech(sessionId, event.type === 'turn_ended' ? 'turn_ended' : 'stale');
      }
      updateRuntime(sessionId, (state) => {
        let next = { ...state, conversation: reduceEvent(state.conversation, event), desktop: reduceDesktopEvent(state.desktop, event), runtimeCenter: reduceRuntimeCenterEvent(state.runtimeCenter, event, sessionId) };
        if (event.type === 'session_resumed') {
          next = {
            ...next,
            runtimeCenter: addRuntimeResources(
              next.runtimeCenter,
              resourcesFromRestoredMessages(sessionId, event.messages),
            ),
          };
        }
        if (event.type === 'turn_started') next = { ...next, error: undefined };
        if (event.type === 'ask_user_question') {
          const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
          resolvedAskUserQuestionIds.delete(event.request.request_id);
          const alreadyQueued = state.askUserQuestionQueue.some((entry) => entry.request_id === event.request.request_id);
          next = {
            ...next,
            resolvedAskUserQuestionIds,
            askUserQuestionQueue: [
              ...state.askUserQuestionQueue.filter((entry) => entry.request_id !== event.request.request_id),
              event.request,
            ],
            ...(state.pendingInteractionsOverride !== undefined && !alreadyQueued
              ? { pendingInteractionsOverride: state.pendingInteractionsOverride + 1 }
              : {}),
            ...(state.pendingAskUserQuestionsOverride !== undefined && !alreadyQueued
              ? { pendingAskUserQuestionsOverride: state.pendingAskUserQuestionsOverride + 1 }
              : {}),
          };
        }
        if (event.type === 'ask_user_question_resolved') {
          const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
          resolvedAskUserQuestionIds.add(event.request_id);
          const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
          next = {
            ...next,
            askUserQuestionQueue: state.askUserQuestionQueue.filter((entry) => entry.request_id !== event.request_id),
            resolvedAskUserQuestionIds,
            pendingInteractionsOverride: pendingCountAfterResponse(
              state.pendingInteractionsOverride,
              Math.max(
                summary?.pendingInteractions ?? 0,
                state.permissionQueue.length + state.computerAccessQueue.length + state.askUserQuestionQueue.length,
              ),
            ),
            pendingAskUserQuestionsOverride: pendingCountAfterResponse(
              state.pendingAskUserQuestionsOverride,
              Math.max(summary?.pendingAskUserQuestions ?? 0, state.askUserQuestionQueue.length),
            ),
          };
        }
        if (event.type === 'permission_request_resolved') {
          const resolvedPermissionIds = new Set(state.resolvedPermissionIds);
          resolvedPermissionIds.add(event.request_id);
          const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
          next = {
            ...next,
            permissionQueue: state.permissionQueue.filter((entry) => entry.request_id !== event.request_id),
            resolvedPermissionIds,
            pendingInteractionsOverride: pendingCountAfterResponse(
              state.pendingInteractionsOverride,
              Math.max(
                summary?.pendingInteractions ?? 0,
                state.permissionQueue.length + state.computerAccessQueue.length + state.askUserQuestionQueue.length,
              ),
            ),
          };
        }
        if (event.type === 'turn_ended' || event.type === 'session_ended') {
          next = {
            ...next,
            permissionQueue: [],
            computerAccessQueue: [],
            askUserQuestionQueue: [],
            pendingInteractionsOverride: 0,
            pendingAskUserQuestionsOverride: 0,
            resolvedPermissionIds: new Set(),
            resolvedAskUserQuestionIds: new Set(),
            isCancelling: false,
          };
        }
        return next;
      });
      if (event.type === 'session_started' || event.type === 'turn_ended') scheduleProjectCatalogRefresh(sessionId);
      if (event.type === 'settings_snapshot' && activeSessionIdRef.current === sessionId) setSettingsSnapshotEvent(event);
      if (event.type === 'mcp_servers' && activeSessionIdRef.current === sessionId) setMcpServersEvent(event);
      if (event.type === 'skills' && activeSessionIdRef.current === sessionId) setSkillsEvent(event);
      if (activeSessionIdRef.current === sessionId) {
        if (event.type === 'skill_catalog') setSkillCatalogEvent(event);
        if (event.type === 'skill_document') setSkillDocumentEvent(event);
        if (event.type === 'mcp_configuration_snapshot') setMcpConfigurationSnapshotEvent(event);
        if (event.type === 'plugin_catalog') setPluginCatalogEvent(event);
        if (event.type === 'configuration_operation') {
          setConfigurationOperations((previous) => ({ ...previous, [event.domain]: event }));
          if (event.status === 'succeeded' || event.status === 'failed') {
            const key = `${event.domain}:${event.operation_id}`;
            const pending = pendingConfigurationOperations.current.get(key);
            if (pending) {
              clearTimeout(pending.timer);
              pendingConfigurationOperations.current.delete(key);
              if (event.status === 'succeeded') pending.resolve(event);
              else pending.reject(new Error(event.message ?? `${event.domain} configuration operation failed`));
            }
          }
        }
      }
      if (event.type === 'error' && !/^force_compact failed:\s*/i.test(event.message)) {
        updateRuntime(sessionId, (state) => ({ ...state, error: event.message }));
        if (activeSessionIdRef.current === sessionId) setError(event.message);
      }
      if (event.type === 'audio_request') {
        const result = host.audio
          ? host.audio.executeEngineRequest(sessionId, event.op)
          : Promise.resolve(event.op.type === 'is_recording'
              ? { type: 'recording_state', recording: false } as const
              : {
                  type: 'failed',
                  kind: 'unavailable',
                  message: 'native audio is unavailable on this host',
                } as const);
        void result
          .catch((cause) => {
            const message = messageFrom(cause);
            updateRuntime(sessionId, (state) => ({ ...state, error: message }));
            if (activeSessionIdRef.current === sessionId) setError(message);
            return { type: 'failed', kind: 'other', message } as const;
          })
          .then((result) => host.command(sessionId, {
            type: 'audio_response',
            request_id: event.request_id,
            result,
          }))
          .catch((cause) => {
            const message = messageFrom(cause);
            updateRuntime(sessionId, (state) => ({ ...state, error: message }));
            if (activeSessionIdRef.current === sessionId) setError(message);
          });
      }
    });
    const offState = host.onConnectionStateChanged((envelope) => {
      const sessionId = envelope.sessionId;
      const state = envelope.event;
      if (isRuntimeRemovedState(state)) {
        removedRuntimeIds.current.add(sessionId);
        setBootstrap((previous) => {
          if (!previous) return previous;
          const previousActiveSession = previous.activeSession ?? previous.settings.activeSession;
          const activeSession = previousActiveSession?.sessionId === sessionId ? undefined : previous.activeSession;
          const settings = previous.settings.activeSession?.sessionId === sessionId
            ? { ...previous.settings, activeSession: undefined }
            : previous.settings;
          return {
            ...previous,
            settings,
            ...(activeSession ? { activeSession } : {}),
            runtimes: previous.runtimes.filter((runtime) => runtime.sessionId !== sessionId),
          };
        });
        setRuntimeStates((previous) => {
          const next = new Map(previous);
          removeRuntimeFromMaps(sessionId, next);
          return next.size === previous.size ? previous : next;
        });
        removeRuntimeFromMaps(sessionId, turnActiveRefs.current, slashPendingRefs.current, cancellingRefs.current, cancellationTasks.current);
        completeTrackedSpeech(sessionId, 'stale');
        return;
      }
      if (removedRuntimeIds.current.has(sessionId)) return;
      updateRuntime(sessionId, (current) => {
        const next: RuntimeState = { ...current, connection: state };
        if (shouldResetBridgeRuntime(state)) {
          const reset = emptyRuntimeState(state);
          return {
            ...reset,
            connection: state,
            runtimeCenter: resetRuntimeCenterConnection(current.runtimeCenter),
          };
        }
        if (shouldClearPendingPermissions(state)) {
          next.conversation = reduceEvent(current.conversation, {
            type: 'compaction_status', phase: 'error',
            error: 'Connection lost during compaction',
          });
          next.permissionQueue = [];
          next.computerAccessQueue = [];
          next.askUserQuestionQueue = [];
          next.pendingInteractionsOverride = 0;
          next.pendingAskUserQuestionsOverride = 0;
          next.resolvedPermissionIds = new Set();
          next.resolvedAskUserQuestionIds = new Set();
          next.isCancelling = false;
        }
        return next;
      });
      if (shouldClearPendingPermissions(state)) {
        turnActiveRefs.current.set(sessionId, false);
        // Same lockstep requirement as the session-event reset above: a
        // connection reset (respawn/disconnect/error/idle) clears the turn
        // claim, so the outstanding slash claim it may have been carrying
        // must be cleared with it.
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
        completeTrackedSpeech(sessionId, 'stale');
      }
      if (activeSessionIdRef.current === sessionId && state.status === 'error') setError(state.message);
      if (activeSessionIdRef.current === sessionId && state.status === 'disconnected' && state.reason) setError(state.reason);
      if (state.status === 'connected') {
        void host.bootstrap().then(applyBootstrap).catch((cause) => setError(messageFrom(cause)));
      }
    });
    const offPermission = host.onPermission((envelope) => {
      if (removedRuntimeIds.current.has(envelope.sessionId)) return;
      updateRuntime(envelope.sessionId, (state) => {
        if (state.resolvedPermissionIds.has(envelope.event.request_id)) return state;
        const alreadyQueued = state.permissionQueue.some((entry) => entry.request_id === envelope.event.request_id);
        return {
          ...state,
          runtimeCenter: reduceRuntimeCenterPermission(state.runtimeCenter, envelope.event, envelope.sessionId),
          permissionQueue: [
            ...state.permissionQueue.filter((entry) => entry.request_id !== envelope.event.request_id),
            envelope.event,
          ],
          ...(state.pendingInteractionsOverride !== undefined && !alreadyQueued
            ? { pendingInteractionsOverride: state.pendingInteractionsOverride + 1 }
            : {}),
        };
      });
    });
    const offComputerAccess = host.onComputerAccess((envelope) => {
      if (removedRuntimeIds.current.has(envelope.sessionId)) return;
      updateRuntime(envelope.sessionId, (state) => {
        const alreadyQueued = state.computerAccessQueue.some((entry) => entry.request_id === envelope.event.request_id);
        return {
          ...state,
          computerAccessQueue: [
            ...state.computerAccessQueue.filter((entry) => entry.request_id !== envelope.event.request_id),
            envelope.event,
          ],
          ...(state.pendingInteractionsOverride !== undefined && !alreadyQueued
            ? { pendingInteractionsOverride: state.pendingInteractionsOverride + 1 }
            : {}),
        };
      });
    });

    void host.bootstrap()
      .then((snapshot) => {
        applyBootstrap(snapshot);
        setLoading(false);
      })
      .catch((cause) => {
        setError(messageFrom(cause));
        setLoading(false);
      });

    return () => {
      offEvent();
      offState();
      offPermission();
      offComputerAccess();
    };
  }, [applyBootstrap, capture, completeTrackedSpeech, host, scheduleProjectCatalogRefresh, updateRuntime]);

  useEffect(() => {
    if (sessionLoading || !host || !activeSessionId || connection.status !== 'connected') return;
    const trusted = bootstrap?.workspace.trusted;
    if (!trusted) return;
    void Promise.all([
      host.command(activeSessionId, { type: 'list_sessions', limit: 100 }),
      host.command(activeSessionId, { type: 'list_models' }),
      host.command(activeSessionId, { type: 'get_conversation_controls' }),
      requestTaskList(activeSessionId),
      host.command(activeSessionId, { type: 'list_session_agents' }),
      host.command(activeSessionId, {
        type: 'refresh_listings',
        which: [{ type: 'auth' }, { type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }, { type: 'hooks' }, { type: 'agents' }],
      }),
      host.command(activeSessionId, { type: 'refresh_listings', which: [{ type: 'mcp' }, { type: 'skills' }] }),
      host.command(activeSessionId, { type: 'skill_admin', command: { action: 'get_catalog' } }),
      host.command(activeSessionId, { type: 'mcp_admin', command: { action: 'get_snapshot' } }),
      host.command(activeSessionId, { type: 'plugin_admin', command: { action: 'get_catalog' } }),
      host.command(activeSessionId, { type: 'hook_admin', command: { action: 'get_document' } }),
    ]).catch((cause) => setError(messageFrom(cause)));
  }, [activeSessionId, bootstrap?.workspace.trusted, connection.status, host, requestTaskList, sessionLoading]);

  const sendTrackedPrompt = useCallback((
    text: string,
    images: ImageRefDto[] = [],
    imageNames: string[] = [],
    filePaths: string[] = [],
    options: { purpose?: TrackedPromptPurpose } = {},
  ): { token: DesktopTurnToken; queued: Promise<void> } | null => {
    const trimmed = text.trim();
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !trimmed || !host || !sessionId) return null;
    const wasTurnActive = turnActiveRefs.current.get(sessionId) === true;
    turnActiveRefs.current.set(sessionId, true);
    runtimeResourceSendSequence.current += 1;
    trackedTurnSequence.current += 1;
    const token = createDesktopTurnToken(sessionId, trackedTurnSequence.current, options.purpose ?? 'composer');
    enqueueTrackedTurn(pendingTrackedTurns.current, token);
    const sendToken = `${sessionId}:${runtimeResourceSendSequence.current}`;
    const resources = promptRuntimeResources(sessionId, sendToken, images, imageNames, filePaths);
    const sessionRef = bootstrapRef.current?.runtimes.find((entry) => entry.sessionId === sessionId)
      ?? activeSession;
    const saved = sessionRef && bootstrapRef.current?.projectCatalogs[sessionRef.projectPath]?.sessions.find((entry) => entry.uuid === sessionId);
    const submittedSession: SubmittedSession | undefined = sessionRef && (!saved || saved.message_count === 0) ? {
      projectPath: sessionRef.projectPath,
      row: {
        uuid: sessionId, title: trimmed.replace(/\s+/g, ' ').slice(0, 120),
        modified_rfc3339: new Date().toISOString(), message_count: 1,
        mode: saved?.mode ?? 'code', path: saved?.path ?? '',
      },
    } : undefined;
    updateRuntime(sessionId, (state) => ({
      ...state,
      submittedSession: state.submittedSession ?? submittedSession,
      conversation: appendPendingUserPrompt(state.conversation, trimmed, images),
      runtimeCenter: addRuntimeResources(state.runtimeCenter, resources),
    }));
    const queued = host.sendPrompt(sessionId, trimmed, images).then(() => {
      if (removedRuntimeIds.current.has(sessionId)) return;
      updateRuntime(sessionId, (state) => ({
        ...state,
        runtimeCenter: commitRuntimeResources(state.runtimeCenter, sendToken),
      }));
    }).catch((cause) => {
      turnActiveRefs.current.set(sessionId, wasTurnActive);
      dequeueTrackedTurn(pendingTrackedTurns.current, token);
      trackedSpeechListeners.current.delete(trackedListenerKey(token));
      if (!removedRuntimeIds.current.has(sessionId)) {
        updateRuntime(sessionId, (state) => ({
          ...state,
          conversation: {
            ...reduceEvent(state.conversation, wasTurnActive
              ? { type: 'system_notice', message: 'Failed to queue the pending message.', is_error: true }
              : { type: 'error', kind: { type: 'transport' }, message: 'Failed to send the prompt to the engine.' }),
            running: wasTurnActive,
          },
          runtimeCenter: rollbackRuntimeResources(state.runtimeCenter, sendToken),
        }));
      }
      capture(cause);
      throw cause;
    });
    return { token, queued };
  }, [activeSession, capture, host, updateRuntime]);

  const sendPrompt = useCallback(async (
    text: string,
    images: ImageRefDto[] = [],
    imageNames: string[] = [],
    filePaths: string[] = [],
  ) => {
    const tracked = sendTrackedPrompt(text, images, imageNames, filePaths);
    if (!tracked) return;
    await tracked.queued;
  }, [sendTrackedPrompt]);

  const subscribeTrackedSpeech = useCallback((token: DesktopTurnToken, listener: TrackedSpeechListener): (() => void) => {
    const key = trackedListenerKey(token);
    const listeners = trackedSpeechListeners.current.get(key) ?? new Set<TrackedSpeechListener>();
    if (listeners.size >= MAX_TRACKED_SPEECH_SUBSCRIBERS) {
      throw new Error('too many tracked speech subscribers for one turn');
    }
    listeners.add(listener);
    trackedSpeechListeners.current.set(key, listeners);
    return () => {
      const current = trackedSpeechListeners.current.get(key);
      if (!current) return;
      current.delete(listener);
      if (current.size === 0) trackedSpeechListeners.current.delete(key);
    };
  }, []);

  const runSlashCommand = useCallback(async (raw: string) => {
    const command = raw.trim();
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId || !command.startsWith('/')) return;
    turnActiveRefs.current.set(sessionId, true);
    claimSlashTurn(slashPendingRefs.current, sessionId);
    updateRuntime(sessionId, (state) => ({ ...state, conversation: beginSlashCommand(state.conversation, command) }));
    try {
      await host.command(sessionId, { type: 'run_slash_command', raw: command });
    } catch (cause) {
      // A genuine dispatch failure: the command never ran, so release both
      // claims and surface the error exactly as before. This is also
      // terminal for the session's cancellation bookkeeping -- see the
      // matching reset on the slash_command_result/error release branches
      // in the event fan-out above.
      turnActiveRefs.current.set(sessionId, false);
      clearSlashTurnClaim(slashPendingRefs.current, sessionId);
      {
        const cancelling = cancellingRefs.current.get(sessionId);
        const task = cancellationTasks.current.get(sessionId);
        if (cancelling && task) clearCancellationRuntime(cancelling, task);
      }
      updateRuntime(sessionId, (state) => ({
        ...state,
        conversation: reduceEvent(state.conversation, { type: 'error', kind: { type: 'transport' }, message: 'Failed to run the slash command.' }),
        isCancelling: false,
      }));
      capture(cause);
      return;
    }
    try {
      // Best-effort refresh of the slash-command listing (a command can
      // register/deregister others). It runs after dispatch already
      // succeeded — possibly starting a real turn — so its own failure must
      // not mislabel that success as a dispatch failure, nor release a claim
      // or turn that may still be live.
      await host.command(sessionId, { type: 'refresh_listings', which: [{ type: 'slash_commands' }] });
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host, updateRuntime]);

  const beginLocalCommand = useCallback((raw: string) => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({ ...state, conversation: beginLocalSlashCommand(state.conversation, raw) }));
  }, [updateRuntime]);

  const emitCommandOutput = useCallback((output: string, isError: boolean) => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({
      ...state,
      conversation: reduceEvent(state.conversation, { type: 'slash_command_result', display: output, is_error: isError }),
    }));
  }, [updateRuntime]);

  const cancel = useCallback((turnId?: number): Promise<void> => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId || !turnActiveRefs.current.get(sessionId)) return Promise.resolve();
    const cancelling = cancellingRefs.current.get(sessionId) ?? { current: false };
    const taskRef = cancellationTasks.current.get(sessionId) ?? { current: null };
    cancellingRefs.current.set(sessionId, cancelling);
    cancellationTasks.current.set(sessionId, taskRef);
    if (cancelling.current) return taskRef.current ?? Promise.resolve();
    cancelling.current = true;
    updateRuntime(sessionId, (state) => ({ ...state, isCancelling: true }));
    let task: Promise<void>;
    task = host.cancel(sessionId, turnId).then(() => {
      updateRuntime(sessionId, (state) => ({
        ...state,
        permissionQueue: [],
        computerAccessQueue: [],
        askUserQuestionQueue: [],
        pendingInteractionsOverride: 0,
        pendingAskUserQuestionsOverride: 0,
        resolvedPermissionIds: new Set(),
        resolvedAskUserQuestionIds: new Set(),
      }));
    }).catch((cause) => {
      if (taskRef.current === task) {
        cancelling.current = false;
        taskRef.current = null;
        updateRuntime(sessionId, (state) => ({ ...state, isCancelling: false }));
      }
      capture(cause);
    });
    taskRef.current = task;
    return task;
  }, [capture, host, updateRuntime]);

  const approve = useCallback(async (requestId: number, response?: PermissionResponseDto) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.approve(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'permission'); }
    catch (cause) {
      if (isPermissionRequestGone(cause)) {
        acknowledgeInteraction(sessionId, requestId, 'permission');
        return;
      }
      capture(cause);
    }
  }, [acknowledgeInteraction, capture, host]);

  const deny = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.deny(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'permission'); }
    catch (cause) {
      if (isPermissionRequestGone(cause)) {
        acknowledgeInteraction(sessionId, requestId, 'permission');
        return;
      }
      capture(cause);
    }
  }, [acknowledgeInteraction, capture, host]);

  const approveComputerAccess = useCallback(async (requestId: number, response: ComputerAccessResponseDto) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.approveComputerAccess(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'computerAccess'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const denyComputerAccess = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.denyComputerAccess(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'computerAccess'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const answerAskUserQuestion = useCallback(async (requestId: number, answers: Record<string, string>) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.answerAskUserQuestion(sessionId, requestId, answers); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const cancelAskUserQuestion = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.cancelAskUserQuestion(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const openSystemSettings = useCallback(async (pane: SystemSettingsPane) => {
    if (!host) return;
    try { await host.openSystemSettings(pane); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const microphonePermission = useCallback(async (): Promise<VoicePermissionStatus> => {
    try { return await hostMicrophonePermissionReader(host)(); } catch { return 'unavailable'; }
  }, [host]);

  const addProject = useCallback(async () => {
    if (!host) return null;
    const operationId = beginNavigationOperation();
    try {
      const workspace = await host.pickWorkspace();
      if (workspace && isCurrentNavigationOperation(operationId)) {
        const snapshot = await host.bootstrap();
        if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot);
      }
      return workspace;
    } catch (cause) {
      // The host persists the first project before starting its initial
      // session. Recover that saved project even if engine startup fails.
      if (isCurrentNavigationOperation(operationId)) {
        try {
          const snapshot = await host.bootstrap();
          if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot);
        } catch { /* Preserve the original project/startup error. */ }
      }
      if (isCurrentNavigationOperation(operationId)) return capture(cause);
      return null;
    }
  }, [applyBootstrap, beginNavigationOperation, capture, host, isCurrentNavigationOperation]);

  const activateProject = useCallback(async (path: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    const operationId = beginNavigationOperation();
    try {
      const workspace = await host.setWorkspace(path);
      const settings = await host.settings();
      if (isCurrentNavigationOperation(operationId)) patchBootstrap({ settings });
      return workspace;
    } catch (cause) {
      if (isCurrentNavigationOperation(operationId)) return capture(cause);
      return null;
    }
  }, [beginNavigationOperation, capture, host, isCurrentNavigationOperation, patchBootstrap]);

  const removeProject = useCallback(async (path: string) => {
    if (!host) return;
    const operationId = beginNavigationOperation();
    try {
      const snapshot = await host.removeProject(path);
      if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot);
    }
    catch (cause) { if (isCurrentNavigationOperation(operationId)) capture(cause); }
  }, [applyBootstrap, beginNavigationOperation, capture, host, isCurrentNavigationOperation]);

  const preflightSessionArchive = useCallback((projectPath: string, sessionId: string) => {
    if (!host) return Promise.reject(new Error('Desktop host unavailable.'));
    return host.preflightSessionArchive(projectPath, sessionId);
  }, [host]);
  const archiveSession = useCallback(async (projectPath: string, sessionId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    const operationId = beginNavigationOperation();
    try {
      const snapshot = await host.archiveSession(projectPath, sessionId);
      if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot);
    } catch (error) {
      try { const snapshot = await host.bootstrap(); if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot); } catch { /* Preserve the original archival error. */ }
      throw error;
    }
  }, [host, beginNavigationOperation, isCurrentNavigationOperation, applyBootstrap]);
  const setSessionPinned = useCallback(async (session: SessionPinInput, pinned: boolean) => {
    if (!host) return;
    const operationId = nextOperationId(pinOperationRef.current);
    pinOperationRef.current = operationId;
    try {
      const settings = await host.setSessionPinned(session, pinned);
      if (isLatestOperation(operationId, pinOperationRef.current)) patchBootstrap({ settings });
    }
    catch (cause) { if (isLatestOperation(operationId, pinOperationRef.current)) capture(cause); }
  }, [capture, host, patchBootstrap]);

  const openSession = useCallback(async (projectPath: string, sessionId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    const operationId = beginNavigationOperation();
    const target = { projectPath, sessionId } satisfies SessionRef;
    pendingSessionRef.current = target;
    sessionLoadingRef.current = true;
    setPendingSession(target);
    removedRuntimeIds.current.delete(sessionId);
    try {
      const snapshot = await host.openSession(projectPath, sessionId);
      if (isCurrentNavigationOperation(operationId)) {
        pendingSessionRef.current = null;
        sessionLoadingRef.current = false;
        setPendingSession(null);
        applyBootstrap(snapshot);
        setError(null);
      }
    }
    catch (cause) {
      if (!isCurrentNavigationOperation(operationId)) return;
      pendingSessionRef.current = null;
      sessionLoadingRef.current = false;
      setPendingSession(null);
      try {
        await recoverLatestNavigationFailure(
          operationId,
          isCurrentNavigationOperation,
          () => host.bootstrap(),
          applyBootstrap,
        );
      } catch {
        // Preserve the navigation failure as the user-facing error. A bootstrap
        // refresh failure cannot make the stale local selection authoritative.
      }
      if (isCurrentNavigationOperation(operationId)) capture(cause);
    }
  }, [applyBootstrap, beginNavigationOperation, capture, host, isCurrentNavigationOperation]);

  const sessionRuntimeStatus = useCallback((sessionId: string): SessionRuntimeStatus | undefined => {
    const summary: SessionRuntimeSummary | undefined = bootstrap?.runtimes.find((runtime) => runtime.sessionId === sessionId);
    const state = runtimeStates.get(sessionId);
    if (!summary && !state) return undefined;
    return {
      backgroundAgentsRunning: Object.values(state?.runtimeCenter.agents ?? {}).some((agent) => agent.agent_id !== 'main' && ['running', 'working', 'in_progress'].includes(agent.status)),
      connection: state?.connection ?? summary?.connection ?? { status: 'idle' },
      turnActive: turnActiveRefs.current.get(sessionId) ?? summary?.turnActive ?? false,
      pendingInteractions: state?.pendingInteractionsOverride
        ?? (state
          ? Math.max(
            state.permissionQueue.length + state.computerAccessQueue.length + state.askUserQuestionQueue.length,
            summary?.pendingInteractions ?? 0,
          )
          : summary?.pendingInteractions ?? 0),
      pendingAskUserQuestions: state?.pendingAskUserQuestionsOverride
        ?? (state
          ? Math.max(state.askUserQuestionQueue.length, summary?.pendingAskUserQuestions ?? 0)
          : summary?.pendingAskUserQuestions ?? 0),
      ...(state?.error ? { error: state.error } : {}),
    };
  }, [bootstrap, runtimeStates]);

  const searchWorkspaceFiles = useCallback(async (query: string) => {
    if (!host) return { files: [], truncated: false };
    try { return await host.searchWorkspaceFiles(query); } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const setProviderCredential = useCallback(async (providerId: string, credential: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const update = await host.setProviderCredential(providerId, credential);
      setBootstrap((previous) => previous ? {
        ...previous, settings: update.settings,
        providerCredentials: [...(previous.providerCredentials ?? []).filter((entry) => entry.providerId !== providerId), update.credential],
      } : previous);
      return update;
    } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const clearProviderCredential = useCallback(async (providerId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const metadata = await host.clearProviderCredential(providerId);
      setBootstrap((previous) => previous ? {
        ...previous, providerCredentials: [...(previous.providerCredentials ?? []).filter((entry) => entry.providerId !== providerId), metadata],
      } : previous);
      return metadata;
    } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const testProviderConnection = useCallback(async (providerId: string, credentialOverride?: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try { return await host.testProviderConnection(providerId, credentialOverride); }
    catch (cause) { return capture(cause); }
  }, [capture, host]);

  const refreshProviderCredential = useCallback(async (providerId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      patchBootstrap({ providerCredentials: await host.providerCredentials(providerId) });
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const pluginSecret = useCallback(async (pluginId: string, key: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try { return await host.pluginSecret(pluginId, key); }
    catch (cause) { return capture(cause); }
  }, [capture, host]);

  const setPluginSecret = useCallback(async (pluginId: string, key: string, secret: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try { return await host.setPluginSecret(pluginId, key, secret); }
    catch (cause) { return capture(cause); }
  }, [capture, host]);

  const clearPluginSecret = useCallback(async (pluginId: string, key: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try { return await host.clearPluginSecret(pluginId, key); }
    catch (cause) { return capture(cause); }
  }, [capture, host]);

  const setThemePreference = useCallback(async (theme: 'dark' | 'light' | 'system') => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ theme }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setCollapseThoughtsByDefault = useCallback(async (collapseThoughtsByDefault: boolean) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ collapseThoughtsByDefault }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setApiBaseUrl = useCallback(async (apiBaseUrl: string | null) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ apiBaseUrl }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setVoicePreferences = useCallback(async (voice: VoicePreferences) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ voice }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setNotificationPreferences = useCallback(async (notifications: NotificationPreferences) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ notifications }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setModelPickerVisibility = useCallback(async (modelPickerVisibility: ModelPickerVisibilitySettings) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ modelPickerVisibility }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const restartBridge = useCallback(async (requestedSessionId?: string) => {
    const sessionId = requestedSessionId ?? activeSessionIdRef.current;
    const preconditionError = restartBridgePreconditionError(sessionLoadingRef.current, Boolean(host), sessionId);
    if (preconditionError) return capture(preconditionError);
    // Keep the values narrowed after the pure validation helper; this branch
    // is defensive if its validation rules are ever changed independently.
    if (!host || !sessionId) return capture(new Error('The engine restart request is missing its host or session.'));
    setError(null);
    try {
      await restartBridgeWithTimeout(() => restartBridgeSingleFlight(
        restartOperationsRef.current,
        sessionId,
        () => host.restartBridge(sessionId),
      ));
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host]);

  const refreshDiagnostics = useCallback(async () => {
    if (!host) return [];
    try { const diagnostics = await host.diagnostics(); patchBootstrap({ diagnostics }); return diagnostics; }
    catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const copyDiagnostics = useCallback(async () => {
    if (!host) return;
    try { await host.copyDiagnostics(); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const copyText = useCallback(async (text: string) => {
    if (!host) return;
    try { await host.copyText(text); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const exportDiagnostics = useCallback(async () => {
    if (!host) return null;
    try { return await host.exportDiagnostics(); } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const audioRequest = useCallback(async (value: NativeAudioCommand): Promise<NativeAudioResponse> => {
    if (!host?.audio) {
      return {
        type: 'error',
        snapshot: defaultNativeAudioSnapshot(),
        error: { code: 'unavailable', message: 'native audio is unavailable on this host' },
      };
    }
    try {
      const response = await host.audio.request(value);
      setAudioSnapshot(response.snapshot);
      return response;
    } catch (cause) {
      capture(cause);
      return {
        type: 'error',
        snapshot: defaultNativeAudioSnapshot(),
        error: { code: 'native-error', message: messageFrom(cause) },
      };
    }
  }, [capture, host]);

  const command = useCallback(async (value: Parameters<NonNullable<typeof host>['command']>[1]) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.command(sessionId, value); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const refresh = useCallback(async () => {
    await Promise.all([
      command({ type: 'list_sessions', limit: 100 }),
      command({ type: 'list_models' }),
      command({ type: 'get_conversation_controls' }),
      requestTaskList(),
      command({ type: 'list_session_agents' }),
      command({
        type: 'refresh_listings',
        which: [{ type: 'auth' }, { type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }, { type: 'hooks' }, { type: 'agents' }],
      }),
      command({ type: 'refresh_listings', which: [{ type: 'mcp' }, { type: 'skills' }] }),
      command({ type: 'skill_admin', command: { action: 'get_catalog' } }),
      command({ type: 'mcp_admin', command: { action: 'get_snapshot' } }),
      command({ type: 'plugin_admin', command: { action: 'get_catalog' } }),
      command({ type: 'hook_admin', command: { action: 'get_document' } }),
      refreshDiagnostics(),
    ]);
  }, [command, refreshDiagnostics, requestTaskList]);

  const newSession = useCallback(async (requestedProjectPath?: string) => {
    if (sessionLoadingRef.current) return;
    const projectPath = requestedProjectPath ?? bootstrap?.settings.activeProject;
    if (!host || !projectPath) {
      await addProject();
      return;
    }
    const operationId = beginNavigationOperation();
    try {
      const snapshot = await host.newSession(projectPath, desktop.currentModel ?? undefined);
      if (isCurrentNavigationOperation(operationId)) applyBootstrap(snapshot);
    }
    catch (cause) { if (isCurrentNavigationOperation(operationId)) capture(cause); }
  }, [addProject, applyBootstrap, beginNavigationOperation, bootstrap?.settings.activeProject, capture, desktop.currentModel, host, isCurrentNavigationOperation]);

  const resumeSession = useCallback(async (sessionId: string) => {
    const projectPath = activeSession?.projectPath ?? bootstrap?.settings.activeProject;
    if (!projectPath || !host) return;
    await openSession(projectPath, sessionId);
  }, [activeSession?.projectPath, bootstrap?.settings.activeProject, host, openSession]);

  const setModel = useCallback((model: string) => command({ type: 'set_model', model }), [command]);
  const setReasoningSelection = useCallback((selection: ReasoningSelectionDto) => command({ type: 'set_reasoning_selection', selection }), [command]);
  const setFastMode = useCallback((enabled: boolean) => command({ type: 'set_fast_mode', enabled }), [command]);
  const setPermissionMode = useCallback((mode: PermissionModeId) => command({ type: 'set_permission_mode', mode }), [command]);
  const login = useCallback(() => command({ type: 'login' }), [command]);
  const logout = useCallback(() => command({ type: 'logout' }), [command]);
  const forceCompact = useCallback(async (instructions?: string) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    setError(null);
    updateRuntime(sessionId, (state) => ({
      ...state,
      error: undefined,
      conversation: beginCompaction(state.conversation),
    }));
    try {
      await host.command(sessionId, instructions?.trim()
        ? { type: 'run_slash_command', raw: `/compact ${instructions.trim()}` }
        : { type: 'force_compact' });
    } catch (cause) {
      updateRuntime(sessionId, (state) => ({
        ...state,
        conversation: reduceEvent(state.conversation, {
          type: 'error',
          kind: { type: 'transport' },
          message: `force_compact failed: ${messageFrom(cause)}`,
        }),
      }));
      capture(cause);
    }
  }, [capture, host, updateRuntime]);
  const clearSession = useCallback(async () => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try {
      await host.clearSession(sessionId);
      const snapshot = await host.bootstrap();
      applyBootstrap(snapshot);
      setError(null);
    } catch (cause) {
      capture(cause);
    }
  }, [applyBootstrap, capture, host]);
  const refreshTasks = useCallback(
    (options?: { preserve?: boolean }) => requestTaskList(activeSessionIdRef.current, options),
    [requestTaskList],
  );
  const refreshAuth = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'auth' }] }),
    [command],
  );
  const refreshHooks = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'hooks' }] }),
    [command],
  );
  const refreshAgents = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'agents' }] }),
    [command],
  );
  const refreshStatus = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'status' }] }),
    [command],
  );
  const refreshDoctor = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'doctor' }] }),
    [command],
  );
  const refreshSlashCommands = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'slash_commands' }] }),
    [command],
  );
  const refreshSessionAgents = useCallback(async () => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try {
      await host.command(sessionId, { type: 'list_session_agents' });
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host]);
  const loadSessionAgentTranscript = useCallback(async (agentId: string) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId || !agentId.trim()) return;
    try {
      await host.command(sessionId, { type: 'load_session_agent_transcript', agent_id: agentId });
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host]);
  const openRuntimeItem = useCallback((item: RuntimeCenterItemRef): void => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId || sessionLoadingRef.current) return;
    updateRuntime(sessionId, (state) => ({
      ...state,
      runtimeCenter: openRuntimeCenterItem(state.runtimeCenter, item),
    }));
    if (item.kind === 'agent') void loadSessionAgentTranscript(item.id).catch(() => undefined);
    if (item.kind === 'section' && item.id === 'agents') void refreshSessionAgents().catch(() => undefined);
  }, [loadSessionAgentTranscript, refreshSessionAgents, updateRuntime]);
  const closeRuntimeItem = useCallback((item: RuntimeCenterItemRef): void => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({ ...state, runtimeCenter: closeRuntimeCenterItem(state.runtimeCenter, item) }));
  }, [updateRuntime]);
  const setRuntimeInspectorOpenAction = useCallback((open: boolean): void => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({ ...state, runtimeCenter: setRuntimeInspectorOpen(state.runtimeCenter, open) }));
  }, [updateRuntime]);
  const setRuntimeCenterOverviewOpenAction = useCallback((open: boolean): void => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId || sessionLoadingRef.current) return;
    updateRuntime(sessionId, (state) => ({ ...state, runtimeCenter: setRuntimeCenterOverviewOpen(state.runtimeCenter, open) }));
    if (open) void refreshSessionAgents().catch(() => undefined);
  }, [refreshSessionAgents, updateRuntime]);
  const toggleRuntimeCenterSectionAction = useCallback((section: RuntimeCenterSection): void => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({ ...state, runtimeCenter: toggleRuntimeCenterSection(state.runtimeCenter, section) }));
  }, [updateRuntime]);
  const previewWorkspaceFile = useCallback(async (path: string): Promise<WorkspaceFilePreview> => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) throw new Error('Open a session before previewing a file.');
    return host.previewWorkspaceFile(sessionId, path);
  }, [host]);
  const taskOutput = useCallback((taskId: string) => command({ type: 'task_output', task_id: taskId, offset: 0 }), [command]);
  const stopTask = useCallback((taskId: string) => command({ type: 'task_stop', task_id: taskId }), [command]);
  const refreshSettingsSnapshot = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'settings' }] }),
    [command],
  );
  const updateProviderSettings = useCallback(
    async (destination: 'user' | 'project' | 'local', patch: Record<string, unknown>) => {
      const sessionId = activeSessionIdRef.current;
      if (sessionLoadingRef.current || !host || !sessionId) throw new Error('Open a connected session before saving provider settings.');
      if (providerSettingsSaves.current.size > 0) throw new Error('A provider settings save is already in progress.');
      const controller = new AbortController();
      providerSettingsSaves.current.add(controller);
      try {
        await saveProviderSettings(host, sessionId, destination, patch, controller.signal);
        if (activeSessionIdRef.current !== sessionId) throw new Error('Provider settings session changed.');
      } finally {
        providerSettingsSaves.current.delete(controller);
      }
    }, [host],
  );
  const updateEngineSettings = useCallback(
    async (destination: 'user' | 'project' | 'local', patch: Record<string, unknown>) => {
      await command({ type: 'update_settings', destination, patch_json: JSON.stringify(patch) });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const updatePermissionRules = useCallback(
    async (destination: SettingsDestinationDto, behavior: PermissionBehaviorDto, add: string[], remove: string[]) => {
      await command({ type: 'update_permission_rules', destination, behavior, add, remove });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const setDefaultPermissionMode = useCallback(
    async (destination: SettingsDestinationDto, mode: string) => {
      await command({ type: 'set_default_permission_mode', destination, mode });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const updateWorkspaceDirectories = useCallback(
    async (destination: SettingsDestinationDto, add: string[], remove: string[]) => {
      await command({ type: 'update_workspace_directories', destination, add, remove });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const refreshMcpServers = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'mcp' }] }),
    [command],
  );
  const refreshSkills = useCallback(
    () => command({ type: 'refresh_listings', which: [{ type: 'skills' }] }),
    [command],
  );
  const upsertMcpServer = useCallback(
    async (scope: McpScopeDto, name: string, config: Record<string, unknown>) => {
      await command({ type: 'upsert_mcp_server', scope, name, config_json: JSON.stringify(config) });
      await refreshMcpServers();
    },
    [command, refreshMcpServers],
  );
  const removeMcpServer = useCallback(
    async (scope: McpScopeDto, name: string) => {
      await command({ type: 'remove_mcp_server', scope, name });
      await refreshMcpServers();
    },
    [command, refreshMcpServers],
  );
  const manageCron = useCallback((request: CronRequestDto): Promise<CronJobDto[]> => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) {
      return Promise.reject(new Error('Open a connected session before managing scheduled tasks.'));
    }
    return requestCronManagement(host, sessionId, request);
  }, [host]);
  const runConfigurationAdmin = useCallback(async (
    domain: ConfigurationDomainDto,
    envelope:
      | { type: 'skill_admin'; command: SkillAdminCommandDto }
      | { type: 'mcp_admin'; command: McpAdminCommandDto }
      | { type: 'plugin_admin'; command: PluginAdminCommandDto }
      | { type: 'hook_admin'; command: HookAdminCommandDto },
  ): Promise<ConfigurationOperationEvent | void> => {
    const operationId = configurationOperationId(envelope.command);
    if (operationId === undefined) {
      await command(envelope);
      return;
    }
    if (sessionLoadingRef.current || !host || !activeSessionIdRef.current) {
      throw new Error('Open a connected session before changing configuration.');
    }
    const key = `${domain}:${operationId}`;
    if (pendingConfigurationOperations.current.has(key)) {
      throw new Error(`configuration operation ${operationId} is already pending`);
    }
    let pending!: PendingConfigurationOperation;
    const terminal = new Promise<ConfigurationOperationEvent>((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingConfigurationOperations.current.delete(key);
        reject(new Error(`Timed out waiting for the ${domain} configuration operation.`));
      }, CONFIGURATION_OPERATION_TIMEOUT_MS);
      pending = { domain, resolve, reject, timer };
      pendingConfigurationOperations.current.set(key, pending);
    });
    try {
      await command(envelope);
    } catch (cause) {
      clearTimeout(pending.timer);
      pendingConfigurationOperations.current.delete(key);
      throw cause;
    }
    return terminal;
  }, [command, host]);
  const skillAdmin = useCallback(
    (adminCommand: SkillAdminCommandDto) => runConfigurationAdmin('skill', { type: 'skill_admin', command: adminCommand }),
    [runConfigurationAdmin],
  );
  const mcpAdmin = useCallback(
    (adminCommand: McpAdminCommandDto) => runConfigurationAdmin('mcp', { type: 'mcp_admin', command: adminCommand }),
    [runConfigurationAdmin],
  );
  const pluginAdmin = useCallback(
    (adminCommand: PluginAdminCommandDto) => runConfigurationAdmin('plugin', { type: 'plugin_admin', command: adminCommand }),
    [runConfigurationAdmin],
  );
  const hookAdmin = useCallback(
    (adminCommand: HookAdminCommandDto) => runConfigurationAdmin('hook', { type: 'hook_admin', command: adminCommand }),
    [runConfigurationAdmin],
  );

  const presentedBootstrap = useMemo(() => bootstrap ? {
    ...bootstrap,
    projectCatalogs: submittedSessionCatalogs(
      bootstrap.projectCatalogs,
      [...runtimeStates.values()].flatMap((state) => state.submittedSession ? [state.submittedSession] : []),
      bootstrap.settings.archivedSessions ?? [],
    ),
  } : null, [bootstrap, runtimeStates]);

  return {
    manageCron,
    hosted,
    loading,
    bootstrap: presentedBootstrap,
    settingsSnapshotEvent,
    audioSnapshot,
    mcpServersEvent,
    skillsEvent,
    skillCatalogEvent,
    skillDocumentEvent,
    mcpConfigurationSnapshotEvent,
    pluginCatalogEvent,
    configurationOperations,
    activeSession,
    sessionLoading,
    connection,
    connected: !sessionLoading && connection.status === 'connected',
    conversation,
    desktop,
    authState: desktop.auth,
    hooksCatalog: desktop.hooks,
    agentCatalog: desktop.agents,
    cost: desktop.lastCost,
    lastCompaction: desktop.lastCompaction,
    retryState: desktop.lastApiRetry,
    runtimeCenter,
    usage: conversation.usage,
    running: conversation.running,
    isCancelling,
    pendingPermission: permissionQueue[0] ?? null,
    pendingComputerAccess: computerAccessQueue[0] ?? null,
    pendingAskUserQuestion: askUserQuestionQueue[0] ?? null,
    error,
    clearError,
    sendTrackedPrompt,
    subscribeTrackedSpeech,
    sendPrompt,
    runSlashCommand,
    beginLocalCommand,
    emitCommandOutput,
    cancel,
    approve,
    deny,
    approveComputerAccess,
    denyComputerAccess,
    answerAskUserQuestion,
    cancelAskUserQuestion,
    openSystemSettings,
    microphonePermission,
    audioRequest,
    addProject,
    activateProject,
    removeProject,
    archiveSession,
    preflightSessionArchive,
    setSessionPinned,
    openSession,
    listProjectSessions,
    sessionRuntimeStatus,
    searchWorkspaceFiles,
    setProviderCredential,
    clearProviderCredential,
    testProviderConnection,
    refreshProviderCredential,
    pluginSecret,
    setPluginSecret,
    clearPluginSecret,
    setThemePreference,
    setCollapseThoughtsByDefault,
    setApiBaseUrl,
    setVoicePreferences,
    setNotificationPreferences,
    setModelPickerVisibility,
    updateProviderSettings,
    updateEngineSettings,
    updatePermissionRules,
    setDefaultPermissionMode,
    updateWorkspaceDirectories,
    refreshMcpServers,
    refreshSkills,
    upsertMcpServer,
    removeMcpServer,
    skillAdmin,
    mcpAdmin,
    pluginAdmin,
    hookAdmin,
    restartBridge,
    refreshDiagnostics,
    copyDiagnostics,
    copyText,
    exportDiagnostics,
    refresh,
    newSession,
    resumeSession,
    setModel,
    setReasoningSelection,
    setFastMode,
    setPermissionMode,
    login,
    logout,
    forceCompact,
    clearSession,
    refreshTasks,
    refreshAuth,
    refreshHooks,
    refreshAgents,
    refreshStatus,
    refreshDoctor,
    refreshSlashCommands,
    refreshSessionAgents,
    loadSessionAgentTranscript,
    openRuntimeItem,
    closeRuntimeItem,
    setRuntimeCenterOverviewOpen: setRuntimeCenterOverviewOpenAction,
    setRuntimeInspectorOpen: setRuntimeInspectorOpenAction,
    toggleRuntimeCenterSection: toggleRuntimeCenterSectionAction,
    previewWorkspaceFile,
    taskOutput,
    stopTask,
    refreshSettingsSnapshot,
  };
}
