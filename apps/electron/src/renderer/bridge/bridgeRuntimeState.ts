import type {
  AskUserQuestionRequestDto,
  ClientEvent,
  ComputerAccessRequestDto,
} from '@lingxi/bridge-client';
import type { HostPermissionRequest } from '../../shared/permission.js';
import { emptyConversation } from './conversation.js';
import type { ConversationState } from './conversation.js';
import { emptyDesktopState } from './desktopState.js';
import type { DesktopState } from './desktopState.js';
import type { BootstrapState, ConnectionState, SessionRef, WorkspaceMetadata } from './lingxi.js';
import { emptyRuntimeCenterState } from './runtimeCenterState.js';
import type { RuntimeCenterState } from './runtimeCenterState.js';
import type { SubmittedSession } from './submittedSessionCatalog.js';

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
  permissionQueue: HostPermissionRequest[];
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

export function pendingAskQueueFromBootstrap(snapshot: BootstrapState): AskUserQuestionRequestDto[] {
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

export interface RuntimeState {
  submittedSession?: SubmittedSession;
  connection: ConnectionState;
  conversation: ConversationState;
  desktop: DesktopState;
  runtimeCenter: RuntimeCenterState;
  permissionQueue: HostPermissionRequest[];
  computerAccessQueue: ComputerAccessRequestDto[];
  askUserQuestionQueue: AskUserQuestionRequestDto[];
  pendingInteractionsOverride?: number;
  pendingAskUserQuestionsOverride?: number;
  resolvedPermissionIds: Set<number>;
  resolvedAskUserQuestionIds: Set<number>;
  isCancelling: boolean;
  error?: string;
}

/** Project-scoped listing data that is safe to paint while a new runtime refreshes. */
export interface SharedDesktopCache {
  models: string[];
  modelDetails: DesktopState['modelDetails'];
  providerModelCatalog: DesktopState['providerModelCatalog'];
  slashCommands: DesktopState['slashCommands'];
}

export function updateSharedDesktopCache(
  cache: SharedDesktopCache | undefined,
  event: ClientEvent,
): SharedDesktopCache | undefined {
  switch (event.type) {
    case 'model_list':
      return {
        models: [...event.models],
        modelDetails: [...(event.details ?? [])],
        providerModelCatalog: cache?.providerModelCatalog ?? [],
        slashCommands: cache?.slashCommands ?? [],
      };
    case 'provider_model_catalog':
      return {
        models: cache?.models ?? [],
        modelDetails: cache?.modelDetails ?? [],
        providerModelCatalog: [...event.providers],
        slashCommands: cache?.slashCommands ?? [],
      };
    case 'slash_command_catalog':
    case 'commands_changed':
      return {
        models: cache?.models ?? [],
        modelDetails: cache?.modelDetails ?? [],
        providerModelCatalog: cache?.providerModelCatalog ?? [],
        slashCommands: [...event.commands],
      };
    default:
      return cache;
  }
}

export function seedDesktopFromCache(state: DesktopState, cache: SharedDesktopCache | undefined): DesktopState {
  if (!cache) return state;
  return {
    ...state,
    models: state.models.length > 0 ? state.models : [...cache.models],
    modelDetails: state.modelDetails.length > 0 ? state.modelDetails : [...cache.modelDetails],
    providerModelCatalog: state.providerModelCatalog.length > 0
      ? state.providerModelCatalog
      : [...cache.providerModelCatalog],
    slashCommands: state.slashCommands.length > 0 ? state.slashCommands : [...cache.slashCommands],
  };
}

export function emptyRuntimeState(connection: ConnectionState = { status: 'idle' }): RuntimeState {
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
