import { CH_SCHEDULED } from '../shared/scheduled.js';
import type { ScheduledTaskService } from './scheduled.js';
import type { HostNotifier } from './notifications.js';
import type { GitService } from './git.js';
import { CH_GIT_REQUEST, CH_GIT_EVENT, type GitRequest } from '../shared/git.js';
import type { TerminalManager } from './terminal.js';
import { TerminalDelivery } from './terminal-delivery.js';
import {
  CH_TERMINAL_REQUEST,
  CH_TERMINAL_EVENT,
  TERMINAL_DRAFT_SESSION,
  type TerminalScope,
} from '../shared/terminal.js';
import { createRequire } from 'node:module';
import type { IpcMainInvokeEvent, WebContents } from 'electron';
import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readSync,
  realpathSync,
  writeFileSync,
} from 'node:fs';
import { open, utimes } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';

import type { AskUserQuestionRequestDto, SessionRowDto } from '@lingxi/bridge-client';
import type {
  BridgeRuntimeVersions,
  ConnectionState,
  ProviderConnectionTestResult,
  RuntimeEventEnvelope,
  SessionRef,
  SessionRuntimeSummary,
} from './bridgeTypes.js';
import type { SessionRuntimeManager } from './sessionRuntimeManager.js';
import { isSessionId } from './sessionIdentity.js';
import { WorkspaceFileSearch } from './file-search.js';
import { ProjectSessionCatalog, type ProjectSessionCatalogRow } from './session-catalog.js';
import {
  resolveCodexOAuthSession,
  resolveProviderTestCredential,
  type CredentialBrokerStatus,
  type ProviderCredentialBroker,
} from './credential-broker.js';
import {
  canonicalWorkspace,
  DiagnosticBuffer,
  diagnosticEvent,
  sanitizeDiagnostic,
  type DiagnosticEntry,
  type PinnedSessionRecord,
  type PublicSettings,
} from './host-utils.js';
import { validateClipboardText, validateClientCommand } from './validation.js';
import { readMicrophoneAccess, type MediaAccessReader } from './microphoneAccess.js';
import type { NativeAudioManager } from './audio/nativeAudioManager.js';
import {
  CH_NATIVE_AUDIO_CANCEL,
  CH_NATIVE_AUDIO_EVENT,
  CH_NATIVE_AUDIO_FINISH_LISTEN,
  CH_NATIVE_AUDIO_OPERATION,
  CH_NATIVE_AUDIO_REQUEST,
} from '../shared/nativeAudio.js';
import { PROVIDER_IDS, providerById } from '../shared/providers.js';
import { CODEX_PROVIDER_ID, loginCodex } from './codex-auth.js';
import type { SettingsStore } from './settings.js';

export interface CredentialMetadata {
  configured: boolean;
  encryptionAvailable: boolean;
  /** Display-safe fixed mask plus at most the final four credential characters. */
  credentialPreview?: string;
  /** The running engine received a credential from an external runtime source. */
  runtimeOnly?: true;
  /** Display-safe reason why the signed credential broker is unavailable. */
  storageError?: string;
  /**
   * Signed-in Codex account address, for display only. Never carries a token;
   * the credential itself stays in the main process.
   */
  codexAccountEmail?: string;
}

export interface ProviderCredentialMetadata extends CredentialMetadata {
  providerId: string;
}

export interface PluginSecretMetadata {
  pluginId: string;
  key: string;
  configured: boolean;
  maskedValue?: string;
  storageError?: string;
  restartRequired?: boolean;
}

export const CH_BOOTSTRAP = 'lingxi:bootstrap';
export const CH_SETTINGS_GET = 'lingxi:settings:get';
export const CH_SETTINGS_FILE_OPEN = 'lingxi:settings:file:open';
export const CH_SETTINGS_UPDATE = 'lingxi:settings:update';
export const CH_WORKSPACE_PICK = 'lingxi:workspace:pick';
export const CH_WORKSPACE_SET = 'lingxi:workspace:set';
export const CH_PROJECT_REMOVE = 'lingxi:project:remove';
export const CH_SESSION_PIN_SET = 'lingxi:session-pin:set';
export const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
export const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
export const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
export const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
export const CH_PROVIDER_CONNECTION_TEST = 'lingxi:provider-connection:test';
export const CH_CODEX_LOGIN = 'lingxi:codex:login';
export const CH_CODEX_CANCEL = 'lingxi:codex:cancel';
export const CH_PLUGIN_SECRET_GET = 'lingxi:plugin-secret:get';
export const CH_PLUGIN_SECRET_SET = 'lingxi:plugin-secret:set';
export const CH_PLUGIN_SECRET_CLEAR = 'lingxi:plugin-secret:clear';
export const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
export const CH_DIAGNOSTICS_GET = 'lingxi:diagnostics:get';
export const CH_DIAGNOSTICS_COPY = 'lingxi:diagnostics:copy';
export const CH_DIAGNOSTICS_EXPORT = 'lingxi:diagnostics:export';
export const CH_CLIPBOARD_WRITE_TEXT = 'lingxi:clipboard:writeText';
export const CH_OPEN_SYSTEM_SETTINGS = 'lingxi:openSystemSettings';
export const CH_MICROPHONE_ACCESS_GET = 'lingxi:microphone-access:get';
export const CH_PROJECT_SESSIONS_LIST = 'lingxi:project-sessions:list';
export const CH_SESSION_NEW = 'lingxi:session:new';
export const CH_SESSION_OPEN = 'lingxi:session:open';
export const CH_SESSION_ARCHIVE = 'lingxi:session:archive';
export const CH_SESSION_ARCHIVE_PREFLIGHT = 'lingxi:session:archive-preflight';
export const CH_SESSION_CLEAR = 'lingxi:session:clear';
export const CH_SESSION_TOUCH = 'lingxi:session:touch';
export const CH_SESSION_RENAME = 'lingxi:session:rename';
export const CH_WORKSPACE_FILE_PREVIEW = 'lingxi:workspace-file:preview';

export interface WorkspaceFilePreview {
  kind: 'text' | 'binary';
  path: string;
  size: number;
  content?: string;
  truncated: boolean;
}

const MAX_WORKSPACE_FILE_PREVIEW_BYTES = 512 * 1024;
const CREDENTIAL_STATUS_CACHE_TTL_MS = 15_000;

function credentialBrokerDisplayError(diagnostic: string): string {
  if (/ENOENT|not found|code signature|TeamIdentifier|authorize broker caller/i.test(diagnostic)) {
    return '凭据代理未正确签名或未随应用安装。macOS 开发构建需要 Apple Development 签名和有效的 provisioning profile。';
  }
  if (/protocol mismatch|protocol version|incompatible/i.test(diagnostic)) {
    return '凭据代理版本与当前应用不兼容，请升级 LingXi Desktop、CLI 和 TUI。';
  }
  return `macOS 安全凭据存储不可用：${diagnostic}`;
}

function pathEscapes(root: string, candidate: string): boolean {
  const value = relative(root, candidate);
  return value === '..' || value.startsWith(`..${sep}`) || isAbsolute(value);
}

function decodedTextLooksBinary(value: string): boolean {
  let controls = 0;
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code === 0) return true;
    if ((code < 0x20 && code !== 0x09 && code !== 0x0a && code !== 0x0c && code !== 0x0d) || code === 0x7f) {
      controls += 1;
    }
  }
  return controls > Math.max(3, Math.floor(value.length / 100));
}

function decodeWorkspacePreview(sample: Buffer, truncated: boolean): string | undefined {
  const maxTrim = truncated ? Math.min(3, sample.length) : 0;
  for (let trim = 0; trim <= maxTrim; trim += 1) {
    try {
      const value = new TextDecoder('utf-8', { fatal: true }).decode(
        trim === 0 ? sample : sample.subarray(0, sample.length - trim),
      );
      return decodedTextLooksBinary(value) ? undefined : value;
    } catch {
      // Only an incomplete UTF-8 sequence at the bounded preview edge is
      // recoverable. Never scan backwards through the whole file: an invalid
      // byte in the middle is binary/corrupt, and doing so would be O(n²).
    }
  }
  return undefined;
}

/**
 * Read a bounded UTF-8 workspace file without following symlinks or allowing
 * traversal outside the canonical project root. Binary files intentionally
 * return metadata only; the renderer never receives arbitrary bytes.
 */
export function readWorkspaceFilePreview(workspace: string, input: unknown): WorkspaceFilePreview {
  if (typeof input !== 'string' || input.length === 0 || input.length > 4_096 || input.includes('\0')) {
    throw new Error('invalid workspace file path');
  }
  if (isAbsolute(input)) throw new Error('workspace file path must be relative');
  const segments = input.split(/[\\/]/);
  if (segments.some((segment) => segment === '..')) throw new Error('workspace file path traversal is not allowed');
  const root = realpathSync.native(resolve(workspace));
  if (!lstatSync(root).isDirectory()) throw new Error('workspace root is not a directory');
  const candidate = resolve(root, input);
  const escaped = relative(root, candidate);
  if (pathEscapes(root, candidate)) {
    throw new Error('workspace file path is outside the project');
  }
  let component = root;
  for (const segment of escaped.split(sep).filter(Boolean)) {
    component = join(component, segment);
    if (lstatSync(component).isSymbolicLink()) {
      throw new Error('workspace file symlinks are not previewable');
    }
  }
  const canonical = realpathSync.native(candidate);
  if (pathEscapes(root, canonical)) {
    throw new Error('workspace file resolves outside the project');
  }
  const noFollow = process.platform === 'win32' ? 0 : constants.O_NOFOLLOW;
  const fd = openSync(candidate, constants.O_RDONLY | noFollow);
  try {
    const metadata = fstatSync(fd);
    if (!metadata.isFile()) throw new Error('workspace preview requires a regular file');
    const openedPath = lstatSync(candidate);
    if (!openedPath.isFile() || openedPath.isSymbolicLink()) {
      throw new Error('workspace preview target changed while opening');
    }
    if (openedPath.dev !== metadata.dev || openedPath.ino !== metadata.ino) {
      throw new Error('workspace preview target changed while opening');
    }
    // Re-check containment after opening. O_NOFOLLOW pins the final component
    // on Unix; this second realpath check also catches an ancestor swap on the
    // normal (non-adversarial) cross-platform path.
    if (pathEscapes(root, realpathSync.native(candidate))) {
      throw new Error('workspace file resolves outside the project');
    }
    const size = metadata.size;
    const length = Math.min(size, MAX_WORKSPACE_FILE_PREVIEW_BYTES);
    const bytes = Buffer.alloc(length);
    let offset = 0;
    while (offset < length) {
      const read = readSync(fd, bytes, offset, length - offset, offset);
      if (read === 0) break;
      offset += read;
    }
    const sample = bytes.subarray(0, offset);
    const truncated = size > MAX_WORKSPACE_FILE_PREVIEW_BYTES;
    const content = decodeWorkspacePreview(sample, truncated);
    return content === undefined
      ? { kind: 'binary', path: input, size, truncated }
      : { kind: 'text', path: input, size, content, truncated };
  } finally {
    closeSync(fd);
  }
}

/**
 * The macOS System Settings deep links this app ever opens: the `computer`
 * tool's TCC panel (Accessibility / Screen Recording), plus `microphone`
 * (Task 9 of the desktop-audio-capability plan: the voice settings page's
 * denied-microphone row). A fixed allowlist, not a renderer-supplied URL —
 * `shell.openExternal` must never be handed an arbitrary string from the
 * renderer.
 */
const SYSTEM_SETTINGS_PANES = {
  accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
  screen_recording: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
  microphone: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
  speech_recognition: 'x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition',
} as const;
const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
export type SystemSettingsPane = keyof typeof SYSTEM_SETTINGS_PANES;

export interface WorkspaceMetadata {
  path?: string;
  trusted: boolean;
  fingerprint?: string;
  recovery?: {
    state: 'missing';
    message: string;
  };
}

export interface BootstrapState {
  revision: number;
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  activeSession?: SessionRef;
  scheduledWorkspace?: string;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  providerCredentials?: ProviderCredentialMetadata[];
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
  /**
   * The three numbers the About page needs. `engine` is only known once a
   * bridge runtime has connected at least once (same source as
   * `diagnosticReport`'s `bridgeRuntime` field below) — absent, not a fake
   * value, before that.
   */
  versions: { app: string; electron: string; engine?: BridgeRuntimeVersions };
}

export interface ProjectSessionCatalogState {
  sessions: SessionRowDto[];
  error?: string;
}

function origin(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'file:' ? 'file://' : url.origin;
  } catch {
    return undefined;
  }
}

const require = createRequire(import.meta.url);
const electronModule = require('electron');
const app = typeof electronModule === 'string' ? undefined : electronModule.app;
const clipboard = typeof electronModule === 'string' ? undefined : electronModule.clipboard;
const dialog = typeof electronModule === 'string' ? undefined : electronModule.dialog;
const shell = typeof electronModule === 'string' ? undefined : electronModule.shell;
const ipcMain = (typeof electronModule === 'string' ? undefined : electronModule.ipcMain) ?? {
  handle: () => { throw new Error('ipcMain is unavailable outside Electron'); },
  removeHandler: () => undefined,
};

/** Open only existing LingXi configuration files, never arbitrary renderer paths. */
export async function openSettingsFile(
  path: unknown,
  openPath: (path: string) => Promise<string> = (filePath) => shell.openPath(filePath),
): Promise<void> {
  if (typeof path !== 'string' || path.includes('\0') || !isAbsolute(path)) {
    throw new Error('settings file path must be absolute');
  }
  const filePath = resolve(path);
  const isSettingsPath = (candidate: string) => basename(dirname(candidate)) === '.lingxi'
    && ['settings.json', 'settings.local.json'].includes(basename(candidate));
  if (!isSettingsPath(filePath)) throw new Error('unsupported settings file path');
  if (!lstatSync(filePath).isFile()) throw new Error('settings file must be a regular file');
  const canonicalPath = realpathSync.native(filePath);
  if (!isSettingsPath(canonicalPath)) throw new Error('unsupported settings file target');
  const error = await openPath(canonicalPath);
  if (error) throw new Error(error);
}

function workspaceRecovery(workspace: string, error: unknown): NonNullable<WorkspaceMetadata['recovery']> {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  const missing = code === 'ENOENT' || code === 'ENOTDIR' || /workspace path is not a directory/i.test(String(error));
  return {
    state: 'missing',
    message: missing
      ? `stored project is unavailable: ${workspace}`
      : sanitizeDiagnostic(error),
  };
}

export class HostController {
  private git?: GitService;
  private offGit?: () => void;
  private readonly gitWatches = new Map<WebContents, Map<string, Promise<() => void>>>();
  private terminals?: TerminalManager;
  private offTerminals?: () => void;
  private readonly terminalDeliveries = new Map<WebContents, TerminalDelivery>();
  private codexLoginAbort?: AbortController;
  private registered = false;
  private navigationQueue: Promise<void> = Promise.resolve();
  private bootstrapRevision = 0;
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly workspaceFiles = new WorkspaceFileSearch();
  private readonly sessionCatalog: ProjectSessionCatalog;
  private readonly catalogs = new Map<string, ProjectSessionCatalogState>();
  private readonly catalogRequestGenerations = new Map<string, number>();
  private readonly brokerConfiguredProviders = new Set<string>();
  private readonly brokerCredentialPreviews = new Map<string, string>();
  private brokerStorageError: string | undefined;
  private codexAccountEmail: string | undefined;
  private credentialStatusPromise?: Promise<void>;
  private credentialStatusCacheExpiresAt = 0;
  private offNativeAudio?: () => void;
  private scheduled?: ScheduledTaskService;
  private notifier?: HostNotifier;

  constructor(
    private readonly settings: SettingsStore,
    private readonly bridge: SessionRuntimeManager,
    private readonly diagnostics: DiagnosticBuffer,
    sessionCatalog?: ProjectSessionCatalog,
    private readonly ipc: Pick<typeof ipcMain, 'handle' | 'removeHandler'> = ipcMain,
    /**
     * Overrides the OS microphone-grant source. `undefined` means "the real
     * one" — `readMicrophoneAccess`'s own default is Electron's
     * `systemPreferences`, so a test can drive every OS answer without this
     * class ever holding a second, drift-prone copy of that wiring.
     */
    private readonly mediaAccess?: MediaAccessReader,
    private readonly credentialBroker?: ProviderCredentialBroker,
    private readonly nativeAudio?: NativeAudioManager,
    private readonly codexLogin: typeof loginCodex = loginCodex,
  ) {
    this.sessionCatalog = sessionCatalog ?? new ProjectSessionCatalog();
  }

  attachScheduled(service: ScheduledTaskService): void { this.scheduled = service; }

  /**
   * The notifier holds ARMED TIMERS, so it cannot poll the settings store —
   * a preference written while an idle timer is already counting down has to
   * be pushed at it. `CH_SETTINGS_UPDATE` above is the one writer.
   */
  attachNotifier(notifier: HostNotifier): void { this.notifier = notifier; }

  attachGit(service: GitService): void {
    this.git = service;
    this.offGit?.();
    this.offGit = service.onChanged((event) => {
      for (const target of this.gitWatches.keys()) if (!target.isDestroyed()) target.send(CH_GIT_EVENT, event);
    });
  }

  private detachGit(target: WebContents): void {
    const watches = this.gitWatches.get(target);
    this.gitWatches.delete(target);
    for (const watch of watches?.values() ?? []) void watch.then(stop => stop()).catch(() => undefined);
  }

  attachTerminals(manager: TerminalManager): void {
    this.offTerminals?.();
    this.terminals = manager;
    this.offTerminals = manager.onEvent((event) => {
      for (const delivery of this.terminalDeliveries.values()) delivery.push(event);
    });
  }

  private terminalDelivery(target: WebContents): TerminalDelivery {
    let delivery = this.terminalDeliveries.get(target);
    if (!delivery) {
      delivery = new TerminalDelivery((event) => {
        if (!target.isDestroyed()) target.send(CH_TERMINAL_EVENT, event);
      }, (paused) => this.terminals?.setOutputPaused(String(target.id), paused));
      this.terminalDeliveries.set(target, delivery);
    }
    return delivery;
  }

  private async validateSessionScope(value: unknown, projectScoped = false): Promise<TerminalScope> {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid workspace scope');
    const candidate = value as Record<string, unknown>;
    const projectPath = this.requireProject(candidate['projectPath']);
    if (this.isProjectClosing(projectPath)) throw new Error('project is closing');
    const sessionId = candidate['sessionId'];
    if (sessionId === TERMINAL_DRAFT_SESSION) {
      const active = this.settings.getPublic().activeSession;
      if (!projectScoped && active?.projectPath === projectPath) throw new Error('use the active session for this workspace');
      return { projectPath, sessionId };
    }
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    const scope = { projectPath, sessionId };
    if (this.settings.isSessionArchived(scope)) throw new Error('session is archived');
    const runtime = this.bridge.get(sessionId);
    if (runtime) {
      if (runtime.projectPath !== projectPath) throw new Error('session belongs to a different project');
    } else if (!this.terminals?.list(scope).length) {
      await this.assertSessionBelongsToProject(scope);
    }
    return scope;
  }

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const allowedOrigin = origin(rendererUrl);
    if (!allowedOrigin) throw new Error('invalid renderer URL');
    const origins = this.targets.get(webContents) ?? new Set<string>();
    origins.add(allowedOrigin);
    this.targets.set(webContents, origins);
    this.bridge.registerWindow(webContents, rendererUrl);
    const detachTerminals = () => {
      this.detachGit(webContents);
      this.terminalDeliveries.get(webContents)?.dispose();
      this.terminalDeliveries.delete(webContents);
    };
    const detachAudio = () => {
      void this.nativeAudio?.cancelUiAudioOperations(String(webContents.id), true).catch((error) => {
        this.diagnostics.add('warn', 'host', `desktop UI audio teardown failed: ${sanitizeDiagnostic(error)}`);
      });
    };
    // A replacement renderer will subscribe afresh. Its predecessor cannot
    // keep the PTY paused waiting for acknowledgements that will never arrive.
    webContents.on?.('did-start-navigation', (_event, _url, inPlace, mainFrame) => {
      if (mainFrame && !inPlace) { detachTerminals(); detachAudio(); }
    });
    webContents.on?.('render-process-gone', () => { detachTerminals(); detachAudio(); });
    webContents.once('destroyed', () => {
      this.targets.delete(webContents);
      detachTerminals();
      detachAudio();
    });
  }

  /** Re-deliver pending interaction prompts after the renderer document reloads. */
  replayPendingInteractions(webContents: WebContents): void {
    this.bridge.replayPendingInteractions(webContents);
  }

  registerIpc(): void {
    if (this.registered) return;
    this.registered = true;
    this.bridge.registerIpc();
    if (this.nativeAudio && !this.offNativeAudio) {
      this.offNativeAudio = this.nativeAudio.onEvent((audioEvent) => {
        for (const webContents of this.targets.keys()) {
          if (webContents.isDestroyed()) continue;
          if (audioEvent.type === 'input_level' && audioEvent.owner.kind === 'ui'
              && audioEvent.owner.id !== String(webContents.id)) continue;
          webContents.send(CH_NATIVE_AUDIO_EVENT, audioEvent);
        }
      });
    }
    this.ipc.handle(CH_GIT_REQUEST, async (event: IpcMainInvokeEvent, scopeValue: unknown, request: unknown) => {
      this.assertSender(event);
      if (!this.git) throw new Error('Git service is unavailable');
      const scope = await this.validateSessionScope(scopeValue, true);
      this.assertSender(event);
      if (!request || typeof request !== 'object' || Array.isArray(request)) throw new Error('invalid Git request');
      if (this.isProjectClosing(scope.projectPath)) throw new Error('project is closing');
      let watches = this.gitWatches.get(event.sender);
      if (!watches) { watches = new Map(); this.gitWatches.set(event.sender, watches); }
      if (!watches.has(scope.projectPath)) {
        const watch = this.git.watch(scope);
        watches.set(scope.projectPath, watch);
        void watch.catch(() => watches!.delete(scope.projectPath));
      }
      const result = await this.git.request(scope, request as GitRequest);
      // A non-repository watch is a no-op; retry after initialization (including
      // initialization in an external terminal) rather than caching it forever.
      if (result.status && (!result.status.repository || (request as GitRequest).kind === 'init')) {
        const previous = watches.get(scope.projectPath);
        watches.delete(scope.projectPath);
        if (previous) void previous.then(stop => stop()).catch(() => undefined);
        // Re-watch only into the map this window still owns: a renderer reload
        // racing this request runs `detachGit`, which drops the whole map, and
        // a watcher installed into the detached copy could never be stopped.
        // The `.catch` mirrors the creation path above — without it a rejected
        // watch is an unhandled rejection AND poisons the entry, so the
        // `!watches.has(...)` guard above never re-creates the watcher and the
        // Review panel silently stops refreshing for this project.
        const live = this.gitWatches.get(event.sender);
        if (result.status.repository && live === watches && this.targets.has(event.sender)) {
          const watch = this.git.watch(scope);
          watches.set(scope.projectPath, watch);
          void watch.catch(() => watches.delete(scope.projectPath));
        }
      }
      return result;
    });
    this.ipc.handle(CH_TERMINAL_REQUEST, async (event: IpcMainInvokeEvent, value: unknown) => {
      this.assertSender(event);
      if (!this.terminals) throw new Error('terminal service is unavailable');
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid terminal request');
      const request = value as Record<string, unknown>;
      if (request['kind'] === 'list' || request['kind'] === 'create') {
        return this.enqueueNavigation(async () => {
          this.assertSender(event);
          const scope = await this.validateSessionScope(request['scope']);
          this.assertSender(event);
          const delivery = this.terminalDelivery(event.sender);
          if (request['kind'] === 'list') {
            const snapshots = this.terminals!.list(scope);
            for (const snapshot of snapshots) delivery.watch(snapshot);
            return snapshots;
          }
          const snapshot = await this.terminals!.create(scope);
          delivery.watch(snapshot);
          return snapshot;
        });
      }
      const id = request['terminalId'];
      if (typeof id !== 'string' || id.length > 128) throw new Error('invalid terminal id');
      if (request['kind'] === 'acknowledge') {
        const sequence = request['sequence'];
        if (!Number.isSafeInteger(sequence) || (sequence as number) < 0) throw new Error('invalid terminal sequence');
        this.terminalDeliveries.get(event.sender)?.acknowledge(id, sequence as number);
        return;
      }
      const terminal = this.terminals.get(id);
      if (!terminal) throw new Error('terminal no longer exists');
      if (!this.terminalDeliveries.get(event.sender)?.has(id)) throw new Error('terminal is not attached to this window');
      if (!this.settings.isTrustedWorkspace(terminal.scope.projectPath) || this.isProjectClosing(terminal.scope.projectPath)) throw new Error('project is unavailable');
      switch (request['kind']) {
        case 'input':
          if (typeof request['data'] !== 'string' || request['data'].length > 65_536) throw new Error('invalid terminal input');
          return this.terminals.input(id, request['data']);
        case 'resize':
          if (!Number.isInteger(request['cols']) || !Number.isInteger(request['rows'])
            || (request['cols'] as number) < 1 || (request['cols'] as number) > 1000
            || (request['rows'] as number) < 1 || (request['rows'] as number) > 1000) throw new Error('invalid terminal dimensions');
          return this.terminals.resize(id, request['cols'] as number, request['rows'] as number);
        case 'close': return this.terminals.close(id);
        default: throw new Error('unsupported terminal request');
      }
    });
    this.ipc.handle(CH_BOOTSTRAP, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.bootstrap();
    });
    this.ipc.handle(CH_SETTINGS_FILE_OPEN, async (event: IpcMainInvokeEvent, path: unknown) => {
      this.assertSender(event);
      await openSettingsFile(path);
    });
    this.ipc.handle(CH_SETTINGS_GET, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.settings.getPublic(); });
    this.ipc.handle(CH_SETTINGS_UPDATE, async (event: IpcMainInvokeEvent, patch: unknown) => {
      this.assertSender(event);
      if (!patch || typeof patch !== 'object' || Array.isArray(patch)) throw new Error('invalid settings patch');
      const keys = Object.keys(patch);
      if (keys.some((key) => key !== 'theme' && key !== 'collapseThoughtsByDefault' && key !== 'model' && key !== 'apiBaseUrl' && key !== 'voice' && key !== 'voiceRevision' && key !== 'notifications' && key !== 'modelPickerVisibility' && key !== 'sidebar')) throw new Error('unsupported setting');
      const restartsBridge = 'apiBaseUrl' in patch;
      if (restartsBridge) this.assertNoActiveTurn();
      // `model` is applied to a live session through `set_model`, then mirrored
      // here by `onModelChanged`; persisting the default must not restart any
      // session. `voice` is a separately revisioned device-local configuration;
      // each operation snapshots it from main when dispatched. `notifications` does not restart anything
      // either, but it IS pushed at the notifier below: the notifier lives in
      // the main process and holds armed timers, so it cannot re-read a value
      // it is never told about. Only the legacy Anthropic API base changes the
      // construction of an already-running provider client.
      const result = this.settings.update(patch as {
        theme?: 'dark' | 'light' | 'system';
        collapseThoughtsByDefault?: boolean;
        model?: string | null;
        apiBaseUrl?: string | null;
        voice?: unknown;
        voiceRevision?: number;
        notifications?: unknown;
        modelPickerVisibility?: unknown;
        sidebar?: unknown;
      });
      if ('notifications' in patch) this.notifier?.setPreferences(result.notifications);
      if (restartsBridge) await this.restartIfConfigured();
      return result;
    });
    this.ipc.handle(CH_WORKSPACE_PICK, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const result = await dialog.showOpenDialog({ properties: ['openDirectory', 'createDirectory'], securityScopedBookmarks: false });
        if (result.canceled || !result.filePaths[0]) return null;
        return this.selectWorkspaceInternal(result.filePaths[0], true);
      });
    });
    this.ipc.handle(CH_WORKSPACE_SET, async (event: IpcMainInvokeEvent, workspace: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        if (typeof workspace !== 'string') throw new Error('invalid workspace path');
        const canonical = canonicalWorkspace(workspace);
        if (!this.settings.hasProject(canonical)) throw new Error('project is not in the project list');
        return this.selectWorkspaceInternal(canonical, false);
      });
    });
    this.ipc.handle(CH_PROJECT_SESSIONS_LIST, async (event: IpcMainInvokeEvent, projectPath: unknown) => {
      this.assertSender(event);
      const project = this.requireProject(projectPath);
      const result = await this.loadProjectSessions(project);
      return { projectPath: project, ...result };
    });
    this.ipc.handle(CH_SESSION_NEW, async (event: IpcMainInvokeEvent, projectPath: unknown, model: unknown) => {
      this.assertSender(event);
      const requestedAt = Date.now();
      return this.enqueueNavigation(async () => {
        const project = this.requireProject(projectPath);
        if (model !== undefined && typeof model !== 'string') throw new Error('invalid model');
        const navigationStartedAt = Date.now();
        this.diagnostics.add('info', 'host', diagnosticEvent('session_new_started', {
          model: typeof model === 'string' ? model : undefined,
          projectPath: project,
          queueWaitMs: navigationStartedAt - requestedAt,
        }));
        try {
          const runtimeStartedAt = Date.now();
          const ref = await this.bridge.newSession(project, model as string | undefined);
          const runtimeDurationMs = Date.now() - runtimeStartedAt;
          this.diagnostics.add('info', 'host', diagnosticEvent('session_new_runtime_ready', {
            durationMs: runtimeDurationMs,
            projectPath: project,
            sessionId: ref.sessionId,
          }));
          this.settings.activateProject(project);
          this.terminals?.migrateScope({ projectPath: project, sessionId: TERMINAL_DRAFT_SESSION }, ref);
          this.settings.setActiveSessionDraft(ref);
          const bootstrapStartedAt = Date.now();
          const snapshot = await this.bootstrap();
          this.diagnostics.add('info', 'host', diagnosticEvent('session_new_completed', {
            bootstrapDurationMs: Date.now() - bootstrapStartedAt,
            durationMs: Date.now() - requestedAt,
            projectPath: project,
            runtimeDurationMs,
            sessionId: ref.sessionId,
          }));
          return snapshot;
        } catch (error) {
          this.diagnostics.add('error', 'host', diagnosticEvent('session_new_failed', {
            durationMs: Date.now() - requestedAt,
            error: sanitizeDiagnostic(error),
            projectPath: project,
          }));
          throw error;
        }
      });
    });
    this.ipc.handle(CH_SESSION_OPEN, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const project = this.requireProject(projectPath);
        if (!isSessionId(sessionId)) throw new Error('invalid session id');
        const ref = { projectPath: project, sessionId } satisfies SessionRef;
        return this.openSessionAndActivateInternal(ref);
      });
    });
    this.ipc.handle(CH_SCHEDULED, async (event: IpcMainInvokeEvent, input: unknown) => {
      this.assertSender(event);
      if (!this.scheduled || !input || typeof input !== 'object') throw new Error('Scheduled task service unavailable');
      const request = input as Record<string, unknown>;
      if (request['action'] === 'scopes') return this.scheduled.scopes();
      if (typeof request['scopeId'] !== 'string') throw new Error('Invalid scheduled scope');
      if (request['action'] === 'context') return this.scheduled.context(request['scopeId']);
      if (request['action'] === 'manage') {
        const validated = validateClientCommand({ type: 'cron_manage', request_id: 'host-scheduled', request: request['request'] });
        if (validated.type !== 'cron_manage') throw new Error('Invalid scheduled request');
        return this.scheduled.manage(request['scopeId'], validated.request);
      }
      if (request['action'] === 'open') {
        if (!isSessionId(request['sessionId'])) throw new Error('Invalid scheduled session');
        const projectPath = this.scheduled.resolveScope(request['scopeId']);
        return this.enqueueNavigation(() => this.openSessionAndActivateInternal({ projectPath, sessionId: request['sessionId'] as string }));
      }
      throw new Error('Invalid scheduled action');
    });
    this.ipc.handle(CH_SESSION_ARCHIVE_PREFLIGHT, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown) => {
      this.assertSender(event);
      const ref = this.archiveRef(projectPath, sessionId);
      const row = await this.assertSessionBelongsToProject(ref);
      return this.bridge.withBackgroundSession(ref, row?.empty_session === true, row?.resume_model, async (runtime) => {
        const jobs = await runtime.manageCron({ action: 'list' });
        return jobs.filter((job) => job.automation?.targetSessionId === ref.sessionId || job.automation?.ownedSessionId === ref.sessionId);
      });
    });
    this.ipc.handle(CH_SESSION_ARCHIVE, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(() => this.archiveSessionInternal(this.archiveRef(projectPath, sessionId)));
    });
    this.ipc.handle(CH_SESSION_CLEAR, async (event: IpcMainInvokeEvent, sessionId: unknown, name?: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId)) throw new Error('invalid session id');
      if (name !== undefined && typeof name !== 'string') throw new Error('invalid session name');
      const clearName = typeof name === 'string' ? name.trim() : '';
      if (clearName.length > 200 || /[\u0000-\u001f\u007f]/.test(clearName)) {
        throw new Error('session name must be 1–200 printable characters');
      }
      await this.enqueueNavigation(() => this.clearActiveSession(sessionId, clearName || undefined));
    });
    this.ipc.handle(CH_SESSION_TOUCH, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const ref = this.archiveRef(projectPath, sessionId);
        const row = await this.assertSessionBelongsToProject(ref) ?? await this.sessionCatalog.find(ref.projectPath, ref.sessionId);
        if (!row) throw new Error('session must be persisted before it can be reordered');
        const sessionPath = realpathSync.native(row.path);
        if (!lstatSync(sessionPath).isFile()) throw new Error('session path is not a file');
        const now = new Date();
        await utimes(sessionPath, now, now);
        this.sessionCatalog.invalidate(ref.projectPath);
        const catalog = await this.loadProjectSessions(ref.projectPath);
        return { projectPath: ref.projectPath, ...catalog };
      });
    });
    this.ipc.handle(CH_SESSION_RENAME, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, title: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const ref = this.archiveRef(projectPath, sessionId);
        if (typeof title !== 'string') throw new Error('invalid session title');
        const customTitle = title.trim();
        if (!customTitle || customTitle.length > 200 || /[\u0000-\u001f\u007f]/.test(customTitle)) {
          throw new Error('session title must be 1–200 printable characters');
        }
        const catalog = await this.persistSessionTitle(ref, customTitle);
        return { projectPath: ref.projectPath, ...catalog };
      });
    });
    this.ipc.handle(CH_PROJECT_REMOVE, async (event: IpcMainInvokeEvent, projectPath: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const project = this.requireProject(projectPath);
        return this.removeProjectInternal(project);
      });
    });
    this.ipc.handle(CH_SESSION_PIN_SET, (event: IpcMainInvokeEvent, input: unknown, pinned: unknown) => {
      this.assertSender(event);
      if (!input || typeof input !== 'object' || Array.isArray(input) || typeof pinned !== 'boolean') {
        throw new Error('invalid pinned session');
      }
      const candidate = input as Record<string, unknown>;
      const projectPath = candidate['projectPath'];
      const sessionId = candidate['sessionId'];
      const title = candidate['title'];
      if (typeof projectPath !== 'string' || !this.settings.isTrustedWorkspace(projectPath)) {
        throw new Error('pinned session project is not in the project list');
      }
      if (typeof sessionId !== 'string' || !SESSION_ID_PATTERN.test(sessionId)) {
        throw new Error('invalid pinned session id');
      }
      if (pinned && (typeof title !== 'string' || title.trim().length === 0 || title.length > 512)) {
        throw new Error('invalid pinned session title');
      }
      const record: PinnedSessionRecord = {
        projectPath,
        sessionId,
        title: typeof title === 'string' && title.trim() ? title.trim() : 'Untitled session',
        pinnedAt: new Date().toISOString(),
      };
      return this.settings.setSessionPinned(record, pinned);
    });
    this.ipc.handle(CH_WORKSPACE_FILES_SEARCH, async (event: IpcMainInvokeEvent, query: unknown) => {
      this.assertSender(event);
      const workspace = this.requireWorkspace();
      if (!this.settings.hasProject(workspace)) throw new Error('project is not in the project list');
      return this.workspaceFiles.search(workspace, query);
    });
    this.ipc.handle(CH_WORKSPACE_FILE_PREVIEW, async (event: IpcMainInvokeEvent, sessionId: unknown, path: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId)) throw new Error('invalid session id');
      const active = this.settings.getPublic().activeSession;
      if (!active || active.sessionId !== sessionId) throw new Error('session is not active');
      const runtime = this.bridge.get(sessionId);
      if (!runtime) throw new Error('session runtime is not open');
      const project = this.requireProject(runtime.projectPath);
      if (canonicalWorkspace(active.projectPath) !== project) throw new Error('session project mismatch');
      return readWorkspaceFilePreview(project, path);
    });
    this.ipc.handle(CH_PROVIDER_CREDENTIALS_GET, async (event: IpcMainInvokeEvent, providerId?: unknown) => {
      this.assertSender(event);
      if (this.credentialBroker) {
        await this.refreshCredentialBrokerStatus(
          providerId !== undefined ? (await this.requireCredentialProvider(providerId)).id : undefined,
        );
      } else if (providerId !== undefined) {
        const provider = await this.requireCredentialProvider(providerId);
        const runtime = this.currentRuntime();
        if (runtime?.connectionState.status === 'connected') {
          await runtime.listProviderCredentials([provider.id], [provider.id]);
        }
      }
      return this.providerCredentialSnapshot();
    });
    this.ipc.handle(CH_PROVIDER_CREDENTIAL_SET, async (event: IpcMainInvokeEvent, providerId: unknown, credential: unknown) => {
      this.assertSender(event);
      const provider = await this.requireCredentialProvider(providerId);
      if (provider.id === CODEX_PROVIDER_ID) throw new Error('请使用 Codex 账号登录。');
      if (typeof credential !== 'string') throw new Error('invalid credential');
      this.assertNoActiveTurn();
      const credentialMetadata = this.credentialBroker
        ? await this.setCredentialThroughBroker(provider.id, credential)
        : await this.setCredentialThroughRuntime(provider.id, credential);
      return { credential: credentialMetadata, settings: this.settings.getPublic() };
    });
    this.ipc.handle(CH_CODEX_LOGIN, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      this.assertNoActiveTurn();
      if (!this.credentialBroker) throw new Error('Codex 登录需要可用的安全凭据存储。');
      if (this.codexLoginAbort) throw new Error('Codex 登录正在进行。');
      const abort = new AbortController();
      this.codexLoginAbort = abort;
      const cancel = () => abort.abort();
      event.sender.once('destroyed', cancel);
      try {
        await this.credentialBroker.health();
        const session = await this.codexLogin({ openExternal: (url) => shell.openExternal(url), signal: abort.signal });
        this.assertNoActiveTurn();
        const stored = await this.bridge.withCodexAuthMutation(async () => {
          if (abort.signal.aborted) throw new Error('Codex 登录已取消。');
          return this.credentialBroker!.set(CODEX_PROVIDER_ID, JSON.stringify(session))
            .catch(() => { throw new Error('Codex 登录凭据无法保存到安全存储，请重试。'); });
        });
        if (!stored.configured) throw new Error('Codex 登录凭据未能保存。');
        this.brokerConfiguredProviders.add(CODEX_PROVIDER_ID);
        this.brokerCredentialPreviews.delete(CODEX_PROVIDER_ID);
        this.brokerStorageError = undefined;
        this.invalidateCredentialBrokerStatus();
        this.bridge.invalidateLaunchConfigCache();
        this.codexAccountEmail = session.email;
        // Authentication is durable even if the unrelated engine launch fails.
        // Keep metadata authoritative; the runtime exposes its own connection error.
        await this.restartIfConfigured().catch(() => {
          this.diagnostics.add('warn', 'host', 'Codex authentication saved; the session runtime could not restart.');
        });
        return { credential: this.providerCredentialMetadata(CODEX_PROVIDER_ID), settings: this.settings.getPublic() };
      } finally {
        event.sender.removeListener('destroyed', cancel);
        if (this.codexLoginAbort === abort) this.codexLoginAbort = undefined;
      }
    });
    this.ipc.handle(CH_CODEX_CANCEL, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      this.codexLoginAbort?.abort();
    });
    this.ipc.handle(CH_PROVIDER_CREDENTIAL_CLEAR, async (event: IpcMainInvokeEvent, providerId: unknown) => {
      this.assertSender(event);
      const provider = await this.requireCredentialProvider(providerId);
      this.assertNoActiveTurn();
      if (provider.id === CODEX_PROVIDER_ID) {
        this.codexLoginAbort?.abort();
        if (!this.credentialBroker) throw new Error('Codex 安全凭据存储不可用。');
        await this.bridge.withCodexAuthMutation(() => this.credentialBroker!.delete(CODEX_PROVIDER_ID));
        this.brokerConfiguredProviders.delete(CODEX_PROVIDER_ID);
        this.brokerCredentialPreviews.delete(CODEX_PROVIDER_ID);
        this.codexAccountEmail = undefined;
        this.bridge.invalidateLaunchConfigCache();
        return this.providerCredentialMetadata(CODEX_PROVIDER_ID);
      }
      if (this.credentialBroker) await this.clearCredentialThroughBroker(provider.id);
      else {
        this.requireWorkspace();
        await this.requireCurrentRuntime().deleteProviderCredential(provider.id);
        this.bridge.invalidateLaunchConfigCache();
      }
      return this.providerCredentialMetadata(provider.id);
    });
    this.ipc.handle(CH_PROVIDER_CONNECTION_TEST, async (
      event: IpcMainInvokeEvent,
      providerId: unknown,
      credentialOverride?: unknown,
    ): Promise<ProviderConnectionTestResult> => {
      this.assertSender(event);
      const provider = this.requireProvider(providerId);
      if (provider.id === CODEX_PROVIDER_ID) throw new Error('Codex 登录使用 ChatGPT 认证，不支持 API Key 连接测试。');
      if (credentialOverride !== undefined && typeof credentialOverride !== 'string') {
        throw new Error('invalid credential');
      }
      this.assertNoActiveTurn();
      this.requireWorkspace();
      const configuredBase = provider.id === 'anthropic'
        ? this.settings.getPublic().apiBaseUrl
        : undefined;
      const reference = provider.defaultModel ?? '';
      const slash = reference.indexOf('/');
      const model = slash >= 0 ? reference.slice(slash + 1) : reference;
      const testCredential = await resolveProviderTestCredential(
        provider.id,
        credentialOverride,
        this.credentialBroker,
      );
      return this.requireCurrentRuntime().testProviderConnection(
        provider.id,
        configuredBase ?? provider.defaultApiBase,
        model,
        testCredential,
      );
    });
    this.ipc.handle(CH_PLUGIN_SECRET_GET, async (
      event: IpcMainInvokeEvent,
      pluginId: unknown,
      key: unknown,
    ) => {
      this.assertSender(event);
      if (typeof pluginId !== 'string' || typeof key !== 'string') throw new Error('invalid plugin secret reference');
      if (!this.credentialBroker) {
        return { pluginId, key, configured: false, storageError: '安全凭据代理不可用。' } satisfies PluginSecretMetadata;
      }
      const preview = await this.credentialBroker.previewPluginSecret(pluginId, key);
      return {
        pluginId: preview.pluginId,
        key: preview.key,
        configured: preview.configured,
        ...(preview.maskedValue ? { maskedValue: preview.maskedValue } : {}),
      } satisfies PluginSecretMetadata;
    });
    this.ipc.handle(CH_PLUGIN_SECRET_SET, async (
      event: IpcMainInvokeEvent,
      pluginId: unknown,
      key: unknown,
      secret: unknown,
    ) => {
      this.assertSender(event);
      if (typeof pluginId !== 'string' || typeof key !== 'string' || typeof secret !== 'string') {
        throw new Error('invalid plugin secret');
      }
      if (!this.credentialBroker) throw new Error('secure plugin credential broker is unavailable');
      const preview = await this.credentialBroker.setPluginSecret(pluginId, key, secret);
      this.bridge.invalidateLaunchConfigCache();
      let restartRequired = this.hasActiveWork();
      if (!restartRequired) {
        try {
          await this.restartIfConfigured();
        } catch (error) {
          restartRequired = true;
          this.diagnostics.add('warn', 'host', error);
        }
      }
      return {
        pluginId: preview.pluginId,
        key: preview.key,
        configured: preview.configured,
        ...(preview.maskedValue ? { maskedValue: preview.maskedValue } : {}),
        ...(restartRequired ? { restartRequired: true } : {}),
      } satisfies PluginSecretMetadata;
    });
    this.ipc.handle(CH_PLUGIN_SECRET_CLEAR, async (
      event: IpcMainInvokeEvent,
      pluginId: unknown,
      key: unknown,
    ) => {
      this.assertSender(event);
      if (typeof pluginId !== 'string' || typeof key !== 'string') throw new Error('invalid plugin secret reference');
      if (!this.credentialBroker) throw new Error('secure plugin credential broker is unavailable');
      await this.credentialBroker.deletePluginSecret(pluginId, key);
      this.bridge.invalidateLaunchConfigCache();
      let restartRequired = this.hasActiveWork();
      if (!restartRequired) {
        try {
          await this.restartIfConfigured();
        } catch (error) {
          restartRequired = true;
          this.diagnostics.add('warn', 'host', error);
        }
      }
      return { pluginId, key, configured: false, ...(restartRequired ? { restartRequired: true } : {}) } satisfies PluginSecretMetadata;
    });
    this.ipc.handle(CH_BRIDGE_RESTART, async (event: IpcMainInvokeEvent, sessionId: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId)) throw new Error('invalid session id');
      const runtime = this.bridge.get(sessionId);
      if (!runtime) throw new Error(`session runtime is not open: ${sessionId}`);
      const ref = { projectPath: runtime.projectPath, sessionId } satisfies SessionRef;
      const recovering = runtime.connectionState.status !== 'connected';
      this.assertRestartAllowed(ref, runtime, recovering);
      await this.bridge.restart(ref, () => {
        const current = this.bridge.get(sessionId);
        if (!current || current !== runtime) {
          throw new Error(`session runtime is no longer open: ${sessionId}`);
        }
        this.assertRestartAllowed(ref, current, recovering);
      });
    });
    this.ipc.handle(CH_DIAGNOSTICS_GET, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.diagnostics.snapshot(); });
    this.ipc.handle(CH_DIAGNOSTICS_COPY, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      clipboard.writeText(this.diagnosticReport());
    });
    this.ipc.handle(CH_CLIPBOARD_WRITE_TEXT, (event: IpcMainInvokeEvent, text: unknown) => {
      this.assertSender(event);
      clipboard.writeText(validateClipboardText(text));
    });
    this.ipc.handle(CH_DIAGNOSTICS_EXPORT, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      const result = await dialog.showSaveDialog({
        title: 'Export sanitized LingXi diagnostics',
        defaultPath: `LingXi-Code-diagnostics-${new Date().toISOString().replace(/[:.]/g, '-')}.json`,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (result.canceled || !result.filePath) return null;
      writeFileSync(result.filePath, this.diagnosticReport(), { encoding: 'utf8', mode: 0o600 });
      return result.filePath;
    });
    this.ipc.handle(CH_OPEN_SYSTEM_SETTINGS, async (event: IpcMainInvokeEvent, pane: unknown) => {
      this.assertSender(event);
      if (typeof pane !== 'string' || !(pane in SYSTEM_SETTINGS_PANES)) {
        throw new Error('unsupported System Settings pane');
      }
      await shell.openExternal(SYSTEM_SETTINGS_PANES[pane as SystemSettingsPane]);
    });
    // The renderer cannot read this itself: `navigator.permissions.query`
    // answers `main/index.ts`'s own `setPermissionCheckHandler`, which grants
    // the app's renderer `media` unconditionally and never consults the OS —
    // so the voice page's 麦克风权限 row used to say 已授权 while every
    // recording failed. `systemPreferences.getMediaAccessStatus` is the real
    // grant, and it lives here.
    this.ipc.handle(CH_MICROPHONE_ACCESS_GET, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return readMicrophoneAccess(this.mediaAccess);
    });
    this.ipc.handle(CH_NATIVE_AUDIO_REQUEST, async (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      if (!this.nativeAudio) throw new Error('native audio is unavailable on this host');
      return this.nativeAudio.request(command);
    });
    this.ipc.handle(CH_NATIVE_AUDIO_OPERATION, async (
      event: IpcMainInvokeEvent,
      operation: unknown,
      configurationRevision?: unknown,
      configurationOverride?: unknown,
    ) => {
      this.assertSender(event);
      if (!this.nativeAudio) throw new Error('native audio is unavailable on this host');
      return this.nativeAudio.executeUiAudioOperation(
        operation,
        String(event.sender.id),
        configurationRevision as number | undefined,
        configurationOverride,
      );
    });
    this.ipc.handle(CH_NATIVE_AUDIO_CANCEL, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      await this.nativeAudio?.cancelUiAudioOperations(String(event.sender.id));
    });
    this.ipc.handle(CH_NATIVE_AUDIO_FINISH_LISTEN, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      await this.nativeAudio?.finishUiAudioListen(String(event.sender.id));
    });
  }

  private assertSender(event: IpcMainInvokeEvent): void {
    const origins = this.targets.get(event.sender);
    const frame = event.senderFrame;
    if (!origins || !frame || frame !== event.sender.mainFrame) throw new Error('unauthorized IPC sender');
    const senderOrigin = origin(frame.url);
    if (!senderOrigin || !origins.has(senderOrigin)) throw new Error('unauthorized IPC origin');
  }

  private workspace(): WorkspaceMetadata {
    const workspace = this.settings.getPublic().activeSession?.projectPath ?? this.settings.getWorkspace();
    if (!workspace) return { trusted: false };
    try {
      const trust = this.settings.getTrust(workspace);
      return { path: workspace, fingerprint: trust.fingerprint, trusted: this.settings.isTrustedWorkspace(workspace) };
    } catch (error) {
      this.diagnostics.add('warn', 'host', error);
      return { path: workspace, trusted: false, recovery: workspaceRecovery(workspace, error) };
    }
  }

  private async bootstrap(): Promise<BootstrapState> {
    const bootstrapStartedAt = Date.now();
    if (this.credentialBroker) {
      const credentialStatusStartedAt = Date.now();
      try {
        await this.refreshCredentialBrokerStatus();
        this.diagnostics.add('info', 'host', diagnosticEvent('bootstrap_phase_completed', {
          durationMs: Date.now() - credentialStatusStartedAt,
          phase: 'credential_broker_status',
        }));
      } catch (error) {
        const diagnostic = sanitizeDiagnostic(error);
        this.brokerStorageError = credentialBrokerDisplayError(diagnostic);
        this.brokerConfiguredProviders.clear();
        this.brokerCredentialPreviews.clear();
        this.diagnostics.add('warn', 'host', `credential broker status refresh failed: ${diagnostic}`);
        this.diagnostics.add('warn', 'host', diagnosticEvent('bootstrap_phase_failed', {
          durationMs: Date.now() - credentialStatusStartedAt,
          error: diagnostic,
          phase: 'credential_broker_status',
        }));
      }
    }
    const revision = ++this.bootstrapRevision;
    const activeSession = this.settings.getPublic().activeSession;
    const activeRuntime = this.currentRuntime();
    const legacy = this.bridge as unknown as {
      pendingAskUserQuestions?: readonly AskUserQuestionRequestDto[];
      connectionState?: ConnectionState;
    };
    const pendingAskUserQuestions = activeRuntime?.pendingAskUserQuestions ?? legacy.pendingAskUserQuestions ?? [];
    // Same source `diagnosticReport` below already reads for its
    // `bridgeRuntime` field — reused here rather than recomputed, so the
    // About page's engine version and the exported diagnostic report can
    // never disagree.
    const engineVersions = activeRuntime?.runtimeVersions
      ?? (this.bridge as unknown as { runtimeVersions?: BridgeRuntimeVersions }).runtimeVersions;
    const snapshot = {
      revision,
      scheduledWorkspace: this.settings.scheduledWorkspace,
      settings: this.settings.getPublic(),
      workspace: this.workspace(),
      ...(activeSession ? { activeSession: { ...activeSession } } : {}),
      runtimes: this.runtimeSummaries(),
      projectCatalogs: Object.fromEntries([...this.catalogs.entries()].map(([path, state]) => [path, {
        sessions: state.sessions.map((session) => ({ ...session })),
        ...(state.error ? { error: state.error } : {}),
      }])),
      providerCredentials: this.providerCredentialSnapshot(),
      ...(this.credentialBroker
        ? { credentialBrokerAvailable: this.brokerStorageError === undefined }
        : {}),
      ...(pendingAskUserQuestions.length > 0 ? { pendingAskUserQuestions: [...pendingAskUserQuestions] } : {}),
      connection: activeRuntime?.connectionState ?? legacy.connectionState ?? { status: 'idle' },
      versions: {
        app: app?.getVersion?.() ?? 'unknown',
        electron: process.versions.electron,
        ...(engineVersions ? { engine: engineVersions } : {}),
      },
      diagnostics: this.diagnostics.snapshot(),
    };
    this.diagnostics.add('info', 'host', diagnosticEvent('bootstrap_completed', {
      activeSessionId: activeSession?.sessionId,
      catalogCount: this.catalogs.size,
      durationMs: Date.now() - bootstrapStartedAt,
      revision,
      runtimeCount: this.runtimeSummaries().length,
    }));
    return snapshot;
  }

  private diagnosticReport(): string {
    const workspace = this.workspace();
    return `${JSON.stringify({
      schemaVersion: 1,
      generatedAt: new Date().toISOString(),
      runtime: {
        appVersion: app?.getVersion?.() ?? 'unknown',
        electronVersion: process.versions.electron,
        platform: process.platform,
        architecture: process.arch,
      },
      workspace: {
        path: workspace.path,
        trusted: workspace.trusted,
        fingerprint: workspace.fingerprint,
        recovery: workspace.recovery,
      },
      // The signed-in account address is personal data and stays out of the
      // exported report; the settings page reads it over IPC instead.
      providerCredentials: this.providerCredentialSnapshot()
        .map(({ codexAccountEmail: _email, ...metadata }) => metadata),
      connection: this.currentRuntime()?.connectionState ?? { status: 'idle' },
      bridgeRuntime: this.currentRuntime()?.runtimeVersions
        ?? (this.bridge as unknown as { runtimeVersions?: unknown }).runtimeVersions,
      diagnostics: this.diagnostics.snapshot(),
    }, null, 2)}\n`;
  }

  private requireWorkspace(): string {
    const workspace = this.settings.getPublic().activeSession?.projectPath ?? this.settings.getWorkspace();
    if (!workspace) throw new Error('select a workspace first');
    return workspace;
  }

  private requireProject(value: unknown): string {
    if (typeof value !== 'string') throw new Error('invalid project path');
    if (value === this.settings.scheduledWorkspace) mkdirSync(value, { recursive: true, mode: 0o700 });
    const project = canonicalWorkspace(value);
    if (!this.settings.isTrustedWorkspace(project)) throw new Error('project is not in the project list');
    return project;
  }

  private currentRuntime() {
    const ref = this.settings.getPublic().activeSession;
    const manager = this.bridge as unknown as { get?: (sessionId: string) => ReturnType<SessionRuntimeManager['get']> };
    return ref && manager.get ? manager.get(ref.sessionId) : undefined;
  }

  private runtimeSummaries(): SessionRuntimeSummary[] {
    const manager = this.bridge as unknown as { runtimeSummaries?: SessionRuntimeSummary[] };
    return manager.runtimeSummaries ? manager.runtimeSummaries.map((summary) => ({
      ...summary,
      connection: { ...summary.connection },
    })) : [];
  }

  private hasActiveWork(projectPath?: string): boolean {
    if (projectPath) return this.bridge.hasActiveWork(projectPath);
    const active = this.settings.getPublic().activeProject;
    if (active && this.bridge.hasActiveWork(active)) return true;
    return Boolean((this.bridge as unknown as { turnActive?: boolean }).turnActive);
  }

  private providerCredentialSnapshot(): ProviderCredentialMetadata[] {
    const runtime = this.currentRuntime();
    const legacy = this.bridge as unknown as {
      connectionState?: ConnectionState;
      activeCredentialProviderIds?: readonly string[];
      persistedCredentialProviderIds?: readonly string[];
      providerCredentialStorageEncrypted?: boolean;
      providerCredentialPreviews?: Readonly<Record<string, string>>;
    };
    const activeProviders = new Set(
      (runtime?.connectionState ?? legacy.connectionState)?.status === 'connected'
        ? runtime?.activeCredentialProviderIds ?? legacy.activeCredentialProviderIds ?? []
        : [],
    );
    const persistedProviders = this.credentialBroker
      ? new Set(this.brokerConfiguredProviders)
      : new Set(runtime?.persistedCredentialProviderIds ?? legacy.persistedCredentialProviderIds ?? []);
    const engineStorageEncrypted = this.credentialBroker
      ? this.brokerStorageError === undefined
      : runtime?.providerCredentialStorageEncrypted ?? legacy.providerCredentialStorageEncrypted ?? false;
    const credentialPreviews = this.credentialBroker
      ? Object.fromEntries(this.brokerCredentialPreviews)
      : runtime?.providerCredentialPreviews ?? legacy.providerCredentialPreviews ?? {};
    return this.credentialProviderIds().map((providerId) => {
      if (providerId === CODEX_PROVIDER_ID && !this.credentialBroker) {
        return { providerId, configured: false, encryptionAvailable: false,
          storageError: 'Codex 登录需要支持安全凭据代理的桌面构建。' };
      }
      const codexEmail = providerId === CODEX_PROVIDER_ID && this.codexAccountEmail
        ? { codexAccountEmail: this.codexAccountEmail }
        : {};
      if (persistedProviders.has(providerId)) {
        return {
          providerId,
          configured: true,
          encryptionAvailable: engineStorageEncrypted,
          ...(credentialPreviews[providerId]
            ? { credentialPreview: credentialPreviews[providerId] }
            : {}),
          ...codexEmail,
        };
      }
      if (activeProviders.has(providerId)) {
        return {
          providerId,
          configured: true,
          encryptionAvailable: false,
          ...(credentialPreviews[providerId]
            ? { credentialPreview: credentialPreviews[providerId] }
            : {}),
          runtimeOnly: true,
          ...codexEmail,
        };
      }
      return {
        providerId,
        configured: false,
        encryptionAvailable: engineStorageEncrypted,
        ...(this.brokerStorageError ? { storageError: this.brokerStorageError } : {}),
      };
    });
  }

  private providerCredentialMetadata(providerId: string): ProviderCredentialMetadata {
    return this.providerCredentialSnapshot().find((metadata) => metadata.providerId === providerId)
      ?? { providerId, configured: false, encryptionAvailable: false };
  }

  private async refreshCredentialBrokerStatus(previewProviderId?: string): Promise<void> {
    if (!this.credentialBroker) return;
    if (previewProviderId === undefined) {
      if (Date.now() < this.credentialStatusCacheExpiresAt) return;
      if (this.credentialStatusPromise) return this.credentialStatusPromise;
      const operation = this.refreshCredentialBrokerStatusUncached();
      this.credentialStatusPromise = operation
        .then(() => { this.credentialStatusCacheExpiresAt = Date.now() + CREDENTIAL_STATUS_CACHE_TTL_MS; })
        .finally(() => { this.credentialStatusPromise = undefined; });
      return this.credentialStatusPromise;
    }
    await this.refreshCredentialBrokerStatusUncached(previewProviderId);
  }

  private async refreshCredentialBrokerStatusUncached(previewProviderId?: string): Promise<void> {
    if (!this.credentialBroker) return;
    const nextConfigured = new Set(
      (await this.credentialBroker.listStatus(this.credentialProviderIds()))
        .filter((entry: CredentialBrokerStatus) => entry.configured)
        .map((entry: CredentialBrokerStatus) => entry.providerId),
    );
    this.brokerStorageError = undefined;
    this.brokerConfiguredProviders.clear();
    for (const providerId of nextConfigured) this.brokerConfiguredProviders.add(providerId);
    for (const providerId of this.credentialProviderIds()) {
      if (!nextConfigured.has(providerId)) this.brokerCredentialPreviews.delete(providerId);
    }
    // The account address is read back here rather than carried in memory from
    // login, so it survives an app restart. It is display-only: a session that
    // fails to parse must not fail the broker status refresh, which is what
    // gates every other provider row.
    try {
      this.codexAccountEmail = nextConfigured.has(CODEX_PROVIDER_ID)
        ? (await resolveCodexOAuthSession(this.credentialBroker))?.email
        : undefined;
    } catch {
      this.codexAccountEmail = undefined;
    }
    if (!previewProviderId || previewProviderId === CODEX_PROVIDER_ID) return;
    const preview = await this.credentialBroker.preview(previewProviderId);
    if (!preview.configured) {
      this.brokerConfiguredProviders.delete(previewProviderId);
      this.brokerCredentialPreviews.delete(previewProviderId);
      return;
    }
    this.brokerConfiguredProviders.add(previewProviderId);
    if (preview.maskedValue) this.brokerCredentialPreviews.set(previewProviderId, preview.maskedValue);
    else this.brokerCredentialPreviews.delete(previewProviderId);
  }

  private invalidateCredentialBrokerStatus(): void {
    this.credentialStatusCacheExpiresAt = 0;
  }

  private async setCredentialThroughRuntime(providerId: string, credential: string): Promise<ProviderCredentialMetadata> {
    this.requireWorkspace();
    const stored = await this.requireCurrentRuntime().setProviderCredential(providerId, credential);
    if (!stored.configured_provider_ids.includes(providerId)) {
      throw new Error(`provider credential was not persisted (${providerId})`);
    }
    this.bridge.invalidateLaunchConfigCache();
    return {
      providerId,
      configured: true,
      encryptionAvailable: stored.storage_encrypted,
      ...(stored.credential_previews?.[providerId]
        ? { credentialPreview: stored.credential_previews[providerId] }
        : {}),
    };
  }

  private async setCredentialThroughBroker(providerId: string, credential: string): Promise<ProviderCredentialMetadata> {
    const stored = await this.credentialBroker!.set(providerId, credential);
    if (!stored.configured) throw new Error(`provider credential was not persisted (${providerId})`);
    await this.bridge.refreshCachedProviderCredential(providerId, credential);
    this.bridge.invalidateLaunchConfigCache();
    this.brokerStorageError = undefined;
    this.brokerConfiguredProviders.add(providerId);
    this.invalidateCredentialBrokerStatus();
    if (stored.maskedValue) this.brokerCredentialPreviews.set(providerId, stored.maskedValue);
    else this.brokerCredentialPreviews.delete(providerId);
    return {
      providerId,
      configured: true,
      encryptionAvailable: true,
      ...(stored.maskedValue ? { credentialPreview: stored.maskedValue } : {}),
    };
  }

  private async clearCredentialThroughBroker(providerId: string): Promise<void> {
    await this.bridge.clearCachedProviderCredential(providerId);
    await this.credentialBroker!.delete(providerId);
    this.brokerConfiguredProviders.delete(providerId);
    this.brokerCredentialPreviews.delete(providerId);
    this.brokerStorageError = undefined;
    this.invalidateCredentialBrokerStatus();
  }

  private credentialProviderIds(): string[] {
    return [...new Set([...PROVIDER_IDS, ...(this.currentRuntime()?.customProviderIds ?? [])])];
  }

  private async requireCredentialProvider(providerId: unknown): Promise<{ id: string }> {
    if (typeof providerId !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(providerId)) {
      throw new Error('invalid provider id');
    }
    if (providerById(providerId)) return this.requireProvider(providerId);
    const runtime = this.currentRuntime();
    if (!runtime?.customProviderIds?.includes(providerId)) {
      if (!runtime?.ensureCustomProviderConfigured) throw new Error('unsupported provider');
      await runtime.ensureCustomProviderConfigured(providerId);
      if (runtime !== this.currentRuntime() || !runtime.customProviderIds.includes(providerId)) throw new Error('unsupported provider');
    }
    return { id: providerId };
  }

  private requireProvider(providerId: unknown) {
    if (typeof providerId !== 'string') throw new Error('invalid provider id');
    const provider = providerById(providerId);
    if (!provider) throw new Error('unsupported provider');
    if (!provider.available) throw new Error(`${provider.label} sign-in is not available in this desktop build; use the CLI/TUI connect flow`);
    return provider;
  }

  private enqueueNavigation<T>(operation: () => Promise<T>): Promise<T> {
    const run = this.navigationQueue.then(operation, operation);
    this.navigationQueue = run.then(() => undefined, () => undefined);
    return run;
  }

  private async selectWorkspace(input: string, addProject: boolean): Promise<WorkspaceMetadata> {
    return this.enqueueNavigation(() => this.selectWorkspaceInternal(input, addProject));
  }

  private async selectWorkspaceInternal(input: string, addProject: boolean): Promise<WorkspaceMetadata> {
    const workspace = canonicalWorkspace(input);
    if (this.closingProjects.has(workspace)) throw new Error('project is closing');
    this.workspaceFiles.invalidate();
    if (addProject) {
      const wasEmpty = this.settings.getPublic().projects.length === 0;
      this.settings.addProject(workspace);
      // Adding a project is metadata-only after the first project. The first
      // project gets one host-owned runtime so the app has an initial session.
      const manager = this.bridge as unknown as {
        newSession?: (projectPath: string) => Promise<SessionRef>;
        restart?: () => Promise<void>;
      };
      if (wasEmpty && manager.newSession) {
        const ref = await this.bridge.newSession(workspace);
        this.settings.activateProject(workspace);
        this.settings.setActiveSessionDraft(ref);
      } else if (wasEmpty) {
        // Compatibility for the pre-manager unit-test double. Production
        // always takes the session-owned branch above.
        await manager.restart?.();
      }
    } else {
      this.settings.activateProject(workspace);
    }
    let fingerprint: string | undefined;
    try { fingerprint = this.settings.getTrust(workspace).fingerprint; }
    catch (error) { this.diagnostics.add('warn', 'host', error); }
    return { path: workspace, trusted: this.settings.isTrustedWorkspace(workspace), ...(fingerprint ? { fingerprint } : {}) };
  }

  private assertNoActiveTurn(): void {
    if (this.hasActiveWork()) throw new Error('cancel the active turn before changing engine settings');
  }

  private assertRestartAllowed(ref: SessionRef, runtime: ReturnType<SessionRuntimeManager['get']>, recovering = false): void {
    if (!runtime) throw new Error(`session runtime is not open: ${ref.sessionId}`);
    const active = this.settings.getPublic().activeSession;
    if (!active || active.sessionId !== ref.sessionId || active.projectPath !== ref.projectPath) {
      throw new Error('the requested session is no longer active; credential was saved but the engine was not restarted');
    }
    if (!recovering && (runtime.turnActive || runtime.pendingInteractions > 0 || this.bridge.hasActiveWork(ref.projectPath))) {
      throw new Error('cancel active turns and pending interactions before restarting the engine');
    }
  }

  private async restartIfConfigured(): Promise<void> {
    const ref = this.settings.getPublic().activeSession;
    if (!ref) return;
    const runtime = this.bridge.get(ref.sessionId);
    if (!runtime) return;
    const recovering = runtime.connectionState.status !== 'connected' || runtime.isStarting;
    const assertCurrent = () => {
      const active = this.settings.getPublic().activeSession;
      if (!active || active.sessionId !== ref.sessionId || active.projectPath !== ref.projectPath
        || this.bridge.get(ref.sessionId) !== runtime) {
        throw new Error('the requested session is no longer active; credential was saved but the engine was not restarted');
      }
    };
    assertCurrent();
    await this.bridge.restart(ref, assertCurrent);
    assertCurrent();
    if (recovering && runtime.recoveredLiveConnection) {
      // Recovery preserves the living process. Launch-only material still
      // needs a normal restart, whose active-work guard can safely refuse it.
      await this.bridge.restart(ref, assertCurrent);
      assertCurrent();
    }
  }

  private requireCurrentRuntime() {
    const ref = this.settings.getPublic().activeSession;
    if (!ref) throw new Error('open a session first');
    return this.bridge.require(ref);
  }

  private async loadProjectSessions(projectPath: string): Promise<ProjectSessionCatalogState> {
    const generation = (this.catalogRequestGenerations.get(projectPath) ?? 0) + 1;
    this.catalogRequestGenerations.set(projectPath, generation);
    try {
      const result = await this.sessionCatalog.list(projectPath);
      const state = {
        sessions: result.sessions.filter((session) => !this.scheduled?.isControllerSession(projectPath, session.uuid) && !this.settings.isSessionArchived({ projectPath, sessionId: session.uuid })).map(({
          empty_session: _emptySession,
          resume_model: _resumeModel,
          ...session
        }) => session),
      };
      if (this.catalogRequestGenerations.get(projectPath) === generation) this.catalogs.set(projectPath, state);
      return state;
    } catch (error) {
      const state = { sessions: [], error: sanitizeDiagnostic(error) };
      if (this.catalogRequestGenerations.get(projectPath) === generation) this.catalogs.set(projectPath, state);
      return state;
    }
  }

  private async assertSessionBelongsToProject(ref: SessionRef): Promise<ProjectSessionCatalogRow | undefined> {
    if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
    if (this.isProjectClosing(ref.projectPath)) {
      throw new Error('project is closing');
    }
    const open = this.bridge.get(ref.sessionId);
    if (open) {
      if (open.projectPath !== ref.projectPath) throw new Error('session id is owned by a different project');
      // A connected runtime is already authoritative for this compound
      // identity. A newly-created session may not be persisted yet.
      if (open.connectionState.status === 'connected') return undefined;
      return this.sessionCatalog.find(ref.projectPath, ref.sessionId);
    }
    const session = await this.sessionCatalog.find(ref.projectPath, ref.sessionId);
    if (!session) {
      throw new Error('session does not belong to this project');
    }
    return session;
  }

  async openSessionAndActivate(ref: SessionRef): Promise<BootstrapState> {
    return this.enqueueNavigation(() => this.openSessionAndActivateInternal(ref));
  }

  private async openSessionAndActivateInternal(ref: SessionRef): Promise<BootstrapState> {
    if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
    const project = this.requireProject(ref.projectPath);
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    const canonical = { projectPath: project, sessionId: ref.sessionId } satisfies SessionRef;
    const session = await this.assertSessionBelongsToProject(canonical);
    await this.bridge.openSession(
      canonical,
      session?.empty_session === true,
      session?.resume_model,
    );
    // Persist navigation only after the engine has emitted a matching
    // session_resumed event. A failed/corrupt resume leaves the visible session
    // and selected Project unchanged.
    this.settings.activateProject(project);
    const wasArchived = this.settings.isSessionArchived(canonical);
    if (wasArchived) this.settings.setSessionArchived(canonical, false);
    this.settings.setActiveSession(canonical);
    if (wasArchived) await this.loadProjectSessions(project);
    return this.bootstrap();
  }

  private archiveRef(projectPath: unknown, sessionId: unknown): SessionRef {
    const project = this.requireProject(projectPath);
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    return { projectPath: project, sessionId };
  }

  private async archiveSessionInternal(ref: SessionRef): Promise<BootstrapState> {
    const row = await this.assertSessionBelongsToProject(ref);
    return this.bridge.withBackgroundSession(ref, row?.empty_session === true, row?.resume_model, async (runtime) => {
      const release = runtime.beginArchive();
      let removed = 0;
      try {
        const jobs = (await runtime.manageCron({ action: 'list' })).filter((job) => job.automation?.targetSessionId === ref.sessionId || job.automation?.ownedSessionId === ref.sessionId);
        for (const job of jobs) {
          if (job.automation && job.automation.status !== 'completed') await runtime.manageCron({ action: 'pause', id: job.id, automation: { ...job.automation, status: 'paused', statusReason: 'Target chat was archived. Select another chat to resume.' } });
          removed++;
        }
        const active = this.settings.getPublic().activeSession?.sessionId === ref.sessionId;
        const title = row?.title ?? this.catalogs.get(ref.projectPath)?.sessions.find((item) => item.uuid === ref.sessionId)?.title;
        if (runtime.hasActiveWork) throw new Error('Wait for active work and pending interactions before archiving this chat.');
        this.settings.setSessionArchived(ref, true, title);
        await this.terminals?.closeScope(ref);
        await this.bridge.closeSession(ref);
        this.sessionCatalog.invalidate(ref.projectPath);
        await this.loadProjectSessions(ref.projectPath);
        if (active) {
          const replacement = await this.bridge.newSession(ref.projectPath);
          this.settings.setActiveSessionDraft(replacement);
        }
        return this.bootstrap();
      } catch (error) {
        if (!this.settings.isSessionArchived(ref)) throw new Error(`Chat was not archived. ${removed ? `${removed} scheduled task(s) were already paused. ` : ''}${error instanceof Error ? error.message : String(error)}`);
        throw new Error(`Chat was archived, but opening the next chat failed: ${error instanceof Error ? error.message : String(error)}`);
      } finally { release(); }
    });
  }

  private async removeProjectInternal(project: string): Promise<BootstrapState> {
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    if (this.bridge.hasActiveWork(project)) throw new Error('cancel active turns and pending interactions before removing a project');
    // Set the host-side guard before invoking the manager. The manager then
    // synchronously removes matching runtimes from its routable map before its
    // first await, so a racing prompt cannot enter a disposing runtime.
    if (this.scheduled) {
      const jobs = await this.scheduled.manage(project, { action: 'list' });
      for (const job of jobs) if (job.automation?.status === 'active') {
        await this.scheduled.manage(project, { action: 'pause', id: job.id, automation: { ...job.automation, status: 'paused', statusReason: 'Project was removed. Add the project again to resume.' } });
      }
    }
    this.closingProjects.add(project);
    try {
      if (this.bridge.hasActiveWork(project)) throw new Error('cancel active work and pending interactions before removing a project');
      await this.terminals?.closeProject(project);
      await this.bridge.closeProject(project);
      this.settings.removeProject(project);
      for (const watches of this.gitWatches.values()) {
        const watch = watches.get(project);
        watches.delete(project);
        if (watch) void watch.then(stop => stop()).catch(() => undefined);
      }
      this.workspaceFiles.invalidate();
      this.catalogs.delete(project);
      this.sessionCatalog.invalidate(project);
      this.catalogRequestGenerations.set(project, (this.catalogRequestGenerations.get(project) ?? 0) + 1);
      return this.bootstrap();
    } finally {
      this.closingProjects.delete(project);
    }
  }

  private isProjectClosing(projectPath: string): boolean {
    const manager = this.bridge as unknown as { isProjectClosing?: (path: string) => boolean };
    return this.closingProjects.has(projectPath) || manager.isProjectClosing?.(projectPath) === true;
  }

  async restoreProjectSession(projectPath: string, activeSession?: SessionRef): Promise<SessionRef> {
    const project = this.requireProject(projectPath);
    if (activeSession?.projectPath === project) {
      await this.openSessionAndActivate(activeSession);
      return activeSession;
    }
    const catalog = await this.sessionCatalog.list(project);
    catalog.sessions = catalog.sessions.filter((session) => !this.scheduled?.isControllerSession(project, session.uuid) && !this.settings.isSessionArchived({ projectPath: project, sessionId: session.uuid }));
    this.catalogs.set(project, {
      sessions: catalog.sessions.map(({
        empty_session: _emptySession,
        resume_model: _resumeModel,
        ...session
      }) => session),
    });
    const first = catalog.sessions[0];
    const ref = first
      ? { projectPath: project, sessionId: first.uuid }
      : await this.bridge.newSession(project);
    if (first) await this.bridge.openSession(ref, first.empty_session === true, first.resume_model);
    this.settings.activateProject(project);
    if (first) this.settings.setActiveSession(ref);
    else this.settings.setActiveSessionDraft(ref);
    this.terminals?.migrateScope({ projectPath: project, sessionId: TERMINAL_DRAFT_SESSION }, ref);
    return ref;
  }

  private async persistSessionTitle(ref: SessionRef, customTitle: string): Promise<ProjectSessionCatalogState> {
    const row = await this.assertSessionBelongsToProject(ref) ?? await this.sessionCatalog.find(ref.projectPath, ref.sessionId);
    if (!row) throw new Error('session must be persisted before it can be renamed');
    // Renaming is persisted metadata, not a model operation. Starting or
    // resuming an engine here can wait on credentials or an active turn.
    // Use the same append-only side record as JsonlWriter::append_custom_title;
    // its metadata backstop adopts the newest on-disk title on later writes.
    const file = await open(row.path, constants.O_WRONLY | constants.O_APPEND | constants.O_NOFOLLOW);
    try {
      if (!(await file.stat()).isFile()) throw new Error('session path is not a file');
      await file.writeFile(`${JSON.stringify({ type: 'custom-title', customTitle, sessionId: ref.sessionId })}\n`);
      await file.sync();
    } finally {
      await file.close();
    }
    this.sessionCatalog.invalidate(ref.projectPath);
    const previousCatalog = this.catalogs.get(ref.projectPath);
    const catalog = await this.loadProjectSessions(ref.projectPath);
    if (catalog.error && previousCatalog) {
      // The append is already durable. Keep the last usable snapshot so a
      // transient catalog failure cannot turn a successful rename into a
      // retryable error (and a duplicate custom-title record).
      const fallback = {
        sessions: previousCatalog.sessions.map((session) => session.uuid === ref.sessionId
          ? { ...session, title: customTitle }
          : session),
        error: catalog.error,
      };
      // Do not overwrite a newer successful catalog request that raced the
      // refresh, but keep the fallback available for a later rename while the
      // catalog process remains unavailable.
      if (this.catalogs.get(ref.projectPath)?.error === catalog.error) {
        this.catalogs.set(ref.projectPath, fallback);
      }
      return fallback;
    }
    return catalog;
  }

  private async clearActiveSession(sessionId: string, name?: string): Promise<void> {
    const active = this.settings.getPublic().activeSession;
    if (!active || active.sessionId !== sessionId) {
      throw new Error('the requested session is no longer active');
    }
    const runtime = this.bridge.get(active.sessionId);
    if (this.hasActiveWork(active.projectPath)) {
      throw new Error('cancel the active turn before clearing the session');
    }
    if ((runtime?.pendingInteractions ?? 0) > 0 || (runtime?.pendingAskUserQuestions.length ?? 0) > 0) {
      throw new Error('resolve pending interactions before clearing the session');
    }
    // Claude Code's `/clear [name]` labels the conversation being left, not
    // the empty conversation it starts. Draft sessions have no persisted
    // transcript yet, so there is nothing to label in that case.
    if (name) {
      const previous = await this.sessionCatalog.find(active.projectPath, active.sessionId);
      if (previous) await this.persistSessionTitle(active, name);
    }
    if (this.hasActiveWork(active.projectPath)) throw new Error('cancel active work before clearing the session');
    await this.terminals?.closeScope(active);
    await this.bridge.closeSession(active);
    const replacement = await this.bridge.newSession(active.projectPath);
    this.settings.activateProject(active.projectPath);
    this.settings.setActiveSessionDraft(replacement);
    this.workspaceFiles.invalidate();
  }

  dispose(): void {
    this.scheduled?.dispose();
    this.ipc.removeHandler(CH_SCHEDULED);
    this.codexLoginAbort?.abort();
    this.offTerminals?.();
    this.offGit?.();
    for (const target of this.gitWatches.keys()) this.detachGit(target);
    for (const delivery of this.terminalDeliveries.values()) delivery.dispose();
    this.terminalDeliveries.clear();
    if (!this.registered) return;
    for (const channel of [
      CH_GIT_REQUEST, CH_TERMINAL_REQUEST, CH_BOOTSTRAP, CH_SETTINGS_GET, CH_SETTINGS_UPDATE, CH_SETTINGS_FILE_OPEN, CH_WORKSPACE_PICK, CH_WORKSPACE_SET,
      CH_PROJECT_REMOVE, CH_SESSION_PIN_SET,
      CH_PROJECT_SESSIONS_LIST, CH_SESSION_NEW, CH_SESSION_OPEN, CH_SESSION_CLEAR, CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT,
      CH_WORKSPACE_FILES_SEARCH,
      CH_PROVIDER_CREDENTIALS_GET, CH_PROVIDER_CREDENTIAL_SET, CH_PROVIDER_CREDENTIAL_CLEAR,
      CH_PROVIDER_CONNECTION_TEST, CH_CODEX_LOGIN, CH_CODEX_CANCEL,
      CH_PLUGIN_SECRET_GET, CH_PLUGIN_SECRET_SET, CH_PLUGIN_SECRET_CLEAR,
      CH_BRIDGE_RESTART, CH_DIAGNOSTICS_GET, CH_MICROPHONE_ACCESS_GET,
      CH_DIAGNOSTICS_COPY, CH_DIAGNOSTICS_EXPORT, CH_CLIPBOARD_WRITE_TEXT, CH_OPEN_SYSTEM_SETTINGS,
      CH_NATIVE_AUDIO_REQUEST, CH_NATIVE_AUDIO_OPERATION, CH_NATIVE_AUDIO_CANCEL, CH_NATIVE_AUDIO_FINISH_LISTEN,
    ]) this.ipc.removeHandler(channel);
    this.offNativeAudio?.();
    this.offNativeAudio = undefined;
    this.registered = false;
    this.targets.clear();
  }
}
