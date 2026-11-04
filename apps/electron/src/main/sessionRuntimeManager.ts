import { assertSessionRef, isSessionId } from './sessionIdentity.js';
import type { ClientEvent, PermissionModeId } from '@lingxi/bridge-client';
import type { IpcMainInvokeEvent, WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import { resolve } from 'node:path';
import { SessionRuntime } from './bridge.js';
import { urlOrigin } from './bridgeEvents.js';
import {
  CH_ANSWER_ASK_USER_QUESTION,
  CH_APPROVE,
  CH_APPROVE_COMPUTER_ACCESS,
  CH_CANCEL,
  CH_CANCEL_ASK_USER_QUESTION,
  CH_COMMAND,
  CH_MOD_UI_CONTROL,
  CH_MOD_UI_OPERATION,
  CH_CONNECTION_STATE,
  CH_DENY,
  CH_DENY_COMPUTER_ACCESS,
  CH_EVENT_REPLAY,
  CH_SEND_PROMPT,
  ipcMain,
  require,
} from './bridgeIpc.js';
import type {
  BridgeManagerOptions,
  RuntimeSessionMode,
  SequencedRuntimeEventEnvelope,
  SessionRef,
  SessionRuntimeManagerOptions,
  SessionRuntimeSummary,
} from './bridgeTypes.js';
import { sanitizeDiagnostic } from './host-utils.js';
import { VisualizationRouter } from './visualization.js';
import {
  CH_VISUALIZATION_MOUNT,
  CH_VISUALIZATION_UNMOUNT,
  CH_VISUALIZATION_WRITE_STATE,
  parseVisualizationReference,
  parseVisualizationTheme,
} from '../shared/visualization.js';
import { validateRequestId } from './validation.js';


const DEFAULT_MAX_CACHED_RUNTIMES = 6;

const MAX_CONFIGURED_CACHED_RUNTIMES = 32;

export interface LiveDraftSession {
  sessionId: string;
  projectPath: string;
  mode: RuntimeSessionMode;
  engineCwd: string;
}

interface DraftSession extends SessionRef {
  mode: RuntimeSessionMode;
  projectId: string | null;
}

function draftScope(mode: RuntimeSessionMode, projectId: string | null): string {
  return JSON.stringify([mode, projectId]);
}

/** Accept only a live, active, host-created draft with explicit runtime mode and cwd. */
export function resolveActiveDraftSession(
  requested: SessionRef,
  activeRef: SessionRef | undefined,
  draftRef: SessionRef | undefined,
  runtime: Pick<SessionRuntime, 'sessionId' | 'projectPath' | 'sessionMode' | 'engineWorkspacePath' | 'connectionState'> | undefined,
  activeRuntimeSessionId: string | null,
): LiveDraftSession | undefined {
  try { assertSessionRef(requested); } catch { return undefined; }
  if (!activeRef || activeRef.projectPath !== requested.projectPath || activeRef.sessionId !== requested.sessionId
    || !draftRef || draftRef.projectPath !== requested.projectPath || draftRef.sessionId !== requested.sessionId
    || !runtime || activeRuntimeSessionId !== requested.sessionId
    || runtime.projectPath !== requested.projectPath || runtime.sessionId !== requested.sessionId
    || (runtime.sessionMode !== 'code' && runtime.sessionMode !== 'chat') || runtime.connectionState.status !== 'connected'
    || typeof runtime.engineWorkspacePath !== 'string' || !runtime.engineWorkspacePath) return undefined;
  return { projectPath: runtime.projectPath, sessionId: runtime.sessionId, mode: runtime.sessionMode,
    engineCwd: runtime.engineWorkspacePath };
}

/** Runtimes that exist only because a remote viewer opened them; more than this is refused. */
export const MAX_REMOTE_OPENED_RUNTIMES = 3;

/** Remote viewers hold this many runtimes open already (`capacity` on the wire). */
export class RemoteCapacityError extends Error {
  constructor() {
    super('too many sessions are open for remote viewers');
    this.name = 'RemoteCapacityError';
  }
}

/** A runtime kept open for a remote viewer. Releasing it lets the cache trim the runtime again. */
export interface RemoteLease {
  readonly runtime: SessionRuntime;
  release(): void;
}



/**
 * Owns all Electron bridge processes. A runtime is keyed by the engine's
 * stable session UUID, never by a display name or the currently selected
 * project. Project selection is therefore a renderer/navigation concern and
 * cannot accidentally restart another session.
 */
export class SessionRuntimeManager {
  private readonly runtimes = new Map<string, SessionRuntime>();
  private readonly openingSessions = new Map<string, {
    projectPath: string; transcriptPath?: string; mode?: RuntimeSessionMode;
    promise: Promise<SessionRuntime>; activationModes: Set<RuntimeSessionMode | undefined>;
  }>();
  private readonly restartingSessions = new Map<string, Set<Promise<void>>>();
  private readonly backgroundSessionLeases = new Map<string, number>();
  private readonly draftSessions = new Map<string, DraftSession>();
  private readonly openingDraftSessions = new Map<string, Promise<SessionRef>>();
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly targetDestroyedHandlers = new Map<WebContents, () => void>();
  private readonly lastUsed = new Map<string, number>();
  private readonly sessionModelHints = new Map<string, string>();
  private readonly sessionDirectoryGrants = new Map<string, readonly string[]>();
  private readonly sessionPromptDirectories = new Map<string, readonly string[]>();
  private readonly sessionSettingsEnvironment = new Map<string, Record<string, string>>();
  private readonly sessionProjectRoots = new Map<string, string>();
  private readonly sessionTranscriptPaths = new Map<string, string>();
  private readonly sourceCaptureWanted = new Map<string, { ref: SessionRef; scheduled: boolean }>();
  private readonly sourceCapturePending = new Map<string, Promise<void>>();
  private readonly sessionConfigOwners = new Map<string, { configHome: string; globalConfigPath?: string }>();
  private readonly pendingEvictions = new Set<Promise<void>>();
  private readonly evictingSessions = new Map<string, Promise<void>>();
  private readonly maxCachedRuntimes: number;
  private readonly runtimeCreatedListeners = new Set<(runtime: SessionRuntime) => void>();
  /** Sessions whose runtime was opened for a remote viewer and is still leased by one. */
  private readonly remoteOpened = new Set<string>();
  private activeSessionId: string | null = null;
  private accessSequence = 0;
  private cacheTrimScheduled = false;
  private registered = false;

  /** Routes visualization documents and state writes to the session that issued the mount. */
  readonly visualizations = new VisualizationRouter({
    get: (sessionId) => {
      const runtime = this.runtimes.get(sessionId);
      return runtime?.visualizationReady ? runtime : undefined;
    },
    any: () => [...this.runtimes.values()].find((runtime) => runtime.visualizationReady),
  });

  refreshProviderCatalog(trigger: string): void {
    for (const runtime of this.runtimes.values()) runtime.refreshProviderCatalog(trigger);
  }

  constructor(private readonly opts: SessionRuntimeManagerOptions) {
    const requestedLimit = opts.maxCachedRuntimes ?? DEFAULT_MAX_CACHED_RUNTIMES;
    if (!Number.isInteger(requestedLimit) || requestedLimit < 1 || requestedLimit > MAX_CONFIGURED_CACHED_RUNTIMES) {
      throw new Error(`maxCachedRuntimes must be between 1 and ${MAX_CONFIGURED_CACHED_RUNTIMES}`);
    }
    this.maxCachedRuntimes = requestedLimit;
  }

  async refreshProviderAccounts(): Promise<void> {
    await Promise.all([...this.runtimes.values()].filter(runtime => runtime.connectionState.status === 'connected')
      .map(runtime => runtime.refreshProviderAccounts()));
  }

  async applyProviderSettings(): Promise<void> {
    const targets = [...this.runtimes.values()].map(runtime => ({ runtime, ref: { projectPath: runtime.projectPath, sessionId: runtime.sessionId } }));
    for (const { runtime, ref } of targets) {
      if (this.runtimes.get(ref.sessionId) !== runtime) continue;
      await this.restart(ref, () => {
        if (this.runtimes.get(ref.sessionId) !== runtime || [...this.runtimes.values()].some(item => item.hasActiveWork)) throw new Error('Provider settings were saved; wait for active work before applying them.');
      });
    }
  }

  get size(): number {
    return this.runtimes.size;
  }

  get runtimeSummaries(): SessionRuntimeSummary[] {
    return [...this.runtimes.values()].map((runtime) => runtime.summary);
  }

  get draftReferences(): SessionRef[] {
    return [...this.draftSessions.values()].map(({ projectPath, sessionId, mode, projectId }) => ({ projectPath, sessionId, mode, projectId }));
  }

  hasOwnedDraft(ref: SessionRef): boolean { return this.findDraft(ref) !== undefined; }

  get replaySnapshots(): readonly SequencedRuntimeEventEnvelope<ClientEvent>[] {
    return Object.freeze([...this.runtimes.values()].flatMap((runtime) => runtime.replaySnapshot()));
  }



  get(sessionId: string): SessionRuntime | undefined {
    return this.runtimes.get(sessionId);
  }

  getActiveDraftSession(ref: SessionRef, activeRef: SessionRef | undefined): LiveDraftSession | undefined {
    return resolveActiveDraftSession(ref, activeRef, this.findDraft(ref), this.runtimes.get(ref.sessionId), this.activeSessionId);
  }

  /** Reopen the exact UUID captured by a previously admitted durable creation. */
  async restoreDurableDraft(ref: SessionRef, title: string): Promise<SessionRuntime> {
    assertSessionRef(ref);
    if (ref.mode !== 'code' || !ref.projectId || !title.trim()) throw new Error('Invalid Local App draft owner');
    const existing = this.runtimes.get(ref.sessionId);
    if (existing) throw new Error('The Local App draft has an unverified runtime owner');
    const key = `${draftScope('code', ref.projectId)}:${ref.sessionId}`;
    const draft = { ...ref, mode: 'code' as const, projectId: ref.projectId };
    this.draftSessions.set(key, draft);
    try { return await this.ensure(draft, true, undefined, undefined, 'code', title); }
    catch (error) { this.draftSessions.delete(key); throw error; }
  }

  private draftKey(ref: SessionRef): string | undefined {
    return [...this.draftSessions].find(([, draft]) => draft.projectPath === ref.projectPath && draft.sessionId === ref.sessionId)?.[0];
  }

  private findDraft(ref: SessionRef): DraftSession | undefined {
    const key = this.draftKey(ref);
    return key === undefined ? undefined : this.draftSessions.get(key);
  }

  async reopenOwnedDraft(ref: SessionRef, shouldActivate?: () => boolean): Promise<SessionRef> {
    const draft = this.findDraft(ref);
    if (!draft) throw new Error('The draft conversation is no longer available.');
    return this.reopenDraftSession(draft, undefined, shouldActivate);
  }

  /**
   * Be told about every runtime, now and whenever one is created (before it is
   * started). Remote Control uses this to watch sessions it did not open.
   */
  onRuntimeCreated(listener: (runtime: SessionRuntime) => void): () => void {
    this.runtimeCreatedListeners.add(listener);
    for (const runtime of [...this.runtimes.values()]) this.announceRuntime(listener, runtime);
    return () => { this.runtimeCreatedListeners.delete(listener); };
  }

  private announceRuntime(listener: (runtime: SessionRuntime) => void, runtime: SessionRuntime): void {
    try { listener(runtime); } catch (error) {
      this.opts.diagnostics?.add('warn', 'host', `runtime listener failed: ${sanitizeDiagnostic(error)}`);
    }
  }

  setSessionDirectoryGrants(sessionId: string, directories: readonly string[]): void {
    this.sessionDirectoryGrants.set(sessionId, [...directories]);
  }

  setSessionProjectRoot(sessionId: string, projectRoot: string): void {
    this.sessionProjectRoots.set(sessionId, projectRoot);
  }

  setSessionPromptDirectories(sessionId: string, directories: readonly string[]): void {
    this.sessionPromptDirectories.set(sessionId, [...directories]);
  }

  /** Preserve the unsent draft across a same-UUID directory remount. */
  prepareDraftRelocation(ref: SessionRef, projectPath: string): () => void {
    assertSessionRef(ref);
    const draft = this.findDraft(ref);
    if (!draft || draft.sessionId !== ref.sessionId || projectPath === ref.projectPath) return () => {};
    const key = this.draftKey(ref)!;
    const targetDraft = this.draftSessions.get(draftScope(draft.mode, draft.projectId));
    if (targetDraft && targetDraft.sessionId !== ref.sessionId) {
      throw new Error('The destination project already has an unsent draft.');
    }
    // The host invokes this immediately after its settings transaction, with
    // no asynchronous boundary, before releasing the old workspace lease.
    return () => {
      this.draftSessions.set(key, { ...draft, projectPath });
    };
  }

  private touch(sessionId: string): void {
    if (this.runtimes.has(sessionId)) this.lastUsed.set(sessionId, ++this.accessSequence);
  }

  private activate(runtime: SessionRuntime): void {
    this.activeSessionId = runtime.sessionId;
    this.touch(runtime.sessionId);
    this.trimCache();
  }

  /** Select an already owned runtime without stopping the previously visible one. */
  activateSession(ref: SessionRef): void {
    this.activate(this.require(ref));
  }

  clearActiveSelection(): void {
    this.activeSessionId = null;
    this.trimCache();
  }

  private runtimeIsPinned(runtime: SessionRuntime): boolean {
    const status = runtime.connectionState.status;
    return runtime.sessionId === this.activeSessionId
      || runtime.hasActiveWork
      || runtime.cronOperationPending
      || status === 'spawning'
      || status === 'restarting'
      || status === 'connecting'
      || this.openingSessions.has(runtime.sessionId)
      || [...this.draftSessions].some(([key, draft]) => draft.sessionId === runtime.sessionId
        && this.openingDraftSessions.has(key))
      || this.restartingSessions.has(runtime.sessionId)
      || this.backgroundSessionLeases.has(runtime.sessionId);
  }

  private trimCache(): void {
    while (this.runtimes.size > this.maxCachedRuntimes) {
      const victim = [...this.runtimes.values()]
        .filter((runtime) => !this.runtimeIsPinned(runtime))
        .sort((left, right) => (this.lastUsed.get(left.sessionId) ?? 0) - (this.lastUsed.get(right.sessionId) ?? 0))[0];
      if (!victim) return;
      this.runtimes.delete(victim.sessionId);
      this.sourceCaptureWanted.delete(victim.sessionId);
      this.visualizations.forgetSession(victim.sessionId);
      this.lastUsed.delete(victim.sessionId);
      this.sessionModelHints.delete(victim.sessionId);
      const eviction = victim.dispose()
        .catch((error) => this.opts.diagnostics?.add('warn', 'host', `session cache eviction failed: ${sanitizeDiagnostic(error)}`))
        .finally(() => {
          this.pendingEvictions.delete(eviction);
          if (this.evictingSessions.get(victim.sessionId) === eviction) this.evictingSessions.delete(victim.sessionId);
        });
      this.pendingEvictions.add(eviction);
      this.evictingSessions.set(victim.sessionId, eviction);
    }
  }

  private scheduleCacheTrim(sessionId?: string): void {
    if (sessionId) this.touch(sessionId);
    if (this.cacheTrimScheduled) return;
    this.cacheTrimScheduled = true;
    queueMicrotask(() => {
      this.cacheTrimScheduled = false;
      this.trimCache();
    });
  }

  require(ref: SessionRef): SessionRuntime {
    assertSessionRef(ref);
    const runtime = this.runtimes.get(ref.sessionId);
    if (!runtime) throw new Error(`session runtime is not open: ${ref.sessionId}`);
    if (runtime.projectPath !== ref.projectPath) {
      throw new Error('session id is owned by a different project');
    }
    return runtime;
  }

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const allowedOrigin = urlOrigin(rendererUrl);
    if (!allowedOrigin) throw new Error('invalid renderer URL');
    const rendererUrls = this.targets.get(webContents) ?? new Set<string>();
    rendererUrls.add(rendererUrl);
    this.targets.set(webContents, rendererUrls);
    if (!this.targetDestroyedHandlers.has(webContents)) {
      const onDestroyed = (): void => {
        this.detachWindow(webContents);
      };
      this.targetDestroyedHandlers.set(webContents, onDestroyed);
      webContents.once('destroyed', onDestroyed);
    }
    for (const runtime of this.runtimes.values()) runtime.registerWindow(webContents, rendererUrl);
  }

  /** Re-deliver pending interaction prompts after a renderer document reload. */
  replayPendingInteractions(webContents: WebContents): void {
    for (const runtime of this.runtimes.values()) runtime.replayPendingInteractions(webContents);
  }

  /** Create, validate, and optionally start one session-owned runtime. */
  async ensure(ref: SessionRef, start = true, resumeModel?: string, remountControls?: string, sessionMode?: RuntimeSessionMode, durableDraftTitle?: string): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const eviction = this.evictingSessions.get(ref.sessionId);
    if (eviction) {
      await eviction;
      return this.ensure(ref, start, resumeModel, remountControls, sessionMode, durableDraftTitle);
    }
    const existing = this.runtimes.get(ref.sessionId);
    if (existing) {
      if (durableDraftTitle) throw new Error('The new durable draft already has a runtime owner.');
      if (existing.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      if (ref.mode && existing.sessionMode && ref.mode !== existing.sessionMode) throw new Error('session mode does not match its owner');
      if (start) await existing.start();
      this.touch(existing.sessionId);
      return existing;
    }

    if (resumeModel) this.sessionModelHints.set(ref.sessionId, resumeModel);
    const runtimeOptions = this.runtimeOptions(ref);
    const draft = this.findDraft(ref);
    const draftMode = draft?.sessionId === ref.sessionId ? draft.mode : sessionMode;
    const runtime = new SessionRuntime({ ...runtimeOptions, remountControls }, draftMode, durableDraftTitle);
    this.runtimes.set(ref.sessionId, runtime);
    this.touch(ref.sessionId);
    for (const listener of [...this.runtimeCreatedListeners]) this.announceRuntime(listener, runtime);
    for (const [webContents, origins] of this.targets) {
      if (webContents.isDestroyed()) {
        this.detachWindow(webContents);
        continue;
      }
      for (const rendererUrl of origins) runtime.registerWindow(webContents, rendererUrl);
    }
    try {
      if (start) await runtime.start();
      return runtime;
    } catch (error) {
      this.runtimes.delete(ref.sessionId);
      this.sourceCaptureWanted.delete(ref.sessionId);
      this.visualizations.forgetSession(ref.sessionId);
      this.lastUsed.delete(ref.sessionId);
      this.sessionModelHints.delete(ref.sessionId);
      await runtime.dispose().catch(() => undefined);
      throw error;
    }
  }

  openSession(ref: SessionRef, empty = false, resumeModel?: string, activate = true, transcriptPath?: string, remountControls?: string, durableDraftTitle?: string): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (durableDraftTitle && existing) throw new Error('The new durable draft already has a runtime owner.');
    if (existing && existing.projectPath !== ref.projectPath) {
      throw new Error('session id is owned by a different project');
    }
    if (existing && ref.mode && existing.sessionMode && existing.sessionMode !== ref.mode) {
      throw new Error('session mode does not match its owner');
    }
    const pending = this.openingSessions.get(ref.sessionId);
    if (pending) {
      if (durableDraftTitle) throw new Error('The new durable draft is already being opened.');
      if (pending.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      if (transcriptPath && pending.transcriptPath !== transcriptPath) throw new Error('another transcript selection is being opened');
      if (ref.mode && pending.mode && ref.mode !== pending.mode) throw new Error('session mode does not match its owner');
      if (activate) pending.activationModes.add(ref.mode);
      if (ref.mode && !pending.mode) return pending.promise.then(runtime => {
        if (runtime.sessionMode !== ref.mode) throw new Error('session mode does not match its owner');
        return runtime;
      });
      return pending.promise;
    }
    const restarting = this.restartingSessions.get(ref.sessionId);
    if (restarting?.size) {
      // A restart owns both the handshake and transcript restoration. Reassess
      // after it settles rather than opening a second resume on the same UUID.
      // Retain the pending opener until it can activate its runtime: releasing
      // the restart pin can otherwise evict this still-background session.
      const release = this.retainBackgroundSession(ref);
      return Promise.all([...restarting].map(operation => operation.catch(() => undefined))).then(() => {
        if (this.runtimes.get(ref.sessionId) !== existing) throw new Error('session runtime was replaced while opening');
        return this.openSession(ref, empty, resumeModel, activate, transcriptPath, remountControls, durableDraftTitle);
      }).finally(release);
    }
    if (!existing && resumeModel) this.sessionModelHints.set(ref.sessionId, resumeModel);
    if (existing?.connectionState.status === 'connected' && !existing.isStarting && !existing.needsSessionResume) {
      if (transcriptPath) {
        this.sessionTranscriptPaths.set(ref.sessionId, transcriptPath);
        existing.rememberTranscriptSelection(transcriptPath);
      }
      if (activate) this.activate(existing);
      return Promise.resolve(existing);
    }

    const promise = this.openSessionInternal(ref, existing, empty, transcriptPath, remountControls, durableDraftTitle).then((runtime) => {
      runtime.completeWorkspaceRemount();
      if (transcriptPath) this.sessionTranscriptPaths.set(ref.sessionId, transcriptPath);
      const requested = this.openingSessions.get(ref.sessionId)?.activationModes;
      if (requested?.has(undefined) || (runtime.sessionMode && requested?.has(runtime.sessionMode))) this.activate(runtime);
      return runtime;
    });
    const trackedPromise = promise.finally(() => {
      if (this.openingSessions.get(ref.sessionId)?.promise === trackedPromise) this.openingSessions.delete(ref.sessionId);
    });
    this.openingSessions.set(ref.sessionId, { projectPath: ref.projectPath, transcriptPath, mode: ref.mode,
      promise: trackedPromise, activationModes: new Set(activate ? [ref.mode] : []) });
    return trackedPromise;
  }

  private async openSessionInternal(ref: SessionRef, existing: SessionRuntime | undefined, empty: boolean, transcriptPath?: string, remountControls?: string, durableDraftTitle?: string): Promise<SessionRuntime> {
    let runtime: SessionRuntime | undefined;
    const previousTranscriptPath = this.sessionTranscriptPaths.get(ref.sessionId);
    try {
      if (transcriptPath) {
        this.sessionTranscriptPaths.set(ref.sessionId, transcriptPath);
        existing?.rememberTranscriptSelection(transcriptPath);
      }
      if (existing) {
        let reconnectOnly = false;
        if (!empty && !transcriptPath && !this.sessionTranscriptPaths.get(ref.sessionId)) {
          let saved: { path: string; mode: RuntimeSessionMode } | undefined;
          try { saved = await this.opts.resolveSavedTranscript?.(ref); }
          catch (error) {
            // A metadata failure must not strand a still-living writer and its
            // admitted work. Reconnection authenticates that exact process.
            this.opts.diagnostics?.add('warn', 'bridge', `session source unavailable during reconnect ${ref.sessionId}: ${sanitizeDiagnostic(error)}`);
          }
          if (saved) {
            if ((ref.mode && saved.mode !== ref.mode) || (existing.sessionMode && saved.mode !== existing.sessionMode)) {
              throw new Error('session mode does not match its saved source');
            }
            transcriptPath = saved.path;
            this.sessionTranscriptPaths.set(ref.sessionId, transcriptPath);
            existing.rememberTranscriptSelection(transcriptPath);
          } else {
            reconnectOnly = true;
          }
        }
        // A disconnected transport does not authorize replacing its living
        // session process, and its old foreground latch cannot gate recovery.
        await existing.start(reconnectOnly ? { reconnectOnly: true } : undefined);
        if (reconnectOnly && (!existing.recoveredLiveConnection || existing.needsSessionResume)) {
          throw new Error('The session source is unavailable. Choose an exact transcript before opening it.');
        }
        runtime = existing;
      } else {
        const expectedMode = ref.mode ?? (!empty ? await this.opts.resolveSessionMode?.(ref) : undefined);
        if (!empty && !expectedMode) throw new Error('The session mode is unavailable.');
        if (empty && !expectedMode) throw new Error('The session creation mode is unavailable.');
        if (!empty && !transcriptPath && !this.sessionTranscriptPaths.get(ref.sessionId)) {
          const saved = await this.opts.resolveSavedTranscript?.(ref);
          if (saved) {
            if (saved.mode !== expectedMode) throw new Error('session mode does not match its saved source');
            transcriptPath = saved.path;
            this.sessionTranscriptPaths.set(ref.sessionId, transcriptPath);
          }
        }
        const selectedPath = transcriptPath ?? this.sessionTranscriptPaths.get(ref.sessionId);
        if (!empty && !selectedPath) throw new Error('The session source is unavailable. Choose an exact transcript before opening it.');
        if (durableDraftTitle && (!empty || selectedPath)) throw new Error('A durable draft anchor requires a new chat.');
        runtime = await this.ensure({ ...ref, ...(expectedMode ? { mode: expectedMode } : {}) }, true, undefined, remountControls,
          empty && !selectedPath ? expectedMode : undefined, durableDraftTitle);
        if (durableDraftTitle && this.opts.captureSessionSource) {
          const path = await this.opts.captureSessionSource(ref, runtime, true);
          if (this.runtimes.get(ref.sessionId) !== runtime) throw new Error('The session runtime changed while its source was being recorded.');
          if (path) {
            this.sessionTranscriptPaths.set(ref.sessionId, path);
            runtime.rememberTranscriptSelection(path);
          }
        }
      }
      if (runtime.needsSessionResume || (!empty && !runtime.recoveredLiveConnection)) await runtime.resumeOwnedSession(transcriptPath);
      return runtime;
    } catch (error) {
      if (transcriptPath) {
        if (previousTranscriptPath) this.sessionTranscriptPaths.set(ref.sessionId, previousTranscriptPath);
        else this.sessionTranscriptPaths.delete(ref.sessionId);
        existing?.rememberTranscriptSelection(previousTranscriptPath);
      }
      if (!existing && runtime) {
        if (this.runtimes.get(ref.sessionId) === runtime) {
          this.runtimes.delete(ref.sessionId);
          this.sourceCaptureWanted.delete(ref.sessionId);
          this.visualizations.forgetSession(ref.sessionId);
          this.lastUsed.delete(ref.sessionId);
          this.sessionModelHints.delete(ref.sessionId);
        }
        await runtime.dispose().catch(() => undefined);
      }
      throw error;
    }
  }

  retainBackgroundSession(ref: SessionRef): () => void {
    assertSessionRef(ref);
    this.backgroundSessionLeases.set(ref.sessionId, (this.backgroundSessionLeases.get(ref.sessionId) ?? 0) + 1);
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const count = (this.backgroundSessionLeases.get(ref.sessionId) ?? 1) - 1;
      if (count) this.backgroundSessionLeases.set(ref.sessionId, count);
      else this.backgroundSessionLeases.delete(ref.sessionId);
      this.trimCache();
    };
  }

  async requireSavedSessionSource(ref: SessionRef): Promise<{ path: string; mode: RuntimeSessionMode }> {
    const saved = await this.opts.resolveSavedTranscript?.(ref);
    if (!saved) throw new Error('The session source is unavailable. Choose an exact transcript before opening it.');
    if (ref.mode && ref.mode !== saved.mode) throw new Error('session mode does not match its saved source');
    return saved;
  }

  /**
   * Open a session for a remote viewer, and keep it open until the lease is
   * released. Nothing is shown on the desktop: the session is not activated.
   * Resolves once the engine has resumed it. A session that has no runtime yet
   * counts against {@link MAX_REMOTE_OPENED_RUNTIMES} for as long as the lease
   * lasts (`RemoteCapacityError` beyond that); one that is already open is only
   * kept. Take one lease per session, not one per viewer.
   */
  async openForRemote(ref: SessionRef, limit = MAX_REMOTE_OPENED_RUNTIMES): Promise<RemoteLease> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (existing && existing.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
    const counted = !existing && !this.openingSessions.has(ref.sessionId);
    if (counted && this.remoteOpened.size >= limit) throw new RemoteCapacityError();
    if (counted) this.remoteOpened.add(ref.sessionId);
    const releaseBackground = this.retainBackgroundSession(ref);
    let released = false;
    const release = (): void => {
      if (released) return;
      released = true;
      if (counted) this.remoteOpened.delete(ref.sessionId);
      releaseBackground();
    };
    try {
      const runtime = await this.openSession(ref, false, undefined, false);
      return { runtime, release };
    } catch (error) {
      release();
      throw error;
    }
  }

  /** Keep an inactive runtime alive for the whole host operation, then enforce the cache bound. */
  async withBackgroundSession<T>(
    ref: SessionRef,
    empty: boolean,
    resumeModel: string | undefined,
    operation: (runtime: SessionRuntime) => Promise<T>,
    transcriptPath?: string,
    durableDraftTitle?: string,
  ): Promise<T> {
    assertSessionRef(ref);
    this.backgroundSessionLeases.set(ref.sessionId, (this.backgroundSessionLeases.get(ref.sessionId) ?? 0) + 1);
    try {
      return await operation(await this.openSession(ref, empty, resumeModel, false, transcriptPath, undefined, durableDraftTitle));
    } finally {
      const count = (this.backgroundSessionLeases.get(ref.sessionId) ?? 1) - 1;
      if (count) this.backgroundSessionLeases.set(ref.sessionId, count);
      else this.backgroundSessionLeases.delete(ref.sessionId);
      this.trimCache();
    }
  }

  async newSession(projectPath: string, model: string | undefined, ownership: { mode: RuntimeSessionMode; projectId: string | null; activate?: () => boolean; forceNew?: boolean; durableDraftTitle?: string; onAllocated?: (ref: SessionRef) => Promise<void> }): Promise<SessionRef> {
    if (typeof projectPath !== 'string' || projectPath.length === 0) throw new Error('invalid project path');
    this.assertProjectNotClosing(projectPath);
    if (!ownership || (ownership.mode !== 'chat' && ownership.mode !== 'code')) throw new Error('The session creation mode is unavailable.');
    const { mode, projectId } = ownership;
    if (projectId !== null && (typeof projectId !== 'string' || !projectId)) throw new Error('The session project owner is unavailable.');
    const key = ownership.forceNew ? `${draftScope(mode, projectId)}:${randomUUID()}` : draftScope(mode, projectId);
    const pendingDraft = this.openingDraftSessions.get(key);
    if (pendingDraft) return pendingDraft;
    const draft = this.draftSessions.get(key);
    const promise = draft ? this.reopenDraftSession(draft, model, ownership.activate)
      : this.allocateDraftSession(projectPath, projectId, mode, key, model, ownership.activate, ownership.durableDraftTitle, ownership.onAllocated);
    const trackedPromise = promise.finally(() => {
      if (this.openingDraftSessions.get(key) === trackedPromise) this.openingDraftSessions.delete(key);
      this.scheduleCacheTrim();
    });
    this.openingDraftSessions.set(key, trackedPromise);
    return trackedPromise;
  }

  private async reopenDraftSession(draft: DraftSession, model?: string, shouldActivate?: () => boolean): Promise<SessionRef> {
    const runtime = await this.ensure(draft, true, model);
    if (!shouldActivate || shouldActivate()) this.activate(runtime);
    return { projectPath: draft.projectPath, sessionId: draft.sessionId, projectId: draft.projectId, mode: draft.mode };
  }

  private async allocateDraftSession(projectPath: string, projectId: string | null, mode: RuntimeSessionMode, key: string, model?: string, shouldActivate?: () => boolean, durableDraftTitle?: string, onAllocated?: (ref: SessionRef) => Promise<void>): Promise<SessionRef> {
    let ref: SessionRef | undefined;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const candidate = { projectPath, projectId, mode, sessionId: randomUUID() } satisfies SessionRef;
      // UUIDv4 has enough entropy that checking every persisted transcript is
      // strictly more expensive than the collision it is trying to prevent.
      // Only live in-process ownership matters here; the bridge itself remains
      // the authority for durable session identity once it starts.
      const alreadyOwned = this.runtimes.has(candidate.sessionId)
        || [...this.draftSessions.values()].some((draft) => draft.sessionId === candidate.sessionId);
      if (!alreadyOwned) {
        ref = candidate;
        break;
      }
    }
    if (!ref) throw new Error('could not allocate a new session id');
    this.draftSessions.set(key, { ...ref, projectId, mode });
    try {
      await onAllocated?.(ref);
      const runtime = await this.ensure(ref, true, model, undefined, mode, durableDraftTitle);
      if (!shouldActivate || shouldActivate()) this.activate(runtime);
    } catch (error) {
      this.clearDraftSession(ref);
      const failedRuntime = this.runtimes.get(ref.sessionId);
      if (failedRuntime) {
        this.runtimes.delete(ref.sessionId);
        this.sourceCaptureWanted.delete(ref.sessionId);
        this.visualizations.forgetSession(ref.sessionId);
        this.lastUsed.delete(ref.sessionId);
        this.sessionModelHints.delete(ref.sessionId);
        await failedRuntime.dispose().catch(() => undefined);
      }
      throw error;
    }
    // The boot argument is the source of truth for a new session. Sending a
    // second `new_session` command would create/switch the engine to another
    // UUID and leave the runtime key pointing at the wrong transcript.
    return { ...ref };
  }

  async restart(ref: SessionRef, beforeRestart?: () => void): Promise<void> {
    const pending = this.restartingSessions.get(ref.sessionId) ?? new Set<Promise<void>>();
    const operation = this.restartInternal(ref, beforeRestart);
    const completion = operation.finally(() => {
      pending.delete(completion);
      if (pending.size === 0 && this.restartingSessions.get(ref.sessionId) === pending) this.restartingSessions.delete(ref.sessionId);
      this.scheduleCacheTrim();
    });
    pending.add(completion);
    this.restartingSessions.set(ref.sessionId, pending);
    return completion;
  }

  private async restartInternal(ref: SessionRef, beforeRestart?: () => void): Promise<void> {
    const runtime = this.require(ref);
    if (runtime.hasActiveAudio) throw new Error('Stop audio before restarting this chat.');
    if (runtime.hasActiveMetadata) throw new Error('Wait for the session metadata operation before restarting this chat.');
    const recovering = runtime.connectionState.status !== 'connected' || runtime.isStarting;
    const opening = this.openingSessions.get(ref.sessionId);
    if (opening) {
      if (opening.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      if (recovering) beforeRestart?.();
      await opening.promise;
      if (this.require(ref) !== runtime) throw new Error('session runtime was replaced while restarting');
      if (runtime.hasActiveAudio) throw new Error('Stop audio before restarting this chat.');
      if (recovering) {
        // The open already completed transport recovery and owned restoration.
        // Joining that recovery must not replace a still-living sidecar.
        beforeRestart?.();
        return;
      }
    }
    if (runtime.connectionState.status !== 'connected' || runtime.isStarting) {
      beforeRestart?.();
      if (runtime.hasActiveAudio) throw new Error('Stop audio before restarting this chat.');
      await runtime.start();
      beforeRestart?.();
      if (!runtime.recoveredLiveConnection || runtime.needsSessionResume) await runtime.restoreOwnedSessionIfNeeded();
      return;
    }
    await runtime.restart(() => {
      if (runtime.hasActiveWork) throw new Error('Wait for active work and pending interactions before restarting this chat.');
      beforeRestart?.();
    });
    await runtime.restoreOwnedSessionIfNeeded();
  }

  async closeSession(ref: SessionRef, options?: { replacing?: boolean }): Promise<void> {
    const runtime = this.require(ref);
    if (runtime.hasActiveWork) throw new Error('Wait for active work and pending interactions before closing this chat.');
    // Remove from the routable map BEFORE the first await, for the same reason
    // `closeProject` does: while `dispose()` is in flight `get()` would still
    // hand this runtime out, and `restart()` now rejects on a disposed runtime
    // instead of resolving silently — surfacing a failure for a settings or
    // credential write that actually succeeded.
    this.runtimes.delete(ref.sessionId);
    this.sourceCaptureWanted.delete(ref.sessionId);
    this.visualizations.forgetSession(ref.sessionId);
    this.lastUsed.delete(ref.sessionId);
    this.sessionModelHints.delete(ref.sessionId);
    if (this.activeSessionId === ref.sessionId) this.activeSessionId = null;
    this.clearDraftSession(ref);
    await runtime.dispose(options);
  }

  async closeProject(projectPath: string): Promise<void> {
    this.assertProjectNotClosing(projectPath);
    if (this.hasActiveWork(projectPath)) throw new Error('cancel active turns and pending interactions before removing a project');
    this.closingProjects.add(projectPath);
    for (const [key, draft] of this.draftSessions) {
      if (draft.projectPath === projectPath) { this.draftSessions.delete(key); this.openingDraftSessions.delete(key); }
    }
    const projectRuntimes = [...this.runtimes.values()].filter((runtime) => runtime.projectPath === projectPath);
    // Remove runtimes from the routable map before the first await. A prompt
    // arriving while disposal is in progress must fail instead of entering a
    // runtime whose child is already being torn down.
    for (const runtime of projectRuntimes) {
      this.runtimes.delete(runtime.sessionId);
      this.sourceCaptureWanted.delete(runtime.sessionId);
      this.visualizations.forgetSession(runtime.sessionId);
    }
    for (const runtime of projectRuntimes) this.lastUsed.delete(runtime.sessionId);
    for (const runtime of projectRuntimes) this.sessionModelHints.delete(runtime.sessionId);
    if (projectRuntimes.some((runtime) => runtime.sessionId === this.activeSessionId)) this.activeSessionId = null;
    try {
      await Promise.all(projectRuntimes.map((runtime) => runtime.dispose()));
    } finally {
      this.closingProjects.delete(projectPath);
    }
  }

  isProjectClosing(projectPath: string): boolean {
    return this.closingProjects.has(projectPath);
  }

  hasActiveWork(projectPath: string): boolean {
    return [...this.runtimes.values()].some((runtime) => (
      runtime.projectPath === projectPath
      && runtime.hasActiveWork
    ));
  }

  async disableTelemetry(): Promise<void> {
    // Revoke all host exporters synchronously before any network acknowledgement.
    const results = await Promise.allSettled([...this.runtimes.values()].map(runtime => runtime.disableTelemetry()));
    const failure = results.find(result => result.status === 'rejected');
    if (failure?.status === 'rejected') throw new Error('Some running chats could not confirm diagnostics were disabled.');
  }

  async dispose(): Promise<void> {
    const runtimes = [...this.runtimes.values()];
    this.runtimes.clear();
    this.sourceCaptureWanted.clear();
    this.lastUsed.clear();
    this.sessionModelHints.clear();
    this.activeSessionId = null;
    this.openingSessions.clear();
    this.restartingSessions.clear();
    this.remoteOpened.clear();
    this.runtimeCreatedListeners.clear();
    this.draftSessions.clear();
    this.openingDraftSessions.clear();
    await Promise.all(runtimes.map((runtime) => runtime.dispose().catch(() => undefined)));
    await Promise.all([...this.pendingEvictions]);
    this.evictingSessions.clear();
    this.unregisterIpc();
    for (const webContents of [...this.targets.keys()]) this.detachWindow(webContents);
  }

  registerIpc(): void {
    if (this.registered) return;
    this.registered = true;
    ipcMain.handle(CH_EVENT_REPLAY, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.replaySnapshots;
    });
    ipcMain.handle(CH_SEND_PROMPT, async (event: IpcMainInvokeEvent, sessionId: unknown, text: unknown, images: unknown, turnId?: unknown, visualizationContext?: unknown) => {
      this.assertSender(event);
      const runtime = this.requireById(sessionId);
      await this.waitForRuntimeReadiness(runtime, event, sessionId);
      return runtime.sendPrompt(text, images, turnId, visualizationContext);
    });
    ipcMain.handle(CH_VISUALIZATION_MOUNT, (event: IpcMainInvokeEvent, sessionId: unknown, reference: unknown, theme: unknown, locale: unknown, expanded: unknown) => {
      this.assertSender(event);
      const runtime = this.requireById(sessionId);
      return this.visualizations.mount(
        runtime.sessionId,
        parseVisualizationReference(reference),
        parseVisualizationTheme(theme),
        typeof locale === 'string' && /^[A-Za-z0-9-]{1,35}$/.test(locale) ? locale : 'en',
        expanded === true,
      );
    });
    ipcMain.handle(CH_VISUALIZATION_WRITE_STATE, (event: IpcMainInvokeEvent, sessionId: unknown, token: unknown, generation: unknown, baseVersion: unknown, modelContent: unknown, privateContent: unknown) => {
      this.assertSender(event);
      const runtime = this.requireById(sessionId);
      if (typeof token !== 'string' || !Number.isSafeInteger(generation) || !Number.isSafeInteger(baseVersion)
        || typeof modelContent !== 'string' || typeof privateContent !== 'string') {
        throw new Error('invalid visualization state write');
      }
      return this.visualizations.writeState(runtime.sessionId, token, generation as number, baseVersion as number, modelContent, privateContent);
    });
    ipcMain.handle(CH_VISUALIZATION_UNMOUNT, (event: IpcMainInvokeEvent, sessionId: unknown, token: unknown) => {
      this.assertSender(event);
      if (typeof sessionId !== 'string' || typeof token !== 'string') return;
      return this.visualizations.unmount(sessionId, token);
    });
    ipcMain.handle(CH_APPROVE, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown, response: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).approvePermission(validateRequestId(requestId), response);
    });
    ipcMain.handle(CH_DENY, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).denyPermission(validateRequestId(requestId));
    });
    ipcMain.handle(CH_APPROVE_COMPUTER_ACCESS, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown, response: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).approveComputerAccess(validateRequestId(requestId), response);
    });
    ipcMain.handle(CH_DENY_COMPUTER_ACCESS, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).denyComputerAccess(validateRequestId(requestId));
    });
    ipcMain.handle(CH_ANSWER_ASK_USER_QUESTION, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown, answers: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).answerAskUserQuestion(validateRequestId(requestId), answers);
    });
    ipcMain.handle(CH_CANCEL_ASK_USER_QUESTION, (event: IpcMainInvokeEvent, sessionId: unknown, requestId: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).cancelAskUserQuestion(validateRequestId(requestId));
    });
    ipcMain.handle(CH_CANCEL, (event: IpcMainInvokeEvent, sessionId: unknown, turnId: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).cancelTurn(turnId);
    });
    ipcMain.handle(CH_COMMAND, async (event: IpcMainInvokeEvent, sessionId: unknown, command: unknown) => {
      this.assertSender(event);
      const runtime = this.requireById(sessionId);
      // Connectivity precedes owned transcript/control recovery. Only ordinary
      // commands wait here; approvals, questions and cancellation must remain
      // available while the recovery transaction waits for those interactions.
      await this.waitForRuntimeReadiness(runtime, event, sessionId);
      await runtime.dispatchCommand(command);
    });
    ipcMain.handle(CH_MOD_UI_CONTROL, (event: IpcMainInvokeEvent, sessionId: unknown, request: unknown) => {
      this.assertSender(event);
      return this.requireById(sessionId).dispatchModUiControl(request);
    });
    ipcMain.handle(CH_MOD_UI_OPERATION, (event: IpcMainInvokeEvent, sessionId: unknown, operation: unknown) => {
      this.assertSender(event);
      return this.requireById(sessionId).dispatchModUiOperation(operation);
    });
    ipcMain.handle(CH_CONNECTION_STATE, (event: IpcMainInvokeEvent, sessionId: unknown) => {
      this.assertSender(event);
      return this.requireById(sessionId).connectionState;
    });
  }

  private runtimeOptions(ref: SessionRef): BridgeManagerOptions {
    const {
      launchConfig,
      accessState,
      onModelChanged,
      onModelSelected,
      getSavedModel,
      resolveSessionMode: _resolveSessionMode,
      resolveSavedTranscript: _resolveSavedTranscript,
      captureSessionSource: _captureSessionSource,
      getSavedPermissionMode,
      onPermissionModeSelected,
      getSavedFastMode,
      onFastModeSelected,
      onFirstPromptSent,
      onPromptAdmitted,
      maxCachedRuntimes: _maxCachedRuntimes,
      workspaceSettings,
      ...base
    } = this.opts;
    return {
      ...base,
      sessionId: ref.sessionId,
      projectPath: ref.projectPath,
      expectedSessionMode: ref.mode,
      resumeTranscriptPath: this.sessionTranscriptPaths.get(ref.sessionId),
      envelopeEvents: true,
      launchConfig: async () => {
        const config = await launchConfig(ref, this.sessionModelHints.get(ref.sessionId));
        const initialProjectRoot = this.sessionProjectRoots.get(ref.sessionId) ?? config.initialProjectRoot ?? ref.projectPath;
        this.sessionProjectRoots.set(ref.sessionId, initialProjectRoot);
        const settings = await workspaceSettings?.(ref.projectPath, ref);
        const environment = settings ? {
          ...this.sessionSettingsEnvironment.get(ref.sessionId),
          ...settings.environment,
        } : config.settingsEnvironment;
        const owner = this.sessionConfigOwners.get(ref.sessionId) ?? (settings ? {
          configHome: settings.configHome, globalConfigPath: settings.globalConfigPath,
        } : undefined);
        if (owner) this.sessionConfigOwners.set(ref.sessionId, owner);
        if (environment) this.sessionSettingsEnvironment.set(ref.sessionId, { ...environment });
        return { ...config, ...owner, settingsEnvironment: environment, initialProjectRoot,
          additionalDirectories: this.sessionDirectoryGrants.get(ref.sessionId) ?? settings?.additionalDirectories,
          promptDirectories: this.sessionPromptDirectories.get(ref.sessionId) };
      },
      ...(onModelSelected ? { onModelSelected: (model: string) => onModelSelected(ref, model) } : {}),
      ...(getSavedModel ? { getSavedModel: () => getSavedModel(ref) } : {}),
      ...(getSavedPermissionMode ? { getSavedPermissionMode: () => getSavedPermissionMode(ref) } : {}),
      ...(onPermissionModeSelected ? { onPermissionModeSelected: (mode: PermissionModeId) => onPermissionModeSelected(ref, mode) } : {}),
      ...(getSavedFastMode ? { getSavedFastMode: () => getSavedFastMode(ref) } : {}),
      ...(onFastModeSelected ? { onFastModeSelected: (enabled: boolean) => onFastModeSelected(ref, enabled) } : {}),
      ...(accessState ? { accessState: () => accessState(ref) } : {}),
      onModelChanged: (model: string) => {
        this.sessionModelHints.set(ref.sessionId, model);
        onModelChanged?.(ref, model);
      },
      onActivityChanged: () => {
        this.scheduleCacheTrim(ref.sessionId);
        if (this.sourceCaptureWanted.has(ref.sessionId)) this.captureSessionSource(ref);
      },
      onDurableSessionActivity: () => this.wantSessionSource(ref, true),
      onPromptAdmitted: () => onPromptAdmitted?.(ref),
      onFirstPromptSent: () => {
        this.wantSessionSource(ref, false);
        if (!onFirstPromptSent) {
          this.clearDraftSession(ref);
          return true;
        }
        try {
          onFirstPromptSent(ref, this.activeSessionId === ref.sessionId);
          this.clearDraftSession(ref);
          return true;
        } catch (error) {
          base.diagnostics?.add('error', 'bridge', `failed to commit draft session ${ref.sessionId}: ${sanitizeDiagnostic(error)}`);
          return false;
        }
      },
    };
  }

  private wantSessionSource(ref: SessionRef, scheduled: boolean): void {
    if (!this.opts.captureSessionSource) return;
    try {
      const previous = this.sourceCaptureWanted.get(ref.sessionId);
      this.sourceCaptureWanted.set(ref.sessionId, { ref, scheduled: scheduled || previous?.scheduled === true });
      this.captureSessionSource(ref);
    } catch (error) {
      this.opts.diagnostics?.add('warn', 'bridge', `failed to schedule session source recording ${ref.sessionId}: ${sanitizeDiagnostic(error)}`);
    }
  }

  /** Metadata work must never reject an already-admitted prompt. */
  private captureSessionSource(ref: SessionRef): void {
    const wanted = this.sourceCaptureWanted.get(ref.sessionId), runtime = this.runtimes.get(ref.sessionId);
    if (!wanted || !runtime || !this.opts.captureSessionSource || this.sourceCapturePending.has(ref.sessionId)) return;
    const release = this.retainBackgroundSession(ref);
    const operation = (async () => {
      let failure: unknown;
      for (const delay of [0, 150, 500, 1500]) {
        if (delay) await new Promise<void>((resolve) => setTimeout(resolve, delay));
        if (this.runtimes.get(ref.sessionId) !== runtime) return;
        try {
          const path = await this.opts.captureSessionSource!(wanted.ref, runtime, wanted.scheduled);
          if (this.runtimes.get(ref.sessionId) !== runtime) return;
          if (path) {
            this.sessionTranscriptPaths.set(ref.sessionId, path);
            runtime.rememberTranscriptSelection(path);
          }
          this.sourceCaptureWanted.delete(ref.sessionId);
          return;
        } catch (error) { failure = error; }
      }
      this.opts.diagnostics?.add('warn', 'bridge', `failed to record session source ${ref.sessionId}: ${sanitizeDiagnostic(failure)}`);
    })().finally(() => {
      if (this.sourceCapturePending.get(ref.sessionId) === operation) this.sourceCapturePending.delete(ref.sessionId);
      release();
    });
    this.sourceCapturePending.set(ref.sessionId, operation);
  }

  private clearDraftSession(ref: SessionRef): void {
    const key = this.draftKey(ref);
    if (key !== undefined) this.draftSessions.delete(key);
  }

  private async waitForRuntimeReadiness(runtime: SessionRuntime, event: IpcMainInvokeEvent, sessionId: unknown): Promise<void> {
    const openingDraft = () => {
      const key = this.draftKey(runtime);
      return key === undefined ? undefined : this.openingDraftSessions.get(key);
    };
    do {
      const opening = this.openingSessions.get(runtime.sessionId)?.promise;
      const restarting = this.restartingSessions.get(runtime.sessionId) ?? [];
      await Promise.all([openingDraft(), opening, ...restarting, runtime.waitForLifecycle()]);
      this.assertSender(event);
      if (this.requireById(sessionId) !== runtime) throw new Error('session runtime was replaced');
    } while (openingDraft() !== undefined || this.openingSessions.get(runtime.sessionId)?.promise !== undefined
      || this.restartingSessions.has(runtime.sessionId));
  }

  private requireById(value: unknown): SessionRuntime {
    if (!isSessionId(value)) throw new Error('invalid session id');
    const runtime = this.runtimes.get(value);
    if (!runtime) throw new Error(`session runtime is not open: ${value}`);
    this.touch(runtime.sessionId);
    return runtime;
  }

  private assertProjectNotClosing(projectPath: string): void {
    if (this.closingProjects.has(projectPath)) throw new Error('project is closing');
  }

  private detachWindow(webContents: WebContents): void {
    this.targets.delete(webContents);
    const handler = this.targetDestroyedHandlers.get(webContents);
    if (handler) {
      webContents.removeListener('destroyed', handler);
      this.targetDestroyedHandlers.delete(webContents);
    }
    for (const runtime of this.runtimes.values()) runtime.unregisterWindow(webContents);
  }

  private assertSender(event: IpcMainInvokeEvent): void {
    const rendererUrls = this.targets.get(event.sender);
    const senderFrame = event.senderFrame;
    if (!rendererUrls || !senderFrame || senderFrame !== event.sender.mainFrame) throw new Error('unauthorized IPC sender');
    const senderOrigin = urlOrigin(senderFrame.url);
    if (!senderOrigin || ![...rendererUrls].some((rendererUrl) => urlOrigin(rendererUrl) === senderOrigin)) {
      throw new Error('unauthorized IPC origin');
    }
  }

  private unregisterIpc(): void {
    if (!this.registered) return;
    for (const channel of [
      CH_EVENT_REPLAY,
      CH_SEND_PROMPT, CH_APPROVE, CH_DENY, CH_APPROVE_COMPUTER_ACCESS, CH_DENY_COMPUTER_ACCESS,
      CH_ANSWER_ASK_USER_QUESTION, CH_CANCEL_ASK_USER_QUESTION, CH_CANCEL, CH_COMMAND, CH_CONNECTION_STATE,
      CH_MOD_UI_CONTROL, CH_MOD_UI_OPERATION,
      CH_VISUALIZATION_MOUNT, CH_VISUALIZATION_WRITE_STATE, CH_VISUALIZATION_UNMOUNT,
    ]) ipcMain.removeHandler(channel);
    this.registered = false;
  }
}
