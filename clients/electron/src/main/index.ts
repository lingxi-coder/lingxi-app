import { app, BrowserWindow, safeStorage, shell, type Session } from 'electron';
import { join } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { BridgeManager } from './bridge.js';
import { HostController } from './host.js';
import { DiagnosticBuffer, sanitizeDiagnostic } from './host-utils.js';
import { SettingsStore } from './settings.js';
import { MacKeychainCredentialStore } from './keychain.js';
import { PROVIDER_IDS } from '../shared/providers.js';

const moduleDirectory = dirname(fileURLToPath(import.meta.url));
const securedSessions = new WeakSet<Session>();
let bridge: BridgeManager | null = null;
let host: HostController | null = null;
let quitting = false;

function developmentRendererUrl(): string | undefined {
  if (app.isPackaged) return undefined;
  const raw = process.env['ELECTRON_RENDERER_URL'];
  if (!raw) return undefined;
  try {
    const url = new URL(raw);
    const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '::1';
    if ((url.protocol === 'http:' || url.protocol === 'https:') && loopback) return url.toString();
  } catch {
    // Invalid development URL falls back to the bundled renderer.
  }
  return undefined;
}

function isLocalRendererUrl(raw: string): boolean {
  try {
    const url = new URL(raw);
    return url.protocol === 'file:'
      || ((url.protocol === 'http:' || url.protocol === 'https:')
        && (url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '::1'));
  } catch {
    return false;
  }
}

function secureSession(session: Session): void {
  if (securedSessions.has(session)) return;
  securedSessions.add(session);
  // The composer can use the browser's speech recognition API, but no other
  // permission is needed by the desktop app. Keep the allowlist scoped to the
  // local renderer so a future navigation cannot inherit microphone access.
  session.setPermissionCheckHandler((_webContents, permission, requestingOrigin) => (
    permission === 'media' && isLocalRendererUrl(requestingOrigin)
  ));
  session.setPermissionRequestHandler((webContents, permission, callback) => callback(
    permission === 'media' && isLocalRendererUrl(webContents.getURL())
  ));
  session.on('will-download', (event) => event.preventDefault());
}

function rendererTarget(): { url: string; load: (window: BrowserWindow) => Promise<void> } {
  const devUrl = developmentRendererUrl();
  if (devUrl) return { url: devUrl, load: (window) => window.loadURL(devUrl) };
  const file = join(moduleDirectory, '../renderer/index.html');
  return { url: pathToFileURL(file).toString(), load: (window) => window.loadFile(file) };
}

function isHttpsUrl(raw: string): boolean {
  try {
    const url = new URL(raw);
    return url.protocol === 'https:' && !url.username && !url.password;
  } catch { return false; }
}

const providerEnvironmentVariables: Readonly<Record<string, string>> = {
  anthropic: 'ANTHROPIC_API_KEY',
  openai: 'OPENAI_API_KEY',
  deepseek: 'DEEPSEEK_API_KEY',
  gemini: 'GEMINI_API_KEY',
  openrouter: 'OPENROUTER_API_KEY',
  zai: 'ZAI_API_KEY',
  'glm-coding': 'GLM_API_KEY',
  'github-copilot': 'GITHUB_TOKEN',
};

/** Developer opt-in fallback for a locked/missing Keychain item. */
function readEnvironmentCredential(providerId: string): string | undefined {
  const variable = providerEnvironmentVariables[providerId];
  const value = variable ? process.env[variable] : undefined;
  return typeof value === 'string' && value.length > 0 && value.length <= 16_384 ? value : undefined;
}

function createWindow(): BrowserWindow {
  const target = rendererTarget();
  const mainWindow = new BrowserWindow({
    width: 1320,
    height: 860,
    minWidth: 900,
    minHeight: 600,
    show: false,
    backgroundColor: '#0c0b10',
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: -100, y: -100 },
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(moduleDirectory, '../preload/index.cjs'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      webviewTag: false,
      navigateOnDragDrop: false,
      webSecurity: true,
      devTools: !app.isPackaged,
    },
  });

  secureSession(mainWindow.webContents.session);
  bridge?.registerWindow(mainWindow.webContents, target.url);
  host?.registerWindow(mainWindow.webContents, target.url);

  mainWindow.once('ready-to-show', () => mainWindow.show());
  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    if (isHttpsUrl(url)) void shell.openExternal(url);
    return { action: 'deny' };
  });
  mainWindow.webContents.on('will-attach-webview', (event) => event.preventDefault());
  mainWindow.webContents.on('will-navigate', (event, url) => {
    const expected = new URL(target.url);
    const proposed = new URL(url);
    const sameDocumentOrigin = expected.protocol === 'file:'
      ? proposed.protocol === 'file:' && proposed.pathname === expected.pathname
      : proposed.origin === expected.origin;
    if (!sameDocumentOrigin) event.preventDefault();
  });

  void target.load(mainWindow);
  return mainWindow;
}

const hasSingleInstanceLock = app.requestSingleInstanceLock();
if (!hasSingleInstanceLock) app.quit();

if (hasSingleInstanceLock) void app.whenReady().then(() => {
  const userData = app.getPath('userData');
  const diagnostics = new DiagnosticBuffer(join(userData, 'logs', 'desktop.jsonl'));
  diagnostics.add('info', 'host', `desktop start: app=${app.getVersion()} electron=${process.versions.electron} platform=${process.platform} arch=${process.arch}`);
  diagnostics.add('info', 'host', 'credential store: shared engine secure storage');
  // SettingsStore remains a read-only migration source for pre-unification
  // Electron ciphertext and generic-password items. New writes go through the
  // bridge into the Rust CredentialManager shared with CLI/TUI.
  const settings = new SettingsStore(userData, safeStorage, new MacKeychainCredentialStore());
  bridge = new BridgeManager({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    bridgeRoot: join(app.getPath('userData'), 'bridge-runtime'),
    providerIds: PROVIDER_IDS,
    diagnostics,
    accessState: () => {
      const workspace = settings.getWorkspace();
      return workspace
        ? { workspace, trusted: settings.getTrust(workspace).trusted }
        : { trusted: false };
    },
    onModelChanged: (model) => { settings.update({ model }); },
    onProviderCredentialMigrated: (providerId) => {
      settings.clearProviderCredential(providerId);
    },
    launchConfig: async () => {
      const workspace = settings.getWorkspace();
      if (!workspace) throw new Error('select a workspace before starting the bridge');
      const configured = settings.getPublic();
      const providerIds = PROVIDER_IDS;
      const legacyCredentials = settings.readProviderCredentials(providerIds);
      const credentials = { ...legacyCredentials };
      for (const providerId of providerIds) {
        if (credentials[providerId] === undefined) {
          const environmentCredential = readEnvironmentCredential(providerId);
          if (environmentCredential !== undefined) credentials[providerId] = environmentCredential;
        }
      }
      const unavailable = settings
        .providerCredentialMetadataFor(providerIds)
        .filter((entry) => entry.configured && credentials[entry.providerId] === undefined)
        .map((entry) => entry.providerId);
      if (unavailable.length > 0) {
        diagnostics.add(
          'warn',
          'host',
          `legacy provider credential is unreadable and will be ignored (${unavailable.join(', ')})`,
        );
      }
      return {
        workspace,
        trusted: settings.getTrust(workspace).trusted,
        apiKey: credentials['anthropic'],
        providerCredentials: Object.fromEntries(
          Object.entries(credentials).filter(([providerId]) => providerId !== 'anthropic'),
        ),
        providerCredentialsToMigrate: legacyCredentials,
        model: configured.model,
        apiBaseUrl: configured.apiBaseUrl,
      };
    },
  });
  host = new HostController(settings, bridge, diagnostics);
  host.registerIpc();
  createWindow();

  if (settings.getWorkspace()) {
    void bridge.start().catch((error: unknown) => {
      diagnostics.add('error', 'host', sanitizeDiagnostic(error));
    });
  }

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
  });

  app.on('second-instance', () => {
  const window = BrowserWindow.getAllWindows()[0];
  if (!window) return;
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
  });

  app.on('window-all-closed', () => {
  // On macOS the app remains alive and the bridge stays available for reopen.
  if (process.platform !== 'darwin') app.quit();
  });

  app.on('before-quit', (event) => {
  if (quitting) return;
  event.preventDefault();
  quitting = true;
  host?.dispose();
  host = null;
  const currentBridge = bridge;
  bridge = null;
  void (currentBridge?.dispose() ?? Promise.resolve()).finally(() => app.quit());
  });
