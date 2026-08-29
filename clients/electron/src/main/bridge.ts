import { spawn, type ChildProcess } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { IpcMainInvokeEvent, WebContents } from 'electron';
import {
  type AskUserQuestionRequestDto,
  BridgeClient,
  type ClientCommand,
  type ClientEvent,
  type ComputerAccessRequestDto,
  type PermissionRequest,
} from '@lingxi/bridge-client';

import { buildBridgeArguments, buildBridgeEnvironment, buildCredentialEnvelope, diagnosticEvent, DiagnosticBuffer, sanitizeDiagnostic } from './host-utils.js';
import {
  assertCommandAllowedDuringTurn,
  validateClientCommand,
  validateAskUserQuestionAnswers,
  validateBridgeLockfile,
  validateComputerAccessResponse,
  validateOptionalTurnId,
  validatePermissionResponse,
  validateImageRefs,
  validatePrompt,
  validateRequestId,
} from './validation.js';

export const CH_SEND_PROMPT = 'lingxi:sendPrompt';
export const CH_APPROVE = 'lingxi:approve';
export const CH_DENY = 'lingxi:deny';
export const CH_APPROVE_COMPUTER_ACCESS = 'lingxi:approveComputerAccess';
export const CH_DENY_COMPUTER_ACCESS = 'lingxi:denyComputerAccess';
export const CH_ANSWER_ASK_USER_QUESTION = 'lingxi:answerAskUserQuestion';
export const CH_CANCEL_ASK_USER_QUESTION = 'lingxi:cancelAskUserQuestion';
export const CH_CANCEL = 'lingxi:cancel';
export const CH_COMMAND = 'lingxi:command';
export const CH_CONNECTION_STATE = 'lingxi:connectionState';
export const CH_EVENT = 'lingxi:event';
export const CH_EVENT_REPLAY = 'lingxi:event:replay';
export const CH_PERMISSION = 'lingxi:permission';
export const CH_COMPUTER_ACCESS = 'lingxi:computerAccess';
export const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';

const SESSION_RUNTIME_DISPOSED_REASON = 'session runtime disposed';

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
  trusted: boolean;
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
  onModelChanged?: (model: string) => void;
  /** SECURITY: consulted before a `set_permission_mode: bypassPermissions`
   * command is forwarded to the engine. Must show a blocking acceptance dialog
   * (once — persisted) and resolve `true` only on explicit consent. When absent,
   * bypassPermissions is refused (never one-click enabled). */
  confirmBypassPermissions?: () => Promise<boolean>;
}

type ProviderCredentialStatus = Extract<ClientEvent, { type: 'provider_credential_status' }>;

interface PendingCredentialOperation {
  providerIds: readonly string[];
  resolve: (status: ProviderCredentialStatus) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

interface PendingAskUserQuestionRequest {
  request: AskUserQuestionRequestDto;
}

interface PendingSessionResume {
  sessionId: string;
  generation: number;
  resolve: () => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

const SERVER_BIN_NAME = process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
const MAX_PENDING_PERMISSIONS = 1_000;
const MAX_PENDING_COMPUTER_ACCESS = 1_000;
const MAX_PENDING_ASK_USER_QUESTION = 1_000;
const require = createRequire(import.meta.url);
const electronModule = require('electron');
const ipcMain = (typeof electronModule === 'string' ? undefined : electronModule.ipcMain) ?? {
  handle: () => { throw new Error('ipcMain is unavailable outside Electron'); },
  removeHandler: () => undefined,
};

function moduleDir(): string {
  try {
    return dirname(fileURLToPath(import.meta.url));
  } catch {
    return process.cwd();
  }
}

function findWorkspaceServerBin(start: string): string | undefined {
  let dir = start;
  for (;;) {
    for (const profile of ['debug', 'release']) {
      const candidate = join(dir, 'lingxi-code', 'target', profile, SERVER_BIN_NAME);
      if (existsSync(candidate)) return candidate;
    }
    const parent = dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

/** Packaged builds use only process.resourcesPath; overrides/search are development-only. */
export function resolveServerBin(opts: Omit<BridgeManagerOptions, 'launchConfig'> = {}): string {
  if (opts.isPackaged) {
    const resourcesPath = opts.resourcesPath ?? process.resourcesPath;
    const packagedBinary = join(resourcesPath, 'bin', SERVER_BIN_NAME);
    if (existsSync(packagedBinary)) return packagedBinary;
    throw new Error(`packaged bridge-server is missing from application resources`);
  }
  const explicit = opts.serverBin ?? process.env['LINGXI_BRIDGE_SERVER_BIN'];
  if (explicit) return explicit;
  const found = findWorkspaceServerBin(moduleDir());
  if (found) return found;
  throw new Error('bridge-server binary not found; build it or set LINGXI_BRIDGE_SERVER_BIN in development');
}

function lockfiles(dir: string): string[] {
  try {
    return readdirSync(dir).filter((name) => name.endsWith('.lock') && !name.startsWith('.'));
  } catch {
    return [];
  }
}

function urlOrigin(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'file:' ? 'file://' : url.origin;
  } catch {
    return undefined;
  }
}

function connectionDiagnostic(state: ConnectionState, generation: number): string {
  return diagnosticEvent('connection_state', { generation, state });
}

function childExitDiagnostic(code: number | null, signal: NodeJS.Signals | null, generation: number): string {
  return diagnosticEvent('child_exit', {
    clean: signal === null && code === 0,
    code,
    generation,
    signal,
  });
}

function bridgeVersionDiagnostic(server: string, serverProtocol: string, clientProtocol: string): string {
  return diagnosticEvent('bridge_handshake', {
    clientProtocol,
    server,
    serverProtocol,
  });
}

function ignorableLockfileError(input: unknown): boolean {
  if (!(input instanceof Error)) return false;
  return (input as NodeJS.ErrnoException).code === 'ENOENT'
    || input instanceof SyntaxError
    || /bridge lockfile|JSON/.test(input.message);
}

export function isTurnOwnedEvent(event: ClientEvent): boolean {
  return [
    'ask_user_question',
    'thinking_delta',
    'tool_use_started',
    'tool_heartbeat',
    'tool_use_result',
    'message_complete',
    'cost_update',
    'coordinator_status',
    'coordinator_worker',
    'usage_update',
    'api_retry',
  ].includes(event.type);
}

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

const TRANSCRIPT_REPLAY_BASE_EVENTS = new Set<ClientEvent['type']>([
  'session_started',
  'session_resumed',
]);

/**
 * Engine events that must be answered EXACTLY ONCE, and therefore go to a
 * single renderer rather than to every registered one.
 *
 * `audio_request` is not a notification. `audio_bridge.rs` parks the engine
 * call waiting for one `audio_response` (5s / 30s / 180s per op). Broadcast to
 * N windows, each renderer would service it independently: N calls to
 * `getUserMedia`, N real recordings, N answers. The engine drops all but the
 * first, so the WIRE looks correct and nothing reports a problem — but the
 * DEVICE is wrong, and the user sees two recording indicators.
 *
 * Only one `BrowserWindow` exists today (`main/index.ts`), which is precisely
 * why this is enforced structurally instead of noted in a comment: whoever
 * adds a second window will not be looking for this, and the symptom would
 * appear at the microphone rather than in any test or log. Any future
 * engine->client request that expects a single reply belongs in this set.
 */
const SINGLE_RESPONDER_EVENTS = new Set<ClientEvent['type']>(['audio_request']);

const TRANSCRIPT_REPLAY_EVENTS = new Set<ClientEvent['type']>([
  'turn_started',
  'turn_ended',
  'text_delta',
  'thinking_delta',
  'tool_use_started',
  'tool_heartbeat',
  'tool_use_result',
  'plan_updated',
  'message_complete',
  'usage_update',
  'error',
  'system_notice',
  'session_ended',
]);

export interface SessionRuntimeSummary {
  projectPath: string;
  sessionId: string;
  connection: ConnectionState;
  turnActive: boolean;
  pendingInteractions: number;
  pendingAskUserQuestions: number;
  runtimeVersions?: BridgeRuntimeVersions;
}

export interface SessionRuntimeManagerOptions extends Omit<BridgeManagerOptions, 'launchConfig' | 'accessState' | 'onModelChanged' | 'sessionId' | 'projectPath' | 'envelopeEvents' | 'registerIpc'> {
  launchConfig: (ref: SessionRef) => BridgeLaunchConfig | Promise<BridgeLaunchConfig>;
  accessState?: (ref: SessionRef) => { workspace?: string; trusted: boolean };
  onModelChanged?: (ref: SessionRef, model: string) => void;
  sessionIdAvailable?: (ref: SessionRef) => boolean | Promise<boolean>;
}

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function isSessionId(value: unknown): value is string {
  return typeof value === 'string' && SESSION_ID_PATTERN.test(value);
}

function assertSessionRef(ref: SessionRef): void {
  if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
  if (typeof ref.projectPath !== 'string' || ref.projectPath.length === 0 || ref.projectPath.length > 32_768 || ref.projectPath.includes('\0')) {
    throw new Error('invalid project path');
  }
}

export class SessionRuntime {
  readonly sessionId: string;
  readonly projectPath: string;
  private child: ChildProcess | null = null;
  private client: BridgeClient | null = null;
  private state: ConnectionState = { status: 'idle' };
  private disposed = false;
  private ipcRegistered = false;
  private restartChain: Promise<void> = Promise.resolve();
  private generation = 0;
  private launchDir: string | null = null;
  private activeWorkspace: string | undefined;
  private activeWorkspaceTrusted = false;
  private runtimeCredentialProviders = new Set<string>();
  private persistedCredentialProviders = new Set<string>();
  private activeCredentialProviders = new Set<string>();
  private credentialStorageEncrypted = false;
  private nextCredentialOperationId = 1;
  private readonly pendingCredentialOperations = new Map<number, PendingCredentialOperation>();
  private activeTurn = false;
  private activeTurnId: number | undefined;
  private cancellingTurn = false;
  private lastRuntimeVersions: BridgeRuntimeVersions | undefined;
  private readonly pendingPermissionIds = new Set<number>();
  private readonly pendingComputerAccessIds = new Set<number>();
  private readonly pendingAskUserQuestionIds = new Set<number>();
  private readonly pendingAskUserQuestionRequests = new Map<number, PendingAskUserQuestionRequest>();
  private pendingSessionResume: PendingSessionResume | null = null;
  private sessionHasHistory = false;
  private eventSequence = 0;
  private replayEvents: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly diagnostics: DiagnosticBuffer;
  private startPromise: Promise<void> | null = null;

  constructor(private readonly opts: BridgeManagerOptions) {
    this.sessionId = opts.sessionId ?? randomUUID();
    this.projectPath = opts.projectPath ?? '';
    this.diagnostics = opts.diagnostics ?? new DiagnosticBuffer();
  }

  get connectionState(): ConnectionState {
    return this.state;
  }

  get turnActive(): boolean {
    return this.activeTurn;
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

  get runtimeVersions(): BridgeRuntimeVersions | undefined {
    return this.lastRuntimeVersions ? { ...this.lastRuntimeVersions } : undefined;
  }

  get pendingAskUserQuestions(): readonly AskUserQuestionRequestDto[] {
    return [...this.pendingAskUserQuestionRequests.values()].map((entry) => entry.request);
  }

  get pendingInteractions(): number {
    return this.pendingPermissionIds.size
      + this.pendingComputerAccessIds.size
      + this.pendingAskUserQuestionIds.size;
  }

  get summary(): SessionRuntimeSummary {
    return {
      projectPath: this.projectPath || this.activeWorkspace || '',
      sessionId: this.sessionId,
      connection: this.connectionState,
      turnActive: this.activeTurn,
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

  private eventEnvelope(event: ClientEvent, retain: boolean): SequencedRuntimeEventEnvelope<ClientEvent> {
    const envelope = Object.freeze({
      sessionId: this.sessionId,
      sequence: ++this.eventSequence,
      event: structuredClone(event),
    });
    if (TRANSCRIPT_REPLAY_BASE_EVENTS.has(event.type)) {
      this.replayEvents = [envelope];
    } else if (retain && this.replayEvents.length > 0 && TRANSCRIPT_REPLAY_EVENTS.has(event.type)) {
      this.replayEvents = [...this.replayEvents, envelope];
    }
    return envelope;
  }

  private sendClientEvent(webContents: WebContents, event: ClientEvent, retain: boolean): void {
    const envelope = this.eventEnvelope(event, retain);
    webContents.send(CH_EVENT, this.opts.envelopeEvents ? envelope : event);
  }

  /**
   * The one renderer that answers single-responder requests: the
   * first-registered live window. Deterministic (a `Map` preserves insertion
   * order) and self-healing — destroyed windows are dropped as they are
   * encountered, the same bookkeeping `broadcast` does.
   */
  private responderTarget(): WebContents | null {
    for (const webContents of this.targets.keys()) {
      if (webContents.isDestroyed()) this.targets.delete(webContents);
      else return webContents;
    }
    return null;
  }

  private broadcastClientEvent(event: ClientEvent): void {
    if (SINGLE_RESPONDER_EVENTS.has(event.type)) {
      // Sent to one window, or to none — never to several. See
      // SINGLE_RESPONDER_EVENTS. `sendClientEvent` handles both the
      // enveloped and bare wire shapes, so this needs no second branch.
      const responder = this.responderTarget();
      if (responder) this.sendClientEvent(responder, event, false);
      return;
    }
    const envelope = this.eventEnvelope(event, true);
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
  }

  private clearTurnInteractions(): void {
    this.pendingPermissionIds.clear();
    this.pendingComputerAccessIds.clear();
    for (const requestId of [...this.pendingAskUserQuestionIds]) {
      this.clearPendingAskUserQuestion(requestId);
    }
  }

  async start(): Promise<void> {
    if (this.disposed) throw new Error('SessionRuntime is disposed');
    if (this.state.status === 'connected') return;
    if (this.startPromise) return this.startPromise;
    if (this.child || this.client) return;
    if (this.opts.registerIpc !== false) this.registerIpc();
    this.startPromise = (async () => {
      try {
        await this.startInternal();
      } catch (error) {
        // launchConfig runs before the child lifecycle begins. Surface failures
        // such as an unreadable Keychain credential through the same renderer
        // connection state as spawn/protocol failures.
        if (this.state.status !== 'error') this.fail(error);
        throw error;
      } finally {
        this.startPromise = null;
      }
    })();
    return this.startPromise;
  }

  restart(beforeRestart?: () => void): Promise<void> {
    if (this.opts.registerIpc !== false) this.registerIpc();
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) throw new Error('session runtime is no longer open');
      // This runs after any earlier queued lifecycle work and immediately
      // before stopping the child. Callers can re-check ownership/work here
      // to close the queueing race between IPC validation and restart.
      beforeRestart?.();
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
    if (this.opts.registerIpc !== false) this.registerIpc();
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) return;
      await this.stopBridge();
      this.setState({ status: 'idle' });
    });
    return this.restartChain;
  }

  private async startInternal(): Promise<void> {
    const launch = await this.opts.launchConfig();
    if (this.disposed) throw new Error('SessionRuntime is disposed');
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
    this.credentialStorageEncrypted = false;
    const bridgeDir = this.createLaunchDirectory();
    const generation = ++this.generation;

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
    this.captureLogs(child, [launch.apiKey, ...Object.values(launch.providerCredentials ?? {})].filter((value): value is string => Boolean(value)));

    child.once('exit', (code, signal) => {
      this.diagnostics.add('info', 'host', childExitDiagnostic(code, signal, generation));
      if (generation !== this.generation) return;
      this.child = null;
      if (!this.disposed) {
        this.setState({ status: 'disconnected', reason: `bridge-server exited (code=${code ?? 'null'}, signal=${signal ?? 'null'})` });
      }
    });
    child.once('error', (error) => {
      if (generation === this.generation && !this.disposed) this.fail(`failed to spawn bridge-server: ${error.message}`);
    });

    try {
      const lockfilePath = await this.waitForLockfile(bridgeDir, launch, generation);
      if (this.disposed) throw new Error('SessionRuntime is disposed');
      this.setState({ status: 'connecting' });
      const client = new BridgeClient({ lockfilePath, clientName: 'lingxi-electron/0.1.0' });
      this.client = client;
      this.wireClient(client, generation);
      const hello = await client.connect();
      if (generation !== this.generation || this.disposed) return;
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
      this.setState({ status: 'connected' });
      // Reading credential metadata may consult the login Keychain, so it
      // must never hold the desktop in a spawning/restarting state.
      void this.refreshProviderCredentials();
    } catch (error) {
      if (generation === this.generation) {
        this.fail(error);
        await this.stopBridge();
      }
      throw error;
    }
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
      workspace: launch.workspace,
      sessionId: this.sessionId,
      bridgeDir,
      model: launch.model,
      hasApiKey: Boolean(launch.apiKey),
      hasCredentialStdin: Object.keys(launch.providerCredentials ?? {}).length > 0,
      trusted: launch.trusted,
      packagedCredentialBoundary: Boolean(this.opts.isPackaged),
    });

    const child = spawn(bin, args, {
      cwd: launch.workspace,
      env: buildBridgeEnvironment(process.env, launch.apiBaseUrl),
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      detached: process.platform !== 'win32',
    });
    const providerCredentials = launch.providerCredentials ?? {};
    if (Object.keys(providerCredentials).length > 0) {
      child.stdin?.end(buildCredentialEnvelope(launch));
    } else if (launch.apiKey) child.stdin?.end(`${launch.apiKey}\n`);
    else child.stdin?.end();
    return child;
  }

  listProviderCredentials(providerIds: readonly string[]): Promise<ProviderCredentialStatus> {
    const ids = providerIds.map((providerId) => this.validateProviderId(providerId));
    if (ids.length > 32) throw new Error('too many provider credentials requested');
    return this.requestCredentialOperation(ids, (operationId) => ({
      type: 'list_provider_credentials',
      operation_id: operationId,
      provider_ids: ids,
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
    const attach = (stream: NodeJS.ReadableStream | null, level: 'info' | 'error'): void => {
      if (!stream) return;
      let buffered = '';
      const flush = (): void => {
        const text = sanitizeDiagnostic(buffered, secrets);
        buffered = '';
        if (text) this.diagnostics.add(level, 'bridge', text, secrets);
      };
      stream.on('data', (chunk: Buffer | string) => {
        buffered += chunk.toString();
        const lines = buffered.split(/\r?\n/);
        buffered = lines.pop() ?? '';
        for (const line of lines) {
          const text = sanitizeDiagnostic(line, secrets);
          if (text) this.diagnostics.add(level, 'bridge', text, secrets);
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
    client.on('event', (event: ClientEvent) => {
      if (generation !== this.generation) return;
      if (isTurnOwnedEvent(event) && !this.activeTurn) {
        this.diagnostics.add('warn', 'bridge', `dropped unowned turn event: ${event.type}`);
        return;
      }
      if (event.type === 'provider_credential_status') {
        this.handleProviderCredentialStatus(event);
      }
      if (event.type === 'session_resumed') {
        if (event.session_id !== this.sessionId) {
          this.rejectPendingSessionResume(new Error('engine resumed a different session id'));
          this.fail('engine resumed a different session id');
          return;
        }
        this.sessionHasHistory = true;
        this.resolvePendingSessionResume();
      }
      if (event.type === 'error' && this.pendingSessionResume) {
        this.rejectPendingSessionResume(new Error(sanitizeDiagnostic(event.message)));
      }
      if (event.type === 'turn_started') {
        if (!this.activeTurn) this.cancellingTurn = false;
        this.activeTurn = true;
        this.activeTurnId = event.turn_id;
      }
      if (event.type === 'turn_ended' || event.type === 'session_ended') {
        this.activeTurn = false;
        this.activeTurnId = undefined;
        this.cancellingTurn = false;
        this.clearTurnInteractions();
      }
      if (event.type === 'model_changed') {
        try { this.opts.onModelChanged?.(event.model); }
        catch (error) { this.diagnostics.add('warn', 'host', error); }
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
      }
      if (event.type === 'ask_user_question_resolved') {
        this.clearPendingAskUserQuestion(event.request_id);
      }
      this.broadcastClientEvent(event);
    });
    client.on('permission', (request: PermissionRequest) => {
      if (generation !== this.generation || !this.activeTurn || this.cancellingTurn) return;
      if (Number.isSafeInteger(request.request_id) && request.request_id >= 0) {
        if (!this.pendingPermissionIds.has(request.request_id) && this.pendingPermissionIds.size >= MAX_PENDING_PERMISSIONS) {
          this.diagnostics.add('warn', 'bridge', 'permission request limit reached');
          return;
        }
        this.pendingPermissionIds.add(request.request_id);
        this.broadcast(CH_PERMISSION, request);
      }
    });
    client.on('computerAccess', (request: ComputerAccessRequestDto) => {
      if (generation !== this.generation || !this.activeTurn || this.cancellingTurn) return;
      if (Number.isSafeInteger(request.request_id) && request.request_id >= 0) {
        if (
          !this.pendingComputerAccessIds.has(request.request_id)
          && this.pendingComputerAccessIds.size >= MAX_PENDING_COMPUTER_ACCESS
        ) {
          this.diagnostics.add('warn', 'bridge', 'computer access request limit reached');
          return;
        }
        this.pendingComputerAccessIds.add(request.request_id);
        this.broadcast(CH_COMPUTER_ACCESS, request);
      }
    });
    client.on('close', (code, reason) => {
      if (generation === this.generation && !this.disposed) this.setState({ status: 'disconnected', reason: reason || `ws closed (code=${code})` });
    });
    client.on('error', (error) => {
      if (generation === this.generation && !this.disposed) this.fail(error);
    });
  }

  private handleProviderCredentialStatus(event: ProviderCredentialStatus): void {
    const pending = this.pendingCredentialOperations.get(event.operation_id);
    if (pending) {
      clearTimeout(pending.timer);
      this.pendingCredentialOperations.delete(event.operation_id);
    }
    const configured = new Set(event.configured_provider_ids);
    const unavailable = new Set(event.unavailable_provider_ids ?? []);
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
      if (configured.has(providerId)) this.persistedCredentialProviders.add(providerId);
      else if (clearAbsent) this.persistedCredentialProviders.delete(providerId);
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

  private assertSender(event: IpcMainInvokeEvent): void {
    const origins = this.targets.get(event.sender);
    const senderFrame = event.senderFrame;
    if (!origins || !senderFrame || senderFrame !== event.sender.mainFrame) throw new Error('unauthorized IPC sender');
    const origin = urlOrigin(senderFrame.url);
    if (!origin || !origins.has(origin)) throw new Error('unauthorized IPC origin');
  }

  sendPrompt(text: unknown, images: unknown = []): void {
    const prompt = validatePrompt(text);
    const validatedImages = validateImageRefs(images);
    this.requireClient().sendPrompt(prompt, { images: validatedImages });
    this.sessionHasHistory = true;
    // Claim the local slot as soon as the command crossed the authenticated
    // bridge boundary. `turn_started` may arrive on a later event-loop tick;
    // without this pending owner an immediate Cancel (or permission request)
    // can fall through the same gap fixed in the Rust connection.
    if (!this.activeTurn) {
      this.activeTurn = true;
      this.activeTurnId = undefined;
      this.cancellingTurn = false;
    }
  }

  cancelTurn(turnId: unknown): void {
    const id = validateOptionalTurnId(turnId);
    this.requireClient().cancel(id);
    if (this.activeTurn && (id === undefined || id === this.activeTurnId)) {
      this.cancellingTurn = true;
      this.clearTurnInteractions();
    }
  }

  approvePermission(requestId: number, response?: unknown): void {
    const id = validateRequestId(requestId);
    const permissionResponse = validatePermissionResponse(response);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().approvePermission(id, permissionResponse);
    this.pendingPermissionIds.delete(id);
  }

  denyPermission(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().denyPermission(id);
    this.pendingPermissionIds.delete(id);
  }

  approveComputerAccess(requestId: number, response: unknown): void {
    const id = validateRequestId(requestId);
    const computerAccessResponse = validateComputerAccessResponse(response);
    if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
    this.requireClient().approveComputerAccess(id, computerAccessResponse);
    this.pendingComputerAccessIds.delete(id);
  }

  denyComputerAccess(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
    this.requireClient().denyComputerAccess(id);
    this.pendingComputerAccessIds.delete(id);
  }

  answerAskUserQuestion(requestId: number, answers: unknown): void {
    const id = validateRequestId(requestId);
    const validatedAnswers = validateAskUserQuestionAnswers(answers);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().answerAskUserQuestion(id, validatedAnswers);
    this.clearPendingAskUserQuestion(id);
  }

  cancelAskUserQuestion(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().cancelAskUserQuestion(id);
    this.clearPendingAskUserQuestion(id);
  }

  private registerIpc(): void {
    if (this.ipcRegistered) return;
    this.ipcRegistered = true;
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, text: unknown, images: unknown) => {
      this.assertSender(event);
      // The engine owns provider credential resolution. The Electron host must
      // not reject a prompt merely because no secret crossed its stdin boundary;
      // CLI/TUI may already have populated the shared secure store.
      this.sendPrompt(text, images);
    });
    ipcMain.handle(CH_APPROVE, (event: IpcMainInvokeEvent, requestId: unknown, response: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      const permissionResponse = validatePermissionResponse(response);
      if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
      this.requireClient().approvePermission(id, permissionResponse);
      this.pendingPermissionIds.delete(id);
    });
    ipcMain.handle(CH_DENY, (event: IpcMainInvokeEvent, requestId: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
      this.requireClient().denyPermission(id);
      this.pendingPermissionIds.delete(id);
    });
    ipcMain.handle(CH_APPROVE_COMPUTER_ACCESS, (event: IpcMainInvokeEvent, requestId: unknown, response: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      const computerAccessResponse = validateComputerAccessResponse(response);
      if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
      this.requireClient().approveComputerAccess(id, computerAccessResponse);
      this.pendingComputerAccessIds.delete(id);
    });
    ipcMain.handle(CH_DENY_COMPUTER_ACCESS, (event: IpcMainInvokeEvent, requestId: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
      this.requireClient().denyComputerAccess(id);
      this.pendingComputerAccessIds.delete(id);
    });
    ipcMain.handle(CH_ANSWER_ASK_USER_QUESTION, (event: IpcMainInvokeEvent, requestId: unknown, answers: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      const validatedAnswers = validateAskUserQuestionAnswers(answers);
      if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
      this.requireClient().answerAskUserQuestion(id, validatedAnswers);
      this.clearPendingAskUserQuestion(id);
    });
    ipcMain.handle(CH_CANCEL_ASK_USER_QUESTION, (event: IpcMainInvokeEvent, requestId: unknown) => {
      this.assertSender(event);
      const id = validateRequestId(requestId);
      if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
      this.requireClient().cancelAskUserQuestion(id);
      this.clearPendingAskUserQuestion(id);
    });
    ipcMain.handle(CH_CANCEL, (event: IpcMainInvokeEvent, turnId: unknown) => {
      this.assertSender(event);
      this.cancelTurn(turnId);
    });
    ipcMain.handle(CH_COMMAND, async (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      await this.dispatchCommand(command);
    });
    ipcMain.handle(CH_CONNECTION_STATE, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.state;
    });
  }

  private unregisterIpc(): void {
    if (!this.ipcRegistered) return;
    for (const channel of [
      CH_SEND_PROMPT, CH_APPROVE, CH_DENY, CH_APPROVE_COMPUTER_ACCESS, CH_DENY_COMPUTER_ACCESS,
      CH_ANSWER_ASK_USER_QUESTION, CH_CANCEL_ASK_USER_QUESTION,
      CH_CANCEL, CH_COMMAND, CH_CONNECTION_STATE,
    ]) {
      ipcMain.removeHandler(channel);
    }
    this.ipcRegistered = false;
  }

  /**
   * Validate a renderer command and forward it to the engine. SECURITY:
   * `set_permission_mode: bypassPermissions` is gated behind explicit,
   * persisted acceptance (a blocking main-process dialog, shown once) — it is
   * NEVER one-click. A decline (or no confirmer wired) throws and the command
   * never reaches the engine; the renderer's mode display reads the engine's
   * actual (unchanged) mode, so nothing to revert.
   */
  async dispatchCommand(command: unknown): Promise<void> {
    const validated = validateClientCommand(command, this.activeWorkspace);
    assertCommandAllowedDuringTurn(validated, this.activeTurn);
    if (validated.type === 'set_permission_mode' && validated.mode === 'bypassPermissions') {
      const accepted = (await this.opts.confirmBypassPermissions?.()) ?? false;
      if (!accepted) {
        throw new Error('Bypass Permissions mode was not accepted');
      }
    }
    this.requireClient().sendCommand(validated);
  }

  /** Resume the one session owned by this runtime and wait for engine proof. */
  resumeOwnedSession(): Promise<void> {
    if (this.pendingSessionResume) return Promise.reject(new Error('session resume is already pending'));
    const client = this.requireClient();
    const generation = this.generation;
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pendingSessionResume?.generation !== generation) return;
        this.pendingSessionResume = null;
        reject(new Error(`timed out resuming session ${this.sessionId}`));
      }, this.opts.sessionResumeTimeoutMs ?? 15_000);
      timer.unref();
      this.pendingSessionResume = {
        sessionId: this.sessionId,
        generation,
        resolve,
        reject,
        timer,
      };
      try {
        client.sendCommand({
          type: 'resume_session',
          session_id: this.sessionId,
          cwd: this.projectPath || this.activeWorkspace,
        });
      } catch (error) {
        this.rejectPendingSessionResume(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  async restoreOwnedSessionIfNeeded(): Promise<void> {
    if (this.sessionHasHistory) await this.resumeOwnedSession();
  }

  private resolvePendingSessionResume(): void {
    const pending = this.pendingSessionResume;
    if (!pending || pending.sessionId !== this.sessionId || pending.generation !== this.generation) return;
    this.pendingSessionResume = null;
    clearTimeout(pending.timer);
    pending.resolve();
  }

  private rejectPendingSessionResume(error: Error): void {
    const pending = this.pendingSessionResume;
    if (!pending) return;
    this.pendingSessionResume = null;
    clearTimeout(pending.timer);
    pending.reject(error);
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

  private sendToWindow(webContents: WebContents, channel: string, payload: unknown): void {
    const value = this.opts.envelopeEvents
      ? { sessionId: this.sessionId, event: payload }
      : payload;
    webContents.send(channel, value);
  }

  private setState(next: ConnectionState): void {
    this.state = next;
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
  }

  private fail(error: unknown): void {
    const message = sanitizeDiagnostic(error);
    this.diagnostics.add('error', 'host', message);
    this.setState({ status: 'error', message });
  }

  private async stopBridge(): Promise<void> {
    ++this.generation;
    this.rejectPendingSessionResume(new Error('session resume was interrupted'));
    this.clearTurnInteractions();
    for (const pending of this.pendingCredentialOperations.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('bridge credential operation was interrupted'));
    }
    this.pendingCredentialOperations.clear();
    this.activeWorkspace = undefined;
    this.activeWorkspaceTrusted = false;
    this.runtimeCredentialProviders.clear();
    this.persistedCredentialProviders.clear();
    this.activeCredentialProviders.clear();
    this.credentialStorageEncrypted = false;
    this.activeTurn = false;
    this.activeTurnId = undefined;
    this.cancellingTurn = false;
    const client = this.client;
    this.client = null;
    if (client) {
      try {
        client.removeAllListeners();
        client.close();
      } catch {
        // A half-open socket may throw while closing.
      }
    }

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
      try { this.signalChildTree(child, 'SIGINT'); } catch { finish(); return; }
      timer = setTimeout(() => {
        try { this.signalChildTree(child, 'SIGKILL'); } catch { /* already gone */ }
        finish();
      }, this.opts.stopTimeoutMs ?? 2_000);
      timer.unref();
    });
    this.removeLaunchDirectory();
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

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    await this.restartChain.catch(() => undefined);
    if (this.opts.envelopeEvents) {
      this.setState({ status: 'disconnected', reason: SESSION_RUNTIME_DISPOSED_REASON });
    }
    await this.stopBridge();
    this.unregisterIpc();
    this.targets.clear();
  }
}

/**
 * Backwards-compatible name for the old single-runtime test surface. Production
 * Electron code uses {@link SessionRuntimeManager}; keeping this thin subclass
 * lets older focused tests exercise one runtime without registering the global
 * multi-session IPC router.
 */
export class BridgeManager extends SessionRuntime {}

/**
 * Owns all Electron bridge processes. A runtime is keyed by the engine's
 * stable session UUID, never by a display name or the currently selected
 * project. Project selection is therefore a renderer/navigation concern and
 * cannot accidentally restart another session.
 */
export class SessionRuntimeManager {
  private readonly runtimes = new Map<string, SessionRuntime>();
  private readonly openingSessions = new Map<string, { projectPath: string; promise: Promise<SessionRuntime> }>();
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly targetDestroyedHandlers = new Map<WebContents, () => void>();
  private registered = false;

  constructor(private readonly opts: SessionRuntimeManagerOptions) {}

  get size(): number {
    return this.runtimes.size;
  }

  get runtimeSummaries(): SessionRuntimeSummary[] {
    return [...this.runtimes.values()].map((runtime) => runtime.summary);
  }

  get replaySnapshots(): readonly SequencedRuntimeEventEnvelope<ClientEvent>[] {
    return Object.freeze([...this.runtimes.values()].flatMap((runtime) => runtime.replaySnapshot()));
  }

  get(sessionId: string): SessionRuntime | undefined {
    return this.runtimes.get(sessionId);
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

  /** Create, validate, and optionally start one session-owned runtime. */
  async ensure(ref: SessionRef, start = true): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (existing) {
      if (existing.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      if (start) await existing.start();
      return existing;
    }

    const runtimeOptions = this.runtimeOptions(ref);
    const runtime = new SessionRuntime(runtimeOptions);
    this.runtimes.set(ref.sessionId, runtime);
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
      await runtime.dispose().catch(() => undefined);
      throw error;
    }
  }

  openSession(ref: SessionRef, empty = false): Promise<SessionRuntime> {
    assertSessionRef(ref);
    this.assertProjectNotClosing(ref.projectPath);
    const existing = this.runtimes.get(ref.sessionId);
    if (existing && existing.projectPath !== ref.projectPath) {
      throw new Error('session id is owned by a different project');
    }
    const pending = this.openingSessions.get(ref.sessionId);
    if (pending) {
      if (pending.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      return pending.promise;
    }
    if (existing?.connectionState.status === 'connected') return Promise.resolve(existing);

    const promise = this.openSessionInternal(ref, existing, empty);
    const trackedPromise = promise.finally(() => {
      if (this.openingSessions.get(ref.sessionId)?.promise === trackedPromise) this.openingSessions.delete(ref.sessionId);
    });
    this.openingSessions.set(ref.sessionId, { projectPath: ref.projectPath, promise: trackedPromise });
    return trackedPromise;
  }

  private async openSessionInternal(ref: SessionRef, existing: SessionRuntime | undefined, empty: boolean): Promise<SessionRuntime> {
    let runtime: SessionRuntime | undefined;
    try {
      if (existing) {
        await existing.restart();
        runtime = existing;
      } else {
        runtime = await this.ensure(ref, true);
      }
      if (!empty) await runtime.resumeOwnedSession();
      return runtime;
    } catch (error) {
      if (!existing && runtime) {
        this.runtimes.delete(ref.sessionId);
        await runtime.dispose().catch(() => undefined);
      }
      throw error;
    }
  }

  async newSession(projectPath: string, _model?: string): Promise<SessionRef> {
    if (typeof projectPath !== 'string' || projectPath.length === 0) throw new Error('invalid project path');
    this.assertProjectNotClosing(projectPath);
    let ref: SessionRef | undefined;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const candidate = { projectPath, sessionId: randomUUID() } satisfies SessionRef;
      if (!this.opts.sessionIdAvailable || await this.opts.sessionIdAvailable(candidate)) {
        ref = candidate;
        break;
      }
    }
    if (!ref) throw new Error('could not allocate a new session id');
    await this.ensure(ref, true);
    // The boot argument is the source of truth for a new session. Sending a
    // second `new_session` command would create/switch the engine to another
    // UUID and leave the runtime key pointing at the wrong transcript.
    return ref;
  }

  async restart(ref: SessionRef, beforeRestart?: () => void): Promise<void> {
    const runtime = this.require(ref);
    await runtime.restart(beforeRestart);
    await runtime.restoreOwnedSessionIfNeeded();
  }

  async closeSession(ref: SessionRef): Promise<void> {
    const runtime = this.require(ref);
    // Remove from the routable map BEFORE the first await, for the same reason
    // `closeProject` does: while `dispose()` is in flight `get()` would still
    // hand this runtime out, and `restart()` now rejects on a disposed runtime
    // instead of resolving silently — surfacing a failure for a settings or
    // credential write that actually succeeded.
    this.runtimes.delete(ref.sessionId);
    await runtime.dispose();
  }

  async closeProject(projectPath: string): Promise<void> {
    this.assertProjectNotClosing(projectPath);
    if (this.hasActiveWork(projectPath)) throw new Error('cancel active turns and pending interactions before removing a project');
    this.closingProjects.add(projectPath);
    const projectRuntimes = [...this.runtimes.values()].filter((runtime) => runtime.projectPath === projectPath);
    // Remove runtimes from the routable map before the first await. A prompt
    // arriving while disposal is in progress must fail instead of entering a
    // runtime whose child is already being torn down.
    for (const runtime of projectRuntimes) this.runtimes.delete(runtime.sessionId);
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
      && (runtime.turnActive || runtime.pendingInteractions > 0)
    ));
  }

  async dispose(): Promise<void> {
    const runtimes = [...this.runtimes.values()];
    this.runtimes.clear();
    this.openingSessions.clear();
    await Promise.all(runtimes.map((runtime) => runtime.dispose().catch(() => undefined)));
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
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, sessionId: unknown, text: unknown, images: unknown) => {
      this.assertSender(event);
      this.requireById(sessionId).sendPrompt(text, images);
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
      ...base
    } = this.opts;
    return {
      ...base,
      sessionId: ref.sessionId,
      projectPath: ref.projectPath,
      envelopeEvents: true,
      registerIpc: false,
      launchConfig: () => launchConfig(ref),
      ...(accessState ? { accessState: () => accessState(ref) } : {}),
      ...(onModelChanged ? { onModelChanged: (model: string) => onModelChanged(ref, model) } : {}),
    };
  }

  private requireById(value: unknown): SessionRuntime {
    if (!isSessionId(value)) throw new Error('invalid session id');
    const runtime = this.runtimes.get(value);
    if (!runtime) throw new Error(`session runtime is not open: ${value}`);
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
    ]) ipcMain.removeHandler(channel);
    this.registered = false;
  }
}
