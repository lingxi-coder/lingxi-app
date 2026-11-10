package com.lingxi.code

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import androidx.compose.foundation.background
import androidx.activity.viewModels
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import com.lingxi.code.vision.CameraController
import com.lingxi.code.share.ShareController
import com.lingxi.code.notify.NotificationController
import com.lingxi.code.clipboard.ClipboardController
import com.lingxi.code.device.AndroidDeviceControlController
import com.lingxi.code.device.DeviceReadPermissionController
import com.lingxi.code.location.LocationController
import com.lingxi.code.offload.NativeOffloadRuntime
import com.lingxi.code.voice.audio.AndroidMediaPermissionCoordinator
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.key
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.cron.CronAlarmScheduler
import com.lingxi.code.onboarding.SetupWizardOverlay
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.DeepLinkImportOverlay
import com.lingxi.code.tabs.MainTabShell
import com.lingxi.code.tabs.SettingsTab
import com.lingxi.code.tabs.TabLogic
import com.lingxi.code.tabs.rememberTabShellState
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.conversation.ConversationLaunchRequest
import com.lingxi.code.terminal.TerminalRoute
import com.lingxi.code.terminal.TerminalRouteArgs
import com.lingxi.code.terminal.createTerminalGateway
import com.lingxi.code.theme.AppearancePrefs
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.LocaleWrapper
import com.lingxi.code.theme.ThemeMode
import kotlinx.coroutines.launch

/**
 * The single Activity host. Edge-to-edge, Compose-only — everything else is
 * composed under [MainTabShell]: the Chats tab ([RootScreen]: drawer, conversation,
 * voice overlay), the Bots, Teams and Settings tabs. Onboarding, the terminal and
 * the import preview are overlays above the shell.
 *
 * Theme + accent are read from the DataStore-backed [AppearanceStore] and fed
 * into [LingXiTheme], so the appearance is persisted and live. Later phases fill
 * in [RootScreen] and let the Appearance page mutate the same store.
 */
class MainActivity : ComponentActivity() {
    private val cameraOwner = Any()
    private val mediaPermissionOwner = Any()
    private val locationOwner = Any()
    private val cameraResultKey = java.util.UUID.randomUUID().toString()
    private val locationResultKey = java.util.UUID.randomUUID().toString()
    private lateinit var deviceReadPermissions: DeviceReadPermissionController
    private val linkInbox: com.lingxi.code.navigation.DeepLinkInbox by viewModels()
    private fun enqueueCanonicalLink(intent: Intent, restoringActivity: Boolean = false) {
        if (intent.getBooleanExtra(com.lingxi.code.conversation.ConversationNotificationRoute.EXTRA_NOTIFICATION_OWNER, false)) {
            if (!restoringActivity) {
                com.lingxi.code.conversation.ConversationNotificationRoute.parse(intent)?.let {
                    pendingConversationLaunch.value = it
                }
            }
            return
        }
        val raw = intent.dataString
        if (raw == null) return
        val notice = when {
            com.lingxi.code.navigation.DeepLink.parse(raw) == null -> getString(R.string.deep_link_invalid)
            !linkInbox.deliver(raw, restoringActivity) -> getString(R.string.deep_link_queue_full)
            else -> null
        }
        notice?.let { android.widget.Toast.makeText(this, it, android.widget.Toast.LENGTH_LONG).show() }
    }
    private val pendingConversationLaunch =
        kotlinx.coroutines.flow.MutableStateFlow<ConversationLaunchRequest?>(null)

    // Apply the persisted in-app language to the base context before the
    // activity (and its resources) are created, so the whole surface renders
    // in the chosen locale. On selection the Language page persists + recreates.
    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(LocaleWrapper.wrap(newBase))
    }

    override fun onResume() {
        super.onResume()
        com.lingxi.code.remote.RemoteControl.foreground()
        com.lingxi.code.push.PushRegistration.enqueue(applicationContext)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Register process-owned app visibility before this Activity starts;
        // the first Compose composition can occur after onStart/onResume.
        com.lingxi.code.apps.UnifiedLocalApps.get(applicationContext)
        // Register the camera/picker launchers before the Activity is STARTED and
        // hand them to the process-global CameraController, which the UniFFI
        // AndroidCamera adapter drives across the FFI seam (the device-vision
        // analog of how the mic is invoked for STT). Must run before super/
        // setContent so registerForActivityResult is valid.
        // Unique registration keys stop saved old OS results being replayed
        // into a replacement Activity's new operation.
        val cameraPermLauncher = activityResultRegistry.register(
            "$cameraResultKey:permission", this, ActivityResultContracts.RequestPermission(),
        ) { granted -> CameraController.onCameraPermission(cameraOwner, granted) }
        val mediaPermissionLauncher = activityResultRegistry.register(
            "$cameraResultKey:media-permissions", this, ActivityResultContracts.RequestMultiplePermissions(),
        ) { grants -> AndroidMediaPermissionCoordinator.shared.onPermissionResult(mediaPermissionOwner, grants) }
        val takePictureLauncher = activityResultRegistry.register(
            "$cameraResultKey:picture", this, ActivityResultContracts.TakePicturePreview(),
        ) { bitmap -> CameraController.onPictureTaken(cameraOwner, bitmap) }
        val pickMediaLauncher = activityResultRegistry.register(
            "$cameraResultKey:media", this, ActivityResultContracts.PickVisualMedia(),
        ) { uri -> CameraController.onMediaPicked(cameraOwner, uri) }
        // Android 13+ (TIRAMISU) gates posting on the POST_NOTIFICATIONS runtime
        // permission — declaring it in the manifest is NOT enough at targetSdk 33+.
        // Without this request the permission stays denied and EVERY notification
        // (the engine `notification` tool AND the cron result / foreground-service
        // notifications) silently fails. Registered before STARTED; launched below.
        val notificationPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { com.lingxi.code.push.PushRegistration.enqueue(applicationContext) }
        val locationPermLauncher = activityResultRegistry.register(
            "$locationResultKey:permission", this, ActivityResultContracts.RequestMultiplePermissions(),
        ) { grants -> LocationController.onLocationPermission(locationOwner, grants) }
        val deviceReadPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { granted -> deviceReadPermissions.onPermissionResult(granted) }
        deviceReadPermissions = DeviceReadPermissionController(
            isGranted = { permission ->
                ContextCompat.checkSelfPermission(applicationContext, permission) ==
                    PackageManager.PERMISSION_GRANTED
            },
            isForeground = { lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED) },
            requestPermission = { permission -> deviceReadPermLauncher.launch(permission) },
        )
        CameraController.attach(
            CameraController.makeLaunchers(
                owner = cameraOwner,
                context = applicationContext,
                requestCameraPermission = cameraPermLauncher,
                takePicture = takePictureLauncher,
                pickMedia = pickMediaLauncher,
            ),
        )
        AndroidMediaPermissionCoordinator.shared.attach(
            owner = mediaPermissionOwner,
            isGranted = { permission ->
                ContextCompat.checkSelfPermission(applicationContext, permission) == PackageManager.PERMISSION_GRANTED
            },
            isForeground = { lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED) },
            requestPermissions = { permissions -> mediaPermissionLauncher.launch(permissions) },
        )
        // Device-share: hand the application Context to the process-global
        // ShareController, which the UniFFI AndroidShare adapter drives across
        // the FFI seam to launch the system share sheet (the device analog of
        // how the camera/picker is invoked, but with no ActivityResult to await).
        ShareController.attach(applicationContext)
        // Device-notifications: hand the application Context to the process-global
        // NotificationController, which the UniFFI AndroidNotification adapter
        // drives across the FFI seam to post to the system NotificationManager
        // (engine-driven through tool-notification; no UI affordance).
        NotificationController.attach(applicationContext)
        // Device-clipboard: hand the application Context to the process-global
        // ClipboardController, which the UniFFI AndroidClipboard adapter drives
        // across the FFI seam to read/write the system ClipboardManager
        // (engine-driven through tool-clipboard; no UI affordance). No manifest
        // permission is required for clipboard access.
        ClipboardController.attach(applicationContext)
        AndroidDeviceControlController.attach(applicationContext, deviceReadPermissions)
        // Device-location: the engine capability gate runs first; this controller
        // then owns Android's fine/coarse runtime permission and one-shot fix.
        LocationController.attach(
            LocationController.makeLaunchers(
                owner = locationOwner,
                context = applicationContext,
                requestPermission = locationPermLauncher,
            ),
        )
        // One flavor-resolved native-offload host. Protected operations remain
        // fail-closed until the engine's existing permission chain authorizes
        // the invocation; Direct and Play expose different compile-time catalogs.
        NativeOffloadRuntime.attach(applicationContext)
        // Offline voice-model downloader — process-global so a language-pack
        // download started in the setup wizard survives leaving that step.
        com.lingxi.code.voice.offline.VoiceModelDownloader.attach(applicationContext)

        // Cron: re-arm the next scheduled-task alarm on every launch. Exact alarms
        // do not survive process death / app updates, so arming here self-heals the
        // schedule and picks up jobs created via chat. Off the main thread (it
        // builds a transient engine to query the next fire) and best-effort.
        lifecycleScope.launch(kotlinx.coroutines.Dispatchers.Default) {
            runCatching { CronAlarmScheduler.armNext(applicationContext) }
        }

        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        enqueueCanonicalLink(intent, restoringActivity = savedInstanceState != null)

        // Request POST_NOTIFICATIONS on Android 13+ if not already granted (launched
        // AFTER super.onCreate so the ActivityResultRegistry can dispatch). The OS
        // shows the dialog only when undecided; an already-granted or
        // permanently-denied state returns immediately with no prompt, so launching
        // on every start is safe. Notifications stay denied (and silent) until this.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                this,
                Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            val store = remember { AppearanceStore(applicationContext) }
            val settingsStore: SettingsStore =
                viewModel(factory = SettingsStore.factory(applicationContext))
            val settingsState by settingsStore.state.collectAsState()
            val canonicalLinks by linkInbox.links.collectAsState()
            val activityLifecycle by lifecycle.currentStateFlow.collectAsState()
            val requestedConversationLaunch by pendingConversationLaunch.collectAsState()
            val scope = rememberCoroutineScope()
            val prefs by store.prefs.collectAsState(initial = AppearancePrefs())
            val darkTheme = when (prefs.themeMode) {
                ThemeMode.Dark -> true
                ThemeMode.Light -> false
                ThemeMode.System -> isSystemInDarkTheme()
            }
            // The app's theme can differ from the system's; the bar icons
            // follow the app, or a dark app on a light phone hides the clock.
            LaunchedEffect(darkTheme) {
                val style = if (darkTheme) {
                    SystemBarStyle.dark(Color.TRANSPARENT)
                } else {
                    SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
                }
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }
            // The app shell: four tabs (Chats / Bots / Teams / Settings) and what they ask of each
            // other. Links and shortcuts drive it; the terminal, onboarding and the import preview
            // stay overlays above it.
            val tabState = rememberTabShellState()
            // Monotonic across configuration changes: rotation must not turn a
            // prior reconnect generation back into zero and rebuild a live engine.
            var engineReconnect by rememberSaveable { mutableIntStateOf(0) }
            val desktopProjectStore: com.lingxi.code.project.ProjectStore = viewModel(key = "projects", factory = com.lingxi.code.project.ProjectStore.factory(applicationContext))
            var activeConversationBusy by remember { mutableStateOf(false) }
            var activeConversationSource by remember { mutableStateOf<com.lingxi.code.conversation.ConversationSource?>(null) }
            // 设置页请求的项目切换。宿主持有请求、RootScreen 消费后回调清除 ——
            // 与下面 pendingConversationLaunch 同一个形状。
            var pendingProjectSwitch by remember { mutableStateOf<String?>(null) }
            var engineRuntimeMode by rememberSaveable {
                mutableStateOf(settingsState.linuxRuntime.selectedMode.name)
            }
            var terminalOpen by rememberSaveable { mutableStateOf(false) }
            var terminalSessionId by rememberSaveable { mutableStateOf("interactive") }
            var terminalInitCommand by rememberSaveable { mutableStateOf<String?>(null) }
            var terminalCwd by rememberSaveable { mutableStateOf("") }
            var terminalOwnerKey by rememberSaveable { mutableStateOf("") }
            var terminalLaunchGeneration by rememberSaveable { mutableIntStateOf(0) }
            val openTerminal: (String, String?, String, String) -> Unit = { sessionId, initCommand, cwd, ownerKey ->
                terminalSessionId = sessionId.ifBlank { "interactive" }
                terminalInitCommand = initCommand
                terminalCwd = cwd
                terminalOwnerKey = ownerKey
                terminalLaunchGeneration += 1
                terminalOpen = true
                // The terminal belongs to Chats (decision D16): closing it lands on the chat.
                tabState.select(AppTab.Chats)
            }
            var preparedNativeLink by remember { mutableStateOf<com.lingxi.code.navigation.DeepLink?>(null) }
            var preparedNativeSource by remember { mutableStateOf<com.lingxi.code.conversation.ConversationSource?>(null) }
            var requestedFeatureLink by remember { mutableStateOf<com.lingxi.code.navigation.DeepLink?>(null) }
            var deepLinkBlocked by remember { mutableStateOf(false) }
            var pendingImportRaw by rememberSaveable { mutableStateOf<String?>(null) }
            var botMarketHandoffRaw by rememberSaveable { mutableStateOf<String?>(null) }
            val botMarketHandoff = com.lingxi.code.navigation.DeepLink.parse(botMarketHandoffRaw)
            val pendingImport = com.lingxi.code.navigation.DeepLink.parse(pendingImportRaw)
            LaunchedEffect(canonicalLinks, terminalOpen, requestedFeatureLink, pendingImport, botMarketHandoffRaw, prefs.setupDone, activeConversationSource, activeConversationBusy, activityLifecycle, deepLinkBlocked) {
                val raw = linkInbox.next(com.lingxi.code.navigation.DeepLinkReadiness(
                    foreground = activityLifecycle.isAtLeast(Lifecycle.State.RESUMED),
                    onboardingComplete = prefs.setupDone,
                    sourceBound = activeConversationSource != null,
                    engineBusy = activeConversationBusy,
                    // Settings is a tab now, not a blocking surface (an unsaved edit is still guarded by its own page).
                    blockingSurfaceOpen = terminalOpen || requestedFeatureLink != null || pendingImport != null || botMarketHandoff != null || deepLinkBlocked,
                ))
                val link = com.lingxi.code.navigation.DeepLink.parse(raw)
                if (link != null) {
                    val parts = link.route.split('/')
                    val id = parts.last()
                    var handled = true
                    var awaitingFeature = false
                    // A link selects its tab first (an import preview stays on the current one); the route's
                    // own handler then runs inside that tab.
                    val landingTab = TabLogic.tabForLink(link.route).tab
                    when {
                        link.route.startsWith("market/bot/") -> { botMarketHandoffRaw = raw; com.lingxi.code.apps.BotMarketHandoff(parts[2], link.params["versionId"], link.params["intentId"]).save(applicationContext) }
                        link.route in setOf("team/join", "team/inbox") -> tabState.requestTeams(link.params["code"])
                        parts.first() == "remote" -> com.lingxi.code.remote.RemoteRoute.fromLink(link)
                            ?.let { tabState.requestBots(it.hostId, it.sessionId) } ?: run { handled = false }
                        parts.first() == "friend" -> com.lingxi.code.friends.FriendLink.from(link)?.let(tabState::requestFriends) ?: run { handled = false }
                        link.params["workspace"] != null && parts.first() in setOf("settings", "provider", "extensions", "terminal") && preparedNativeLink != link -> { requestedFeatureLink = link; awaitingFeature = true }
                        // Chats routes, including the scheduled-task pages (decision D14), are RootScreen's.
                        parts.first() in setOf("session", "new", "project", "apps", "market", "runtime", "import", "cron") -> {
                            landingTab?.let(tabState::select)
                            requestedFeatureLink = link
                            awaitingFeature = true
                        }
                        parts.first() == "terminal" -> {
                            if (link.params["workspace"] == null) {
                                val cwd = com.lingxi.code.project.MobileUserStorage.directories(applicationContext).configGuestPath
                                openTerminal(id, link.payload["command"] as? String, cwd, "runtime-link")
                            } else {
                                val source = preparedNativeSource
                                val workspace = source?.workspaceState?.value
                                val activation = source?.activeSessionState?.value
                                if (source == null || source !== activeConversationSource ||
                                    source.recoverySpec?.workspaceKey != link.params["workspace"] ||
                                    workspace == null || activation == null || workspace.cwd.isBlank() ||
                                    com.lingxi.code.model.canonicalSessionId(workspace.sessionId) !=
                                    com.lingxi.code.model.canonicalSessionId(activation.sessionId)) {
                                    handled = false
                                } else {
                                    val owner = com.lingxi.code.model.canonicalSessionId(activation.sessionId)
                                    openTerminal(id, link.payload["command"] as? String, workspace.cwd, "conversation:$owner")
                                }
                            }
                        }
                        parts.first() == "settings" -> {
                            val pages = mapOf("general" to SettingsRoutes.GENERAL, "account" to SettingsRoutes.ACCOUNT, "appearance" to SettingsRoutes.APPEARANCE, "notifications" to SettingsRoutes.NOTIFICATIONS, "voice" to SettingsRoutes.VOICE, "custom-providers" to SettingsRoutes.CUSTOM_PROVIDERS, "fusion" to SettingsRoutes.FUSION, "permissions" to SettingsRoutes.ENGINE_PERMISSIONS, "tools-agent" to SettingsRoutes.TOOLS_AGENT, "skills" to SettingsRoutes.ENGINE_SKILLS, "mcp" to SettingsRoutes.ENGINE_MCP, "hooks" to SettingsRoutes.HOOKS, "plugins" to SettingsRoutes.PLUGINS, "diagnostics" to SettingsRoutes.DIAGNOSTICS, "about" to SettingsRoutes.ABOUT, "projects" to SettingsRoutes.PROJECTS, "archived-chats" to SettingsRoutes.ARCHIVED, "language" to SettingsRoutes.LANGUAGE, "input" to SettingsRoutes.INPUT, "privacy" to SettingsRoutes.PRIVACY, "permission-mode" to SettingsRoutes.PERMISSION_MODE, "open-source" to SettingsRoutes.OPEN_SOURCE, "linux-runtime" to SettingsRoutes.LINUX_RUNTIME, "typescript-lsp" to SettingsRoutes.TYPESCRIPT_LSP, "computer-use" to SettingsRoutes.COMPUTER_USE, "dream" to SettingsRoutes.DREAM, "provider-credentials" to SettingsRoutes.PROVIDER_ACCOUNTS)
                            pages[id]?.let { tabState.openSettings(it) } ?: run { handled = false }
                        }
                        parts.first() == "provider" -> tabState.openSettings(SettingsRoutes.providerEdit(ProviderKind.Llm.name, android.net.Uri.encode(id)))
                        parts.first() == "extensions" -> tabState.openSettings(
                            when (parts[1]) {
                                "plugin" -> SettingsRoutes.pluginDetail(id)
                                "mcp" -> SettingsRoutes.mcpEdit(android.net.Uri.encode(id), link.params["scope"])
                                else -> SettingsRoutes.skillDetail(android.net.Uri.encode(id))
                            },
                        )
                        else -> handled = false
                    }
                    if (!handled) android.widget.Toast.makeText(applicationContext, "This link destination is unavailable in this build", android.widget.Toast.LENGTH_LONG).show()
                    if (!awaitingFeature) { preparedNativeLink = null; preparedNativeSource = null; raw?.let(linkInbox::complete) }
                }
            }
            LaunchedEffect(requestedConversationLaunch) {
                if (requestedConversationLaunch != null) {
                    tabState.select(AppTab.Chats)
                    terminalOpen = false
                }
            }
            LaunchedEffect(settingsState.linuxRuntime.selectedMode) {
                val selected = settingsState.linuxRuntime.selectedMode
                if (engineRuntimeMode != selected.name) {
                    if (engineRuntimeMode ==
                        com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux.name
                    ) {
                        runCatching {
                            com.lingxi.code.settings.LinuxRuntimeBridge.shutdown(
                                applicationContext,
                                com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux,
                            )
                        }
                    }
                    engineRuntimeMode = selected.name
                    // Replacing the retained conversation source closes the old
                    // engine; runtime shutdown above reaps its PTYs/background jobs.
                    engineReconnect += 1
                    terminalOpen = false
                }
            }
            // 引擎就绪后登记 LingXi 的插件市场（只加市场、不装插件，所以不用重连），
            // 让设置 → 插件里能装它的插件；用户删掉这个市场后不再加回。
            val lingxiMarketplace = remember { com.lingxi.code.plugins.LingxiMarketplaceSeeder(applicationContext) }
            LaunchedEffect(activeConversationSource) {
                activeConversationSource?.let { lingxiMarketplace.ensure(it) }
            }

            // A scheduled run's session was chosen on the scheduled-task page: the chat opens it.
            val openScheduledSession: (com.lingxi.code.cron.CronRunRecord) -> Unit = { run ->
                run.sessionId?.let { sessionId ->
                    scope.launch {
                        val workspace = run.sessionScopeKey
                        val id = com.lingxi.code.model.canonicalSessionId(sessionId)
                        val receiptMode = com.lingxi.code.model.SessionMode.entries.firstOrNull { it.wireValue == run.sessionMode }
                        val owners = runCatching {
                            com.lingxi.code.project.ScopeStateStore(applicationContext).let { store ->
                                store.validate()
                                store.readSessionOwners()
                            }
                        }.getOrElse {
                            android.widget.Toast.makeText(applicationContext, getString(R.string.chat_resume_session_failed), android.widget.Toast.LENGTH_LONG).show()
                            return@launch
                        }
                        val owner = owners[id]
                        if (workspace.isNullOrBlank() || receiptMode == null || run.sessionTranscriptPath.isNullOrBlank() ||
                            owner != com.lingxi.code.project.ScopeStateStore.SessionOwner(workspace, receiptMode)) {
                            android.widget.Toast.makeText(applicationContext, getString(R.string.chat_resume_session_failed), android.widget.Toast.LENGTH_LONG).show()
                            return@launch
                        }
                        pendingConversationLaunch.value = ConversationLaunchRequest(
                            sessionId = sessionId, turnId = null,
                            workspaceKey = workspace, sessionMode = receiptMode,
                            transcriptPath = run.sessionTranscriptPath,
                        )
                        tabState.select(AppTab.Chats)
                    }
                }
            }

            LingXiTheme(darkTheme = darkTheme, accentId = prefs.accentId) {
                Box(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg)) {
                    MainTabShell(
                        state = tabState,
                        // Full-screen overlays drawn above the shell cover its bar (and rail).
                        overlayOpen = !prefs.setupDone || terminalOpen || pendingImport != null || botMarketHandoff != null,
                        chats = {
                            RootScreen(
                                isDark = darkTheme,
                                tabState = tabState,
                                // Every shortcut into Settings remembers Chats as the way back.
                                onOpenPermissionSettings = {
                                    tabState.openSettings(SettingsRoutes.PERMISSION_MODE, AppTab.Chats)
                                },
                                onOpenSettings = {
                                    tabState.openSettings(SettingsRoutes.MAIN, AppTab.Chats)
                                },
                                onOpenAccount = {
                                    tabState.openSettings(SettingsRoutes.ACCOUNT, AppTab.Chats)
                                },
                                onOpenModelSettings = {
                                    tabState.openSettings(SettingsRoutes.providerList(ProviderKind.Llm.name), AppTab.Chats)
                                },
                                onOpenProviderSettings = { providerSettingsId ->
                                    tabState.openSettings(
                                        providerSettingsId
                                            ?.let { SettingsRoutes.providerEdit(ProviderKind.Llm.name, it) }
                                            ?: SettingsRoutes.providerList(ProviderKind.Llm.name),
                                        AppTab.Chats,
                                    )
                                },
                                onOpenVoiceSettings = {
                                    tabState.openSettings(SettingsRoutes.VOICE, AppTab.Chats)
                                },
                                onOpenComputerUseSettings = {
                                    tabState.openSettings(SettingsRoutes.COMPUTER_USE, AppTab.Chats)
                                },
                                onOpenTerminal = openTerminal,
                                onOpenBotsTab = { tabState.select(AppTab.Bots) },
                                onOpenScheduledSession = openScheduledSession,
                                onChromeChanged = tabState::reportChatsChrome,
                                modelSetupRequired = settingsState.needsLlmSetup,
                                assistantName = prefs.assistantName,
                                inputDialog = prefs.inputDialog,
                                reconnectToken = engineReconnect,
                                settingsStore = settingsStore,
                                onConversationSourceChanged = { activeConversationSource = it },
                                requestedProjectSwitch = pendingProjectSwitch,
                                onProjectSwitchHandled = { pendingProjectSwitch = null },
                                onConversationBusyChanged = { activeConversationBusy = it },
                                requestedDeepLink = requestedFeatureLink,
                                onDeepLinkBlockingChanged = { deepLinkBlocked = it },
                                onDeepLinkNativeReady = { link, source -> preparedNativeSource = source; preparedNativeLink = link },
                                onDeepLinkImportReady = { link ->
                                    pendingImportRaw = com.lingxi.code.navigation.DeepLink.build(link.route, link.params, link.payload)
                                },
                                onDeepLinkHandled = { handledLink ->
                                    if (requestedFeatureLink == handledLink) {
                                        if (pendingImportRaw == null && preparedNativeLink == null) canonicalLinks.firstOrNull()?.let(linkInbox::complete)
                                        requestedFeatureLink = null
                                    }
                                },
                                requestedConversationLaunch = requestedConversationLaunch,
                                onConversationLaunchHandled = { pendingConversationLaunch.value = null },
                            )
                        },
                        settings = { body ->
                            SettingsTab(
                                state = tabState,
                                appearanceStore = store,
                                engineSource = activeConversationSource,
                                projectStore = desktopProjectStore,
                                store = settingsStore,
                                isDark = darkTheme,
                                accentId = prefs.accentId,
                                // 「在这里换项目」要真的把引擎换过去，而不是只挪一个记号：
                                // 项目层写进哪个目录由引擎进程的 cwd 决定。回到对话页、
                                // 把请求交给 RootScreen 的 switchEngineScope，与抽屉里换项目
                                // 走的是同一条路径。
                                onSwitchProject = { projectId ->
                                    pendingProjectSwitch = projectId
                                    tabState.select(AppTab.Chats)
                                },
                                // 关于 → 重新观看引导: clear setupDone (replays the wizard)
                                // and drop back to the conversation behind it.
                                onReplayOnboarding = {
                                    scope.launch { store.setSetupDone(false) }
                                    tabState.select(AppTab.Chats)
                                },
                                // 重新连接引擎: rebuild the engine against the just-saved key
                                // and drop back to the (now-real) conversation.
                                onReconnectEngine = {
                                    if (activeConversationBusy) {
                                        android.widget.Toast.makeText(applicationContext, getString(R.string.voice_session_busy_retry), android.widget.Toast.LENGTH_SHORT).show()
                                        false
                                    } else {
                                        engineReconnect += 1
                                        tabState.select(AppTab.Chats)
                                        true
                                    }
                                },
                                onOpenTerminal = {
                                    val cwd = com.lingxi.code.project.MobileUserStorage.directories(applicationContext).configGuestPath
                                    openTerminal(it.sessionId, it.initCommand, cwd, "runtime-settings")
                                },
                                onPermissionModeChanged = { mode ->
                                    val source = activeConversationSource
                                        ?: error("engine is not connected")
                                    source.setPermissionMode(mode)
                                },
                                onTypescriptLspModeChanged = { mode ->
                                    val source = activeConversationSource
                                        ?: error("engine is not connected")
                                    source.submitClientCommand(
                                        com.lingxi.code.bindings.client.ClientCommand.SetTypescriptLspMode(mode),
                                    )
                                },
                                modifier = body,
                            )
                        },
                    )

                    botMarketHandoff?.let { handoff ->
                        androidx.activity.compose.BackHandler { botMarketHandoffRaw = null }
                        androidx.compose.material3.Surface(modifier = Modifier.fillMaxSize()) {
                            com.lingxi.code.apps.BotMarketHandoffPage(handoff.route.split('/')[2], handoff.params["versionId"], handoff.params["intentId"], onBack = { botMarketHandoffRaw = null })
                        }
                    }
                    // The import preview of a lingxi://import link: above whichever tab is showing.
                    DeepLinkImportOverlay(
                        link = pendingImport,
                        engineSource = activeConversationSource,
                        onDismiss = {
                            canonicalLinks.firstOrNull()?.let(linkInbox::complete)
                            pendingImportRaw = null
                        },
                    )

                    // First-run setup wizard — shown until onboarding completes;
                    // sits above the tabs so the replay path covers them too. On
                    // finish it only flips setupDone.
                    SetupWizardOverlay(
                        visible = !prefs.setupDone,
                        onFinish = {
                            scope.launch {
                                store.setSetupDone(true)
                            }
                        },
                    )

                    AnimatedVisibility(
                        visible = terminalOpen,
                        enter = slideInVertically(initialOffsetY = { it }),
                        exit = slideOutVertically(targetOffsetY = { it }),
                    ) {
                        if (terminalCwd.isNotBlank() && terminalOwnerKey.isNotBlank()) key(terminalLaunchGeneration) {
                            val terminalGateway = remember(
                                terminalLaunchGeneration,
                                settingsState.linuxRuntime.selectedMode,
                            ) {
                                createTerminalGateway(
                                    applicationContext,
                                    settingsState.linuxRuntime.selectedMode,
                                    terminalCwd,
                                    terminalOwnerKey,
                                )
                            }
                            TerminalRoute(
                                args = TerminalRouteArgs(
                                    sessionId = terminalSessionId,
                                    cwd = terminalCwd,
                                    initCommand = terminalInitCommand,
                                ),
                                instanceKey = com.lingxi.code.terminal.terminalOwnerViewKey(
                                    terminalSessionId, terminalOwnerKey, terminalCwd, settingsState.linuxRuntime.selectedMode.name,
                                ),
                                launchGeneration = terminalLaunchGeneration,
                                gateway = terminalGateway,
                                onBack = { terminalOpen = false },
                                onOpenUrl = { url ->
                                    val uri = runCatching { android.net.Uri.parse(url) }.getOrNull()
                                    if (uri?.scheme in setOf("http", "https")) {
                                        runCatching {
                                            startActivity(Intent(Intent.ACTION_VIEW, uri))
                                        }
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }
    }

    override fun onStop() {
        if (!isChangingConfigurations) com.lingxi.code.remote.RemoteControl.background()
        super.onStop()
    }

    override fun onDestroy() {
        // Drop the launcher references so a finishing Activity can't be leaked by
        // the process-global controller and any in-flight capture is cancelled.
        CameraController.detach(cameraOwner)
        AndroidMediaPermissionCoordinator.shared.detach(mediaPermissionOwner)
        ShareController.detach()
        NotificationController.detach()
        ClipboardController.detach()
        AndroidDeviceControlController.detach(deviceReadPermissions)
        LocationController.detach(locationOwner)
        NativeOffloadRuntime.detach()
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        enqueueCanonicalLink(intent)
    }
}
