import { promises as fs } from 'node:fs';
import { LocalAppCoreClient } from './local-apps/coreClient.js';
import { RemoteFileStore } from './remoteControl/fileStore.js';
import { LocalAppHost } from './local-apps/host.js';
import { resolveDesktopAuthoringBinding } from './local-apps/authoring.js';
import { LocalAppAgentBroker } from './local-apps/agentBroker.js';
import { LocalAppAgentExecutor } from './local-apps/agentExecutor.js';
import { LocalAppAgentError } from './local-apps/agentPolicy.js';
import { configureNativeLocale, nativeTr, onNativeLocaleChange } from '../shared/nativeLocale.js';
import { persistInterfaceLocale, restoreInterfaceLocale } from './localePreferences.js';
import { LOCAL_APP_SCHEME } from '../shared/localApps.js';
import { installDeepLinks } from './deepLinks.js';
import { installCrashReporting } from './crashReporting.js';
import { AccountService } from './account.js';
import { accountSessionStore } from './account-storage.js';
import { brokerAccountSessionStore } from './account-broker-storage.js';
import firebaseConfig from '../shared/firebaseConfig.json';
import { ScheduledTaskService } from './scheduled.js';
import { GitService } from './git.js';
import { app, BrowserWindow, dialog, Menu, net, powerMonitor, powerSaveBlocker, protocol, session as electronSession, shell, Notification, type Session } from 'electron';
import { applicationMenuTemplate } from './applicationMenu.js';
import { CH_APPLICATION_MENU } from '../shared/applicationMenu.js';
import { installDesktopUpdates } from './desktopUpdates.js';
import { desktopUpdateManifestUrl } from '../shared/productConfig.js';
import { CH_BOTS_PROMPT_SENT, CH_BOTS_TASKS_CHANGED, type BotPromptSentUi, type BotTasksChangedUi } from '../shared/bots.js';
import { join } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import type { SessionRef } from './bridgeTypes.js';
import { SessionRuntimeManager } from './sessionRuntimeManager.js';
import {
  createMacCredentialBrokerClient,
  readProviderEnvironmentCredential,
  resolveProviderCredential,
  SessionLaunchCache,
} from './credential-broker.js';
import { CodexTokenManager, codexAccessToken } from './codex-tokens.js';
import { CODEX_PROVIDER_ID } from './codex-auth.js';
import { createRemoteControl } from './remoteControl/electronBindings.js';
import { RemotePolicy, managedSettingsDirectory } from './remoteControl/policy.js';
import { RemoteCommands } from './remoteControl/remoteCommands.js';
import { nativeImageCodec } from './remoteControl/nativeImageCodec.js';
import { PushRequester, RemotePush } from './remoteControl/pushRequests.js';
import { RemoteControlUi } from './remoteControl/rendererIpc.js';
import { BotContent } from './bots/content.js';
import { KeepAwake } from './bots/keepAwake.js';
import { LoginItem } from './bots/loginItem.js';
import { ownerPreferenceHooks } from './bots/ownerPreferences.js';
import { BotsUi } from './bots/rendererIpc.js';
import { BotRouter } from './bots/router.js';
import { BotService } from './bots/service.js';
import { BotStore } from './bots/store.js';
import { TaskChangeAnnouncer } from './bots/taskChanges.js';
import { BotTasks } from './bots/taskService.js';
import { SessionGateway } from './remoteControl/sessionGateway.js';
import { LocalAppsGateway } from './remoteControl/localAppsGateway.js';
import { createRemoteLocalAppsDelegate } from './remoteControl/localAppsBindings.js';
import { LocalAppRtc } from './local-apps/remoteRtc.js';
import { PeerHost } from './remoteControl/peerHost.js';
import { createRtcWindow } from './remoteControl/rtcWindow.js';
import { parseRtcIceServers } from '../shared/remoteRtc.js';
import { remoteControlConfig } from './remoteControl/config.js';
import { DEFAULT_STUN_URLS, systemClock } from '@lingxi/bridge-client/remote';
import type { RemoteControlService } from './remoteControl/RemoteControlService.js';
import { ProviderAudioHost } from './audio/providerAudioHost.js';
import { NativeAudioManager } from './audio/nativeAudioManager.js';
import { audioConfigurationDefaults } from '../shared/generatedAudioConfiguration.js';
import { TerminalManager } from './terminal.js';
import { HostController } from './host.js';
import { HostNotifier } from './notifications.js';
import { notificationPresentation } from './notificationPresentation.js';
import { BOT_PERMISSION_TIMEOUT_SECS, DiagnosticBuffer, diagnosticEvent, sanitizeDiagnostic, resolveModBunExecutable } from './host-utils.js';
import { SettingsStore } from './settings.js';
import { ComputerSettingsStore } from './computerSettings.js';
import { BrowserIntegration } from './browserIntegration.js';
import { resolveServerBin } from './bridgeDiscovery.js';
import { CH_COMPUTER_SETTINGS_CHANGED } from '../shared/computerSettings.js';
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
let browserIntegration: BrowserIntegration | null = null;
let scheduled: ScheduledTaskService | null = null;
let git: GitService | null = null;
let terminals: TerminalManager | null = null;
let nativeAudio: NativeAudioManager | null = null;
let notifier: HostNotifier | null = null;
let quitting = false;
let shutdownDiagnostics: DiagnosticBuffer | undefined;
let localApps: LocalAppHost | null = null;
let remoteControl: RemoteControlService | null = null;
let remotePolicy: RemotePolicy | null = null;
let remoteGateway: SessionGateway | null = null;
let remotePush: RemotePush | null = null;
let remoteUi: RemoteControlUi | null = null;
/** Desktop Bots; absent until the product paths are known. */
let bots: BotService | null = null;
/** The endpoint through which Bots hand tasks to each other; made with the Bots. */
let botRouter: BotRouter | null = null;
let botTasks: BotTasks | null = null;
let remotePolicyTimer: NodeJS.Timeout | undefined;
/** How often the administrator's `managed-settings.json` is read again. */
const REMOTE_POLICY_RECHECK_MS = 30_000;
const mainWindows = new Set<BrowserWindow>();
let desktopUpdates: ReturnType<typeof installDesktopUpdates> | null = null;
// On macOS the app remains alive and the bridge stays available for reopen.
// Elsewhere it quits with its last window, unless a background app or a
// connected phone or browser still needs it.
function quitIfNothingNeedsTheApp(): void {
  if (process.platform === 'darwin' || quitting) return;
  void (async () => {
    const background = await localApps?.hasBackgroundApps();
    const viewers = (remoteControl?.status().connections.length ?? 0) > 0;
    if (!background && !localApps?.hasRemoteAccess() && !viewers && BrowserWindow.getAllWindows().length === 0) app.quit();
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
    icon: join(app.isPackaged ? process.resourcesPath : join(moduleDirectory, '../../assets/icons'), 'icon.png'),
    width: 1320,
    height: 860,
    minWidth: 900,
    minHeight: 600,
    show: false,
    backgroundColor: '#0c0b10',
    titleBarStyle: 'hiddenInset',
    // Expose the native traffic-light exclusion area to the renderer.
    titleBarOverlay: process.platform === 'darwin' ? true : undefined,
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
  desktopUpdates?.registerWindow(mainWindow.webContents, target.url);
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

configureNativeLocale(() => app.getPreferredSystemLanguages());
const interfaceLocalePath = join(app.getPath('userData'), 'interface-language.json');
restoreInterfaceLocale(interfaceLocalePath);
if (hasSingleInstanceLock) void app.whenReady().then(async () => {
  const refreshApplicationMenu = () => Menu.setApplicationMenu(Menu.buildFromTemplate(applicationMenuTemplate(process.platform, (action) => {
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
  onNativeLocaleChange(refreshApplicationMenu);
  refreshApplicationMenu();
  const userData = app.getPath('userData');
  const diagnostics = new DiagnosticBuffer(join(userData, 'logs', 'desktop.jsonl'));
  shutdownDiagnostics = diagnostics;
  onNativeLocaleChange(() => {
    try { persistInterfaceLocale(interfaceLocalePath); }
    catch (error) { diagnostics.add('warn', 'host', `interface language persistence failed: ${sanitizeDiagnostic(error)}`); }
  });
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
  let lastComputerRevision = '';
  const computerSettings = new ComputerSettingsStore(userData, {
    showExtension: async path => { const error = await shell.openPath(path); if (error) throw new Error(error); },
    onChanged: () => {
      const state = computerSettings.snapshot();
      if (lastComputerRevision && state.revision !== lastComputerRevision) {
        for (const summary of bridge?.runtimeSummaries ?? []) {
          try {
            const runtime = bridge?.get(summary.sessionId);
            if (state.error) void runtime?.stopForComputerPolicyFailure().catch(error => diagnostics.add('warn', 'host', sanitizeDiagnostic(error)));
            else runtime?.revokeComputerAccess();
          }
          catch (error) { diagnostics.add('warn', 'host', sanitizeDiagnostic(error)); }
        }
      }
      lastComputerRevision = state.revision;
      host?.sendToWindows(CH_COMPUTER_SETTINGS_CHANGED, state);
    },
  });
  lastComputerRevision = computerSettings.policy().revision;
  let browserExecutable = '';
  try { browserExecutable = resolveServerBin({ isPackaged: app.isPackaged, resourcesPath: process.resourcesPath }); }
  catch { /* The setup action reports missing bridge resources. */ }
  const browserDescriptor = join(userData, 'computer-use', 'browser-connection.json');
  browserIntegration = new BrowserIntegration({
    policy: () => computerSettings.policy(),
    descriptorPath: browserDescriptor,
    nativeHostPath: join(userData, 'computer-use', 'browser-native-host'),
    bridgeExecutable: browserExecutable,
    extensionDirectory: app.isPackaged ? join(process.resourcesPath, 'browser-extension') : join(app.getAppPath(), 'browser-extension'),
    onChanged: () => host?.sendToWindows(CH_COMPUTER_SETTINGS_CHANGED, computerSettings.snapshot()),
  });
  computerSettings.attachBrowser(browserIntegration);
  try { await browserIntegration.start(); }
  catch (error) {
    const failedBrowser = browserIntegration;
    browserIntegration = null;
    await failedBrowser.close().catch(() => {});
    computerSettings.browserUnavailable('Browser integration could not start. Restart Desktop to try again.');
    diagnostics.add('warn', 'host', `browser integration startup failed: ${sanitizeDiagnostic(error)}`);
  }
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
  const catalogOptions = {
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    serverBin: app.isPackaged ? undefined : process.env['LINGXI_BRIDGE_SERVER_BIN'],
  };
  const sessionCatalog = new ProjectSessionCatalog(catalogOptions);
  // Off unless the owner (or, unpackaged, LINGXI_REMOTE_CONTROL=1) turns Remote Control on for this computer.
  const remoteControlOn = (): boolean => settings.getRemoteControlEnabled() || (!app.isPackaged && process.env.LINGXI_REMOTE_CONTROL === '1');
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
    // Timers stand still while the Mac sleeps: the routines that came due and the
    // tasks whose deadline has passed are looked for by the clock.
    scheduled?.wake();
    botTasks?.checkDeadlines();
  });
  /**
   * The one place an OS notification is raised. Shared by the scheduled-task
   * service and `HostNotifier` so both get the same click behaviour: show
   * the session the notification came from, then raise the window.
   */
  const showNotification = (title: string, body: string, ref?: SessionRef): void => {
    if (!Notification.isSupported()) return;
    // A Bot's workspace is a folder nobody named; the notification names the Bot.
    const notification = new Notification(notificationPresentation(title, body, ref, process.platform, ref ? bots?.nameFor(ref.projectPath) : undefined));
    notification.on('click', () => {
      const shown = ref ? host?.showNotifiedSession(ref).catch((error) => diagnostics.add('error', 'host', sanitizeDiagnostic(error))) : undefined;
      const window = getMainWindow();
      if (window) { window.show(); window.focus(); return; }
      // Opened at login, the app has no window: the click makes one, once the session is
      // chosen, so that the window opens on it.
      void Promise.resolve(shown).then(() => {
        if (!host) return;
        const created = getMainWindow() ?? createWindow();
        created.show(); created.focus();
      });
    });
    notification.show();
  };
  notifier = new HostNotifier({
    show: showNotification,
    isWindowFocused: () => windowFocused,
    // Remote Control is built after the notifier; both are looked up when used.
    remote: (kind, sessionId) => remotePush?.notify(kind, sessionId),
    systemIdleSeconds: () => powerMonitor.getSystemIdleTime(),
    // The tasks between Bots are made after the notifier too.
    delegation: (sessionKey) => botTasks?.doing(sessionKey),
  });
  notifier.setPreferences(settings.getPublic().notifications);
  bridge = new SessionRuntimeManager({
    getTelemetryConfig: () => settings.getTelemetryConfig(),
    // A Bot's working directory is not a project: it has its own configuration home and grants.
    workspaceSettings: (projectPath, ref) => bots?.ownsWorkspace(projectPath)
      ? bots.launchSettings(projectPath, ref.sessionId) : sessionCatalog.workspaceSettings(projectPath),
    appVersion: app.getVersion(),
    ...ownerPreferenceHooks(settings, (ref) => bots?.ownsWorkspace(ref.projectPath) === true),
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
    getMediaAudioConfigurationJson: () => JSON.stringify(
      settings.getPublic().voice ?? audioConfigurationDefaults(),
    ),
    accessState: (ref: SessionRef) => {
      try {
        // Adding a Project is the Desktop trust decision. Repository edits must
        // not silently revoke a live session or reintroduce the removed setup
        // prompt; removal from the Project list revokes access instead.
        return { workspace: ref.projectPath, trusted: settings.isTrustedWorkspace(ref.projectPath) || bots?.ownsWorkspace(ref.projectPath) === true };
      } catch {
        return { workspace: ref.projectPath, trusted: false };
      }
    },
    // A Bot's model is its own; choosing one in a Bot's session must not become the default of the owner's sessions.
    onModelSelected: (ref, model) => { if (!bots?.ownsWorkspace(ref.projectPath)) settings.setSessionModel(ref, model); },
    getSavedModel: (ref) => bots?.ownsWorkspace(ref.projectPath) ? undefined : settings.getSessionModel(ref),
    resolveProviderCredential: (providerId) => resolveProviderCredential(providerId, { credentialBroker }),
    codexTokens,
    invalidateLaunchConfigCache: () => launchCache.invalidate(),
    onFirstPromptSent: (ref) => {
      // A Bot's session is not one of the owner's: it is neither the one to restore at startup nor one to share.
      if (bots?.ownsWorkspace(ref.projectPath)) return;
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
        buttons: [nativeTr('common_cancel'), nativeTr('native_bypass_accept')],
        defaultId: 0,
        cancelId: 0,
        noLink: true,
        title: nativeTr('native_bypass_title'),
        message: nativeTr('native_bypass_message'),
        detail: nativeTr('native_bypass_detail') + '\n\nhttps://code.claude.com/docs/en/security',
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
      const botWorkspace = bots?.ownsWorkspace(workspace) === true;
      const model = botWorkspace
        ? resumeModel ?? bots?.modelFor(workspace) ?? configured.model
        : settings.getSessionModel(ref) ?? resumeModel ?? configured.model;
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
        computerUseEnvironment: {
          LINGXI_DESKTOP_COMPUTER_POLICY: computerSettings.executionPolicyPath,
          LINGXI_DESKTOP_BROWSER_DESCRIPTOR: browserDescriptor,
        },
        sessionId: ref.sessionId,
        // Making a Bot is the owner's decision to trust its folders; they are not in the project list.
        trusted: settings.isTrustedWorkspace(workspace) || botWorkspace,
        telemetry: settings.getTelemetryConfig(),
        allowInsights: settings.getInsightsAllowed(),
        allowTeamOnboarding: settings.getTeamOnboardingAllowed(),
        // A Bot's sessions are launched for remote devices when the Bot's own switch is on as well; its owner may be away for a day.
        remoteControlEnabled: botWorkspace
          ? remotePolicy?.botReachable(workspace) === true
          : remoteControlOn() && (remotePolicy?.allows() ?? true),
        ...(botWorkspace ? { permissionTimeoutSecs: BOT_PERMISSION_TIMEOUT_SECS } : {}),
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
  host.attachComputerSettings(computerSettings);
  // Whether the Mac may sleep while Bots work is the owner's to say; the switch is in the Bots page.
  const keepAwake = new KeepAwake({
    start: () => powerSaveBlocker.start('prevent-app-suspension'),
    stop: (id) => powerSaveBlocker.stop(id),
    enabled: settings.getPublic().botsKeepAwake === true,
  });
  host.attachKeepAwake(keepAwake);
  // Opened at login, the app waits in the background: the Bots work without a window.
  const loginItem = new LoginItem({ app, openExternal: (url) => shell.openExternal(url) });
  scheduled = new ScheduledTaskService(settings, bridge, sessionCatalog, (title, body, ref) => {
    // Propagate the refusal: `scheduledRun` returns false when the kind is off
    // or the notifier is disposed, and the caller keeps its dedupe key.
    return notifier?.scheduledRun(title, body, ref) ?? false;
  }, (error) => diagnostics.add('warn', 'host', sanitizeDiagnostic(error)), { keepAwake });
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
    computerEnabled: remoteControlOn,
    bots: () => bots,
  });
  remotePolicy = policy;
  const runtimes = bridge;
  const gateway = new SessionGateway({
    files: new RemoteFileStore(join(userData, 'remote-files')),
    runtimes,
    policy,
    bots: () => bots,
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
  let remoteAccessGeneration = 0;
  const remoteOwnerGenerations = new Map<string, number>();
  const remoteOwnerKey = (owner: { accountUid: string; deviceId: string; hostId: string }) => JSON.stringify([owner.accountUid, owner.deviceId, owner.hostId]);
  const revokeRemoteAppAccess = (): void => {
    remoteAccessGeneration++;
    remoteOwnerGenerations.clear();
    localApps?.remoteRevokeAll();
  };
  let remoteAccountUid = account.snapshot().user?.uid;
  account.onChanged(() => {
    const uid = account.snapshot().user?.uid;
    if (uid !== remoteAccountUid) { remoteAccountUid = uid; revokeRemoteAppAccess(); }
  });
  const appsGateway = new LocalAppsGateway(gateway, createRemoteLocalAppsDelegate({
    host: () => localApps,
    accountUid: () => account.snapshot().user?.uid,
    hostId: () => remoteControl?.status().host_id,
    active: () => remoteControl?.status().state === 'online' && policy.allows(),
    generation: viewer => `${remoteAccessGeneration}:${remoteOwnerGenerations.get(remoteOwnerKey({ accountUid: viewer.account_uid!, deviceId: viewer.device_id, hostId: viewer.host_id })) ?? 0}`,
    approvedDevice: deviceId => remoteControl?.listDevices().some(device => device.device_id === deviceId) ?? false,
  }));
  remoteControl = createRemoteControl({
    account,
    settings,
    gateway: appsGateway,
    deviceRevoked: owner => { const key = remoteOwnerKey(owner); remoteOwnerGenerations.set(key, (remoteOwnerGenerations.get(key) ?? 0) + 1); localApps?.remoteRevokeOwner(owner); },
    accessStopped: revokeRemoteAppAccess,
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
      // A Bot's chats stay on this computer: `/remote-control` in one finds no session to share.
      return runtime && !bots?.ownsWorkspace(runtime.projectPath) ? { projectPath: runtime.projectPath, sessionId } : undefined;
    },
    signedIn: () => account.snapshot().user !== null,
    onAccountChanged: (callback) => account.onChanged(() => callback()),
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
  policy.onChange(() => { if (!policy.allows()) revokeRemoteAppAccess(); service.refresh(); });
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
  const productPaths = await sessionCatalog.productPaths().catch(error => {
    diagnostics.add('warn', 'host', `local application directory unavailable: ${sanitizeDiagnostic(error)}`);
    return undefined;
  });
  if (productPaths) {
    // The Bots live beside the app's other data and keep clear of the default configuration home.
    try {
      const botStore = new BotStore({ userData, home: app.getPath('home'), defaultConfigHome: productPaths.configHome });
      const manager = bridge;
      const botWarn = (message: string): void => { diagnostics.add('warn', 'host', `bots: ${message}`); };
      // Tasks between Bots, and the endpoint they are handed through. The router listens
      // before any session of a Bot is launched, which asks it for an address and a token.
      const taskChanges = new TaskChangeAnnouncer((botIds) => host?.sendToWindows(CH_BOTS_TASKS_CHANGED, { botIds } satisfies BotTasksChangedUi));
      const tasks = new BotTasks({
        store: botStore,
        runtimes: { withSession: (ref, empty, model, operation) => manager.withBackgroundSession(ref, empty, model, operation) },
        available: () => bots?.availability().available === true,
        isDeleting: (botId) => bots?.isDeleting(botId) === true,
        warn: botWarn,
        keepAwake,
        changed: (botIds) => taskChanges.changed(botIds),
        // The engine's events carry no user prompts: the window shows the one the task began with.
        promptSent: (sessionId, text) => host?.sendToWindows(CH_BOTS_PROMPT_SENT, { sessionId, text } satisfies BotPromptSentUi),
      });
      botTasks = tasks;
      tasks.load();
      const router = new BotRouter({ backend: tasks, warn: botWarn });
      botRouter = router;
      router.start().catch((error: unknown) => botWarn(`the router cannot listen: ${sanitizeDiagnostic(error)}`));
      router.watchRuntimes(manager);
      bots = new BotService({
        store: botStore,
        runtimes: bridge,
        router,
        delegation: tasks,
        // Remote devices reach a Bot's chats by session id, and its switch decides whether they may.
        openProject: (sessionId) => manager.get(sessionId)?.projectPath,
        changed: () => remotePolicy?.notifySessionsChanged(),
        catalogFor: (configHome) => new ProjectSessionCatalog({ ...catalogOptions, configHome }),
        // The scheduler was made first; it learns of the Bots below, and is asked for these only when used.
        routines: {
          isControllerSession: (workspace, sessionId) => scheduled?.isControllerSession(workspace, sessionId) === true,
          releaseScope: (workspace) => scheduled?.releaseScope(workspace),
        },
        warn: botWarn,
      });
      scheduled?.attachBots(bots);
      host.attachBots(new BotsUi({ service: bots, content: new BotContent(botStore), tasks, loginItem }));
    } catch (error) {
      diagnostics.add('warn', 'host', `bots unavailable: ${sanitizeDiagnostic(error)}`);
    }
    const agentRoot = join(productPaths.configHome, 'local-apps', 'v2');
    const agentExecutor = new LocalAppAgentExecutor({
      configHome: productPaths.configHome,
      root: agentRoot,
      isPackaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      credentials: async (providerId, refresh) => {
        if (providerId === CODEX_PROVIDER_ID) {
          const session = await (refresh ? codexTokens.renew() : codexTokens.current());
          if (!session) throw new LocalAppAgentError('model_unavailable', 'Sign in to the selected Agent model in LingXi settings.');
          return { openaiOAuth: codexAccessToken(session) };
        }
        const key = await resolveProviderCredential(providerId, { credentialBroker });
        if (!key) throw new LocalAppAgentError('model_unavailable', 'Configure the selected Agent model in LingXi settings.');
        return { providerCredentials: { [providerId]: key } };
      },
      credentialStatus: async providerIds => {
        const rows = await credentialBroker?.listStatus(providerIds) ?? [];
        const configured = new Set(rows.filter(row => row.configured).map(row => row.providerId));
        for (const id of providerIds) if (readProviderEnvironmentCredential(id)) configured.add(id);
        return configured;
      },
      permission: (input, signal) => localApps!.promptAgentPermission(input, signal),
      question: (input, signal) => localApps!.promptAgentQuestion(input, signal),
    });
    localApps = new LocalAppHost({
      configHome: productPaths.configHome,
      onRemoteAccessEnded: quitIfNothingNeedsTheApp,
      accountToken: () => account.getIdToken(),
      accountId: () => account.snapshot().user?.uid ?? undefined,
      core: new LocalAppCoreClient(resolveServerBin({ isPackaged: app.isPackaged, resourcesPath: process.resourcesPath }), productPaths.configHome, message => diagnostics.add('warn', 'host', message)),
      resolveAuthoringBinding: async (_appId, session) => {
        const activeSession = settings.getPublic().activeSession ?? undefined;
        const liveDraft = bridge?.getActiveDraftSession(session, activeSession);
        return resolveDesktopAuthoringBinding(sessionCatalog, session, liveDraft);
      },
      preload: join(moduleDirectory, '../preload/localApp.cjs'),
      nodeExecutable: process.execPath,
      bunExecutable: resolveModBunExecutable({ isPackaged: app.isPackaged, resourcesPath: process.resourcesPath, configuredPath: process.env.LINGXI_MOD_BUN_EXECUTABLE }),
      mainWindow: () => getMainWindow() ?? createWindow(),
      agentCatalog: () => agentExecutor.catalog(),
      edit: async request => {
        const window = getMainWindow() ?? createWindow();
        window.show(); window.focus();
        // The renderer owns navigation and must apply the selected chat's bootstrap
        // before placing an edit request in its session-bound composer draft.
        window.webContents.send('lingxi:local-app:edit', request);
      },
    });
    const agentBroker = new LocalAppAgentBroker({
      root: agentRoot,
      authorize: owner => localApps!.authorizeAgent(owner),
      revalidate: (owner, authorization) => localApps!.revalidateAgent(owner, authorization),
      execute: execution => agentExecutor.execute(execution),
    });
    try {
      await agentBroker.initialize();
      localApps.setAgentBroker(agentBroker);
    } catch {
      diagnostics.add('error', 'host', 'Application Agent task storage is unavailable. Existing task records were preserved.');
    }
    await localApps.initialize();
    if (localApps.remoteConfiguration().configured) {
      try {
        const ice = parseRtcIceServers(process.env.LINGXI_LOCAL_APP_ICE_SERVERS
          ? JSON.parse(process.env.LINGXI_LOCAL_APP_ICE_SERVERS)
          : [...DEFAULT_STUN_URLS]);
        if (!ice) throw new Error('Invalid ICE configuration');
        const appHost = localApps;
        const peers = new PeerHost({
          createWindow: () => createRtcWindow({ preload: join(moduleDirectory, '../preload/remoteRtc.cjs'), load: remoteRtcPage() }),
          diagnostics: remoteDiagnostics,
        });
        appHost.setRemoteRtc(new LocalAppRtc({
          peerHost: peers,
          current: run => appHost.remoteCurrent(run),
          resource: (run, request) => appHost.remoteResource(run, request),
          iceServers: ice,
        }));
      } catch {
        diagnostics.add('warn', 'host', 'Local App WebRTC configuration is unavailable; HTTPS access remains available.');
      }
    }
  }
  try {
    await installVisualizationSession(electronSession.fromPartition(VISUALIZATION_PARTITION), bridge.visualizations);
  } catch (error) {
    diagnostics.add('warn', 'host', `inline visualizations unavailable: ${sanitizeDiagnostic(error)}`);
  }
  deepLinkProjects = () => settings.getPublic().projects;
  deepLinks.register();
  desktopUpdates = installDesktopUpdates({
    currentVersion: app.getVersion(),
    manifestUrl: desktopUpdateManifestUrl,
    preferencePath: join(userData, 'desktop-update-reminder.json'),
    onError: error => diagnostics.add('warn', 'host', `Desktop update check failed: ${sanitizeDiagnostic(error)}`),
  });
  // Read once, here: the system says "opened at login" only for the start that it did.
  if (!loginItem.openedInBackground()) createWindow();
  if (app.isPackaged) {
    desktopUpdates.start();
    powerMonitor.on('resume', () => { void desktopUpdates?.check(); });
  }

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
  desktopUpdates?.dispose();
  desktopUpdates = null;
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
  const currentBrowser = browserIntegration;
  browserIntegration = null;
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
  bots = null;
  botTasks = null;
  const currentBotRouter = botRouter;
  botRouter = null;
  const shutdownPhases: Array<[string, () => Promise<void>]> = [
    ['bot_router', () => currentBotRouter?.dispose() ?? Promise.resolve()],
    ['browser', () => currentBrowser?.close() ?? Promise.resolve()],
    ['local_apps', () => currentLocalApps?.dispose() ?? Promise.resolve()],
    ['git', () => currentGit?.dispose() ?? Promise.resolve()],
    ['terminals', () => currentTerminals?.dispose() ?? Promise.resolve()],
    ['audio', () => currentAudio?.dispose() ?? Promise.resolve()],
    ['bridge', () => currentBridge?.dispose() ?? Promise.resolve()],
    ['remote', () => currentRemote?.dispose() ?? Promise.resolve()],
  ];
  void Promise.all(shutdownPhases.map(async ([phase, dispose]) => {
    const started = Date.now();
    const record = (status: string): void => shutdownDiagnostics?.add('info', 'host', JSON.stringify({
      event: 'shutdown_phase', phase, status, durationMs: Date.now() - started,
    }));
    record('started');
    try { await dispose(); record('completed'); }
    catch (error) { record('failed'); throw error; }
  })).finally(() => app.quit());
  });
