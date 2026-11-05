import type { ProviderAccountSnapshot } from '../shared/providerAccounts.js';
import { CH_PROVIDER_ACCOUNTS, CH_PROVIDER_ACCOUNTS_CANCEL, type ProviderAccountCommand } from '../shared/providerAccounts.js';
import type { DesktopProviderAccounts } from './provider-accounts.js';
import { CH_ACCOUNT, CH_ACCOUNT_CHANGED } from '../shared/account.js';
import { CH_IMAGE_SAVE } from '../shared/imageSave.js';
import { registerImageSave } from './imageSave.js';
import { CH_APPLICATION_ICON } from '../shared/applicationIcon.js';
import { CH_INTERFACE_LOCALE } from '../shared/interfaceLocale.js';
import { nativeTr, setNativeLocale } from '../shared/nativeLocale.js';
import { CH_COMPUTER_SETTINGS } from '../shared/computerSettings.js';
import type { ComputerSettingsStore } from './computerSettings.js';
import { applicationIcon } from './applicationIcon.js';
import type { AccountService } from './account.js';
import { RealtimeAudioController } from './audio/realtimeAudioController.js';
import { CH_REALTIME_AUDIO_COMMAND, CH_REALTIME_AUDIO_STATE } from '../shared/realtimeAudio.js';
import { CH_SCHEDULED } from '../shared/scheduled.js';
import type { ScheduledTaskService } from './scheduled.js';
import type { HostNotifier } from './notifications.js';
import type { GitService } from './git.js';
import { CH_GIT_REQUEST, CH_GIT_EVENT, type GitRequest } from '../shared/git.js';
import type { RemoteControlUi } from './remoteControl/rendererIpc.js';
import { CH_BOTS_DELETE, CH_BOTS_SESSION_OPEN, CH_BOTS_SESSION_SHOWN, type BotSessionRefUi } from '../shared/bots.js';
import type { KeepAwake } from './bots/keepAwake.js';
import type { BotsUi } from './bots/rendererIpc.js';
import type { TerminalManager } from './terminal.js';
import { TerminalDelivery } from './terminal-delivery.js';
import {
  CH_TERMINAL_REQUEST,
  CH_TERMINAL_EVENT,
  TERMINAL_DRAFT_SESSION,
  type TerminalScope,
} from '../shared/terminal.js';
import { createRequire } from 'node:module';
import { createHash, randomUUID } from 'node:crypto';
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
  rmdirSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';

import type { AskUserQuestionRequestDto, CronJobDto, SessionRowDto } from '@lingxi/bridge-client';
import type {
  BridgeRuntimeVersions,
  ConnectionState,
  RuntimeEventEnvelope,
  SessionRef,
  SessionRuntimeSummary,
} from './bridgeTypes.js';
import type { SessionRuntimeManager } from './sessionRuntimeManager.js';
import { validateNewAuthoringIntent, type AuthoringSessionIntent, type AuthoringSessionSource } from './local-apps/authoringSessionIntents.js';
import type { LocalAppNewSessionIntent } from '../shared/localApps.js';
import type { SessionRuntime } from './bridge.js';
import { isSessionId } from './sessionIdentity.js';
import { CH_PROJECT_APPS_DEFAULT, CH_PROJECT_CREATE, CH_PROJECT_FOLDER_PICK, type ProjectFolderSelection } from '../shared/projectCreation.js';
import { CH_SESSION_WORKSPACE, sessionCatalogScope, type SessionCatalogRow, type SessionCatalogScope, type SessionOpenSelection, type SessionMetadataSelection, type SessionWorkspaceOperation } from '../shared/sessionWorkspace.js';
import { CH_CONVERSATION_SHOWN, CH_CONVERSATION_SPACE, CH_SESSION_HANDOFF, CH_SESSION_HANDOFF_CANCEL, type ConversationSpace, type ConversationSpacesSnapshot, type OwnedConversation } from '../shared/conversationSpaces.js';
import { WorkspaceFileSearch } from './file-search.js';
import { ProjectSessionCatalog, type ProjectSessionCatalogRow } from './session-catalog.js';
import {
  deletePluginSecretsForPlugin,
  validatePluginSecretPluginId,
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
  CH_NATIVE_AUDIO_FLOW_LEASE,
  CH_NATIVE_AUDIO_OPERATION,
  CH_NATIVE_AUDIO_REQUEST,
} from '../shared/nativeAudio.js';
import { providerById, providersFromCatalog } from '../shared/providers.js';
import type { SettingsStore } from './settings.js';


export interface PluginSecretMetadata {
  pluginId: string;
  key: string;
  configured: boolean;
  maskedValue?: string;
  storageError?: string;
  restartRequired?: boolean;
}

export interface PluginSecretCleanupResult {
  pluginId: string;
  status: 'cleared' | 'retained' | 'deferred' | 'partial';
  deletedCount: number;
  failedCount: number;
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
export const CH_PLUGIN_SECRET_GET = 'lingxi:plugin-secret:get';
export const CH_PLUGIN_SECRET_SET = 'lingxi:plugin-secret:set';
export const CH_PLUGIN_SECRET_CLEAR = 'lingxi:plugin-secret:clear';
export const CH_PLUGIN_SECRET_CLEANUP = 'lingxi:plugin-secret:cleanup-after-uninstall';
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
export const CH_SESSION_METADATA_CAPTURE = 'lingxi:session:metadata-capture';
export const CH_SESSION_METADATA_RELEASE = 'lingxi:session:metadata-release';
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

const MAX_PLUGIN_REGISTRY_BYTES = 8 * 1024 * 1024;

interface InstalledPluginRegistrySnapshot {
  revision: string;
  installed: boolean;
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

/** Read the canonical V2 registry without traversing symlinked plugin paths. */
function readInstalledPluginRegistrySnapshot(configHome: string, pluginId: string): InstalledPluginRegistrySnapshot {
  validatePluginSecretPluginId(pluginId);
  if (!isAbsolute(configHome) || configHome.includes('\0')) throw new Error('plugin registry root is unavailable');

  const root = realpathSync.native(resolve(configHome));
  if (!lstatSync(root).isDirectory()) throw new Error('plugin registry root is unavailable');
  const pluginsDir = join(root, 'plugins');
  const pluginsMetadata = lstatSync(pluginsDir);
  if (!pluginsMetadata.isDirectory() || pluginsMetadata.isSymbolicLink()) {
    throw new Error('plugin registry directory is unsafe');
  }
  const canonicalPluginsDir = realpathSync.native(pluginsDir);
  if (canonicalPluginsDir !== pluginsDir || pathEscapes(root, canonicalPluginsDir)) {
    throw new Error('plugin registry directory is unsafe');
  }

  const registryPath = join(pluginsDir, 'installed_plugins.json');
  const before = lstatSync(registryPath);
  if (!before.isFile() || before.isSymbolicLink()) throw new Error('plugin registry file is unsafe');
  if (before.size > MAX_PLUGIN_REGISTRY_BYTES) throw new Error('plugin registry file is too large');
  const canonicalRegistryPath = realpathSync.native(registryPath);
  if (canonicalRegistryPath !== registryPath || pathEscapes(pluginsDir, canonicalRegistryPath)) {
    throw new Error('plugin registry file is unsafe');
  }

  const noFollow = process.platform === 'win32' ? 0 : constants.O_NOFOLLOW;
  const fd = openSync(registryPath, constants.O_RDONLY | noFollow);
  let bytes: Buffer;
  try {
    const metadata = fstatSync(fd);
    if (!metadata.isFile() || metadata.size > MAX_PLUGIN_REGISTRY_BYTES
      || metadata.dev !== before.dev || metadata.ino !== before.ino) {
      throw new Error('plugin registry file changed while opening');
    }
    bytes = Buffer.alloc(metadata.size);
    let offset = 0;
    while (offset < bytes.length) {
      const read = readSync(fd, bytes, offset, bytes.length - offset, offset);
      if (read === 0) throw new Error('plugin registry file changed while reading');
      offset += read;
    }
    const after = lstatSync(registryPath);
    if (after.isSymbolicLink() || !after.isFile() || after.dev !== metadata.dev || after.ino !== metadata.ino
      || realpathSync.native(registryPath) !== canonicalRegistryPath) {
      throw new Error('plugin registry file changed while reading');
    }
  } finally {
    closeSync(fd);
  }

  let document: unknown;
  try {
    document = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  } catch {
    throw new Error('plugin registry file is invalid');
  }
  if (!isPlainRecord(document) || document['version'] !== 2 || !isPlainRecord(document['plugins'])) {
    throw new Error('plugin registry format is unsupported');
  }
  const plugins = document['plugins'];
  let installed = false;
  for (const [id, records] of Object.entries(plugins)) {
    if (!id || !Array.isArray(records) || records.length === 0) {
      throw new Error('plugin registry records are invalid');
    }
    for (const record of records) {
      if (!isPlainRecord(record) || typeof record['scope'] !== 'string' || !record['scope']
        || typeof record['installPath'] !== 'string' || !record['installPath']) {
        throw new Error('plugin registry records are invalid');
      }
    }
    if (id === pluginId) installed = true;
  }
  return { revision: createHash('sha256').update(bytes).digest('hex'), installed };
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
  conversationSpaces: ConversationSpacesSnapshot;
  runtimes: SessionRuntimeSummary[];
  projectCatalogs: Record<string, ProjectSessionCatalogState>;
  providerAccountSnapshot?: ProviderAccountSnapshot;
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
  sessions: SessionCatalogRow[];
  error?: string;
}

export interface WorkspaceDialogs {
  pickDirectory(defaultPath: string): Promise<string | undefined>;
  trustDirectory(directory: string): Promise<boolean>;
}

const workspaceDialogs: WorkspaceDialogs = {
  async pickDirectory(defaultPath) {
    const result = await dialog.showOpenDialog({ defaultPath, properties: ['openDirectory', 'createDirectory'] });
    return result.canceled ? undefined : result.filePaths[0];
  },
  async trustDirectory(directory) {
    const result = await dialog.showMessageBox({ type: 'question', title: nativeTr('native_trust_title'),
      message: nativeTr('native_trust_message', directory),
      detail: nativeTr('native_trust_detail'),
      buttons: [nativeTr('common_cancel'), nativeTr('native_trust_continue')], defaultId: 0, cancelId: 0, noLink: true });
    return result.response === 1;
  },
};

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

/** Revalidate a saved source for non-UI cold opens, including remote viewers. */
export async function resolveSavedConversationTranscript(
  settings: SettingsStore, catalog: ProjectSessionCatalog, ref: SessionRef,
): Promise<{ path: string; mode: ConversationSpace } | undefined> {
  const row = await resolveSavedConversationTranscriptRow(settings, catalog, ref);
  return row ? { path: row.path, mode: row.mode } : undefined;
}

async function resolveSavedConversationTranscriptRow(
  settings: SettingsStore, catalog: ProjectSessionCatalog, ref: SessionRef,
): Promise<ProjectSessionCatalogRow | undefined> {
  const owner = settings.conversationSpaces.findBySessionId(ref.sessionId);
  // Look up the saved path through the UUID owner first, so a caller cannot
  // bypass it by presenting the same UUID with a different workspace.
  const selected = settings.getSessionTranscriptSelection(owner ?? ref);
  if (!selected) return undefined;
  if (!owner || owner.projectPath !== ref.projectPath || (ref.mode !== undefined && owner.mode !== ref.mode)
    || (ref.projectId !== undefined && owner.projectId !== ref.projectId)) {
    throw new Error('The saved session owner does not match this conversation.');
  }
  const row = await catalog.select(selected.catalogProjectPath, ref.sessionId,
    { scope: selected.scope, transcriptPath: selected.transcriptPath });
  if (!row || row.uuid.toLowerCase() !== ref.sessionId.toLowerCase() || row.mode !== owner.mode
    || row.path !== selected.transcriptPath || !isAbsolute(row.path)) {
    throw new Error('The saved session selection is unavailable. Refresh the session list and choose a transcript.');
  }
  if (row.workspace_path) {
    try {
      if (canonicalWorkspace(row.workspace_path) !== ref.projectPath) throw new Error('The saved session workspace changed.');
    } catch (error) {
      if (!['ENOENT', 'ENOTDIR'].includes((error as NodeJS.ErrnoException).code ?? '')
        || selected.catalogProjectPath !== ref.projectPath) throw error;
    }
  }
  return row;
}

type ResolvedMetadataSource = { ref: SessionRef; row?: ProjectSessionCatalogRow; transcriptPath?: string };
type MetadataAction = 'rename' | 'touch' | 'verify';
type CapturedLocalAppAuthoring = { sourceDirectory: string; commit<T>(operation: () => T): Promise<T> };
type CapturedMetadataSource = {
  requested: SessionRef;
  source: ResolvedMetadataSource;
  owner: OwnedConversation;
  selection: SessionMetadataSelection;
  sender?: WebContents;
  runtime?: SessionRuntime;
  apply(action: MetadataAction, title?: string): Promise<void>;
  release(): void;
  timer: ReturnType<typeof setTimeout>;
};

/** Prove two path spellings still name the same existing regular transcript. */
function sameTranscriptFile(first: unknown, second: unknown): boolean {
  if (typeof first !== 'string' || typeof second !== 'string' || !isAbsolute(first) || !isAbsolute(second)) return false;
  try {
    const a = lstatSync(first), b = lstatSync(second);
    return a.isFile() && b.isFile() && a.dev === b.dev && a.ino === b.ino
      && realpathSync.native(first) === realpathSync.native(second);
  } catch { return false; }
}

function metadataOpenSelection(value: unknown): SessionOpenSelection | undefined {
  if (value === undefined) return undefined;
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid session metadata selection.');
  const input = value as Record<string, unknown>;
  if (Object.keys(input).some(key => !['scope', 'transcriptPath'].includes(key))
    || input.scope === undefined || typeof input.transcriptPath !== 'string' || !isAbsolute(input.transcriptPath)
    || input.transcriptPath.includes('\0') || input.transcriptPath.length > 32_768) throw new Error('Invalid session metadata selection.');
  return { scope: sessionCatalogScope(input.scope), transcriptPath: input.transcriptPath };
}

function metadataSelection(value: unknown): SessionMetadataSelection {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Capture the session source before continuing.');
  const input = value as Record<string, unknown>;
  if (Object.keys(input).some(key => !['scope', 'transcriptPath', 'sourceToken'].includes(key))
    || !isSessionId(input.sourceToken)) throw new Error('Invalid session metadata selection.');
  const selected = metadataOpenSelection({ scope: input.scope, transcriptPath: input.transcriptPath });
  return { scope: selected!.scope, transcriptPath: selected!.transcriptPath!, sourceToken: input.sourceToken };
}

export class HostController {
  private readonly metadataSources = new Map<string, CapturedMetadataSource>();
  private readonly metadataSenderGenerations = new WeakMap<WebContents, number>();
  private metadataLifecycleEpoch = 0;
  private git?: GitService;
  private offGit?: () => void;
  private remoteControlUi?: RemoteControlUi;
  private remoteControlChannels: string[] = [];
  private botsUi?: BotsUi;
  private botsChannels: string[] = [];
  private readonly gitWatches = new Map<WebContents, Map<string, Promise<() => void>>>();
  private terminals?: TerminalManager;
  private offTerminals?: () => void;
  private readonly terminalDeliveries = new Map<WebContents, TerminalDelivery>();
  private registered = false;
  private navigationQueue: Promise<void> = Promise.resolve();
  private navigationGeneration = 0;
  private readonly handoffs = new Map<string, AbortController>();
  private readonly handoffRetries = new Map<string, { sourceId: string; target: ConversationSpace; release: () => void; expires?: number; timer?: ReturnType<typeof setTimeout>; prepared?: { cutoff: string; summary: string } }>();
  private readonly draftSelections = new Map<ConversationSpace, OwnedConversation>();
  private readonly draftAdmissionGenerations = new Map<ConversationSpace, number>();
  private bootstrapRevision = 0;
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly workspaceFiles = new WorkspaceFileSearch();
  private readonly projectFolderSelections = new WeakMap<WebContents, ProjectFolderSelection & { localApp: boolean }>();
  private readonly projectFolderPickRequests = new WeakMap<WebContents, object>();
  private realtimeAudioController?: RealtimeAudioController;
  private realtimeAudioWindow?: WebContents;
  private realtimeAudioStartingSession?: string;
  private realtimeAudioStartToken?: object;
  private readonly audioFlowLeases = new Map<WebContents, { sessionId: string; leaseId: string; runtime: SessionRuntime; release: () => void }>();
  private readonly detachedAudioLeases = new Set<{ release: () => void }>();
  private readonly sessionCatalog: ProjectSessionCatalog;
  private readonly catalogs = new Map<string, ProjectSessionCatalogState>();
  private readonly catalogRequestGenerations = new Map<string, number>();
  private catalogEpoch = 0;
  private brokerStorageError: string | undefined;
  private readonly credentialMutations = new Map<string, Promise<unknown>>();
  private offNativeAudio?: () => void;
  private scheduled?: ScheduledTaskService;
  private notifier?: HostNotifier;
  private keepAwake?: KeepAwake;
  private captureLocalAppAuthoring?: (appId: string, ref: SessionRef, version: number) => Promise<CapturedLocalAppAuthoring>;
  private localAppSession?: {
    begin(input: LocalAppNewSessionIntent): { allocate(ref: SessionRef): Promise<void>; recordSource(source: AuthoringSessionSource): Promise<void>; finish(ref: SessionRef): Promise<void> };
    get(requestId: string): Promise<AuthoringSessionIntent>;
    recordSource(requestId: string, source: AuthoringSessionSource): Promise<AuthoringSessionIntent>;
    finish(saved: AuthoringSessionIntent): Promise<void>;
  };

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
    private readonly directoryDialogs: WorkspaceDialogs = workspaceDialogs,
  ) {
    this.sessionCatalog = sessionCatalog ?? new ProjectSessionCatalog();
  }

  private providerAccounts?: DesktopProviderAccounts;
  attachProviderAccounts(service: DesktopProviderAccounts): void { this.providerAccounts = service; }
  attachLocalAppAuthoring(capture: (appId: string, ref: SessionRef, version: number) => Promise<CapturedLocalAppAuthoring>): void {
    this.captureLocalAppAuthoring = capture;
  }
  attachLocalAppSessions(value: NonNullable<HostController['localAppSession']>): void { this.localAppSession = value; }

  private account?: AccountService;
  private offAccount?: () => void;
  attachAccount(service: AccountService): void {
    this.offAccount?.();
    this.account = service;
    this.offAccount = service.onChanged(state => {
      for (const target of this.targets.keys()) if (!target.isDestroyed()) target.send(CH_ACCOUNT_CHANGED, state);
    });
  }

  attachScheduled(service: ScheduledTaskService): void { this.scheduled = service; }

  private computerSettings?: ComputerSettingsStore;
  attachComputerSettings(service: ComputerSettingsStore): void { this.computerSettings = service; }

  /** The Remote Control settings page's handlers; registered with the rest by `registerIpc`. */
  attachRemoteControl(ui: RemoteControlUi): void {
    this.remoteControlUi = ui;
    if (this.registered) this.registerRemoteControlIpc();
  }

  /** The Bots page's handlers; registered with the rest by `registerIpc`. */
  attachBots(ui: BotsUi): void {
    this.botsUi = ui;
    if (this.registered) this.registerBotsIpc();
  }

  /** Send to every window of the app that is still alive. */
  sendToWindows(channel: string, payload: unknown): void {
    for (const target of this.targets.keys()) if (!target.isDestroyed()) target.send(channel, payload);
  }

  private registerRemoteControlIpc(): void {
    const ui = this.remoteControlUi;
    if (!ui || this.remoteControlChannels.length > 0) return;
    for (const [channel, handler] of Object.entries(ui.handlers())) {
      this.ipc.handle(channel, (event: IpcMainInvokeEvent, ...args: unknown[]) => {
        this.assertSender(event);
        return handler(...args);
      });
      this.remoteControlChannels.push(channel);
    }
  }

  private registerBotsIpc(): void {
    const ui = this.botsUi;
    if (!ui || this.botsChannels.length > 0) return;
    for (const [channel, handler] of Object.entries(ui.handlers())) {
      this.ipc.handle(channel, (event: IpcMainInvokeEvent, ...args: unknown[]) => {
        this.assertSender(event);
        // Opening a Bot's session and deleting a Bot change which session the window shows, so they take their turn with the other navigation.
        if (channel === CH_BOTS_SESSION_OPEN) {
          ++this.navigationGeneration;
          return this.enqueueNavigation(async () => this.showBotSession(await handler(...args) as BotSessionRefUi));
        }
        if (channel === CH_BOTS_DELETE) {
          return this.enqueueNavigation(async () => {
            const workspace = typeof args[0] === 'string' ? this.botsUi?.workspaceOf(args[0]) : undefined;
            await handler(...args);
            if (workspace !== undefined) this.settings.dropVolatileSessionIn(workspace);
          });
        }
        return handler(...args);
      });
      this.botsChannels.push(channel);
    }
  }

  /**
   * The window now shows a session of a Bot. A Bot's folder is not a project,
   * so the session is held in memory beside the settings, never in them.
   */
  private showBotSession(ref: BotSessionRefUi): BotSessionRefUi {
    this.settings.setActiveBotSession({ projectPath: ref.projectPath, sessionId: ref.sessionId });
    this.workspaceFiles.invalidate();
    return ref;
  }

  /** Open a Bot's session (a new one without `sessionId`) and make it the one the window shows. */
  private async openBotSession(botId: string, sessionId?: string): Promise<BotSessionRefUi> {
    if (!this.botsUi) throw new Error('Bots are unavailable.');
    return this.showBotSession(await this.botsUi.openSession(botId, sessionId));
  }

  private isBotWorkspace(projectPath: string): boolean {
    return this.botsUi?.botIdFor(projectPath) !== undefined;
  }

  /**
   * The notifier holds ARMED TIMERS, so it cannot poll the settings store —
   * a preference written while an idle timer is already counting down has to
   * be pushed at it. `CH_SETTINGS_UPDATE` above is the one writer.
   */
  attachNotifier(notifier: HostNotifier): void { this.notifier = notifier; }

  /**
   * Like the notifier, the blocker that keeps the computer awake has to be told
   * when the owner turns the preference on or off, or it would hold on until
   * the Bots' work was over, or never begin.
   */
  attachKeepAwake(keepAwake: KeepAwake): void { this.keepAwake = keepAwake; }

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
      const lease = this.audioFlowLeases.get(webContents);
      this.audioFlowLeases.delete(webContents);
      if (lease) this.detachedAudioLeases.add(lease);
      if (this.realtimeAudioWindow === webContents) this.realtimeAudioStartToken = undefined;
      if (this.realtimeAudioWindow === webContents) void this.realtimeAudioController?.stop().catch((error) => {
        this.diagnostics.add('warn', 'host', `realtime audio teardown failed: ${sanitizeDiagnostic(error)}`);
      });
      const cancelled = this.nativeAudio?.cancelUiAudioOperations(String(webContents.id), true) ?? Promise.resolve();
      void cancelled.then(() => {
        if (lease) { this.detachedAudioLeases.delete(lease); lease.release(); }
      }).catch((error) => {
        this.diagnostics.add('warn', 'host', `desktop UI audio teardown failed: ${sanitizeDiagnostic(error)}`);
      });
    };
    // A replacement renderer will subscribe afresh. Its predecessor cannot
    // keep the PTY paused waiting for acknowledgements that will never arrive.
    webContents.on?.('did-start-navigation', (_event, _url, inPlace, mainFrame) => {
      if (mainFrame && !inPlace) { detachTerminals(); detachAudio(); this.releaseMetadataSourcesFor(webContents); }
    });
    webContents.on?.('render-process-gone', () => { detachTerminals(); detachAudio(); this.releaseMetadataSourcesFor(webContents); });
    webContents.once('destroyed', () => {
      this.targets.delete(webContents);
      detachTerminals();
      detachAudio();
      this.releaseMetadataSourcesFor(webContents);
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
    this.ipc.handle(CH_COMPUTER_SETTINGS, async (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      if (!this.computerSettings) throw new Error('Computer settings are unavailable');
      const result = await this.computerSettings.execute(command, () => this.assertSender(event));
      this.assertSender(event);
      return result;
    });
    this.ipc.handle(CH_APPLICATION_ICON, async (event: IpcMainInvokeEvent, bundleId: unknown) => {
      this.assertSender(event);
      const icon = await applicationIcon(bundleId);
      this.assertSender(event);
      return icon;
    });
    registerImageSave(this.ipc, event => this.assertSender(event), options => dialog!.showSaveDialog(options));
    this.registerRemoteControlIpc();
    this.registerBotsIpc();
    this.ipc.handle(CH_ACCOUNT, (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      if (!this.account) throw new Error('Account service is unavailable');
      return this.account.execute(command);
    });
    if (this.nativeAudio && !this.offNativeAudio) {
      this.offNativeAudio = this.nativeAudio.onEvent((audioEvent) => {
        if (audioEvent.type === 'capture_chunk') return;
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
      if (keys.some((key) => key !== 'telemetryEnabled' && key !== 'theme' && key !== 'collapseThoughtsByDefault' && key !== 'botsKeepAwake' && key !== 'model' && key !== 'apiBaseUrl' && key !== 'voice' && key !== 'voiceRevision' && key !== 'notifications' && key !== 'modelPickerVisibility' && key !== 'sidebar')) throw new Error('unsupported setting');
      const restartsBridge = 'apiBaseUrl' in patch || ('telemetryEnabled' in patch && patch.telemetryEnabled === true);
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
        telemetryEnabled?: boolean;
        theme?: 'dark' | 'light' | 'system';
        collapseThoughtsByDefault?: boolean;
        botsKeepAwake?: boolean;
        model?: string | null;
        apiBaseUrl?: string | null;
        voice?: unknown;
        voiceRevision?: number;
        notifications?: unknown;
        modelPickerVisibility?: unknown;
        sidebar?: unknown;
      });
      if ('voice' in patch && result.voice) {
        const configurationJson = JSON.stringify(result.voice);
        const updates = this.bridge.runtimeSummaries
          .filter((summary) => summary.connection.status === 'connected')
          .map((summary) => this.bridge.get(summary.sessionId)?.updateMediaAudioSettings(configurationJson))
          .filter((update): update is Promise<void> => update !== undefined);
        const settled = await Promise.allSettled(updates);
        let failedUpdates = 0;
        for (const outcome of settled) {
          if (outcome.status === 'rejected') {
            failedUpdates += 1;
            this.diagnostics.add('warn', 'bridge', `saved media audio settings were not applied to a live session: ${sanitizeDiagnostic(outcome.reason)}`);
          }
        }
        if (failedUpdates > 0) {
          throw new Error(`Audio settings were saved, but ${failedUpdates} live media session(s) did not apply them. Reconnect those sessions to load the saved configuration.`);
        }
      }
      if ('telemetryEnabled' in patch && patch.telemetryEnabled === false) await this.bridge.disableTelemetry();
      if ('notifications' in patch) this.notifier?.setPreferences(result.notifications);
      if ('botsKeepAwake' in patch) this.keepAwake?.setEnabled(result.botsKeepAwake === true);
      if (restartsBridge) await this.restartIfConfigured();
      return result;
    });
    this.ipc.handle(CH_WORKSPACE_PICK, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      ++this.navigationGeneration;
      return this.enqueueNavigation(async () => {
        const result = await dialog.showOpenDialog({ properties: ['openDirectory', 'createDirectory'], securityScopedBookmarks: false });
        if (result.canceled || !result.filePaths[0]) return null;
        return this.selectWorkspaceInternal(result.filePaths[0], true);
      });
    });
    this.ipc.handle(CH_PROJECT_APPS_DEFAULT, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      if (this.currentConversationMode() !== 'code') throw new Error(nativeTr('local_app_development_requires_code'));
      const request = {};
      this.projectFolderPickRequests.set(event.sender, request);
      // Directory discovery does not load project or managed settings.
      const { appProjectsRoot } = await this.sessionCatalog.productPaths();
      this.assertSender(event);
      if (this.projectFolderPickRequests.get(event.sender) !== request) return null;
      const selection = { id: randomUUID(), path: appProjectsRoot, createDirectory: true as const };
      this.projectFolderSelections.set(event.sender, { ...selection, localApp: true });
      return selection;
    });
    this.ipc.handle(CH_PROJECT_FOLDER_PICK, async (event: IpcMainInvokeEvent, purpose: unknown = 'project') => {
      this.assertSender(event);
      if (purpose !== 'project' && purpose !== 'local-app') throw new Error('Invalid project purpose.');
      if (purpose === 'local-app' && this.currentConversationMode() !== 'code') {
        throw new Error(nativeTr('local_app_development_requires_code'));
      }
      ++this.navigationGeneration;
      const request = {};
      this.projectFolderPickRequests.set(event.sender, request);
      const defaultPath = purpose === 'local-app' ? this.projectFolderSelections.get(event.sender)?.path : undefined;
      const path = await this.directoryDialogs.pickDirectory(defaultPath ?? this.settings.getPublic().activeProject ?? '');
      this.assertSender(event);
      if (this.projectFolderPickRequests.get(event.sender) !== request || path === undefined) return null;
      const selection = { id: randomUUID(), path: canonicalWorkspace(path) };
      this.projectFolderSelections.set(event.sender, { ...selection, localApp: purpose === 'local-app' });
      return { ...selection };
    });
    this.ipc.handle(CH_PROJECT_CREATE, async (event: IpcMainInvokeEvent, selectionId: unknown, directoryName?: unknown) => {
      this.assertSender(event);
      const selection = this.projectFolderSelections.get(event.sender);
      if (!selection || !selection.path || selection.id !== selectionId) throw new Error('Choose a source folder before creating the project.');
      if (selection.localApp && this.currentConversationMode() !== 'code') {
        throw new Error(nativeTr('local_app_development_requires_code'));
      }
      ++this.navigationGeneration;
      return this.enqueueNavigation(async () => {
        this.assertSender(event);
        if (this.projectFolderSelections.get(event.sender) !== selection) throw new Error('The source folder selection has changed.');
        let path = selection.path;
        let created: { dev: bigint; ino: bigint } | undefined;
        if (selection.createDirectory) {
          if (typeof directoryName !== 'string' || !directoryName.trim()
            || /[\\/\0]/.test(directoryName) || directoryName.trim() === '.' || directoryName.trim() === '..') {
            throw new Error('Enter a project name without directory separators.');
          }
          mkdirSync(selection.path, { recursive: true, mode: 0o700 });
          path = join(selection.path, directoryName.trim());
          try {
            mkdirSync(path, { mode: 0o700 });
            created = lstatSync(path, { bigint: true });
          }
          catch (error) {
            if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new Error('A folder with this project name already exists. Choose another name or select the existing folder.');
            throw error;
          }
        } else if (directoryName !== undefined) throw new Error('An existing folder uses its directory name.');
        try {
          const workspace = await this.selectWorkspaceInternal(path, true, !selection.localApp, selection.localApp ? 'code' : undefined);
          if (this.projectFolderSelections.get(event.sender) === selection) this.projectFolderSelections.delete(event.sender);
          return workspace;
        } catch (error) {
          // Remove only our empty, unregistered directory. Never remove source
          // files or a project persisted before an engine startup error.
          if (created && !this.settings.hasProject(path)) {
            try {
              const current = lstatSync(path, { bigint: true });
              if (current.isDirectory() && current.dev === created.dev && current.ino === created.ino) rmdirSync(path);
            } catch { /* A concurrent writer owns its files. */ }
          }
          throw error;
        }
      });
    });
    this.ipc.handle(CH_WORKSPACE_SET, async (event: IpcMainInvokeEvent, workspace: unknown) => {
      this.assertSender(event);
      ++this.navigationGeneration;
      return this.enqueueNavigation(async () => {
        if (typeof workspace !== 'string') throw new Error('invalid workspace path');
        const canonical = canonicalWorkspace(workspace);
        if (!this.settings.hasProject(canonical)) throw new Error('project is not in the project list');
        return this.selectWorkspaceInternal(canonical, false);
      });
    });
    this.ipc.handle(CH_PROJECT_SESSIONS_LIST, async (event: IpcMainInvokeEvent, projectPath: unknown, scope: unknown) => {
      this.assertSender(event);
      const project = this.requireProject(projectPath);
      const result = await this.loadProjectSessions(project, sessionCatalogScope(scope));
      return { projectPath: project, ...result, conversationSpaces: this.conversationSpacesSnapshot() };
    });
    this.ipc.handle(CH_CONVERSATION_SPACE, async (event: IpcMainInvokeEvent, value: unknown) => {
      this.assertSender(event);
      if (value !== 'chat' && value !== 'code') throw new Error('invalid conversation space');
      const previous = this.settings.conversationSpaces.snapshot().activeMode;
      const displayed = this.settings.getPublic().activeSession;
      const confirmed = displayed ? this.settings.conversationSpaces.find(displayed)?.mode ?? previous : previous;
      if (previous === value) return this.bootstrap();
      const generation = ++this.navigationGeneration;
      this.settings.conversationSpaces.setMode(value);
      return this.enqueueNavigation(async () => {
        try { return await this.switchConversationSpace(value, generation); }
        catch (error) {
          if (generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === value) {
            this.settings.conversationSpaces.setMode(confirmed);
          }
          throw error;
        }
      });
    });
    this.ipc.handle(CH_SESSION_HANDOFF, async (event: IpcMainInvokeEvent, sourceId: unknown, target: unknown, requestId: unknown, retry: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sourceId) || (target !== 'chat' && target !== 'code')
        || typeof requestId !== 'string' || !/^[a-zA-Z0-9._-]{1,128}$/.test(requestId)
        || (retry !== undefined && typeof retry !== 'boolean')) throw new Error('Invalid handoff request.');
      return this.prepareHandoff(sourceId, target, requestId, retry === true);
    });
    this.ipc.handle(CH_SESSION_HANDOFF_CANCEL, (event: IpcMainInvokeEvent, requestId: unknown) => {
      this.assertSender(event);
      if (typeof requestId !== 'string') throw new Error('Invalid handoff request.');
      this.handoffs.get(requestId)?.abort();
      this.discardHandoff(requestId);
    });
    this.ipc.handle(CH_SESSION_NEW, async (event: IpcMainInvokeEvent, projectPath: unknown, model: unknown, purpose?: unknown) => {
      this.assertSender(event);
      let authoring: LocalAppNewSessionIntent | undefined;
      let recoveryId: string | undefined;
      if (purpose !== undefined) {
        if (!purpose || typeof purpose !== 'object' || Array.isArray(purpose)) throw new Error('invalid new session purpose');
        const input = purpose as Record<string, unknown>;
        if (input.kind === 'local-app' && Object.keys(input).every(key => key === 'kind' || key === 'intent')) {
          authoring = validateNewAuthoringIntent(input.intent);
        } else if (input.kind === 'recover-local-app' && Object.keys(input).every(key => key === 'kind' || key === 'requestId') && isSessionId(input.requestId)) {
          recoveryId = input.requestId;
        } else throw new Error('invalid new session purpose');
      }
      const space = this.settings.conversationSpaces.snapshot().activeMode;
      if (purpose !== undefined && this.currentConversationMode() !== 'code') throw new Error(nativeTr('local_app_development_requires_code'));
      if (purpose !== undefined && !this.localAppSession) throw new Error('The Local App authoring host is unavailable');
      // Reserve the per-App ordering at admission, not after a slow runtime boot.
      const localApp = authoring ? this.localAppSession!.begin(authoring) : undefined;
      const requestedAt = Date.now();
      const generation = ++this.navigationGeneration;
      this.draftAdmissionGenerations.set(space, generation);
      return this.enqueueNavigation(async () => {
        if (recoveryId) return this.recoverLocalAppSession(recoveryId, generation);
        const projectId = projectPath === null ? null : this.requireProject(projectPath);
        if (localApp && (projectId === null || space !== 'code')) throw new Error('Local App authoring requires a Code Project');
        if (projectId !== null && (!this.settings.hasProject(projectId)
          || !this.settings.conversationSpaces.snapshot().projects[projectId]?.includes(space))) {
          throw new Error('project is not in this conversation space');
        }
        const project = projectId ?? this.settings.conversationSpaces.globalWorkspaces[space];
        if (model !== undefined && typeof model !== 'string') throw new Error('invalid model');
        const navigationStartedAt = Date.now();
        this.diagnostics.add('info', 'host', diagnosticEvent('session_new_started', {
          model: typeof model === 'string' ? model : undefined,
          projectPath: project,
          queueWaitMs: navigationStartedAt - requestedAt,
        }));
        let release: (() => void) | undefined;
        try {
          const runtimeStartedAt = Date.now();
          const current = () => generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === space;
          const ref = await this.bridge.newSession(project, model as string | undefined, { mode: space, projectId, activate: localApp ? () => false : current,
            ...(localApp ? { forceNew: true, durableDraftTitle: authoring!.name.trim() || 'Local App',
              onAllocated: async (allocated: SessionRef) => {
                release = this.bridge.retainBackgroundSession(allocated);
                await localApp.allocate(allocated);
              } } : {}) });
          const owned: OwnedConversation = { ...ref, mode: space, projectId };
          this.settings.conversationSpaces.register(owned);
          if (localApp) {
            const runtime = this.bridge.get(ref.sessionId);
            if (!runtime) throw new Error('The Local App Code session is unavailable');
            const path = await this.rememberCreatedSessionSource(owned, runtime, false, source => localApp.recordSource(source));
            const selected = this.settings.getSessionTranscriptSelection(owned);
            if (!selected || selected.catalogProjectPath !== project || selected.transcriptPath !== path) throw new Error('The Local App session source changed');
            await localApp.finish(owned);
            await this.assertLocalAppSessionRuntime(owned, runtime, path);
          }
          if ((!localApp || current()) && this.draftAdmissionGenerations.get(space) === generation) this.draftSelections.set(space, owned);
          const runtimeDurationMs = Date.now() - runtimeStartedAt;
          this.diagnostics.add('info', 'host', diagnosticEvent('session_new_runtime_ready', {
            durationMs: runtimeDurationMs,
            projectPath: project,
            sessionId: ref.sessionId,
          }));
          if (current()) {
            this.bridge.activateSession(owned);
            if (projectId !== null) this.settings.activateProject(projectId);
            this.terminals?.migrateScope({ projectPath: project, sessionId: TERMINAL_DRAFT_SESSION }, ref);
            this.settings.setActiveSessionDraft(owned);
          }
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
        } finally { release?.(); }
      });
    });
    this.ipc.handle(CH_SESSION_OPEN, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, selection?: unknown, linkedMode?: unknown, authoringAppId?: unknown, authoringVersion?: unknown) => {
      this.assertSender(event);
      if (linkedMode !== undefined && linkedMode !== 'chat' && linkedMode !== 'code') throw new Error('invalid linked session mode');
      if ((authoringAppId === undefined) !== (authoringVersion === undefined)
        || (authoringAppId !== undefined && (typeof authoringAppId !== 'string' || !/^a[a-f0-9]{32}$/.test(authoringAppId)
          || linkedMode !== 'code' || selection !== undefined || typeof authoringVersion !== 'number'
          || !Number.isSafeInteger(authoringVersion) || authoringVersion < 0))) {
        throw new Error('invalid Local App authoring session');
      }
      if (authoringAppId !== undefined && this.currentConversationMode() !== 'code') {
        throw new Error(nativeTr('local_app_development_requires_code'));
      }
      const generation = ++this.navigationGeneration;
      const expectedMode = linkedMode ?? this.settings.conversationSpaces.snapshot().activeMode;
      this.draftAdmissionGenerations.delete(expectedMode);
      return this.enqueueNavigation(async () => {
        const project = this.requireProject(projectPath);
        if (!isSessionId(sessionId)) throw new Error('invalid session id');
        const ref = { projectPath: project, sessionId } satisfies SessionRef;
        const authoring = authoringAppId === undefined ? undefined : await this.captureLocalAppAuthoring?.(authoringAppId as string, ref, authoringVersion as number);
        if (authoringAppId !== undefined && !authoring) throw new Error('The Local App authoring host is unavailable');
        if (authoring && authoring.sourceDirectory !== project) throw new Error('The Local App authoring workspace changed');
        let selected: SessionOpenSelection | undefined;
        if (selection !== undefined) {
          if (!selection || typeof selection !== 'object' || Array.isArray(selection)) throw new Error('invalid session selection');
          const input = selection as Record<string, unknown>;
          if (input.scope === undefined || (input.transcriptPath !== undefined && (typeof input.transcriptPath !== 'string'
            || !isAbsolute(input.transcriptPath) || input.transcriptPath.includes('\0') || input.transcriptPath.length > 32_768))) {
            throw new Error('invalid session selection');
          }
          selected = { scope: sessionCatalogScope(input.scope), ...(typeof input.transcriptPath === 'string' ? { transcriptPath: input.transcriptPath } : {}) };
        }
        if (linkedMode === undefined) return this.openSessionAndActivateInternal(ref, selected, generation, expectedMode);
        const known = this.settings.conversationSpaces.findBySessionId(sessionId);
        if (!known) throw new Error('The session has no authenticated conversation scope.');
        if (known.mode !== linkedMode) throw new Error('The session belongs to a different conversation space.');
        if (generation !== this.navigationGeneration) return this.bootstrap();
        const previousMode = this.settings.conversationSpaces.snapshot().activeMode;
        this.settings.conversationSpaces.setMode(linkedMode);
        try { return await this.openSessionAndActivateInternal(ref, selected, generation, linkedMode, authoring); }
        catch (error) {
          if (generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === linkedMode) {
            this.settings.conversationSpaces.setMode(previousMode);
          }
          throw error;
        }
      });
    });
    this.ipc.handle(CH_SESSION_WORKSPACE, async (event: IpcMainInvokeEvent, sessionId: unknown, operation: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId) || !operation || typeof operation !== 'object' || Array.isArray(operation)) throw new Error('invalid session workspace operation');
      const input = operation as Record<string, unknown>;
      if ((input.action !== 'cd' && input.action !== 'add-dir')
        || (input.path !== undefined && (typeof input.path !== 'string' || input.path.length > 32_768 || input.path.includes('\0')))) {
        throw new Error('invalid session workspace operation');
      }
      return this.updateSessionWorkspace(sessionId, input as unknown as SessionWorkspaceOperation);
    });
    this.ipc.handle(CH_SCHEDULED, async (event: IpcMainInvokeEvent, input: unknown) => {
      this.assertSender(event);
      if (!this.scheduled || !input || typeof input !== 'object') throw new Error('Scheduled task service unavailable');
      const request = input as Record<string, unknown>;
      if (request['action'] === 'scopes') {
        const spaces = this.settings.conversationSpaces.snapshot();
        return this.scheduled.scopes().filter((scope) => !scope.projectPath || spaces.projects[scope.projectPath]?.includes(spaces.activeMode));
      }
      if (typeof request['scopeId'] !== 'string') throw new Error('Invalid scheduled scope');
      if (request['action'] === 'context') return this.scheduled.context(request['scopeId'], this.settings.conversationSpaces.snapshot().activeMode);
      if (request['action'] === 'manage') {
        const validated = validateClientCommand({ type: 'cron_manage', request_id: 'host-scheduled', request: request['request'] });
        if (validated.type !== 'cron_manage') throw new Error('Invalid scheduled request');
        return this.scheduled.manage(request['scopeId'], validated.request, this.settings.conversationSpaces.snapshot().activeMode);
      }
      if (request['action'] === 'open') {
        const generation = ++this.navigationGeneration;
        if (!isSessionId(request['sessionId'])) throw new Error('Invalid scheduled session');
        const ref = this.scheduled.resolveSession(request['scopeId'], request['sessionId'] as string);
        const botId = this.botsUi?.botIdFor(ref.projectPath);
        if (botId !== undefined) return this.enqueueNavigation(async () => {
          if (generation !== this.navigationGeneration) return this.bootstrap();
          if (!this.botsUi) throw new Error('Bots are unavailable.');
          const opened = await this.botsUi.openSession(botId, ref.sessionId, undefined, false);
          if (generation === this.navigationGeneration) {
            this.bridge.activateSession(opened);
            this.showBotSession(opened);
          }
          return this.bootstrap();
        });
        const previousMode = this.settings.conversationSpaces.snapshot().activeMode;
        const mode = ref.mode ?? previousMode;
        return this.enqueueNavigation(async () => {
          if (generation !== this.navigationGeneration) return this.bootstrap();
          this.settings.conversationSpaces.setMode(mode);
          try { return await this.openSessionAndActivateInternal(ref, undefined, generation, mode); }
          catch (error) {
            if (generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === mode) {
              this.settings.conversationSpaces.setMode(previousMode);
            }
            throw error;
          }
        });
      }
      throw new Error('Invalid scheduled action');
    });
    this.ipc.handle(CH_SESSION_METADATA_CAPTURE, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, selection?: unknown) => {
      this.assertSender(event);
      const parsed = metadataOpenSelection(selection);
      return this.prepareMetadataSource(this.archiveRef(projectPath, sessionId), parsed, event.sender);
    });
    this.ipc.handle(CH_SESSION_METADATA_RELEASE, (event: IpcMainInvokeEvent, selection: unknown) => {
      this.assertSender(event);
      const parsed = metadataSelection(selection);
      const captured = this.metadataSources.get(parsed.sourceToken);
      if (captured && captured.sender === event.sender && captured.selection.transcriptPath === parsed.transcriptPath
        && captured.selection.scope === parsed.scope) this.releaseMetadataSource(parsed.sourceToken);
    });
    this.ipc.handle(CH_SESSION_ARCHIVE_PREFLIGHT, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, selection?: unknown) => {
      this.assertSender(event);
      const captured = await this.metadataSource(this.archiveRef(projectPath, sessionId), metadataSelection(selection), event.sender);
      await captured.apply('verify');
      const jobs = await this.sessionArchiveJobs(captured);
      await captured.apply('verify');
      return jobs;
    });
    this.ipc.handle(CH_SESSION_ARCHIVE, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, selection?: unknown) => {
      this.assertSender(event);
      const ref = this.archiveRef(projectPath, sessionId);
      const captured = await this.metadataSource(ref, metadataSelection(selection), event.sender);
      return this.enqueueNavigation(async () => {
        try {
          const result = await this.archiveSessionInternal(ref, captured);
          this.releaseMetadataSource(captured.selection.sourceToken);
          return result;
        } catch (error) {
          if (this.settings.isSessionArchived(captured.source.ref)) this.releaseMetadataSource(captured.selection.sourceToken);
          throw error;
        }
      });
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
    this.ipc.handle(CH_SESSION_TOUCH, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, selection?: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const ref = this.archiveRef(projectPath, sessionId);
        const prepared = await this.prepareMetadataSource(ref, metadataOpenSelection(selection), event.sender);
        const captured = await this.metadataSource(ref, prepared, event.sender);
        try {
          await captured.apply('touch');
          this.sessionCatalog.invalidate(captured.source.ref.projectPath);
          if (captured.source.ref.projectPath !== ref.projectPath) await this.loadProjectSessions(captured.source.ref.projectPath);
          return { projectPath: ref.projectPath, ...await this.loadProjectSessions(ref.projectPath) };
        } finally { this.releaseMetadataSource(captured.selection.sourceToken!); }
      });
    });
    this.ipc.handle(CH_SESSION_RENAME, async (event: IpcMainInvokeEvent, projectPath: unknown, sessionId: unknown, title: unknown, selection?: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        const ref = this.archiveRef(projectPath, sessionId);
        if (typeof title !== 'string') throw new Error('invalid session title');
        const customTitle = title.trim();
        if (!customTitle || customTitle.length > 200 || /[\u0000-\u001f\u007f]/.test(customTitle)) throw new Error('session title must be 1–200 printable characters');
        const captured = await this.metadataSource(ref, metadataSelection(selection), event.sender);
        await captured.apply('rename', customTitle);
        this.releaseMetadataSource(captured.selection.sourceToken);
        this.sessionCatalog.invalidate(captured.source.ref.projectPath);
        if (captured.source.ref.projectPath !== ref.projectPath) await this.refreshSessionTitleCatalog(captured.source.ref, customTitle, captured);
        return { projectPath: ref.projectPath, ...await this.refreshSessionTitleCatalog(ref, customTitle, captured) };
      });
    });
    this.ipc.handle(CH_PROJECT_REMOVE, async (event: IpcMainInvokeEvent, projectPath: unknown) => {
      this.assertSender(event);
      return this.enqueueNavigation(async () => {
        // Remove the saved list entry even if its directory has moved or now
        // resolves through a symlink to a different path. Cleanup uses this key.
        if (typeof projectPath !== 'string') throw new Error('invalid project path');
        if (!this.settings.hasProject(projectPath)) throw new Error('project is not in the project list');
        const spaces = this.settings.conversationSpaces.snapshot();
        const memberships = spaces.projects[projectPath] ?? [];
        if (!memberships.includes(spaces.activeMode)) throw new Error('project is not in this conversation space');
        if (memberships.length > 1) {
          this.settings.conversationSpaces.removeProject(projectPath, spaces.activeMode);
          if (this.draftSelections.get(spaces.activeMode)?.projectId === projectPath) this.draftSelections.delete(spaces.activeMode);
          const active = this.settings.getPublic().activeSession;
          if (active?.projectId === projectPath && active.mode === spaces.activeMode) {
            this.bridge.clearActiveSelection();
            this.settings.setActiveSession(undefined);
          }
          return this.bootstrap();
        }
        return this.removeProjectInternal(projectPath);
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
    this.ipc.handle('lingxi:provider-accounts:routing:get', (event: IpcMainInvokeEvent) => {
      this.assertSender(event); return this.providerAccounts?.lastRouting;
    });
    this.ipc.handle('lingxi:provider-settings', async (event: IpcMainInvokeEvent, patch?: unknown, expected?: unknown) => {
      this.assertSender(event);
      if (patch !== undefined) {
        if (!patch || typeof patch !== 'object' || Array.isArray(patch) || !expected || typeof expected !== 'object' || Array.isArray(expected)) throw new Error('Invalid provider settings.');
        this.assertNoActiveTurn();
      }
      const result = await this.sessionCatalog.providerSettings(patch as Record<string, unknown> | undefined, expected as Record<string, unknown> | undefined);
      if (patch !== undefined) {
        try { await this.bridge.applyProviderSettings(); }
        catch { throw new Error('Provider settings were saved but could not be applied. Retry when active work has finished.'); }
      }
      return result;
    });
    this.ipc.handle(CH_PROVIDER_ACCOUNTS, async (event: IpcMainInvokeEvent, request?: unknown, credential?: unknown) => {
      this.assertSender(event);
      if (!this.providerAccounts) throw new Error('Secure provider account storage is unavailable.');
      if (request === undefined) return this.providerAccounts.snapshot();
      if (!request || typeof request !== 'object' || Array.isArray(request)) throw new Error('Invalid account operation.');
      const command = request as ProviderAccountCommand;
      if (!['list', 'add', 'update', 'reorder', 'enable', 'remove', 'test', 'logout', 'reconnect', 'set_policy'].includes(command.action)
        || !Number.isSafeInteger(command.expectedRevision) || command.expectedRevision < 0
        || (command.operationId !== undefined && (!Number.isSafeInteger(command.operationId) || command.operationId < 0))
        || (credential !== undefined && typeof credential !== 'string')) throw new Error('Invalid account operation.');
      if (command.action !== 'list' && command.action !== 'test') this.assertNoActiveTurn();
      if (command.action === 'add') {
        const provider = await this.requireCredentialProvider(command.executionBindingId ?? command.providerId);
        if ((command.authKind === 'oauth') !== (provider.id === 'openai-chatgpt')
          || command.providerId !== (provider.id === 'openai-chatgpt' ? 'openai' : provider.id)) throw new Error('Authentication method is unavailable for this provider.');
      }
      const abort = new AbortController();
      const cancel = () => abort.abort();
      event.sender.once('destroyed', cancel);
      try { return await this.providerAccounts.command(command, credential as string | undefined, abort.signal); }
      finally { event.sender.removeListener('destroyed', cancel); }
    });
    this.ipc.handle(CH_PROVIDER_ACCOUNTS_CANCEL, (event: IpcMainInvokeEvent, operationId?: unknown) => {
      this.assertSender(event);
      if (typeof operationId !== 'number' || !Number.isSafeInteger(operationId) || operationId < 0) throw new Error('Invalid account operation.');
      this.providerAccounts?.cancel(operationId as number | undefined);
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
      validatePluginSecretPluginId(pluginId);
      return this.enqueuePluginSecretMutation(pluginId, async () => {
        const preview = await this.credentialBroker!.setPluginSecret(pluginId, key, secret);
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
    });
    this.ipc.handle(CH_PLUGIN_SECRET_CLEAR, async (
      event: IpcMainInvokeEvent,
      pluginId: unknown,
      key: unknown,
    ) => {
      this.assertSender(event);
      if (typeof pluginId !== 'string' || typeof key !== 'string') throw new Error('invalid plugin secret reference');
      if (!this.credentialBroker) throw new Error('secure plugin credential broker is unavailable');
      validatePluginSecretPluginId(pluginId);
      return this.enqueuePluginSecretMutation(pluginId, async () => {
        await this.credentialBroker!.deletePluginSecret(pluginId, key);
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
    });
    this.ipc.handle(CH_PLUGIN_SECRET_CLEANUP, async (
      event: IpcMainInvokeEvent,
      pluginId: unknown,
      lastScopeRemoved: unknown,
    ): Promise<PluginSecretCleanupResult> => {
      this.assertSender(event);
      if (typeof pluginId !== 'string' || lastScopeRemoved !== true) {
        throw new Error('plugin credential cleanup requires a confirmed last-scope uninstall');
      }
      validatePluginSecretPluginId(pluginId);
      return this.enqueuePluginSecretMutation(pluginId, () => this.deletePluginSecretsAfterUninstall(pluginId));
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
    this.ipc.handle(CH_INTERFACE_LOCALE, (event: IpcMainInvokeEvent, locale: unknown, followSystem: unknown) => {
      this.assertSender(event);
      if (typeof followSystem !== 'boolean') throw new TypeError('Invalid interface language preference');
      setNativeLocale(locale, followSystem);
    });
    this.ipc.handle(CH_DIAGNOSTICS_EXPORT, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      const result = await dialog.showSaveDialog({
        title: nativeTr('native_export_diagnostics'),
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
      flow?: unknown,
    ) => {
      this.assertSender(event);
      if (!this.nativeAudio) throw new Error('native audio is unavailable on this host');
      const owner = this.requireAudioFlowAccess(event.sender, flow);
      const sessionId = owner?.sessionId ?? this.settings.getPublic().activeSession?.sessionId;
      const runtime = owner?.runtime ?? (sessionId ? this.bridge.get(sessionId) : undefined);
      if (owner && !runtime) throw new Error('audio flow session is unavailable');
      const session = runtime?.audioSessionContext;
      const release = owner ? this.retainAudioRuntime(owner.runtime) : () => {};
      return this.nativeAudio.executeUiAudioOperation(
        operation,
        String(event.sender.id),
        configurationRevision as number | undefined,
        configurationOverride,
        { session: session ? { ...session } : undefined },
        owner ? release : undefined,
      );
    });
    this.ipc.handle(CH_REALTIME_AUDIO_COMMAND, async (event: IpcMainInvokeEvent, command: unknown) => {
      this.assertSender(event);
      if (command !== 'start' && command !== 'commit' && command !== 'stop') throw new Error('invalid realtime audio command');
      if (!this.nativeAudio) throw new Error('native audio is unavailable on this host');
      this.realtimeAudioController ??= new RealtimeAudioController({
        audio: this.nativeAudio,
        recordUsage: (metadata, usage) => this.nativeAudio?.recordAudioUsage(metadata, usage),
        currentRuntime: () => { const sessionId = this.settings.getPublic().activeSession?.sessionId; return sessionId ? this.bridge.get(sessionId) : undefined; },
        retainRuntime: (runtime) => {
          const owned = this.bridge.get(runtime.sessionId);
          if (owned !== runtime) throw new Error('The realtime audio session is no longer available.');
          return this.retainAudioRuntime(owned);
        },
        publish: (state) => { if (this.realtimeAudioWindow && !this.realtimeAudioWindow.isDestroyed()) this.realtimeAudioWindow.send(CH_REALTIME_AUDIO_STATE, state); },
      });
      if (command === 'start') {
        const error = this.settings.getAudioConfigurationError();
        if (error) throw new Error(error);
        if (this.realtimeAudioWindow && this.realtimeAudioWindow !== event.sender) throw new Error('another desktop window owns realtime audio');
        const session = this.settings.getPublic().activeSession;
        const runtime = session ? this.bridge.get(session.sessionId) : undefined;
        if (!session || !runtime) throw new Error('open a session before starting realtime audio');
        if (this.realtimeAudioStartingSession) throw new Error('realtime audio is already starting');
        if (this.realtimeAudioController.sessionId && this.realtimeAudioController.sessionId !== session.sessionId) {
          throw new Error('another conversation owns realtime audio');
        }
        const configuration = this.settings.getPublic().voice;
        if (!configuration) throw new Error('configure realtime audio before starting');
        const revision = this.settings.getPublic().voiceRevision ?? 0;
        const release = this.retainAudioRuntime(runtime);
        const startToken = {};
        this.realtimeAudioStartToken = startToken;
        this.realtimeAudioStartingSession = session.sessionId;
        this.realtimeAudioWindow = event.sender;
        try {
        const capability = (await this.nativeAudio.request({ type: 'get_snapshot' })).snapshot.realtimeReadiness;
        if (this.realtimeAudioStartToken !== startToken) return;
        if (!capability?.ready) throw new Error(capability?.reason ?? '实时音频尚未就绪，请检查语音设置。');
        const voice = configuration.conversation.voice;
        if (voice && (voice.source !== 'provider' || voice.profileId !== capability.profileId || (voice.modelId ?? null) !== (configuration.conversation.cloud.modelId ?? capability.defaultModelId ?? null))) throw new Error('实时音色属于其他 profile 或音频模型，请重新选择。');
        const context = runtime.audioSessionContext;
        await this.realtimeAudioController.start(configuration, capability.profileId && capability.providerId ? { kind: 'realtime', configurationRevision: revision, profileId: capability.profileId, providerId: capability.providerId, sessionId: context?.sessionId } : undefined, runtime);
        } finally {
          release();
          this.realtimeAudioStartingSession = undefined;
          if (this.realtimeAudioStartToken === startToken) this.realtimeAudioStartToken = undefined;
          if (!this.realtimeAudioController.sessionId) this.realtimeAudioWindow = undefined;
        }
      } else if (this.realtimeAudioWindow === event.sender) {
        if (command === 'stop' && this.realtimeAudioStartingSession === this.settings.getPublic().activeSession?.sessionId) {
          this.realtimeAudioStartToken = undefined;
          return;
        }
        if (this.realtimeAudioController.sessionId !== this.settings.getPublic().activeSession?.sessionId) {
          throw new Error('realtime audio belongs to another conversation');
        }
        if (command === 'commit') await this.realtimeAudioController.commit();
        else { await this.realtimeAudioController.stop(); this.realtimeAudioWindow = undefined; }
      }
    });
    this.ipc.handle(CH_NATIVE_AUDIO_FLOW_LEASE, (event: IpcMainInvokeEvent, sessionId: unknown, leaseId: unknown, retain: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId) || !isSessionId(leaseId) || typeof retain !== 'boolean') throw new Error('invalid audio flow session');
      const previous = this.audioFlowLeases.get(event.sender);
      if (!retain) {
        if (previous?.sessionId === sessionId && previous.leaseId === leaseId) {
          previous.release();
          this.audioFlowLeases.delete(event.sender);
        }
        return;
      }
      if (previous?.sessionId === sessionId && previous.leaseId === leaseId) return;
      if (previous) throw new Error('another conversation owns audio flow');
      const runtime = this.bridge.get(sessionId);
      if (!runtime || this.settings.getPublic().activeSession?.sessionId !== sessionId) throw new Error('audio flow session is unavailable');
      const release = this.retainAudioRuntime(runtime);
      this.audioFlowLeases.set(event.sender, { sessionId, leaseId, runtime, release });
    });
    this.ipc.handle(CH_NATIVE_AUDIO_CANCEL, async (event: IpcMainInvokeEvent, flow?: unknown) => {
      this.assertSender(event);
      this.requireAudioFlowAccess(event.sender, flow);
      await this.nativeAudio?.cancelUiAudioOperations(String(event.sender.id));
    });
    this.ipc.handle(CH_NATIVE_AUDIO_FINISH_LISTEN, async (event: IpcMainInvokeEvent, flow?: unknown) => {
      this.assertSender(event);
      this.requireAudioFlowAccess(event.sender, flow);
      await this.nativeAudio?.finishUiAudioListen(String(event.sender.id));
    });
  }

  private requireAudioFlowAccess(sender: WebContents, value: unknown): { sessionId: string; leaseId: string; runtime: SessionRuntime } | undefined {
    const owner = this.audioFlowLeases.get(sender);
    if (!owner && value === undefined) return undefined;
    if (!owner || !value || typeof value !== 'object' || Array.isArray(value)) throw new Error('another conversation owns audio flow');
    const input = value as Record<string, unknown>;
    if (input['sessionId'] !== owner.sessionId || input['leaseId'] !== owner.leaseId) throw new Error('another conversation owns audio flow');
    if (this.bridge.get(owner.sessionId) !== owner.runtime) throw new Error('The audio session owner changed.');
    return owner;
  }

  private retainAudioRuntime(runtime: SessionRuntime): () => void {
    if (this.bridge.get(runtime.sessionId) !== runtime || this.isProjectClosing(runtime.projectPath)) {
      throw new Error('The audio session is no longer available.');
    }
    const releaseOperation = runtime.retainAudioOperation();
    let releaseBackground: () => void;
    try { releaseBackground = this.bridge.retainBackgroundSession({ projectPath: runtime.projectPath, sessionId: runtime.sessionId }); }
    catch (error) { releaseOperation(); throw error; }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      releaseOperation();
      releaseBackground();
    };
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
      // A Bot's folder is trusted without being a project.
      return { path: workspace, fingerprint: trust.fingerprint, trusted: this.settings.isTrustedWorkspace(workspace) || this.isBotWorkspace(workspace) };
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
    const pendingAskUserQuestions = activeRuntime?.pendingAskUserQuestions ?? [];
    // Same source `diagnosticReport` below already reads for its
    // `bridgeRuntime` field — reused here rather than recomputed, so the
    // About page's engine version and the exported diagnostic report can
    // never disagree.
    const engineVersions = activeRuntime?.runtimeVersions;
    const snapshot = {
      revision,
      scheduledWorkspace: this.settings.scheduledWorkspace,
      conversationSpaces: this.conversationSpacesSnapshot(),
      settings: this.settings.getPublic(),
      workspace: this.workspace(),
      ...(activeSession ? { activeSession: { ...activeSession } } : {}),
      runtimes: this.runtimeSummaries(),
      projectCatalogs: Object.fromEntries([...this.catalogs.entries()].map(([path, state]) => [path, {
        sessions: state.sessions.map((session) => ({ ...session })),
        ...(state.error ? { error: state.error } : {}),
      }])),
      providerAccountSnapshot: this.providerAccounts?.latestSnapshot,
      ...(this.credentialBroker
        ? { credentialBrokerAvailable: this.brokerStorageError === undefined }
        : {}),
      ...(pendingAskUserQuestions.length > 0 ? { pendingAskUserQuestions: [...pendingAskUserQuestions] } : {}),
      connection: activeRuntime?.connectionState ?? { status: 'idle' },
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
      providerAccountSnapshot: this.providerAccounts?.latestSnapshot,
      connection: this.currentRuntime()?.connectionState ?? { status: 'idle' },
      bridgeRuntime: this.currentRuntime()?.runtimeVersions,
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
    return ref ? this.bridge.get(ref.sessionId) : undefined;
  }

  private runtimeSummaries(): SessionRuntimeSummary[] {
    return this.bridge.runtimeSummaries.flatMap((summary) => {
      const botId = this.botsUi?.botIdFor(summary.projectPath);
      // The hidden session that watches a Bot's clock is no chat of the Bot's, and must not make the Bot look at work.
      if (botId !== undefined && this.scheduled?.isControllerSession(summary.projectPath, summary.sessionId)) return [];
      return [{
        ...summary,
        ...(botId !== undefined ? { botId } : {}),
        connection: { ...summary.connection },
      }];
    });
  }

  private hasActiveWork(projectPath?: string): boolean {
    if (projectPath) return this.bridge.hasActiveWork(projectPath);
    const active = this.settings.getPublic().activeProject;
    if (active && this.bridge.hasActiveWork(active)) return true;
    return this.bridge.runtimeSummaries.some((runtime) => runtime.turnActive || runtime.pendingInteractions > 0);
  }

  private async refreshCredentialBrokerStatus(): Promise<void> {
    await this.providerAccounts?.snapshot();
    this.brokerStorageError = undefined;
  }

  private enqueueCredentialMutation<T>(providerId: string, operation: () => Promise<T>): Promise<T> {
    const previous = this.credentialMutations.get(providerId) ?? Promise.resolve();
    const mutation = previous.catch(() => undefined).then(operation);
    const completion = mutation.finally(() => {
      if (this.credentialMutations.get(providerId) === completion) this.credentialMutations.delete(providerId);
    });
    this.credentialMutations.set(providerId, completion);
    return completion;
  }

  private enqueuePluginSecretMutation<T>(pluginId: string, operation: () => Promise<T>): Promise<T> {
    return this.enqueueCredentialMutation(`plugin-secret:${pluginId}`, operation);
  }

  private async deletePluginSecretsAfterUninstall(pluginId: string): Promise<PluginSecretCleanupResult> {
    if (!this.credentialBroker) throw new Error('secure plugin credential broker is unavailable');
    const { brandHome } = await this.sessionCatalog.productPaths();
    const initial = readInstalledPluginRegistrySnapshot(brandHome, pluginId);
    if (initial.installed) {
      return { pluginId, status: 'retained', deletedCount: 0, failedCount: 0 };
    }

    type RegistryGuardStatus = 'stable' | 'retained' | 'changed' | 'unavailable';
    let guardStatus: RegistryGuardStatus = 'stable';
    const canDelete = async (): Promise<boolean> => {
      let current: InstalledPluginRegistrySnapshot;
      try {
        current = readInstalledPluginRegistrySnapshot(brandHome, pluginId);
      } catch (error) {
        guardStatus = 'unavailable';
        this.diagnostics.add('warn', 'host', `plugin credential cleanup registry check failed: ${sanitizeDiagnostic(error)}`);
        return false;
      }
      if (current.installed) {
        guardStatus = 'retained';
        return false;
      }
      if (current.revision !== initial.revision) {
        guardStatus = 'changed';
        return false;
      }
      guardStatus = 'stable';
      return true;
    };

    const summary = await deletePluginSecretsForPlugin(pluginId, this.credentialBroker, canDelete);
    if (summary.failedCount > 0) {
      this.diagnostics.add('warn', 'host', `plugin credential cleanup left ${summary.failedCount} broker key(s)`);
    }
    const guardStatusAtCleanupEnd = guardStatus as RegistryGuardStatus;
    await canDelete();
    const checkedGuardStatus = guardStatus as RegistryGuardStatus;
    const effectiveGuardStatus = checkedGuardStatus === 'stable' && guardStatusAtCleanupEnd !== 'stable'
      ? guardStatusAtCleanupEnd
      : checkedGuardStatus;

    let restartRequired = false;
    if (summary.deletedCount > 0) {
      restartRequired = this.hasActiveWork();
      if (!restartRequired) {
        try {
          await this.restartIfConfigured();
        } catch (error) {
          restartRequired = true;
          this.diagnostics.add('warn', 'host', error);
        }
      }
    }

    let status: PluginSecretCleanupResult['status'];
    if (effectiveGuardStatus === 'retained' && summary.deletedCount === 0) status = 'retained';
    else if (effectiveGuardStatus !== 'stable') status = summary.deletedCount > 0 || summary.failedCount > 0 ? 'partial' : 'deferred';
    else if (summary.aborted || summary.failedCount > 0) status = 'partial';
    else status = 'cleared';
    return {
      pluginId,
      status,
      deletedCount: summary.deletedCount,
      failedCount: summary.failedCount,
      ...(restartRequired ? { restartRequired: true } : {}),
    };
  }

  private async requireCredentialProvider(providerId: unknown): Promise<{ id: string }> {
    if (typeof providerId !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(providerId)) {
      throw new Error('invalid provider id');
    }
    if (providerById(providerId)) return this.requireProvider(providerId);
    const profiles = (await this.sessionCatalog.providerSettings()).effective.providers;
    if (profiles && typeof profiles === 'object' && !Array.isArray(profiles) && Object.hasOwn(profiles, providerId)) return { id: providerId };
    const runtime = this.currentRuntime();
    if (!(runtime?.configuredCredentialIds ?? runtime?.customProviderIds)?.includes(providerId)) {
      if (!runtime?.ensureCustomProviderConfigured) throw new Error('unsupported provider');
      await runtime.ensureCustomProviderConfigured(providerId);
      if (runtime !== this.currentRuntime()
        || !(runtime.configuredCredentialIds ?? runtime.customProviderIds).includes(providerId)) throw new Error('unsupported provider');
    }
    return { id: providerId };
  }

  private requireProvider(providerId: unknown) {
    if (typeof providerId !== 'string') throw new Error('invalid provider id');
    const provider = providersFromCatalog(this.currentRuntime()?.officialProviderCatalog ?? []).find((p) => p.id === providerId) ?? providerById(providerId);
    if (!provider) throw new Error('unsupported provider');
    if (!provider.available) throw new Error(`${provider.label} sign-in is not available in this desktop build; use the CLI/TUI connect flow`);
    return provider;
  }

  private enqueueNavigation<T>(operation: () => Promise<T>): Promise<T> {
    const run = this.navigationQueue.then(operation, operation);
    this.navigationQueue = run.then(() => undefined, () => undefined);
    return run;
  }

  private conversationSpacesSnapshot(): ConversationSpacesSnapshot {
    const snapshot = this.settings.conversationSpaces.snapshot();
    snapshot.drafts = (this.bridge.draftReferences ?? []).flatMap((ref) => {
      const owner = this.settings.conversationSpaces.find(ref);
      return owner ? [owner] : [];
    });
    for (const [space, session] of this.draftSelections) {
      snapshot.navigation[space] = { projectId: session.projectId, session: { ...session } };
    }
    return snapshot;
  }

  private discardHandoff(requestId: string): void {
    const operation = this.handoffRetries.get(requestId);
    if (!operation) return;
    this.handoffRetries.delete(requestId);
    if (operation.timer) clearTimeout(operation.timer);
    operation.release();
  }

  private async prepareHandoff(sourceId: string, target: ConversationSpace, requestId: string, retry = false): Promise<{ snapshot: BootstrapState; summary: string; requestId: string } | null> {
    if (this.handoffs.has(requestId)) throw new Error('Handoff request is already active.');
    for (const [id, operation] of this.handoffRetries) if (operation.expires !== undefined && operation.expires <= Date.now()) this.discardHandoff(id);
    let retained = this.handoffRetries.get(requestId);
    if (retry) {
      if (!retained || retained.sourceId !== sourceId || retained.target !== target) throw new Error('This handoff can no longer be retried.');
    } else {
      if (retained) throw new Error('Use retry for an existing handoff.');
      if (this.handoffRetries.size >= 8) throw new Error('Too many handoffs are waiting to retry.');
    }
    const active = this.settings.getPublic().activeSession;
    const source = active && this.settings.conversationSpaces.find(active);
    if (!source || source.sessionId !== sourceId || source.mode === target
      || this.settings.conversationSpaces.snapshot().activeMode !== source.mode) throw new Error('The source conversation is no longer selected.');
    const runtime = this.bridge.get(sourceId);
    if (!runtime) throw new Error('The source conversation is unavailable.');
    const generation = this.navigationGeneration;
    const controller = new AbortController();
    const current = () => {
      const selected = this.settings.getPublic().activeSession;
      return !controller.signal.aborted && generation === this.navigationGeneration
        && this.settings.conversationSpaces.snapshot().activeMode === source.mode
        && selected?.sessionId === sourceId && selected.projectPath === source.projectPath;
    };
    if (!retained) {
      retained = { sourceId, target, release: this.bridge.retainBackgroundSession(source) };
      this.handoffRetries.set(requestId, retained);
    }
    if (retained.timer) clearTimeout(retained.timer);
    retained.timer = undefined;
    retained.expires = undefined;
    this.handoffs.set(requestId, controller);
    try {
      const result = retained.prepared ?? await runtime.prepareSessionHandoff(requestId, target, controller.signal, retry);
      retained.prepared = { cutoff: result.cutoff, summary: result.summary };
      if (!current()) { this.discardHandoff(requestId); return null; }
      const output = await this.enqueueNavigation(async () => {
        if (!current()) return null;
        const projectPath = this.settings.conversationSpaces.globalWorkspaces[target];
        const ref = await this.bridge.newSession(projectPath, undefined,
          { mode: target, projectId: null, forceNew: true, activate: current });
        if (!current()) {
          await this.bridge.closeSession(ref);
          return null;
        }
        const owned: OwnedConversation = { ...ref, mode: target, projectId: null, handoff: {
          sourceSessionId: source.sessionId, sourceProjectPath: source.projectPath,
          sourceMode: source.mode, cutoff: result.cutoff, requestId,
        } };
        this.settings.conversationSpaces.register(owned);
        this.draftSelections.set(target, owned);
        this.settings.conversationSpaces.setMode(target);
        this.bridge.activateSession(owned);
        this.settings.setActiveSessionDraft(owned);
        return { snapshot: await this.bootstrap(), summary: result.summary, requestId };
      });
      this.discardHandoff(requestId);
      return output;
    } catch (error) {
      if (this.handoffRetries.get(requestId) === retained) {
        if (!current()) this.discardHandoff(requestId);
        else {
          retained.expires = Date.now() + 5 * 60_000;
          retained.timer = setTimeout(() => this.discardHandoff(requestId), 5 * 60_000);
          retained.timer.unref?.();
        }
      }
      throw error;
    } finally {
      if (this.handoffs.get(requestId) === controller) this.handoffs.delete(requestId);
    }
  }

  /** The first admitted prompt makes the selected draft eligible for cold restoration. */
  commitConversationDraft(ref: SessionRef): void {
    const owned = this.settings.conversationSpaces.find(ref);
    if (!owned) return;
    const selected = this.draftSelections.get(owned.mode);
    if (selected?.sessionId === owned.sessionId) {
      this.draftSelections.delete(owned.mode);
      this.settings.conversationSpaces.setNavigation(owned.mode, owned.projectId, owned);
    }
  }

  /** Persist only the transcript confirmed by the live writer and its catalog. */
  private async recoverLocalAppSession(requestId: string, generation: number): Promise<BootstrapState> {
    if (!this.localAppSession) throw new Error('The Local App authoring host is unavailable');
    let saved = await this.localAppSession.get(requestId);
    const project = this.requireProject(saved.projectPath);
    if (!this.settings.hasProject(project) || !this.settings.conversationSpaces.snapshot().projects[project]?.includes('code')) {
      throw new Error('Add the saved Local App Project before recovering its Code session');
    }
    const owned: OwnedConversation = { projectPath: project, projectId: project, sessionId: saved.sessionId, mode: 'code' };
    const existing = this.settings.conversationSpaces.findBySessionId(owned.sessionId);
    if (existing && (existing.projectPath !== project || existing.projectId !== project || existing.mode !== 'code')) {
      throw new Error('The Local App session owner changed');
    }
    const release = this.bridge.retainBackgroundSession(owned);
    try {
      this.sessionCatalog.invalidate(project);
      const remembered = this.settings.getSessionTranscriptSelection(owned);
      if (remembered && remembered.catalogProjectPath !== project) throw new Error('The Local App session source changed');
      if (!saved.source && (remembered || this.bridge.get(owned.sessionId))) {
        throw new Error('The Local App draft source was not confirmed; choose its exact session before continuing');
      }
      if (saved.source && remembered && (saved.source.transcriptPath !== remembered.transcriptPath || saved.source.scope !== remembered.scope)) {
        throw new Error('The Local App session source changed');
      }
      const selected = saved.source;
      const row = selected ? await this.sessionCatalog.select(project, owned.sessionId, selected) : undefined;
      if (selected && (!row || row.path !== selected.transcriptPath)) throw new Error('The Local App session source is unavailable');
      if (row && (row.uuid.toLowerCase() !== owned.sessionId.toLowerCase() || row.mode !== 'code' || !row.path || !isAbsolute(row.path)
        || (row.workspace_path && canonicalWorkspace(row.workspace_path) !== project))) {
        throw new Error('The Local App session source changed');
      }
      const live = this.bridge.get(owned.sessionId);
      let runtime: SessionRuntime;
      if (live) {
        if (!selected || live.projectPath !== project || live.sessionMode !== 'code' || live.connectionState.status !== 'connected'
          || live.isStarting || live.needsSessionResume) throw new Error('The Local App session source cannot be confirmed');
        const state = await live.getWorkspaceState();
        if (this.bridge.get(owned.sessionId) !== live || live.connectionState.status !== 'connected'
          || live.sessionMode !== 'code' || !sameTranscriptFile(state.transcript_path, selected.transcriptPath) || canonicalWorkspace(state.cwd) !== project) {
          throw new Error('The Local App session source changed');
        }
        runtime = live;
      } else {
        runtime = row
          ? await this.bridge.openSession(owned, row.empty_session === true, row.resume_model, false, row.path)
          : await this.bridge.restoreDurableDraft(owned, saved.name.trim() || 'Local App');
      }
      this.settings.conversationSpaces.register(owned);
      const path = await this.rememberCreatedSessionSource(owned, runtime, false, async source => {
        saved = await this.localAppSession!.recordSource(saved.requestId, source);
      }, selected);
      const confirmed = this.settings.getSessionTranscriptSelection(owned);
      if (!confirmed || confirmed.catalogProjectPath !== project || confirmed.transcriptPath !== path
        || (selected && (selected.transcriptPath !== path || selected.scope !== confirmed.scope))) throw new Error('The Local App session source changed');
      // Recovery never changes an App's current owner; a newer explicit rebind wins.
      await this.localAppSession.finish(saved);
      await this.assertLocalAppSessionRuntime(owned, runtime, path);
      if (generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === 'code') {
        this.bridge.activateSession(owned);
        this.settings.activateProject(project);
        this.settings.setActiveSessionDraft(owned);
      }
      return this.bootstrap();
    } finally { release(); }
  }

  private async assertLocalAppSessionRuntime(owner: OwnedConversation, runtime: SessionRuntime, path: string): Promise<void> {
    const state = await runtime.getWorkspaceState();
    const current = this.settings.conversationSpaces.findBySessionId(owner.sessionId);
    if (this.bridge.get(owner.sessionId) !== runtime || runtime.projectPath !== owner.projectPath || runtime.sessionMode !== 'code'
      || !current || current.mode !== 'code' || current.projectId !== owner.projectId || current.projectPath !== owner.projectPath
      || (owner.projectId !== null && (!this.settings.hasProject(owner.projectId) || this.isProjectClosing(owner.projectId)))
      || !sameTranscriptFile(state.transcript_path, path) || canonicalWorkspace(state.cwd) !== owner.projectPath) {
      throw new Error('The Local App session source changed');
    }
  }

  async rememberCreatedSessionSource(ref: SessionRef, runtime: SessionRuntime, scheduled: boolean,
    beforeRemember?: (source: AuthoringSessionSource) => Promise<void>, preferredSource?: AuthoringSessionSource): Promise<string> {
    const known = this.settings.conversationSpaces.findBySessionId(ref.sessionId);
    const managed = this.settings.scheduledSessionWorkspace(ref.sessionId);
    const projectId = managed?.projectPath === ref.projectPath ? null
      : this.settings.hasProject(ref.projectPath) ? ref.projectPath : undefined;
    const owner = known ?? (scheduled && projectId !== undefined && ref.mode
      ? { projectPath: ref.projectPath, sessionId: ref.sessionId, projectId, mode: ref.mode } : undefined);
    if (!owner || owner.projectPath !== ref.projectPath || runtime.projectPath !== owner.projectPath
      || runtime.sessionId.toLowerCase() !== owner.sessionId.toLowerCase() || runtime.sessionMode !== owner.mode
      || (ref.mode !== undefined && ref.mode !== owner.mode)
      || (ref.projectId !== undefined && ref.projectId !== owner.projectId)) throw new Error('The session owner changed before its source was recorded.');
    const state = await runtime.getWorkspaceState();
    const path = state.transcript_path;
    if (typeof path !== 'string' || !isAbsolute(path) || path.includes('\0')
      || basename(path).toLowerCase() !== `${owner.sessionId.toLowerCase()}.jsonl`
      || canonicalWorkspace(state.cwd) !== owner.projectPath || !lstatSync(path).isFile()) {
      throw new Error('The live session has no verified transcript source.');
    }
    if (preferredSource && !sameTranscriptFile(preferredSource.transcriptPath, path)) throw new Error('The Local App session source changed');
    const catalogPath = preferredSource?.transcriptPath ?? path;
    this.sessionCatalog.invalidate(owner.projectPath);
    let selected: { catalogProjectPath: string; scope: SessionCatalogScope; transcriptPath: string } | undefined;
    for (const scope of preferredSource ? [preferredSource.scope] : ['current', 'repository', 'all'] as const) {
      const row = await this.sessionCatalog.select(owner.projectPath, owner.sessionId, { scope, transcriptPath: catalogPath });
      if (!row || row.path !== catalogPath) continue;
      if (row.uuid.toLowerCase() !== owner.sessionId.toLowerCase() || row.mode !== owner.mode
        || (row.workspace_path && canonicalWorkspace(row.workspace_path) !== owner.projectPath)) {
        throw new Error('The catalog source does not match the session owner.');
      }
      selected = { catalogProjectPath: owner.projectPath, scope, transcriptPath: catalogPath };
      break;
    }
    if (!selected) throw new Error('The live session transcript is not available in its catalog.');
    const current = this.settings.conversationSpaces.findBySessionId(owner.sessionId);
    if (this.bridge.get(owner.sessionId) !== runtime || runtime.projectPath !== owner.projectPath || runtime.sessionMode !== owner.mode
      || (current && (current.projectPath !== owner.projectPath || current.projectId !== owner.projectId || current.mode !== owner.mode))
      || (!current && known)) throw new Error('The session owner changed before its source was recorded.');
    await beforeRemember?.({ scope: selected.scope, transcriptPath: selected.transcriptPath });
    if (beforeRemember) {
      const confirmed = await runtime.getWorkspaceState();
      const after = this.settings.conversationSpaces.findBySessionId(owner.sessionId);
      if (this.bridge.get(owner.sessionId) !== runtime || runtime.sessionMode !== owner.mode || !after
        || (after && (after.projectPath !== owner.projectPath || after.projectId !== owner.projectId || after.mode !== owner.mode))
        || (owner.projectId !== null && (!this.settings.hasProject(owner.projectId) || this.isProjectClosing(owner.projectId)))
        || !sameTranscriptFile(confirmed.transcript_path, selected.transcriptPath) || canonicalWorkspace(confirmed.cwd) !== owner.projectPath) {
        throw new Error('The session owner changed before its source was recorded.');
      }
    }
    if (!current) this.settings.conversationSpaces.register(owner);
    this.settings.rememberSessionTranscriptSelection(owner, selected);
    runtime.rememberTranscriptSelection(selected.transcriptPath);
    return selected.transcriptPath;
  }

  /** Return a mode only when the displayed owner agrees with the committed space. */
  currentConversationMode(): ConversationSpace | undefined {
    const mode = this.settings.conversationSpaces.snapshot().activeMode;
    const active = this.settings.getPublic().activeSession;
    if (!active) return mode;
    return this.settings.conversationSpaces.find(active)?.mode === mode ? mode : undefined;
  }

  private async switchConversationSpace(space: ConversationSpace, generation: number): Promise<BootstrapState> {
    const current = () => generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === space;
    if (!current()) return this.bootstrap();
    const draft = this.draftSelections.get(space);
    if (draft) {
      const ref = await this.bridge.reopenOwnedDraft(draft, current);
      if (current()) {
        if (ref.sessionId !== draft.sessionId) throw new Error('The draft conversation changed.');
        this.bridge.activateSession(draft);
        this.settings.setActiveSessionDraft(draft);
      }
      return this.bootstrap();
    }
    const navigation = this.settings.conversationSpaces.snapshot().navigation[space];
    if (navigation.session) return this.openSessionAndActivateInternal(navigation.session, undefined, generation, space);
    // An empty space still needs a live draft: the composer controls require
    // an active engine even before the first message is sent.
    const projectPath = this.settings.conversationSpaces.globalWorkspaces[space];
    const ref = await this.bridge.newSession(projectPath, undefined, { mode: space, projectId: null, activate: current });
    const owned: OwnedConversation = { ...ref, mode: space, projectId: null };
    this.settings.conversationSpaces.register(owned);
    if (current()) {
      this.draftSelections.set(space, owned);
      this.bridge.activateSession(owned);
      this.settings.setActiveSessionDraft(owned);
    }
    return this.bootstrap();
  }

  private async selectWorkspace(input: string, addProject: boolean): Promise<WorkspaceMetadata> {
    return this.enqueueNavigation(() => this.selectWorkspaceInternal(input, addProject));
  }

  private async selectWorkspaceInternal(input: string, addProject: boolean, createInitialSession = true,
    admittedMode?: ConversationSpace): Promise<WorkspaceMetadata> {
    const workspace = canonicalWorkspace(input);
    if (this.closingProjects.has(workspace)) throw new Error('project is closing');
    this.workspaceFiles.invalidate();
    if (addProject) {
      this.assertNotAppData(workspace);
      const wasEmpty = this.settings.getPublic().projects.length === 0;
      this.settings.addProject(workspace);
      this.settings.conversationSpaces.addProject(workspace, admittedMode ?? this.settings.conversationSpaces.snapshot().activeMode);
      // General project creation supplies the first draft. App creation only
      // adds metadata; the renderer explicitly admits its one authoring chat.
      if (wasEmpty && createInitialSession) {
        const mode = this.settings.conversationSpaces.snapshot().activeMode;
        const ref = await this.bridge.newSession(workspace, undefined, { mode, projectId: workspace });
        const owned: OwnedConversation = { ...ref, mode, projectId: workspace };
        this.settings.conversationSpaces.register(owned);
        this.draftSelections.set(mode, owned);
        this.settings.activateProject(workspace);
        this.settings.setActiveSessionDraft(owned);
      }
    } else {
      this.settings.conversationSpaces.addProject(workspace, this.settings.conversationSpaces.snapshot().activeMode);
      this.settings.activateProject(workspace);
    }
    let fingerprint: string | undefined;
    try { fingerprint = this.settings.getTrust(workspace).fingerprint; }
    catch (error) { this.diagnostics.add('warn', 'host', error); }
    return { path: workspace, trusted: this.settings.isTrustedWorkspace(workspace), ...(fingerprint ? { fingerprint } : {}) };
  }

  /** The app's own data, the Bots among it, is not a place for a project. */
  private assertNotAppData(canonical: string): void {
    if (this.settings.isAppDataFolder(canonical)) throw new Error("This folder belongs to the app's own data and cannot be used as a project.");
  }

  private assertNoActiveTurn(): void {
    if (this.hasActiveWork()) throw new Error('cancel the active turn before changing engine settings');
  }

  private assertRestartAllowed(ref: SessionRef, runtime: ReturnType<SessionRuntimeManager['get']>, recovering = false): void {
    if (!runtime) throw new Error(`session runtime is not open: ${ref.sessionId}`);
    if (runtime.hasActiveAudio) throw new Error('Stop audio before restarting this chat.');
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
    const launchInFlight = runtime.isStarting || ['spawning', 'restarting', 'connecting'].includes(runtime.connectionState.status);
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
    if (recovering && (runtime.recoveredLiveConnection || launchInFlight)) {
      // A living process or a launch begun before this configuration change
      // may still contain the old material. Apply a normal restart after it
      // settles, whose active-work guard can safely refuse it.
      await this.bridge.restart(ref, assertCurrent);
      assertCurrent();
    }
  }

  private requireCurrentRuntime() {
    const ref = this.settings.getPublic().activeSession;
    if (!ref) throw new Error('open a session first');
    return this.bridge.require(ref);
  }

  private async loadProjectSessions(projectPath: string, scope: SessionCatalogScope = 'current'): Promise<ProjectSessionCatalogState> {
    const key = `${projectPath}\0${scope}`;
    const epoch = this.catalogEpoch;
    const generation = (this.catalogRequestGenerations.get(key) ?? 0) + 1;
    this.catalogRequestGenerations.set(key, generation);
    try {
      const result = await this.sessionCatalog.list(projectPath, scope, (session) =>
        !this.scheduled?.isControllerSession(session.workspace_path ?? projectPath, session.uuid)
        && !this.settings.isSessionArchived({ projectPath, sessionId: session.uuid })
        && !this.settings.isSessionArchived({ projectPath: session.workspace_path ?? projectPath, sessionId: session.uuid }),
      (session) => {
        const owner = this.settings.conversationSpaces.findBySessionId(session.uuid);
        return JSON.stringify([session.mode, owner?.projectId]);
      });
      const unknown = result.sessions.filter((session) => !this.settings.conversationSpaces.findBySessionId(session.uuid));
      if (unknown.length) this.diagnostics.add('warn', 'host', `Ignored ${unknown.length} session(s) without an authenticated conversation scope.`);
      const state = {
        sessions: result.sessions.filter((session) => this.settings.conversationSpaces.findBySessionId(session.uuid)?.mode === session.mode
          && !this.scheduled?.isControllerSession(session.workspace_path ?? projectPath, session.uuid)
          && !this.settings.isSessionArchived({ projectPath, sessionId: session.uuid })
          && !this.settings.isSessionArchived({ projectPath: session.workspace_path ?? projectPath, sessionId: session.uuid })).map(({
          empty_session: _emptySession,
          resume_model: _resumeModel,
          workspace_path: _workspacePath,
          ...session
        }) => session),
      };
      if (scope === 'current' && this.catalogEpoch === epoch && this.catalogRequestGenerations.get(key) === generation) this.catalogs.set(projectPath, state);
      return state;
    } catch (error) {
      const state = { sessions: [], error: sanitizeDiagnostic(error) };
      if (scope === 'current' && this.catalogEpoch === epoch && this.catalogRequestGenerations.get(key) === generation) this.catalogs.set(projectPath, state);
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

  /** Resolve metadata ownership without changing the caller's sidebar scope. */
  private async resolveSessionSource(ref: SessionRef, requirePersisted = false, selection?: SessionOpenSelection): Promise<ResolvedMetadataSource> {
    const project = this.requireProject(ref.projectPath);
    if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    const runtime = this.bridge.get(ref.sessionId);
    let source = runtime?.projectPath ?? project;
    if (this.isProjectClosing(source)) throw new Error('project is closing');
    // The manager keys runtimes by stable session UUID. A connected engine
    // already proves its own source directory, including an unpersisted chat.
    if (runtime?.connectionState.status === 'connected') {
      const sourceRef = { projectPath: source, sessionId: ref.sessionId, mode: runtime.sessionMode };
      if (!requirePersisted) return { ref: sourceRef };
      // Same-UUID copies can share a workspace. Only the writer's live target
      // identifies the selected copy, so metadata must never resolve it again.
      const state = await runtime.getWorkspaceState();
      const path = state.transcript_path;
      if (this.bridge.get(ref.sessionId) !== runtime || runtime.projectPath !== source) {
        throw new Error('The session changed. Try again.');
      }
      if (typeof path !== 'string' || !isAbsolute(path) || path.includes('\0')
        || basename(path).toLowerCase() !== `${ref.sessionId.toLowerCase()}.jsonl`) {
        throw new Error('session must have an authoritative transcript before its metadata can be changed');
      }
      if (!lstatSync(path).isFile()) throw new Error('session path is not a file');
      if (selection?.transcriptPath && selection.transcriptPath !== path) throw new Error('The displayed transcript is not this chat owner. Refresh the session list.');
      return { ref: sourceRef, transcriptPath: path };
    }
    if (runtime) throw new Error('Reconnect this chat before changing its metadata.');
    this.sessionCatalog.invalidate(source);
    // A previously confirmed owner retains its exact source across cold
    // metadata actions. A first discovery may use the displayed Current row.
    const lookupRef = { ...ref, projectPath: source };
    const savedSource = this.settings.getSessionTranscriptSelection(lookupRef);
    if (selection?.transcriptPath && savedSource && savedSource.transcriptPath !== selection.transcriptPath) {
      throw new Error('The displayed transcript differs from the saved owner. Refresh the session list.');
    }
    if (!savedSource && !selection?.transcriptPath) throw new Error('Choose an exact transcript before changing its metadata.');
    const row = selection?.transcriptPath
      ? await this.sessionCatalog.select(project, ref.sessionId, selection)
      : savedSource
      ? await resolveSavedConversationTranscriptRow(this.settings, this.sessionCatalog, lookupRef)
      : undefined;
    if (!row) throw new Error('session does not belong to this project');
    const known = this.settings.conversationSpaces.findBySessionId(ref.sessionId);
    if (known && known.mode !== row.mode) throw new Error('The transcript mode differs from its fixed owner.');
    if (row.workspace_path) {
      try { source = canonicalWorkspace(row.workspace_path); }
      catch (error) {
        if (!['ENOENT', 'ENOTDIR'].includes((error as NodeJS.ErrnoException).code ?? '')) throw error;
        // Keep the recorded owner and selected transcript when a stale
        // workspace hint names a directory that no longer exists.
        source = known?.projectPath ?? project;
      }
    }
    if (this.isProjectClosing(source)) throw new Error('project is closing');
    return { ref: { projectPath: source, sessionId: ref.sessionId, mode: row.mode }, row, transcriptPath: row.path };
  }

  private releaseMetadataSource(token: string): void {
    const captured = this.metadataSources.get(token);
    if (!captured) return;
    this.metadataSources.delete(token);
    clearTimeout(captured.timer);
    captured.release();
  }

  private releaseMetadataSourcesFor(sender: WebContents): void {
    this.metadataSenderGenerations.set(sender, (this.metadataSenderGenerations.get(sender) ?? 0) + 1);
    for (const [token, captured] of this.metadataSources) {
      if (captured.sender === sender) this.releaseMetadataSource(token);
    }
  }

  private async prepareMetadataSource(ref: SessionRef, selection?: SessionOpenSelection, sender?: WebContents): Promise<SessionMetadataSelection> {
    const lifecycleEpoch = this.metadataLifecycleEpoch;
    const senderGeneration = sender ? this.metadataSenderGenerations.get(sender) ?? 0 : undefined;
    const source = await this.resolveSessionSource(ref, true, selection);
    const path = source.transcriptPath;
    const runtime = this.bridge.get(ref.sessionId);
    const mode = source.ref.mode ?? source.row?.mode ?? runtime?.sessionMode;
    if (!path || (mode !== 'chat' && mode !== 'code')) throw new Error('The metadata source is unavailable.');
    const existing = this.settings.conversationSpaces.findBySessionId(ref.sessionId);
    if (!existing) throw new Error('The session has no authenticated conversation scope.');
    if (existing.mode !== mode || existing.projectPath !== source.ref.projectPath) throw new Error('The session owner changed. Reopen this action.');
    const owner = existing;
    const assertOwner = () => {
      const current = this.settings.conversationSpaces.findBySessionId(ref.sessionId);
      if ((!current && existing) || (current && (current.mode !== owner.mode || current.projectPath !== owner.projectPath
        || current.projectId !== owner.projectId))) throw new Error('The session owner changed. Reopen this action.');
    };
    if (runtime && (runtime.projectPath !== source.ref.projectPath || runtime.sessionMode !== mode
      || runtime.connectionState.status !== 'connected')) throw new Error('The metadata runtime owner changed during capture.');
    const releaseLease = runtime ? this.bridge.retainBackgroundSession(source.ref) : () => {};
    let released = false;
    let inFlight = 0;
    let releaseReceipt: (() => Promise<void>) | undefined;
    let releaseStarted = false;
    const finishRelease = () => {
      if (!released || inFlight || releaseStarted) return;
      releaseStarted = true;
      if (releaseReceipt) {
        // The server treats release as idempotent. Retry a lost acknowledgement
        // on the same captured socket; never replay it to a replacement socket.
        void (async () => {
          for (let attempt = 0; attempt < 3; attempt++) {
            try { await releaseReceipt!(); return; }
            catch (error) {
              if (attempt === 2) this.diagnostics.add('warn', 'host', `metadata release failed: ${sanitizeDiagnostic(error)}`);
            }
          }
        })();
      }
      // Sending release begins synchronously. If the runtime is then evicted,
      // closing its socket also clears every receipt on that connection.
      releaseLease();
    };
    const release = () => { if (!released) { released = true; finishRelease(); } };
    try {
      const apply = runtime
        ? await (async () => {
          const captured = await runtime.captureSessionMetadataOwner(path, mode);
          releaseReceipt = captured.release;
          return captured.apply;
        })()
        : await (async () => {
          const receipt = await this.sessionCatalog.captureMetadataSource(source.ref.projectPath, ref.sessionId, path, mode);
          return async (action: MetadataAction, title?: string) => {
            if (this.bridge.get(ref.sessionId)) throw new Error('The metadata source acquired another runtime. Reopen this action.');
            await this.sessionCatalog.mutateMetadataSource(source.ref.projectPath, receipt, action, title);
          };
        })();
      if (this.metadataLifecycleEpoch !== lifecycleEpoch || this.bridge.get(ref.sessionId) !== runtime || (sender && (!this.targets.has(sender) || sender.isDestroyed()
        || (this.metadataSenderGenerations.get(sender) ?? 0) !== senderGeneration))) throw new Error('The metadata owner changed during capture.');
      assertOwner();
      while (this.metadataSources.size >= 16) this.releaseMetadataSource(this.metadataSources.keys().next().value!);
      const token = randomUUID();
      const capturedSelection = { scope: selection?.scope ?? 'current', transcriptPath: path, sourceToken: token } satisfies SessionMetadataSelection;
      const timer = setTimeout(() => this.releaseMetadataSource(token), 5 * 60_000);
      timer.unref();
      this.metadataSources.set(token, { requested: { ...ref }, source, owner, selection: capturedSelection, sender, runtime,
        apply: async (action, title) => {
          if (released || (runtime && (this.bridge.get(ref.sessionId) !== runtime || runtime.projectPath !== source.ref.projectPath
            || runtime.sessionMode !== mode))) throw new Error('The captured metadata owner was released or replaced.');
          assertOwner();
          inFlight++;
          try {
            await apply(action, title);
            try {
              if (action === 'verify' && released) throw new Error('The captured metadata owner was released.');
              if (this.bridge.get(ref.sessionId) !== runtime || (runtime && (runtime.projectPath !== source.ref.projectPath
                || runtime.sessionMode !== mode))) throw new Error('The session runtime owner changed.');
              assertOwner();
            } catch (error) {
              const detail = error instanceof Error ? error.message : String(error);
              throw new Error(action === 'verify' ? detail : `The metadata may have been applied, but its owner changed: ${detail}`);
            }
          }
          finally { --inFlight; finishRelease(); }
        }, release, timer });
      return capturedSelection;
    } catch (error) { release(); throw error; }
  }

  private async metadataSource(ref: SessionRef, prepared: SessionMetadataSelection, sender?: WebContents): Promise<CapturedMetadataSource> {
    const captured = this.metadataSources.get(prepared.sourceToken);
    if (!captured || captured.sender !== sender || captured.requested.projectPath !== ref.projectPath
      || captured.requested.sessionId !== ref.sessionId || captured.selection.transcriptPath !== prepared.transcriptPath
      || captured.selection.scope !== prepared.scope) throw new Error('The metadata selection expired or changed. Reopen this action.');
    return captured;
  }

  private async trustSessionSource(ref: SessionRef): Promise<void> {
    if (this.isProjectClosing(ref.projectPath)) throw new Error('project is closing');
    if (this.settings.isTrustedWorkspace(ref.projectPath)) return;
    this.assertNotAppData(ref.projectPath);
    if (!await this.directoryDialogs.trustDirectory(ref.projectPath)) throw new Error('Opening this directory was cancelled.');
    if (this.isProjectClosing(ref.projectPath)) throw new Error('project is closing');
    if (canonicalWorkspace(ref.projectPath) !== ref.projectPath) throw new Error('The session directory changed. Refresh the session list and try again.');
    this.settings.conversationSpaces.trustWorkspace(ref.projectPath);
  }

  async openSessionAndActivate(ref: SessionRef, selection?: SessionOpenSelection): Promise<BootstrapState> {
    ++this.navigationGeneration;
    return this.enqueueNavigation(() => this.openSessionAndActivateInternal(ref, selection));
  }

  async updateSessionWorkspace(sessionId: string, operation: SessionWorkspaceOperation): Promise<BootstrapState | null> {
    ++this.navigationGeneration;
    return this.enqueueNavigation(async () => {
      const ref = this.settings.getPublic().activeSession;
      if (!ref || ref.sessionId !== sessionId) throw new Error('This chat is no longer active.');
      // The folders a Bot may use are the ones in its settings, not whatever a chat asks for.
      if (this.isBotWorkspace(ref.projectPath)) throw new Error("A Bot works in its own folder. Change the folders it may use in the Bot's settings.");
      const runtime = this.bridge.require(ref);
      const release = runtime.beginWorkspaceChange();
      let originalCwd: string | undefined;
      let attemptedMove = false;
      let committed = false;
      try {
        const before = await runtime.getWorkspaceState();
        if (before.initial_project_root) this.bridge.setSessionProjectRoot(sessionId, before.initial_project_root);
        if (before.prompt_directories) this.bridge.setSessionPromptDirectories(sessionId, before.prompt_directories);
        originalCwd = before.cwd;
        const path = operation.path ?? await this.directoryDialogs.pickDirectory(before.cwd);
        if (path === undefined) return null;
        if (operation.action === 'add-dir') {
          const state = await runtime.addWorkingDirectory(path);
          this.bridge.setSessionDirectoryGrants(sessionId, state.additional_directories);
          if (state.prompt_directories) this.bridge.setSessionPromptDirectories(sessionId, state.prompt_directories);
          this.sessionCatalog.invalidate();
          this.catalogEpoch++;
          this.catalogRequestGenerations.clear();
          await this.loadProjectSessions(ref.projectPath);
          return this.bootstrap();
        }
        this.settings.assertSessionRelocationAllowed(ref.projectPath, ref);
        this.bridge.setSessionDirectoryGrants(sessionId, before.additional_directories);
        let previousSourceIdentity: { dev: number; ino: number } | undefined;
        if (before.transcript_path) {
          try {
            const stat = lstatSync(before.transcript_path);
            if (!stat.isFile()) throw new Error('The session source is not a regular file.');
            previousSourceIdentity = { dev: stat.dev, ino: stat.ino };
          } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
          }
        }
        attemptedMove = true;
        let result = await runtime.setWorkingDirectory({ path });
        // Every affirmative answer is pinned to the canonical path shown by
        // the engine. A symlink change must ask again for the new directory.
        while (result.status === 'needs_trust') {
          this.assertNotAppData(result.directory);
          this.settings.assertSessionRelocationAllowed(result.directory, ref);
          if (!await this.directoryDialogs.trustDirectory(result.directory)) return null;
          result = await runtime.setWorkingDirectory({ path, trust_accepted: true, trusted_directory: result.directory });
        }
        if (result.status === 'rejected') throw new Error(result.message);
        if (!result.changed) return this.bootstrap();
        const after = await runtime.getWorkspaceState();
        const target = canonicalWorkspace(result.cwd);
        if (this.isProjectClosing(target)) throw new Error('project is closing');
        this.assertNotAppData(target);
        const transcriptPath = after.transcript_path;
        const savedSource = this.settings.getSessionTranscriptSelection(ref);
        if (transcriptPath !== undefined && (!isAbsolute(transcriptPath) || transcriptPath.includes('\0')
          || basename(transcriptPath).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`)) {
          throw new Error('The engine reported an invalid session source.');
        }
        if ((previousSourceIdentity || before.has_conversation === true || savedSource)
          && before.transcript_path !== transcriptPath) throw new Error('The session source changed during the directory update.');
        if (savedSource && savedSource.transcriptPath !== transcriptPath) throw new Error('The session source changed during the directory update.');
        let sourceExists = false;
        if (transcriptPath) {
          try {
            const stat = lstatSync(transcriptPath);
            if (!stat.isFile()) throw new Error('The session source is not a regular file.');
            if (previousSourceIdentity && (stat.dev !== previousSourceIdentity.dev || stat.ino !== previousSourceIdentity.ino)) {
              throw new Error('The session source changed during the directory update.');
            }
            sourceExists = true;
          } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
          }
        }
        if (!sourceExists && (previousSourceIdentity || before.has_conversation === true || savedSource || after.has_conversation !== false)) {
          throw new Error('The session source is unavailable after the directory update.');
        }
        // A durable empty anchor still owns its exact transcript. Only a
        // genuinely unwritten draft may be remounted as a fresh session.
        const empty = !sourceExists;
        this.sessionCatalog.invalidate();
        const row = sourceExists ? await this.sessionCatalog.select(target, sessionId,
          { scope: 'current', transcriptPath: transcriptPath! }) : undefined;
        if (sourceExists && (!row || row.path !== transcriptPath)) throw new Error('The session source is unavailable in its new directory.');
        if (row && (row.uuid.toLowerCase() !== sessionId.toLowerCase()
          || (runtime.sessionMode && row.mode !== runtime.sessionMode)
          || (row.workspace_path && canonicalWorkspace(row.workspace_path) !== target))) {
          throw new Error('The session catalog does not match its source owner.');
        }
        if (previousSourceIdentity && transcriptPath) {
          const stat = lstatSync(transcriptPath);
          if (!stat.isFile() || stat.dev !== previousSourceIdentity.dev || stat.ino !== previousSourceIdentity.ino) {
            throw new Error('The session source changed during the directory update.');
          }
        }
        const relocateDraft = this.bridge.prepareDraftRelocation(ref, target);
        this.settings.relocateSession(ref, target, empty ? null : {
          catalogProjectPath: target, scope: 'current', transcriptPath: transcriptPath!,
        });
        relocateDraft();
        const relocatedOwner = this.settings.conversationSpaces.find({ ...ref, projectPath: target });
        if (relocatedOwner && this.draftSelections.get(relocatedOwner.mode)?.sessionId === sessionId) {
          this.draftSelections.set(relocatedOwner.mode, relocatedOwner);
        }
        committed = true;
        if (after.initial_project_root) this.bridge.setSessionProjectRoot(sessionId, after.initial_project_root);
        if (after.prompt_directories) this.bridge.setSessionPromptDirectories(sessionId, after.prompt_directories);
        const moved = { projectPath: target, sessionId };
        this.terminals?.migrateScope(ref, moved);
        this.sessionCatalog.invalidate();
        this.catalogEpoch++;
        this.catalogRequestGenerations.clear();
        this.catalogs.delete(ref.projectPath);
        this.workspaceFiles.invalidate();
        // Releasing and removing the old runtime are synchronous up to the
        // close's first await. No prompt can enter the old composition here.
        release();
        await this.bridge.closeSession(ref, { replacing: true });
        await this.bridge.openSession(moved, empty,
          row?.resume_model ?? this.settings.getSessionModel(moved), true, empty ? undefined : transcriptPath,
          after.runtime_controls);
        await this.loadProjectSessions(target);
        return this.bootstrap();
      } catch (error) {
        if (attemptedMove && !committed && originalCwd) {
          try {
            // A response timeout is not evidence that the mutation failed.
            const actual = await runtime.getWorkspaceState();
            if (actual.cwd !== originalCwd) {
              const restored = await runtime.setWorkingDirectory({ path: originalCwd,
                trust_accepted: true, trusted_directory: originalCwd });
              if (restored.status !== 'ok' || restored.cwd !== originalCwd) throw new Error('directory rollback was rejected');
            }
          } catch (recoveryError) {
            release();
            await this.bridge.closeSession(ref).catch(() => undefined);
            this.sessionCatalog.invalidate();
            throw new Error(`${sanitizeDiagnostic(error)} The engine was stopped because its directory could not be restored: ${sanitizeDiagnostic(recoveryError)}`);
          }
        }
        throw error;
      } finally { release(); }
    });
  }

  private async openSessionAndActivateInternal(ref: SessionRef, selection?: SessionOpenSelection, generation?: number, expectedMode?: ConversationSpace,
    authoring?: CapturedLocalAppAuthoring): Promise<BootstrapState> {
    if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
    const project = this.requireProject(ref.projectPath);
    ref = { projectPath: project, sessionId: ref.sessionId.toLowerCase() };
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    const draft = !selection && this.bridge.hasOwnedDraft?.(ref) ? this.settings.conversationSpaces.find(ref) : undefined;
    if (draft) {
      if (expectedMode && draft.mode !== expectedMode) throw new Error('The session belongs to a different conversation space.');
      const current = () => generation === undefined || (generation === this.navigationGeneration
        && this.settings.conversationSpaces.snapshot().activeMode === expectedMode);
      if (authoring && draft.projectPath !== authoring.sourceDirectory) throw new Error('The Local App authoring workspace changed');
      await this.bridge.reopenOwnedDraft(draft, () => !authoring && current());
      const activate = () => {
        if (!current()) return;
        this.bridge.activateSession(draft);
        this.draftSelections.set(draft.mode, draft);
        this.settings.setActiveSessionDraft(draft);
      };
      if (authoring && current()) {
        const runtime = this.bridge.get(draft.sessionId);
        if (!runtime || runtime.sessionMode !== 'code' || canonicalWorkspace((await runtime.getWorkspaceState()).cwd) !== authoring.sourceDirectory) {
          throw new Error('The Local App authoring workspace changed');
        }
        await authoring.commit(activate);
      } else if (!authoring) activate();
      return this.bootstrap();
    }
    const existing = this.bridge.get(ref.sessionId);
    const remembered = !selection && !existing ? this.settings.getSessionTranscriptSelection(ref) : undefined;
    const catalogProject = remembered?.catalogProjectPath ?? project;
    // Current aliases and scoped picker copies deliberately differ from UUID
    // resolution. A cold engine must use the last acknowledged exact source.
    if (remembered) selection = { scope: remembered.scope, transcriptPath: remembered.transcriptPath };
    if ((!existing && !selection) || (selection && !selection.transcriptPath)) {
      throw new Error('The session source is unavailable. Choose an exact transcript before opening it.');
    }
    let source = existing?.projectPath ?? project;
    let session = remembered ? await resolveSavedConversationTranscriptRow(this.settings, this.sessionCatalog, ref)
      : selection ? await this.sessionCatalog.select(catalogProject, ref.sessionId, selection) : undefined;
    if (remembered && !session) throw new Error('The saved session selection is unavailable. Refresh the session list and choose a transcript.');
    if ((!existing || selection) && !session) throw new Error('session does not belong to this project');
    if (selection?.transcriptPath !== undefined && session?.path !== selection.transcriptPath) throw new Error('This session selection has changed. Refresh the session list and try again.');
    if (selection && (!session?.path || !isAbsolute(session.path))) throw new Error('invalid selected transcript path');
    if ((!existing || selection) && session?.workspace_path) {
      try { source = canonicalWorkspace(session.workspace_path); }
      catch (error) {
        if (!['ENOENT', 'ENOTDIR'].includes((error as NodeJS.ErrnoException).code ?? '')) throw error;
        // A removed worktree resumes in the launch directory, while the
        // engine keeps the authoritative transcript path it resolved by UUID.
        source = project;
      }
    }
    if (authoring) {
      if (source !== authoring.sourceDirectory) throw new Error('The Local App authoring workspace changed');
      if (existing?.connectionState.status === 'connected'
        && (existing.sessionMode !== 'code' || canonicalWorkspace((await existing.getWorkspaceState()).cwd) !== authoring.sourceDirectory)) {
        throw new Error('The Local App authoring workspace changed');
      }
    }
    const known = this.settings.conversationSpaces.find({ projectPath: source, sessionId: ref.sessionId })
      ?? this.settings.conversationSpaces.find(ref) ?? this.settings.conversationSpaces.findBySessionId(ref.sessionId);
    if (!known) throw new Error('The session has no authenticated conversation scope.');
    const ownerMode = known.mode;
    if ((session && session.mode !== ownerMode) || (expectedMode && expectedMode !== ownerMode)) {
      throw new Error('The session belongs to a different conversation space.');
    }
    const projectId = known.projectId;
    let releaseCopy: (() => void) | undefined;
    if (selection && existing) {
      const state = existing.connectionState.status === 'connected' ? await existing.getWorkspaceState() : undefined;
      if (existing.projectPath !== source || state?.transcript_path !== session?.path) {
        releaseCopy = existing.beginWorkspaceChange();
      }
    }
    try {
      await this.trustSessionSource({ projectPath: source, sessionId: ref.sessionId });
      if (releaseCopy && existing) {
        releaseCopy();
        await this.bridge.closeSession({ projectPath: existing.projectPath, sessionId: existing.sessionId }, { replacing: true });
      }
      const canonical: OwnedConversation = { projectPath: source, sessionId: ref.sessionId, mode: ownerMode, projectId };
      await this.bridge.openSession(canonical, session?.empty_session === true, session?.resume_model,
        generation === undefined && !authoring, selection ? session?.path : undefined);
      if (generation !== undefined && (generation !== this.navigationGeneration
        || this.settings.conversationSpaces.snapshot().activeMode !== expectedMode)) return this.bootstrap();
      if (authoring) {
        const runtime = this.bridge.get(ref.sessionId);
        if (!runtime || runtime.sessionMode !== 'code' || canonicalWorkspace((await runtime.getWorkspaceState()).cwd) !== authoring.sourceDirectory) {
          throw new Error('The Local App authoring workspace changed');
        }
      }
      const commit = () => {
        if (generation !== undefined && (generation !== this.navigationGeneration
          || this.settings.conversationSpaces.snapshot().activeMode !== expectedMode)) return { current: false, wasArchived: false };
        // The engine must acknowledge the exact copy before persistent ownership moves.
        if (known.projectPath !== source) this.settings.conversationSpaces.relocate(known, source);
        if (generation !== undefined || authoring) this.bridge.activateSession(canonical);
        const wasArchived = this.settings.isSessionArchived(canonical) || this.settings.isSessionArchived(ref);
        this.settings.relocateSession(known ?? { projectPath: project, sessionId: ref.sessionId }, source, selection ? {
          catalogProjectPath: catalogProject, scope: selection.scope, transcriptPath: session!.path,
        } : undefined);
        const recorded = this.settings.conversationSpaces.find(canonical);
        if (recorded) {
          this.settings.conversationSpaces.setMode(recorded.mode);
          this.settings.conversationSpaces.setNavigation(recorded.mode, recorded.projectId, recorded);
          this.draftSelections.delete(recorded.mode);
        }
        if (wasArchived) { this.settings.setSessionArchived(canonical, false); this.remoteControlUi?.sessionsChanged(); }
        return { current: true, wasArchived };
      };
      const result = authoring ? await authoring.commit(commit) : commit();
      if (!result.current) return this.bootstrap();
      if (result.wasArchived) await this.loadProjectSessions(source);
      return this.bootstrap();
    } finally { releaseCopy?.(); }
  }

  private archiveRef(projectPath: unknown, sessionId: unknown): SessionRef {
    const project = this.requireProject(projectPath);
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    return { projectPath: project, sessionId };
  }

  private archiveOwner(captured: CapturedMetadataSource): OwnedConversation {
    const owner = captured.owner;
    const current = this.settings.conversationSpaces.findBySessionId(owner.sessionId);
    if (current && (current.mode !== owner.mode || current.projectPath !== owner.projectPath
      || current.projectId !== owner.projectId)) throw new Error('The session owner changed. Reopen this action.');
    return owner;
  }

  private async sessionArchiveJobs(captured: CapturedMetadataSource): Promise<CronJobDto[]> {
    if (!this.scheduled) throw new Error('Scheduled tasks are unavailable. Try again when they are ready.');
    const owner = this.archiveOwner(captured);
    const scope = owner.projectId ?? 'global';
    const path = captured.source.transcriptPath;
    if (!path) throw new Error('The session source is unavailable.');
    const sameId = (value: string | undefined) => value?.replace(/^sess:/i, '').toLowerCase() === owner.sessionId.toLowerCase();
    const jobs = await this.scheduled.manage(scope, { action: 'list' }, owner.mode);
    return jobs.filter(({ automation }) => {
      if (!automation || automation.status === 'completed') return false;
      if (automation.runMode === 'selected_session') return sameId(automation.targetSessionId)
        && automation.targetSessionTranscriptPath === path && automation.targetSessionMode === owner.mode
        && automation.targetSessionScopeKey === scope;
      if (automation.runMode === 'task_session') return sameId(automation.ownedSessionId)
        && automation.ownedSessionTranscriptPath === path && automation.ownedSessionMode === owner.mode
        && automation.ownedSessionScopeKey === scope;
      return false;
    });
  }

  private async archiveSessionInternal(ref: SessionRef, captured: CapturedMetadataSource): Promise<BootstrapState> {
    const generation = this.navigationGeneration;
    const source = captured.source;
    const sourceRef = source.ref;
    const owner = this.archiveOwner(captured);
    const scope = owner.projectId ?? 'global';
    const runtime = captured.runtime;
    const release = runtime?.beginArchive() ?? (() => {});
      let removed = 0;
      try {
        await captured.apply('verify');
        await this.trustSessionSource(sourceRef);
        await captured.apply('verify');
        const jobs = await this.sessionArchiveJobs(captured);
        for (const job of jobs) {
          await captured.apply('verify');
          if (job.automation) await this.scheduled!.manage(scope, { action: 'pause', id: job.id, automation: { ...job.automation, status: 'paused', statusReason: 'Target chat was archived. Select another chat to resume.' } }, owner.mode);
          removed++;
        }
        await captured.apply('verify');
        const active = this.settings.getPublic().activeSession?.sessionId === ref.sessionId;
        const catalogTitle = (projectPath: string) => this.catalogs.get(projectPath)?.sessions.find((item) =>
          item.uuid.toLowerCase() === ref.sessionId.toLowerCase() && item.path === source.transcriptPath && item.mode === owner.mode)?.title;
        const title = source.row?.title ?? catalogTitle(sourceRef.projectPath) ?? catalogTitle(ref.projectPath);
        if (runtime?.hasActiveWork) throw new Error('Wait for active work and pending interactions before archiving this chat.');
        this.settings.setSessionArchived(sourceRef, true, title);
        const selected = this.settings.conversationSpaces.snapshot().navigation[owner.mode].session;
        if (selected?.sessionId === owner.sessionId) this.settings.conversationSpaces.setNavigation(owner.mode, owner.projectId);
        this.remoteControlUi?.sessionsChanged();
        await this.terminals?.closeScope(sourceRef);
        if (runtime && this.bridge.get(ref.sessionId) === runtime) await this.bridge.closeSession(sourceRef);
        this.sessionCatalog.invalidate(sourceRef.projectPath);
        await this.loadProjectSessions(sourceRef.projectPath);
        if (sourceRef.projectPath !== ref.projectPath) await this.loadProjectSessions(ref.projectPath);
        const current = () => generation === this.navigationGeneration
          && this.settings.conversationSpaces.snapshot().activeMode === owner.mode;
        if (active && current()) {
          const replacement = await this.bridge.newSession(sourceRef.projectPath, undefined,
            { mode: owner.mode, projectId: owner.projectId, activate: current });
          const draft: OwnedConversation = { ...replacement, mode: owner.mode, projectId: owner.projectId };
          this.settings.conversationSpaces.register(draft);
          if (current()) {
            this.draftSelections.set(owner.mode, draft);
            this.settings.setActiveSessionDraft(draft);
          }
        }
        return this.bootstrap();
      } catch (error) {
        if (!this.settings.isSessionArchived(sourceRef)) throw new Error(`Chat was not archived. ${removed ? `${removed} scheduled task(s) were already paused. ` : ''}${error instanceof Error ? error.message : String(error)}`);
        throw new Error(`Chat was archived, but opening the next chat failed: ${error instanceof Error ? error.message : String(error)}`);
      } finally { release(); }
  }

  private hasRetainedProjectForDirectory(project: string): boolean {
    let directory: string;
    try { directory = canonicalWorkspace(project); }
    catch { return false; }
    return this.settings.getPublic().projects.some((other) => {
      if (other === project) return false;
      try { return canonicalWorkspace(other) === directory; }
      catch { return false; }
    });
  }

  private async removeProjectInternal(project: string): Promise<BootstrapState> {
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    if (this.bridge.hasActiveWork(project)) throw new Error('cancel active turns and pending interactions before removing a project');
    // Schedules live in the directory, so another saved path to the same
    // directory must keep its tasks running when only this entry is removed.
    if (this.scheduled && !this.hasRetainedProjectForDirectory(project)) {
      const jobs = await this.scheduled.manage(project, { action: 'list' });
      for (const job of jobs) if (job.automation?.status === 'active') {
        await this.scheduled.manage(project, { action: 'pause', id: job.id, automation: { ...job.automation, status: 'paused', statusReason: 'Project was removed. Add the project again to resume.' } });
      }
    }
    // Set the host-side guard before invoking the manager. The manager then
    // synchronously removes matching runtimes from its routable map before its
    // first await, so a racing prompt cannot enter a disposing runtime.
    this.closingProjects.add(project);
    try {
      if (this.bridge.hasActiveWork(project)) throw new Error('cancel active work and pending interactions before removing a project');
      await this.terminals?.closeProject(project);
      await this.bridge.closeProject(project);
      this.settings.removeProject(project);
      this.settings.conversationSpaces.removeProject(project);
      for (const [mode, draft] of this.draftSelections) if (draft.projectId === project) this.draftSelections.delete(mode);
      for (const watches of this.gitWatches.values()) {
        const watch = watches.get(project);
        watches.delete(project);
        if (watch) void watch.then(stop => stop()).catch(() => undefined);
      }
      this.workspaceFiles.invalidate();
      this.catalogs.delete(project);
      this.sessionCatalog.invalidate(project);
      this.catalogEpoch++;
      this.catalogRequestGenerations.clear();
      return this.bootstrap();
    } finally {
      this.closingProjects.delete(project);
    }
  }

  private isProjectClosing(projectPath: string): boolean {
    return this.closingProjects.has(projectPath) || this.bridge.isProjectClosing(projectPath);
  }

  restoreProjectSession(projectPath: string, activeSession?: SessionRef): Promise<SessionRef> {
    return this.enqueueNavigation(() => this.restoreProjectSessionInternal(projectPath, activeSession));
  }

  /** Restore the selected space without deriving project ownership from cwd. */
  restoreConversationNavigation(): Promise<void> {
    const generation = this.navigationGeneration;
    return this.enqueueNavigation(async () => {
      if (generation !== this.navigationGeneration) return;
      const snapshot = this.settings.conversationSpaces.snapshot();
      const session = snapshot.navigation[snapshot.activeMode].session;
      if (session) await this.openSessionAndActivateInternal(session, undefined, undefined, session.mode);
      else await this.switchConversationSpace(snapshot.activeMode, generation);
    });
  }

  /**
   * The owner clicked a notification: show the session it came from. A Bot's
   * session is no project's, so it opens through the Bot, and the window is told
   * which session it now shows, because it shows only what it has been given.
   */
  async showNotifiedSession(ref: SessionRef): Promise<void> {
    const botId = this.botsUi?.botIdFor(ref.projectPath);
    if (botId === undefined) {
      const generation = ++this.navigationGeneration;
      await this.enqueueNavigation(async () => {
        const projectPath = this.requireProject(ref.projectPath);
        const requested = { ...ref, projectPath };
        const owner = this.settings.conversationSpaces.findBySessionId(ref.sessionId);
        const runtime = this.bridge.get(ref.sessionId);
        if ((owner && owner.projectPath !== projectPath) || (runtime && runtime.projectPath !== projectPath)) {
          throw new Error('The notification session owner does not match this workspace.');
        }
        const live = runtime;
        const saved = live ? undefined : await resolveSavedConversationTranscript(this.settings, this.sessionCatalog, requested);
        if (!live && !saved) throw new Error('The session source is unavailable. Choose an exact transcript before opening it.');
        const mode = owner?.mode ?? live?.sessionMode ?? saved?.mode;
        if (!mode) throw new Error('The notification session mode is unavailable.');
        if ((live?.sessionMode && live.sessionMode !== mode) || (requested.mode && requested.mode !== mode)) {
          throw new Error('The notification session belongs to a different conversation space.');
        }
        if (generation !== this.navigationGeneration) return;
        const previousMode = this.settings.conversationSpaces.snapshot().activeMode;
        this.settings.conversationSpaces.setMode(mode);
        try {
          const snapshot = await this.openSessionAndActivateInternal(requested, undefined, generation, mode);
          if (generation === this.navigationGeneration) this.sendToWindows(CH_CONVERSATION_SHOWN, snapshot);
        } catch (error) {
          if (generation === this.navigationGeneration && this.settings.conversationSpaces.snapshot().activeMode === mode) {
            this.settings.conversationSpaces.setMode(previousMode);
          }
          throw error;
        }
      });
      return;
    }
    const shown = await this.enqueueNavigation(() => this.openBotSession(botId, ref.sessionId));
    this.sendToWindows(CH_BOTS_SESSION_SHOWN, shown);
  }

  private async restoreProjectSessionInternal(projectPath: string, activeSession?: SessionRef): Promise<SessionRef> {
    const project = this.requireProject(projectPath);
    if (activeSession?.projectPath === project) {
      await this.openSessionAndActivateInternal(activeSession);
      const active = this.settings.getPublic().activeSession;
      if (!active || active.sessionId !== activeSession.sessionId) throw new Error('restored session is no longer active');
      return active;
    }
    const catalog = await this.sessionCatalog.list(project, 'current', (session) =>
      !this.scheduled?.isControllerSession(session.workspace_path ?? project, session.uuid)
      && !this.settings.isSessionArchived({ projectPath: project, sessionId: session.uuid })
      && !this.settings.isSessionArchived({ projectPath: session.workspace_path ?? project, sessionId: session.uuid }),
    (session) => {
      const owner = this.settings.conversationSpaces.findBySessionId(session.uuid);
      return JSON.stringify([session.mode, owner?.projectId]);
    });
    const unknownCount = catalog.sessions.filter((session) => !this.settings.conversationSpaces.findBySessionId(session.uuid)).length;
    if (unknownCount) this.diagnostics.add('warn', 'host', `Ignored ${unknownCount} session(s) without an authenticated conversation scope.`);
    catalog.sessions = catalog.sessions.filter((session) => this.settings.conversationSpaces.findBySessionId(session.uuid)?.mode === session.mode
      && !this.scheduled?.isControllerSession(session.workspace_path ?? project, session.uuid)
      && !this.settings.isSessionArchived({ projectPath: project, sessionId: session.uuid })
      && !this.settings.isSessionArchived({ projectPath: session.workspace_path ?? project, sessionId: session.uuid }));
    this.catalogs.set(project, {
      sessions: catalog.sessions.map(({
        empty_session: _emptySession,
        resume_model: _resumeModel,
        workspace_path: _workspacePath,
        ...session
      }) => session),
    });
    const mode = this.settings.conversationSpaces.snapshot().activeMode;
    const first = catalog.sessions.find((session) => {
      const owner = this.settings.conversationSpaces.findBySessionId(session.uuid);
      return session.mode === mode && owner?.mode === mode && owner.projectId === project;
    });
    const ref = first
      ? { projectPath: project, sessionId: first.uuid }
      : await this.bridge.newSession(project, undefined, { mode, projectId: project });
    if (first) {
      await this.openSessionAndActivateInternal(ref, { scope: 'current', transcriptPath: first.path });
      const active = this.settings.getPublic().activeSession;
      if (!active || active.sessionId !== ref.sessionId) throw new Error('restored session is no longer active');
      this.terminals?.migrateScope({ projectPath: project, sessionId: TERMINAL_DRAFT_SESSION }, active);
      return active;
    }
    const owned: OwnedConversation = { ...ref, mode, projectId: project };
    this.settings.conversationSpaces.register(owned);
    this.draftSelections.set(mode, owned);
    this.settings.activateProject(project);
    this.settings.setActiveSessionDraft(owned);
    this.terminals?.migrateScope({ projectPath: project, sessionId: TERMINAL_DRAFT_SESSION }, ref);
    return owned;
  }

  private async persistSessionTitle(ref: SessionRef, customTitle: string): Promise<ProjectSessionCatalogState> {
    const selection = await this.prepareMetadataSource(ref);
    const captured = await this.metadataSource(ref, selection);
    try {
      await captured.apply('rename', customTitle);
      this.sessionCatalog.invalidate(captured.source.ref.projectPath);
      if (captured.source.ref.projectPath !== ref.projectPath) await this.refreshSessionTitleCatalog(captured.source.ref, customTitle, captured);
      return this.refreshSessionTitleCatalog(ref, customTitle, captured);
    } finally { this.releaseMetadataSource(captured.selection.sourceToken!); }
  }

  private async refreshSessionTitleCatalog(ref: SessionRef, customTitle: string, source: CapturedMetadataSource): Promise<ProjectSessionCatalogState> {
    const previousCatalog = this.catalogs.get(ref.projectPath);
    const catalog = await this.loadProjectSessions(ref.projectPath);
    if (catalog.error && previousCatalog) {
      // The append is already durable. Keep the last usable snapshot so a
      // transient catalog failure cannot turn a successful rename into a
      // retryable error (and a duplicate custom-title record).
      const fallback = {
        sessions: previousCatalog.sessions.map((session) => session.uuid === ref.sessionId
          && session.path === source.selection.transcriptPath && session.mode === source.owner.mode
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
    const owner = this.settings.conversationSpaces.find(active);
    const botWorkspace = this.isBotWorkspace(active.projectPath);
    const mode = owner?.mode ?? (botWorkspace ? runtime?.sessionMode : undefined);
    if (!mode) throw new Error('The session mode is unavailable.');
    const generation = this.navigationGeneration;
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
    const current = () => generation === this.navigationGeneration && (!owner
      || this.settings.conversationSpaces.snapshot().activeMode === owner.mode);
    const replacement = await this.bridge.newSession(active.projectPath, undefined,
      { mode, projectId: owner?.projectId ?? null, activate: current });
    if (owner) {
      const draft: OwnedConversation = { ...replacement, mode: owner.mode, projectId: owner.projectId };
      this.settings.conversationSpaces.register(draft);
      if (current()) {
        this.draftSelections.set(owner.mode, draft);
        this.settings.setActiveSessionDraft(draft);
      }
    } else if (current() && botWorkspace) this.settings.setActiveBotSession(replacement);
    this.workspaceFiles.invalidate();
  }

  dispose(): void {
    this.metadataLifecycleEpoch++;
    for (const token of this.metadataSources.keys()) this.releaseMetadataSource(token);
    this.realtimeAudioStartToken = undefined;
    for (const lease of this.audioFlowLeases.values()) lease.release();
    this.audioFlowLeases.clear();
    for (const lease of this.detachedAudioLeases) lease.release();
    this.detachedAudioLeases.clear();
    void this.realtimeAudioController?.stop().catch((error) => {
      this.diagnostics.add('warn', 'host', `realtime audio teardown failed: ${sanitizeDiagnostic(error)}`);
    });
    this.scheduled?.dispose();
    for (const handoff of this.handoffs.values()) handoff.abort();
    this.handoffs.clear();
    for (const id of [...this.handoffRetries.keys()]) this.discardHandoff(id);
    this.offAccount?.();
    this.account?.dispose();
    this.ipc.removeHandler(CH_ACCOUNT);
    this.ipc.removeHandler(CH_SCHEDULED);
    for (const channel of this.remoteControlChannels) this.ipc.removeHandler(channel);
    this.remoteControlChannels = [];
    this.remoteControlUi?.dispose();
    this.remoteControlUi = undefined;
    for (const channel of this.botsChannels) this.ipc.removeHandler(channel);
    this.botsChannels = [];
    this.botsUi = undefined;
    this.providerAccounts?.cancel();
    this.offTerminals?.();
    this.offGit?.();
    for (const target of this.gitWatches.keys()) this.detachGit(target);
    for (const delivery of this.terminalDeliveries.values()) delivery.dispose();
    this.terminalDeliveries.clear();
    if (!this.registered) return;
    for (const channel of [
      CH_APPLICATION_ICON, CH_IMAGE_SAVE,
      CH_GIT_REQUEST, CH_TERMINAL_REQUEST, CH_BOOTSTRAP, CH_SETTINGS_GET, CH_SETTINGS_UPDATE, CH_SETTINGS_FILE_OPEN, CH_WORKSPACE_PICK, CH_WORKSPACE_SET,
      CH_PROJECT_FOLDER_PICK, CH_PROJECT_APPS_DEFAULT, CH_PROJECT_CREATE,
      CH_PROJECT_REMOVE, CH_SESSION_PIN_SET, CH_CONVERSATION_SPACE, CH_SESSION_HANDOFF, CH_SESSION_HANDOFF_CANCEL,
      CH_PROJECT_SESSIONS_LIST, CH_SESSION_NEW, CH_SESSION_OPEN, CH_SESSION_CLEAR, CH_SESSION_ARCHIVE, CH_SESSION_ARCHIVE_PREFLIGHT,
      CH_SESSION_METADATA_CAPTURE, CH_SESSION_METADATA_RELEASE, CH_SESSION_TOUCH, CH_SESSION_RENAME,
      CH_SESSION_WORKSPACE, CH_NATIVE_AUDIO_FLOW_LEASE,
      CH_WORKSPACE_FILES_SEARCH, CH_COMPUTER_SETTINGS,
      CH_PROVIDER_ACCOUNTS, CH_PROVIDER_ACCOUNTS_CANCEL, 'lingxi:provider-accounts:routing:get', 'lingxi:provider-settings',
      CH_PLUGIN_SECRET_GET, CH_PLUGIN_SECRET_SET, CH_PLUGIN_SECRET_CLEAR, CH_PLUGIN_SECRET_CLEANUP,
      CH_BRIDGE_RESTART, CH_DIAGNOSTICS_GET, CH_MICROPHONE_ACCESS_GET,
      CH_DIAGNOSTICS_COPY, CH_DIAGNOSTICS_EXPORT, CH_CLIPBOARD_WRITE_TEXT, CH_INTERFACE_LOCALE, CH_OPEN_SYSTEM_SETTINGS,
      CH_REALTIME_AUDIO_COMMAND, CH_NATIVE_AUDIO_REQUEST, CH_NATIVE_AUDIO_OPERATION, CH_NATIVE_AUDIO_CANCEL, CH_NATIVE_AUDIO_FINISH_LISTEN,
    ]) this.ipc.removeHandler(channel);
    this.offNativeAudio?.();
    this.offNativeAudio = undefined;
    this.registered = false;
    this.targets.clear();
  }
}
