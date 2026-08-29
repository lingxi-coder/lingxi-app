import { createRequire } from 'node:module';
import type { IpcMainInvokeEvent, WebContents } from 'electron';
import { writeFileSync } from 'node:fs';

import type { AskUserQuestionRequestDto, SessionRowDto } from '@lingxi/bridge-client';
import type {
  BridgeRuntimeVersions,
  ConnectionState,
  RuntimeEventEnvelope,
  SessionRef,
  SessionRuntimeManager,
  SessionRuntimeSummary,
} from './bridge.js';
import { isSessionId } from './bridge.js';
import { WorkspaceFileSearch } from './file-search.js';
import { ProjectSessionCatalog, type ProjectSessionCatalogRow } from './session-catalog.js';
import {
  canonicalWorkspace,
  DiagnosticBuffer,
  sanitizeDiagnostic,
  type DiagnosticEntry,
  type PinnedSessionRecord,
  type PublicSettings,
} from './host-utils.js';
import { validateClipboardText } from './validation.js';
import { readMicrophoneAccess, type MediaAccessReader } from './microphoneAccess.js';
import { PROVIDER_IDS, providerById } from '../shared/providers.js';
import type { SettingsStore } from './settings.js';

export interface CredentialMetadata {
  configured: boolean;
  encryptionAvailable: boolean;
  /** The running engine received a credential from an external runtime source. */
  runtimeOnly?: true;
}

export interface ProviderCredentialMetadata extends CredentialMetadata {
  providerId: string;
}

export const CH_BOOTSTRAP = 'lingxi:bootstrap';
export const CH_SETTINGS_GET = 'lingxi:settings:get';
export const CH_SETTINGS_UPDATE = 'lingxi:settings:update';
export const CH_WORKSPACE_PICK = 'lingxi:workspace:pick';
export const CH_WORKSPACE_SET = 'lingxi:workspace:set';
export const CH_PROJECT_REMOVE = 'lingxi:project:remove';
export const CH_SESSION_PIN_SET = 'lingxi:session-pin:set';
export const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
export const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
export const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
export const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
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
  private registered = false;
  private navigationQueue: Promise<void> = Promise.resolve();
  private bootstrapRevision = 0;
  private readonly closingProjects = new Set<string>();
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly workspaceFiles = new WorkspaceFileSearch();
  private readonly sessionCatalog: ProjectSessionCatalog;
  private readonly catalogs = new Map<string, ProjectSessionCatalogState>();
  private readonly catalogRequestGenerations = new Map<string, number>();

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
  ) {
    this.sessionCatalog = sessionCatalog ?? new ProjectSessionCatalog();
  }

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const allowedOrigin = origin(rendererUrl);
    if (!allowedOrigin) throw new Error('invalid renderer URL');
    const origins = this.targets.get(webContents) ?? new Set<string>();
    origins.add(allowedOrigin);
    this.targets.set(webContents, origins);
    this.bridge.registerWindow(webContents, rendererUrl);
    webContents.once('destroyed', () => this.targets.delete(webContents));
  }

  registerIpc(): void {
    if (this.registered) return;
    this.registered = true;
    this.bridge.registerIpc();
    this.ipc.handle(CH_BOOTSTRAP, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.bootstrap(); });
    this.ipc.handle(CH_SETTINGS_GET, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.settings.getPublic(); });
    this.ipc.handle(CH_SETTINGS_UPDATE, async (event: IpcMainInvokeEvent, patch: unknown) => {
      this.assertSender(event);
      if (!patch || typeof patch !== 'object' || Array.isArray(patch)) throw new Error('invalid settings patch');
      const keys = Object.keys(patch);
      if (keys.some((key) => key !== 'theme' && key !== 'model' && key !== 'apiBaseUrl' && key !== 'voice')) throw new Error('unsupported setting');
      const restartsBridge = 'model' in patch || 'apiBaseUrl' in patch;
      if (restartsBridge) this.assertNoActiveTurn();
      // `voice` never restarts the bridge: recognition/synthesis read
      // `bootstrap.settings.voice` fresh on every audio request
      // (`renderer/audio/requests.ts`'s `playback()`), so a write here takes
      // effect on the NEXT request with no engine restart needed — unlike
      // `model`/`apiBaseUrl`, which change what the running engine talks to.
      const result = this.settings.update(patch as { theme?: 'dark' | 'light' | 'system'; model?: string | null; apiBaseUrl?: string | null; voice?: unknown });
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
      return this.enqueueNavigation(async () => {
        const project = this.requireProject(projectPath);
        if (model !== undefined && typeof model !== 'string') throw new Error('invalid model');
        const ref = await this.bridge.newSession(project, model as string | undefined);
        this.settings.activateProject(project);
        this.settings.setActiveSession(ref);
        return this.bootstrap();
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
      if (typeof projectPath !== 'string' || !this.settings.hasProject(projectPath)) {
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
    this.ipc.handle(CH_PROVIDER_CREDENTIALS_GET, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.providerCredentialSnapshot();
    });
    this.ipc.handle(CH_PROVIDER_CREDENTIAL_SET, async (event: IpcMainInvokeEvent, providerId: unknown, credential: unknown) => {
      this.assertSender(event);
      const provider = this.requireProvider(providerId);
      if (typeof credential !== 'string') throw new Error('invalid credential');
      this.assertNoActiveTurn();
      this.requireWorkspace();
      const runtime = this.requireCurrentRuntime();
      const stored = await runtime.setProviderCredential(provider.id, credential);
      if (!stored.configured_provider_ids.includes(provider.id)) {
        throw new Error(`provider credential was not persisted (${provider.id})`);
      }
      const credentialMetadata: ProviderCredentialMetadata = {
        providerId: provider.id,
        configured: true,
        encryptionAvailable: stored.storage_encrypted,
      };
      if (provider.defaultModel) this.updateProviderDefaultModel(provider.defaultModel);
      // Persistence is the boundary of this IPC operation. Restart is owned by
      // the renderer so it can clear the secret before handling recovery.
      return { credential: credentialMetadata, settings: this.settings.getPublic() };
    });
    this.ipc.handle(CH_PROVIDER_CREDENTIAL_CLEAR, async (event: IpcMainInvokeEvent, providerId: unknown) => {
      this.assertSender(event);
      const provider = this.requireProvider(providerId);
      this.assertNoActiveTurn();
      this.requireWorkspace();
      await this.requireCurrentRuntime().deleteProviderCredential(provider.id);
      await this.restartIfConfigured();
      return this.providerCredentialMetadata(provider.id);
    });
    this.ipc.handle(CH_BRIDGE_RESTART, async (event: IpcMainInvokeEvent, sessionId: unknown) => {
      this.assertSender(event);
      if (!isSessionId(sessionId)) throw new Error('invalid session id');
      const runtime = this.bridge.get(sessionId);
      if (!runtime) throw new Error(`session runtime is not open: ${sessionId}`);
      const ref = { projectPath: runtime.projectPath, sessionId } satisfies SessionRef;
      this.assertRestartAllowed(ref, runtime);
      await this.bridge.restart(ref, () => {
        const current = this.bridge.get(sessionId);
        if (!current || current !== runtime) {
          throw new Error(`session runtime is no longer open: ${sessionId}`);
        }
        this.assertRestartAllowed(ref, current);
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
      return { path: workspace, fingerprint: trust.fingerprint, trusted: this.settings.hasProject(workspace) };
    } catch (error) {
      this.diagnostics.add('warn', 'host', error);
      return { path: workspace, trusted: false, recovery: workspaceRecovery(workspace, error) };
    }
  }

  private bootstrap(): BootstrapState {
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
    return {
      revision,
      settings: this.settings.getPublic(),
      workspace: this.workspace(),
      ...(activeSession ? { activeSession: { ...activeSession } } : {}),
      runtimes: this.runtimeSummaries(),
      projectCatalogs: Object.fromEntries([...this.catalogs.entries()].map(([path, state]) => [path, {
        sessions: state.sessions.map((session) => ({ ...session })),
        ...(state.error ? { error: state.error } : {}),
      }])),
      providerCredentials: this.providerCredentialSnapshot(),
      ...(pendingAskUserQuestions.length > 0 ? { pendingAskUserQuestions: [...pendingAskUserQuestions] } : {}),
      connection: activeRuntime?.connectionState ?? legacy.connectionState ?? { status: 'idle' },
      versions: {
        app: app?.getVersion?.() ?? 'unknown',
        electron: process.versions.electron,
        ...(engineVersions ? { engine: engineVersions } : {}),
      },
      diagnostics: this.diagnostics.snapshot(),
    };
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
      providerCredentials: this.providerCredentialSnapshot(),
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
    const project = canonicalWorkspace(value);
    if (!this.settings.hasProject(project)) throw new Error('project is not in the project list');
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
    };
    const activeProviders = new Set(
      (runtime?.connectionState ?? legacy.connectionState)?.status === 'connected'
        ? runtime?.activeCredentialProviderIds ?? legacy.activeCredentialProviderIds ?? []
        : [],
    );
    const persistedProviders = new Set(runtime?.persistedCredentialProviderIds ?? legacy.persistedCredentialProviderIds ?? []);
    const engineStorageEncrypted = runtime?.providerCredentialStorageEncrypted ?? legacy.providerCredentialStorageEncrypted ?? false;
    return PROVIDER_IDS.map((providerId) => {
      if (persistedProviders.has(providerId)) {
        return {
          providerId,
          configured: true,
          encryptionAvailable: engineStorageEncrypted,
        };
      }
      if (activeProviders.has(providerId)) {
        return {
          providerId,
          configured: true,
          encryptionAvailable: false,
          runtimeOnly: true,
        };
      }
      return { providerId, configured: false, encryptionAvailable: engineStorageEncrypted };
    });
  }

  private providerCredentialMetadata(providerId: string): ProviderCredentialMetadata {
    return this.providerCredentialSnapshot().find((metadata) => metadata.providerId === providerId)
      ?? { providerId, configured: false, encryptionAvailable: false };
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
        this.settings.setActiveSession(ref);
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
    return { path: workspace, trusted: this.settings.hasProject(workspace), ...(fingerprint ? { fingerprint } : {}) };
  }

  private assertNoActiveTurn(): void {
    if (this.hasActiveWork()) throw new Error('cancel the active turn before changing engine settings');
  }

  private assertRestartAllowed(ref: SessionRef, runtime: ReturnType<SessionRuntimeManager['get']>): void {
    if (!runtime) throw new Error(`session runtime is not open: ${ref.sessionId}`);
    const active = this.settings.getPublic().activeSession;
    if (!active || active.sessionId !== ref.sessionId || active.projectPath !== ref.projectPath) {
      throw new Error('the requested session is no longer active; credential was saved but the engine was not restarted');
    }
    if (runtime.turnActive || runtime.pendingInteractions > 0 || this.bridge.hasActiveWork(ref.projectPath)) {
      throw new Error('cancel active turns and pending interactions before restarting the engine');
    }
  }

  private async restartIfConfigured(): Promise<void> {
    const ref = this.settings.getPublic().activeSession;
    if (!ref || !this.bridge.get(ref.sessionId)) return;
    await this.bridge.restart(ref);
  }

  private updateProviderDefaultModel(model: string): void {
    try {
      this.settings.update({ model });
    } catch (error) {
      // The credential write is already authoritative. A settings mirror
      // failure is recoverable and must not turn a successful credential write
      // into a renderer-visible persistence failure.
      this.diagnostics.add('error', 'host', `provider credential persisted but default model update failed: ${sanitizeDiagnostic(error)}`);
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
      const state = { sessions: result.sessions.map(({ empty_session: _emptySession, ...session }) => session) };
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
    await this.bridge.openSession(canonical, session?.empty_session === true);
    // Persist navigation only after the engine has emitted a matching
    // session_resumed event. A failed/corrupt resume leaves the visible session
    // and selected Project unchanged.
    this.settings.activateProject(project);
    this.settings.setActiveSession(canonical);
    return this.bootstrap();
  }

  private async removeProjectInternal(project: string): Promise<BootstrapState> {
    if (this.isProjectClosing(project)) throw new Error('project is closing');
    if (this.bridge.hasActiveWork(project)) throw new Error('cancel active turns and pending interactions before removing a project');
    // Set the host-side guard before invoking the manager. The manager then
    // synchronously removes matching runtimes from its routable map before its
    // first await, so a racing prompt cannot enter a disposing runtime.
    this.closingProjects.add(project);
    try {
      await this.bridge.closeProject(project);
      this.settings.removeProject(project);
      this.workspaceFiles.invalidate();
      this.catalogs.delete(project);
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
    this.catalogs.set(project, {
      sessions: catalog.sessions.map(({ empty_session: _emptySession, ...session }) => session),
    });
    const first = catalog.sessions[0];
    const ref = first
      ? { projectPath: project, sessionId: first.uuid }
      : await this.bridge.newSession(project);
    if (first) await this.bridge.openSession(ref, first.empty_session === true);
    this.settings.activateProject(project);
    this.settings.setActiveSession(ref);
    return ref;
  }

  dispose(): void {
    if (!this.registered) return;
    for (const channel of [
      CH_BOOTSTRAP, CH_SETTINGS_GET, CH_SETTINGS_UPDATE, CH_WORKSPACE_PICK, CH_WORKSPACE_SET,
      CH_PROJECT_REMOVE, CH_SESSION_PIN_SET,
      CH_PROJECT_SESSIONS_LIST, CH_SESSION_NEW, CH_SESSION_OPEN,
      CH_WORKSPACE_FILES_SEARCH,
      CH_PROVIDER_CREDENTIALS_GET, CH_PROVIDER_CREDENTIAL_SET, CH_PROVIDER_CREDENTIAL_CLEAR,
      CH_BRIDGE_RESTART, CH_DIAGNOSTICS_GET,
      CH_DIAGNOSTICS_COPY, CH_DIAGNOSTICS_EXPORT, CH_CLIPBOARD_WRITE_TEXT, CH_OPEN_SYSTEM_SETTINGS,
    ]) this.ipc.removeHandler(channel);
    this.registered = false;
    this.targets.clear();
  }
}
