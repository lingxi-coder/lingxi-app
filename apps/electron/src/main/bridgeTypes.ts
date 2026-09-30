import type {
  AudioCapabilitySnapshotDto,
  AudioOperationIdDto,
  AudioOperationRequestDto,
  AudioOperationResultDto,
  AudioOwnerDto,
  ClientEvent,
  PermissionModeId,
} from '@lingxi/bridge-client';
import type { SessionRuntime } from './bridge.js';
import type { ProcessCommand, listProcessCommands, readProcessCommand } from './bridgeDiscovery.js';
import { DiagnosticBuffer } from './host-utils.js';
import type { OpenAiOAuthSession } from './host-utils.js';
import type { HostNotifier } from './notifications.js';

export type { HostPermissionRequest } from '../shared/permission.js';

export interface DesktopAudioService {
  getCapabilities(): AudioCapabilitySnapshotDto;
  initializeCapabilities(): Promise<void>;
  executeAudioRequest(request: AudioOperationRequestDto): Promise<AudioOperationResultDto>;
  cancelAudioRequest(identity: AudioOperationIdDto): Promise<void>;
  endAudioOwner(owner: AudioOwnerDto): Promise<void>;
  onEvent(callback: (event: { type: string; snapshot?: { capabilities?: AudioCapabilitySnapshotDto } }) => void): () => void;
}

export type ConnectionState =
  | { status: 'idle' }
  | { status: 'spawning' }
  | { status: 'restarting' }
  | { status: 'connecting' }
  | { status: 'connected' }
  | { status: 'disconnected'; reason?: string }
  | { status: 'error'; message: string };

export interface BridgeLaunchConfig {
  workspace: string;
  /** Stable identity for the engine process and its persisted transcript. */
  sessionId?: string;
  apiKey?: string;
  providerCredentials?: Record<string, string>;
  openaiOAuth?: OpenAiOAuthSession;
  /** Broker-owned sensitive plugin options, passed only over child stdin. */
  pluginSecrets?: Record<string, Record<string, string>>;
  trusted: boolean;
  scheduledController?: boolean;
  model?: string;
  apiBaseUrl?: string;
}

export interface BridgeRuntimeVersions {
  serverName: string;
  serverProtocol: string;
  clientProtocol: string;
}

export interface BridgeManagerOptions {
  /** Development-only binary override. Ignored in packaged builds. */
  serverBin?: string;
  /** Parent for owner-only, per-launch discovery directories. */
  bridgeRoot?: string;
  lockfileTimeoutMs?: number;
  sessionResumeTimeoutMs?: number;
  stopTimeoutMs?: number;
  isPackaged?: boolean;
  resourcesPath?: string;
  launchConfig: () => BridgeLaunchConfig | Promise<BridgeLaunchConfig>;
  /** Provider ids whose shared secure-store status is cached after connect. */
  providerIds?: readonly string[];
  /** Synchronous trust snapshot used by privileged IPC checks. */
  accessState?: () => { workspace?: string; trusted: boolean };
  /** Stable session identity owned by the Electron host. */
  sessionId?: string;
  /** Project path associated with this runtime. */
  projectPath?: string;
  /** Wrap renderer-bound payloads in `{ sessionId, event }`. */
  envelopeEvents?: boolean;
  /** Direct unit-test/legacy mode can keep the old runtime IPC registration. */
  registerIpc?: boolean;
  diagnostics?: DiagnosticBuffer;
  /** Main-process device service. Engine audio is dispatched here directly. */
  audioService?: DesktopAudioService;
  /** Internal process-inspection seam used to recover detached Desktop runtimes after a main-process restart. */
  readProcessCommand?: (pid: number) => string | undefined;
  /** Internal process-table seam used to attach sessions still owned by another local Desktop/test host. */
  listProcessCommands?: () => readonly ProcessCommand[];
  onCronRunRequested?: (runtime: SessionRuntime, event: Extract<ClientEvent, { type: 'cron_run_requested' }>) => Promise<{ sessionId: string; summary: string }>;
  onModelChanged?: (model: string) => void;
  /** Persist only an explicitly requested, engine-confirmed model selection. */
  onModelSelected?: (model: string) => void;
  /** This session’s explicit selection; never the default for new sessions. */
  getSavedModel?: () => string | undefined;
  getSavedPermissionMode?: () => PermissionModeId | undefined;
  onPermissionModeSelected?: (mode: PermissionModeId) => void;
  getSavedFastMode?: () => boolean | undefined;
  onFastModeSelected?: (enabled: boolean) => void;
  /**
   * OS notifications. Lives here rather than in the renderer because the
   * renderer's session denies every Web permission but `media`, and because
   * `permission_request` never reaches the renderer as a `ClientEvent` — it
   * is a separate `Frame` arm handled by `client.on('permission')` below.
   */
  notifier?: HostNotifier;
  /** Resolve a broker-owned credential only when this session first selects its provider. */
  resolveProviderCredential?: (providerId: string) => Promise<string | undefined>;
  resolveOpenAiOAuth?: () => Promise<OpenAiOAuthSession | undefined>;
  onOpenAiOAuthUpdated?: (session: OpenAiOAuthSession) => Promise<void>;
  beforeOpenAiOAuthLaunch?: () => Promise<void>;
  /** Internal cache hook used by SessionRuntimeManager; never exposed to renderer IPC. */
  onActivityChanged?: () => void;
  /** Invalidate Electron-main launch material after a credential/config mutation. */
  invalidateLaunchConfigCache?: () => void;
  onFirstPromptSent?: () => boolean | void;
  /** SECURITY: consulted before a `set_permission_mode: bypassPermissions`
   * command is forwarded to the engine. Must show a blocking acceptance dialog
   * (once — persisted) and resolve `true` only on explicit consent. When absent,
   * bypassPermissions is refused (never one-click enabled). */
  confirmBypassPermissions?: () => Promise<boolean>;
}

export type ProviderConnectionTestResult = Extract<ClientEvent, { type: 'provider_connection_tested' }>;

export interface SessionRef {
  projectPath: string;
  sessionId: string;
}

export interface RuntimeEventEnvelope<T = unknown> {
  sessionId: string;
  event: T;
}

export interface SequencedRuntimeEventEnvelope<T = unknown> extends RuntimeEventEnvelope<T> {
  sequence: number;
}

export interface SessionRuntimeSummary {
  projectPath: string;
  sessionId: string;
  connection: ConnectionState;
  turnActive: boolean;
  pendingInteractions: number;
  pendingAskUserQuestions: number;
  runtimeVersions?: BridgeRuntimeVersions;
}

export interface SessionRuntimeManagerOptions extends Omit<BridgeManagerOptions, 'launchConfig' | 'accessState' | 'onModelChanged' | 'onModelSelected' | 'getSavedModel' | 'onFirstPromptSent' | 'onActivityChanged' | 'sessionId' | 'projectPath' | 'envelopeEvents' | 'registerIpc'> {
  launchConfig: (ref: SessionRef, resumeModel?: string) => BridgeLaunchConfig | Promise<BridgeLaunchConfig>;
  accessState?: (ref: SessionRef) => { workspace?: string; trusted: boolean };
  onModelChanged?: (ref: SessionRef, model: string) => void;
  onModelSelected?: (ref: SessionRef, model: string) => void;
  getSavedModel?: (ref: SessionRef) => string | undefined;
  onFirstPromptSent?: (ref: SessionRef) => boolean | void;
  /** Maximum retained runtimes when enough idle sessions are evictable. */
  maxCachedRuntimes?: number;
}
