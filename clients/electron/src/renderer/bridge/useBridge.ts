import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AgentDto,
  AskUserQuestionRequestDto,
  AuthStateDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  HookDto,
  ImageRefDto,
  McpScopeDto,
  PermissionBehaviorDto,
  PermissionModeId,
  PermissionRequest,
  PermissionResponseDto,
  ReasoningSelectionDto,
  SettingsDestinationDto,
} from '@lingxi/bridge-client';

import { browserMicrophoneCaptureDeps, MicrophoneCapture } from '../audio/capture';
import { handleAudioRequestEvent, type AudioRequestDeps } from '../audio/requests';
import { browserSynthesisDeps, synthesize } from '../audio/synthesis';
import { hostMicrophonePermissionReader, type VoicePermissionStatus } from '../audio/capabilities';
import { defaultVoicePreferences, type VoicePreferences } from '../../shared/voicePreferences';
import {
  appendPendingUserPrompt,
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
  ProviderCredentialMetadata,
  ProviderCredentialUpdate,
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

export interface UseBridge {
  readonly hosted: boolean;
  readonly loading: boolean;
  readonly bootstrap: BootstrapState | null;
  readonly settingsSnapshotEvent: SettingsSnapshotEvent | null;
  readonly mcpServersEvent: McpServersEvent | null;
  readonly skillsEvent: SkillsEvent | null;
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
  clearError(): void;
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
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<void>;
  openSession(projectPath: string, sessionId: string): Promise<void>;
  listProjectSessions(projectPath: string): Promise<void>;
  sessionRuntimeStatus(sessionId: string): SessionRuntimeStatus | undefined;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  setThemePreference(theme: 'dark' | 'light' | 'system'): Promise<void>;
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
  restartBridge(sessionId?: string): Promise<void>;
  refreshDiagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  copyText(text: string): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  refresh(): Promise<void>;
  newSession(projectPath?: string): Promise<void>;
  resumeSession(sessionId: string): Promise<void>;
  setModel(model: string): Promise<void>;
  setReasoningSelection(selection: ReasoningSelectionDto): Promise<void>;
  setFastMode(enabled: boolean): Promise<void>;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  login(): Promise<void>;
  logout(): Promise<void>;
  forceCompact(): Promise<void>;
  clearSession(): Promise<void>;
  refreshTasks(): Promise<void>;
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
  readonly connection: ConnectionState;
  readonly turnActive: boolean;
  readonly pendingInteractions: number;
  readonly pendingAskUserQuestions: number;
  readonly error?: string;
}

function getHost() {
  return typeof window !== 'undefined' ? window.lingxi : undefined;
}

function messageFrom(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return 'The desktop host could not complete that action.';
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
  connection: ConnectionState;
  conversation: ConversationState;
  desktop: DesktopState;
  runtimeCenter: RuntimeCenterState;
  permissionQueue: PermissionRequest[];
  computerAccessQueue: ComputerAccessRequestDto[];
  askUserQuestionQueue: AskUserQuestionRequestDto[];
  pendingInteractionsOverride?: number;
  pendingAskUserQuestionsOverride?: number;
  resolvedAskUserQuestionIds: Set<number>;
  isCancelling: boolean;
  error?: string;
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
  // Settings are file-layer state, not per-conversation state, so this is
  // one value for the whole app rather than something keyed into `runtimeStates`.
  const [settingsSnapshotEvent, setSettingsSnapshotEvent] = useState<SettingsSnapshotEvent | null>(null);
  // Same "one value for the whole app" treatment as `settingsSnapshotEvent`
  // above: MCP server definitions and discovered skills are not per-session
  // state either.
  const [mcpServersEvent, setMcpServersEvent] = useState<McpServersEvent | null>(null);
  const [skillsEvent, setSkillsEvent] = useState<SkillsEvent | null>(null);
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

  const persistedActiveSession = bootstrap?.activeSession ?? bootstrap?.settings.activeSession;
  const activeSession = displayedSession(persistedActiveSession, pendingSession);
  const sessionLoading = pendingSession !== null;
  const activeSessionId = activeSession?.sessionId ?? null;
  const activeSessionIdRef = useRef<string | null>(null);
  activeSessionIdRef.current = activeSessionId;
  sessionLoadingRef.current = sessionLoading;
  bootstrapRef.current = bootstrap;

  // Settings are captured per-CONNECTION on the engine side (`SettingsContext`
  // is built once per `assemble_with_provider_keys` call), so a snapshot from
  // a previous session/project is not a valid answer for a new one — without
  // this, switching sessions would leave the old session's `project`/`local`
  // values on screen until a fresh snapshot happened to arrive.
  useEffect(() => {
    setSettingsSnapshotEvent(null);
  }, [activeSessionId]);

  const capture = useCallback((cause: unknown) => {
    const message = messageFrom(cause);
    setError(message);
    throw cause;
  }, []);

  /**
   * The microphone/speaker bindings the engine's `audio_request` events are
   * serviced with, keyed by session — see {@link sessionAudioBindings} for why
   * one instance per SESSION rather than one per hook or one per request.
   * Built lazily because it touches `navigator.mediaDevices` and
   * `window.speechSynthesis`, which a server-rendered probe of this hook has
   * neither of.
   */
  const audioBindings = useRef(new Map<string, AudioRequestDeps>());
  const audioRequestDeps = useCallback((sessionId: string): AudioRequestDeps => (
    sessionAudioBindings(audioBindings.current, sessionId, () => {
      const synthesisDeps = browserSynthesisDeps();
      return {
        recorder: new MicrophoneCapture(browserMicrophoneCaptureDeps()),
        synthesize: (text, voiceId, rate) => synthesize(text, voiceId, rate, synthesisDeps),
        // `AudioOpDto::Synthesize` carries the text and sometimes a voice,
        // never a rate — that is a device preference. Read through the ref
        // on every request so a settings change takes effect without
        // rebuilding these bindings.
        playback: () => {
          const preferences = bootstrapRef.current?.settings.voice ?? defaultVoicePreferences();
          return { voiceSelection: preferences.voiceSelection, rate: preferences.rate };
        },
      };
    })
  ), []);

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
      // Not `pruneRuntimeMaps`: a discarded session may still hold the
      // microphone, and nothing else can release it.
      pruneAudioBindings(audioBindings.current, runtimeIds);
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
  }, []);

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

  const listProjectSessions = useCallback(async (projectPath: string): Promise<void> => {
    if (!host) return;
    const generation = beginProjectCatalogRequest(catalogRequestGenerations.current, projectPath);
    const result = await host.listProjectSessions(projectPath);
    if (!isLatestProjectCatalogRequest(catalogRequestGenerations.current, projectPath, generation)) return;
    setBootstrap((previous) => previous ? {
      ...previous,
      projectCatalogs: {
        ...previous.projectCatalogs,
        [result.projectPath]: {
          sessions: result.sessions.map((session) => ({ ...session })),
          ...(result.error ? { error: result.error } : {}),
        },
      },
    } : previous);
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

  const requestTaskList = useCallback(async (sessionId = activeSessionIdRef.current) => {
    if (sessionLoadingRef.current || !host || !sessionId) return;
    updateRuntime(sessionId, (state) => {
      const desktopState = beginTaskRefresh(state.desktop);
      return desktopState === state.desktop ? state : { ...state, desktop: desktopState };
    });
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
      if (event.type === 'turn_started') clearSlashTurnClaim(slashPendingRefs.current, sessionId);
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
        if (event.type === 'turn_ended' || event.type === 'session_ended') {
          next = {
            ...next,
            permissionQueue: [],
            computerAccessQueue: [],
            askUserQuestionQueue: [],
            pendingInteractionsOverride: 0,
            pendingAskUserQuestionsOverride: 0,
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
      if (event.type === 'error') {
        updateRuntime(sessionId, (state) => ({ ...state, error: event.message }));
        if (activeSessionIdRef.current === sessionId) setError(event.message);
      }
      if (event.type === 'audio_request') {
        // The engine has no microphone or speaker of its own on desktop: it
        // asks the connected client and PARKS the call on a deadline (5s /
        // 30s / 180s per op, `audio_bridge.rs`). Every request must produce
        // exactly one `audio_response`, which is what
        // `handleAudioRequestEvent` guarantees — including on its error
        // paths, so `void` here can never leave a request unanswered nor
        // raise an unhandled rejection.
        void handleAudioRequestEvent(
          sessionId,
          event,
          () => audioRequestDeps(sessionId),
          (target, command) => host.command(target, command),
          // Deliberately NOT `capture`: that helper rethrows, which would
          // strand the parked engine call. A failure to answer at all is
          // reported the same way an engine `error` event is, above.
          (cause) => {
            const message = messageFrom(cause);
            updateRuntime(sessionId, (state) => ({ ...state, error: message }));
            if (activeSessionIdRef.current === sessionId) setError(message);
          },
        );
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
        // Not `removeRuntimeFromMaps`: a removed session may still hold the
        // microphone, and nothing else can release it.
        discardAudioBindings(audioBindings.current, sessionId);
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
            runtimeCenter: { ...current.runtimeCenter, overviewOpen: false },
          };
        }
        if (shouldClearPendingPermissions(state)) {
          next.permissionQueue = [];
          next.computerAccessQueue = [];
          next.askUserQuestionQueue = [];
          next.pendingInteractionsOverride = 0;
          next.pendingAskUserQuestionsOverride = 0;
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
        const alreadyQueued = state.permissionQueue.some((entry) => entry.request_id === envelope.event.request_id);
        return {
          ...state,
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
  }, [applyBootstrap, audioRequestDeps, capture, host, scheduleProjectCatalogRefresh, updateRuntime]);

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
    ]).catch((cause) => setError(messageFrom(cause)));
  }, [activeSessionId, bootstrap?.workspace.trusted, connection.status, host, requestTaskList, sessionLoading]);

  const sendPrompt = useCallback(async (
    text: string,
    images: ImageRefDto[] = [],
    imageNames: string[] = [],
    filePaths: string[] = [],
  ) => {
    const trimmed = text.trim();
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !trimmed || !host || !sessionId) return;
    const wasTurnActive = turnActiveRefs.current.get(sessionId) === true;
    turnActiveRefs.current.set(sessionId, true);
    runtimeResourceSendSequence.current += 1;
    const sendToken = `${sessionId}:${runtimeResourceSendSequence.current}`;
    const resources = promptRuntimeResources(sessionId, sendToken, images, imageNames, filePaths);
    updateRuntime(sessionId, (state) => {
      return {
        ...state,
        conversation: appendPendingUserPrompt(state.conversation, trimmed, images),
        runtimeCenter: addRuntimeResources(state.runtimeCenter, resources),
      };
    });
    try {
      await host.sendPrompt(sessionId, trimmed, images);
      if (!removedRuntimeIds.current.has(sessionId)) {
        updateRuntime(sessionId, (state) => ({
          ...state,
          runtimeCenter: commitRuntimeResources(state.runtimeCenter, sendToken),
        }));
      }
    } catch (cause) {
      turnActiveRefs.current.set(sessionId, wasTurnActive);
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
    }
  }, [capture, host, updateRuntime]);

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
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const deny = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try { await host.deny(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'permission'); }
    catch (cause) { capture(cause); }
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
      patchBootstrap({ settings: update.settings, providerCredentials: (bootstrap?.providerCredentials ?? []).map((entry) => entry.providerId === providerId ? update.credential : entry) });
      return update;
    } catch (cause) { return capture(cause); }
  }, [bootstrap?.providerCredentials, capture, host, patchBootstrap]);

  const clearProviderCredential = useCallback(async (providerId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const metadata = await host.clearProviderCredential(providerId);
      patchBootstrap({ providerCredentials: (bootstrap?.providerCredentials ?? []).map((entry) => entry.providerId === providerId ? metadata : entry) });
      return metadata;
    } catch (cause) { return capture(cause); }
  }, [bootstrap?.providerCredentials, capture, host, patchBootstrap]);

  const setThemePreference = useCallback(async (theme: 'dark' | 'light' | 'system') => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ theme }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setApiBaseUrl = useCallback(async (apiBaseUrl: string | null) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ apiBaseUrl }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setVoicePreferences = useCallback(async (voice: VoicePreferences) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ voice }) }); } catch (cause) { capture(cause); }
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
  const forceCompact = useCallback(() => command({ type: 'force_compact' }), [command]);
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
  const refreshTasks = useCallback(() => requestTaskList(), [requestTaskList]);
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
      runtimeCenter: setRuntimeCenterOverviewOpen(openRuntimeCenterItem(state.runtimeCenter, item), false),
    }));
    if (item.kind === 'agent') void loadSessionAgentTranscript(item.id).catch(() => undefined);
  }, [loadSessionAgentTranscript, updateRuntime]);
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

  return {
    hosted,
    loading,
    bootstrap,
    settingsSnapshotEvent,
    mcpServersEvent,
    skillsEvent,
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
    clearError: () => setError(null),
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
    addProject,
    activateProject,
    removeProject,
    setSessionPinned,
    openSession,
    listProjectSessions,
    sessionRuntimeStatus,
    searchWorkspaceFiles,
    setProviderCredential,
    clearProviderCredential,
    setThemePreference,
    setApiBaseUrl,
    setVoicePreferences,
    updateEngineSettings,
    updatePermissionRules,
    setDefaultPermissionMode,
    updateWorkspaceDirectories,
    refreshMcpServers,
    refreshSkills,
    upsertMcpServer,
    removeMcpServer,
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
