import { LocalAppHost } from './local-apps/host.js';
import { LOCAL_APP_SCHEME } from '../shared/localApps.js';
import { homedir } from 'node:os';
import { installDeepLinks } from './deepLinks.js';
import { installCrashReporting } from './crashReporting.js';
import { AccountService } from './account.js';
import { accountSessionStore } from './account-storage.js';
import { brokerAccountSessionStore } from './account-broker-storage.js';
import firebaseConfig from '../shared/firebaseConfig.json';
import { ScheduledTaskService } from './scheduled.js';
import { GitService } from './git.js';
import { app, BrowserWindow, dialog, Menu, net, powerMonitor, protocol, session as electronSession, shell, Notification, type Session } from 'electron';
import { applicationMenuTemplate } from './applicationMenu.js';
import { CH_APPLICATION_MENU } from '../shared/applicationMenu.js';
import { join } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import type { SessionRef } from './bridgeTypes.js';
import { SessionRuntimeManager } from './sessionRuntimeManager.js';
import {
  createMacCredentialBrokerClient,
  resolveProviderCredential,
  SessionLaunchCache,
} from './credential-broker.js';
import { CodexTokenManager } from './codex-tokens.js';
import { createRemoteControl } from './remoteControl/electronBindings.js';
import { RemotePolicy, managedSettingsDirectory } from './remoteControl/policy.js';
import { RemoteCommands } from './remoteControl/remoteCommands.js';
import { nativeImageCodec } from './remoteControl/nativeImageCodec.js';
import { PushRequester, RemotePush } from './remoteControl/pushRequests.js';
import { RemoteControlUi } from './remoteControl/rendererIpc.js';
import { SessionGateway } from './remoteControl/sessionGateway.js';
import { remoteControlConfig } from './remoteControl/config.js';
import { systemClock } from '@lingxi/bridge-client/remote';
import type { RemoteControlService } from './remoteControl/RemoteControlService.js';
import { ProviderAudioHost } from './audio/providerAudioHost.js';
import { NativeAudioManager } from './audio/nativeAudioManager.js';
import { audioConfigurationDefaults } from '../shared/generatedAudioConfiguration.js';
import { TerminalManager } from './terminal.js';
import { HostController } from './host.js';
import { HostNotifier } from './notifications.js';
import { notificationPresentation } from './notificationPresentation.js';
import { DiagnosticBuffer, diagnosticEvent, sanitizeDiagnostic, resolveModBunExecutable } from './host-utils.js';
import { SettingsStore } from './settings.js';
import { requestMicrophoneAccess } from './microphoneAccess.js';
import { ProjectSessionCatalog } from './session-catalog.js';
import { ignoreBrokenPipe } from './process-streams.js';
import { PROVIDER_IDS } from '../shared/providers.js';
import {
  guardVisualizationWebview,
  hardenVisualizationGuest,
  installVisualizationSession,
} from './visualization.js';
import {
  CH_VISUALIZATION_GUEST_EVENT,
  VISUALIZATION_PARTITION,
  VISUALIZATION_SCHEME,
} from '../shared/visualization.js';

ignoreBrokenPipe(process.stdout);
ignoreBrokenPipe(process.stderr);

installCrashReporting();

// Inline visualization guests load `lingxi-viz://visualization/...` only. The
// scheme must be privileged before `app.ready` so its pages get a real origin
// and secure context; its handler is installed on the dedicated partition.
protocol.registerSchemesAsPrivileged([{
  scheme: LOCAL_APP_SCHEME,
  privileges: { standard: true, secure: true, supportFetchAPI: true, corsEnabled: true, stream: true, allowServiceWorkers: true },
}, {
  scheme: VISUALIZATION_SCHEME,
  privileges: { standard: true, secure: true, supportFetchAPI: false, corsEnabled: false, stream: false, codeCache: false },
}]);

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
let localApps: LocalAppHost | null = null;
let remoteControl: RemoteControlService | null = null;
let remotePolicy: RemotePolicy | null = null;
let remoteGateway: SessionGateway | null = null;
let remotePush: RemotePush | null = null;
let remoteUi: RemoteControlUi | null = null;
let remotePolicyTimer: NodeJS.Timeout | undefined;
/** How often the administrator's `managed-settings.json` is read again. */
const REMOTE_POLICY_RECHECK_MS = 30_000;
const mainWindows = new Set<BrowserWindow>();
// On macOS the app remains alive and the bridge stays available for reopen.
// Elsewhere it quits with its last window, unless a background app or a
// connected phone or browser still needs it.
function quitIfNothingNeedsTheApp(): void {
  if (process.platform === 'darwin' || quitting) return;
  void (async () => {
    const background = await localApps?.hasBackgroundApps();
    const viewers = (remoteControl?.status().connections.length ?? 0) > 0;
    if (!background && !viewers && BrowserWindow.getAllWindows().length === 0) app.quit();
  })().catch(error => console.error('Unable to check background applications', error));
}
function getMainWindow(): BrowserWindow | undefined { return [...mainWindows].find(window => !window.isDestroyed()); }
let deepLinkProjects: () => string[] = () => [];
let resolveDeepLinkReady: () => void;
const deepLinkReady = new Promise<void>(resolve => { resolveDeepLinkReady = resolve; });
const deepLinks = installDeepLinks(() => deepLinkProjects(), () => {
  let window = getMainWindow();
  if (!window && host) { createWindow(); window = getMainWindow(); }
  if (window) { if (window.isMinimized()) window.restore(); window.show(); window.focus(); }
}, () => deepLinkReady);


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

/** The hidden page of Remote Control's WebRTC window: the dev server's in development, the built file otherwise. */
function remoteRtcPage(): (window: BrowserWindow) => Promise<void> {
  const devUrl = developmentRendererUrl();
  if (devUrl) {
    const url = new URL('remote-rtc.html', devUrl).toString();
    return (window) => window.loadURL(url);
  }
  const file = join(moduleDirectory, '../renderer/remote-rtc.html');
  return (window) => window.loadFile(file);
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
    // Center the 12px native buttons in the renderer's 44px desktop toolbar.
    trafficLightPosition: { x: 14, y: 16 },
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(moduleDirectory, '../preload/index.cjs'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      // Enabled only for inline visualizations: `will-attach-webview` below
      // refuses every other webview and overwrites the guest's preferences.
      webviewTag: true,
      navigateOnDragDrop: false,
      webSecurity: true,
      devTools: !app.isPackaged,
    },
  });

  mainWindows.add(mainWindow);
  mainWindow.once('closed', () => mainWindows.delete(mainWindow));
  localApps?.registerMainWindow(mainWindow, target.url);
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
  mainWindow.webContents.on('will-attach-webview', (event, webPreferences, params) => {
    guardVisualizationWebview(
      event,
      webPreferences,
      params,
      join(moduleDirectory, '../preload/visualization.cjs'),
      !app.isPackaged,
    );
  });
  mainWindow.webContents.on('did-attach-webview', (_event, guest) => {
    hardenVisualizationGuest(guest, (guestEvent) => {
      if (!mainWindow.isDestroyed()) mainWindow.webContents.send(CH_VISUALIZATION_GUEST_EVENT, guestEvent);
    });
  });
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
  Menu.setApplicationMenu(Menu.buildFromTemplate(applicationMenuTemplate(process.platform, (action) => {
    if (action === 'open-link') { deepLinks.openClipboard(); return; }
    const window = getMainWindow() ?? createWindow();
    const send = () => {
      if (window.isDestroyed()) return;
      if (window.isMinimized()) window.restore();
      window.show();
      window.focus();
      window.webContents.send(CH_APPLICATION_MENU, action);
    };
    if (window.webContents.isLoading()) window.webContents.once('did-finish-load', send);
    else send();
  }, !app.isPackaged)));
  const userData = app.getPath('userData');
  const diagnostics = new DiagnosticBuffer(join(userData, 'logs', 'desktop.jsonl'));
  diagnostics.add('info', 'host', `desktop start: app=${app.getVersion()} electron=${process.versions.electron} platform=${process.platform} arch=${process.arch}`);
  const bridgeRoot = join(userData, 'bridge-runtime');
  const credentialBroker = createMacCredentialBrokerClient({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
  });
  diagnostics.add('info', 'host', credentialBroker
    ? 'credential store: macOS credential broker'
    : 'credential store: shared engine secure storage');
  const settings = new SettingsStore(userData);
  const audioSessionContext = (owner?: import('@lingxi/bridge-client').AudioOwnerDto) => {
    const sessionId = owner?.type === 'session' ? owner.session_id : settings.getPublic().activeSession?.sessionId;
    return sessionId ? bridge?.get(sessionId)?.audioSessionContext : undefined;
  };
  const providerAudio = new ProviderAudioHost({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    cwd: () => settings.getPublic().activeSession?.projectPath ?? settings.getWorkspace() ?? app.getPath('home'),
    session: () => audioSessionContext(),
    realtimeSupported: () => { const sessionId = settings.getPublic().activeSession?.sessionId; return !!sessionId && bridge?.get(sessionId)?.realtimeAudioSupported === true; },
    resolveCredential: (credentialId) => resolveProviderCredential(credentialId, { credentialBroker }),
  });
  nativeAudio = process.platform === 'darwin'
    ? new NativeAudioManager({
        isPackaged: app.isPackaged,
        resourcesPath: process.resourcesPath,
        userDataPath: userData,
        diagnostics,
        providerAudio,
        getAudioSessionContext: audioSessionContext,
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
  // The one refresher of the Codex session. Engines get only access tokens.
  // The wall clock is checked every minute and on wake: a timer alone would
  // sleep with the laptop.
  const codexTokens = new CodexTokenManager({
    store: credentialBroker,
    onError: (message) => diagnostics.add('warn', 'host', message),
  });
  codexTokens.start();
  powerMonitor.on('resume', () => {
    codexTokens.check();
    remoteControl?.reconnect();
  });
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
      const window = getMainWindow();
      window?.show(); window?.focus();
    });
    notification.show();
  };
  notifier = new HostNotifier({
    show: showNotification,
    isWindowFocused: () => windowFocused,
    // Remote Control is built after the notifier; both are looked up when used.
    remote: (kind, sessionId) => remotePush?.notify(kind, sessionId),
    systemIdleSeconds: () => powerMonitor.getSystemIdleTime(),
  });
  notifier.setPreferences(settings.getPublic().notifications);
  bridge = new SessionRuntimeManager({
    workspaceSettings: (projectPath) => sessionCatalog.workspaceSettings(projectPath),
    appVersion: app.getVersion(),
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
    codexTokens,
    invalidateLaunchConfigCache: () => launchCache.invalidate(),
    onFirstPromptSent: (ref) => {
      settings.setActiveSession(ref);
      // `remoteControlAtStartup`: a new chat starts shared with the user's other devices.
      try { remotePolicy?.sessionCreated(ref); } catch (error) { diagnostics.add('warn', 'host', `remote control: ${sanitizeDiagnostic(error)}`); }
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
      const parent = getMainWindow();
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
      // A chat opens even when its credentials cannot be read. The engine
      // starts without them, and the first prompt that needs one loads it
      // again or fails saying why. Every engine starts with the current
      // Codex access token, whatever its model.
      const phase = async <T>(name: string, load: () => Promise<T>, fallback: T): Promise<T> => {
        const phaseStartedAt = Date.now();
        try {
          const value = await load();
          diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_phase', {
            durationMs: Date.now() - phaseStartedAt,
            parallel: true,
            phase: name,
            projectPath: workspace,
            sessionId: ref.sessionId,
          }));
          return value;
        } catch (error) {
          diagnostics.add('warn', 'host', diagnosticEvent('session_launch_config_phase_failed', {
            durationMs: Date.now() - phaseStartedAt,
            error: sanitizeDiagnostic(error),
            phase: name,
            projectPath: workspace,
            sessionId: ref.sessionId,
          }));
          return fallback;
        }
      };
      const [credentials, pluginSecrets, openaiOAuth] = await Promise.all([
        phase('provider_credentials', () => launchCache.credentials(model, credentialBroker), {}),
        phase('plugin_secrets', () => launchCache.pluginSecrets(credentialBroker), {}),
        phase('codex_session', () => codexTokens.current(), undefined),
      ]);
      diagnostics.add('info', 'host', diagnosticEvent('session_launch_config_ready', {
        durationMs: Date.now() - startedAt,
        parallel: true,
        hasApiKey: Boolean(credentials.apiKey),
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
        allowInsights: settings.getInsightsAllowed(),
        allowTeamOnboarding: settings.getTeamOnboardingAllowed(),
        remoteControlEnabled: (settings.getRemoteControlEnabled() || (!app.isPackaged && process.env.LINGXI_REMOTE_CONTROL === '1'))
          && (remotePolicy?.allows() ?? true),
        scheduledController: scheduled?.isControllerSession(workspace, ref.sessionId) ?? false,
        ...credentials,
        ...(openaiOAuth ? { openaiOAuth } : {}),
        pluginSecrets,
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
  const account = new AccountService(firebaseConfig, credentialBroker
    ? brokerAccountSessionStore(credentialBroker)
    : accountSessionStore(join(app.getPath('userData'), 'firebase-account.enc')), url => shell.openExternal(url));
  host.attachAccount(account);
  // Off unless the user (or, unpackaged, LINGXI_REMOTE_CONTROL=1) turns it on.
  const remoteDiagnostics = (level: 'info' | 'warn' | 'error', message: string): void => {
    diagnostics.add(level, 'host', `remote control: ${message}`);
  };
  const policy = new RemotePolicy({
    settings,
    managedDirectory: managedSettingsDirectory(process.env, process.platform),
  });
  remotePolicy = policy;
  const runtimes = bridge;
  const gateway = new SessionGateway({
    runtimes,
    policy,
    commands: new RemoteCommands({
      allowAlwaysAllow: () => policy.allowAlwaysAllow(),
      echo: (echo) => remoteUi?.echo(echo),
      codec: nativeImageCodec,
      clock: systemClock,
    }),
    titleOf: async (ref) => (await sessionCatalog.find(ref.projectPath, ref.sessionId))?.title,
    modelOf: (ref) => settings.getSessionModel(ref),
    clock: systemClock,
    diagnostics: remoteDiagnostics,
  });
  remoteGateway = gateway;
  remoteControl = createRemoteControl({
    account,
    settings,
    gateway,
    policyAllows: () => policy.allows(),
    credentialBroker,
    userData,
    preload: join(moduleDirectory, '../preload/remoteRtc.cjs'),
    loadPage: remoteRtcPage(),
    parentWindow: getMainWindow,
    diagnostics: remoteDiagnostics,
  });
  const service = remoteControl;
  remotePush = new RemotePush({
    requester: new PushRequester({
      baseUrl: remoteControlConfig(process.env, app.isPackaged).pushApiUrl,
      hostId: () => service.status().host_id,
      getIdToken: (forceRefresh) => account.getIdToken(forceRefresh),
      fetch: (url, init) => net.fetch(url, init),
      diagnostics: remoteDiagnostics,
    }),
    presence: gateway,
    active: () => service.status().state === 'online' && policy.allows(),
    clock: systemClock,
  });
  remoteUi = new RemoteControlUi({
    service,
    settings,
    policy,
    gateway,
    runtimeRef: (sessionId) => {
      const runtime = bridge?.get(sessionId);
      return runtime ? { projectPath: runtime.projectPath, sessionId } : undefined;
    },
    signedIn: () => account.snapshot().user !== null,
    send: (channel, payload) => host?.sendToWindows(channel, payload),
  });
  host.attachRemoteControl(remoteUi);
  // The last viewer leaving a windowless app is the other moment to quit.
  let hadViewers = false;
  service.onStatus((status) => {
    const has = status.connections.length > 0;
    if (hadViewers && !has && mainWindows.size === 0) quitIfNothingNeedsTheApp();
    hadViewers = has;
  });
  // A change to the administrator's veto or to the shared sessions re-evaluates whether the service runs.
  policy.onChange(() => service.refresh());
  remotePolicyTimer = setInterval(() => { policy.recheck(); }, REMOTE_POLICY_RECHECK_MS);
  remotePolicyTimer.unref();
  void remoteControl.start();
  host.attachScheduled(scheduled);
  host.attachNotifier(notifier);
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
  localApps = new LocalAppHost({
    configHome: process.env.LINGXI_CONFIG_DIR ?? join(homedir(), '.lingxi'),
    preload: join(moduleDirectory, '../preload/localApp.cjs'),
    nodeExecutable: process.execPath,
    bunExecutable: resolveModBunExecutable({ isPackaged: app.isPackaged, resourcesPath: process.resourcesPath, configuredPath: process.env.LINGXI_MOD_BUN_EXECUTABLE }),
    mainWindow: getMainWindow,
    edit: async row => {
      const window = getMainWindow() ?? createWindow();
      window.show(); window.focus();
      // The renderer owns navigation and must apply the selected chat's bootstrap
      // before placing an edit request in its session-bound composer draft.
      window.webContents.send('lingxi:local-app:edit', { id: row.id, name: row.name, sourcePath: row.sourcePath, sourceSession: row.sourceSession });
    },
  });
  await localApps.initialize();
  try {
    await installVisualizationSession(electronSession.fromPartition(VISUALIZATION_PARTITION), bridge.visualizations);
  } catch (error) {
    diagnostics.add('warn', 'host', `inline visualizations unavailable: ${sanitizeDiagnostic(error)}`);
  }
  deepLinkProjects = () => settings.getPublic().projects;
  deepLinks.register();
  createWindow();

  const publicSettings = settings.getPublic();
  const activeProject = publicSettings.activeProject;
  const foregroundRestore = activeProject
    ? host.restoreProjectSession(activeProject, publicSettings.activeSession).then(() => undefined, (error: unknown) => {
        diagnostics.add('error', 'host', sanitizeDiagnostic(error));
      })
    : Promise.resolve();
  // Scheduled-task controllers are engine sessions too. Started beside the
  // restored chat, every runtime contended for CPU, disk and the credential
  // broker; start them once the foreground chat is up, or after a bound so a
  // stuck restore cannot hold the schedule.
  let scheduledStarted = false;
  const startScheduled = (): void => {
    if (scheduledStarted || quitting) return;
    scheduledStarted = true;
    scheduled?.start();
  };
  const scheduledFallback = setTimeout(startScheduled, 10_000);
  void foregroundRestore.then(() => {
    resolveDeepLinkReady();
    clearTimeout(scheduledFallback);
    startScheduled();
  });

  app.on('activate', () => {
    if (!getMainWindow()) createWindow();
  });

  });

  app.on('second-instance', () => {
  const window = getMainWindow() ?? (host ? createWindow() : undefined);
  if (!window) return;
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
  });

  app.on('window-all-closed', quitIfNothingNeedsTheApp);

  app.on('before-quit', (event) => {
  if (quitting) return;
  event.preventDefault();
  quitting = true;
  host?.dispose();
  host = null;
  notifier?.dispose();
  notifier = null;
  const currentLocalApps = localApps;
  localApps = null;
  const currentGit = git;
  git = null;
  const currentTerminals = terminals;
  terminals = null;
  const currentAudio = nativeAudio;
  nativeAudio = null;
  const currentBridge = bridge;
  bridge = null;
  const currentRemote = remoteControl;
  remoteControl = null;
  clearInterval(remotePolicyTimer);
  remotePolicyTimer = undefined;
  remoteGateway?.dispose();
  remoteGateway = null;
  remotePush = null;
  remoteUi = null;
  remotePolicy = null;
  void Promise.all([
    currentLocalApps?.dispose() ?? Promise.resolve(),
    currentGit?.dispose() ?? Promise.resolve(),
    currentTerminals?.dispose() ?? Promise.resolve(),
    currentAudio?.dispose() ?? Promise.resolve(),
    currentBridge?.dispose() ?? Promise.resolve(),
    currentRemote?.dispose() ?? Promise.resolve(),
  ]).finally(() => app.quit());
  });
