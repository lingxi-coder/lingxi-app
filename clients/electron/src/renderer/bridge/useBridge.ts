import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AskUserQuestionRequestDto,
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  ImageRefDto,
  PermissionModeId,
  PermissionRequest,
  PermissionResponseDto,
  ReasoningSelectionDto,
} from '@lingxi/bridge-client';

import {
  appendPendingUserPrompt,
  appendUserPrompt,
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
import type {
  BootstrapState,
  ConnectionState,
  DiagnosticEntry,
  ProviderCredentialMetadata,
  ProviderCredentialUpdate,
  RuntimeEventEnvelope,
  SessionPinInput,
  SessionRef,
  SessionRuntimeSummary,
  SystemSettingsPane,
  WorkspaceFileSearchResult,
  WorkspaceMetadata,
} from './lingxi';

export interface UseBridge {
  readonly hosted: boolean;
  readonly loading: boolean;
  readonly bootstrap: BootstrapState | null;
  readonly connection: ConnectionState;
  readonly connected: boolean;
  readonly conversation: ConversationState;
  readonly desktop: DesktopState;
  readonly usage: UsageSnapshot | null;
  readonly running: boolean;
  readonly isCancelling: boolean;
  readonly pendingPermission: PermissionRequest | null;
  readonly pendingComputerAccess: ComputerAccessRequestDto | null;
  readonly pendingAskUserQuestion: AskUserQuestionRequestDto | null;
  readonly error: string | null;
  clearError(): void;
  sendPrompt(text: string, images?: ImageRefDto[]): Promise<void>;
  runSlashCommand(raw: string): Promise<void>;
  cancel(turnId?: number): Promise<void>;
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(requestId: number): Promise<void>;
  approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(requestId: number): Promise<void>;
  answerAskUserQuestion(requestId: number, answers: Record<string, string>): Promise<void>;
  cancelAskUserQuestion(requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  addProject(): Promise<WorkspaceMetadata | null>;
  activateProject(path: string): Promise<WorkspaceMetadata | null>;
  removeProject(path: string): Promise<void>;
  setSessionPinned(session: SessionPinInput, pinned: boolean): Promise<void>;
  openSession(projectPath: string, sessionId: string): Promise<void>;
  listProjectSessions(projectPath: string): Promise<void>;
  sessionRuntimeStatus(sessionId: string): SessionRuntimeStatus | undefined;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  setWorkspaceTrusted(trusted: boolean): Promise<WorkspaceMetadata>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  setThemePreference(theme: 'dark' | 'light'): Promise<void>;
  restartBridge(): Promise<void>;
  refreshDiagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  refresh(): Promise<void>;
  newSession(): Promise<void>;
  resumeSession(sessionId: string): Promise<void>;
  setModel(model: string): Promise<void>;
  setReasoningSelection(selection: ReasoningSelectionDto): Promise<void>;
  setFastMode(enabled: boolean): Promise<void>;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  refreshTasks(): Promise<void>;
  taskOutput(taskId: string): Promise<void>;
  stopTask(taskId: string): Promise<void>;
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

interface RuntimeState {
  connection: ConnectionState;
  conversation: ConversationState;
  desktop: DesktopState;
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
  const [runtimeStates, setRuntimeStates] = useState<Map<string, RuntimeState>>(new Map());
  const [error, setError] = useState<string | null>(null);
  const turnActiveRefs = useRef(new Map<string, boolean>());
  const cancellingRefs = useRef(new Map<string, { current: boolean }>());
  const cancellationTasks = useRef(new Map<string, { current: Promise<void> | null }>());
  const removedRuntimeIds = useRef(new Set<string>());
  const bootstrapRef = useRef<BootstrapState | null>(null);
  const latestBootstrapRevisionRef = useRef<number | null>(null);
  const navigationOperationRef = useRef(0);
  const pinOperationRef = useRef(0);
  const catalogRefreshTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>());

  const activeSession = bootstrap?.activeSession ?? bootstrap?.settings.activeSession;
  const activeSessionId = activeSession?.sessionId ?? null;
  const activeSessionIdRef = useRef<string | null>(null);
  activeSessionIdRef.current = activeSessionId;
  bootstrapRef.current = bootstrap;

  const capture = useCallback((cause: unknown) => {
    const message = messageFrom(cause);
    setError(message);
    throw cause;
  }, []);

  const runtime = activeSessionId ? runtimeStates.get(activeSessionId) : undefined;
  const connection = runtime?.connection ?? bootstrap?.connection ?? { status: 'idle' as const };
  const conversation = runtime?.conversation ?? emptyConversation();
  const desktop = runtime?.desktop ?? emptyDesktopState();
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
    setBootstrap(snapshot);
    setRuntimeStates((previous) => {
      const next = new Map(previous);
      const summaries = snapshot.runtimes ?? [];
      const runtimeIds = new Set(summaries.map((summary) => summary.sessionId));
      // The snapshot is authoritative, so disposal markers have been
      // reconciled once it arrives and must not accumulate across sessions.
      removedRuntimeIds.current.clear();
      pruneRuntimeMaps(runtimeIds, next, turnActiveRefs.current, cancellingRefs.current, cancellationTasks.current);
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
      const sessionId = snapshot.activeSession?.sessionId ?? snapshot.settings.activeSession?.sessionId;
      if (sessionId && snapshot.pendingAskUserQuestions) {
        const current = next.get(sessionId) ?? emptyRuntimeState();
        next.set(sessionId, {
          ...current,
          askUserQuestionQueue: pendingAskQueueFromBootstrap(snapshot)
            .filter((request) => !current.resolvedAskUserQuestionIds.has(request.request_id)),
        });
      }
      return next;
    });
  }, []);

  const beginNavigationOperation = useCallback((): number => {
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
    const result = await host.listProjectSessions(projectPath);
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
    if (!host || !sessionId) return;
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

    const offEvent = host.onEvent((envelope: RuntimeEventEnvelope<ClientEvent>) => {
      const sessionId = envelope.sessionId;
      if (removedRuntimeIds.current.has(sessionId)) return;
      const event = envelope.event;
      if (event.type === 'turn_started') turnActiveRefs.current.set(sessionId, true);
      if (event.type === 'turn_ended' || event.type === 'session_ended') turnActiveRefs.current.set(sessionId, false);
      updateRuntime(sessionId, (state) => {
        let next = { ...state, conversation: reduceEvent(state.conversation, event), desktop: reduceDesktopEvent(state.desktop, event) };
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
      if (event.type === 'error') {
        updateRuntime(sessionId, (state) => ({ ...state, error: event.message }));
        if (activeSessionIdRef.current === sessionId) setError(event.message);
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
        removeRuntimeFromMaps(sessionId, turnActiveRefs.current, cancellingRefs.current, cancellationTasks.current);
        return;
      }
      if (removedRuntimeIds.current.has(sessionId)) return;
      updateRuntime(sessionId, (current) => {
        const next: RuntimeState = { ...current, connection: state };
        if (shouldResetBridgeRuntime(state)) {
          const reset = emptyRuntimeState(state);
          return { ...reset, connection: state };
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
      if (shouldClearPendingPermissions(state)) turnActiveRefs.current.set(sessionId, false);
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
  }, [applyBootstrap, capture, host, scheduleProjectCatalogRefresh, updateRuntime]);

  useEffect(() => {
    if (!host || !activeSessionId || connection.status !== 'connected') return;
    const trusted = bootstrap?.workspace.trusted;
    if (!trusted) return;
    void Promise.all([
      host.command(activeSessionId, { type: 'list_sessions', limit: 100 }),
      host.command(activeSessionId, { type: 'list_models' }),
      host.command(activeSessionId, { type: 'get_conversation_controls' }),
      requestTaskList(activeSessionId),
      host.command(activeSessionId, { type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] }),
    ]).catch((cause) => setError(messageFrom(cause)));
  }, [activeSessionId, bootstrap?.workspace.trusted, connection.status, host, requestTaskList]);

  const sendPrompt = useCallback(async (text: string, images: ImageRefDto[] = []) => {
    const trimmed = text.trim();
    const sessionId = activeSessionIdRef.current;
    if (!trimmed || !host || !sessionId) return;
    turnActiveRefs.current.set(sessionId, true);
    updateRuntime(sessionId, (state) => ({ ...state, conversation: appendPendingUserPrompt(state.conversation, trimmed, images) }));
    try {
      await host.sendPrompt(sessionId, trimmed, images);
    } catch (cause) {
      turnActiveRefs.current.set(sessionId, false);
      updateRuntime(sessionId, (state) => ({
        ...state,
        conversation: {
          ...reduceEvent(state.conversation, { type: 'error', kind: { type: 'transport' }, message: 'Failed to send the prompt to the engine.' }),
          running: false,
        },
      }));
      capture(cause);
      throw cause;
    }
  }, [capture, host, updateRuntime]);

  const runSlashCommand = useCallback(async (raw: string) => {
    const command = raw.trim();
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId || !command.startsWith('/')) return;
    updateRuntime(sessionId, (state) => ({ ...state, conversation: appendUserPrompt(state.conversation, command) }));
    try {
      await host.command(sessionId, { type: 'run_slash_command', raw: command });
      await host.command(sessionId, { type: 'refresh_listings', which: [{ type: 'slash_commands' }] });
    } catch (cause) {
      updateRuntime(sessionId, (state) => ({
        ...state,
        conversation: reduceEvent(state.conversation, { type: 'error', kind: { type: 'transport' }, message: 'Failed to run the slash command.' }),
      }));
      capture(cause);
    }
  }, [capture, host, updateRuntime]);

  const cancel = useCallback((turnId?: number): Promise<void> => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId || !turnActiveRefs.current.get(sessionId)) return Promise.resolve();
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
    if (!host || !sessionId) return;
    try { await host.approve(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'permission'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const deny = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.deny(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'permission'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const approveComputerAccess = useCallback(async (requestId: number, response: ComputerAccessResponseDto) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.approveComputerAccess(sessionId, requestId, response); acknowledgeInteraction(sessionId, requestId, 'computerAccess'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const denyComputerAccess = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.denyComputerAccess(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'computerAccess'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const answerAskUserQuestion = useCallback(async (requestId: number, answers: Record<string, string>) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.answerAskUserQuestion(sessionId, requestId, answers); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const cancelAskUserQuestion = useCallback(async (requestId: number) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.cancelAskUserQuestion(sessionId, requestId); acknowledgeInteraction(sessionId, requestId, 'askUserQuestion'); }
    catch (cause) { capture(cause); }
  }, [acknowledgeInteraction, capture, host]);

  const openSystemSettings = useCallback(async (pane: SystemSettingsPane) => {
    if (!host) return;
    try { await host.openSystemSettings(pane); } catch (cause) { capture(cause); }
  }, [capture, host]);

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
    try {
      const snapshot = await host.openSession(projectPath, sessionId);
      if (isCurrentNavigationOperation(operationId)) {
        applyBootstrap(snapshot);
        setError(null);
      }
    }
    catch (cause) {
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

  const setWorkspaceTrusted = useCallback(async (trusted: boolean) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try { const workspace = await host.setWorkspaceTrusted(trusted); patchBootstrap({ workspace }); return workspace; }
    catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

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

  const setThemePreference = useCallback(async (theme: 'dark' | 'light') => {
    if (!host) return;
    try { patchBootstrap({ settings: await host.updateSettings({ theme }) }); } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const restartBridge = useCallback(async () => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    setError(null);
    try { await host.restartBridge(sessionId); } catch (cause) { capture(cause); }
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

  const exportDiagnostics = useCallback(async () => {
    if (!host) return null;
    try { return await host.exportDiagnostics(); } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const command = useCallback(async (value: Parameters<NonNullable<typeof host>['command']>[1]) => {
    const sessionId = activeSessionIdRef.current;
    if (!host || !sessionId) return;
    try { await host.command(sessionId, value); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const refresh = useCallback(async () => {
    await Promise.all([
      command({ type: 'list_sessions', limit: 100 }),
      command({ type: 'list_models' }),
      command({ type: 'get_conversation_controls' }),
      requestTaskList(),
      command({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] }),
      refreshDiagnostics(),
    ]);
  }, [command, refreshDiagnostics, requestTaskList]);

  const newSession = useCallback(async () => {
    const projectPath = bootstrap?.settings.activeProject;
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
    const projectPath = bootstrap?.activeSession?.projectPath ?? bootstrap?.settings.activeProject;
    if (!projectPath || !host) return;
    await openSession(projectPath, sessionId);
  }, [bootstrap?.activeSession?.projectPath, bootstrap?.settings.activeProject, host, openSession]);

  const setModel = useCallback((model: string) => command({ type: 'set_model', model }), [command]);
  const setReasoningSelection = useCallback((selection: ReasoningSelectionDto) => command({ type: 'set_reasoning_selection', selection }), [command]);
  const setFastMode = useCallback((enabled: boolean) => command({ type: 'set_fast_mode', enabled }), [command]);
  const setPermissionMode = useCallback((mode: PermissionModeId) => command({ type: 'set_permission_mode', mode }), [command]);
  const refreshTasks = useCallback(() => requestTaskList(), [requestTaskList]);
  const taskOutput = useCallback((taskId: string) => command({ type: 'task_output', task_id: taskId, offset: 0 }), [command]);
  const stopTask = useCallback((taskId: string) => command({ type: 'task_stop', task_id: taskId }), [command]);

  return {
    hosted,
    loading,
    bootstrap,
    connection,
    connected: connection.status === 'connected',
    conversation,
    desktop,
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
    cancel,
    approve,
    deny,
    approveComputerAccess,
    denyComputerAccess,
    answerAskUserQuestion,
    cancelAskUserQuestion,
    openSystemSettings,
    addProject,
    activateProject,
    removeProject,
    setSessionPinned,
    openSession,
    listProjectSessions,
    sessionRuntimeStatus,
    searchWorkspaceFiles,
    setWorkspaceTrusted,
    setProviderCredential,
    clearProviderCredential,
    setThemePreference,
    restartBridge,
    refreshDiagnostics,
    copyDiagnostics,
    exportDiagnostics,
    refresh,
    newSession,
    resumeSession,
    setModel,
    setReasoningSelection,
    setFastMode,
    setPermissionMode,
    refreshTasks,
    taskOutput,
    stopTask,
  };
}
