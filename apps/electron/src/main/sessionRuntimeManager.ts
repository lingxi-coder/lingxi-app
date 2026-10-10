import { assertSessionRef, isSessionId } from './sessionIdentity.js';
import type { ClientEvent } from '@lingxi/bridge-client';
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



/**
 * Owns all Electron bridge processes. A runtime is keyed by the engine's
 * stable session UUID, never by a display name or the currently selected
 * project. Project selection is therefore a renderer/navigation concern and
 * cannot accidentally restart another session.
 */
export class SessionRuntimeManager {
  private readonly runtimes = new Map<string, SessionRuntime>();
  private readonly openingSessions = new Map<string, { projectPath: string; promise: Promise<SessionRuntime>; activate: boolean }>();
  private readonly backgroundSessionLeases = new Map<string, number>();
  private readonly draftSessions = new Map<string, SessionRef>();
  private readonly openingDraftSessions = new Map<string, Promise<SessionRef>>();
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly targetDestroyedHandlers = new Map<WebContents, () => void>();
  private readonly lastUsed = new Map<string, number>();
  private readonly sessionModelHints = new Map<string, string>();
  private readonly pendingEvictions = new Set<Promise<void>>();
  private readonly maxCachedRuntimes: number;
  private activeSessionId: string | null = null;
  private accessSequence = 0;
  private cacheTrimScheduled = false;
  private registered = false;
  private oauthOwner: string | undefined;
  private oauthLifecycle: Promise<void> = Promise.resolve();

  /** Routes visualization documents and state writes to the session that issued the mount. */
  readonly visualizations = new VisualizationRouter({
    get: (sessionId) => {
      const runtime = this.runtimes.get(sessionId);
      return runtime?.visualizationReady ? runtime : undefined;
    },
    any: () => [...this.runtimes.values()].find((runtime) => runtime.visualizationReady),
  });

  constructor(private readonly opts: SessionRuntimeManagerOptions) {
    const requestedLimit = opts.maxCachedRuntimes ?? DEFAULT_MAX_CACHED_RUNTIMES;
    if (!Number.isInteger(requestedLimit) || requestedLimit < 1 || requestedLimit > MAX_CONFIGURED_CACHED_RUNTIMES) {
      throw new Error(`maxCachedRuntimes must be between 1 and ${MAX_CONFIGURED_CACHED_RUNTIMES}`);
    }
    this.maxCachedRuntimes = requestedLimit;
  }

  get size(): number {
    return this.runtimes.size;
  }

  get runtimeSummaries(): SessionRuntimeSummary[] {
    return [...this.runtimes.values()].map((runtime) => runtime.summary);
  }

  get replaySnapshots(): readonly SequencedRuntimeEventEnvelope<ClientEvent>[] {
    return Object.freeze([...this.runtimes.values()].flatMap((runtime) => runtime.replaySnapshot()));
  }

  invalidateLaunchConfigCache(): void {
    this.opts.invalidateLaunchConfigCache?.();
  }

  get(sessionId: string): SessionRuntime | undefined {
    return this.runtimes.get(sessionId);
  }

  private touch(sessionId: string): void {
    if (this.runtimes.has(sessionId)) this.lastUsed.set(sessionId, ++this.accessSequence);
  }

  private activate(runtime: SessionRuntime): void {
    this.activeSessionId = runtime.sessionId;
    this.touch(runtime.sessionId);
    this.trimCache();
  }

  private runtimeIsPinned(runtime: SessionRuntime): boolean {
    const status = runtime.connectionState.status;
    return runtime.sessionId === this.activeSessionId
      || runtime.turnActive
      || runtime.hasActiveAgents
      || runtime.pendingInteractions > 0
      || runtime.cronOperationPending
      || status === 'spawning'
      || status === 'restarting'
      || status === 'connecting'
      || this.openingSessions.has(runtime.sessionId)
      || this.backgroundSessionLeases.has(runtime.sessionId);
  }

  private trimCache(): void {
    while (this.runtimes.size > this.maxCachedRuntimes) {
      const victim = [...this.runtimes.values()]
        .filter((runtime) => !this.runtimeIsPinned(runtime))
        .sort((left, right) => (this.lastUsed.get(left.sessionId) ?? 0) - (this.lastUsed.get(right.sessionId) ?? 0))[0];
      if (!victim) return;
      this.runtimes.delete(victim.sessionId);
      this.visualizations.forgetSession(victim.sessionId);
      this.lastUsed.delete(victim.sessionId);
      this.sessionModelHints.delete(victim.sessionId);
      const eviction = victim.dispose()
        .catch((error) => this.opts.diagnostics?.add('warn', 'host', `session cache eviction failed: ${sanitizeDiagnostic(error)}`))
        .finally(() => this.pendingEvictions.delete(eviction));
      this.pendingEvictions.add(eviction);
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
  async ensure(ref: SessionRef, start = true, resumeModel?: string): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (existing) {
      if (existing.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      if (start) await existing.start();
      this.touch(existing.sessionId);
      return existing;
    }

    if (resumeModel) this.sessionModelHints.set(ref.sessionId, resumeModel);
    const runtimeOptions = this.runtimeOptions(ref);
    const runtime = new SessionRuntime(runtimeOptions);
    this.runtimes.set(ref.sessionId, runtime);
    this.touch(ref.sessionId);
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
      this.visualizations.forgetSession(ref.sessionId);
      this.lastUsed.delete(ref.sessionId);
      this.sessionModelHints.delete(ref.sessionId);
      await runtime.dispose().catch(() => undefined);
      throw error;
    }
  }

  openSession(ref: SessionRef, empty = false, resumeModel?: string, activate = true): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (existing && existing.projectPath !== ref.projectPath) {
      throw new Error('session id is owned by a different project');
    }
    const pending = this.openingSessions.get(ref.sessionId);
    if (pending) {
      if (pending.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      pending.activate ||= activate;
      return pending.promise;
    }
    if (!existing && resumeModel) this.sessionModelHints.set(ref.sessionId, resumeModel);
    if (existing?.connectionState.status === 'connected' && !existing.isStarting) {
      if (activate) this.activate(existing);
      return Promise.resolve(existing);
    }

    const promise = this.openSessionInternal(ref, existing, empty).then((runtime) => {
      if (this.openingSessions.get(ref.sessionId)?.activate) this.activate(runtime);
      return runtime;
    });
    const trackedPromise = promise.finally(() => {
      if (this.openingSessions.get(ref.sessionId)?.promise === trackedPromise) this.openingSessions.delete(ref.sessionId);
    });
    this.openingSessions.set(ref.sessionId, { projectPath: ref.projectPath, promise: trackedPromise, activate });
    return trackedPromise;
  }

  private async openSessionInternal(ref: SessionRef, existing: SessionRuntime | undefined, empty: boolean): Promise<SessionRuntime> {
    let runtime: SessionRuntime | undefined;
    try {
      if (existing) {
        // A disconnected transport does not authorize replacing its living
        // session process, and its old foreground latch cannot gate recovery.
        await existing.start();
        runtime = existing;
      } else {
        runtime = await this.ensure(ref, true);
      }
      if (!empty && !runtime.recoveredLiveConnection) await runtime.resumeOwnedSession();
      return runtime;
    } catch (error) {
      if (!existing && runtime) {
        this.runtimes.delete(ref.sessionId);
        this.visualizations.forgetSession(ref.sessionId);
        this.lastUsed.delete(ref.sessionId);
        this.sessionModelHints.delete(ref.sessionId);
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

  /** Keep an inactive runtime alive for the whole host operation, then enforce the cache bound. */
  async withBackgroundSession<T>(
    ref: SessionRef,
    empty: boolean,
    resumeModel: string | undefined,
    operation: (runtime: SessionRuntime) => Promise<T>,
  ): Promise<T> {
    assertSessionRef(ref);
    this.backgroundSessionLeases.set(ref.sessionId, (this.backgroundSessionLeases.get(ref.sessionId) ?? 0) + 1);
    try {
      return await operation(await this.openSession(ref, empty, resumeModel, false));
    } finally {
      const count = (this.backgroundSessionLeases.get(ref.sessionId) ?? 1) - 1;
      if (count) this.backgroundSessionLeases.set(ref.sessionId, count);
      else this.backgroundSessionLeases.delete(ref.sessionId);
      this.trimCache();
    }
  }

  async newSession(projectPath: string, model?: string): Promise<SessionRef> {
    if (typeof projectPath !== 'string' || projectPath.length === 0) throw new Error('invalid project path');
    this.assertProjectNotClosing(projectPath);
    const pendingDraft = this.openingDraftSessions.get(projectPath);
    if (pendingDraft) return pendingDraft;
    const draft = this.draftSessions.get(projectPath);
    if (draft) {
      const runtime = await this.ensure(draft, true, model);
      this.activate(runtime);
      return { ...draft };
    }
    const promise = this.allocateDraftSession(projectPath, model);
    const trackedPromise = promise.finally(() => {
      if (this.openingDraftSessions.get(projectPath) === trackedPromise) this.openingDraftSessions.delete(projectPath);
    });
    this.openingDraftSessions.set(projectPath, trackedPromise);
    return trackedPromise;
  }

  private async allocateDraftSession(projectPath: string, model?: string): Promise<SessionRef> {
    let ref: SessionRef | undefined;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const candidate = { projectPath, sessionId: randomUUID() } satisfies SessionRef;
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
    this.draftSessions.set(projectPath, ref);
    try {
      const runtime = await this.ensure(ref, true, model);
      this.activate(runtime);
    } catch (error) {
      this.clearDraftSession(ref);
      throw error;
    }
    // The boot argument is the source of truth for a new session. Sending a
    // second `new_session` command would create/switch the engine to another
    // UUID and leave the runtime key pointing at the wrong transcript.
    return { ...ref };
  }

  async restart(ref: SessionRef, beforeRestart?: () => void): Promise<void> {
    const runtime = this.require(ref);
    if (runtime.connectionState.status !== 'connected' || runtime.isStarting) {
      beforeRestart?.();
      await runtime.start();
      beforeRestart?.();
      if (!runtime.recoveredLiveConnection) await runtime.restoreOwnedSessionIfNeeded();
      return;
    }
    await runtime.restart(() => {
      if (runtime.hasActiveWork) throw new Error('Wait for active work and pending interactions before restarting this chat.');
      beforeRestart?.();
    });
    await runtime.restoreOwnedSessionIfNeeded();
  }

  async closeSession(ref: SessionRef): Promise<void> {
    const runtime = this.require(ref);
    if (runtime.hasActiveWork) throw new Error('Wait for active work and pending interactions before closing this chat.');
    // Remove from the routable map BEFORE the first await, for the same reason
    // `closeProject` does: while `dispose()` is in flight `get()` would still
    // hand this runtime out, and `restart()` now rejects on a disposed runtime
    // instead of resolving silently — surfacing a failure for a settings or
    // credential write that actually succeeded.
    this.runtimes.delete(ref.sessionId);
    this.visualizations.forgetSession(ref.sessionId);
    this.lastUsed.delete(ref.sessionId);
    this.sessionModelHints.delete(ref.sessionId);
    if (this.activeSessionId === ref.sessionId) this.activeSessionId = null;
    this.clearDraftSession(ref);
    await runtime.dispose();
  }

  async closeProject(projectPath: string): Promise<void> {
    this.assertProjectNotClosing(projectPath);
    if (this.hasActiveWork(projectPath)) throw new Error('cancel active turns and pending interactions before removing a project');
    this.closingProjects.add(projectPath);
    this.draftSessions.delete(projectPath);
    this.openingDraftSessions.delete(projectPath);
    const projectRuntimes = [...this.runtimes.values()].filter((runtime) => runtime.projectPath === projectPath);
    // Remove runtimes from the routable map before the first await. A prompt
    // arriving while disposal is in progress must fail instead of entering a
    // runtime whose child is already being torn down.
    for (const runtime of projectRuntimes) {
      this.runtimes.delete(runtime.sessionId);
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

  private async stopOtherCodexRuntimes(sessionId?: string): Promise<void> {
    const owner = this.oauthOwner ? this.runtimes.get(this.oauthOwner) : undefined;
    const candidates = [...this.runtimes.values()].filter(runtime => runtime.sessionId !== sessionId
      && (runtime.hasOpenAiOAuth || runtime === owner));
    if (candidates.some(runtime => runtime.isStarting || runtime.turnActive || runtime.hasActiveAgents || runtime.pendingInteractions > 0
      || !['connected', 'idle', 'error', 'disconnected'].includes(runtime.connectionState.status))) {
      throw new Error('Wait for the other Codex chat to finish before changing Codex authentication.');
    }
    for (const runtime of candidates) {
      if (runtime.connectionState.status === 'connected') await runtime.assertNoBackgroundTasks();
      else if (runtime.hasOpenAiOAuth && runtime.connectionState.status !== 'idle') {
        throw new Error('Reconnect or close the other Codex chat before changing Codex authentication; its background work cannot be checked.');
      }
    }
    if (candidates.some(runtime => runtime.turnActive || runtime.hasActiveAgents || runtime.pendingInteractions > 0)) {
      throw new Error('Wait for the other Codex chat to finish before changing Codex authentication.');
    }
    for (const runtime of candidates) await runtime.stop();
    this.oauthOwner = sessionId;
  }

  private claimCodexRuntime(sessionId?: string): Promise<void> {
    const operation = this.oauthLifecycle.catch(() => undefined).then(() => this.stopOtherCodexRuntimes(sessionId));
    this.oauthLifecycle = operation;
    return operation;
  }

  /** Serialize credential mutation with launch ownership, including broker I/O. */
  withCodexAuthMutation<T>(mutation: () => Promise<T>): Promise<T> {
    const operation = this.oauthLifecycle.catch(() => undefined).then(async () => {
      await this.stopOtherCodexRuntimes();
      return mutation();
    });
    this.oauthLifecycle = operation.then(() => undefined, () => undefined);
    return operation;
  }

  async invalidateCodexRuntimes(): Promise<void> {
    await this.withCodexAuthMutation(async () => undefined);
  }

  async refreshCachedProviderCredential(providerId: string, credential: string): Promise<void> {
    const runtimes = [...this.runtimes.values()]
      .filter((runtime) => (
        runtime.connectionState.status === 'connected'
        && runtime.hasCachedProviderCredential(providerId)
      ));
    await Promise.all(runtimes.map((runtime) => runtime.cacheProviderCredential(providerId, credential)));
  }

  async clearCachedProviderCredential(providerId: string): Promise<void> {
    const runtimes = [...this.runtimes.values()]
      .filter((runtime) => (
        runtime.connectionState.status === 'connected'
        && runtime.hasCachedProviderCredential(providerId)
      ));
    await Promise.all(runtimes.map((runtime) => runtime.clearCachedProviderCredential(providerId)));
  }

  async dispose(): Promise<void> {
    const runtimes = [...this.runtimes.values()];
    this.runtimes.clear();
    this.lastUsed.clear();
    this.sessionModelHints.clear();
    this.activeSessionId = null;
    this.openingSessions.clear();
    this.draftSessions.clear();
    this.openingDraftSessions.clear();
    await Promise.all(runtimes.map((runtime) => runtime.dispose().catch(() => undefined)));
    await Promise.all([...this.pendingEvictions]);
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
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, sessionId: unknown, text: unknown, images: unknown, turnId?: unknown, visualizationContext?: unknown) => {
      this.assertSender(event);
      return this.requireById(sessionId).sendPrompt(text, images, turnId, visualizationContext);
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
      await this.requireById(sessionId).dispatchCommand(command);
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
      onFirstPromptSent,
      maxCachedRuntimes: _maxCachedRuntimes,
      ...base
    } = this.opts;
    return {
      ...base,
      sessionId: ref.sessionId,
      projectPath: ref.projectPath,
      envelopeEvents: true,
      registerIpc: false,
      launchConfig: () => launchConfig(ref, this.sessionModelHints.get(ref.sessionId)),
      ...(onModelSelected ? { onModelSelected: (model: string) => onModelSelected(ref, model) } : {}),
      ...(getSavedModel ? { getSavedModel: () => getSavedModel(ref) } : {}),
      beforeOpenAiOAuthLaunch: () => this.claimCodexRuntime(ref.sessionId),
      ...(accessState ? { accessState: () => accessState(ref) } : {}),
      onModelChanged: (model: string) => {
        this.sessionModelHints.set(ref.sessionId, model);
        onModelChanged?.(ref, model);
      },
      onActivityChanged: () => this.scheduleCacheTrim(ref.sessionId),
      onFirstPromptSent: () => {
        if (!onFirstPromptSent) {
          this.clearDraftSession(ref);
          return true;
        }
        try {
          if (this.activeSessionId === ref.sessionId) onFirstPromptSent(ref);
          this.clearDraftSession(ref);
          return true;
        } catch (error) {
          base.diagnostics?.add('error', 'bridge', `failed to commit draft session ${ref.sessionId}: ${sanitizeDiagnostic(error)}`);
          return false;
        }
      },
    };
  }

  private clearDraftSession(ref: SessionRef): void {
    const current = this.draftSessions.get(ref.projectPath);
    if (current?.sessionId === ref.sessionId) this.draftSessions.delete(ref.projectPath);
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
