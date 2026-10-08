import type {
  AudioOperationDto,
  ClientEvent,
  ComputerAccessResponseDto,
  ConfigurationDomainDto,
  CronJobDto,
  CronRequestDto,
  CronRunDto,
  HookAdminCommandDto,
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
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { AudioConfigurationV3 } from '../../shared/generatedAudioConfiguration.js';
import { hostMicrophonePermissionReader } from '../../shared/microphoneAccess.js';
import type {
  MicrophonePermissionStatus as VoicePermissionStatus,
} from '../../shared/microphoneAccess.js';
import { defaultNativeAudioSnapshot } from '../../shared/nativeAudio.js';
import type {
  NativeAudioCommand,
  NativeAudioOperationResponse,
  NativeAudioResponse,
  NativeAudioSnapshot,
} from '../../shared/nativeAudio.js';
import type { NotificationPreferences } from '../../shared/notificationPreferences.js';
import type { ScheduledContext, ScheduledScope } from '../../shared/scheduled.js';
import type { ModelPickerVisibilitySettings } from '../../shared/settings.js';
import type { VoicePreferences } from '../../shared/voicePreferences.js';
import {
  isPermissionRequestGone,
  messageFrom,
  restartBridgePreconditionError,
  restartBridgeSingleFlight,
  restartBridgeWithTimeout,
  stopSessionSubagents,
} from './bridgeConnection.js';
import {
  beginProjectCatalogRequest,
  claimSlashTurn,
  clearCancellationRuntime,
  clearSlashTurnClaim,
  displayedSession,
  emptyRuntimeState,
  isLatestOperation,
  isLatestProjectCatalogRequest,
  isRuntimeRemovedState,
  nextOperationId,
  pendingAskQueueFromBootstrap,
  pendingCountAfterResponse,
  pruneRuntimeMaps,
  reconcilePendingCount,
  recoverLatestNavigationFailure,
  removeRuntimeFromMaps,
  seedDesktopFromCache,
  shouldApplyBootstrapSnapshot,
  shouldClearPendingPermissions,
  shouldReleaseSlashTurn,
  shouldResetBridgeRuntime,
  updateSharedDesktopCache,
} from './bridgeRuntimeState.js';
import type { RuntimeState, SharedDesktopCache } from './bridgeRuntimeState.js';
import type {
  ConfigurationOperationEvent,
  DesktopTurnToken,
  McpConfigurationSnapshotEvent,
  McpServersEvent,
  PluginCatalogEvent,
  SessionRuntimeStatus,
  SettingsSnapshotEvent,
  SkillCatalogEvent,
  SkillDocumentEvent,
  SkillsEvent,
  TrackedPromptPurpose,
  TrackedSpeechTerminal,
  UseBridge,
} from './bridgeTypes.js';
import {
  acknowledgePromptDispatch,
  appendPendingUserPrompt,
  beginCompaction,
  beginLocalSlashCommand,
  beginSlashCommand,
  emptyConversation,
  failSlashCommand,
  isPendingFusionSessionRestore,
  reduceEvent,
  reduceEventWithPendingFusion,
  slashCommandEchoesRequest,
} from './conversation.js';
import type { ConversationState } from './conversation.js';
import { requestCronManagement } from './cronManagement.js';
import { beginTaskRefresh, emptyDesktopState, reduceDesktopEvent } from './desktopState.js';
import type {
  LingxiApi,
  BootstrapState,
  ConnectionState,
  ProjectSessionCatalogState,
  SequencedRuntimeEventEnvelope,
  SessionPinInput,
  SessionRef,
  SessionRuntimeSummary,
  SystemSettingsPane,
  WorkspaceFilePreview,
} from './lingxi.js';
import { saveProviderSettings } from './providerSettingsSave.js';
import { createUiSurfaceClientId, UiSurfaceLifecycleQueue } from './uiSurfaceLifecycle.js';
import {
  interruptConfigurationOperations,
  requestConfigurationOperation,
  settleConfigurationOperation,
} from './configurationOperations.js';
import type { PendingConfigurationOperation } from './configurationOperations.js';
import {
  addRuntimeResources,
  closeRuntimeCenterItem,
  commitRuntimeResources,
  emptyRuntimeCenterState,
  interruptedSideQuestionAgents,
  openRuntimeCenterItem,
  promptRuntimeResources,
  reduceRuntimeCenterEvent,
  reduceRuntimeCenterPermission,
  resetRuntimeCenterConnection,
  resourcesFromRestoredMessages,
  rollbackRuntimeResources,
  runningSubagentIds,
  setRuntimeCenterOverviewOpen,
  setRuntimeInspectorOpen,
  toggleRuntimeCenterSection,
} from './runtimeCenterState.js';
import type { RuntimeCenterItemRef, RuntimeCenterSection } from './runtimeCenterState.js';
import {
  SIDE_QUESTION_AGENT_PREFIX,
  beginSideQuestion,
  finishSideQuestion,
  isSideQuestionCommand,
} from './sideQuestion.js';
import { firstSubmittedSession, submittedSessionCatalogs } from './submittedSessionCatalog.js';
import type { SubmittedSession } from './submittedSessionCatalog.js';
import {
  MAX_TRACKED_SPEECH_SUBSCRIBERS,
  appendTrackedTurnDelta,
  acceptTrackedMessages,
  allocateTrackedTurnId,
  identifyTrackedMessage,
  sealTrackedMessage,
  retractTrackedMessage,
  bindTrackedTurn,
  clearTrackedTurnState,
  completeTrackedTurn,
  createDesktopTurnToken,
  dequeueTrackedTurn,
  emitTrackedSpeech,
  enqueueTrackedTurn,
  trackedListenerKey,
} from './trackedTurns.js';
import type { ActiveTrackedTurn, TrackedSpeechListener } from './trackedTurns.js';

const CONFIGURATION_OPERATION_TIMEOUT_MS = 5 * 60_000;

function configurationOperationId(
  command: SkillAdminCommandDto | McpAdminCommandDto | PluginAdminCommandDto | HookAdminCommandDto,
): number | undefined {
  return 'operation_id' in command && typeof command.operation_id === 'number' ? command.operation_id : undefined;
}

function getHost(): LingxiApi | undefined {
  return typeof window !== 'undefined' ? window.lingxi : undefined;
}

export function useBridge(): UseBridge {
  const hostRef = useRef(getHost());
  const host = hostRef.current;
  const uiSurfaceClientId = useRef<string | null>(null);
  if (uiSurfaceClientId.current === null) uiSurfaceClientId.current = createUiSurfaceClientId();
  const stableUiSurfaceClientId = uiSurfaceClientId.current;
  const uiSurfaceLifecycle = useRef<UiSurfaceLifecycleQueue | null>(null);
  if (uiSurfaceLifecycle.current === null) uiSurfaceLifecycle.current = new UiSurfaceLifecycleQueue();
  const hosted = host !== undefined;
  const [loading, setLoading] = useState(hosted);
  const [bootstrap, setBootstrap] = useState<BootstrapState | null>(null);
  const [pendingSession, setPendingSession] = useState<SessionRef | null>(null);
  // A new runtime can be used as soon as its bridge handshake completes. The
  // host may still be finishing bootstrap metadata in parallel, so keep that
  // separate from the navigation target itself.
  const [sessionReady, setSessionReady] = useState(false);
  const [runtimeStates, setRuntimeStates] = useState<Map<string, RuntimeState>>(new Map());
  const sharedDesktopCacheRef = useRef(new Map<string, SharedDesktopCache>());
  // `updateRuntime` replaces this Map on every engine event, so any callback
  // that lists it as a dependency changes identity per streamed token. Consumers
  // that only need to READ the latest snapshot at call time go through the ref
  // and stay referentially stable — `bridge.cancel` is keyed on by an effect
  // that disposes and rebuilds the voice-flow controller.
  const runtimeStatesRef = useRef(runtimeStates);
  runtimeStatesRef.current = runtimeStates;
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
  const engineTurnActiveRefs = useRef(new Map<string, boolean>());
  const slashPendingRefs = useRef(new Map<string, boolean>());
  const pendingFusionDispatches = useRef(new Map<string, { raw: string }>());
  const sideQuestionTurns = useRef(new Map<string, Set<number>>());
  const nextSideQuestionTurn = useRef(Date.now());
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
  const cancelledTrackedTokens = useRef(new Set<string>());
  const pendingTrackedDispatches = useRef(new Set<string>());

  useEffect(() => () => {
    interruptConfigurationOperations(pendingConfigurationOperations.current);
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
  const sessionLoading = pendingSession !== null && !sessionReady;
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

  useEffect(() => {
    if (sessionLoading || !host || !activeSessionId || connection.status !== 'connected') return;
    const clientId = stableUiSurfaceClientId;
    const lifecycle = uiSurfaceLifecycle.current;
    if (!clientId || !lifecycle) return;

    void lifecycle.attach(host, activeSessionId, clientId).catch(() => undefined);
    return () => {
      void lifecycle.detach(host, activeSessionId, clientId).catch(() => undefined);
    };
  }, [activeSessionId, connection.status, host, sessionLoading, stableUiSurfaceClientId]);

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
    request: { request_id: number },
  ): void => {
    const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
    updateRuntime(sessionId, (state) => {
      const queue = kind === 'permission' ? state.permissionQueue
        : kind === 'computerAccess' ? state.computerAccessQueue : state.askUserQuestionQueue;
      const pending = queue.find((entry) => entry.request_id === requestId);
      if (pending !== request) return state;
      const next: RuntimeState = { ...state };
      if (kind === 'permission') {
        if (state.resolvedPermissionIds.has(requestId)) return state;
        next.permissionQueue = state.permissionQueue.filter((entry) => entry.request_id !== requestId);
        next.resolvedPermissionIds = new Set(state.resolvedPermissionIds).add(requestId);
      } else if (kind === 'computerAccess') {
        next.computerAccessQueue = state.computerAccessQueue.filter((entry) => entry.request_id !== requestId);
      } else {
        if (state.resolvedAskUserQuestionIds.has(requestId)) return state;
        next.askUserQuestionQueue = state.askUserQuestionQueue.filter((entry) => entry.request_id !== requestId);
        const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
        resolvedAskUserQuestionIds.add(requestId);
        next.resolvedAskUserQuestionIds = resolvedAskUserQuestionIds;
      }
      const localPendingInteractions = state.permissionQueue.length
        + state.computerAccessQueue.length
        + state.askUserQuestionQueue.length;
      const fallbackPendingInteractions = Math.max(summary?.pendingInteractions ?? 0, localPendingInteractions);
      next.pendingInteractionsOverride = Math.max(
        next.permissionQueue.length + next.computerAccessQueue.length + next.askUserQuestionQueue.length,
        pendingCountAfterResponse(state.pendingInteractionsOverride, fallbackPendingInteractions),
      );
      if (kind === 'askUserQuestion') {
        const fallbackPendingQuestions = Math.max(summary?.pendingAskUserQuestions ?? 0, state.askUserQuestionQueue.length);
        next.pendingAskUserQuestionsOverride = Math.max(
          next.askUserQuestionQueue.length,
          pendingCountAfterResponse(state.pendingAskUserQuestionsOverride, fallbackPendingQuestions),
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
      pruneRuntimeMaps(runtimeIds, next, turnActiveRefs.current, engineTurnActiveRefs.current, slashPendingRefs.current, pendingFusionDispatches.current, sideQuestionTurns.current, cancellingRefs.current, cancellationTasks.current);
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
        engineTurnActiveRefs.current.set(summary.sessionId, summary.turnActive);
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
    const activeProjects = new Set(snapshot.settings.projects);
    for (const projectPath of sharedDesktopCacheRef.current.keys()) {
      if (!activeProjects.has(projectPath)) sharedDesktopCacheRef.current.delete(projectPath);
    }
  }, [completeTrackedSpeech]);

  const beginNavigationOperation = useCallback((): number => {
    pendingSessionRef.current = null;
    setSessionReady(false);
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
      const eventProjectPath = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId)?.projectPath
        ?? (pendingSessionRef.current?.sessionId.startsWith('pending-new:')
          ? pendingSessionRef.current.projectPath
          : pendingSessionRef.current?.sessionId === sessionId ? pendingSessionRef.current.projectPath : undefined);
      if (eventProjectPath) {
        const cached = updateSharedDesktopCache(sharedDesktopCacheRef.current.get(eventProjectPath), event);
        if (cached) sharedDesktopCacheRef.current.set(eventProjectPath, cached);
      }
      // Side answers use the existing slash protocol, but never claim or release
      // the main turn, enter its transcript, or reach its speech/plan reducers.
      if (event.type === 'slash_command_result' && event.turn_id !== undefined
        && sideQuestionTurns.current.get(sessionId)?.has(event.turn_id)) {
        const turnId = event.turn_id;
        updateRuntime(sessionId, (state) => ({
          ...state,
          runtimeCenter: finishSideQuestion(state.runtimeCenter, sessionId, turnId, event.display, event.is_error),
        }));
        return;
      }
      if (event.type === 'turn_started') {
        turnActiveRefs.current.set(sessionId, true);
        engineTurnActiveRefs.current.set(sessionId, true);
      }
      if (event.type === 'turn_ended' || event.type === 'session_ended') {
        turnActiveRefs.current.set(sessionId, false);
        engineTurnActiveRefs.current.set(sessionId, false);
      }
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
      const pendingFusion = pendingFusionDispatches.current.get(sessionId);
      const restoringFusion = isPendingFusionSessionRestore(event, sessionId, pendingFusion?.raw);
      if (event.type === 'session_started' || event.type === 'session_ended' || event.type === 'session_resumed') {
        if (restoringFusion) {
          claimSlashTurn(slashPendingRefs.current, sessionId);
          turnActiveRefs.current.set(sessionId, true);
        } else {
          pendingFusionDispatches.current.delete(sessionId);
          clearSlashTurnClaim(slashPendingRefs.current, sessionId);
        }
      }
      if (event.type === 'slash_command_result' || event.type === 'error' || event.type === 'turn_started') {
        pendingFusionDispatches.current.delete(sessionId);
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
      if (event.type === 'text_delta' || event.type === 'tool_use_started' || event.type === 'tool_use_result'
        || event.type === 'tool_heartbeat' || event.type === 'turn_ended') {
        for (const accepted of acceptTrackedMessages(activeTrackedTurns.current, sessionId)) {
          emitTrackedSpeech(trackedSpeechListeners.current, accepted);
        }
      }
      if (event.type === 'message_identity') identifyTrackedMessage(activeTrackedTurns.current, sessionId, event.message_id);
      if (event.type === 'message_retracted') retractTrackedMessage(activeTrackedTurns.current, sessionId, event.message_id);
      if (event.type === 'message_complete') sealTrackedMessage(activeTrackedTurns.current, sessionId);
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
      if (event.type === 'turn_ended' || event.type === 'session_ended' || event.type === 'session_started' || event.type === 'session_resumed') {
        completeTrackedSpeech(sessionId, event.type === 'turn_ended' ? 'turn_ended' : 'stale');
      }
      updateRuntime(sessionId, (state) => {
        let next = { ...state, conversation: reduceEventWithPendingFusion(state.conversation, event, sessionId, restoringFusion ? pendingFusion?.raw : undefined), desktop: reduceDesktopEvent(state.desktop, event), runtimeCenter: reduceRuntimeCenterEvent(state.runtimeCenter, event, sessionId) };
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
          if (state.resolvedAskUserQuestionIds.has(event.request.request_id)) return next;
          const alreadyQueued = state.askUserQuestionQueue.some((entry) => entry.request_id === event.request.request_id);
          next = {
            ...next,
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
          if (state.resolvedAskUserQuestionIds.has(event.request_id)) return next;
          const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
          resolvedAskUserQuestionIds.add(event.request_id);
          const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
          const questionWasQueued = state.askUserQuestionQueue.some((entry) => entry.request_id === event.request_id);
          const remainingQuestions = state.askUserQuestionQueue.filter((entry) => entry.request_id !== event.request_id).length;
          const pendingInteractions = questionWasQueued
            ? pendingCountAfterResponse(
                state.pendingInteractionsOverride,
                Math.max(summary?.pendingInteractions ?? 0,
                  state.permissionQueue.length + state.computerAccessQueue.length + state.askUserQuestionQueue.length),
              )
            : state.pendingInteractionsOverride;
          const pendingQuestions = questionWasQueued
            ? pendingCountAfterResponse(state.pendingAskUserQuestionsOverride,
                Math.max(summary?.pendingAskUserQuestions ?? 0, state.askUserQuestionQueue.length))
            : state.pendingAskUserQuestionsOverride;
          next = {
            ...next,
            askUserQuestionQueue: state.askUserQuestionQueue.filter((entry) => entry.request_id !== event.request_id),
            resolvedAskUserQuestionIds,
            pendingInteractionsOverride: pendingInteractions === undefined ? undefined
              : Math.max(pendingInteractions, state.permissionQueue.length + state.computerAccessQueue.length + remainingQuestions),
            pendingAskUserQuestionsOverride: pendingQuestions === undefined ? undefined : Math.max(pendingQuestions, remainingQuestions),
          };
        }
        if (event.type === 'permission_request_resolved') {
          if (state.resolvedPermissionIds.has(event.request_id)) return next;
          const resolvedPermissionIds = new Set(state.resolvedPermissionIds);
          resolvedPermissionIds.add(event.request_id);
          const summary = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId);
          const requestWasQueued = state.permissionQueue.some((entry) => entry.request_id === event.request_id);
          const remainingInteractions = state.permissionQueue.filter((entry) => entry.request_id !== event.request_id).length
            + state.computerAccessQueue.length + state.askUserQuestionQueue.length;
          // The host can resolve a request whose scope check never admitted a
          // card here. That resolution must not consume a different worker's
          // pending count, nor hide any interactions we can still verify.
          const pendingInteractions = requestWasQueued
            ? pendingCountAfterResponse(
                state.pendingInteractionsOverride,
                Math.max(
                  summary?.pendingInteractions ?? 0,
                  state.permissionQueue.length + state.computerAccessQueue.length + state.askUserQuestionQueue.length,
                ),
              )
            : state.pendingInteractionsOverride;
          next = {
            ...next,
            permissionQueue: state.permissionQueue.filter((entry) => entry.request_id !== event.request_id),
            resolvedPermissionIds,
            pendingInteractionsOverride: pendingInteractions === undefined
              ? undefined : Math.max(pendingInteractions, remainingInteractions),
          };
        }
        if (event.type === 'turn_ended' || event.type === 'session_ended') {
          // A detached SDK worker owns a separate permission entry. Its card
          // remains actionable after the foreground turn has already ended.
          const permissionQueue = event.type === 'turn_ended'
            ? state.permissionQueue.filter((entry) => entry.backgroundOwned === true)
            : [];
          const resolvedPermissionIds = event.type === 'session_ended' ? new Set<number>() : new Set(state.resolvedPermissionIds);
          const resolvedAskUserQuestionIds = event.type === 'session_ended' ? new Set<number>() : new Set(state.resolvedAskUserQuestionIds);
          if (event.type === 'turn_ended') {
            for (const entry of state.permissionQueue) {
              if (entry.backgroundOwned !== true) resolvedPermissionIds.add(entry.request_id);
            }
            for (const entry of state.askUserQuestionQueue) resolvedAskUserQuestionIds.add(entry.request_id);
          }
          next = {
            ...next,
            permissionQueue,
            computerAccessQueue: [],
            askUserQuestionQueue: [],
            pendingInteractionsOverride: permissionQueue.length,
            pendingAskUserQuestionsOverride: 0,
            resolvedPermissionIds,
            resolvedAskUserQuestionIds,
            isCancelling: false,
          };
        }
        return next;
      });
      // `cost_update`/`turn_ended` normally carry the new cumulative totals,
      // but the durable status ledger is the recovery source for engines that
      // emit a delayed or incomplete cost snapshot. Pull it after the turn has
      // fully settled so the toolbar cannot remain pinned to the previous
      // session total.
      if (event.type === 'turn_ended' && host) {
        void host.command(sessionId, { type: 'refresh_listings', which: [{ type: 'status' }] }).catch(() => undefined);
      }
      // A background subagent can settle after the parent turn has already
      // ended. Its usage is written to the session cost ledger at the same
      // lifecycle edge, so refresh the toolbar totals when that edge arrives.
      if (event.type === 'session_agent_updated' && event.agent.status !== 'running' && host) {
        void host.command(sessionId, { type: 'refresh_listings', which: [{ type: 'status' }] }).catch(() => undefined);
      }
      if (event.type === 'session_started' || event.type === 'turn_ended') scheduleProjectCatalogRefresh(sessionId);
      if (event.type === 'settings_snapshot' && activeSessionIdRef.current === sessionId) setSettingsSnapshotEvent(event);
      if (event.type === 'mcp_servers' && activeSessionIdRef.current === sessionId) setMcpServersEvent(event);
      if (event.type === 'skills' && activeSessionIdRef.current === sessionId) setSkillsEvent(event);
      if (event.type === 'configuration_operation') {
        settleConfigurationOperation(pendingConfigurationOperations.current, sessionId, event);
      }
      if (activeSessionIdRef.current === sessionId) {
        if (event.type === 'skill_catalog') setSkillCatalogEvent(event);
        if (event.type === 'skill_document') setSkillDocumentEvent(event);
        if (event.type === 'mcp_configuration_snapshot') setMcpConfigurationSnapshotEvent(event);
        if (event.type === 'plugin_catalog') setPluginCatalogEvent(event);
        if (event.type === 'configuration_operation') {
          setConfigurationOperations((previous) => ({ ...previous, [event.domain]: event }));
        }
      }
      if (event.type === 'error' && !/^force_compact failed:\s*/i.test(event.message)) {
        updateRuntime(sessionId, (state) => ({ ...state, error: event.message }));
        if (activeSessionIdRef.current === sessionId) setError(event.message);
      }
    });
    const offState = host.onConnectionStateChanged((envelope) => {
      const sessionId = envelope.sessionId;
      const state = envelope.event;
      if (state.status !== 'connected') {
        interruptConfigurationOperations(pendingConfigurationOperations.current, sessionId);
      }
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
        removeRuntimeFromMaps(sessionId, turnActiveRefs.current, engineTurnActiveRefs.current, slashPendingRefs.current, pendingFusionDispatches.current, sideQuestionTurns.current, cancellingRefs.current, cancellationTasks.current);
        completeTrackedSpeech(sessionId, 'stale');
        return;
      }
      if (removedRuntimeIds.current.has(sessionId)) return;
      if (state.status === 'error' || state.status === 'disconnected' || state.status === 'idle') pendingFusionDispatches.current.delete(sessionId);
      const stateProjectPath = bootstrapRef.current?.runtimes.find((runtime) => runtime.sessionId === sessionId)?.projectPath
        ?? (pendingSessionRef.current?.sessionId.startsWith('pending-new:')
          ? pendingSessionRef.current.projectPath
          : pendingSessionRef.current?.sessionId === sessionId ? pendingSessionRef.current.projectPath : undefined);
      const cachedDesktop = stateProjectPath ? sharedDesktopCacheRef.current.get(stateProjectPath) : undefined;
      updateRuntime(sessionId, (current) => {
        const next: RuntimeState = {
          ...current,
          connection: state,
          desktop: seedDesktopFromCache(current.desktop, cachedDesktop),
        };
        if (shouldResetBridgeRuntime(state)) {
          const reset = emptyRuntimeState(state);
          return {
            ...reset,
            connection: state,
            desktop: seedDesktopFromCache(reset.desktop, cachedDesktop),
            runtimeCenter: resetRuntimeCenterConnection(current.runtimeCenter),
          };
        }
        if (shouldClearPendingPermissions(state)) {
          next.runtimeCenter = {
            ...current.runtimeCenter,
            agents: { ...current.runtimeCenter.agents, ...interruptedSideQuestionAgents(current.runtimeCenter) },
          };
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
      if (shouldClearPendingPermissions(state) && !pendingFusionDispatches.current.has(sessionId)) {
        turnActiveRefs.current.set(sessionId, false);
        engineTurnActiveRefs.current.set(sessionId, false);
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
        void host.bootstrap().then((snapshot) => {
          // `newSession` publishes a synthetic target while the main process
          // finishes its navigation transaction. Once the actual runtime has
          // handshaken, promote that target immediately so prompt/model/file
          // actions can use the live bridge while catalog and diagnostics
          // metadata continue to arrive in the background.
          const pending = pendingSessionRef.current;
          const connectedRuntime = pending?.sessionId.startsWith('pending-new:')
            ? snapshot.runtimes.find((runtime) => runtime.sessionId === sessionId
              && runtime.projectPath === pending.projectPath
              && runtime.connection.status === 'connected')
            : undefined;
          if (connectedRuntime && pending && pendingSessionRef.current?.sessionId === pending.sessionId) {
            const target = { projectPath: connectedRuntime.projectPath, sessionId: connectedRuntime.sessionId } satisfies SessionRef;
            pendingSessionRef.current = target;
            setPendingSession(target);
            setSessionReady(true);
          }
          applyBootstrap(snapshot);
        }).catch((cause) => setError(messageFrom(cause)));
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

  const submittedSessionFor = useCallback((sessionId: string, text: string): SubmittedSession | undefined => {
    const ref = bootstrapRef.current?.runtimes.find((entry) => entry.sessionId === sessionId)
      ?? (activeSessionIdRef.current === sessionId
        ? pendingSessionRef.current ?? bootstrapRef.current?.activeSession ?? bootstrapRef.current?.settings.activeSession : undefined);
    const saved = ref && bootstrapRef.current?.projectCatalogs[ref.projectPath]?.sessions.find((entry) => entry.uuid === sessionId);
    return firstSubmittedSession(ref, text, saved);
  }, []);

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
    turnActiveRefs.current.set(sessionId, true);
    runtimeResourceSendSequence.current += 1;
    trackedTurnSequence.current += 1;
    const token = createDesktopTurnToken(sessionId, trackedTurnSequence.current, options.purpose ?? 'composer', allocateTrackedTurnId());
    enqueueTrackedTurn(pendingTrackedTurns.current, token);
    const sendToken = `${sessionId}:${runtimeResourceSendSequence.current}`;
    const resources = promptRuntimeResources(sessionId, sendToken, images, imageNames, filePaths);
    const submittedSession = submittedSessionFor(sessionId, trimmed);
    let promptItem: ConversationState['items'][number] | undefined;
    updateRuntime(sessionId, (state) => {
      const conversation = appendPendingUserPrompt(state.conversation, trimmed, images);
      promptItem = conversation.items.at(-1);
      return {
        ...state,
        submittedSession: state.submittedSession ?? submittedSession,
        conversation,
        runtimeCenter: addRuntimeResources(state.runtimeCenter, resources),
      };
    });
    pendingTrackedDispatches.current.add(token.clientTurnId);
    const queued = host.sendPrompt(sessionId, trimmed, images, token.turnId).then(() => {
      pendingTrackedDispatches.current.delete(token.clientTurnId);
      cancelledTrackedTokens.current.delete(token.clientTurnId);
      if (removedRuntimeIds.current.has(sessionId)) return;
      updateRuntime(sessionId, (state) => ({
        ...state,
        conversation: acknowledgePromptDispatch(state.conversation, promptItem),
        runtimeCenter: commitRuntimeResources(state.runtimeCenter, sendToken),
      }));
    }).catch((cause) => {
      pendingTrackedDispatches.current.delete(token.clientTurnId);
      dequeueTrackedTurn(pendingTrackedTurns.current, token);
      trackedSpeechListeners.current.delete(trackedListenerKey(token));
      const stillRunning = () => engineTurnActiveRefs.current.get(sessionId) === true
        || activeTrackedTurns.current.has(sessionId) || Boolean(pendingTrackedTurns.current.get(sessionId)?.length)
        || slashPendingRefs.current.get(sessionId) === true;
      turnActiveRefs.current.set(sessionId, stillRunning());
      if (!removedRuntimeIds.current.has(sessionId)) {
        updateRuntime(sessionId, (state) => ({
          ...state,
          conversation: {
            ...reduceEvent({ ...state.conversation, items: state.conversation.items.map((item) =>
              item === promptItem && item.type === 'narration' ? { ...item, delivery: 'failed' as const } : item) }, stillRunning()
              ? { type: 'system_notice', message: 'Failed to queue the pending message.', is_error: true }
              : { type: 'error', kind: { type: 'transport' }, message: 'Failed to send the prompt to the engine.' }),
            running: stillRunning(),
          },
          runtimeCenter: rollbackRuntimeResources(state.runtimeCenter, sendToken),
        }));
      }
      if (!cancelledTrackedTokens.current.delete(token.clientTurnId) && activeSessionIdRef.current === sessionId) capture(cause);
      throw cause;
    });
    return { token, queued };
  }, [capture, host, submittedSessionFor, updateRuntime]);

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

  /** Cancellation follows the submitted owner, including admission before turn_started. */
  const cancelTrackedPrompt = useCallback(async (token: DesktopTurnToken): Promise<void> => {
    if (!host || token.turnId === undefined || removedRuntimeIds.current.has(token.sessionId)) return;
    const pending = pendingTrackedTurns.current.get(token.sessionId)?.some((entry) => entry.clientTurnId === token.clientTurnId);
    const active = activeTrackedTurns.current.get(token.sessionId)?.token.clientTurnId === token.clientTurnId;
    if (!pending && !active) return;
    if (pendingTrackedDispatches.current.has(token.clientTurnId)) cancelledTrackedTokens.current.add(token.clientTurnId);
    try { await host.cancel(token.sessionId, token.turnId); }
    catch (cause) {
      cancelledTrackedTokens.current.delete(token.clientTurnId);
      throw cause;
    }
    if (pending) {
      dequeueTrackedTurn(pendingTrackedTurns.current, token);
      trackedSpeechListeners.current.delete(trackedListenerKey(token));
      if (!activeTrackedTurns.current.has(token.sessionId) && !(pendingTrackedTurns.current.get(token.sessionId)?.length)
        && engineTurnActiveRefs.current.get(token.sessionId) !== true && slashPendingRefs.current.get(token.sessionId) !== true) {
        turnActiveRefs.current.set(token.sessionId, false);
        updateRuntime(token.sessionId, (state) => ({ ...state, conversation: { ...state.conversation, running: false } }));
      }
    }
  }, [host, updateRuntime]);

  const runSlashCommand = useCallback(async (raw: string) => {
    const command = raw.trim();
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId || !command.startsWith('/')) return;
    if (isSideQuestionCommand(command, runtimeStatesRef.current.get(sessionId)?.desktop.slashCommands ?? [])) {
      const turnId = ++nextSideQuestionTurn.current;
      const turns = sideQuestionTurns.current.get(sessionId) ?? new Set<number>();
      turns.add(turnId);
      sideQuestionTurns.current.set(sessionId, turns);
      updateRuntime(sessionId, (state) => ({
        ...state, runtimeCenter: beginSideQuestion(state.runtimeCenter, sessionId, turnId, command),
      }));
      try {
        await host.command(sessionId, { type: 'run_slash_command', raw: command, turn_id: turnId });
      } catch (cause) {
        updateRuntime(sessionId, (state) => ({
          ...state, runtimeCenter: finishSideQuestion(state.runtimeCenter, sessionId, turnId, messageFrom(cause), true),
        }));
      }
      return;
    }
    const pendingFusion = /^\/fusion(?:\s|$)/i.test(command) ? { raw: command } : undefined;
    if (pendingFusion) pendingFusionDispatches.current.set(sessionId, pendingFusion);
    turnActiveRefs.current.set(sessionId, true);
    claimSlashTurn(slashPendingRefs.current, sessionId);
    const submittedSession = slashCommandEchoesRequest(command)
      ? submittedSessionFor(sessionId, command)
      : undefined;
    updateRuntime(sessionId, (state) => ({
      ...state,
      submittedSession: state.submittedSession ?? submittedSession,
      conversation: beginSlashCommand(state.conversation, command),
    }));
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
        conversation: failSlashCommand(state.conversation, command,
          pendingFusion ? messageFrom(cause) : 'Failed to run the slash command.'),
        isCancelling: false,
      }));
      capture(cause);
      return;
    } finally {
      if (pendingFusionDispatches.current.get(sessionId) === pendingFusion) pendingFusionDispatches.current.delete(sessionId);
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
  }, [capture, host, submittedSessionFor, updateRuntime]);

  const beginLocalCommand = useCallback((raw: string) => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({ ...state, conversation: beginLocalSlashCommand(state.conversation, raw) }));
  }, [updateRuntime]);

  const dismissCommandResult = useCallback(() => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({
      ...state, conversation: { ...state.conversation, commandResult: null },
    }));
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
    if (sessionLoadingRef.current || !host || !sessionId) return Promise.resolve();
    const foregroundActive = turnActiveRefs.current.get(sessionId) === true;
    const agentIds = runningSubagentIds(runtimeStatesRef.current.get(sessionId)?.runtimeCenter);
    if (!foregroundActive && !agentIds.length) return Promise.resolve();
    const cancelling = cancellingRefs.current.get(sessionId) ?? { current: false };
    const taskRef = cancellationTasks.current.get(sessionId) ?? { current: null };
    cancellingRefs.current.set(sessionId, cancelling);
    cancellationTasks.current.set(sessionId, taskRef);
    if (cancelling.current) return taskRef.current ?? Promise.resolve();
    cancelling.current = true;
    pendingFusionDispatches.current.delete(sessionId);
    updateRuntime(sessionId, (state) => ({ ...state, isCancelling: true }));
    let task: Promise<void>;
    const stop = foregroundActive
      ? host.cancel(sessionId, turnId)
      : stopSessionSubagents(host, sessionId, agentIds);
    task = stop.then(() => {
      updateRuntime(sessionId, (state) => {
        const permissionQueue = state.permissionQueue.filter((entry) => entry.backgroundOwned === true);
        const resolvedPermissionIds = new Set(state.resolvedPermissionIds);
        for (const entry of state.permissionQueue) {
          if (entry.backgroundOwned !== true) resolvedPermissionIds.add(entry.request_id);
        }
        const resolvedAskUserQuestionIds = new Set(state.resolvedAskUserQuestionIds);
        for (const entry of state.askUserQuestionQueue) resolvedAskUserQuestionIds.add(entry.request_id);
        return {
          ...state,
          permissionQueue,
          computerAccessQueue: [],
          askUserQuestionQueue: [],
          pendingInteractionsOverride: permissionQueue.length,
          pendingAskUserQuestionsOverride: 0,
          resolvedPermissionIds,
          resolvedAskUserQuestionIds,
        };
      });
    }).catch((cause) => {
      if (taskRef.current === task) {
        cancelling.current = false;
        taskRef.current = null;
        updateRuntime(sessionId, (state) => ({ ...state, isCancelling: false }));
      }
      capture(cause);
    }).finally(() => {
      // Background-only cancellation has no turn_ended event to release this latch.
      if (!foregroundActive && taskRef.current === task) {
        clearCancellationRuntime(cancelling, taskRef);
        updateRuntime(sessionId, (state) => ({ ...state, isCancelling: false }));
      }
    });
    taskRef.current = task;
    return task;
  }, [capture, host, updateRuntime]);

  const approve = useCallback(async (requestId: number, response?: PermissionResponseDto) => {
    const sessionId = activeSessionId;
    const permission = permissionQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !permission || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.permissionQueue.find((entry) => entry.request_id === requestId) !== permission) return;
    try { await host.approve(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'permission', permission); }
    catch (cause) {
      if (isPermissionRequestGone(cause)) {
        acknowledgeInteraction(sessionId, requestId, 'permission', permission);
        return;
      }
      if (!removedRuntimeIds.current.has(sessionId)) updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
      if (activeSessionIdRef.current === sessionId) capture(cause);
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, capture, host, permissionQueue, updateRuntime]);

  const deny = useCallback(async (requestId: number) => {
    const sessionId = activeSessionId;
    const permission = permissionQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !permission || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.permissionQueue.find((entry) => entry.request_id === requestId) !== permission) return;
    try { await host.deny(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'permission', permission); }
    catch (cause) {
      if (isPermissionRequestGone(cause)) {
        acknowledgeInteraction(sessionId, requestId, 'permission', permission);
        return;
      }
      if (!removedRuntimeIds.current.has(sessionId)) updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
      if (activeSessionIdRef.current === sessionId) capture(cause);
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, capture, host, permissionQueue, updateRuntime]);

  const approveComputerAccess = useCallback(async (requestId: number, response: ComputerAccessResponseDto) => {
    const sessionId = activeSessionId;
    const request = computerAccessQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !request || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.computerAccessQueue.find((entry) => entry.request_id === requestId) !== request) return;
    try { await host.approveComputerAccess(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'computerAccess', request); }
    catch (cause) {
      if (!removedRuntimeIds.current.has(sessionId)
        && runtimeStatesRef.current.get(sessionId)?.computerAccessQueue.find((entry) => entry.request_id === requestId) === request) {
        updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
        if (activeSessionIdRef.current === sessionId) capture(cause);
      }
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, capture, computerAccessQueue, host, updateRuntime]);

  const denyComputerAccess = useCallback(async (requestId: number) => {
    const sessionId = activeSessionId;
    const request = computerAccessQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !request || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.computerAccessQueue.find((entry) => entry.request_id === requestId) !== request) return;
    try { await host.denyComputerAccess(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'computerAccess', request); }
    catch (cause) {
      if (!removedRuntimeIds.current.has(sessionId)
        && runtimeStatesRef.current.get(sessionId)?.computerAccessQueue.find((entry) => entry.request_id === requestId) === request) {
        updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
        if (activeSessionIdRef.current === sessionId) capture(cause);
      }
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, capture, computerAccessQueue, host, updateRuntime]);

  const answerAskUserQuestion = useCallback(async (requestId: number, answers: Record<string, string>) => {
    const sessionId = activeSessionId;
    const request = askUserQuestionQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !request || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.askUserQuestionQueue.find((entry) => entry.request_id === requestId) !== request) return;
    try { await host.answerAskUserQuestion(sessionId, requestId, answers); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion', request); }
    catch (cause) {
      if (!removedRuntimeIds.current.has(sessionId)
        && runtimeStatesRef.current.get(sessionId)?.askUserQuestionQueue.find((entry) => entry.request_id === requestId) === request) {
        updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
        if (activeSessionIdRef.current === sessionId) capture(cause);
      }
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, askUserQuestionQueue, capture, host, updateRuntime]);

  const cancelAskUserQuestion = useCallback(async (requestId: number) => {
    const sessionId = activeSessionId;
    const request = askUserQuestionQueue.find((entry) => entry.request_id === requestId);
    if (!host || !sessionId || !request || removedRuntimeIds.current.has(sessionId)
      || runtimeStatesRef.current.get(sessionId)?.askUserQuestionQueue.find((entry) => entry.request_id === requestId) !== request) return;
    try { await host.cancelAskUserQuestion(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion', request); }
    catch (cause) {
      if (!removedRuntimeIds.current.has(sessionId)
        && runtimeStatesRef.current.get(sessionId)?.askUserQuestionQueue.find((entry) => entry.request_id === requestId) === request) {
        updateRuntime(sessionId, (state) => ({ ...state, error: messageFrom(cause) }));
        if (activeSessionIdRef.current === sessionId) capture(cause);
      }
      throw cause;
    }
  }, [acknowledgeInteraction, activeSessionId, askUserQuestionQueue, capture, host, updateRuntime]);

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
  const updateSidebarPreferences = useCallback(async (sidebar: import('../../shared/settings').SidebarPreferences) => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ sidebar }) }); }
    catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);
  const touchSession = useCallback(async (projectPath: string, sessionId: string) => {
    if (!host) return;
    try {
      const result = await host.touchSession(projectPath, sessionId);
      setBootstrap((previous) => previous ? {
        ...previous,
        projectCatalogs: {
          ...previous.projectCatalogs,
          [result.projectPath]: { sessions: result.sessions.map((session) => ({ ...session })), ...(result.error ? { error: result.error } : {}) },
        },
      } : previous);
    } catch (cause) { capture(cause); }
  }, [capture, host]);
  const renameSession = useCallback(async (projectPath: string, sessionId: string, title: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const result = await host.renameSession(projectPath, sessionId, title);
      setBootstrap((previous) => previous ? {
        ...previous,
        projectCatalogs: {
          ...previous.projectCatalogs,
          [result.projectPath]: { sessions: result.sessions.map((session) => ({ ...session })), ...(result.error ? { error: result.error } : {}) },
        },
      } : previous);
    } catch (cause) {
      capture(cause);
      throw cause;
    }
  }, [capture, host]);

  const openSession = useCallback(async (projectPath: string, sessionId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    const operationId = beginNavigationOperation();
    const target = { projectPath, sessionId } satisfies SessionRef;
    pendingSessionRef.current = target;
    setSessionReady(false);
    sessionLoadingRef.current = true;
    setPendingSession(target);
    removedRuntimeIds.current.delete(sessionId);
    try {
      const snapshot = await host.openSession(projectPath, sessionId);
      if (isCurrentNavigationOperation(operationId)) {
        pendingSessionRef.current = null;
        setSessionReady(false);
        sessionLoadingRef.current = false;
        setPendingSession(null);
        applyBootstrap(snapshot);
        setError(null);
      }
    }
    catch (cause) {
      if (!isCurrentNavigationOperation(operationId)) return;
      pendingSessionRef.current = null;
      setSessionReady(false);
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
      backgroundAgentsRunning: runningSubagentIds(state?.runtimeCenter).length > 0,
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

  const loginCodex = useCallback(async () => {
    if (!host) throw new Error('Desktop host unavailable.');
    const update = await host.loginCodex();
    setBootstrap((previous) => previous ? {
      ...previous, settings: update.settings,
      providerCredentials: [...(previous.providerCredentials ?? []).filter((entry) => entry.providerId !== update.credential.providerId), update.credential],
    } : previous);
    return update;
  }, [host]);

  const cancelCodexLogin = useCallback(async () => {
    if (!host) throw new Error('Desktop host unavailable.');
    await host.cancelCodexLogin();
  }, [host]);

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

  const setVoicePreferences = useCallback(async (voice: VoicePreferences, expectedRevision: number) => {
    if (!host) throw new Error('settings host is unavailable');
    patchBootstrap({ settings: await host.updateSettings({ voice, voiceRevision: expectedRevision }) });
  }, [host, patchBootstrap]);

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

  const audioExecute = useCallback(async (
    operation: AudioOperationDto,
    configurationRevision?: number,
    configurationOverride?: AudioConfigurationV3,
  ): Promise<NativeAudioOperationResponse> => {
    if (!host?.audio) throw new Error('native audio is unavailable on this host');
    const response = await host.audio.execute(operation, configurationRevision, configurationOverride);
    setAudioSnapshot(response.snapshot);
    return response;
  }, [host]);

  const audioCancel = useCallback(async () => {
    if (!host?.audio) throw new Error('native audio is unavailable on this host');
    await host.audio.cancel();
  }, [host]);

  const audioFinishListen = useCallback(async () => {
    if (!host?.audio) throw new Error('native audio is unavailable on this host');
    await host.audio.finishListen();
  }, [host]);

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
    // Reserve a local draft while the host creates the real session and starts its engine.
    const target = { projectPath, sessionId: `pending-new:${operationId}` };
    pendingSessionRef.current = target;
    setSessionReady(false);
    sessionLoadingRef.current = true;
    setPendingSession(target);
    setError(null);
    try {
      const snapshot = await host.newSession(projectPath, desktop.currentModel ?? undefined);
      if (isCurrentNavigationOperation(operationId)) {
        pendingSessionRef.current = null;
        setSessionReady(false);
        sessionLoadingRef.current = false;
        setPendingSession(null);
        applyBootstrap(snapshot);
      }
    }
    catch (cause) {
      if (!isCurrentNavigationOperation(operationId)) return;
      pendingSessionRef.current = null;
      setSessionReady(false);
      sessionLoadingRef.current = false;
      setPendingSession(null);
      capture(cause);
    }
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
  const clearSession = useCallback(async (name?: string) => {
    const sessionId = activeSessionIdRef.current;
    if (sessionLoadingRef.current || !host || !sessionId) return;
    try {
      await host.clearSession(sessionId, name);
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
  const refreshModelPicker = useCallback(async () => {
    await Promise.all([
      command({ type: 'list_models' }),
      command({ type: 'get_conversation_controls' }),
    ]);
  }, [command]);
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
    if (agentId.startsWith(SIDE_QUESTION_AGENT_PREFIX)) return;
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
    async (destination: WritableScopeDto, behavior: PermissionBehaviorDto, add: string[], remove: string[]) => {
      await command({ type: 'update_permission_rules', destination, behavior, add, remove });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const setDefaultPermissionMode = useCallback(
    async (destination: WritableScopeDto, mode: string) => {
      await command({ type: 'set_default_permission_mode', destination, mode });
      await refreshSettingsSnapshot();
    },
    [command, refreshSettingsSnapshot],
  );
  const updateWorkspaceDirectories = useCallback(
    async (destination: WritableScopeDto, add: string[], remove: string[]) => {
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
    async (scope: WritableScopeDto, name: string, config: Record<string, unknown>) => {
      await command({ type: 'upsert_mcp_server', scope, name, config_json: JSON.stringify(config) });
      await refreshMcpServers();
    },
    [command, refreshMcpServers],
  );
  const removeMcpServer = useCallback(
    async (scope: WritableScopeDto, name: string) => {
      await command({ type: 'remove_mcp_server', scope, name });
      await refreshMcpServers();
    },
    [command, refreshMcpServers],
  );
  const [remoteScheduledScopes, setRemoteScheduledScopes] = useState<ScheduledScope[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    void host?.scheduled?.scopes().then((scopes) => { if (!cancelled) setRemoteScheduledScopes(scopes); }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [host, bootstrap?.settings.projects]);
  const scheduledScopes = useMemo<ScheduledScope[]>(() => remoteScheduledScopes ?? [
    { id: 'global', label: 'No project' },
    ...(bootstrap?.settings.projects ?? []).map((path) => ({ id: path, projectPath: path, label: path.split('/').filter(Boolean).pop() ?? path })),
  ], [bootstrap?.settings.projects, remoteScheduledScopes]);
  const scheduledContext = useCallback(async (scopeId: string): Promise<ScheduledContext> => {
    if (!host?.scheduled) throw new Error('Scheduled task service unavailable.');
    return host.scheduled.context(scopeId);
  }, [host]);
  const manageScheduled = useCallback(async (scopeId: string, request: CronRequestDto) => {
    if (!host?.scheduled) throw new Error('Scheduled task service unavailable.');
    return host.scheduled.manage(scopeId, request);
  }, [host]);
  const readScheduledHistory = useCallback(async (scopeId: string, jobId: string): Promise<CronRunDto[]> => {
    const jobs = await manageScheduled(scopeId, { action: 'history', id: jobId });
    return jobs.find((job) => job.id === jobId)?.automation?.runs ?? [];
  }, [manageScheduled]);
  const openScheduledSession = useCallback(async (scopeId: string, sessionId: string) => {
    if (!host?.scheduled) throw new Error('Scheduled task service unavailable.');
    await host.scheduled.openSession(scopeId, sessionId);
    applyBootstrap(await host.bootstrap());
  }, [host, applyBootstrap]);
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
    const sessionId = activeSessionIdRef.current;
    return requestConfigurationOperation(
      pendingConfigurationOperations.current,
      sessionId,
      domain,
      operationId,
      () => host.command(sessionId, envelope).catch((cause: unknown) => {
        if (activeSessionIdRef.current === sessionId) return capture(cause);
        throw cause;
      }),
      CONFIGURATION_OPERATION_TIMEOUT_MS,
    );
  }, [capture, command, host]);
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
    scheduledScopes, scheduledContext, manageScheduled, readScheduledHistory, openScheduledSession,
    manageCron,
    uiSurfaceClientId: stableUiSurfaceClientId,
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
    cancelTrackedPrompt,
    sendPrompt,
    runSlashCommand,
    beginLocalCommand,
    emitCommandOutput,
    dismissCommandResult,
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
    audioExecute,
    audioCancel,
    audioFinishListen,
    addProject,
    activateProject,
    removeProject,
    updateSidebarPreferences,
    archiveSession,
    preflightSessionArchive,
    setSessionPinned,
    touchSession,
    renameSession,
    openSession,
    listProjectSessions,
    sessionRuntimeStatus,
    searchWorkspaceFiles,
    setProviderCredential,
    clearProviderCredential,
    loginCodex,
    cancelCodexLogin,
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
    refreshModelPicker,
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
