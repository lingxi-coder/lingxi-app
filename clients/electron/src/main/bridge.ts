import { GitActivityTracker } from './git-activity.js';
import type { OpenAiOAuthSession } from './host-utils.js';
import type { HostNotifier } from './notifications.js';
export type { OpenAiOAuthSession } from './host-utils.js';
import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { IpcMainInvokeEvent, WebContents } from 'electron';
import {
  type AskUserQuestionRequestDto,
  BridgeClient,
  type ClientCommand,
  type ClientEvent,
  type ComputerAccessRequestDto,
  type PermissionRequest,
  type PermissionModeId,
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
import { resolveFusionCredentialProviderIds, resolveModelCredentialProviderIds } from './credential-broker.js';

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
  /** Internal process-inspection seam used to recover detached Desktop runtimes after a main-process restart. */
  readProcessCommand?: (pid: number) => string | undefined;
  /** Internal process-table seam used to attach sessions still owned by another local Desktop/test host. */
  listProcessCommands?: () => readonly ProcessCommand[];
  onCronRunRequested?: (runtime: SessionRuntime, event: Extract<ClientEvent, { type: 'cron_run_requested' }>) => Promise<{ sessionId: string; summary: string }>;
  onModelChanged?: (model: string) => void;
  /** Persist only an explicitly requested, engine-confirmed model selection. */
  onModelSelected?: (model: string) => void;
  getSavedModel?: () => string | undefined;
  getSavedPermissionMode?: () => PermissionModeId | undefined;
  onPermissionModeSelected?: (mode: PermissionModeId) => void;
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

type ProviderCredentialStatus = Extract<ClientEvent, { type: 'provider_credential_status' }>;
export type ProviderConnectionTestResult = Extract<ClientEvent, { type: 'provider_connection_tested' }>;

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
  sessionResumed: boolean;
  model?: string;
  hydrationStarted: boolean;
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

export interface ReusableBridge {
  launchDir: string;
  lockfilePath: string;
  pid: number;
  ownedProcess: boolean;
}

export interface ProcessCommand {
  pid: number;
  ppid?: number;
  command: string;
}

function privateOwnedPath(path: string, kind: 'directory' | 'file'): boolean {
  try {
    const metadata = lstatSync(path);
    if (kind === 'directory' ? !metadata.isDirectory() : !metadata.isFile()) return false;
    if (process.platform !== 'win32' && (metadata.mode & 0o077) !== 0) return false;
    if (process.getuid && metadata.uid !== process.getuid()) return false;
    return true;
  } catch {
    return false;
  }
}

export function processCommandOwnsSession(command: string, sessionId: string): boolean {
  if (!isSessionId(sessionId)) return false;
  const escaped = sessionId.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|\\s)--session-id(?:=|\\s+)${escaped}(?=\\s|$)`).test(command);
}

function readProcessCommand(pid: number): string | undefined {
  if (process.platform === 'win32') return undefined;
  try {
    const command = execFileSync('/bin/ps', ['-p', String(pid), '-o', 'command='], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024,
      timeout: 1_000,
    }).trim();
    return command || undefined;
  } catch {
    return undefined;
  }
}

function listProcessCommands(): ProcessCommand[] {
  if (process.platform === 'win32') return [];
  try {
    return execFileSync('/bin/ps', ['-axww', '-o', 'pid=,ppid=,command='], {
      encoding: 'utf8',
      maxBuffer: 4 * 1024 * 1024,
      timeout: 2_000,
    }).split('\n').flatMap((line) => {
      const match = /^\s*(\d+)\s+(\d+)\s+(.+)$/.exec(line);
      return match ? [{ pid: Number(match[1]), ppid: Number(match[2]), command: match[3] }] : [];
    });
  } catch {
    return [];
  }
}

function lockfilePort(name: string): number | undefined {
  if (!name.endsWith('.lock') || name.startsWith('.')) return undefined;
  const raw = name.slice(0, -'.lock'.length);
  const port = Number(raw);
  return Number.isInteger(port) && port >= 1 && port <= 65_535 && String(port) === raw
    ? port
    : undefined;
}

function reusableBridgeInLaunchDirectory(
  launchDir: string,
  ref: SessionRef,
  pid: number,
  ownedProcess: boolean,
): ReusableBridge | undefined {
  if (!privateOwnedPath(launchDir, 'directory')) return undefined;
  for (const name of lockfiles(launchDir)) {
    if (lockfilePort(name) === undefined) continue;
    const lockfilePath = join(launchDir, name);
    if (!privateOwnedPath(lockfilePath, 'file')) continue;
    try {
      const body = JSON.parse(readFileSync(lockfilePath, 'utf8')) as { pid?: unknown };
      validateBridgeLockfile(body, pid, ref.projectPath);
      return { launchDir, lockfilePath, pid, ownedProcess };
    } catch {
      // A stale or partially-written candidate is not reusable; keep looking.
    }
  }
  return undefined;
}

/**
 * Locate a still-running bridge-server previously detached from this exact
 * Desktop user-data root. The private lockfile authenticates the connection;
 * the process command supplies the missing session-id binding in the legacy
 * lockfile schema without weakening its Claude-compatible wire format.
 */
export function discoverReusableBridge(
  bridgeRoot: string,
  ref: SessionRef,
  processCommand: (pid: number) => string | undefined = readProcessCommand,
): ReusableBridge | undefined {
  if (!privateOwnedPath(bridgeRoot, 'directory')) return undefined;
  let entries: string[];
  try {
    entries = readdirSync(bridgeRoot).sort((left, right) => right.localeCompare(left));
  } catch {
    return undefined;
  }
  for (const entry of entries) {
    if (!entry.startsWith('launch-')) continue;
    const launchDir = join(bridgeRoot, entry);
    if (!privateOwnedPath(launchDir, 'directory')) continue;
    for (const name of lockfiles(launchDir)) {
      if (lockfilePort(name) === undefined) continue;
      const lockfilePath = join(launchDir, name);
      if (!privateOwnedPath(lockfilePath, 'file')) continue;
      try {
        const body = JSON.parse(readFileSync(lockfilePath, 'utf8')) as { pid?: unknown };
        if (!Number.isSafeInteger(body.pid) || Number(body.pid) < 1) continue;
        const pid = Number(body.pid);
        const command = processCommand(pid);
        if (!command || !processCommandOwnsSession(command, ref.sessionId)) continue;
        const reusable = reusableBridgeInLaunchDirectory(launchDir, ref, pid, true);
        if (reusable) return reusable;
      } catch {
        // A stale or partially-written candidate is not reusable; keep looking.
      }
    }
  }
  return undefined;
}

function argumentBetween(command: string, start: string, end: string): string | undefined {
  const startIndex = command.indexOf(start);
  if (startIndex < 0) return undefined;
  const valueStart = startIndex + start.length;
  const endIndex = command.indexOf(end, valueStart);
  if (endIndex < 0) return undefined;
  const value = command.slice(valueStart, endIndex).trim();
  return value || undefined;
}

function processCommandArgument(command: string, name: string): string | undefined {
  const marker = ` --${name} `;
  const startIndex = command.indexOf(marker);
  if (startIndex < 0) return undefined;
  const valueStart = startIndex + marker.length;
  const nextFlag = command.indexOf(' --', valueStart);
  const value = command.slice(valueStart, nextFlag < 0 ? undefined : nextFlag).trim();
  return value || undefined;
}

function processCommandHasFlag(command: string, name: string): boolean {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|\\s)--${escaped}(?=\\s|$)`).test(command);
}

function pathIsWithin(root: string, candidate: string): boolean {
  const relativePath = relative(resolve(root), resolve(candidate));
  return relativePath === '' || (!relativePath.startsWith('..') && !isAbsolute(relativePath));
}

export function discoverExternalReusableBridge(
  ref: SessionRef,
  processes: readonly ProcessCommand[] = listProcessCommands(),
): ReusableBridge | undefined {
  for (const processInfo of processes) {
    if (!processCommandOwnsSession(processInfo.command, ref.sessionId)) continue;
    const workspace = argumentBetween(processInfo.command, ' --cwd ', ' --bridge-dir ');
    const launchDir = argumentBetween(processInfo.command, ' --bridge-dir ', ' --session-id ');
    if (workspace !== ref.projectPath || !launchDir) continue;
    const reusable = reusableBridgeInLaunchDirectory(
      launchDir,
      ref,
      processInfo.pid,
      processInfo.ppid === 1,
    );
    if (reusable) return reusable;
  }
  return undefined;
}

/**
 * Find only pre-broker Desktop bridges that launchd inherited after their
 * parent exited. Current packaged bridges always carry the stdin-only marker;
 * constraining candidates to this private Desktop root avoids terminating CLI,
 * TUI, test, or another app installation's background sessions.
 */
export function discoverLegacyOrphanBridges(
  bridgeRoot: string,
  processes: readonly ProcessCommand[] = listProcessCommands(),
): ReusableBridge[] {
  if (!privateOwnedPath(bridgeRoot, 'directory')) return [];
  const candidates: ReusableBridge[] = [];
  const seen = new Set<number>();
  for (const processInfo of processes) {
    if (processInfo.ppid !== 1 || seen.has(processInfo.pid)) continue;
    if (processCommandHasFlag(processInfo.command, 'packaged-credential-stdin-only')) continue;
    const workspace = processCommandArgument(processInfo.command, 'cwd');
    const launchDir = processCommandArgument(processInfo.command, 'bridge-dir');
    const sessionId = processCommandArgument(processInfo.command, 'session-id');
    if (!workspace || !launchDir || !sessionId || !isSessionId(sessionId)) continue;
    if (!pathIsWithin(bridgeRoot, launchDir)) continue;
    if (!processCommandOwnsSession(processInfo.command, sessionId)) continue;
    const reusable = reusableBridgeInLaunchDirectory(
      launchDir,
      { projectPath: workspace, sessionId },
      processInfo.pid,
      true,
    );
    if (!reusable) continue;
    seen.add(processInfo.pid);
    candidates.push(reusable);
  }
  return candidates;
}

interface StopLegacyOrphanBridgesOptions {
  processes?: readonly ProcessCommand[];
  signalProcess?: (pid: number, signal: NodeJS.Signals) => boolean;
  processIsAlive?: (pid: number) => boolean;
  wait?: (milliseconds: number) => Promise<void>;
  stopTimeoutMs?: number;
}

function signalProcessGroup(pid: number, signal: NodeJS.Signals): boolean {
  try {
    process.kill(-pid, signal);
    return true;
  } catch {
    try {
      process.kill(pid, signal);
      return true;
    } catch {
      return false;
    }
  }
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

export async function stopLegacyOrphanBridges(
  bridgeRoot: string,
  options: StopLegacyOrphanBridgesOptions = {},
): Promise<number[]> {
  const signalProcess = options.signalProcess ?? signalProcessGroup;
  const isAlive = options.processIsAlive ?? processIsAlive;
  const wait = options.wait ?? ((milliseconds: number) => new Promise<void>((resolveWait) => {
    const timer = setTimeout(resolveWait, milliseconds);
    timer.unref();
  }));
  const stopped: number[] = [];
  for (const candidate of discoverLegacyOrphanBridges(bridgeRoot, options.processes)) {
    if (!signalProcess(candidate.pid, 'SIGINT')) continue;
    const deadline = Date.now() + (options.stopTimeoutMs ?? 1_000);
    while (isAlive(candidate.pid) && Date.now() < deadline) await wait(50);
    if (isAlive(candidate.pid)) {
      signalProcess(candidate.pid, 'SIGKILL');
      await wait(25);
    }
    if (!isAlive(candidate.pid)) stopped.push(candidate.pid);
  }
  return stopped;
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

/**
 * How many single-responder requests may be tracked at once. Real use has one
 * or two in flight; past this the request is still delivered but no longer
 * tracked, which is exactly the behaviour before tracking existed - a bound
 * that degrades rather than one that starts refusing real work.
 */
const MAX_OUTSTANDING_RESPONDER_REQUESTS = 32;

/**
 * What the engine is told when a single-responder request arrives with no
 * window that could service it.
 *
 * Distinct from `reassignResponderRequests`' message on purpose: that one names
 * a window that WAS asked and then closed, this one names a request that never
 * reached a renderer at all. Both are answered from main rather than dropped,
 * for the same reason — see `broadcastClientEvent`.
 */
const NO_RESPONDER_WINDOW_MESSAGE =
  'no desktop window is open to perform this audio operation';

/** The correlation id of a single-responder event, or `null` if it carries none. */
function singleResponderRequestId(event: ClientEvent): number | null {
  const requestId = (event as { request_id?: unknown }).request_id;
  return Number.isSafeInteger(requestId) && (requestId as number) >= 0 ? requestId as number : null;
}

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
  'status_snapshot',
  'compaction_status',
  'error',
  'message_identity',
  'message_retracted',
  'system_notice',
  'loop_wakeup',
  'scheduled_task_fire',
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

export interface SessionRuntimeManagerOptions extends Omit<BridgeManagerOptions, 'launchConfig' | 'accessState' | 'onModelChanged' | 'onFirstPromptSent' | 'onActivityChanged' | 'sessionId' | 'projectPath' | 'envelopeEvents' | 'registerIpc'> {
  launchConfig: (ref: SessionRef, resumeModel?: string) => BridgeLaunchConfig | Promise<BridgeLaunchConfig>;
  accessState?: (ref: SessionRef) => { workspace?: string; trusted: boolean };
  onModelChanged?: (ref: SessionRef, model: string) => void;
  onFirstPromptSent?: (ref: SessionRef) => boolean | void;
  /** Maximum retained runtimes when enough idle sessions are evictable. */
  maxCachedRuntimes?: number;
}

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const DEFAULT_MAX_CACHED_RUNTIMES = 6;
const MAX_CONFIGURED_CACHED_RUNTIMES = 32;

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
  private adoptedPid: number | null = null;
  private adoptedProcessOwned = false;
  private activeWorkspace: string | undefined;
  private activeWorkspaceTrusted = false;
  private runtimeCredentialProviders = new Set<string>();
  private persistedCredentialProviders = new Set<string>();
  private activeCredentialProviders = new Set<string>();
  private credentialPreviews = new Map<string, string>();
  private credentialStorageEncrypted = false;
  private nextCredentialOperationId = 1;
  private readonly pendingCredentialOperations = new Map<number, PendingCredentialOperation>();
  private readonly pendingRuntimeCredentialLoads = new Map<string, Promise<void>>();
  private readonly pendingProviderConnectionTests = new Map<number, PendingProviderConnectionTest>();
  private archiving = false;
  private activeCronExecutions = 0;
  private readonly pendingCron = new Map<string, { resolve: (jobs: CronJobDto[]) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout>; creating: boolean }>();
  private readonly pendingScheduledTurns = new Map<string, { resolve: (summary: string) => void; reject: (error: Error) => void }>();
  private activeTurn = false;
  private openAiOAuthActive = false;
  private preparingOpenAiOAuth = false;
  /**
   * The Codex (ChatGPT OAuth) activation currently in flight, or null.
   *
   * Activation RESTARTS the engine, and `restart()` resolves only after the new
   * runtime has already told the renderer it is `connected`. The renderer reacts
   * to that word by firing its once-per-connection listing batch — `list_models`,
   * `get_conversation_controls`, `list_sessions`, the `refresh_listings` sweep.
   * While `preparingOpenAiOAuth` was merely a boolean that made `dispatchCommand`
   * THROW, that batch landed inside the refusal window and was rejected whole,
   * with nothing to retry it: `desktop.models` stayed `[]` for the rest of the
   * connection and the composer's model pill — `disabled` on an empty catalog —
   * was dead until the app was restarted. So a command now WAITS for the
   * activation and runs against the engine that replaces it.
   */
  private openAiOAuthPreparation: Promise<void> | null = null;
  private launchOAuthOverride: OpenAiOAuthSession | undefined;
  private launchOAuthModel: string | undefined;
  private fusionLifecycleEpoch = 0;
  private oauthPersistence: Promise<void> = Promise.resolve();
  private activeTurnId: number | undefined;
  private cancellingTurn = false;
  private lastRuntimeVersions: BridgeRuntimeVersions | undefined;
  /** Permission requests remain replayable until the engine reports a terminal resolution. */
  private readonly pendingPermissionIds = new Map<number, PermissionRequest>();
  /** Prevent a delayed duplicate permission frame from resurrecting a terminal request. */
  private readonly resolvedPermissionIds = new Set<number>();
  private readonly pendingComputerAccessIds = new Set<number>();
  private readonly pendingAskUserQuestionIds = new Set<number>();
  private readonly pendingAskUserQuestionRequests = new Map<number, PendingAskUserQuestionRequest>();
  private pendingSessionResume: PendingSessionResume | null = null;
  private sessionHasHistory = false;
  private sessionIdentityCommitted = false;
  private eventSequence = 0;
  private credentialRoutingSettings: unknown = undefined;
  private pendingModelSwitch: { model: string; sent: boolean; slash: boolean; turnId?: number; promise: Promise<void>; complete(selected?: string): void; fail(error: Error): void } | undefined;

  private pendingPermissionSwitch: {
    mode: PermissionModeId;
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

  private readonly pendingPromptHydrations = new Set<symbol>();
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
    if (this.configuredCustomProviderIds.includes(providerId)) return;
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

  private replayEvents: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  private readonly targets = new Map<WebContents, Set<string>>();
  /**
   * Single-responder requests the engine is currently parked on, and the
   * window each was handed to.
   *
   * Main has to carry this because electing one responder removed the
   * accidental redundancy broadcasting used to provide: a permission request
   * reaches every window, so another can answer it, but an audio request has
   * exactly one addressee and no second chance. If that window dies while the
   * engine waits, only main knows enough to reroute or to fail the call - the
   * dead renderer cannot, and the engine has no idea a window ever existed.
   * The event itself is kept, not just the id, because rerouting means asking
   * the same question again.
   */
  private readonly outstandingResponderRequests = new Map<number, { event: ClientEvent; responder: WebContents }>();
  private readonly diagnostics: DiagnosticBuffer;
  private startPromise: Promise<void> | null = null;

  constructor(private readonly opts: BridgeManagerOptions) {
    this.sessionId = opts.sessionId ?? randomUUID();
    this.projectPath = opts.projectPath ?? '';
    this.diagnostics = opts.diagnostics ?? new DiagnosticBuffer();
  }

  private startupDiagnostic(event: string, details: Record<string, unknown> = {}): void {
    this.diagnostics.add('info', 'host', diagnosticEvent(event, {
      projectPath: this.projectPath,
      sessionId: this.sessionId,
      ...details,
    }));
  }

  beginArchive(): () => void {
    if (this.archiving || this.turnActive || this.pendingInteractions > 0 || this.pendingCron.size > 0) throw new Error('Wait for active work and pending interactions before archiving this chat.');
    this.archiving = true;
    return () => { this.archiving = false; };
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
    const config = task.automation;
    if (!config?.model) throw new Error('paused: Configure a model for this task.');
    const generation = this.generation;
    while (this.turnActive || this.pendingInteractions > 0) {
      if (this.disposed || generation !== this.generation || this.state.status !== 'connected') throw new Error('interrupted: Scheduled execution interrupted.');
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (this.archiving) throw new Error('paused: The target chat is being archived.');
    const client = this.requireClient();
    const token = Symbol('scheduled hydration');
    this.pendingPromptHydrations.add(token);
    const assertPreparing = () => {
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
        this.sessionHasHistory = true;
        try {
          client.sendCommand({ type: 'scheduled_run_turn', run_id: runId, prompt: task.prompt, model: config.model, reasoning: config.reasoning });
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
    return this.preparingOpenAiOAuth || this.activeTurn || this.pendingPromptHydrations.size > 0 || this.pendingModelSwitch !== undefined;
  }

  private readonly gitActivity = new GitActivityTracker();
  get hasActiveAgents(): boolean { return this.gitActivity.active; }

  get hasOpenAiOAuth(): boolean { return this.openAiOAuthActive; }
  get isStarting(): boolean { return this.startPromise !== null; }

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

  private eventEnvelope(event: ClientEvent, retain: boolean): SequencedRuntimeEventEnvelope<ClientEvent> {
    const envelope = Object.freeze({
      sessionId: this.sessionId,
      sequence: ++this.eventSequence,
      event: structuredClone(event),
    });
    if (TRANSCRIPT_REPLAY_BASE_EVENTS.has(event.type)) {
      this.replayEvents = [envelope];
    } else if (retain && this.replayEvents.length > 0 && TRANSCRIPT_REPLAY_EVENTS.has(event.type)) {
      // Status is a snapshot, not a transcript event. Keep only the newest
      // one so repeated listing refreshes cannot grow the renderer replay
      // buffer without bound or replay stale cumulative totals on reload.
      const replay = event.type === 'status_snapshot'
        ? this.replayEvents.filter(({ event: retained }) => retained.type !== 'status_snapshot')
        : this.replayEvents;
      this.replayEvents = [...replay, envelope];
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
      const requestId = singleResponderRequestId(event);
      const responder = this.responderTarget();
      if (!responder) {
        // Nobody can service it, and — unlike the post-dispatch case
        // `reassignResponderRequests` handles — there is nothing recorded for a
        // later reopen to rescue either. The engine parked the moment the sink
        // accepted this event (`FrameAudioSink::emit_request` reports success
        // while main's socket is up, which it is: `main/index.ts` keeps the app
        // and the bridge alive on macOS when the last window closes), so
        // returning silently costs it the full deadline and a failure with
        // nothing to explain it. Answer from here instead, for the same reason
        // `reassignResponderRequests` does.
        if (requestId !== null) this.failResponderRequest(requestId, NO_RESPONDER_WINDOW_MESSAGE);
        return;
      }
      this.sendClientEvent(responder, event, false);
      if (requestId === null) return;
      if (this.outstandingResponderRequests.size >= MAX_OUTSTANDING_RESPONDER_REQUESTS) {
        this.diagnostics.add('warn', 'host', 'too many outstanding audio requests to track');
        return;
      }
      this.outstandingResponderRequests.set(requestId, { event, responder });
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
    // Drop the target FIRST, so `responderTarget()` below cannot hand the
    // request back to the window that is going away.
    this.targets.delete(webContents);
    this.reassignResponderRequests(webContents);
  }

  /**
   * Rescues every request the lost window was going to answer: hands it to
   * another live window if there is one, and otherwise answers the engine
   * from here.
   *
   * Answering from main is not a nicety. `main/index.ts` keeps the app alive
   * on macOS when the last window closes, so "start a recording, close the
   * window" leaves the engine parked with no renderer in existence that could
   * ever reply. Without this it waits out its whole deadline and fails with
   * nothing to explain it; with it the failure is immediate and says what
   * happened.
   */
  private reassignResponderRequests(lost: WebContents): void {
    for (const [requestId, pending] of [...this.outstandingResponderRequests]) {
      if (pending.responder !== lost) continue;
      this.outstandingResponderRequests.delete(requestId);
      const next = this.responderTarget();
      if (next) {
        this.outstandingResponderRequests.set(requestId, { event: pending.event, responder: next });
        this.sendClientEvent(next, pending.event, false);
        continue;
      }
      this.failResponderRequest(
        requestId,
        'the desktop window that was asked to perform this audio operation closed before it could answer',
      );
    }
  }

  /** Answers a parked engine request from main, because no renderer can. */
  private failResponderRequest(requestId: number, message: string): void {
    const command: ClientCommand = {
      type: 'audio_response',
      request_id: requestId,
      result: { type: 'failed', kind: 'unavailable', message },
    };
    try {
      this.client?.sendCommand(command);
    } catch (error) {
      // The transport may already be gone, in which case the engine's own
      // drain will fail the call. Never let this throw out of window teardown.
      this.diagnostics.add('warn', 'host', error);
    }
  }

  private clearPendingAskUserQuestion(requestId: number): void {
    this.pendingAskUserQuestionRequests.delete(requestId);
    this.pendingAskUserQuestionIds.delete(requestId);
  }

  private notifyActivityChanged(): void {
    this.opts.onActivityChanged?.();
  }

  /**
   * Forgets the interactions the engine itself drops when a turn ends.
   *
   * `outstandingResponderRequests` is deliberately NOT among them. The engine
   * drains parked audio requests from `BridgeConnection::close_connection`
   * only — `AudioResponder::drain()` has no other call site, `TurnInteractions`
   * carries the permission gate, the computer-access broker, the
   * AskUserQuestion broker and the tool-name map but no audio responder, and
   * server.rs says so in as many words: "Audio requests are drained on
   * DISCONNECT only, never at end-of-turn". `cancel_active_turn` sets a
   * cooperative token rather than aborting the tool future, and neither
   * `speech` nor `voice` overrides `Tool::interrupt_behavior`, whose default
   * `Block` keeps the parked call alive across a Stop. So an audio request is
   * still parked after this runs, and dropping the tracking here would disarm
   * the window-close rescue for exactly the case it was written for. The
   * matching clear lives in `stopBridge`, which is where the disconnect — and
   * therefore the engine's own drain — actually happens.
   */
  private clearTurnInteractions(): void {
    this.pendingPermissionIds.clear();
    this.resolvedPermissionIds.clear();
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
    const startedAt = Date.now();
    this.startPromise = (async () => {
      try {
        await this.startInternal();
      } catch (error) {
        // launchConfig runs before the child lifecycle begins. Surface failures
        // such as an unreadable Keychain credential through the same renderer
        // connection state as spawn/protocol failures.
        this.startupDiagnostic('bridge_start_failed', {
          durationMs: Date.now() - startedAt,
          error: sanitizeDiagnostic(error),
        });
        if (this.state.status !== 'error') this.fail(error);
        throw error;
      } finally {
        this.startPromise = null;
      }
    })();
    return this.startPromise;
  }

  restart(beforeRestart?: () => void | Promise<void>): Promise<void> {
    ++this.fusionLifecycleEpoch;
    if (this.opts.registerIpc !== false) this.registerIpc();
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) throw new Error('session runtime is no longer open');
      // This runs after any earlier queued lifecycle work and immediately
      // before stopping the child. Callers can re-check ownership/work here
      // to close the queueing race between IPC validation and restart.
      await beforeRestart?.();
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
    ++this.fusionLifecycleEpoch;
    if (this.opts.registerIpc !== false) this.registerIpc();
    this.restartChain = this.restartChain.catch(() => undefined).then(async () => {
      if (this.disposed) return;
      await this.stopBridge();
      this.setState({ status: 'idle' });
    });
    return this.restartChain;
  }

  private async startInternal(): Promise<void> {
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
        const command = (this.opts.readProcessCommand ?? readProcessCommand)(reusable.pid)
          ?? (this.opts.listProcessCommands ?? listProcessCommands)().find(process => process.pid === reusable.pid)?.command;
        // An inherited Codex process has an unknown refresh owner. Never attach
        // or spawn a competing process; the user must close its owning host.
        if (this.launchOAuthOverride || (this.opts.resolveOpenAiOAuth
          && (!command || /(?:^|\s)--model(?:=|\s+)openai-chatgpt\//.test(command)))) {
          throw new Error('An existing Codex runtime is still running. Close its owning application before reopening this chat.');
        }
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
    if (this.launchOAuthOverride) launch.openaiOAuth = this.launchOAuthOverride;
    if (this.launchOAuthModel) launch.model = this.launchOAuthModel;
    if (launch.openaiOAuth) {
      const oauthActivationStartedAt = Date.now();
      await this.opts.beforeOpenAiOAuthLaunch?.();
      // The previous owner may have rotated credentials while it was stopping.
      if (this.opts.resolveOpenAiOAuth) {
        launch.openaiOAuth = await this.opts.resolveOpenAiOAuth();
        if (!launch.openaiOAuth) throw new Error('Codex authentication is unavailable. Sign in again.');
      }
      this.openAiOAuthActive = true;
      this.startupDiagnostic('bridge_start_phase', {
        durationMs: Date.now() - oauthActivationStartedAt,
        generation,
        phase: 'openai_oauth_activation',
      });
    }
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

  private async connectBridgeClient(lockfilePath: string, generation: number, restorePermission = true): Promise<void> {
    this.setState({ status: 'connecting' });
    const startedAt = Date.now();
    this.startupDiagnostic('bridge_connect_started', { generation, restorePermission });
    const client = new BridgeClient({ lockfilePath, clientName: 'lingxi-electron/0.1.0' });
    this.client = client;
    this.wireClient(client, generation);
    const handshakeStartedAt = Date.now();
    const hello = await client.connect();
    this.startupDiagnostic('bridge_connect_phase', {
      durationMs: Date.now() - handshakeStartedAt,
      generation,
      phase: 'websocket_handshake',
    });
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
    if (restorePermission) await this.restorePermissionMode();
    if (generation !== this.generation || this.disposed) return;
    this.setState({ status: 'connected' });
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
      workspace: launch.workspace,
      sessionId: this.sessionId,
      bridgeDir,
      model: launch.model,
      hasApiKey: Boolean(launch.apiKey),
      hasCredentialStdin: Boolean(launch.openaiOAuth) || Object.keys(launch.providerCredentials ?? {}).length > 0
        || Object.keys(launch.pluginSecrets ?? {}).length > 0,
      trusted: launch.trusted,
      scheduledController: launch.scheduledController,
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
    // Restored usage is explicitly marked by the engine and may be separated
    // from SessionResumed by other connection events. Live deltas still need a turn.
    client.on('event', (event: ClientEvent) => {
      if (generation !== this.generation) return;
      const resumedUsage = event.type === 'usage_update' && event.is_snapshot === true;
      this.gitActivity.accept(event);
      if (event.type === 'openai_oauth_updated') {
        this.oauthPersistence = this.oauthPersistence.then(async () => {
          if (!this.opts.onOpenAiOAuthUpdated) throw new Error('OAuth persistence unavailable');
          await this.opts.onOpenAiOAuthUpdated(event.session);
        }).catch(() => {
          // Never include token-bearing event or arbitrary broker error text.
          void this.stop().catch(() => {
            this.diagnostics.add('error', 'host', 'Failed to stop Codex runtime after credential persistence failure.');
          });
          this.diagnostics.add('error', 'host', 'Failed to persist refreshed Codex authentication. Sign in again.');
          this.broadcastClientEvent({ type: 'error', kind: { type: 'internal' }, message: 'Failed to persist refreshed Codex authentication. Sign in again.' });
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
          try {
            if (!this.opts.onCronRunRequested) throw new Error('Scheduled execution is unavailable');
            const result = await this.opts.onCronRunRequested(this, event);
            response = { type: 'cron_run_completed', run_id: event.run_id, session_id: result.sessionId, summary: result.summary };
          } catch (error) {
            response = { type: 'cron_run_completed', run_id: event.run_id, error: error instanceof Error ? error.message : String(error) };
          }
          if (generation !== this.generation || client !== this.client) return;
          try {
            client.sendCommand(response);
            // Keep the controller leased until the scheduler has committed the
            // result, including when the user paused the schedule mid-run.
            const deadline = Date.now() + 15_000;
            while (generation === this.generation && Date.now() < deadline) {
              const jobs = await this.manageCron({ action: 'history', id: event.task.id });
              const run = jobs.find((job) => job.id === event.task.id)?.automation?.runs?.find((item) => item.id === event.run_id);
              if (!run || run.status !== 'running') break;
              await new Promise((resolve) => setTimeout(resolve, 50));
            }
          } catch { this.diagnostics.add('warn', 'bridge', 'Scheduled result connection closed before acknowledgement.'); }
        })().finally(() => { this.activeCronExecutions--; this.notifyActivityChanged(); });
        return;
      }
      if (isTurnOwnedEvent(event) && !this.activeTurn && !resumedUsage) {
        this.diagnostics.add('warn', 'bridge', `dropped unowned turn event: ${event.type}`);
        return;
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
        for (const providerId of this.configuredCustomProviderIds) {
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
      if (event.type === 'error' && event.message.startsWith('set_permission_mode failed:')) {
        this.pendingPermissionSwitch?.fail(new Error(sanitizeDiagnostic(event.message)));
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
      }
      if (event.type === 'ask_user_question_resolved') {
        this.clearPendingAskUserQuestion(event.request_id);
      }
      if (event.type === 'permission_request_resolved') {
        this.resolvedPermissionIds.add(event.request_id);
        this.pendingPermissionIds.delete(event.request_id);
      }
      if (
        event.type === 'turn_started'
        || event.type === 'turn_ended'
        || event.type === 'session_ended'
        || event.type === 'ask_user_question'
        || event.type === 'ask_user_question_resolved'
        || event.type === 'permission_request_resolved'
      ) this.notifyActivityChanged();
      this.updateNotifier(event);
      this.broadcastClientEvent(event);
    });
    client.on('permission', (request: PermissionRequest) => {
      if (generation !== this.generation) return;
      // NEVER drop one of these silently. The engine parks the tool for
      // `DEFAULT_PERMISSION_TIMEOUT` (300s) waiting for an answer that a
      // discarded request can never produce, then fails the tool closed with
      // "permission request timed out" — a five-minute stall whose only trace,
      // before this line existed, was the model being told its tool broke.
      // `activeTurn` mirrors the engine's turn owner; it used to go stale for
      // the whole of any turn the engine started by itself (a background-task
      // rewake, a queue drain), which is exactly when this fired.
      if (!this.activeTurn || this.cancellingTurn) {
        this.diagnostics.add('warn', 'bridge', `dropped permission request ${request.request_id}: ${this.cancellingTurn ? 'turn is cancelling' : 'no active turn'}`);
        return;
      }
      if (Number.isSafeInteger(request.request_id) && request.request_id >= 0) {
        if (this.resolvedPermissionIds.has(request.request_id)) return;
        if (!this.pendingPermissionIds.has(request.request_id) && this.pendingPermissionIds.size >= MAX_PENDING_PERMISSIONS) {
          this.diagnostics.add('warn', 'bridge', 'permission request limit reached');
          return;
        }
        this.pendingPermissionIds.set(request.request_id, request);
        this.notifyActivityChanged();
        this.broadcast(CH_PERMISSION, request);
        // Armed only AFTER the forward. Every early return above is a request
        // the renderer will never draw a prompt for, and a notification about
        // a prompt that does not exist sends the user somewhere with nothing
        // to do. Upstream's 6s delay means a prompt answered promptly — the
        // common case when the window is already in front of you — fires
        // nothing at all.
        this.opts.notifier?.permissionRequested(
          this.sessionId,
          request.request_id,
          request.kind.type === 'tool_use_confirm' ? request.kind.tool_name : request.kind.type,
          this.notificationRef,
        );
        this.opts.notifier?.setDialogsOnScreen(this.sessionId, this.pendingInteractions, this.notificationRef);
      }
    });
    client.on('computerAccess', (request: ComputerAccessRequestDto) => {
      if (generation !== this.generation) return;
      // Same reasoning as the permission handler above: the engine is parked on
      // an answer, so a discarded request is a stall, not a no-op.
      if (!this.activeTurn || this.cancellingTurn) {
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
        this.pendingComputerAccessIds.add(request.request_id);
        this.notifyActivityChanged();
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

  private assertSender(event: IpcMainInvokeEvent): void {
    const origins = this.targets.get(event.sender);
    const senderFrame = event.senderFrame;
    if (!origins || !senderFrame || senderFrame !== event.sender.mainFrame) throw new Error('unauthorized IPC sender');
    const origin = urlOrigin(senderFrame.url);
    if (!origin || !origins.has(origin)) throw new Error('unauthorized IPC origin');
  }

  sendPrompt(text: unknown, images: unknown = []): void | Promise<void> {
    if (this.archiving) throw new Error('This chat is being archived.');
    const prompt = validatePrompt(text);
    const validatedImages = validateImageRefs(images);
    if (this.opts.resolveProviderCredential) {
      const generation = this.generation;
      const client = this.requireClient();
      const token = Symbol('prompt hydration');
      this.pendingPromptHydrations.add(token);
      this.notifyActivityChanged();
      return (async () => {
        try {
          if (this.pendingModelSwitch) await this.pendingModelSwitch.promise;
          if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
          if (this.selectedModelReference) await this.ensureModelProviderCredential(this.selectedModelReference);
          // Optional Fusion routes must not gate a turn using a healthy main
          // model. Explicit /fusion still awaits these same cached loads.
          void this.ensureFusionProviderCredentials(false).catch(() => {
            this.diagnostics.add('warn', 'bridge', 'Optional Fusion credential preload failed.');
          });
          if (this.archiving || !this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) throw new Error('Prompt credential loading was interrupted.');
          this.sendPreparedPrompt(prompt, validatedImages);
        } finally {
          this.pendingPromptHydrations.delete(token);
          this.notifyActivityChanged();
        }
      })();
    }
    this.sendPreparedPrompt(prompt, validatedImages);
  }

  private sendPreparedPrompt(prompt: string, validatedImages: ReturnType<typeof validateImageRefs>): void {
    const needsIdentityCommit = !this.sessionIdentityCommitted;
    this.requireClient().sendPrompt(prompt, { images: validatedImages });
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
      this.activeTurnId = undefined;
      this.cancellingTurn = false;
      this.notifyActivityChanged();
    }
  }

  cancelTurn(turnId: unknown): void {
    const id = validateOptionalTurnId(turnId);
    ++this.fusionLifecycleEpoch;
    this.requireClient().cancel(id);
    this.pendingModelSwitch?.fail(new Error('Model switch was cancelled.'));
    if (this.pendingPromptHydrations.size > 0) {
      this.pendingPromptHydrations.clear();
      this.notifyActivityChanged();
    }
    if (this.activeTurn && (id === undefined || id === this.activeTurnId)) {
      this.cancellingTurn = true;
      this.clearTurnInteractions();
      this.notifyActivityChanged();
    }
  }

  approvePermission(requestId: number, response?: unknown): void {
    const id = validateRequestId(requestId);
    const permissionResponse = validatePermissionResponse(response);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().approvePermission(id, permissionResponse);
    this.pendingPermissionIds.delete(id);
    this.notifyActivityChanged();
  }

  denyPermission(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingPermissionIds.has(id)) throw new Error('permission request is not pending');
    this.requireClient().denyPermission(id);
    this.pendingPermissionIds.delete(id);
    this.notifyActivityChanged();
  }

  approveComputerAccess(requestId: number, response: unknown): void {
    const id = validateRequestId(requestId);
    const computerAccessResponse = validateComputerAccessResponse(response);
    if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
    this.requireClient().approveComputerAccess(id, computerAccessResponse);
    this.pendingComputerAccessIds.delete(id);
    this.notifyActivityChanged();
  }

  denyComputerAccess(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingComputerAccessIds.has(id)) throw new Error('computer access request is not pending');
    this.requireClient().denyComputerAccess(id);
    this.pendingComputerAccessIds.delete(id);
    this.notifyActivityChanged();
  }

  answerAskUserQuestion(requestId: number, answers: unknown): void {
    const id = validateRequestId(requestId);
    const validatedAnswers = validateAskUserQuestionAnswers(answers);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().answerAskUserQuestion(id, validatedAnswers);
    this.clearPendingAskUserQuestion(id);
    this.notifyActivityChanged();
  }

  cancelAskUserQuestion(requestId: number): void {
    const id = validateRequestId(requestId);
    if (!this.pendingAskUserQuestionIds.has(id)) throw new Error('AskUserQuestion request is not pending');
    this.requireClient().cancelAskUserQuestion(id);
    this.clearPendingAskUserQuestion(id);
    this.notifyActivityChanged();
  }

  private registerIpc(): void {
    if (this.ipcRegistered) return;
    this.ipcRegistered = true;
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, text: unknown, images: unknown) => {
      this.assertSender(event);
      // The engine owns provider credential resolution. The Electron host must
      // not reject a prompt merely because no secret crossed its stdin boundary;
      // CLI/TUI may already have populated the shared secure store.
      return this.sendPrompt(text, images);
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
    if (this.archiving) throw new Error('This chat is being archived.');
    // Queue behind an in-flight Codex activation instead of refusing (see
    // `openAiOAuthPreparation`). Its failure belongs to the `set_model` that
    // started it, not to whoever happened to arrive during it, so it is only
    // waited on here. The archive check is repeated because the wait is long
    // enough for the chat to have been archived meanwhile.
    //
    // Deliberately NOT extended to `restartChain`/`startPromise`: an ordinary
    // restart does not claim to be connected while it refuses commands, and
    // waiting on it would invert the contract that a restart INTERRUPTS an
    // in-flight model switch. A restart is visible to the renderer as
    // `connected: false`, which is what the model control gates on.
    const preparation = this.openAiOAuthPreparation;
    if (preparation) {
      await preparation.catch(() => undefined);
      if (this.archiving) throw new Error('This chat is being archived.');
    }
    const validated = validateClientCommand(command, this.activeWorkspace);
    if (validated.type === 'cron_manage') {
      await this.sendCronCommand(validated);
      return;
    }
    if ((validated.type === 'set_model' || validated.type === 'run_slash_command') && this.pendingModelSwitch) throw new Error('A model switch is already in progress.');
    assertCommandAllowedDuringTurn(validated, this.turnActive);
    if (validated.type === 'run_slash_command' && /^\/fusion(?:\s|$)/i.test(validated.raw.trim())
      && !/^\/fusion\s+setup\s*$/i.test(validated.raw.trim())
      && !/(?:^|\s)--retry-publication(?:\s|$)/.test(validated.raw)
      && (this.opts.resolveProviderCredential || this.opts.resolveOpenAiOAuth)) {
      let generation = this.generation;
      let client = this.requireClient();
      const token = Symbol('fusion credential hydration');
      this.pendingPromptHydrations.add(token);
      this.notifyActivityChanged();
      try {
        await this.ensureFusionOAuth(token);
        generation = this.generation;
        client = this.requireClient();
        await this.ensureFusionProviderCredentials(true);
        if (this.archiving || !this.pendingPromptHydrations.has(token) || generation !== this.generation || client !== this.client) {
          throw new Error('Fusion credential loading was interrupted.');
        }
        client.sendCommand(validated);
        this.commitFusionHistory(validated);
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
    if (validated.type === 'audio_response') {
      // The renderer answered; nothing left for a window closure to rescue.
      // Forgetting BEFORE the forward matters: a failure invented afterwards
      // would race a real reply the engine has already accepted.
      this.outstandingResponderRequests.delete(validated.request_id);
    }
    if (validated.type === 'set_model' && (this.opts.resolveProviderCredential || this.opts.resolveOpenAiOAuth || this.opts.onModelSelected)) {
      if (validated.model.startsWith('openai-chatgpt/') && !this.openAiOAuthActive && this.opts.resolveOpenAiOAuth) {
        if (this.activeTurn) throw new Error('Cancel the active turn before activating Codex authentication; this requires restarting the session engine.');
        this.preparingOpenAiOAuth = true;
        // Published BEFORE the first await so a command that arrives during the
        // restart finds something to wait on rather than a closed door.
        let preparation!: Promise<void>;
        preparation = (async () => {
          try {
            const session = await this.opts.resolveOpenAiOAuth!();
            if (session) {
              this.launchOAuthOverride = session;
              this.launchOAuthModel = validated.model;
              await this.restart();
              await this.restoreOwnedSessionIfNeeded();
            }
          } finally {
            this.launchOAuthOverride = undefined;
            this.launchOAuthModel = undefined;
            this.preparingOpenAiOAuth = false;
            if (this.openAiOAuthPreparation === preparation) this.openAiOAuthPreparation = null;
          }
        })();
        this.openAiOAuthPreparation = preparation;
        await preparation;
      }
      return this.switchModel(validated.model);
    }
    if (validated.type === 'set_permission_mode' && (this.opts.getSavedPermissionMode || this.opts.onPermissionModeSelected)) {
      return this.applyPermissionMode(validated.mode, true);
    }
    this.requireClient().sendCommand(validated);
    this.commitFusionHistory(validated);
  }

  private commitFusionHistory(command: ClientCommand): void {
    if (command.type !== 'run_slash_command' || !/^\/fusion(?:\s|$)/i.test(command.raw.trim())) return;
    this.sessionHasHistory = true;
    if (!this.sessionIdentityCommitted && this.opts.onFirstPromptSent?.() !== false) this.sessionIdentityCommitted = true;
  }

  /** Query the task registry before an implicit authentication restart. */
  async assertNoBackgroundTasks(): Promise<void> {
    const client = this.requireClient();
    const requestId = randomUUID();
    await new Promise<void>((resolve, reject) => {
      const finish = (error?: Error) => {
        clearTimeout(timer);
        client.off('event', onEvent);
        if (error) reject(error); else resolve();
      };
      const onEvent = (event: ClientEvent) => {
        if (event.type !== 'task_list_complete' || event.request_id !== requestId) return;
        if (event.error) finish(new Error('Could not check background work before activating Codex.'));
        else if (event.active_count > 0) finish(new Error('Wait for background tasks to finish before activating Codex.'));
        else finish();
      };
      const timer = setTimeout(() => finish(new Error('Background task check timed out; the session was not restarted.')), 5_000);
      client.on('event', onEvent);
      try { client.sendCommand({ type: 'task_list', request_id: requestId }); }
      catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
    });
  }

  private async ensureFusionOAuth(token: symbol): Promise<void> {
    const generation = this.generation;
    const client = this.requireClient();
    let epoch = this.fusionLifecycleEpoch;
    const assertCurrent = () => {
      if (this.disposed || this.archiving || epoch !== this.fusionLifecycleEpoch
        || generation !== this.generation || client !== this.client || !this.pendingPromptHydrations.has(token)) {
        throw new Error('Fusion credential loading was interrupted.');
      }
    };
    if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
    assertCurrent();
    if (this.openAiOAuthActive || !resolveFusionCredentialProviderIds(this.credentialRoutingSettings, true).includes('openai-chatgpt')) return;
    if (!this.opts.resolveOpenAiOAuth) throw new Error('Codex authentication is unavailable. Sign in again.');
    if (this.activeTurn || this.hasActiveAgents) throw new Error('Wait for active work to finish before activating Codex for Fusion; this requires restarting the session engine.');
    const model = this.selectedModelReference;
    this.preparingOpenAiOAuth = true;
    let preparation!: Promise<void>;
    preparation = (async () => {
      try {
        const session = await this.opts.resolveOpenAiOAuth!();
        assertCurrent();
        if (!session) throw new Error('Codex authentication is unavailable. Sign in again.');
        this.launchOAuthOverride = session;
        // OAuth is an additional Fusion provider; preserve the conversation model.
        this.launchOAuthModel = model;
        ++epoch;
        await this.restart(async () => {
          // restart() increments the epoch synchronously before entering its queue.
          assertCurrent();
          await this.assertNoBackgroundTasks();
          assertCurrent();
          if (this.activeTurn || this.hasActiveAgents) throw new Error('Wait for active work to finish before activating Codex for Fusion.');
        });
        const restartedGeneration = this.generation;
        const restartedClient = this.requireClient();
        await this.restoreOwnedSessionIfNeeded();
        if (this.disposed || this.archiving || epoch !== this.fusionLifecycleEpoch
          || restartedGeneration !== this.generation || restartedClient !== this.client) {
          throw new Error('Fusion credential loading was interrupted.');
        }
        // The intentional restart clears old hydrations; continue on its new client.
        this.pendingPromptHydrations.add(token);
      } finally {
        this.launchOAuthOverride = undefined;
        this.launchOAuthModel = undefined;
        this.preparingOpenAiOAuth = false;
        if (this.openAiOAuthPreparation === preparation) this.openAiOAuthPreparation = null;
      }
    })();
    this.openAiOAuthPreparation = preparation;
    await preparation;
  }

  private async ensureProviderCredentialCached(providerId: string): Promise<void> {
    if (this.runtimeCredentialProviders.has(providerId) || !this.opts.resolveProviderCredential) return;
    const existing = this.pendingRuntimeCredentialLoads.get(providerId);
    if (existing) return existing;
    const generation = this.generation;
    const client = this.requireClient();
    let loading!: Promise<void>;
    loading = (async () => {
      const credential = await this.opts.resolveProviderCredential!(providerId);
      if (generation !== this.generation || client !== this.client) {
        throw new Error('provider credential loading was interrupted');
      }
      if (credential) await this.cacheProviderCredential(providerId, credential);
    })().finally(() => {
      if (this.pendingRuntimeCredentialLoads.get(providerId) === loading) {
        this.pendingRuntimeCredentialLoads.delete(providerId);
      }
    });
    this.pendingRuntimeCredentialLoads.set(providerId, loading);
    return loading;
  }

  private async ensureFusionProviderCredentials(explicit: boolean): Promise<void> {
    if (!this.opts.resolveProviderCredential) return;
    const generation = this.generation;
    const client = this.requireClient();
    if (this.credentialRoutingSettings === undefined) await this.ensureCredentialSettings();
    if (generation !== this.generation || client !== this.client) throw new Error('Fusion credential loading was interrupted.');
    await Promise.all(resolveFusionCredentialProviderIds(this.credentialRoutingSettings, explicit)
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

  hasCachedProviderCredential(providerId: string): boolean {
    return this.runtimeCredentialProviders.has(providerId);
  }

  async cacheProviderCredential(providerId: string, credential: string): Promise<void> {
    await this.setProviderCredential(providerId, credential);
    this.persistedCredentialProviders.delete(providerId);
    this.runtimeCredentialProviders.add(providerId);
    this.activeCredentialProviders = new Set([
      ...this.runtimeCredentialProviders,
      ...this.persistedCredentialProviders,
    ]);
  }

  async clearCachedProviderCredential(providerId: string): Promise<void> {
    if (!this.runtimeCredentialProviders.has(providerId)) return;
    await this.deleteProviderCredential(providerId);
    this.runtimeCredentialProviders.delete(providerId);
    this.persistedCredentialProviders.delete(providerId);
    this.activeCredentialProviders = new Set([
      ...this.runtimeCredentialProviders,
      ...this.persistedCredentialProviders,
    ]);
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
        sessionResumed: false,
        hydrationStarted: false,
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
    }).then(async () => {
      await this.restoreModel();
      await this.restorePermissionMode();
    });
  }

  private completePendingSessionResumeIfReady(): void {
    const pending = this.pendingSessionResume;
    if (!pending?.sessionResumed || !pending.model || pending.hydrationStarted) return;
    pending.hydrationStarted = true;
    void this.ensureModelProviderCredential(pending.model).then(
      () => this.resolvePendingSessionResume(),
      (error: unknown) => this.rejectPendingSessionResume(
        error instanceof Error ? error : new Error(String(error)),
      ),
    );
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
    if (next.status === 'disconnected' || next.status === 'error' || next.status === 'idle') {
      this.pendingPermissionSwitch?.fail(new Error('Permission mode change was interrupted.'));
    this.pendingModelSwitch?.fail(new Error('Model switch was interrupted.'));
      // `runScheduledTurn` sets `activeTurn` by hand and only the
      // `scheduled_run_finished` handler clears it — and that handler needs the
      // pending entry this block is about to delete. Without this the latch
      // stays true forever, which blocks credential writes, Codex login, engine
      // restart and every rewriting git operation.
      if (this.pendingScheduledTurns.size > 0) {
        this.activeTurn = false;
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
    this.notifyActivityChanged();
  }

  private fail(error: unknown): void {
    const message = sanitizeDiagnostic(error);
    this.diagnostics.add('error', 'host', message);
    this.setState({ status: 'error', message });
  }

  private async stopBridge(): Promise<void> {
    await this.oauthPersistence;
    this.openAiOAuthActive = false;
    for (const pending of this.pendingCron.values()) { clearTimeout(pending.timer); pending.reject(new Error('Scheduled task connection interrupted.')); }
    this.pendingCron.clear();
    for (const pending of this.pendingScheduledTurns.values()) pending.reject(new Error('Scheduled execution interrupted.'));
    this.pendingScheduledTurns.clear();
    for (const pending of this.pendingRunBindings.values()) { clearTimeout(pending.timer); pending.reject(new Error('Scheduled execution interrupted.')); }
    this.pendingRunBindings.clear();
    ++this.generation;
    this.rejectPendingSessionResume(new Error('session resume was interrupted'));
    this.pendingPermissionSwitch?.fail(new Error('Permission mode change was interrupted.'));
    this.pendingModelSwitch?.fail(new Error('Model switch was interrupted.'));
    this.pendingCredentialSettings?.reject(new Error('Provider settings loading was interrupted.'));
    this.pendingCredentialSettings = undefined;
    this.credentialRoutingSettings = undefined;
    this.pendingPromptHydrations.clear();
    this.clearTurnInteractions();
    // Losing the connection IS the engine's own drain: `close_connection`
    // drops every parked audio sender, so each in-flight call has already
    // failed and a window closing later must not answer one nobody is waiting
    // on. This is the only place that premise holds — see
    // `clearTurnInteractions`.
    this.outstandingResponderRequests.clear();
    for (const pending of this.pendingCredentialOperations.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('bridge credential operation was interrupted'));
    }
    this.pendingCredentialOperations.clear();
    this.pendingRuntimeCredentialLoads.clear();
    for (const pending of this.pendingProviderConnectionTests.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('provider connection test was interrupted'));
    }
    this.pendingProviderConnectionTests.clear();
    this.activeWorkspace = undefined;
    this.activeWorkspaceTrusted = false;
    this.runtimeCredentialProviders.clear();
    this.persistedCredentialProviders.clear();
    this.activeCredentialProviders.clear();
    this.credentialStorageEncrypted = false;
    this.gitActivity.reset();
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
    const adoptedPid = this.adoptedPid;
    const adoptedProcessOwned = this.adoptedProcessOwned;
    this.adoptedPid = null;
    this.adoptedProcessOwned = false;
    if (adoptedPid && adoptedProcessOwned) await this.stopAdoptedBridge(adoptedPid);
    if (!adoptedPid || adoptedProcessOwned) this.removeLaunchDirectory();
    else this.launchDir = null;
  }

  private processIsAlive(pid: number): boolean {
    return processIsAlive(pid);
  }

  private async stopAdoptedBridge(pid: number): Promise<void> {
    const signal = (value: NodeJS.Signals): boolean => {
      try {
        if (process.platform !== 'win32') process.kill(-pid, value);
        else process.kill(pid, value);
        return true;
      } catch {
        return false;
      }
    };
    if (!signal('SIGINT')) return;
    const deadline = Date.now() + (this.opts.stopTimeoutMs ?? 2_000);
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
    if (existing?.connectionState.status === 'connected') {
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
    for (const runtime of projectRuntimes) this.runtimes.delete(runtime.sessionId);
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
      && (runtime.turnActive || runtime.pendingInteractions > 0)
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
    ipcMain.handle(CH_SEND_PROMPT, (event: IpcMainInvokeEvent, sessionId: unknown, text: unknown, images: unknown) => {
      this.assertSender(event);
      return this.requireById(sessionId).sendPrompt(text, images);
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
    ]) ipcMain.removeHandler(channel);
    this.registered = false;
  }
}
