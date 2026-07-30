import { createRequire } from 'node:module';
import type { IpcMainInvokeEvent, WebContents } from 'electron';
import { writeFileSync } from 'node:fs';

import type { AskUserQuestionRequestDto } from '@lingxi/bridge-client';
import type { BridgeManager, ConnectionState } from './bridge.js';
import { WorkspaceFileSearch } from './file-search.js';
import { canonicalWorkspace, DiagnosticBuffer, sanitizeDiagnostic, type DiagnosticEntry, type PublicSettings } from './host-utils.js';
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
export const CH_WORKSPACE_FILES_SEARCH = 'lingxi:workspace-files:search';
export const CH_TRUST_SET = 'lingxi:trust:set';
export const CH_PROVIDER_CREDENTIALS_GET = 'lingxi:provider-credentials:get';
export const CH_PROVIDER_CREDENTIAL_SET = 'lingxi:provider-credential:set';
export const CH_PROVIDER_CREDENTIAL_CLEAR = 'lingxi:provider-credential:clear';
export const CH_BRIDGE_RESTART = 'lingxi:bridge:restart';
export const CH_DIAGNOSTICS_GET = 'lingxi:diagnostics:get';
export const CH_DIAGNOSTICS_COPY = 'lingxi:diagnostics:copy';
export const CH_DIAGNOSTICS_EXPORT = 'lingxi:diagnostics:export';
export const CH_OPEN_SYSTEM_SETTINGS = 'lingxi:openSystemSettings';

/**
 * The only two macOS System Settings deep links the `computer` tool's TCC
 * panel ever opens (Accessibility / Screen Recording). A fixed allowlist, not
 * a renderer-supplied URL — `shell.openExternal` must never be handed an
 * arbitrary string from the renderer.
 */
const SYSTEM_SETTINGS_PANES = {
  accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
  screen_recording: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
} as const;
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
  settings: PublicSettings;
  workspace: WorkspaceMetadata;
  providerCredentials?: ProviderCredentialMetadata[];
  pendingAskUserQuestions?: AskUserQuestionRequestDto[];
  connection: ConnectionState;
  diagnostics: DiagnosticEntry[];
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
      ? `stored workspace is unavailable: ${workspace}`
      : sanitizeDiagnostic(error),
  };
}

export class HostController {
  private registered = false;
  private readonly targets = new Map<WebContents, Set<string>>();
  private readonly workspaceFiles = new WorkspaceFileSearch();

  constructor(
    private readonly settings: SettingsStore,
    private readonly bridge: BridgeManager,
    private readonly diagnostics: DiagnosticBuffer,
  ) {}

  registerWindow(webContents: WebContents, rendererUrl: string): void {
    const allowedOrigin = origin(rendererUrl);
    if (!allowedOrigin) throw new Error('invalid renderer URL');
    const origins = this.targets.get(webContents) ?? new Set<string>();
    origins.add(allowedOrigin);
    this.targets.set(webContents, origins);
    webContents.once('destroyed', () => this.targets.delete(webContents));
  }

  registerIpc(): void {
    if (this.registered) return;
    this.registered = true;
    ipcMain.handle(CH_BOOTSTRAP, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.bootstrap(); });
    ipcMain.handle(CH_SETTINGS_GET, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.settings.getPublic(); });
    ipcMain.handle(CH_SETTINGS_UPDATE, async (event: IpcMainInvokeEvent, patch: unknown) => {
      this.assertSender(event);
      if (!patch || typeof patch !== 'object' || Array.isArray(patch)) throw new Error('invalid settings patch');
      const keys = Object.keys(patch);
      if (keys.some((key) => key !== 'theme' && key !== 'model' && key !== 'apiBaseUrl')) throw new Error('unsupported setting');
      const restartsBridge = 'model' in patch || 'apiBaseUrl' in patch;
      if (restartsBridge) this.assertNoActiveTurn();
      const result = this.settings.update(patch as { theme?: 'dark' | 'light'; model?: string | null; apiBaseUrl?: string | null });
      if (restartsBridge) await this.restartIfConfigured();
      return result;
    });
    ipcMain.handle(CH_WORKSPACE_PICK, async (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      const result = await dialog.showOpenDialog({ properties: ['openDirectory', 'createDirectory'], securityScopedBookmarks: false });
      if (result.canceled || !result.filePaths[0]) return null;
      return this.selectWorkspace(result.filePaths[0]);
    });
    ipcMain.handle(CH_WORKSPACE_SET, async (event: IpcMainInvokeEvent, workspace: unknown) => {
      this.assertSender(event);
      if (typeof workspace !== 'string') throw new Error('invalid workspace path');
      const canonical = canonicalWorkspace(workspace);
      if (!this.settings.isRecentWorkspace(canonical)) throw new Error('workspace is not in the recent list');
      return this.selectWorkspace(canonical);
    });
    ipcMain.handle(CH_WORKSPACE_FILES_SEARCH, async (event: IpcMainInvokeEvent, query: unknown) => {
      this.assertSender(event);
      const workspace = this.requireWorkspace();
      if (!this.settings.getTrust(workspace).trusted) throw new Error('workspace trust is required before searching files');
      return this.workspaceFiles.search(workspace, query);
    });
    ipcMain.handle(CH_TRUST_SET, async (event: IpcMainInvokeEvent, trusted: unknown) => {
      this.assertSender(event);
      if (typeof trusted !== 'boolean') throw new Error('invalid trust value');
      this.assertNoActiveTurn();
      const workspace = this.requireWorkspace();
      if (trusted) {
        const confirmation = await dialog.showMessageBox({
          type: 'warning',
          buttons: ['Cancel', 'Trust workspace'],
          defaultId: 0,
          cancelId: 0,
          noLink: true,
          title: 'Trust this workspace?',
          message: 'Project settings can run hooks, tools, and MCP servers.',
          detail: `Only continue if you trust the contents of:\n${workspace}`,
        });
        if (confirmation.response !== 1) return { path: workspace, ...this.settings.getTrust(workspace) };
      }
      const result = this.settings.setTrust(workspace, trusted);
      this.workspaceFiles.invalidate();
      await this.restartIfConfigured();
      return { path: workspace, ...result };
    });
    ipcMain.handle(CH_PROVIDER_CREDENTIALS_GET, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      return this.providerCredentialSnapshot();
    });
    ipcMain.handle(CH_PROVIDER_CREDENTIAL_SET, async (event: IpcMainInvokeEvent, providerId: unknown, credential: unknown) => {
      this.assertSender(event);
      const provider = this.requireProvider(providerId);
      if (typeof credential !== 'string') throw new Error('invalid credential');
      this.assertNoActiveTurn();
      this.requireWorkspace();
      const stored = await this.bridge.setProviderCredential(provider.id, credential);
      if (!stored.configured_provider_ids.includes(provider.id)) {
        throw new Error(`provider credential was not persisted (${provider.id})`);
      }
      const credentialMetadata: ProviderCredentialMetadata = {
        providerId: provider.id,
        configured: true,
        encryptionAvailable: stored.storage_encrypted,
      };
      if (provider.defaultModel) this.settings.update({ model: provider.defaultModel });
      await this.restartIfConfigured();
      return { credential: credentialMetadata, settings: this.settings.getPublic() };
    });
    ipcMain.handle(CH_PROVIDER_CREDENTIAL_CLEAR, async (event: IpcMainInvokeEvent, providerId: unknown) => {
      this.assertSender(event);
      const provider = this.requireProvider(providerId);
      this.assertNoActiveTurn();
      this.requireWorkspace();
      await this.bridge.deleteProviderCredential(provider.id);
      await this.restartIfConfigured();
      return this.providerCredentialMetadata(provider.id);
    });
    ipcMain.handle(CH_BRIDGE_RESTART, async (event: IpcMainInvokeEvent) => { this.assertSender(event); await this.restartIfConfigured(); });
    ipcMain.handle(CH_DIAGNOSTICS_GET, (event: IpcMainInvokeEvent) => { this.assertSender(event); return this.diagnostics.snapshot(); });
    ipcMain.handle(CH_DIAGNOSTICS_COPY, (event: IpcMainInvokeEvent) => {
      this.assertSender(event);
      clipboard.writeText(this.diagnosticReport());
    });
    ipcMain.handle(CH_DIAGNOSTICS_EXPORT, async (event: IpcMainInvokeEvent) => {
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
    ipcMain.handle(CH_OPEN_SYSTEM_SETTINGS, async (event: IpcMainInvokeEvent, pane: unknown) => {
      this.assertSender(event);
      if (typeof pane !== 'string' || !(pane in SYSTEM_SETTINGS_PANES)) {
        throw new Error('unsupported System Settings pane');
      }
      await shell.openExternal(SYSTEM_SETTINGS_PANES[pane as SystemSettingsPane]);
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
    const workspace = this.settings.getWorkspace();
    if (!workspace) return { trusted: false };
    try {
      return { path: workspace, ...this.settings.getTrust(workspace) };
    } catch (error) {
      this.diagnostics.add('warn', 'host', error);
      return { path: workspace, trusted: false, recovery: workspaceRecovery(workspace, error) };
    }
  }

  private bootstrap(): BootstrapState {
    const pendingAskUserQuestions = this.bridge.pendingAskUserQuestions ?? [];
    return {
      settings: this.settings.getPublic(),
      workspace: this.workspace(),
      providerCredentials: this.providerCredentialSnapshot(),
      ...(pendingAskUserQuestions.length > 0 ? { pendingAskUserQuestions: [...pendingAskUserQuestions] } : {}),
      connection: this.bridge.connectionState,
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
      connection: this.bridge.connectionState,
      bridgeRuntime: this.bridge.runtimeVersions,
      diagnostics: this.diagnostics.snapshot(),
    }, null, 2)}\n`;
  }

  private requireWorkspace(): string {
    const workspace = this.settings.getWorkspace();
    if (!workspace) throw new Error('select a workspace first');
    return workspace;
  }

  private providerCredentialSnapshot(): ProviderCredentialMetadata[] {
    const activeProviders = new Set(
      this.bridge.connectionState.status === 'connected'
        ? this.bridge.activeCredentialProviderIds ?? []
        : [],
    );
    const persistedProviders = new Set(this.bridge.persistedCredentialProviderIds ?? []);
    const engineStorageEncrypted = this.bridge.providerCredentialStorageEncrypted ?? false;
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

  private async selectWorkspace(input: string): Promise<WorkspaceMetadata> {
    this.assertNoActiveTurn();
    const workspace = canonicalWorkspace(input);
    this.workspaceFiles.invalidate();
    this.settings.setWorkspace(workspace);
    await this.restartIfConfigured();
    return { path: workspace, ...this.settings.getTrust(workspace) };
  }

  private assertNoActiveTurn(): void {
    if (this.bridge.turnActive) throw new Error('cancel the active turn before changing engine settings');
  }

  private async restartIfConfigured(): Promise<void> {
    if (!this.settings.getWorkspace()) return;
    await this.bridge.restart();
  }

  dispose(): void {
    if (!this.registered) return;
    for (const channel of [
      CH_BOOTSTRAP, CH_SETTINGS_GET, CH_SETTINGS_UPDATE, CH_WORKSPACE_PICK, CH_WORKSPACE_SET,
      CH_WORKSPACE_FILES_SEARCH,
      CH_TRUST_SET,
      CH_PROVIDER_CREDENTIALS_GET, CH_PROVIDER_CREDENTIAL_SET, CH_PROVIDER_CREDENTIAL_CLEAR,
      CH_BRIDGE_RESTART, CH_DIAGNOSTICS_GET,
      CH_DIAGNOSTICS_COPY, CH_DIAGNOSTICS_EXPORT, CH_OPEN_SYSTEM_SETTINGS,
    ]) ipcMain.removeHandler(channel);
    this.registered = false;
    this.targets.clear();
  }
}
