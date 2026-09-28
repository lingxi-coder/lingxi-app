import { ScheduledTaskService } from './scheduled.js';
import { GitService } from './git.js';
import { app, BrowserWindow, dialog, shell, Notification, type Session } from 'electron';
import { join } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { SessionRuntimeManager, stopLegacyOrphanBridges, type SessionRef } from './bridge.js';
import {
  createMacCredentialBrokerClient,
  resolveProviderCredential,
  SessionLaunchCache,
} from './credential-broker.js';
import { CODEX_PROVIDER_ID, parseCodexSession } from './codex-auth.js';
import { NativeAudioManager } from './audio/nativeAudioManager.js';
import { audioConfigurationDefaults } from '../shared/generatedAudioConfiguration.js';
import { TerminalManager } from './terminal.js';
import { HostController } from './host.js';
import { HostNotifier } from './notifications.js';
import { notificationPresentation } from './notificationPresentation.js';
import { DiagnosticBuffer, diagnosticEvent, sanitizeDiagnostic } from './host-utils.js';
import { SettingsStore } from './settings.js';
import { requestMicrophoneAccess } from './microphoneAccess.js';
import { ProjectSessionCatalog } from './session-catalog.js';
import { ignoreBrokenPipe } from './process-streams.js';
import { PROVIDER_IDS } from '../shared/providers.js';

ignoreBrokenPipe(process.stdout);
ignoreBrokenPipe(process.stderr);

const moduleDirectory = dirname(fileURLToPath(import.meta.url));
const securedSessions = new WeakSet<Session>();
let bridge: SessionRuntimeManager | null = null;
let host: HostController | null = null;
let scheduled: ScheduledTaskService | null = null;
let git: GitService | null = null;
let terminals: TerminalManager | null = null;
let nativeAudio: NativeAudioManager | null = null;
let notifier: HostNotifier | null = null;
let quitting = false;

/**
 * Whether the main window currently has OS focus. Net-new state: nothing in
 * this app tracked window focus before, and `HostNotifier` needs it to avoid
 * banners over the window the user is already looking at.
 */
let windowFocused = false;


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
  // Keep legacy renderer media access scoped to the local app origin. The
  // production voice path is the signed native Audio Helper; this remains for
  // compatibility tests and cannot authorize an arbitrary navigation.
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
  mainWindow.webContents.on('did-finish-load', () => {
    // Permission requests travel on their own IPC channel and are not part of
    // the sequenced event replay. Re-deliver any still-pending request after a
    // renderer reload so a parked engine turn never loses its only UI owner.
    host?.replayPendingInteractions(mainWindow.webContents);
  });
  const releaseAudio = (): void => {
    void nativeAudio?.suspend('window hidden').catch(() => undefined);
  };
  mainWindow.on('hide', releaseAudio);
  mainWindow.on('minimize', releaseAudio);
  mainWindow.on('closed', releaseAudio);
  // Taking focus is the strongest available proof the user came back, which is
  // exactly what `idle_prompt` re-checks before firing.
  mainWindow.on('focus', () => { windowFocused = true; notifier?.noteInteraction(); });
  mainWindow.on('blur', () => { windowFocused = false; });
  mainWindow.on('hide', () => { windowFocused = false; });
  mainWindow.on('minimize', () => { windowFocused = false; });
  mainWindow.on('closed', () => { windowFocused = false; });

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

if (hasSingleInstanceLock) void app.whenReady().then(async () => {
  const userData = app.getPath('userData');
  const diagnostics = new DiagnosticBuffer(join(userData, 'logs', 'desktop.jsonl'));
  diagnostics.add('info', 'host', `desktop start: app=${app.getVersion()} electron=${process.versions.electron} platform=${process.platform} arch=${process.arch}`);
  const bridgeRoot = join(userData, 'bridge-runtime');
  if (process.platform === 'darwin' && app.isPackaged) {
    const stoppedLegacyPids = await stopLegacyOrphanBridges(bridgeRoot);
    if (stoppedLegacyPids.length > 0) {
      diagnostics.add('warn', 'host', `stopped legacy orphan bridge processes: ${stoppedLegacyPids.join(', ')}`);
    }
  }
  const credentialBroker = createMacCredentialBrokerClient({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
  });
  diagnostics.add('info', 'host', credentialBroker
    ? 'credential store: macOS credential broker'
    : 'credential store: shared engine secure storage');
  const settings = new SettingsStore(userData);
  nativeAudio = process.platform === 'darwin'
    ? new NativeAudioManager({
        isPackaged: app.isPackaged,
        resourcesPath: process.resourcesPath,
        userDataPath: userData,
        diagnostics,
        requestMicrophoneAccess: () => requestMicrophoneAccess(),
        getAudioConfiguration: () => {
          const error = settings.getAudioConfigurationError();
          if (error) throw new Error(error);
          return settings.getPublic().voice ?? audioConfigurationDefaults();
        },
        getAudioConfigurationRevision: () => settings.getPublic().voiceRevision ?? 0,
        isForeground: () => windowFocused,
      })
    : null;
  const sessionCatalog = new ProjectSessionCatalog({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    serverBin: app.isPackaged ? undefined : process.env['LINGXI_BRIDGE_SERVER_BIN'],
  });
  const launchCache = new SessionLaunchCache();
  /**
   * The one place an OS notification is raised. Shared by the scheduled-task
   * service and `HostNotifier` so both get the same click behaviour: restore
   * the session the notification came from, then raise the window.
   */
  const showNotification = (title: string, body: string, ref?: SessionRef): void => {
    if (!Notification.isSupported()) return;
    const notification = new Notification(notificationPresentation(title, body, ref));
    notification.on('click', () => {
      if (ref) void host?.restoreProjectSession(ref.projectPath, ref).catch((error) => diagnostics.add('error', 'host', sanitizeDiagnostic(error)));
      const window = BrowserWindow.getAllWindows()[0];
      window?.show(); window?.focus();
    });
    notification.show();
  };
  notifier = new HostNotifier({
    show: showNotification,
    isWindowFocused: () => windowFocused,
  });
  notifier.setPreferences(settings.getPublic().notifications);
  bridge = new SessionRuntimeManager({
    getSavedPermissionMode: () => settings.getLastPermissionMode(),
    onPermissionModeSelected: (mode) => settings.setLastPermissionMode(mode),
    getSavedFastMode: () => settings.getLastFastMode(),
    onFastModeSelected: (enabled) => settings.setLastFastMode(enabled),
    notifier,
    onCronRunRequested: (runtime, event) => scheduled ? scheduled.run(runtime, event.run_id, event.task) : Promise.reject(new Error('Scheduled task service unavailable')),
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    bridgeRoot,
    // Runtime assembly includes live MCP discovery before publishing the
    // bridge lockfile. A healthy project can exceed the generic 15s default
    // on a cold network; keep the test-injected short timeout untouched while
    // giving the real Desktop launch enough room to finish.
    lockfileTimeoutMs: 30_000,
    providerIds: PROVIDER_IDS,
    diagnostics,
    audioService: nativeAudio ?? undefined,
    accessState: (ref: SessionRef) => {
      try {
        // Adding a Project is the Desktop trust decision. Repository edits must
        // not silently revoke a live session or reintroduce the removed setup
        // prompt; removal from the Project list revokes access instead.
        return { workspace: ref.projectPath, trusted: settings.isTrustedWorkspace(ref.projectPath) };
      } catch {
        return { workspace: ref.projectPath, trusted: false };
      }
    },
    onModelSelected: (ref, model) => settings.setSessionModel(ref, model),
    getSavedModel: (ref) => settings.getSessionModel(ref),
    resolveProviderCredential: (providerId) => resolveProviderCredential(providerId, { credentialBroker }),
    resolveOpenAiOAuth: () => launchCache.openAiOAuth(credentialBroker),
    onOpenAiOAuthUpdated: async (session) => {
      if (!credentialBroker) throw new Error('Codex secure credential storage is unavailable');
      await credentialBroker.set(CODEX_PROVIDER_ID, JSON.stringify(parseCodexSession(session)));
      launchCache.invalidate();
    },
    invalidateLaunchConfigCache: () => launchCache.invalidate(),
    onFirstPromptSent: (ref) => { settings.setActiveSession(ref); },
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
    launchConfig: async (ref: SessionRef, resumeModel?: string) => {
      const workspace = ref.projectPath;
      const configured = settings.getPublic();
      const model = settings.getSessionModel(ref) ?? resumeModel ?? configured.model;
      const startedAt = Date.now();
      const credentialStartedAt = Date.now();
      const credentialsPromise = launchCache.credentials(model, credentialBroker).then((credentials) => {
        diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_phase', {
          durationMs: Date.now() - credentialStartedAt,
          parallel: true,
          phase: 'provider_credentials',
          projectPath: workspace,
          sessionId: ref.sessionId,
        }));
        return credentials;
      });
      const pluginSecretsStartedAt = Date.now();
      const pluginSecretsPromise = launchCache.pluginSecrets(credentialBroker).then((pluginSecrets) => {
        diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_phase', {
          durationMs: Date.now() - pluginSecretsStartedAt,
          parallel: true,
          phase: 'plugin_secrets',
          projectPath: workspace,
          sessionId: ref.sessionId,
        }));
        return pluginSecrets;
      });
      const openAiOAuthSelected = model?.startsWith(`${CODEX_PROVIDER_ID}/`) === true;
      const oauthStartedAt = Date.now();
      const openAiOAuthPromise = (openAiOAuthSelected
        ? launchCache.openAiOAuth(credentialBroker)
        : Promise.resolve(undefined)
      ).then((openaiOAuth) => {
        if (openAiOAuthSelected) {
          diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_phase', {
            durationMs: Date.now() - oauthStartedAt,
            parallel: true,
            phase: 'openai_oauth',
            projectPath: workspace,
            sessionId: ref.sessionId,
          }));
        }
        return openaiOAuth;
      });
      const [credentials, pluginSecrets, openaiOAuth] = await Promise.all([
        credentialsPromise,
        pluginSecretsPromise,
        openAiOAuthPromise,
      ]);
      diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_ready', {
        durationMs: Date.now() - startedAt,
        parallel: true,
        hasApiKey: Boolean(credentials.apiKey),
        hasOpenAiOAuth: Boolean(openaiOAuth),
        model,
        pluginSecretCount: Object.values(pluginSecrets).reduce((count, values) => count + Object.keys(values).length, 0),
        projectPath: workspace,
        providerCredentialCount: Object.keys(credentials.providerCredentials ?? {}).length,
        sessionId: ref.sessionId,
      }));
      return {
        workspace,
        sessionId: ref.sessionId,
        trusted: settings.isTrustedWorkspace(workspace),
        scheduledController: scheduled?.isControllerSession(workspace, ref.sessionId) ?? false,
        ...credentials,
        pluginSecrets,
        openaiOAuth,
        model,
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
    nativeAudio ?? undefined,
  );
  scheduled = new ScheduledTaskService(settings, bridge, sessionCatalog, (title, body, ref) => {
    // Propagate the refusal: `scheduledRun` returns false when the kind is off
    // or the notifier is disposed, and the caller keeps its dedupe key.
    return notifier?.scheduledRun(title, body, ref) ?? false;
  }, (error) => diagnostics.add('warn', 'host', sanitizeDiagnostic(error)));
  host.attachScheduled(scheduled);
  host.attachNotifier(notifier);
  scheduled.start();
  terminals = new TerminalManager({ isPackaged: app.isPackaged, resourcesPath: process.resourcesPath });
  host.attachTerminals(terminals);
  git = new GitService({ isBusy: async (root) => {
    for (const summary of bridge?.runtimeSummaries ?? []) {
      if (!summary.turnActive && !summary.pendingInteractions && !bridge?.get(summary.sessionId)?.hasActiveAgents) continue;
      // A session whose project directory was deleted or renamed makes
      // `resolveRoot` throw, and this callback runs at the head of EVERY git
      // request — an unrelated, healthy repository would stop working.
      try {
        if (await git?.resolveRoot(summary.projectPath) === root) return true;
      } catch { /* A stale runtime cannot make a live repository busy. */ }
    }
    return false;
  } });
  host.attachGit(git);
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
  notifier?.dispose();
  notifier = null;
  const currentGit = git;
  git = null;
  const currentTerminals = terminals;
  terminals = null;
  const currentAudio = nativeAudio;
  nativeAudio = null;
  const currentBridge = bridge;
  bridge = null;
  void Promise.all([
    currentGit?.dispose() ?? Promise.resolve(),
    currentTerminals?.dispose() ?? Promise.resolve(),
    currentAudio?.dispose() ?? Promise.resolve(),
    currentBridge?.dispose() ?? Promise.resolve(),
  ]).finally(() => app.quit());
  });
