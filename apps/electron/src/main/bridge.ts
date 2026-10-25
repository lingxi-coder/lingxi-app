import { assertComputerAccessSubset } from './computerAccessApproval.js';
import {
  BridgeClient,
  SessionEventCorrelator,
  buildNativeUiControlCommand,
  parseUiFrameEvent,
  parseUiInvalidateEvent,
  validateNativeUiControlResponseJson,
  validateUiClientOperationResponseJson,
  validateUiControlMetadataJson,
} from '@lingxi/bridge-client';
import type {
  AskUserQuestionRequestDto,
  AudioOperationRequestDto,
  AudioOperationResultDto,
  AudioOwnerDto,
  ClientCommand,
  ClientEvent,
  ComputerAccessRequestDto,
  CronJobDto,
  CronRequestDto,
  FusionStrategyRefDto,
  FusionStrategySelectionDto,
  ImageRefDto,
  PermissionModeId,
  PermissionRequest,
  NativeUiControlResponse,
  UiClientOperationResponse,
  UiControlCallResultDto,
  VisualizationMountDto,
  VisualizationServeDto,
  VisualizationStateWriteDto,
  VisualizationThemeDto,
  SessionWorkspaceState,
  SetWorkingDirectoryRequest,
  SetWorkingDirectoryResult,
} from '@lingxi/bridge-client';
import type { WebContents } from 'electron';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { fusionInvocation, pinFusionStrategy } from '../shared/fusionStrategy.js';
import { REMOTE_STATE_EVENT_TYPES } from '../shared/clientCommands.js';
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import {
  discoverExternalReusableBridge,
  discoverReusableBridge,
  ignorableLockfileError,
  listProcessCommands,
  lockfiles,
  processIsAlive,
  readProcessCommand,
  resolveServerBin,
} from './bridgeDiscovery.js';
import {
  SESSION_RUNTIME_DISPOSED_REASON,
  TRANSCRIPT_REPLAY_BASE_EVENTS,
  TRANSCRIPT_REPLAY_EVENTS,
  audioIdentityKey,
  audioOwnerKey,
  bridgeVersionDiagnostic,
  childExitDiagnostic,
  connectionDiagnostic,
  isTurnOwnedEvent,
  slashRequestEchoes,
  urlOrigin,
} from './bridgeEvents.js';
import {
  CH_COMPUTER_ACCESS,
  CH_EVENT,
  CH_MOD_UI_FRAME,
  CH_MOD_UI_INVALIDATE,
  CH_PERMISSION,
  CH_STATE_CHANGED,
} from './bridgeIpc.js';
import type {
  BridgeLaunchConfig,
  BridgeManagerOptions,
  BridgeRuntimeVersions,
  ConnectionState,
  HostPermissionRequest,
  InteractionRecord,
  PromptJournalEntry,
  PromptSource,
  ProviderConnectionTestResult,
  RemoteObservation,
  RemoteObserver,
  RemoteRuntimeView,
  SequencedRuntimeEventEnvelope,
  SessionRef,
  SessionRuntimeSummary,
} from './bridgeTypes.js';
import {
  configuredCredentialProviderIds,
  fusionCredentialSelection,
  resolveFusionCredentialProviderIds,
  resolveModelCredentialProviderIds,
  resolveProviderIdForModel,
} from './credential-broker.js';
import { GitActivityTracker } from './git-activity.js';
import { scheduledRunIdentity } from './scheduled-run-identity.js';
import {
  DiagnosticBuffer,
  buildBridgeArguments,
  buildBridgeEnvironment,
  buildCredentialEnvelope,
  diagnosticEvent,
  resolveModBunExecutable,
  sanitizeDiagnostic,
} from './host-utils.js';
import type { OpenAiOAuthSession } from './host-utils.js';
import { CODEX_PROVIDER_ID } from './codex-auth.js';
import { codexAccessToken } from './codex-tokens.js';
import { isSessionId } from './sessionIdentity.js';
import { parseVisualizationReference, type VisualizationReference } from '../shared/visualization.js';
import {
  assertCommandAllowedDuringTurn,
  validateAskUserQuestionAnswers,
  validateBridgeLockfile,
  validateClientCommand,
  validateModUiControlRequest,
  validateModUiOperation,
  validateComputerAccessResponse,
  validateImageRefs,
  validateOptionalTurnId,
  validatePermissionResponse,
  validatePrompt,
  validateRequestId,
} from './validation.js';

type ProviderCredentialStatus = Extract<ClientEvent, { type: 'provider_credential_status' }>;

const CODEX_SIGN_IN_REQUIRED = 'Codex authentication is unavailable. Sign in again.';

/**
 * `operation_id` of a credential status the engine sends unasked: its Codex
 * access token expired, and it waits for the next one. Requests are numbered
 * from 1.
 */
const CODEX_TOKEN_REQUEST_OPERATION_ID = 0;

function isCodexModel(model: string | undefined): boolean {
  return model?.startsWith(`${CODEX_PROVIDER_ID}/`) === true;
}

interface PendingCredentialOperation {
  providerIds: readonly string[];
  resolve: (status: ProviderCredentialStatus) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface PendingProviderConnectionTest {
  providerId: string;
  resolve: (result: ProviderConnectionTestResult) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface PendingAskUserQuestionRequest {
  request: AskUserQuestionRequestDto;
}

interface PendingSessionResume {
  sessionId: string;
  generation: number;
  client: BridgeClient;
  sessionResumed: boolean;
  responseReceived: boolean;
  controller: AbortController;
  activeTools: Set<string>;
  model?: string;
  hydrationStarted: boolean;
  resolve: () => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

const MAX_PENDING_PERMISSIONS = 1_000;

const MAX_PENDING_COMPUTER_ACCESS = 1_000;

const MAX_PENDING_ASK_USER_QUESTION = 1_000;

const MAX_TRACKED_AUDIO_REQUESTS_PER_RUNTIME = 256;

// Remote Control's memory of what the engine's event stream does not hold.
const MAX_PROMPT_JOURNAL = 500;
const MAX_JOURNAL_ENTRIES_WITH_IMAGES = 20;
const MAX_INTERACTION_RECORDS = 200;

const REMOTE_STATE_EVENTS: ReadonlySet<string> = new Set(REMOTE_STATE_EVENT_TYPES);

// Mod UI requests share the runtime's ordinary command lane, and a transcript
// mounts a render site per tool row, so opening a long chat asks for well over
// a hundred renders at once. The Bridge closes a connection with 64 commands
// waiting behind one slow command, so only this many are outstanding at once.
const MAX_OUTSTANDING_MOD_UI_REQUESTS = 16;

export class SessionRuntime {
  readonly sessionId: string;
  readonly projectPath: string;
  private child: ChildProcess | null = null;
  private client: BridgeClient | null = null;
  private state: ConnectionState = { status: 'idle' };
  private disposed = false;
  private restartChain: Promise<void> = Promise.resolve();
  private generation = 0;
  private launchDir: string | null = null;
  private connectionLockfilePath: string | null = null;
  private adoptedPid: number | null = null;
  private adoptedProcessOwned = false;
  private activeWorkspace: string | undefined;
  private ownedTranscriptPath: string | undefined;
  private remountControls: string | undefined;
  private activeWorkspaceTrusted = false;
  private runtimeCredentialProviders = new Set<string>();
  private persistedCredentialProviders = new Set<string>();
  private activeCredentialProviders = new Set<string>();
  private credentialPreviews = new Map<string, string>();
  private credentialStorageEncrypted = false;
  private nextCredentialOperationId = 1;
  private readonly pendingCredentialOperations = new Map<number, PendingCredentialOperation>();
  private readonly pendingRuntimeCredentialLoads = new Map<string, Promise<void>>();
  private readonly providerCredentialEpochs = new Map<string, number>();
  private readonly providerCredentialWrites = new Map<string, Promise<void>>();
  private readonly invalidatedProviderCredentials = new Set<string>();
  private readonly pendingProviderConnectionTests = new Map<number, PendingProviderConnectionTest>();
  private readonly pendingModUiRequests = new SessionEventCorrelator<Extract<ClientEvent, { type: 'ui_control_result' }>>();
  private outstandingModUiRequests = 0;
  private readonly queuedModUiRequests: Array<{ send(): void; reject(error: Error): void }> = [];
  private archiving = false;
  private changingWorkspace = false;
  private activeCronExecutions = 0;
  private readonly pendingCron = new Map<string, { resolve: (jobs: CronJobDto[]) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout>; creating: boolean }>();
  private readonly pendingScheduledTurns = new Map<string, { resolve: (summary: string) => void; reject: (error: Error) => void }>();
  private activeTurn = false;
  private computerAccessInTurn = false;
  private activeTurnGeneration: number | undefined;
  private disconnectedForegroundPending = false;
  private liveConnectionRecovered = false;
  private lastKnownCost: Extract<ClientEvent, { type: 'turn_ended' }>['cost'] | undefined;
  /** The Codex access token this engine was last given. */
  private servedCodexToken: string | undefined;
  private oauthPersistence: Promise<void> = Promise.resolve();
  private activeTurnId: number | undefined;
  private cancellingTurn = false;
  private lastRuntimeVersions: BridgeRuntimeVersions | undefined;
  /** Permission requests remain replayable until the engine reports a terminal resolution. */
  private readonly pendingPermissionIds = new Map<number, HostPermissionRequest>();
  private readonly backgroundPermissionIds = new Set<number>();
  private readonly permissionScopeChecks = new Map<number, PermissionRequest>();
  private foregroundInteractionEpoch = 0;
  /** Prevent a delayed duplicate permission frame from resurrecting a terminal request. */
  private readonly resolvedPermissionIds = new Set<number>();
  /** Computer-access requests wait here as whole objects, so that a viewer can be shown them. */
  private readonly pendingComputerAccessIds = new Map<number, ComputerAccessRequestDto>();
  private readonly pendingAskUserQuestionIds = new Set<number>();
  private readonly pendingAskUserQuestionRequests = new Map<number, PendingAskUserQuestionRequest>();
  private pendingSessionResume: PendingSessionResume | null = null;
  private resumeOperationInProgress = false;
  private sessionHasHistory = false;
  private ownedSessionResumeRequired = false;
  private sessionIdentityCommitted = false;
  private eventSequence = 0;
  /** Names this instance, so that a recreated runtime for the same session never reuses an epoch. */
  readonly runtimeId = randomBytes(6).toString('hex');
  private remoteBaseSequence = 0;
  private readonly remoteObservers = new Set<RemoteObserver>();
  private promptJournal: PromptJournalEntry[] = [];
  private interactionRecords: InteractionRecord[] = [];
  /** The newest sequence the bounded journal and interaction lists have dropped. */
  private droppedThroughSequence = 0;
  private readonly latestState = new Map<string, SequencedRuntimeEventEnvelope<ClientEvent>>();
  private remotePendingNotified = false;
  private credentialRoutingSettings: unknown = undefined;
  private pendingModelSwitch: { model: string; sent: boolean; slash: boolean; turnId?: number; promise: Promise<void>; complete(selected?: string): void; fail(error: Error): void } | undefined;

  private pendingPermissionSwitch: {
    mode: PermissionModeId;
    complete(): void;
    fail(error: Error): void;
  } | undefined;

  private pendingFastModeSwitch: {
    enabled: boolean;
    complete(): void;
    fail(error: Error): void;
  } | undefined;

  private applyPermissionMode(mode: PermissionModeId, persist: boolean): Promise<void> {
    if (this.pendingPermissionSwitch) return Promise.reject(new Error('A permission mode change is already in progress.'));
    const client = this.requireClient();
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => pending.fail(new Error('Permission mode change timed out.')), 10_000);
      timer.unref();
      const finish = (error?: Error) => {
        if (this.pendingPermissionSwitch !== pending) return;
        this.pendingPermissionSwitch = undefined;
        clearTimeout(timer);
        if (error) reject(error); else resolve();
      };
      const pending = {
        mode,
        complete: () => {
          try {
            if (persist) this.opts.onPermissionModeSelected?.(mode);
            finish();
          } catch (error) {
            finish(new Error(`Could not save permission mode: ${sanitizeDiagnostic(error)}`));
          }
        },
        fail: (error: Error) => finish(error),
      };
      this.pendingPermissionSwitch = pending;
      try { client.sendCommand({ type: 'set_permission_mode', mode }); }
      catch (error) { pending.fail(error instanceof Error ? error : new Error(String(error))); }
    });
  }

  private async restorePermissionMode(): Promise<void> {
    const mode = this.opts.getSavedPermissionMode?.();
    if (!mode) return;
    try {
      if (mode === 'bypassPermissions' && !(await this.opts.confirmBypassPermissions?.())) {
        throw new Error('Saved Bypass Permissions mode was not accepted.');
      }
      await this.applyPermissionMode(mode, false);
    } catch (error) {
      // A changed policy/provider may reject a previously valid preference.
      // Keep the engine usable so the user can select another mode.
      const message = `Could not restore permission mode: ${sanitizeDiagnostic(error)}`;
      this.diagnostics.add('warn', 'host', message);
      this.broadcastClientEvent({ type: 'error', kind: { type: 'rejected' }, message });
    }
  }

  private applyFastMode(enabled: boolean, persist: boolean): Promise<void> {
    if (this.pendingFastModeSwitch) return Promise.reject(new Error('A Fast mode change is already in progress.'));
    const client = this.requireClient();
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => pending.fail(new Error('Fast mode change timed out.')), 10_000);
      timer.unref();
      const finish = (error?: Error) => {
        if (this.pendingFastModeSwitch !== pending) return;
        this.pendingFastModeSwitch = undefined;
        clearTimeout(timer);
        if (error) reject(error); else resolve();
      };
      const pending = {
        enabled,
        complete: () => {
          try {
            if (persist) this.opts.onFastModeSelected?.(enabled);
            finish();
          } catch (error) {
            finish(new Error(`Could not save Fast mode: ${sanitizeDiagnostic(error)}`));
          }
        },
        fail: (error: Error) => finish(error),
      };
      this.pendingFastModeSwitch = pending;
      try { client.sendCommand({ type: 'set_fast_mode', enabled }); }
      catch (error) { pending.fail(error instanceof Error ? error : new Error(String(error))); }
    });
  }

  private async restoreFastMode(): Promise<void> {
    const enabled = this.opts.getSavedFastMode?.();
    if (enabled === undefined) return;
    try {
      await this.applyFastMode(enabled, false);
    } catch (error) {
      // A changed provider/model policy may make Fast mode unavailable. Keep
      // the session usable and retain the saved preference for a later model
      // that supports it.
      const message = `Could not restore Fast mode: ${sanitizeDiagnostic(error)}`;
      this.diagnostics.add('warn', 'host', message);
      this.broadcastClientEvent({ type: 'error', kind: { type: 'rejected' }, message });
    }
  }

  private async restoreModel(): Promise<void> {
    const model = this.opts.getSavedModel?.();
    if (!model || model === this.selectedModelReference) return;
    try {
      await this.switchModel(model, false);
    } catch (error) {
      const message = `Could not restore model: ${sanitizeDiagnostic(error)}`;
      this.diagnostics.add('warn', 'host', message);
      this.broadcastClientEvent({ type: 'error', kind: { type: 'rejected' }, message });
    }
  }

  private switchModel(model: string, persist = true, slashCommand?: Extract<ClientCommand, { type: 'run_slash_command' }>): Promise<void> {
    if (this.pendingModelSwitch) return Promise.reject(new Error('A model switch is already in progress.'));
    const generation = this.generation;
    const client = this.requireClient();
    let resolve!: () => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
    const finish = (error?: Error): void => {
      if (this.pendingModelSwitch !== pending) return;
      clearTimeout(timer);
      this.pendingModelSwitch = undefined;
      if (error) reject(error); else resolve();
      this.notifyActivityChanged();
    };
    const pending = {
      model, sent: false, slash: !!slashCommand, turnId: slashCommand?.turn_id, promise,
      complete: (selected = model) => {
        try {
          if (persist) this.opts.onModelSelected?.(selected);
          finish();
        } catch (error) {
          finish(new Error(`Could not save model: ${sanitizeDiagnostic(error)}`));
        }
      },
      fail: (error: Error) => finish(error),
    };
    const timer = setTimeout(() => finish(new Error('Model switch confirmation timed out.')), 10_000);
    this.pendingModelSwitch = pending;
    this.notifyActivityChanged();
    void (async () => {
      try {
        await this.ensureModelProviderCredential(model);
        if (this.pendingModelSwitch !== pending) return;
        if (generation !== this.generation || client !== this.client || this.archiving) {
          throw new Error('Model switch was interrupted.');
        }
        pending.sent = true;
        client.sendCommand(slashCommand ?? { type: 'set_model', model });
      } catch (error) {
        pending.fail(error instanceof Error ? error : new Error(String(error)));
      }
    })();
    return promise;
  }

  private readonly pendingPromptHydrations = new Map<symbol, number | undefined>();
  private verifiedRealtimeAudio = false;
  get realtimeAudioSupported(): boolean { return this.verifiedRealtimeAudio && this.connectionState.status === 'connected'; }
  private readonly realtimeAudioSubscribers = new Set<(eventJson: string) => void>();
  onRealtimeAudioEvent(callback: (eventJson: string) => void): () => void {
    this.realtimeAudioSubscribers.add(callback); return () => this.realtimeAudioSubscribers.delete(callback);
  }
  startRealtimeAudio(requestJson: string): void { this.requireClient().sendCommand({ type: 'start_realtime_audio', request_json: requestJson }); }
  realtimeAudioInput(inputJson: string): void { this.requireClient().sendCommand({ type: 'realtime_audio_input', input_json: inputJson }); }
  stopRealtimeAudio(): void { this.client?.sendCommand({ type: 'stop_realtime_audio' }); }

  private cachedAudioSessionContext?: { sessionId: string; profileId: string; accountScope: string };

  get audioSessionContext() { return this.cachedAudioSessionContext ? { ...this.cachedAudioSessionContext } : undefined; }

  private selectedModelReference: string | undefined;
  private pendingCredentialSettings: { promise: Promise<void>; resolve(): void; reject(error: Error): void } | undefined;

  private ensureCredentialSettings(): Promise<void> {
    if (this.credentialRoutingSettings !== undefined) return Promise.resolve();
    if (this.pendingCredentialSettings) return this.pendingCredentialSettings.promise;
    const client = this.requireClient();
    let resolve!: () => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
    const timer = setTimeout(() => reject(new Error('Provider settings loading timed out.')), 5_000);
    this.pendingCredentialSettings = { promise, resolve, reject };
    void promise.then(() => clearTimeout(timer), () => clearTimeout(timer)).finally(() => {
      if (this.pendingCredentialSettings?.promise === promise) this.pendingCredentialSettings = undefined;
    });
    try { client.sendCommand({ type: 'refresh_listings', which: [{ type: 'settings' }] }); }
    catch (error) { reject(error instanceof Error ? error : new Error(String(error))); }
    return promise;
  }

  private configuredCustomProviderIds: string[] = [];
  private readonly customProviderWaiters = new Map<string, Set<() => void>>();

  async ensureCustomProviderConfigured(providerId: string): Promise<void> {
    if (this.configuredCredentialIds.includes(providerId)) return;
    const client = this.requireClient();
    await new Promise<void>((resolve, reject) => {
      const waiters = this.customProviderWaiters.get(providerId) ?? new Set<() => void>();
      const finish = (): void => {
        clearTimeout(timer);
        waiters.delete(finish);
        if (waiters.size === 0) this.customProviderWaiters.delete(providerId);
        resolve();
      };
      const timer = setTimeout(() => {
        waiters.delete(finish);
        if (waiters.size === 0) this.customProviderWaiters.delete(providerId);
        reject(new Error('unsupported provider: settings did not confirm this profile'));
      }, 5_000);
      waiters.add(finish);
      this.customProviderWaiters.set(providerId, waiters);
      try {
        client.sendCommand({ type: 'refresh_listings', which: [{ type: 'settings' }] });
      } catch (error) {
        clearTimeout(timer);
        waiters.delete(finish);
        if (waiters.size === 0) this.customProviderWaiters.delete(providerId);
        reject(error);
      }
    });
  }


  get customProviderIds(): readonly string[] {
    return [...this.configuredCustomProviderIds];
  }

  get configuredCredentialIds(): readonly string[] {
    return configuredCredentialProviderIds(this.credentialRoutingSettings);
  }

  private replayEvents: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly diagnostics: DiagnosticBuffer;
  private startPromise: Promise<void> | null = null;
  private lastAudioCapabilities = '';
  private readonly unsubscribeAudioService?: () => void;
  private readonly audioRequestsByGeneration = new Map<number, Map<string, AudioOperationRequestDto>>();
  private readonly audioStartsByGeneration = new Map<number, Map<string, AudioOperationRequestDto>>();
  private readonly audioOwnersByGeneration = new Map<number, Map<string, AudioOwnerDto>>();
  private readonly audioCleanupGenerations = new Map<number, Promise<void>>();
  private readonly closedAudioGenerations = new Set<number>();

  constructor(private readonly opts: BridgeManagerOptions) {
    this.sessionId = opts.sessionId ?? randomUUID();
    this.ownedTranscriptPath = opts.resumeTranscriptPath;
    this.remountControls = opts.remountControls;
    this.projectPath = opts.projectPath ?? '';
    this.diagnostics = opts.diagnostics ?? new DiagnosticBuffer();
    if (opts.audioService) {
      this.lastAudioCapabilities = JSON.stringify(opts.audioService.getCapabilities());
      this.unsubscribeAudioService = opts.audioService.onEvent((event) => {
        if (event.type === 'capture_chunk' || event.type === 'input_level') return;
        const capabilities = event.snapshot?.capabilities;
        if (!capabilities) return;
        const serialized = JSON.stringify(capabilities);
        if (serialized === this.lastAudioCapabilities) return;
        this.lastAudioCapabilities = serialized;
        try {
          this.client?.sendCommand({ type: 'update_audio_capabilities', capabilities });
        } catch (error) {
          this.diagnostics.add('warn', 'bridge', `failed to update audio capabilities: ${sanitizeDiagnostic(error)}`);
        }
      });
    }
  }

  private startupDiagnostic(event: string, details: Record<string, unknown> = {}): void {
    this.diagnostics.add('info', 'host', diagnosticEvent(event, {
      projectPath: this.projectPath,
      sessionId: this.sessionId,
      ...details,
    }));
  }

  beginArchive(): () => void {
    if (this.archiving || this.hasActiveWork || this.pendingCron.size > 0) throw new Error('Wait for active work and pending interactions before archiving this chat.');
    this.archiving = true;
    return () => { this.archiving = false; };
  }

  /** Fence prompts, restarts, eviction and other controls throughout host migration. */
  beginWorkspaceChange(): () => void {
    if (this.ownedSessionResumeRequired && !this.resumeOperationInProgress) throw new Error('Resume this chat before changing its directory.');
    if (this.archiving || this.hasActiveWork || this.pendingCron.size > 0 || this.activeCronExecutions > 0 || this.isStarting) {
      throw new Error('Wait for active work and pending interactions before changing this chat’s directory.');
    }
    this.changingWorkspace = true;
    this.notifyActivityChanged();
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.changingWorkspace = false;
      this.notifyActivityChanged();
    };
  }

  setWorkingDirectory(request: SetWorkingDirectoryRequest): Promise<SetWorkingDirectoryResult> {
    if (!this.changingWorkspace) throw new Error('workspace operation is not fenced');
    return this.requireClient().setWorkingDirectory(request);
  }

  addWorkingDirectory(path: string): Promise<SessionWorkspaceState> {
    if (!this.changingWorkspace) throw new Error('workspace operation is not fenced');
    return this.requireClient().addWorkingDirectory(path);
  }

  getWorkspaceState(): Promise<SessionWorkspaceState> {
    return this.requireClient().getWorkspaceState();
  }

  completeWorkspaceRemount(): void { this.remountControls = undefined; }

  private async restoreRemountControls(): Promise<void> {
    if (!this.remountControls) return;
    // This private restore never updates shared defaults or replays opt-in transitions.
    await this.requireClient().restoreWorkspaceControls(this.remountControls);
  }

  get cronOperationPending(): boolean {
    return this.archiving || this.pendingCron.size > 0 || this.activeCronExecutions > 0;
  }

  manageCron(request: CronRequestDto): Promise<CronJobDto[]> {
    return this.sendCronCommand({ type: 'cron_manage', request_id: randomUUID(), request });
  }

  private readonly pendingRunBindings = new Map<string, { resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  markCronRunStarted(runId: string, sessionId: string): Promise<void> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pendingRunBindings.delete(runId); reject(new Error('Scheduled session binding timed out.')); }, 15_000);
      this.pendingRunBindings.set(runId, { resolve, reject, timer });
      try { this.requireClient().sendCommand({ type: 'cron_run_started', run_id: runId, session_id: sessionId }); }
      catch (error) { clearTimeout(timer); this.pendingRunBindings.delete(runId); reject(error); }
    });
  }

  private readonly modelCatalogWaiters = new Set<(event: Extract<ClientEvent, { type: 'model_list' }>) => void>();

  scheduledModelCatalog(): Promise<Extract<ClientEvent, { type: 'model_list' }>> {
    return new Promise((resolve, reject) => {
      const done = (event: Extract<ClientEvent, { type: 'model_list' }>) => { clearTimeout(timer); this.modelCatalogWaiters.delete(done); resolve(event); };
      const timer = setTimeout(() => { this.modelCatalogWaiters.delete(done); reject(new Error('Model catalog timed out.')); }, 15_000);
      this.modelCatalogWaiters.add(done);
      try { this.requireClient().sendCommand({ type: 'list_models' }); }
      catch (error) { clearTimeout(timer); this.modelCatalogWaiters.delete(done); reject(error); }
    });
  }

  async runScheduledTurn(runId: string, task: CronJobDto, beforeStart?: () => Promise<void>): Promise<string> {
    if (this.resumeOperationInProgress) throw new Error('interrupted: The target chat is being resumed.');
    if (this.ownedSessionResumeRequired) throw new Error('interrupted: The target chat must be resumed.');
    if (this.changingWorkspace) throw new Error('interrupted: The target chat’s directory is being changed.');
    const config = task.automation;
    if (!config?.model) throw new Error('paused: Configure a model for this task.');
    const generation = this.generation;
    while (this.turnActive || this.pendingInteractions > 0) {
      if (this.resumeOperationInProgress) throw new Error('interrupted: The target chat is being resumed.');
      if (this.ownedSessionResumeRequired) throw new Error('interrupted: The target chat must be resumed.');
      if (this.changingWorkspace) throw new Error('interrupted: The target chat’s directory is being changed.');
      if (this.disposed || generation !== this.generation || this.state.status !== 'connected') throw new Error('interrupted: Scheduled execution interrupted.');
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (this.changingWorkspace) throw new Error('interrupted: The target chat’s directory is being changed.');
    if (this.archiving) throw new Error('paused: The target chat is being archived.');
    if (this.resumeOperationInProgress) throw new Error('interrupted: The target chat is being resumed.');
    if (this.ownedSessionResumeRequired) throw new Error('interrupted: The target chat must be resumed.');
    const client = this.requireClient();
    const token = Symbol('scheduled hydration');
    this.pendingPromptHydrations.set(token, undefined);
    const assertPreparing = () => {
      if (this.resumeOperationInProgress) throw new Error('interrupted: The target chat is being resumed.');
      if (this.ownedSessionResumeRequired) throw new Error('interrupted: The target chat must be resumed.');
      if (this.changingWorkspace) throw new Error('interrupted: The target chat’s directory is being changed.');
      if (this.disposed || generation !== this.generation || client !== this.client) throw new Error('interrupted: Scheduled execution interrupted.');
      if (!this.pendingPromptHydrations.has(token)) throw new Error('cancelled: Scheduled execution was cancelled before starting.');
      if (this.archiving) throw new Error('paused: The target chat is being archived.');
    };
    try {
      if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
      assertPreparing();
      try { await this.ensureModelProviderCredential(config.model); }
      catch (error) { assertPreparing(); throw new Error(`paused: ${error instanceof Error ? error.message : String(error)}`); }
      assertPreparing();
      if (isCodexModel(config.model) && this.opts.resolveOpenAiOAuth) {
        try { await this.ensureCodexToken(); }
        catch (error) { assertPreparing(); throw new Error(`paused: ${error instanceof Error ? error.message : String(error)}`); }
        assertPreparing();
      }
      const catalog = await this.scheduledModelCatalog();
      assertPreparing();
      const model = catalog.details?.find((item) => item.reference === config.model);
      if (!model) throw new Error('paused: The configured model is unavailable. Choose another model.');
      const selection = config.reasoning;
      const selectionKey = (value: typeof selection) => value.type === 'level' ? `level:${value.id}` : value.type === 'token_budget' ? `tokens:${value.tokens}` : value.type;
      const supported = selection.type === 'automatic'
        || model.reasoning.options.some((option) => option.persistable && selectionKey(option.selection) === selectionKey(selection))
        || selectionKey(model.reasoning.provider_default) === selectionKey(selection)
        || (selection.type === 'token_budget' && model.reasoning.budget_range && selection.tokens >= model.reasoning.budget_range.min_tokens && selection.tokens <= model.reasoning.budget_range.max_tokens);
      if (!supported) throw new Error('paused: The configured reasoning setting is unavailable. Choose another effort.');
      await beforeStart?.();
      assertPreparing();
      return await new Promise<string>((resolve, reject) => {
        this.pendingScheduledTurns.set(runId, { resolve, reject });
        this.activeTurn = true;
        this.activeTurnGeneration = this.generation;
        this.sessionHasHistory = true;
        try {
          client.sendCommand({ type: 'scheduled_run_turn', run_id: runId, prompt: task.prompt, model: config.model, reasoning: config.reasoning });
          this.journalPrompt(task.prompt, [], 0, { origin: 'scheduled' });
        } catch (error) {
          this.pendingScheduledTurns.delete(runId);
          this.activeTurn = false;
          reject(error);
        }
      });
    } finally {
      this.pendingPromptHydrations.delete(token);
      this.notifyActivityChanged();
    }
  }

  private sendCronCommand(command: Extract<ClientCommand, { type: 'cron_manage' }>): Promise<CronJobDto[]> {
    if (this.resumeOperationInProgress) return Promise.reject(new Error('This chat is being resumed.'));
    if (this.ownedSessionResumeRequired) return Promise.reject(new Error('Resume this chat before managing its scheduled tasks.'));
    if (this.changingWorkspace) return Promise.reject(new Error('This chat’s directory is being changed.'));
    const { request_id, request } = command;
    if (this.pendingCron.has(request_id)) return Promise.reject(new Error('Scheduled task request is already pending.'));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingCron.delete(request_id);
        this.notifyActivityChanged();
        reject(new Error('Scheduled task operation timed out.'));
      }, 30_000);
      this.pendingCron.set(request_id, { resolve, reject, timer, creating: request.action === 'create' });
      this.notifyActivityChanged();
      try { this.requireClient().sendCommand(command); }
      catch (error) {
        clearTimeout(timer);
        this.pendingCron.delete(request_id);
        this.notifyActivityChanged();
        reject(error);
      }
    });
  }

  get connectionState(): ConnectionState {
    return this.state;
  }

  get turnActive(): boolean {
    return this.activeTurn || this.pendingPromptHydrations.size > 0 || this.pendingModelSwitch !== undefined;
  }

  get hasActiveWork(): boolean {
    return this.resumeOperationInProgress || this.changingWorkspace || this.liveSideChats.size > 0 || this.pendingSideCreates.size > 0 || this.turnActive || this.hasActiveAgents || this.pendingInteractions > 0;
  }

  private currentResumeOwner(): PendingSessionResume | null {
    const pending = this.pendingSessionResume;
    return pending?.generation === this.generation && pending.client === this.client && !this.disposed ? pending : null;
  }

  private readonly gitActivity = new GitActivityTracker();
  get hasActiveAgents(): boolean { return this.gitActivity.active; }

  /** A same-process reconnect retains its in-memory session and background scopes. */
  get recoveredLiveConnection(): boolean { return this.liveConnectionRecovered; }

  /** A connected transport alone does not prove that owned history restored. */
  get needsSessionResume(): boolean { return this.ownedSessionResumeRequired; }

  get isStarting(): boolean { return this.startPromise !== null; }

  /** Wait for existing lifecycle work without starting a stopped runtime. */
  async waitForLifecycle(): Promise<void> {
    let lifecycle: Promise<void>;
    do {
      if (this.disposed) throw new Error('session runtime is no longer open');
      lifecycle = this.restartChain;
      // A refused restart can leave the current connection usable. Its caller
      // owns that failure; this fence only waits for lifecycle work to settle.
      await lifecycle.catch(() => undefined);
    } while (lifecycle !== this.restartChain);
    if (this.disposed) throw new Error('session runtime is no longer open');
  }

  get activeCredentialProviderIds(): readonly string[] {
    return [...this.activeCredentialProviders];
  }

  get persistedCredentialProviderIds(): readonly string[] {
    return [...this.persistedCredentialProviders];
  }

  get providerCredentialStorageEncrypted(): boolean {
    return this.credentialStorageEncrypted;
  }

  get providerCredentialPreviews(): Readonly<Record<string, string>> {
    return Object.fromEntries(this.credentialPreviews);
  }

  get runtimeVersions(): BridgeRuntimeVersions | undefined {
    return this.lastRuntimeVersions ? { ...this.lastRuntimeVersions } : undefined;
  }

  get pendingAskUserQuestions(): readonly AskUserQuestionRequestDto[] {
    return [...this.pendingAskUserQuestionRequests.values()].map((entry) => entry.request);
  }

  /**
   * Human descriptions for background tasks, so `agent_completed` can name the
   * task instead of printing a uuid. `task_row` is the only event that carries
   * one; `task_status_changed` carries just the id.
   */
  private readonly taskLabels = new Map<string, string>();

  /** `undefined` until both halves are real — `main/index.ts`'s notification
   * click handler restores a session from this, and half a ref restores
   * nothing. */
  private get notificationRef(): SessionRef | undefined {
    const projectPath = this.projectPath || this.activeWorkspace || '';
    if (!projectPath || !isSessionId(this.sessionId)) return undefined;
    return { projectPath, sessionId: this.sessionId };
  }

  /**
   * Drives `HostNotifier` off the same event stream the renderer sees.
   *
   * Note what is NOT here: a "turn finished" notification. Upstream has none —
   * `turn_ended` only ARMS the idle timer, which fires `idle_prompt` a minute
   * later and only if the user never came back.
   */
  private updateNotifier(event: ClientEvent): void {
    const notifier = this.opts.notifier;
    if (!notifier) return;
    const ref = this.notificationRef;
    switch (event.type) {
      case 'turn_started':
        notifier.turnStarted(this.sessionId, ref);
        break;
      case 'turn_ended':
        notifier.turnEnded(this.sessionId, ref);
        break;
      case 'session_ended':
        notifier.sessionEnded(this.sessionId);
        this.taskLabels.clear();
        return;
      case 'ask_user_question':
        // Only for a request that actually got queued above; one rejected by
        // the pending-limit has no card for the user to answer.
        if (this.pendingAskUserQuestionIds.has(event.request.request_id)) {
          notifier.askUserQuestion(this.sessionId, event.request.request_id, ref);
        }
        break;
      case 'permission_request_resolved':
        notifier.permissionSettled(this.sessionId, event.request_id);
        break;
      case 'task_row':
        if (event.task.description) this.taskLabels.set(event.task.task_id, event.task.description);
        break;
      case 'task_status_changed': {
        const status = event.status.type;
        if (status !== 'completed' && status !== 'failed') break;
        notifier.taskFinished(
          this.sessionId, event.task_id, this.taskLabels.get(event.task_id),
          status === 'failed', ref,
        );
        this.taskLabels.delete(event.task_id);
        break;
      }
      default:
        break;
    }
    notifier.setDialogsOnScreen(this.sessionId, this.pendingInteractions, ref);
  }

  get pendingInteractions(): number {
    return this.pendingPermissionIds.size
      + [...this.permissionScopeChecks.keys()].filter((id) => !this.pendingPermissionIds.has(id)).length
      + this.pendingComputerAccessIds.size
      + this.pendingAskUserQuestionIds.size;
  }

  get summary(): SessionRuntimeSummary {
    return {
      projectPath: this.projectPath || this.activeWorkspace || '',
      sessionId: this.sessionId,
      connection: this.connectionState,
      turnActive: this.turnActive,
      pendingInteractions: this.pendingInteractions,
      pendingAskUserQuestions: this.pendingAskUserQuestionIds.size,
      ...(this.runtimeVersions ? { runtimeVersions: this.runtimeVersions } : {}),
    };
  }

  replaySnapshot(): readonly SequencedRuntimeEventEnvelope<ClientEvent>[] {
    return Object.freeze(this.replayEvents.map((envelope) => Object.freeze({
      sessionId: envelope.sessionId,
      sequence: envelope.sequence,
      event: structuredClone(envelope.event),
    })));
  }

  // ── Remote Control ──────────────────────────────────────────────────────────

  /**
   * Watch the runtime from outside the desktop window: every client event as it
   * is enveloped, prompts, permission and computer-access requests, changes to
   * what is pending, connection states and disposal, all in the order they
   * happen and synchronously. An observer that throws is ignored. Returns the
   * function that stops watching.
   */
  observeRemote(observer: RemoteObserver): () => void {
    this.remoteObservers.add(observer);
    return () => { this.remoteObservers.delete(observer); };
  }

  /** Whether anything is watching with {@link observeRemote}. */
  get remoteObserved(): boolean { return this.remoteObservers.size > 0; }

  /** Read the state a snapshot is built from. Synchronous, so it can be paired with {@link observeRemote} without a gap. */
  remoteView(): RemoteRuntimeView {
    const first = this.replayEvents[0];
    const base = first && TRANSCRIPT_REPLAY_BASE_EVENTS.has(first.event.type) ? first : undefined;
    const controlsEvent = this.latestState.get('conversation_controls_changed')?.event;
    return {
      sessionId: this.sessionId,
      runtimeId: this.runtimeId,
      epoch: `${this.runtimeId}:${this.remoteBaseSequence}`,
      baseSequence: this.remoteBaseSequence,
      base,
      events: base ? this.replayEvents.slice(1) : this.replayEvents,
      state: [...this.latestState.values()],
      prompts: this.promptJournal,
      interactions: this.interactionRecords,
      droppedThroughSequence: this.droppedThroughSequence,
      lastSequence: this.eventSequence,
      pending: this.remotePending(),
      turn: { running: this.turnActive, ...(this.activeTurnId !== undefined ? { turn_id: this.activeTurnId } : {}) },
      controls: controlsEvent?.type === 'conversation_controls_changed' ? controlsEvent.controls : undefined,
      model: this.remoteModel,
      connection: this.state,
    };
  }

  /** The model the session is using: the engine's latest word, else the one it was launched with. */
  get remoteModel(): string | undefined {
    const event = this.latestState.get('model_changed')?.event;
    return event?.type === 'model_changed' ? event.model : this.selectedModelReference;
  }

  /** Everything waiting for an answer, in full. */
  remotePending(): RemoteRuntimeView['pending'] {
    return {
      permissions: [...this.pendingPermissionIds.values()].map(({ backgroundOwned: _owned, ...request }) => structuredClone(request)),
      questions: this.pendingAskUserQuestions.map((request) => structuredClone(request)),
      computer_access: [...this.pendingComputerAccessIds.values()].map((request) => structuredClone(request)),
    };
  }

  /** The permission request with this id, if it is still waiting. */
  pendingPermission(requestId: number): HostPermissionRequest | undefined {
    return this.pendingPermissionIds.get(requestId);
  }

  pendingQuestion(requestId: number): AskUserQuestionRequestDto | undefined {
    return this.pendingAskUserQuestionRequests.get(requestId)?.request;
  }

  /** The bytes of an image attached to the prompt at `sequence`, while the journal still holds them. */
  promptImage(sequence: number, index: number): { media_type: string; base64: string } | undefined {
    const image = this.promptJournal.find((entry) => entry.sequence === sequence)?.images[index];
    return image?.base64 === undefined ? undefined : { media_type: image.media_type, base64: image.base64 };
  }

  private notifyRemote(observation: RemoteObservation): void {
    for (const observer of [...this.remoteObservers]) {
      try { observer(observation); } catch (error) {
        this.diagnostics.add('warn', 'host', `remote observer failed: ${sanitizeDiagnostic(error)}`);
      }
    }
  }

  private noteRemoteEvent(envelope: SequencedRuntimeEventEnvelope<ClientEvent>): void {
    if (REMOTE_STATE_EVENTS.has(envelope.event.type)) this.latestState.set(envelope.event.type, envelope);
    this.notifyRemote({ type: 'event', envelope });
  }

  /** Tell observers the pending set changed. Coalesced into one notification per tick; the set is read when they handle it. */
  private notifyRemotePending(): void {
    if (this.remotePendingNotified || this.remoteObservers.size === 0) return;
    this.remotePendingNotified = true;
    queueMicrotask(() => {
      this.remotePendingNotified = false;
      this.notifyRemote({ type: 'pending' });
    });
  }

  private recordInteraction(record: InteractionRecord['record']): void {
    const interaction: InteractionRecord = { sequence: ++this.eventSequence, record };
    this.interactionRecords = [...this.interactionRecords, interaction];
    if (this.interactionRecords.length > MAX_INTERACTION_RECORDS) {
      const dropped = this.interactionRecords.slice(0, this.interactionRecords.length - MAX_INTERACTION_RECORDS);
      this.interactionRecords = this.interactionRecords.slice(-MAX_INTERACTION_RECORDS);
      this.droppedThroughSequence = Math.max(this.droppedThroughSequence, dropped[dropped.length - 1]!.sequence);
    }
    this.notifyRemote({ type: 'interaction', interaction });
  }

  /** Remember a prompt the renderer shows as a user row. */
  private journalPrompt(text: string, images: readonly ImageRefDto[], turnId: number, source: PromptSource | undefined): void {
    const entry: PromptJournalEntry = {
      sequence: ++this.eventSequence,
      turnId,
      text,
      origin: source?.origin ?? 'desktop',
      ...(source?.deviceName !== undefined ? { deviceName: source.deviceName } : {}),
      images: images.map((image) => ({ media_type: image.media_type, base64: image.base64 })),
    };
    const journal = [...this.promptJournal, entry];
    if (journal.length > MAX_PROMPT_JOURNAL) {
      this.droppedThroughSequence = Math.max(this.droppedThroughSequence, journal[journal.length - MAX_PROMPT_JOURNAL - 1]!.sequence);
    }
    let withBytes = 0;
    this.promptJournal = journal.slice(-MAX_PROMPT_JOURNAL).reverse().map((kept) => {
      if (!kept.images.some((image) => image.base64 !== undefined) || ++withBytes <= MAX_JOURNAL_ENTRIES_WITH_IMAGES) return kept;
      return { ...kept, images: kept.images.map((image) => ({ media_type: image.media_type })) };
    }).reverse();
    this.notifyRemote({ type: 'prompt', entry: this.promptJournal[this.promptJournal.length - 1]! });
  }

  /** The slash commands that make a user row in the renderer are journaled like prompts. */
  private journalSlashCommand(command: ClientCommand, source: PromptSource | undefined): void {
    if (command.type === 'run_slash_command' && slashRequestEchoes(command.raw)) this.journalPrompt(command.raw.trim(), command.images, command.turn_id ?? 0, source);
  }

  private eventEnvelope(event: ClientEvent, retain: boolean): SequencedRuntimeEventEnvelope<ClientEvent> {
    const envelope = Object.freeze({
      sessionId: this.sessionId,
      sequence: ++this.eventSequence,
      event: structuredClone(event),
    });
    if (TRANSCRIPT_REPLAY_BASE_EVENTS.has(event.type)) {
      this.replayEvents = [envelope];
      // What came before the base is part of it (a resumed transcript holds the earlier prompts) or gone (`/clear`).
      this.remoteBaseSequence = envelope.sequence;
      this.promptJournal = [];
      this.interactionRecords = [];
      this.droppedThroughSequence = 0;
    } else if (retain && this.replayEvents.length > 0 && TRANSCRIPT_REPLAY_EVENTS.has(event.type)) {
      // Status is a snapshot, not a transcript event. Keep only the newest
      // one so repeated listing refreshes cannot grow the renderer replay
      // buffer without bound or replay stale cumulative totals on reload.
      const replay = event.type === 'status_snapshot' || event.type === 'session_storage_state'
        ? this.replayEvents.filter(({ event: retained }) => retained.type !== event.type)
        : event.type === 'ui_status'
          ? this.replayEvents.filter(({ event: retained }) =>
              retained.type !== 'ui_status' || retained.plugin !== event.plugin)
          : this.replayEvents;
      this.replayEvents = [...replay, envelope];
    }
    return envelope;
  }

  private sendClientEvent(webContents: WebContents, event: ClientEvent, retain: boolean): void {
    const envelope = this.eventEnvelope(event, retain);
    webContents.send(CH_EVENT, this.opts.envelopeEvents ? envelope : event);
  }

  private broadcastClientEvent(event: ClientEvent): void {
    // SideChat events use the sequenced renderer channel. On reconnect the
    // renderer reconciles child state through side_chat_list / side_chat_get;
    // useBridge filters these events before parent conversation reducers.
    const envelope = this.eventEnvelope(event, true);
    this.noteRemoteEvent(envelope);
    if (!this.opts.envelopeEvents) {
      this.broadcast(CH_EVENT, event);
      return;
    }
    for (const webContents of this.targets.keys()) {
      if (webContents.isDestroyed()) this.targets.delete(webContents);
      else webContents.send(CH_EVENT, envelope);
    }
  }

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const origin = urlOrigin(rendererUrl);
    if (!origin) throw new Error('invalid renderer URL');
    const origins = this.targets.get(webContents) ?? new Set<string>();
    origins.add(origin);
    this.targets.set(webContents, origins);
    this.replayPendingInteractions(webContents);
  }

  /** Re-deliver interactions that may have arrived before a renderer reload. */
  replayPendingInteractions(webContents: WebContents): void {
    for (const request of this.pendingPermissionIds.values()) {
      this.sendToWindow(webContents, CH_PERMISSION, request);
    }
    for (const pending of this.pendingAskUserQuestionRequests.values()) {
      this.sendClientEvent(webContents, { type: 'ask_user_question', request: pending.request }, false);
    }
  }

  unregisterWindow(webContents: WebContents): void {
    this.targets.delete(webContents);
  }

  private clearPendingAskUserQuestion(requestId: number): void {
    this.pendingAskUserQuestionRequests.delete(requestId);
    this.pendingAskUserQuestionIds.delete(requestId);
    this.notifyRemotePending();
  }

  private notifyActivityChanged(): void {
    this.opts.onActivityChanged?.();
  }

  /** A foreground terminal cannot settle independent background permission gates. */
  private clearTurnInteractions(all = false): void {
    ++this.foregroundInteractionEpoch;
    for (const id of this.pendingPermissionIds.keys()) {
      if (all || !this.backgroundPermissionIds.has(id)) this.clearPendingPermission(id);
    }
    if (all) {
      this.permissionScopeChecks.clear();
      this.backgroundPermissionIds.clear();
      this.resolvedPermissionIds.clear();
    }
    this.pendingComputerAccessIds.clear();
    for (const requestId of [...this.pendingAskUserQuestionIds]) {
      this.clearPendingAskUserQuestion(requestId);
    }
    this.notifyRemotePending();
  }

  private clearPendingPermission(id: number): void {
    this.pendingPermissionIds.delete(id);
    this.backgroundPermissionIds.delete(id);
    this.permissionScopeChecks.delete(id);
    this.notifyRemotePending();
  }

  private replayBackgroundPermissions(): void {
    for (const id of this.backgroundPermissionIds) {
      const request = this.pendingPermissionIds.get(id);
      if (request) this.broadcast(CH_PERMISSION, request);
    }
  }

  private async receivePermissionRequest(client: BridgeClient, generation: number, request: PermissionRequest): Promise<void> {
    const id = request.request_id;
    if (generation !== this.generation || client !== this.client || this.disposed
      || !Number.isSafeInteger(id) || id < 0 || this.resolvedPermissionIds.has(id)) return;
    if (!this.pendingPermissionIds.has(id) && !this.permissionScopeChecks.has(id)
      && this.pendingPermissionIds.size + this.permissionScopeChecks.size >= MAX_PENDING_PERMISSIONS) {
      this.diagnostics.add('warn', 'bridge', 'permission request limit reached');
      return;
    }
    const foregroundEpoch = this.foregroundInteractionEpoch;
    const resumeOwner = this.currentResumeOwner();
    this.permissionScopeChecks.set(id, request);
    this.notifyActivityChanged();
    try {
      // Scope comes from the actual pending SDK broker entry. Named and unnamed
      // background runners have the same ownership; display labels confer none.
      const scope = await client.requestPermissionScope(id);
      if (this.permissionScopeChecks.get(id) !== request || generation !== this.generation
        || client !== this.client || this.disposed || this.resolvedPermissionIds.has(id)) return;
      if (!scope) return;
      if (!scope.background_owned && ((!this.activeTurn && (!resumeOwner || this.currentResumeOwner() !== resumeOwner)) || this.cancellingTurn
        || foregroundEpoch !== this.foregroundInteractionEpoch)) {
        this.diagnostics.add('warn', 'bridge', `dropped permission request ${id}: foreground owner ended or is cancelling`);
        return;
      }
      if (scope.background_owned) this.backgroundPermissionIds.add(id);
      else this.backgroundPermissionIds.delete(id);
      this.permissionScopeChecks.delete(id);
      const scopedRequest: HostPermissionRequest = { ...request, backgroundOwned: scope.background_owned };
      this.pendingPermissionIds.set(id, scopedRequest);
      this.recordInteraction({ type: 'permission_request', request: structuredClone(request) });
      this.notifyRemotePending();
      this.broadcast(CH_PERMISSION, scopedRequest);
      this.opts.notifier?.permissionRequested(this.sessionId, id,
        request.kind.type === 'tool_use_confirm' ? request.kind.tool_name : request.kind.type, this.notificationRef);
      this.opts.notifier?.setDialogsOnScreen(this.sessionId, this.pendingInteractions, this.notificationRef);
    } catch (error) {
      if (generation === this.generation && client === this.client && this.permissionScopeChecks.get(id) === request) {
        this.diagnostics.add('warn', 'bridge', `permission request ${id} scope unavailable: ${sanitizeDiagnostic(error)}`);
      }
    } finally {
      if (this.permissionScopeChecks.get(id) === request) this.permissionScopeChecks.delete(id);
      this.completePendingSessionResumeIfReady();
      this.notifyActivityChanged();
    }
  }

  async start(): Promise<void> {
    if (this.disposed) throw new Error('SessionRuntime is disposed');
    if (this.startPromise) return this.startPromise;
    if (this.state.status === 'connected') return;
    const startedAt = Date.now();
    const starting = this.restartChain.catch(() => undefined).then(async () => {
      try {
        if (this.disposed) throw new Error('SessionRuntime is disposed');
        // Explicit restart/stop operations and automatic connection recovery
        // share one lifecycle queue, so only one sidecar can be started.
        if (this.state.status === 'connected') return;
        if (this.child || this.client || this.adoptedPid) await this.recoverConnection();
        else await this.startInternal();
      } catch (error) {
        this.startupDiagnostic('bridge_start_failed', {
          durationMs: Date.now() - startedAt,
          error: sanitizeDiagnostic(error),
        });
        if (!this.disposed && this.state.status !== 'error') this.fail(error);
        throw error;
      }
    });
    this.restartChain = starting;
    const completion = starting.finally(() => {
      if (this.startPromise === completion) this.startPromise = null;
    });
    this.startPromise = completion;
    return completion;
  }

  private async recoverConnection(): Promise<void> {
    this.liveConnectionRecovered = false;
    const interruptedForeground = this.disconnectedForegroundPending || this.activeTurn || this.pendingInteractions > 0 || this.pendingPromptHydrations.size > 0;
    const pid = this.child?.pid ?? this.adoptedPid;
    if (!pid || !this.processIsAlive(pid)) {
      await this.stopBridge();
      if (this.disposed) throw new Error('SessionRuntime is disposed');
      await this.startInternal();
      return;
    }
    // Reconnect only the endpoint already authenticated for this runtime.
    // A living managed or external sidecar must never be replaced merely
    // because its transport disconnected or a retry handshake failed.
    const lockfilePath = this.connectionLockfilePath;
    if (!lockfilePath) throw new Error('The running bridge has no known connection endpoint; no replacement was started.');
    validateBridgeLockfile(JSON.parse(readFileSync(lockfilePath, 'utf8')), pid, this.projectPath || this.activeWorkspace || '');
    const client = this.client;
    this.client = null;
    if (client) this.closeDetachedClient(client);
    const previousGeneration = this.generation++;
    const recoveredGeneration = this.generation;
    await this.teardownAudioGeneration(previousGeneration);
    await this.oauthPersistence;
    if (this.disposed || recoveredGeneration !== this.generation) throw new Error('bridge connection superseded');
    this.clearPendingConnectionOperations();
    try {
      await this.connectBridgeClient(lockfilePath, recoveredGeneration, false, false);
      if (this.disposed || recoveredGeneration !== this.generation) throw new Error('bridge connection superseded');
      const recoveredClient = this.requireClient();
      await recoveredClient.requestRuntimeSnapshot((events) => {
        // Apply at the response boundary, before later live frames. An old
        // connection response must never overwrite a newer runtime's roster.
        if (this.disposed || recoveredGeneration !== this.generation || recoveredClient !== this.client) {
          throw new Error('Runtime background snapshot was interrupted.');
        }
        const roster = events.find((event) => event.type === 'session_agent_list');
        if (roster?.type !== 'session_agent_list' || roster.session_id !== this.sessionId) {
          throw new Error('Runtime background snapshot belongs to another session.');
        }
        this.gitActivity.replaceSnapshot(events);
        for (const event of events) this.broadcastClientEvent(event);
        this.notifyActivityChanged();
      });
      if (this.disposed || recoveredGeneration !== this.generation || recoveredClient !== this.client) {
        throw new Error('bridge connection superseded');
      }
      // Accepted hello follows the Rust connection-close join barrier. The
      // previous foreground driver and its questions are gone; background
      // agents still belong to this living process and must stay pinned.
      const newForegroundStarted = this.activeTurnGeneration === this.generation;
      if (!newForegroundStarted) {
        this.activeTurn = false;
        this.activeTurnGeneration = undefined;
        this.activeTurnId = undefined;
        this.cancellingTurn = false;
        this.clearTurnInteractions();
      }
      this.liveConnectionRecovered = true;
      this.disconnectedForegroundPending = false;
      if (interruptedForeground && !newForegroundStarted) {
        this.broadcastClientEvent({
          type: 'turn_ended', outcome: { type: 'cancelled' },
          cost: { ...(this.lastKnownCost ?? { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0 }), formatted: '' },
        });
        this.broadcastClientEvent({ type: 'system_notice', message: 'The previous turn was interrupted when its engine connection closed.', is_error: true });
      }
      this.notifyActivityChanged();
      this.setState({ status: 'connected' });
    } catch (error) {
      if (recoveredGeneration === this.generation) {
        const failedClient = this.client as BridgeClient | null;
        this.client = null;
        if (failedClient) this.closeDetachedClient(failedClient);
      }
      throw error;
    }
  }

  private closeDetachedClient(client: BridgeClient): void {
    client.removeAllListeners();
    client.on('error', (error) => {
      this.diagnostics.add('warn', 'bridge', `disconnected bridge transport: ${sanitizeDiagnostic(error)}`);
    });
    try { client.close(); } catch { /* a disconnected transport may already be closed */ }
  }

  private readonly liveSideChats = new Set<string>();
  private readonly pendingSideCreates = new Set<string>();

  private assertNoLiveSideChats(): void {
    if (this.liveSideChats.size > 0 || this.pendingSideCreates.size > 0) throw new Error('End all Side Chats before stopping or restarting this chat.');
  }

  restart(beforeRestart?: () => void | Promise<void>): Promise<void> {
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) throw new Error('session runtime is no longer open');
      // This runs after any earlier queued lifecycle work and immediately
      // before stopping the child. Callers can re-check ownership/work here
      // to close the queueing race between IPC validation and restart.
      this.assertNoLiveSideChats();
      await beforeRestart?.();
      this.assertNoLiveSideChats();
      this.setState({ status: 'restarting' });
      try {
        await this.stopBridge();
        await this.startInternal();
      } catch (error) {
        if (this.state.status !== 'error') this.fail(error);
        throw error;
      }
    });
    return this.restartChain;
  }

  stop(): Promise<void> {
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) return;
      this.assertNoLiveSideChats();
      await this.stopBridge();
      this.setState({ status: 'idle' });
    });
    return this.restartChain;
  }

  /** Security revocation cannot wait for the user to close Side Chats. */
  stopForComputerPolicyFailure(): Promise<void> {
    try { this.revokeComputerAccess(); }
    catch (error) { this.diagnostics.add('warn', 'host', sanitizeDiagnostic(error)); }
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) return;
      try { await this.stopBridge(); }
      finally { this.setState({ status: 'error', message: 'Computer permissions became unavailable. Restart this chat to continue.' }); }
    });
    return this.restartChain;
  }

  private async startInternal(): Promise<void> {
    this.liveConnectionRecovered = false;
    this.configuredCustomProviderIds = [];
    this.credentialRoutingSettings = undefined;
    this.selectedModelReference = undefined;
    this.pendingCredentialSettings?.reject(new Error('Provider settings loading was interrupted.'));
    this.pendingCredentialSettings = undefined;
    let generation = ++this.generation;
    const startedAt = Date.now();
    this.startupDiagnostic('bridge_start_started', { generation });
    const bridgeRoot = this.opts.bridgeRoot;
    if (bridgeRoot && this.projectPath) {
      const ref = { projectPath: this.projectPath, sessionId: this.sessionId };
      const discoveryStartedAt = Date.now();
      const reusable = discoverReusableBridge(
        bridgeRoot,
        ref,
        this.opts.readProcessCommand ?? readProcessCommand,
      ) ?? discoverExternalReusableBridge(
        ref,
        (this.opts.listProcessCommands ?? listProcessCommands)(),
      );
      this.startupDiagnostic('bridge_start_phase', {
        durationMs: Date.now() - discoveryStartedAt,
        generation,
        phase: 'reusable_bridge_discovery',
        reused: Boolean(reusable),
      });
      if (reusable) {
        this.launchDir = reusable.launchDir;
        this.adoptedPid = reusable.pid;
        this.adoptedProcessOwned = reusable.ownedProcess;
        this.activeWorkspace = this.projectPath;
        const access = this.opts.accessState?.();
        this.activeWorkspaceTrusted = Boolean(
          access?.trusted
          && (!access.workspace || access.workspace === this.projectPath),
        );
        try {
          await this.connectBridgeClient(reusable.lockfilePath, generation, false);
          this.startupDiagnostic('bridge_start_completed', {
            adopted: true,
            durationMs: Date.now() - startedAt,
            generation,
          });
          this.diagnostics.add('info', 'host', diagnosticEvent('bridge_adopted', {
            pid: reusable.pid,
            sessionId: this.sessionId,
          }));
          return;
        } catch (error) {
          this.diagnostics.add('warn', 'host', `failed to adopt existing session runtime: ${sanitizeDiagnostic(error)}`);
          await this.stopBridge();
          if (!reusable.ownedProcess) throw error;
          generation = ++this.generation;
        }
      }
    }

    const launchConfigStartedAt = Date.now();
    const launch = await this.opts.launchConfig();
    this.startupDiagnostic('bridge_start_phase', {
      durationMs: Date.now() - launchConfigStartedAt,
      generation,
      phase: 'launch_config',
    });
    // An engine receives only the access token; this host refreshes the session.
    if (launch.openaiOAuth) launch.openaiOAuth = codexAccessToken(launch.openaiOAuth);
    this.servedCodexToken = launch.openaiOAuth?.access_token;
    if (this.disposed) throw new Error('SessionRuntime is disposed');
    this.selectedModelReference = launch.model;
    this.activeWorkspace = launch.workspace;
    if (launch.sessionId && launch.sessionId !== this.sessionId) {
      throw new Error('bridge launch session id does not match the runtime');
    }
    this.activeWorkspaceTrusted = launch.trusted;
    this.runtimeCredentialProviders = new Set([
      ...(launch.apiKey ? ['anthropic'] : []),
      ...Object.keys(launch.providerCredentials ?? {}),
    ]);
    this.persistedCredentialProviders.clear();
    this.activeCredentialProviders = new Set(this.runtimeCredentialProviders);
    this.credentialPreviews.clear();
    this.credentialStorageEncrypted = false;
    const bridgeDir = this.createLaunchDirectory();

    this.setState({ status: 'spawning' });
    let child: ChildProcess;
    try {
      child = this.spawnServer(launch, bridgeDir);
    } catch (error) {
      this.fail(error);
      this.removeLaunchDirectory();
      throw error;
    }
    this.child = child;
    this.startupDiagnostic('bridge_start_phase', {
      generation,
      phase: 'spawn',
      pid: child.pid,
    });
    const pluginSecretValues = Object.values(launch.pluginSecrets ?? {}).flatMap((values) => Object.values(values));
    this.captureLogs(child, [
      launch.apiKey,
      launch.openaiOAuth?.access_token,
      launch.openaiOAuth?.refresh_token,
      ...Object.values(launch.providerCredentials ?? {}),
      ...pluginSecretValues,
    ].filter((value): value is string => Boolean(value)));

    child.once('exit', (code, signal) => {
      if (this.child !== child) return;
      const exitGeneration = this.generation;
      this.diagnostics.add('info', 'host', childExitDiagnostic(code, signal, exitGeneration));
      this.child = null;
      void this.teardownAudioGeneration(exitGeneration);
      if (!this.disposed) {
        this.setState({ status: 'disconnected', reason: `bridge-server exited (code=${code ?? 'null'}, signal=${signal ?? 'null'})` });
      }
    });
    child.once('error', (error) => {
      if (this.child !== child) return;
      void this.teardownAudioGeneration(this.generation);
      if (!this.disposed) this.fail(`failed to spawn bridge-server: ${error.message}`);
    });

    try {
      const lockfileStartedAt = Date.now();
      const lockfilePath = await this.waitForLockfile(bridgeDir, launch, generation);
      this.startupDiagnostic('bridge_start_phase', {
        durationMs: Date.now() - lockfileStartedAt,
        generation,
        phase: 'wait_for_lockfile',
      });
      if (this.disposed) throw new Error('SessionRuntime is disposed');
      const connectStartedAt = Date.now();
      await this.connectBridgeClient(lockfilePath, generation);
      this.startupDiagnostic('bridge_start_completed', {
        connectDurationMs: Date.now() - connectStartedAt,
        durationMs: Date.now() - startedAt,
        generation,
        lockfileWaitMs: connectStartedAt - lockfileStartedAt,
      });
    } catch (error) {
      if (generation === this.generation) {
        this.fail(error);
        await this.stopBridge();
      }
      throw error;
    }
  }

  private async connectBridgeClient(lockfilePath: string, generation: number, restorePermission = true, publishConnected = true): Promise<void> {
    if (generation !== this.generation || this.disposed) throw new Error('bridge connection superseded');
    this.connectionLockfilePath = lockfilePath;
    this.setState({ status: 'connecting' });
    const startedAt = Date.now();
    this.startupDiagnostic('bridge_connect_started', { generation, restorePermission });
    try {
      await this.opts.audioService?.initializeCapabilities();
    } catch (error) {
      this.diagnostics.add('warn', 'bridge', `audio capability initialization failed: ${sanitizeDiagnostic(error)}`);
    }
    if (generation !== this.generation || this.disposed) throw new Error('bridge connection superseded');
    const initialAudioCapabilities = this.opts.audioService?.getCapabilities();
    const client = new BridgeClient({
      lockfilePath,
      clientName: 'lingxi-electron/0.1.0',
      ...(initialAudioCapabilities ? { audioCapabilities: initialAudioCapabilities } : {}),
    });
    this.cachedAudioSessionContext = undefined;
    this.verifiedRealtimeAudio = false;
    this.wireClient(client, generation);
    const assertCurrentConnection = (): void => {
      if (generation !== this.generation || client !== this.client || this.disposed) {
        this.closeDetachedClient(client);
        throw new Error('bridge connection superseded');
      }
    };
    const handshakeStartedAt = Date.now();
    const hello = await client.connect();
    assertCurrentConnection();
    this.verifiedRealtimeAudio = hello.capabilities.realtime_audio === true;
    this.startupDiagnostic('bridge_connect_phase', {
      durationMs: Date.now() - handshakeStartedAt,
      generation,
      phase: 'websocket_handshake',
    });
    client.sendCommand({ type: 'get_audio_session_context' });
    const currentAudioCapabilities = this.opts.audioService?.getCapabilities();
    if (
      currentAudioCapabilities
      && JSON.stringify(currentAudioCapabilities) !== JSON.stringify(initialAudioCapabilities)
    ) {
      client.sendCommand({ type: 'update_audio_capabilities', capabilities: currentAudioCapabilities });
      this.lastAudioCapabilities = JSON.stringify(currentAudioCapabilities);
    }
    this.lastRuntimeVersions = {
      serverName: hello.server_name,
      serverProtocol: hello.protocol_version,
      clientProtocol: hello.capabilities.client_protocol_version,
    };
    this.diagnostics.add(
      'info',
      'bridge',
      bridgeVersionDiagnostic(hello.server_name, hello.protocol_version, hello.capabilities.client_protocol_version),
    );
    if (restorePermission) {
      const permissionStartedAt = Date.now();
      if (this.remountControls) await this.restoreRemountControls();
      else await this.restorePermissionMode();
      assertCurrentConnection();
      this.startupDiagnostic('bridge_connect_phase', {
        durationMs: Date.now() - permissionStartedAt,
        generation,
        phase: 'restore_permission_mode',
      });
      const fastStartedAt = Date.now();
      if (!this.remountControls) await this.restoreFastMode();
      assertCurrentConnection();
      this.startupDiagnostic('bridge_connect_phase', {
        durationMs: Date.now() - fastStartedAt,
        generation,
        phase: 'restore_fast_mode',
      });
    }
    assertCurrentConnection();
    // A transport reconnect can reuse a living sidecar whose keys changed
    // while it was offline. Reconcile those keys before exposing readiness.
    for (const providerId of this.invalidatedProviderCredentials) {
      await this.ensureProviderCredentialCached(providerId);
    }
    assertCurrentConnection();
    if (this.liveSideChats.size > 0 || this.pendingSideCreates.size > 0) this.requireClient().sendCommand({ type: 'side_chat_list' });
    if (publishConnected) this.setState({ status: 'connected' });
    this.startupDiagnostic('bridge_connect_completed', {
      durationMs: Date.now() - startedAt,
      generation,
    });
    // Status refreshes use attribute-only broker queries; they do not
    // decrypt every saved credential or expose secret bytes to the renderer.
    void this.refreshProviderCredentials();
  }

  private async refreshProviderCredentials(): Promise<void> {
    const providerIds = this.opts.providerIds ?? [];
    if (providerIds.length > 0) {
      try {
        await this.listProviderCredentials(providerIds);
      } catch (error) {
        this.diagnostics.add('warn', 'bridge', error);
      }
    }
  }

  private createLaunchDirectory(): string {
    const root = this.opts.bridgeRoot ?? join(tmpdir(), 'lingxi-electron-bridge');
    mkdirSync(root, { recursive: true, mode: 0o700 });
    chmodSync(root, 0o700);
    const directory = mkdtempSync(join(root, 'launch-'));
    chmodSync(directory, 0o700);
    this.launchDir = directory;
    return directory;
  }

  private spawnServer(launch: BridgeLaunchConfig, bridgeDir: string): ChildProcess {
    const bin = resolveServerBin(this.opts);
    const args = buildBridgeArguments({
      appVersion: this.opts.appVersion,
      workspace: launch.workspace,
      sessionId: this.sessionId,
      bridgeDir,
      sessionTranscriptPath: this.ownedTranscriptPath,
      additionalDirectories: launch.additionalDirectories,
      promptDirectories: launch.promptDirectories,
      initialProjectRoot: launch.initialProjectRoot,
      configHome: launch.configHome,
      globalConfigPath: launch.globalConfigPath,
      model: launch.model,
      hasApiKey: Boolean(launch.apiKey),
      hasCredentialStdin: Boolean(launch.openaiOAuth) || Object.keys(launch.providerCredentials ?? {}).length > 0
        || Object.keys(launch.pluginSecrets ?? {}).length > 0,
      trusted: launch.trusted,
      allowInsights: launch.allowInsights,
      allowTeamOnboarding: launch.allowTeamOnboarding,
      scheduledController: launch.scheduledController,
      remoteControlEnabled: launch.remoteControlEnabled,
      packagedCredentialBoundary: Boolean(this.opts.isPackaged),
    });

    const environment = { ...buildBridgeEnvironment(process.env, launch.apiBaseUrl), ...launch.settingsEnvironment };
    // The packaged Electron executable has Node mode, so Mod workers do not
    // depend on a separate system Node installation.
    environment.LINGXI_MOD_NODE_EXECUTABLE = process.execPath;
    // Mod UI source is compiled by Bun on the host side. Packaged builds use
    // the app-owned sidecar; development may pin an explicit Bun binary, and
    // otherwise leaves this unset so the engine resolves `bun` from PATH.
    const bunExecutable = resolveModBunExecutable({
      isPackaged: this.opts.isPackaged === true,
      resourcesPath: this.opts.resourcesPath ?? process.resourcesPath,
      configuredPath: process.env.LINGXI_MOD_BUN_EXECUTABLE,
    });
    if (bunExecutable) {
      environment.LINGXI_MOD_BUN_EXECUTABLE = bunExecutable;
    }
    const child = spawn(bin, args, {
      cwd: launch.workspace,
      env: environment,
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      detached: process.platform !== 'win32',
    });
    const providerCredentials = launch.providerCredentials ?? {};
    if (launch.openaiOAuth || Object.keys(providerCredentials).length > 0 || Object.keys(launch.pluginSecrets ?? {}).length > 0) {
      child.stdin?.end(buildCredentialEnvelope(launch));
    } else if (launch.apiKey) child.stdin?.end(`${launch.apiKey}\n`);
    else child.stdin?.end();
    return child;
  }

  listProviderCredentials(
    providerIds: readonly string[],
    previewProviderIds: readonly string[] = [],
  ): Promise<ProviderCredentialStatus> {
    const ids = providerIds.map((providerId) => this.validateProviderId(providerId));
    const previews = previewProviderIds.map((providerId) => this.validateProviderId(providerId));
    if (ids.length > 32) throw new Error('too many provider credentials requested');
    if (previews.some((providerId) => !ids.includes(providerId))) {
      throw new Error('credential preview provider must be included in the status query');
    }
    return this.requestCredentialOperation(ids, (operationId) => ({
      type: 'list_provider_credentials',
      operation_id: operationId,
      provider_ids: ids,
      ...(previews.length > 0 ? { preview_provider_ids: previews } : {}),
    }));
  }

  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialStatus> {
    const id = this.validateProviderId(providerId);
    if (!credential || credential.length > 16_384 || credential.includes('\0')) {
      throw new Error('invalid provider credential');
    }
    return this.requestCredentialOperation([id], (operationId) => ({
      type: 'set_provider_credential',
      operation_id: operationId,
      provider_id: id,
      credential,
    }));
  }

  deleteProviderCredential(providerId: string): Promise<ProviderCredentialStatus> {
    const id = this.validateProviderId(providerId);
    return this.requestCredentialOperation([id], (operationId) => ({
      type: 'delete_provider_credential',
      operation_id: operationId,
      provider_id: id,
    }));
  }

  testProviderConnection(
    providerId: string,
    apiBase: string,
    model: string,
    credentialOverride?: string,
  ): Promise<ProviderConnectionTestResult> {
    const id = this.validateProviderId(providerId);
    if (!apiBase || apiBase.length > 2_048 || apiBase.includes('\0')) throw new Error('invalid provider API base');
    if (model.length > 512 || model.includes('\0')) throw new Error('invalid provider model');
    if (credentialOverride !== undefined
      && (!credentialOverride.trim() || credentialOverride.length > 16_384 || credentialOverride.includes('\0'))) {
      throw new Error('invalid provider credential');
    }
    const client = this.client;
    if (!client) throw new Error(`bridge client not connected (state=${this.state.status})`);
    const operationId = this.nextCredentialOperationId;
    this.nextCredentialOperationId = Number.isSafeInteger(operationId + 1) ? operationId + 1 : 1;
    return new Promise<ProviderConnectionTestResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingProviderConnectionTests.delete(operationId);
        reject(new Error('provider connection test timed out'));
      }, 20_000);
      timer.unref();
      this.pendingProviderConnectionTests.set(operationId, { providerId: id, resolve, reject, timer });
      try {
        client.sendCommand({
          type: 'test_provider_connection',
          operation_id: operationId,
          provider_id: id,
          api_base: apiBase,
          model,
          ...(credentialOverride !== undefined ? { credential_override: credentialOverride } : {}),
        });
      } catch (error) {
        clearTimeout(timer);
        this.pendingProviderConnectionTests.delete(operationId);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  private requestCredentialOperation(
    providerIds: readonly string[],
    command: (operationId: number) => ClientCommand,
  ): Promise<ProviderCredentialStatus> {
    const client = this.client;
    if (!client) throw new Error(`bridge client not connected (state=${this.state.status})`);
    const operationId = this.nextCredentialOperationId;
    this.nextCredentialOperationId = Number.isSafeInteger(operationId + 1)
      ? operationId + 1
      : 1;

    return new Promise<ProviderCredentialStatus>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingCredentialOperations.delete(operationId);
        reject(new Error('provider credential operation timed out'));
      }, 10_000);
      timer.unref();
      this.pendingCredentialOperations.set(operationId, {
        providerIds: [...providerIds],
        resolve,
        reject,
        timer,
      });
      try {
        client.sendCommand(command(operationId));
      } catch (error) {
        clearTimeout(timer);
        this.pendingCredentialOperations.delete(operationId);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  private validateProviderId(providerId: string): string {
    if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(providerId)) {
      throw new Error('invalid provider id');
    }
    return providerId;
  }

  private captureLogs(child: ChildProcess, secrets: readonly string[] = []): void {
    const generation = this.generation;
    const record = (line: string, fallbackLevel: 'info' | 'error'): void => {
      const text = sanitizeDiagnostic(line.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, ''), secrets);
      if (!text) return;
      const level = /\bERROR\b/.test(text) ? 'error' : /\bWARN\b/.test(text) ? 'warn' : fallbackLevel;
      this.diagnostics.add(level, 'bridge', diagnosticEvent('engine_log', {
        sessionId: this.opts.sessionId,
        projectPath: this.opts.projectPath,
        generation,
        message: text,
      }), secrets);
    };
    const attach = (stream: NodeJS.ReadableStream | null, level: 'info' | 'error'): void => {
      if (!stream) return;
      let buffered = '';
      const flush = (): void => {
        record(buffered, level);
        buffered = '';
      };
      stream.on('data', (chunk: Buffer | string) => {
        buffered += chunk.toString();
        const lines = buffered.split(/\r?\n/);
        buffered = lines.pop() ?? '';
        for (const line of lines) {
          record(line, level);
        }
        if (buffered.length > 8_000) flush();
      });
      stream.on('end', flush);
    };
    attach(child.stdout, 'info');
    attach(child.stderr, 'error');
  }

  private waitForLockfile(dir: string, launch: BridgeLaunchConfig, generation: number): Promise<string> {
    const timeoutMs = this.opts.lockfileTimeoutMs ?? 15_000;
    const deadline = Date.now() + timeoutMs;
    return new Promise<string>((resolve, reject) => {
      const tick = (): void => {
        if (this.disposed || generation !== this.generation) return reject(new Error('bridge launch superseded'));
        if (!this.child) return reject(new Error('bridge-server exited before publishing a lockfile'));
        for (const name of lockfiles(dir).sort((left, right) => left.localeCompare(right))) {
          const path = join(dir, name);
          try {
            const metadata = lstatSync(path);
            if (!metadata.isFile()) throw new Error('bridge lockfile is not a regular file');
            if (metadata.size > 64 * 1024) throw new Error('bridge lockfile is too large');
            if (process.platform !== 'win32' && (metadata.mode & 0o077) !== 0) {
              throw new Error('bridge lockfile permissions are not owner-only');
            }
            if (process.getuid && metadata.uid !== process.getuid()) throw new Error('bridge lockfile owner mismatch');
            validateBridgeLockfile(JSON.parse(readFileSync(path, 'utf8')), this.child.pid!, launch.workspace);
            return resolve(path);
          } catch (error) {
            const failure = error instanceof Error ? error : new Error(String(error));
            if (!ignorableLockfileError(failure)) return reject(failure);
            this.diagnostics.add('warn', 'host', diagnosticEvent('lockfile_ignored', { file: path, reason: failure.message }));
          }
        }
        if (Date.now() >= deadline) return reject(new Error(`timed out waiting for bridge lockfile`));
        setTimeout(tick, 50);
      };
      tick();
    });
  }

  private wireClient(client: BridgeClient, generation: number): void {
    if (generation === this.generation && !this.disposed) this.client = client;
    // Restored usage is explicitly marked by the engine and may be separated
    // from SessionResumed by other connection events. Live deltas still need a turn.
    client.on('event', (event: ClientEvent) => {
      if (generation !== this.generation || client !== this.client || this.disposed) return;
      if (event.type === 'side_chat_created') { this.pendingSideCreates.delete(event.request_id); this.liveSideChats.add(event.side_id); }
      if (event.type === 'side_chat_error' && event.request_id) this.pendingSideCreates.delete(event.request_id);
      if (event.type === 'side_chat_list') { this.pendingSideCreates.clear(); this.liveSideChats.clear(); for (const id of event.side_ids) this.liveSideChats.add(id); }
      if (event.type === 'side_chat_ended') this.liveSideChats.delete(event.side_id);
      if (event.type.startsWith('side_chat_')) {
        this.broadcastClientEvent(event);
        this.notifyActivityChanged();
        return;
      }
      if (event.type === 'ui_control_result') {
        this.pendingModUiRequests.resolve(this.sessionId, event.request_id, event);
        return;
      }
      if (event.type === 'ui_client_frame') {
        try {
          const { runtimeId, frame } = parseUiFrameEvent(this.sessionId, event.runtime_id, event.frame_json);
          this.broadcastModUiEvent(CH_MOD_UI_FRAME, { runtimeId, frame });
        } catch {
          this.diagnostics.add('warn', 'bridge', 'dropped invalid Mod UI frame');
        }
        return;
      }
      if (event.type === 'ui_invalidate') {
        try {
          const invalidation = parseUiInvalidateEvent(
            this.sessionId, event.session_id, event.uuid, event.instances_json,
          );
          this.broadcastModUiEvent(CH_MOD_UI_INVALIDATE, {
            uuid: invalidation.uuid,
            ...(invalidation.instances === undefined ? {} : { instances: invalidation.instances }),
          });
        } catch {
          this.diagnostics.add('warn', 'bridge', 'dropped invalid Mod UI invalidation');
        }
        return;
      }
      if (event.type === 'realtime_audio_event') {
        if (event.session_id === this.sessionId) for (const callback of this.realtimeAudioSubscribers) callback(event.event_json);
        return;
      }
      if (event.type === 'audio_session_context') {
        if (event.session_id === this.sessionId) {
          this.cachedAudioSessionContext = { sessionId: event.session_id, profileId: event.profile_id, accountScope: event.account_scope };
          void this.opts.audioService?.refreshHostedCapabilities?.().catch(() => undefined);
        }
      }
      if (event.type === 'model_changed') {
        this.cachedAudioSessionContext = undefined;
        client.sendCommand({ type: 'get_audio_session_context' });
      }
      if (event.type === 'audio_request') {
        void this.dispatchAudioRequest(client, generation, event.request);
        return;
      }
      if (event.type === 'audio_cancel') {
        const identityKey = audioIdentityKey(event.identity);
        const start = this.audioStartsByGeneration.get(generation)?.get(identityKey);
        if (start) this.audioStartsByGeneration.get(generation)?.delete(identityKey);
        this.audioRequestsByGeneration.get(generation)?.delete(audioIdentityKey(event.identity));
        void this.opts.audioService?.cancelAudioRequest(event.identity).catch((error) => {
          this.diagnostics.add('warn', 'bridge', `native audio cancellation failed: ${sanitizeDiagnostic(error)}`);
        });
        if (start) void this.opts.audioService?.endAudioOwner(start.owner).catch((error) => {
          this.diagnostics.add('warn', 'bridge', `cancelled recording rollback failed: ${sanitizeDiagnostic(error)}`);
        });
        return;
      }
      const resumedUsage = event.type === 'usage_update' && event.is_snapshot === true;
      if (event.type === 'turn_ended') this.lastKnownCost = event.cost;
      else if (event.type === 'cost_update') {
        const { type: _type, ...cost } = event;
        this.lastKnownCost = cost;
      }
      this.gitActivity.accept(event);
      if (event.type === 'openai_oauth_updated') {
        // A sign-in made inside this engine. The host adopts it and serves it
        // to every engine, this one included, from then on.
        this.oauthPersistence = this.oauthPersistence.then(async () => {
          if (!this.opts.onOpenAiOAuthUpdated) throw new Error('OAuth persistence unavailable');
          await this.opts.onOpenAiOAuthUpdated(event.session);
        }).catch(() => {
          // Never include token-bearing event or arbitrary broker error text.
          this.diagnostics.add('error', 'host', 'Failed to persist Codex authentication. Sign in again.');
          this.broadcastClientEvent({ type: 'error', kind: { type: 'internal' }, message: 'Failed to persist Codex authentication. Sign in again.' });
        });
        return;
      }
      if (event.type === 'cron_run_bound') {
        const pending = this.pendingRunBindings.get(event.run_id);
        if (pending) {
          clearTimeout(pending.timer); this.pendingRunBindings.delete(event.run_id);
          if (event.error) pending.reject(new Error(event.error)); else pending.resolve();
        }
        return;
      }
      if (event.type === 'model_list') for (const waiter of this.modelCatalogWaiters) waiter(event);
      if (event.type === 'scheduled_run_finished') {
        const pending = this.pendingScheduledTurns.get(event.run_id);
        if (pending) {
          this.pendingScheduledTurns.delete(event.run_id);
          if (!event.error?.startsWith('busy:')) this.activeTurn = false;
          if (event.error) pending.reject(new Error(event.error));
          else pending.resolve(event.summary ?? '');
          this.notifyActivityChanged();
        }
        return;
      }
      if (event.type === 'cron_run_requested') {
        this.activeCronExecutions++;
        this.notifyActivityChanged();
        void (async () => {
          let response: Extract<ClientCommand, { type: 'cron_run_completed' }>;
          let runIdentity: ReturnType<typeof scheduledRunIdentity> | undefined;
          try {
            runIdentity = scheduledRunIdentity(event.run_id, event.task);
            if (!this.opts.onCronRunRequested) throw new Error('Scheduled execution is unavailable');
            const result = await this.opts.onCronRunRequested(this, event);
            response = { type: 'cron_run_completed', run_id: event.run_id, session_id: result.sessionId, summary: result.summary };
          } catch (error) {
            response = { type: 'cron_run_completed', run_id: event.run_id, error: error instanceof Error ? error.message : String(error) };
          }
          if (generation !== this.generation || client !== this.client) return;
          try {
            client.sendCommand(response);
            if (!runIdentity) return;
            // Keep the controller leased until the scheduler has committed the
            // result, including when the user paused the schedule mid-run.
            const deadline = Date.now() + 15_000;
            while (generation === this.generation && Date.now() < deadline) {
              const jobs = await this.manageCron({ action: 'history', id: event.task.id });
              const run = jobs.find((job) => job.id === event.task.id)?.automation?.runs?.find((item) => item.id === runIdentity.occurrenceId);
              if (!run || run.status !== 'running'
                || run.claimGeneration !== runIdentity.claimGeneration) break;
              await new Promise((resolve) => setTimeout(resolve, 50));
            }
          } catch { this.diagnostics.add('warn', 'bridge', 'Scheduled result connection closed before acknowledgement.'); }
        })().finally(() => { this.activeCronExecutions--; this.notifyActivityChanged(); });
        return;
      }
      const resumeOwner = this.currentResumeOwner();
      if (isTurnOwnedEvent(event) && !this.activeTurn && !resumeOwner && !resumedUsage) {
        this.diagnostics.add('warn', 'bridge', `dropped unowned turn event: ${event.type}`);
        return;
      }
      if (resumeOwner) {
        if (event.type === 'tool_use_started') resumeOwner.activeTools.add(event.id);
        else if (event.type === 'tool_use_result') resumeOwner.activeTools.delete(event.id);
        if (event.type === 'tool_use_started' || event.type === 'tool_heartbeat' || event.type === 'tool_use_result') resumeOwner.timer.refresh();
      }
      if (event.type === 'cron_result') {
        const pending = this.pendingCron.get(event.request_id);
        if (pending) {
          clearTimeout(pending.timer);
          this.pendingCron.delete(event.request_id);
          if (event.error) pending.reject(new Error(event.error));
          else {
            // Successful creation also persists an empty transcript anchor in the engine.
            // The owning chat must no longer be reused as an unsent draft.
            if (pending.creating && !this.sessionIdentityCommitted && this.opts.onFirstPromptSent?.() !== false) {
              this.sessionIdentityCommitted = true;
            }
            pending.resolve(event.jobs);
          }
          this.notifyActivityChanged();
        }
      }
      if (event.type === 'settings_snapshot') {
        this.configuredCustomProviderIds = [];
        try {
          const effective: unknown = JSON.parse(event.effective_json);
          this.credentialRoutingSettings = effective;
          this.pendingCredentialSettings?.resolve();
          const providers = effective && typeof effective === 'object' && !Array.isArray(effective)
            ? (effective as Record<string, unknown>).providers : undefined;
          if (providers && typeof providers === 'object' && !Array.isArray(providers)) {
            this.configuredCustomProviderIds = Object.entries(providers).filter(([id, profile]) =>
              /^[a-z0-9][a-z0-9._-]{0,63}$/.test(id)
              && profile !== null && typeof profile === 'object' && !Array.isArray(profile),
            ).map(([id]) => id);
          }
        } catch {
          this.credentialRoutingSettings = undefined;
          this.pendingCredentialSettings?.reject(new Error('Invalid provider settings snapshot.'));
          // An invalid snapshot must not keep stale credential authorization.
        }
        for (const providerId of this.configuredCredentialIds) {
          for (const resolve of this.customProviderWaiters.get(providerId) ?? []) resolve();
        }
      }
      if (event.type === 'provider_credential_status') {
        this.handleProviderCredentialStatus(event);
      }
      if (event.type === 'provider_connection_tested') {
        this.handleProviderConnectionTested(event);
      }
      if (event.type === 'session_resumed') {
        if (event.session_id !== this.sessionId) {
          this.rejectPendingSessionResume(new Error('engine resumed a different session id'));
          this.fail('engine resumed a different session id');
          return;
        }
        this.sessionHasHistory = true;
        this.sessionIdentityCommitted = true;
        if (this.pendingSessionResume) {
          this.pendingSessionResume.sessionResumed = true;
          this.completePendingSessionResumeIfReady();
        }
      }
      if (event.type === 'permission_mode_changed' && event.mode === this.pendingPermissionSwitch?.mode) {
        this.pendingPermissionSwitch.complete();
      }
      if (event.type === 'fast_mode_changed' && event.enabled === this.pendingFastModeSwitch?.enabled) {
        this.pendingFastModeSwitch.complete();
      }
      if (event.type === 'error' && event.message.startsWith('set_permission_mode failed:')) {
        this.pendingPermissionSwitch?.fail(new Error(sanitizeDiagnostic(event.message)));
      }
      if (event.type === 'error' && event.message.startsWith('set_fast_mode failed:')) {
        this.pendingFastModeSwitch?.fail(new Error(sanitizeDiagnostic(event.message)));
      }
      if (event.type === 'error') {
        this.diagnostics.add('error', 'bridge', diagnosticEvent('client_error', {
          sessionId: this.opts.sessionId,
          projectPath: this.opts.projectPath,
          generation,
          turnId: this.activeTurnId,
          kind: event.kind,
          message: event.message,
        }));
        this.pendingCredentialSettings?.reject(new Error('Provider settings loading failed.'));
        this.pendingModelSwitch?.fail(new Error('Model switch failed.'));
      }
      if (event.type === 'slash_command_result' && event.is_error) {
        this.diagnostics.add('error', 'bridge', diagnosticEvent('slash_command_error', {
          sessionId: this.opts.sessionId,
          projectPath: this.opts.projectPath,
          generation,
          message: event.display,
        }));
      }
      if (event.type === 'error' && this.pendingSessionResume) {
        this.rejectPendingSessionResume(new Error(sanitizeDiagnostic(event.message)));
      }
      if (event.type === 'turn_started') {
        this.computerAccessInTurn = false;
        if (!this.activeTurn) this.cancellingTurn = false;
        this.activeTurn = true;
        this.activeTurnGeneration = generation;
        this.activeTurnId = event.turn_id;
      }
      if (event.type === 'tool_use_started' && (event.tool === 'computer' || event.tool === 'browser')) this.computerAccessInTurn = true;
      if (event.type === 'turn_ended' || event.type === 'session_ended') {
        this.activeTurn = false;
        this.activeTurnGeneration = undefined;
        this.activeTurnId = undefined;
        this.cancellingTurn = false;
        this.clearTurnInteractions(event.type === 'session_ended');
      }
      if (event.type === 'slash_command_result' && this.pendingModelSwitch?.slash && this.pendingModelSwitch.sent && event.turn_id === this.pendingModelSwitch.turnId) {
        const pending = this.pendingModelSwitch;
        if (event.is_error) pending.fail(new Error('Model switch failed.'));
        else void this.scheduledModelCatalog().then(catalog => {
          if (this.pendingModelSwitch !== pending) return;
          // Bare model ids may be returned qualified with their actual provider.
          if (catalog.current === pending.model || catalog.current.includes('/') && catalog.current.slice(catalog.current.indexOf('/') + 1) === pending.model) {
            pending.complete(catalog.current);
          } else pending.fail(new Error('Model switch was not confirmed.'));
        }, error => pending.fail(error instanceof Error ? error : new Error(String(error))));
      }
      if (event.type === 'model_changed') {
        this.selectedModelReference = event.model;
        if (this.pendingModelSwitch?.sent && !this.pendingModelSwitch.slash && this.pendingModelSwitch.model === event.model) this.pendingModelSwitch.complete();
        try { this.opts.onModelChanged?.(event.model); }
        catch (error) { this.diagnostics.add('warn', 'host', error); }
        const pending = this.pendingSessionResume;
        if (pending) {
          pending.model = event.model;
          this.completePendingSessionResumeIfReady();
        } else {
          void this.ensureModelProviderCredential(event.model).catch((error: unknown) => {
            this.diagnostics.add('warn', 'host', `failed to load model provider credential: ${sanitizeDiagnostic(error)}`);
          });
        }
      }
      if (event.type === 'ask_user_question') {
        if (this.cancellingTurn) return;
        const requestId = event.request.request_id;
        if (
          !this.pendingAskUserQuestionIds.has(requestId)
          && this.pendingAskUserQuestionIds.size >= MAX_PENDING_ASK_USER_QUESTION
        ) {
          this.diagnostics.add('warn', 'bridge', 'AskUserQuestion request limit reached');
          return;
        }
        this.clearPendingAskUserQuestion(requestId);
        this.pendingAskUserQuestionIds.add(requestId);
        this.pendingAskUserQuestionRequests.set(requestId, {
          request: event.request,
        });
        this.notifyRemotePending();
      }
      if (event.type === 'ask_user_question_resolved') {
        this.clearPendingAskUserQuestion(event.request_id);
      }
      if (event.type === 'permission_request_resolved') {
        this.resolvedPermissionIds.add(event.request_id);
        this.clearPendingPermission(event.request_id);
      }
      if (event.type === 'tool_use_result' || event.type === 'ask_user_question_resolved' || event.type === 'permission_request_resolved') {
        this.completePendingSessionResumeIfReady();
      }
      if (
        event.type === 'turn_started'
        || event.type === 'turn_ended'
        || event.type === 'session_ended'
        || event.type === 'ask_user_question'
        || event.type === 'ask_user_question_resolved'
        || event.type === 'permission_request_resolved'
        || event.type === 'task_row'
        || event.type === 'task_status_changed'
        || event.type === 'task_lifecycle'
        || event.type === 'workflow_resumed'
        || event.type === 'session_agent_list'
        || event.type === 'session_agent_updated'
        || event.type === 'coordinator_worker'
        || event.type === 'coordinator_status'
      ) this.notifyActivityChanged();
      this.updateNotifier(event);
      this.broadcastClientEvent(event);
      // Renderer terminal cleanup is foreground-scoped. Re-deliver retained
      // background gates after that boundary using the existing permission IPC.
      if (event.type === 'turn_ended') this.replayBackgroundPermissions();
    });
    client.on('permission', (request: PermissionRequest) => {
      void this.receivePermissionRequest(client, generation, request).catch((error) => {
        this.diagnostics.add('warn', 'bridge', `permission forwarding failed: ${sanitizeDiagnostic(error)}`);
      });
    });
    client.on('computerAccess', (request: ComputerAccessRequestDto) => {
      if (generation !== this.generation || client !== this.client || this.disposed) return;
      // Same reasoning as the permission handler above: the engine is parked on
      // an answer, so a discarded request is a stall, not a no-op.
      if ((!this.activeTurn && !this.currentResumeOwner()) || this.cancellingTurn) {
        this.diagnostics.add('warn', 'bridge', `dropped computer access request ${request.request_id}: ${this.cancellingTurn ? 'turn is cancelling' : 'no active turn'}`);
        return;
      }
      if (Number.isSafeInteger(request.request_id) && request.request_id >= 0) {
        if (
          !this.pendingComputerAccessIds.has(request.request_id)
          && this.pendingComputerAccessIds.size >= MAX_PENDING_COMPUTER_ACCESS
        ) {
          this.diagnostics.add('warn', 'bridge', 'computer access request limit reached');
          return;
        }
        this.pendingComputerAccessIds.set(request.request_id, structuredClone(request));
        this.recordInteraction({ type: 'computer_access_request', request: structuredClone(request) });
        this.notifyRemotePending();
        this.notifyActivityChanged();
        this.broadcast(CH_COMPUTER_ACCESS, request);
      }
    });
    client.on('close', (code, reason) => {
      if (generation !== this.generation || client !== this.client || this.disposed) return;
      this.permissionScopeChecks.clear();
      void this.teardownAudioGeneration(generation);
      this.setState({ status: 'disconnected', reason: reason || `ws closed (code=${code})` });
    });
    client.on('error', (error) => {
      if (generation !== this.generation || client !== this.client || this.disposed) return;
      this.permissionScopeChecks.clear();
      void this.teardownAudioGeneration(generation);
      this.fail(error);
    });
  }

  private async dispatchAudioRequest(
    client: BridgeClient,
    generation: number,
    request: AudioOperationRequestDto,
  ): Promise<void> {
    const unavailable: AudioOperationResultDto = {
      type: 'failed',
      error: { kind: 'unavailable', message: 'the desktop audio service is unavailable' },
    };
    const requestKey = audioIdentityKey(request.identity);
    const requests = this.audioRequestsByGeneration.get(generation) ?? new Map<string, AudioOperationRequestDto>();
    if (requests.size >= MAX_TRACKED_AUDIO_REQUESTS_PER_RUNTIME && !requests.has(requestKey)) {
      try {
        client.sendCommand({ type: 'audio_response', identity: request.identity, result: {
          type: 'failed', error: { kind: 'busy', message: 'too many audio operations are pending for this runtime' },
        } });
      } catch { /* The connection is already gone. */ }
      return;
    }
    requests.set(requestKey, request);
    this.audioRequestsByGeneration.set(generation, requests);
    const starts = this.audioStartsByGeneration.get(generation) ?? new Map<string, AudioOperationRequestDto>();
    this.audioStartsByGeneration.set(generation, starts);
    const owners = this.audioOwnersByGeneration.get(generation) ?? new Map<string, AudioOwnerDto>();
    owners.set(audioOwnerKey(request.owner), request.owner);
    this.audioOwnersByGeneration.set(generation, owners);
    let result: AudioOperationResultDto = unavailable;
    try {
      result = this.opts.audioService
        ? await this.opts.audioService.executeAudioRequest(request)
        : unavailable;
    } catch (error) {
      result = {
        type: 'failed',
        error: { kind: 'native_failure', message: sanitizeDiagnostic(error) },
      };
    }
    requests.delete(requestKey);
    if (generation !== this.generation || client !== this.client || this.disposed || this.closedAudioGenerations.has(generation)) {
      await this.opts.audioService?.cancelAudioRequest(request.identity).catch(() => undefined);
      if (request.operation.type === 'start_recording' && result.type === 'recording_started') {
        await this.opts.audioService?.endAudioOwner(request.owner).catch(() => undefined);
      }
      return;
    }
    if (
      result.type !== 'failed'
      && (request.operation.type === 'end_owner'
        || request.operation.type === 'stop_recording'
        || request.operation.type === 'listen')
    ) {
      const ownerKey = audioOwnerKey(request.owner);
      owners.delete(ownerKey);
      for (const [identity, start] of starts) if (audioOwnerKey(start.owner) === ownerKey) starts.delete(identity);
    }
    if (request.operation.type === 'start_recording' && result.type === 'recording_started') starts.set(requestKey, request);
    try {
      client.sendCommand({ type: 'audio_response', identity: request.identity, result });
    } catch (error) {
      this.diagnostics.add('warn', 'bridge', `failed to return native audio result: ${sanitizeDiagnostic(error)}`);
      const rollbacks = [() => this.opts.audioService?.cancelAudioRequest(request.identity)];
      if (request.operation.type === 'start_recording' && result.type === 'recording_started') {
        rollbacks.push(() => this.opts.audioService?.endAudioOwner(request.owner));
      }
      // A closed result transport still owes cleanup of admitted native work.
      // Observe every rollback and attempt them independently, even if one
      // throws before returning its promise.
      const cleanup = await Promise.allSettled(rollbacks.map(async (rollback) => rollback()));
      for (const [index, outcome] of cleanup.entries()) {
        if (outcome.status === 'rejected') {
          this.diagnostics.add('warn', 'bridge', `native audio ${index === 0 ? 'cancellation' : 'recording owner teardown'} rollback failed: ${sanitizeDiagnostic(outcome.reason)}`);
        }
      }
    }
  }

  private async teardownAudioGeneration(generation: number): Promise<void> {
    this.closedAudioGenerations.add(generation);
    if (this.closedAudioGenerations.size > 64) this.closedAudioGenerations.delete(this.closedAudioGenerations.values().next().value!);
    const existing = this.audioCleanupGenerations.get(generation);
    if (existing) return existing;
    const requests = this.audioRequestsByGeneration.get(generation);
    const starts = this.audioStartsByGeneration.get(generation);
    const owners = this.audioOwnersByGeneration.get(generation);
    this.audioRequestsByGeneration.delete(generation);
    this.audioStartsByGeneration.delete(generation);
    this.audioOwnersByGeneration.delete(generation);
    const cleanup = Promise.allSettled([
      ...[...(requests?.values() ?? [])].map((request) => this.opts.audioService?.cancelAudioRequest(request.identity)),
      ...[...(starts?.values() ?? [])].map((request) => this.opts.audioService?.cancelAudioRequest(request.identity)),
      ...[...(owners?.values() ?? [])].map((owner) => this.opts.audioService?.endAudioOwner(owner)),
    ]).then(() => undefined);
    this.audioCleanupGenerations.set(generation, cleanup);
    await cleanup;
    this.audioCleanupGenerations.delete(generation);
  }

  private handleProviderCredentialStatus(event: ProviderCredentialStatus): void {
    if (event.operation_id === CODEX_TOKEN_REQUEST_OPERATION_ID) {
      void this.answerCodexTokenRequest().catch((error: unknown) => {
        this.diagnostics.add('warn', 'bridge', `Codex access token was not renewed: ${sanitizeDiagnostic(error)}`);
      });
      return;
    }
    const pending = this.pendingCredentialOperations.get(event.operation_id);
    if (pending) {
      clearTimeout(pending.timer);
      this.pendingCredentialOperations.delete(event.operation_id);
    }
    const configured = new Set(event.configured_provider_ids);
    const unavailable = new Set(event.unavailable_provider_ids ?? []);
    const previews = event.credential_previews ?? {};
    // The configured/unavailable id sets are authoritative regardless of whether
    // the originating promise is still pending — a LATE (post-timeout) event must
    // still fold into the cached state, or providerCredentialSnapshot() reports
    // engine-persisted CLI/TUI credentials as not configured. With no pending
    // entry the event only speaks to the providers it names.
    const scope = pending ? pending.providerIds : [...configured, ...unavailable];
    // Absence is only authoritative on a NON-error event: on an error the
    // enumeration may be partial, so a provider merely absent from the lists must
    // NOT have its persisted flag cleared (it may still hold credentials).
    const clearAbsent = !event.error;
    for (const providerId of scope) {
      if (unavailable.has(providerId)) continue;
      if (configured.has(providerId)) {
        this.persistedCredentialProviders.add(providerId);
        const preview = previews[providerId];
        if (preview) this.credentialPreviews.set(providerId, preview);
        // A status-only query intentionally omits previews so it can use an
        // attribute-only Keychain lookup. Preserve any previously fetched
        // suffix until an explicit preview query replaces it or deletion
        // clears the provider below.
      } else if (clearAbsent) {
        this.persistedCredentialProviders.delete(providerId);
        this.credentialPreviews.delete(providerId);
      }
    }
    this.credentialStorageEncrypted = event.storage_encrypted;
    this.activeCredentialProviders = new Set([
      ...this.runtimeCredentialProviders,
      ...this.persistedCredentialProviders,
    ]);
    if (this.state.status === 'connected') this.broadcast(CH_STATE_CHANGED, this.state);
    if (pending) {
      if (event.error) pending.reject(new Error(sanitizeDiagnostic(event.error)));
      else pending.resolve(event);
    }
  }

  private handleProviderConnectionTested(event: ProviderConnectionTestResult): void {
    const pending = this.pendingProviderConnectionTests.get(event.operation_id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pendingProviderConnectionTests.delete(event.operation_id);
    if (pending.providerId !== event.provider_id) {
      pending.reject(new Error('provider connection test returned a mismatched provider'));
      return;
    }
    pending.resolve(event);
  }

  sendPrompt(text: unknown, images: unknown = [], turnId?: unknown, visualizationContext?: unknown, source?: PromptSource): void | Promise<void> {
    const id = validateOptionalTurnId(turnId);
    if (this.changingWorkspace) throw new Error('This chat’s directory is being changed.');
    if (this.resumeOperationInProgress) throw new Error('This chat is being resumed.');
    if (this.ownedSessionResumeRequired) throw new Error('Resume this chat before sending another prompt.');
    if (this.archiving) throw new Error('This chat is being archived.');
    const prompt = validatePrompt(text);
    const validatedImages = validateImageRefs(images);
    const context = visualizationContext === undefined || visualizationContext === null
      ? undefined
      : parseVisualizationReference(visualizationContext);
    const mayNeedCodex = this.opts.resolveOpenAiOAuth !== undefined
      && (isCodexModel(this.selectedModelReference) || isCodexModel(this.pendingModelSwitch?.model));
    if (this.opts.resolveProviderCredential || mayNeedCodex) {
      const generation = this.generation;
      const client = this.requireClient();
      const token = Symbol('prompt hydration');
      this.pendingPromptHydrations.set(token, id);
      this.notifyActivityChanged();
      return (async () => {
        try {
          if (this.pendingModelSwitch) await this.pendingModelSwitch.promise;
          if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
          if (this.selectedModelReference) await this.ensureModelProviderCredential(this.selectedModelReference);
          if (isCodexModel(this.selectedModelReference) && this.opts.resolveOpenAiOAuth) await this.ensureCodexToken();
          // Optional Fusion routes must not gate a turn using a healthy main
          // model. Explicit /fusion still awaits these same cached loads.
          void this.ensureFusionProviderCredentials(false).catch(() => {
            this.diagnostics.add('warn', 'bridge', 'Optional Fusion credential preload failed.');
          });
          if (this.archiving || !this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) throw new Error('Prompt credential loading was interrupted.');
          this.sendPreparedPrompt(prompt, validatedImages, id, context, source);
        } finally {
          this.pendingPromptHydrations.delete(token);
          this.notifyActivityChanged();
        }
      })();
    }
    this.sendPreparedPrompt(prompt, validatedImages, id, context, source);
  }

  private sendPreparedPrompt(
    prompt: string,
    validatedImages: ReturnType<typeof validateImageRefs>,
    turnId?: number,
    visualizationContext?: VisualizationReference,
    source?: PromptSource,
  ): void {
    const needsIdentityCommit = !this.sessionIdentityCommitted;
    this.requireClient().sendPrompt(prompt, {
      images: validatedImages,
      ...(turnId !== undefined ? { turnId } : {}),
      ...(visualizationContext !== undefined ? { visualizationContext } : {}),
    });
    this.journalPrompt(prompt, validatedImages, turnId ?? 0, source);
    this.sessionHasHistory = true;
    if (needsIdentityCommit && this.opts.onFirstPromptSent?.() !== false) {
      this.sessionIdentityCommitted = true;
    }
    // Claim the local slot as soon as the command crossed the authenticated
    // bridge boundary. `turn_started` may arrive on a later event-loop tick;
    // without this pending owner an immediate Cancel (or permission request)
    // can fall through the same gap fixed in the Rust connection.
    if (!this.activeTurn) {
      this.activeTurn = true;
      this.activeTurnGeneration = this.generation;
      this.activeTurnId = turnId;
      this.cancellingTurn = false;
      // The bridge announces engine-initiated turns, but an ordinary submitted
      // prompt relies on its submitting client to open the turn. Passive remote
      // viewers need that boundary too, before any deltas can be rendered. Keep
      // it in the replay stream without replaying a second start to the sender.
      this.noteRemoteEvent(this.eventEnvelope({
        type: 'turn_started',
        ...(turnId !== undefined ? { turn_id: turnId } : {}),
      }, true));
      this.notifyActivityChanged();
    }
  }

  cancelTurn(turnId: unknown): void {
    const id = validateOptionalTurnId(turnId);
    this.requireClient().cancel(id);
    if (id === undefined || this.pendingModelSwitch?.turnId === id) this.pendingModelSwitch?.fail(new Error('Model switch was cancelled.'));
    let cancelledHydration = false;
    for (const [token, owner] of this.pendingPromptHydrations) {
      if (id === undefined || owner === id) {
        this.pendingPromptHydrations.delete(token);
        cancelledHydration = true;
      }
    }
    if (cancelledHydration) this.notifyActivityChanged();
    if (this.activeTurn && (id === undefined || id === this.activeTurnId)) {
      this.cancellingTurn = true;
      this.clearTurnInteractions();
      this.replayBackgroundPermissions();
      this.notifyActivityChanged();
    }
  }

  approvePermission(requestId: number, response?: unknown): void {
    const id = validateRequestId(requestId);
    const permissionResponse = validatePermissionResponse(response);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().approvePermission(id, permissionResponse);
    this.clearPendingPermission(id);
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  denyPermission(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().denyPermission(id);
    this.clearPendingPermission(id);
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  approveComputerAccess(requestId: number, response: unknown): void {
    const id = validateRequestId(requestId);
    const computerAccessResponse = validateComputerAccessResponse(response);
    const pending = this.pendingComputerAccessIds.get(id);
    if (!pending) throw new Error('computer access request is not pending');
    assertComputerAccessSubset(pending, computerAccessResponse);
    this.requireClient().approveComputerAccess(id, computerAccessResponse);
    this.pendingComputerAccessIds.delete(id);
    this.notifyRemotePending();
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  /** Policy mutation invalidates pending approvals and interrupts active native work. */
  revokeComputerAccess(): void {
    for (const id of [...this.pendingComputerAccessIds.keys()]) this.denyComputerAccess(id);
    if (this.activeTurn && this.computerAccessInTurn) this.cancelTurn(undefined);
  }

  denyComputerAccess(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
    this.requireClient().denyComputerAccess(id);
    this.pendingComputerAccessIds.delete(id);
    this.notifyRemotePending();
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  answerAskUserQuestion(requestId: number, answers: unknown): void {
    const id = validateRequestId(requestId);
    const validatedAnswers = validateAskUserQuestionAnswers(answers);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().answerAskUserQuestion(id, validatedAnswers);
    this.clearPendingAskUserQuestion(id);
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  cancelAskUserQuestion(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().cancelAskUserQuestion(id);
    this.clearPendingAskUserQuestion(id);
    this.completePendingSessionResumeIfReady();
    this.notifyActivityChanged();
  }

  /** Renderer UI control is a narrow typed lane, separate from generic `command()`. */
  async dispatchModUiControl(value: unknown): Promise<UiControlCallResultDto<NativeUiControlResponse>> {
    const request = validateModUiControlRequest(value);
    const result = await this.requestModUiResult((request_id) => buildNativeUiControlCommand(request, request_id));
    if (result.error !== undefined) {
      if (result.response_json !== undefined || result.metadata_json !== undefined) throw new Error('UI control result mixed error and response fields');
      this.diagnostics.add('warn', 'bridge', diagnosticEvent('mod_ui_control_failed', { subtype: request.subtype, reason: sanitizeDiagnostic(result.error).slice(0, 256) }));
      throw new Error(result.error);
    }
    if (result.response_json === undefined) throw new Error('UI control result is missing its response');
    const response = validateNativeUiControlResponseJson(request, result.response_json);
    let metadata: ReturnType<typeof validateUiControlMetadataJson> | undefined;
    if (result.metadata_json !== undefined) metadata = validateUiControlMetadataJson(result.metadata_json);
    const isNoHostRenderFallback = request.subtype === 'ui_render'
      && response !== null
      && 'tree' in response
      && response.tree === null
      && metadata === undefined;
    if (request.subtype === 'ui_render' && !isNoHostRenderFallback && metadata?.renderRevision === undefined) {
      throw new Error('UI render result is missing its render revision');
    }
    if (request.subtype !== 'ui_render' && metadata !== undefined) throw new Error('unexpected UI control metadata');
    return { response, ...(metadata === undefined ? {} : { metadata }) };
  }

  /** Renderer requests for Harness VM operations use the same session/request correlation. */
  async dispatchModUiOperation(value: unknown): Promise<UiClientOperationResponse> {
    const operation = validateModUiOperation(value);
    const result = await this.requestModUiResult((request_id) => ({
      type: 'ui_client_operation', request_id, operation_json: JSON.stringify(operation),
    }));
    if (result.error !== undefined) {
      if (result.response_json !== undefined || result.metadata_json !== undefined) throw new Error('UI operation result mixed error and response fields');
      throw new Error(result.error);
    }
    if (result.response_json === undefined || result.metadata_json !== undefined) throw new Error('invalid UI operation response envelope');
    return validateUiClientOperationResponseJson(operation, result.response_json);
  }

  private requestModUiResult(
    commandFor: (requestId: string) => ClientCommand,
  ): Promise<Extract<ClientEvent, { type: 'ui_control_result' }>> {
    if (this.outstandingModUiRequests < MAX_OUTSTANDING_MOD_UI_REQUESTS) return this.sendModUiRequest(commandFor);
    if (this.queuedModUiRequests.length >= 64) return Promise.reject(new Error('Mod UI request backlog is full.'));
    return new Promise((resolve, reject) => {
      this.queuedModUiRequests.push({ send: () => this.sendModUiRequest(commandFor).then(resolve, reject), reject });
    });
  }

  private sendModUiRequest(
    commandFor: (requestId: string) => ClientCommand,
  ): Promise<Extract<ClientEvent, { type: 'ui_control_result' }>> {
    this.outstandingModUiRequests += 1;
    const requestId = randomUUID();
    const response = this.pendingModUiRequests.request(this.sessionId, requestId, 30_000);
    try {
      this.requireClient().sendCommand(commandFor(requestId));
    } catch (error) {
      this.pendingModUiRequests.reject(
        this.sessionId,
        requestId,
        error instanceof Error ? error : new Error(String(error)),
      );
    }
    const settle = (): void => {
      this.outstandingModUiRequests -= 1;
      this.queuedModUiRequests.shift()?.send();
    };
    response.then(settle, settle);
    return response;
  }

  /**
   * Validate a renderer command and forward it to the engine. SECURITY:
   * `set_permission_mode: bypassPermissions` is gated behind explicit,
   * persisted acceptance (a blocking main-process dialog, shown once) — it is
   * NEVER one-click. A decline (or no confirmer wired) throws and the command
   * never reaches the engine; the renderer's mode display reads the engine's
   * actual (unchanged) mode, so nothing to revert.
   */
  async dispatchCommand(command: unknown, source?: PromptSource): Promise<void> {
    if (this.changingWorkspace) throw new Error('This chat’s directory is being changed.');
    if (this.resumeOperationInProgress) throw new Error('This chat is being resumed.');
    if (this.ownedSessionResumeRequired) throw new Error('Resume this chat before sending another command.');
    if (this.archiving) throw new Error('This chat is being archived.');
    const validated = validateClientCommand(command, this.activeWorkspace);
    if (validated.type.startsWith('side_chat_')) {
      const client = this.requireClient();
      if (validated.type === 'side_chat_create') this.pendingSideCreates.add(validated.request_id);
      try { client.sendCommand(validated); } catch (error) {
        if (validated.type === 'side_chat_create') this.pendingSideCreates.delete(validated.request_id);
        throw error;
      }
      this.notifyActivityChanged();
      return;
    }
    if (validated.type === 'cron_manage') {
      await this.sendCronCommand(validated);
      return;
    }
    if ((validated.type === 'set_model' || validated.type === 'run_slash_command') && this.pendingModelSwitch) throw new Error('A model switch is already in progress.');
    assertCommandAllowedDuringTurn(validated, this.turnActive);
    // The app signs in to Codex and refreshes that one session for every chat.
    // A chat that signed in by itself would refresh its own copy behind the
    // app, and two refreshers of one session invalidate each other's tokens.
    if (validated.type === 'run_slash_command' && this.opts.resolveOpenAiOAuth
      && /^\/connect\s+(?:chatgpt|openai-chatgpt)$/i.test(validated.raw.trim())) {
      throw new Error('Sign in to Codex from Settings. Every chat uses that sign-in.');
    }
    let fusion: ReturnType<typeof fusionInvocation> = null;
    if (validated.type === 'run_slash_command') {
      try { fusion = fusionInvocation(validated.raw); }
      catch (cause) {
        this.recordFusionPreparationFailure(validated, cause, source);
        return;
      }
    }
    if (validated.type === 'run_slash_command' && fusion
      && (this.opts.resolveProviderCredential || this.opts.resolveOpenAiOAuth)) {
      const generation = this.generation;
      const client = this.requireClient();
      const token = Symbol('fusion credential hydration');
      let pinnedRaw = validated.raw;
      this.pendingPromptHydrations.set(token, undefined);
      this.notifyActivityChanged();
      try {
        try {
          const selection = await this.resolveFusionStrategy(fusion.strategy);
          if (!this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) {
            throw new Error('Fusion credential loading was interrupted.');
          }
          // Bare --models entries belong to the conversation's actual profile.
          // Its qualified listing is authoritative even when the selected model
          // was supplied as an alias or an unqualified id.
          const parent = fusion.models?.some((model) => !model.includes(':'))
            ? await this.scheduledModelCatalog() : undefined;
          if (!this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) {
            throw new Error('Fusion credential loading was interrupted.');
          }
          const credentialSelection = fusionCredentialSelection(selection, fusion,
            parent ? (parent.current.includes('/') ? parent.current.slice(0, parent.current.indexOf('/'))
              : resolveProviderIdForModel(parent.current)) : undefined);
          await this.ensureFusionCodexToken(token, credentialSelection);
          await this.ensureFusionProviderCredentials(true, credentialSelection);
          if (parent && (await this.scheduledModelCatalog()).current !== parent.current) {
            throw new Error('The conversation model changed while preparing Fusion; submit the command again.');
          }
          if (this.archiving || !this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) {
            throw new Error('Fusion credential loading was interrupted.');
          }
          pinnedRaw = pinFusionStrategy(validated.raw, selection);
        } catch (cause) {
          // Keep failures in the originating session. Cancellation and external
          // restarts invalidate ownership; they must never publish into a new one.
          if (this.disposed || this.archiving || !this.pendingPromptHydrations.has(token)
            || generation !== this.generation || client !== this.client) throw cause;
          this.recordFusionPreparationFailure(validated, cause, source);
          return;
        }
        client.sendCommand({ ...validated, raw: pinnedRaw });
        this.commitFusionHistory(validated);
        this.journalSlashCommand(validated, source);
      } finally {
        this.pendingPromptHydrations.delete(token);
        this.notifyActivityChanged();
      }
      return;
    }
    // Preserve slash hook provenance and confirm the actual model before saving.
    if (validated.type === 'run_slash_command' && this.opts.onModelSelected) {
      const model = /^\/model\s+([\s\S]+)$/.exec(validated.raw.trim())?.[1]?.trim();
      if (model) return this.switchModel(model, true, validated);
    }
    if (validated.type === 'set_permission_mode' && validated.mode === 'bypassPermissions') {
      const accepted = (await this.opts.confirmBypassPermissions?.()) ?? false;
      if (!accepted) {
        throw new Error('Bypass Permissions mode was not accepted');
      }
    }
    if (validated.type === 'set_model' && (this.opts.resolveProviderCredential || this.opts.resolveOpenAiOAuth || this.opts.onModelSelected)) {
      return this.switchModel(validated.model);
    }
    // Other slash commands may run a Codex turn. Serving the token here is
    // best effort: a command that never needs it must still run.
    if (validated.type === 'run_slash_command' && isCodexModel(this.selectedModelReference) && this.opts.resolveOpenAiOAuth) {
      await this.ensureCodexToken().catch((error) => {
        this.diagnostics.add('warn', 'bridge', `Codex access token was not served for a slash command: ${sanitizeDiagnostic(error)}`);
      });
    }
    if (validated.type === 'set_permission_mode' && (this.opts.getSavedPermissionMode || this.opts.onPermissionModeSelected)) {
      return this.applyPermissionMode(validated.mode, true);
    }
    if (validated.type === 'set_fast_mode' && (this.opts.getSavedFastMode || this.opts.onFastModeSelected)) {
      return this.applyFastMode(validated.enabled, true);
    }
    this.requireClient().sendCommand(validated);
    this.commitFusionHistory(validated);
    this.journalSlashCommand(validated, source);
  }

  private commitFusionHistory(command: ClientCommand): void {
    if (command.type !== 'run_slash_command' || !/^\/fusion(?:\s|$)/i.test(command.raw.trim())) return;
    this.sessionHasHistory = true;
    if (!this.sessionIdentityCommitted && this.opts.onFirstPromptSent?.() !== false) this.sessionIdentityCommitted = true;
  }

  private recordFusionPreparationFailure(command: Extract<ClientCommand, { type: 'run_slash_command' }>, cause: unknown, source?: PromptSource): void {
    const error = cause instanceof Error ? cause.message : String(cause);
    this.requireClient().sendCommand({ type: 'fusion_admin', command: {
      action: 'record_failure', payload_json: JSON.stringify({ raw: command.raw, error }),
    } });
    this.commitFusionHistory(command);
    this.journalSlashCommand(command, source);
  }

  private resolveFusionStrategy(strategy?: FusionStrategyRefDto): Promise<FusionStrategySelectionDto> {
    const client = this.requireClient();
    const generation = this.generation;
    const operationId = ++this.nextFusionResolutionId;
    return new Promise((resolveSelection, reject) => {
      const finish = (error?: Error, selection?: FusionStrategySelectionDto) => {
        clearTimeout(timer);
        client.off('event', onEvent);
        if (error) reject(error);
        else if (selection) resolveSelection(selection);
      };
      const onEvent = (event: ClientEvent) => {
        if (event.type !== 'fusion_strategy_resolved' || event.operation_id !== operationId) return;
        if (generation !== this.generation || client !== this.client || this.disposed || this.archiving) {
          finish(new Error('Fusion strategy resolution was interrupted.'));
          return;
        }
        if (event.error || !event.selection_json) {
          finish(new Error(event.error || 'Choose a Fusion strategy before running.'));
          return;
        }
        try {
          const selection = JSON.parse(event.selection_json) as FusionStrategySelectionDto;
          if (!selection.reference || !['user', 'project', 'local'].includes(selection.reference.scope)
            || typeof selection.reference.id !== 'string' || typeof selection.revision !== 'string'
            || !selection.config || typeof selection.config !== 'object' || Array.isArray(selection.config)) {
            throw new Error('Invalid resolved Fusion strategy.');
          }
          finish(undefined, selection);
        } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
      };
      const timer = setTimeout(() => finish(new Error('Fusion strategy resolution timed out.')), 10_000);
      client.on('event', onEvent);
      try { client.sendCommand({ type: 'fusion_admin', command: {
        action: 'resolve_strategy', operation_id: operationId, payload_json: JSON.stringify({ strategy }),
      } }); } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
    });
  }

  private nextFusionResolutionId = Date.now();

  private async ensureProviderCredentialCached(providerId: string): Promise<void> {
    if (!this.opts.resolveProviderCredential) return;
    const existing = this.pendingRuntimeCredentialLoads.get(providerId);
    if (existing) return existing;
    const generation = this.generation;
    const client = this.requireClient();
    let loading!: Promise<void>;
    loading = (async () => {
      for (;;) {
        const writing = this.providerCredentialWrites.get(providerId);
        if (writing) await writing;
        if (generation !== this.generation || client !== this.client) {
          throw new Error('provider credential loading was interrupted');
        }
        if (this.runtimeCredentialProviders.has(providerId) && !this.invalidatedProviderCredentials.has(providerId)) return;
        const epoch = this.providerCredentialEpochs.get(providerId) ?? 0;
        let credential: string | undefined;
        try { credential = await this.opts.resolveProviderCredential!(providerId); }
        catch (error) {
          if (epoch !== (this.providerCredentialEpochs.get(providerId) ?? 0)) continue;
          throw error;
        }
        if (generation !== this.generation || client !== this.client) {
          throw new Error('provider credential loading was interrupted');
        }
        if (epoch !== (this.providerCredentialEpochs.get(providerId) ?? 0)) continue;
        try {
          if (credential) await this.cacheProviderCredential(providerId, credential);
          else if (this.invalidatedProviderCredentials.has(providerId)) await this.clearCachedProviderCredential(providerId);
        } catch (error) {
          if (epoch !== (this.providerCredentialEpochs.get(providerId) ?? 0)) continue;
          throw error;
        }
        if (epoch === (this.providerCredentialEpochs.get(providerId) ?? 0)) return;
      }
    })().finally(() => {
      if (this.pendingRuntimeCredentialLoads.get(providerId) === loading) {
        this.pendingRuntimeCredentialLoads.delete(providerId);
      }
    });
    this.pendingRuntimeCredentialLoads.set(providerId, loading);
    return loading;
  }

  private async ensureFusionProviderCredentials(explicit: boolean, selected?: FusionStrategySelectionDto): Promise<void> {
    if (!this.opts.resolveProviderCredential) return;
    const generation = this.generation;
    const client = this.requireClient();
    if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
    // Optional default-route warming never loads the other saved strategies.
    const selection = selected ?? await this.resolveFusionStrategy().then((resolved) => fusionCredentialSelection(resolved)).catch((error) => {
      if (explicit) throw error;
      return undefined;
    });
    if (!selection) return;
    if (generation !== this.generation || client !== this.client) throw new Error('Fusion credential loading was interrupted.');
    await Promise.all(resolveFusionCredentialProviderIds(this.credentialRoutingSettings, selection, explicit)
      .filter((providerId) => providerId !== 'openai-chatgpt')
      .map((providerId) => this.ensureProviderCredentialCached(providerId)));
  }

  private async ensureModelProviderCredential(model: string): Promise<void> {
    if (!this.opts.resolveProviderCredential) return;
    const generation = this.generation;
    const client = this.requireClient();
    if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
    if (generation !== this.generation || client !== this.client) throw new Error('Provider credential loading was interrupted.');
    await Promise.all(resolveModelCredentialProviderIds(model, this.credentialRoutingSettings)
      .map((providerId) => this.ensureProviderCredentialCached(providerId)));
  }

  private async ensureFusionCodexToken(token: symbol, selection: FusionStrategySelectionDto): Promise<void> {
    const generation = this.generation;
    const client = this.requireClient();
    const assertCurrent = () => {
      if (this.disposed || this.archiving || generation !== this.generation
        || client !== this.client || !this.pendingPromptHydrations.has(token)) {
        throw new Error('Fusion credential loading was interrupted.');
      }
    };
    if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
    assertCurrent();
    if (!resolveFusionCredentialProviderIds(this.credentialRoutingSettings, selection, true).includes(CODEX_PROVIDER_ID)) return;
    if (!this.opts.resolveOpenAiOAuth) return;
    const session = await this.opts.resolveOpenAiOAuth();
    assertCurrent();
    if (!session) throw new Error(CODEX_SIGN_IN_REQUIRED);
    await this.serveCodexToken(session);
    assertCurrent();
  }

  /** Give this engine the current Codex access token before Codex work. */
  private async ensureCodexToken(): Promise<void> {
    const session = await this.opts.resolveOpenAiOAuth!();
    if (!session) throw new Error(CODEX_SIGN_IN_REQUIRED);
    await this.serveCodexToken(session);
  }

  /**
   * The engine's Codex access token expired or was rejected. Serve a newer
   * one: the host's current token if this engine has not seen it yet,
   * otherwise a renewed one.
   */
  private async answerCodexTokenRequest(): Promise<void> {
    const current = await this.opts.resolveOpenAiOAuth?.();
    const next = current && current.access_token !== this.servedCodexToken
      ? current
      : await this.opts.renewOpenAiOAuth?.();
    if (next) await this.serveCodexToken(next);
  }

  /**
   * Serve `session`'s access token from this engine, or stop serving one.
   * Only a packaged engine takes it: an unpackaged one would store it as an
   * API key.
   */
  async serveCodexToken(session: OpenAiOAuthSession | undefined): Promise<void> {
    if (this.opts.isPackaged !== true || this.state.status !== 'connected') return;
    if (!session) {
      if (this.servedCodexToken === undefined) return;
      this.servedCodexToken = undefined;
      await this.deleteProviderCredential(CODEX_PROVIDER_ID);
      return;
    }
    if (session.access_token === this.servedCodexToken) return;
    const credential = JSON.stringify(codexAccessToken(session));
    await this.requestCredentialOperation([CODEX_PROVIDER_ID], (operationId) => ({
      type: 'set_provider_credential',
      operation_id: operationId,
      provider_id: CODEX_PROVIDER_ID,
      credential,
    }));
    this.servedCodexToken = session.access_token;
  }

  hasCachedProviderCredential(providerId: string): boolean {
    return this.runtimeCredentialProviders.has(providerId);
  }

  /** Invalidate slow reads as well as keys already acknowledged by the engine. */
  async refreshCachedProviderCredential(providerId: string, credential?: string): Promise<void> {
    this.providerCredentialEpochs.set(providerId, (this.providerCredentialEpochs.get(providerId) ?? 0) + 1);
    this.invalidatedProviderCredentials.add(providerId);
    // Keep the invalidation across transport loss; connectBridgeClient reconciles
    // a cached key before a recovered living engine can accept another prompt.
    if (this.state.status !== 'connected') return;
    if (credential !== undefined) await this.cacheProviderCredential(providerId, credential);
    else await this.clearCachedProviderCredential(providerId);
  }

  private writeCachedProviderCredential(providerId: string, credential: string | undefined): Promise<void> {
    const generation = this.generation;
    const client = this.client;
    if (!client) throw new Error(`bridge client not connected (state=${this.state.status})`);
    const epoch = this.providerCredentialEpochs.get(providerId) ?? 0;
    const assertCurrent = (): void => {
      if (generation !== this.generation || client !== this.client
        || epoch !== (this.providerCredentialEpochs.get(providerId) ?? 0)) {
        throw new Error('provider credential update was interrupted');
      }
    };
    const previous = this.providerCredentialWrites.get(providerId) ?? Promise.resolve();
    const operation = previous.catch(() => undefined).then(async () => {
      assertCurrent();
      const status = credential === undefined
        ? await this.deleteProviderCredential(providerId)
        : await this.setProviderCredential(providerId, credential);
      assertCurrent();
      if (status.configured_provider_ids.includes(providerId) !== (credential !== undefined)) {
        throw new Error('provider credential update was not confirmed');
      }
      this.persistedCredentialProviders.delete(providerId);
      if (credential === undefined) this.runtimeCredentialProviders.delete(providerId);
      else this.runtimeCredentialProviders.add(providerId);
      this.invalidatedProviderCredentials.delete(providerId);
      this.activeCredentialProviders = new Set([
        ...this.runtimeCredentialProviders,
        ...this.persistedCredentialProviders,
      ]);
    });
    const completion = operation.finally(() => {
      if (this.providerCredentialWrites.get(providerId) === completion) this.providerCredentialWrites.delete(providerId);
    });
    this.providerCredentialWrites.set(providerId, completion);
    return completion;
  }

  async cacheProviderCredential(providerId: string, credential: string): Promise<void> {
    await this.writeCachedProviderCredential(providerId, credential);
  }

  async clearCachedProviderCredential(providerId: string): Promise<void> {
    await this.writeCachedProviderCredential(providerId, undefined);
  }

  /** Remember a host-verified selection when reusing a connected runtime. */
  rememberTranscriptSelection(transcriptPath: string | undefined): void {
    this.ownedTranscriptPath = transcriptPath;
  }

  /** Resume the one session owned by this runtime and wait for engine proof. */
  resumeOwnedSession(transcriptPath?: string): Promise<void> {
    if (this.resumeOperationInProgress) return Promise.reject(new Error('session resume is already pending'));
    this.ownedSessionResumeRequired = true;
    const client = this.requireClient();
    const generation = this.generation;
    const selectedPath = transcriptPath ?? this.ownedTranscriptPath;
    let selectedResponse: Promise<void> | undefined;
    this.resumeOperationInProgress = true;
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.currentResumeOwner() === pending && (this.pendingInteractions > 0 || this.permissionScopeChecks.size > 0 || pending.activeTools.size > 0)) {
          timer.refresh();
          return;
        }
        this.rejectPendingSessionResume(new Error(`timed out resuming session ${this.sessionId}`), pending);
      }, this.opts.sessionResumeTimeoutMs ?? 15_000);
      timer.unref();
      const pending: PendingSessionResume = {
        sessionId: this.sessionId,
        generation,
        client,
        sessionResumed: false,
        responseReceived: !selectedPath,
        controller: new AbortController(),
        activeTools: new Set(),
        hydrationStarted: false,
        resolve,
        reject,
        timer,
      };
      this.pendingSessionResume = pending;
      this.notifyActivityChanged();
      try {
        if (selectedPath) {
          selectedResponse = client.resumeSessionFromTranscript(this.sessionId, selectedPath, pending.controller.signal);
          void selectedResponse.then(() => {
            if (this.currentResumeOwner() !== pending) return;
            pending.responseReceived = true;
            this.completePendingSessionResumeIfReady();
          }, () => undefined);
          void selectedResponse.catch((error: unknown) => {
            this.rejectPendingSessionResume(error instanceof Error ? error : new Error(String(error)), pending);
          });
        } else {
          client.sendCommand({ type: 'resume_session', session_id: this.sessionId,
            cwd: this.projectPath || this.activeWorkspace });
        }
      } catch (error) {
        this.rejectPendingSessionResume(error instanceof Error ? error : new Error(String(error)), pending);
      }
    }).then(async () => {
      if (selectedResponse) await selectedResponse;
      const assertCurrent = () => {
        if (generation !== this.generation || client !== this.client || this.disposed) throw new Error('session resume was interrupted');
      };
      assertCurrent();
      this.ownedTranscriptPath = selectedPath;
      await this.restoreModel();
      assertCurrent();
      if (this.remountControls) await this.restoreRemountControls();
      else {
        await this.restorePermissionMode();
        assertCurrent();
        await this.restoreFastMode();
      }
      assertCurrent();
      this.ownedSessionResumeRequired = false;
    }).finally(() => {
      this.resumeOperationInProgress = false;
      this.notifyActivityChanged();
    });
  }

  private completePendingSessionResumeIfReady(): void {
    const pending = this.pendingSessionResume;
    if (!pending?.sessionResumed || !pending.responseReceived || !pending.model || pending.hydrationStarted
      || pending.activeTools.size > 0 || this.pendingInteractions > 0 || this.permissionScopeChecks.size > 0) return;
    pending.hydrationStarted = true;
    // A credential that cannot be read does not keep the chat closed: the
    // next prompt that needs it loads it again or fails saying why.
    void this.ensureModelProviderCredential(pending.model).catch((error: unknown) => {
      this.diagnostics.add('warn', 'host', `failed to load model provider credential: ${sanitizeDiagnostic(error)}`);
    }).then(() => this.resolvePendingSessionResume(pending));
  }

  async restoreOwnedSessionIfNeeded(): Promise<void> {
    if (this.sessionHasHistory || this.ownedSessionResumeRequired) await this.resumeOwnedSession();
  }

  private resolvePendingSessionResume(pending: PendingSessionResume): void {
    if (this.pendingSessionResume !== pending || pending.sessionId !== this.sessionId
      || pending.generation !== this.generation || pending.client !== this.client || this.disposed) return;
    this.pendingSessionResume = null;
    clearTimeout(pending.timer);
    pending.controller.abort();
    this.clearTurnInteractions();
    this.notifyActivityChanged();
    pending.resolve();
  }

  private rejectPendingSessionResume(error: Error, owner?: PendingSessionResume): void {
    const pending = this.pendingSessionResume;
    if (!pending || (owner && (owner !== pending || owner.generation !== this.generation || owner.client !== this.client))) return;
    this.pendingSessionResume = null;
    clearTimeout(pending.timer);
    pending.controller.abort();
    this.clearTurnInteractions();
    this.notifyActivityChanged();
    pending.reject(error);
  }

  // ── Inline visualizations (see `visualization.ts`) ─────────────────────────

  visualizationMount(
    reference: VisualizationReference,
    theme: VisualizationThemeDto,
    locale: string,
    expanded: boolean,
  ): Promise<VisualizationMountDto | null> {
    return this.requireClient().visualizationMount(this.sessionId, reference, theme, locale, expanded);
  }

  visualizationServe(path: string): Promise<VisualizationServeDto> {
    return this.requireClient().visualizationServe(path);
  }

  visualizationWriteState(
    token: string,
    generation: number,
    baseVersion: number,
    modelContent: string,
    privateContent: string,
  ): Promise<VisualizationStateWriteDto> {
    return this.requireClient().visualizationWriteState(token, generation, baseVersion, modelContent, privateContent);
  }

  visualizationUnmount(token: string): Promise<void> {
    return this.requireClient().visualizationUnmount(token);
  }

  /** Whether this runtime can answer visualization requests right now. */
  get visualizationReady(): boolean {
    return this.client !== null;
  }

  private requireClient(): BridgeClient {
    this.refreshAccessState();
    if (!this.activeWorkspaceTrusted) throw new Error('workspace trust is required before using the engine');
    if (!this.client) throw new Error(`bridge client not connected (state=${this.state.status})`);
    return this.client;
  }

  private refreshAccessState(): void {
    const workspace = this.activeWorkspace;
    if (!workspace) {
      this.activeWorkspaceTrusted = false;
      return;
    }
    if (this.opts.accessState) {
      const snapshot = this.opts.accessState();
      const matchesWorkspace = snapshot.workspace === workspace;
      this.activeWorkspaceTrusted = matchesWorkspace && snapshot.trusted;
      return;
    }
    const launch = this.opts.launchConfig();
    if (launch instanceof Promise) return;
    const matchesWorkspace = launch.workspace === workspace;
    this.activeWorkspaceTrusted = matchesWorkspace && launch.trusted;
  }

  private broadcast(channel: string, payload: unknown): void {
    for (const webContents of this.targets.keys()) {
      if (webContents.isDestroyed()) this.targets.delete(webContents);
      else this.sendToWindow(webContents, channel, payload);
    }
  }

  /** Mod UI events always carry a session envelope. */
  private broadcastModUiEvent(channel: string, event: unknown): void {
    const envelope = { sessionId: this.sessionId, event };
    for (const webContents of this.targets.keys()) {
      if (webContents.isDestroyed()) this.targets.delete(webContents);
      else webContents.send(channel, envelope);
    }
  }

  private sendToWindow(webContents: WebContents, channel: string, payload: unknown): void {
    const value = this.opts.envelopeEvents
      ? { sessionId: this.sessionId, event: payload }
      : payload;
    webContents.send(channel, value);
  }

  private setState(next: ConnectionState): void {
    this.state = next;
    if (next.status === 'disconnected' || next.status === 'error' || next.status === 'idle') {
      this.disconnectedForegroundPending ||= this.activeTurn || this.pendingInteractions > 0 || this.pendingPromptHydrations.size > 0;
      this.pendingPermissionSwitch?.fail(new Error('Permission mode change was interrupted.'));
      this.pendingFastModeSwitch?.fail(new Error('Fast mode change was interrupted.'));
      this.pendingModelSwitch?.fail(new Error('Model switch was interrupted.'));
      // `runScheduledTurn` sets `activeTurn` by hand and only the
      // `scheduled_run_finished` handler clears it — and that handler needs the
      // pending entry this block is about to delete. Without this the latch
      // stays true forever, which blocks credential writes, Codex login, engine
      // restart and every rewriting git operation.
      if (this.pendingScheduledTurns.size > 0) {
        this.activeTurn = false;
        this.activeTurnGeneration = undefined;
        this.activeTurnId = undefined;
      }
      for (const pending of this.pendingScheduledTurns.values()) pending.reject(new Error('interrupted: Scheduled connection closed.'));
      this.pendingScheduledTurns.clear();
      for (const pending of this.pendingRunBindings.values()) { clearTimeout(pending.timer); pending.reject(new Error('interrupted: Scheduled connection closed.')); }
      this.pendingRunBindings.clear();
    }
    if (
      this.pendingSessionResume
      && (next.status === 'disconnected' || next.status === 'error' || next.status === 'idle')
    ) {
      this.rejectPendingSessionResume(new Error(
        next.status === 'error' ? next.message : `bridge became ${next.status} while resuming the session`,
      ));
    }
    this.diagnostics.add('info', 'host', connectionDiagnostic(next, this.generation));
    this.broadcast(CH_STATE_CHANGED, next);
    this.notifyRemote({ type: 'state', state: next });
    this.notifyActivityChanged();
  }

  private fail(error: unknown): void {
    const message = sanitizeDiagnostic(error);
    this.diagnostics.add('error', 'host', message);
    this.setState({ status: 'error', message });
  }

  private clearPendingConnectionOperations(): void {
    for (const queued of this.queuedModUiRequests.splice(0)) queued.reject(new Error('Mod UI request interrupted.'));
    this.pendingModUiRequests.rejectSession(this.sessionId, new Error('Mod UI request interrupted.'));
    for (const pending of this.pendingCron.values()) { clearTimeout(pending.timer); pending.reject(new Error('Scheduled task connection interrupted.')); }
    this.pendingCron.clear();
    for (const pending of this.pendingScheduledTurns.values()) pending.reject(new Error('Scheduled execution interrupted.'));
    this.pendingScheduledTurns.clear();
    for (const pending of this.pendingRunBindings.values()) { clearTimeout(pending.timer); pending.reject(new Error('Scheduled execution interrupted.')); }
    this.pendingRunBindings.clear();
    this.rejectPendingSessionResume(new Error('session resume was interrupted'));
    this.pendingPermissionSwitch?.fail(new Error('Permission mode change was interrupted.'));
    this.pendingFastModeSwitch?.fail(new Error('Fast mode change was interrupted.'));
    this.pendingModelSwitch?.fail(new Error('Model switch was interrupted.'));
    this.pendingCredentialSettings?.reject(new Error('Provider settings loading was interrupted.'));
    this.pendingCredentialSettings = undefined;
    this.credentialRoutingSettings = undefined;
    this.pendingPromptHydrations.clear();
    this.clearTurnInteractions(true);
    for (const pending of this.pendingCredentialOperations.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('bridge credential operation was interrupted'));
    }
    this.pendingCredentialOperations.clear();
    this.pendingRuntimeCredentialLoads.clear();
    this.providerCredentialWrites.clear();
    for (const pending of this.pendingProviderConnectionTests.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('provider connection test was interrupted'));
    }
    this.pendingProviderConnectionTests.clear();
  }

  private async stopBridge(): Promise<void> {
    const stoppingGeneration = this.generation;
    ++this.generation;
    await this.teardownAudioGeneration(stoppingGeneration);
    await this.oauthPersistence;
    this.servedCodexToken = undefined;
    this.liveSideChats.clear();
    this.pendingSideCreates.clear();
    this.connectionLockfilePath = null;
    this.clearPendingConnectionOperations();
    this.activeWorkspace = undefined;
    this.activeWorkspaceTrusted = false;
    this.runtimeCredentialProviders.clear();
    this.persistedCredentialProviders.clear();
    this.activeCredentialProviders.clear();
    this.credentialStorageEncrypted = false;
    this.gitActivity.reset();
    this.disconnectedForegroundPending = false;
    this.activeTurn = false;
    this.activeTurnGeneration = undefined;
    this.activeTurnId = undefined;
    this.cancellingTurn = false;
    const client = this.client;
    this.client = null;
    const closeClient = (): void => {
      try { client?.close(); } catch { /* a half-open channel can already be closed */ }
    };
    // Windows Node kill() forcibly terminates the process for every signal.
    // Keep the authenticated channel alive until a managed sidecar drains and
    // exits through its existing RequestExit command. External peers are only
    // disconnected; this runtime has no authority to shut them down.
    const ownsProcess = Boolean(this.child || (this.adoptedPid && this.adoptedProcessOwned));
    let requestedExit = false;
    if (client) {
      try {
        client.removeAllListeners();
        if (process.platform === 'win32' && ownsProcess) {
          client.on('error', (error) => this.diagnostics.add('warn', 'host', error));
          client.sendCommand({ type: 'request_exit' });
          requestedExit = true;
        } else closeClient();
      } catch (error) {
        this.diagnostics.add('warn', 'host', error);
        closeClient();
      }
    }

    const stopTimeoutMs = this.opts.stopTimeoutMs ?? (process.platform === 'win32' ? 30_000 : 2_000);
    const child = this.child;
    this.child = null;
    if (child && child.exitCode === null && child.signalCode === null) await new Promise<void>((resolve) => {
      let settled = false;
      let timer: NodeJS.Timeout | undefined;
      const finish = (): void => {
        if (settled) return;
        settled = true;
        if (timer) clearTimeout(timer);
        resolve();
      };
      child.once('exit', finish);
      if (!requestedExit) {
        try { this.signalChildTree(child, 'SIGINT'); } catch { finish(); return; }
      }
      timer = setTimeout(() => {
        try { this.signalChildTree(child, 'SIGKILL'); } catch { /* already gone */ }
        finish();
      }, stopTimeoutMs);
      timer.unref();
    });
    const adoptedPid = this.adoptedPid;
    const adoptedProcessOwned = this.adoptedProcessOwned;
    this.adoptedPid = null;
    this.adoptedProcessOwned = false;
    if (adoptedPid && adoptedProcessOwned) await this.stopAdoptedBridge(adoptedPid, requestedExit);
    if (requestedExit) closeClient();
    if (!adoptedPid || adoptedProcessOwned) this.removeLaunchDirectory();
    else this.launchDir = null;
  }

  private processIsAlive(pid: number): boolean {
    return processIsAlive(pid);
  }

  private async stopAdoptedBridge(pid: number, requestedExit = false): Promise<void> {
    const signal = (value: NodeJS.Signals): boolean => {
      try {
        if (process.platform !== 'win32') process.kill(-pid, value);
        else process.kill(pid, value);
        return true;
      } catch {
        return false;
      }
    };
    if (!requestedExit && !signal('SIGINT')) return;
    const deadline = Date.now() + (this.opts.stopTimeoutMs ?? (process.platform === 'win32' ? 30_000 : 2_000));
    while (this.processIsAlive(pid) && Date.now() < deadline) {
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, 50);
        timer.unref();
      });
    }
    if (this.processIsAlive(pid)) signal('SIGKILL');
  }

  private signalChildTree(child: ChildProcess, signal: NodeJS.Signals): void {
    if (process.platform !== 'win32' && child.pid) process.kill(-child.pid, signal);
    else child.kill(signal);
  }

  private removeLaunchDirectory(): void {
    const launchDir = this.launchDir;
    this.launchDir = null;
    if (launchDir) {
      try { rmSync(launchDir, { recursive: true, force: true }); } catch (error) { this.diagnostics.add('warn', 'host', error); }
    }
  }

  async dispose(options?: { replacing?: boolean }): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    await this.restartChain.catch(() => undefined);
    if (this.opts.envelopeEvents) {
      this.setState({ status: 'disconnected', reason: options?.replacing ? 'session runtime replaced' : SESSION_RUNTIME_DISPOSED_REASON });
    }
    await this.stopBridge();
    this.unsubscribeAudioService?.();
    this.targets.clear();
    this.notifyRemote({ type: 'disposed' });
    this.remoteObservers.clear();
  }
}
