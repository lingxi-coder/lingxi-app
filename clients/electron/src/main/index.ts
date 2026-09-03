import { app, BrowserWindow, dialog, shell, type Session } from 'electron';
import { join } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { SessionRuntimeManager, type SessionRef } from './bridge.js';
import { createMacCredentialBrokerClient, resolveProviderCredential, resolveSessionLaunchCredentials } from './credential-broker.js';
import { HostController } from './host.js';
import { DiagnosticBuffer, sanitizeDiagnostic } from './host-utils.js';
import { SettingsStore } from './settings.js';
import { ProjectSessionCatalog } from './session-catalog.js';
import { ignoreBrokenPipe } from './process-streams.js';
import { PROVIDER_IDS } from '../shared/providers.js';

ignoreBrokenPipe(process.stdout);
ignoreBrokenPipe(process.stderr);

const moduleDirectory = dirname(fileURLToPath(import.meta.url));
const securedSessions = new WeakSet<Session>();
let bridge: SessionRuntimeManager | null = null;
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
  const credentialBroker = createMacCredentialBrokerClient({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
  });
  diagnostics.add('info', 'host', credentialBroker
    ? 'credential store: macOS credential broker'
    : 'credential store: shared engine secure storage');
  const settings = new SettingsStore(userData);
  const sessionCatalog = new ProjectSessionCatalog({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    serverBin: app.isPackaged ? undefined : process.env['LINGXI_BRIDGE_SERVER_BIN'],
  });
  bridge = new SessionRuntimeManager({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    bridgeRoot: join(app.getPath('userData'), 'bridge-runtime'),
    // Runtime assembly includes live MCP discovery before publishing the
    // bridge lockfile. A healthy project can exceed the generic 15s default
    // on a cold network; keep the test-injected short timeout untouched while
    // giving the real Desktop launch enough room to finish.
    lockfileTimeoutMs: 30_000,
    providerIds: PROVIDER_IDS,
    diagnostics,
    accessState: (ref: SessionRef) => {
      try {
        // Adding a Project is the Desktop trust decision. Repository edits must
        // not silently revoke a live session or reintroduce the removed setup
        // prompt; removal from the Project list revokes access instead.
        return { workspace: ref.projectPath, trusted: settings.hasProject(ref.projectPath) };
      } catch {
        return { workspace: ref.projectPath, trusted: false };
      }
    },
    onModelChanged: (_ref, model) => { settings.update({ model }); },
    resolveProviderCredential: (providerId) => resolveProviderCredential(providerId, { credentialBroker }),
    onFirstPromptSent: (ref) => { settings.setActiveSession(ref); },
    sessionIdAvailable: async (ref) => {
      const catalog = await sessionCatalog.list(ref.projectPath);
      return !catalog.sessions.some((session) => session.uuid === ref.sessionId);
    },
    confirmBypassPermissions: async () => {
      // Shown ONCE per install (persisted), mirroring the oracle's
      // `bypassPermissionsModeAccepted`. Body text is the oracle's Bypass
      // Permissions acceptance copy.
      if (settings.getBypassPermissionsAccepted()) return true;
      const options = {
        type: 'warning' as const,
        buttons: ['Cancel', 'Yes, I accept'],
        defaultId: 0,
        cancelId: 0,
        noLink: true,
        title: 'Bypass Permissions mode',
        message:
          'In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands.',
        detail:
          'This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.\n\n' +
          'By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.\n\n' +
          'https://code.claude.com/docs/en/security',
      };
      const parent = BrowserWindow.getAllWindows()[0];
      const confirmation = parent
        ? await dialog.showMessageBox(parent, options)
        : await dialog.showMessageBox(options);
      if (confirmation.response !== 1) return false;
      settings.setBypassPermissionsAccepted(true);
      return true;
    },
    launchConfig: async (ref: SessionRef) => {
      const workspace = ref.projectPath;
      const configured = settings.getPublic();
      const credentials = await resolveSessionLaunchCredentials(configured.model, {
        credentialBroker,
      });
      return {
        workspace,
        sessionId: ref.sessionId,
        trusted: settings.hasProject(workspace),
        ...credentials,
        model: configured.model,
        apiBaseUrl: configured.apiBaseUrl,
      };
    },
  });
  host = new HostController(
    settings,
    bridge,
    diagnostics,
    sessionCatalog,
    undefined,
    undefined,
    credentialBroker,
  );
  host.registerIpc();
  createWindow();

  const publicSettings = settings.getPublic();
  const activeProject = publicSettings.activeProject;
  if (activeProject) {
    void host.restoreProjectSession(activeProject, publicSettings.activeSession).catch((error: unknown) => {
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
