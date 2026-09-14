package com.lingxi.code

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import com.lingxi.code.vision.CameraController
import com.lingxi.code.share.ShareController
import com.lingxi.code.notify.NotificationController
import com.lingxi.code.clipboard.ClipboardController
import com.lingxi.code.device.AndroidDeviceControlController
import com.lingxi.code.location.LocationController
import com.lingxi.code.voice.recorder.RecorderController
import com.lingxi.code.offload.NativeOffloadRuntime
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
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.cron.CronAlarmScheduler
import com.lingxi.code.onboarding.SetupWizardOverlay
import com.lingxi.code.settings.SettingsHost
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.conversation.ConversationLaunchRequest
import com.lingxi.code.conversation.ConversationNotificationRoute
import com.lingxi.code.terminal.TerminalRoute
import com.lingxi.code.terminal.TerminalRouteArgs
import com.lingxi.code.terminal.createTerminalGateway
import com.lingxi.code.theme.AppearancePrefs
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.LocaleWrapper
import com.lingxi.code.theme.ThemeMode
import com.lingxi.code.localapps.widget.LocalAppLaunchRequest
import com.lingxi.code.localapps.widget.LocalAppWidgetDeepLink
import kotlinx.coroutines.launch

/**
 * The single Activity host. Edge-to-edge, Compose-only — everything else
 * (drawer, conversation, settings nav graph, voice overlay) is composed under
 * [RootScreen].
 *
 * Theme + accent are read from the DataStore-backed [AppearanceStore] and fed
 * into [LingXiTheme], so the appearance is persisted and live. Later phases fill
 * in [RootScreen] and let the Appearance page mutate the same store.
 */
class MainActivity : ComponentActivity() {
    private val pendingCronRunId = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
    private val pendingTerminalArgs =
        kotlinx.coroutines.flow.MutableStateFlow<TerminalRouteArgs?>(null)
    private val pendingConversationLaunch =
        kotlinx.coroutines.flow.MutableStateFlow<ConversationLaunchRequest?>(null)
    private val pendingLocalAppLaunch =
        kotlinx.coroutines.flow.MutableStateFlow<LocalAppLaunchRequest?>(null)
    private val pendingOpenLocalApps = kotlinx.coroutines.flow.MutableStateFlow(false)

    // Apply the persisted in-app language to the base context before the
    // activity (and its resources) are created, so the whole surface renders
    // in the chosen locale. On selection the Language page persists + recreates.
    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(LocaleWrapper.wrap(newBase))
    }

    @Suppress("DEPRECATION")
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (
            level == android.content.ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW ||
            level == android.content.ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL ||
            level == android.content.ComponentCallbacks2.TRIM_MEMORY_COMPLETE
        ) {
            com.lingxi.code.localapps.LocalAppsMemoryPressure.notifyPressure()
        }
    }

    override fun onLowMemory() {
        super.onLowMemory()
        com.lingxi.code.localapps.LocalAppsMemoryPressure.notifyPressure()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        pendingCronRunId.value = intent.getStringExtra(
            com.lingxi.code.cron.CronNotifications.EXTRA_CRON_RUN_ID,
        )
        pendingTerminalArgs.value = intent.terminalRouteArgs()
        pendingConversationLaunch.value = ConversationNotificationRoute.parse(intent)
        pendingLocalAppLaunch.value = LocalAppWidgetDeepLink.parse(intent)
        pendingOpenLocalApps.value = intent.getBooleanExtra(
            LocalAppWidgetDeepLink.EXTRA_OPEN_LOCAL_APPS,
            false,
        )
        // Register the camera/picker launchers before the Activity is STARTED and
        // hand them to the process-global CameraController, which the UniFFI
        // AndroidCamera adapter drives across the FFI seam (the device-vision
        // analog of how the mic is invoked for STT). Must run before super/
        // setContent so registerForActivityResult is valid.
        val cameraPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { granted -> CameraController.onCameraPermission(granted) }
        val takePictureLauncher = registerForActivityResult(
            ActivityResultContracts.TakePicturePreview(),
        ) { bitmap -> CameraController.onPictureTaken(bitmap) }
        val pickMediaLauncher = registerForActivityResult(
            ActivityResultContracts.PickVisualMedia(),
        ) { uri -> CameraController.onMediaPicked(uri) }
        // Android 13+ (TIRAMISU) gates posting on the POST_NOTIFICATIONS runtime
        // permission — declaring it in the manifest is NOT enough at targetSdk 33+.
        // Without this request the permission stays denied and EVERY notification
        // (the engine `notification` tool AND the cron result / foreground-service
        // notifications) silently fails. Registered before STARTED; launched below.
        val notificationPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestPermission(),
        ) { /* best-effort: nothing to do on grant/deny — posting self-gates */ }
        val locationPermLauncher = registerForActivityResult(
            ActivityResultContracts.RequestMultiplePermissions(),
        ) { grants -> LocationController.onLocationPermission(grants) }
        CameraController.attach(
            CameraController.makeLaunchers(
                context = applicationContext,
                requestCameraPermission = cameraPermLauncher,
                takePicture = takePictureLauncher,
                pickMedia = pickMediaLauncher,
            ),
        )
        // Device-share: hand the application Context to the process-global
        // ShareController, which the UniFFI AndroidShare adapter drives across
        // the FFI seam to launch the system share sheet (the device analog of
        // how the camera/picker is invoked, but with no ActivityResult to await).
        ShareController.attach(applicationContext)
        // Device-voice: hand the application Context to the process-global
        // RecorderController, which the UniFFI AndroidVoice adapter drives across
        // the FFI seam to run a MediaRecorder mic session (engine-driven through
        // tool-voice; no UI button, unlike the STT hold-to-talk path).
        RecorderController.attach(applicationContext)
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
        AndroidDeviceControlController.attach(applicationContext)
        // Device-location: the engine capability gate runs first; this controller
        // then owns Android's fine/coarse runtime permission and one-shot fix.
        LocationController.attach(
            LocationController.makeLaunchers(
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
            runCatching {
                com.lingxi.code.localapps.LocalAppBackgroundScheduler
                    .ensureWatchdog(applicationContext)
            }
        }

        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

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
            val requestedCronRunId by pendingCronRunId.collectAsState()
            val requestedTerminalArgs by pendingTerminalArgs.collectAsState()
            val requestedConversationLaunch by pendingConversationLaunch.collectAsState()
            val requestedLocalAppLaunch by pendingLocalAppLaunch.collectAsState()
            val requestedOpenLocalApps by pendingOpenLocalApps.collectAsState()
            val scope = rememberCoroutineScope()
            val prefs by store.prefs.collectAsState(initial = AppearancePrefs())
            val darkTheme = when (prefs.themeMode) {
                ThemeMode.Dark -> true
                ThemeMode.Light -> false
                ThemeMode.System -> isSystemInDarkTheme()
            }
            // Settings is a full-surface overlay that slides up over the
            // conversation (the Android analog of the iOS settings sheet); the
            // drawer's account row opens it, system-back / close dismisses it.
            var settingsOpen by remember { mutableStateOf(false) }
            var settingsInitialRoute by remember { mutableStateOf(SettingsRoutes.MAIN) }
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
            var terminalLaunchGeneration by rememberSaveable { mutableIntStateOf(0) }
            val openTerminal: (String, String?) -> Unit = { sessionId, initCommand ->
                terminalSessionId = sessionId.ifBlank { "interactive" }
                terminalInitCommand = initCommand
                terminalLaunchGeneration += 1
                terminalOpen = true
                settingsOpen = false
            }
            LaunchedEffect(requestedCronRunId) {
                requestedCronRunId?.let { runId ->
                    settingsInitialRoute = SettingsRoutes.cronRun(runId)
                    settingsOpen = true
                    pendingCronRunId.value = null
                }
            }
            LaunchedEffect(requestedTerminalArgs) {
                requestedTerminalArgs?.let {
                    openTerminal(it.sessionId, it.initCommand)
                    pendingTerminalArgs.value = null
                }
            }
            LaunchedEffect(requestedConversationLaunch) {
                if (requestedConversationLaunch != null) {
                    settingsOpen = false
                    terminalOpen = false
                }
            }
            LaunchedEffect(requestedLocalAppLaunch, requestedOpenLocalApps) {
                if (requestedLocalAppLaunch != null || requestedOpenLocalApps) {
                    settingsOpen = false
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

            LingXiTheme(darkTheme = darkTheme, accentId = prefs.accentId) {
                Box(Modifier.fillMaxSize()) {
                    RootScreen(
                        isDark = darkTheme,
                        onToggleTheme = {
                            scope.launch {
                                store.setThemeMode(if (darkTheme) ThemeMode.Light else ThemeMode.Dark)
                            }
                        },
                        onOpenSettings = {
                            settingsInitialRoute = SettingsRoutes.MAIN
                            settingsOpen = true
                        },
                        onOpenModelSettings = {
                            settingsInitialRoute = SettingsRoutes.providerList(ProviderKind.Llm.name)
                            settingsOpen = true
                        },
                        onOpenProviderSettings = { providerSettingsId ->
                            settingsInitialRoute = providerSettingsId
                                ?.let { SettingsRoutes.providerEdit(ProviderKind.Llm.name, it) }
                                ?: SettingsRoutes.providerList(ProviderKind.Llm.name)
                            settingsOpen = true
                        },
                        onOpenComputerUseSettings = {
                            settingsInitialRoute = SettingsRoutes.COMPUTER_USE
                            settingsOpen = true
                        },
                        onOpenCronSettings = { taskKey ->
                            settingsInitialRoute = SettingsRoutes.cron(taskKey)
                            settingsOpen = true
                        },
                        onOpenTerminal = openTerminal,
                        modelSetupRequired = settingsState.needsLlmSetup,
                        assistantName = prefs.assistantName,
                        inputDialog = prefs.inputDialog,
                        reconnectToken = engineReconnect,
                        settingsStore = settingsStore,
                        onConversationSourceChanged = { activeConversationSource = it },
                        requestedProjectSwitch = pendingProjectSwitch,
                        onProjectSwitchHandled = { pendingProjectSwitch = null },
                        onConversationBusyChanged = { activeConversationBusy = it },
                        requestedConversationLaunch = requestedConversationLaunch,
                        onConversationLaunchHandled = { pendingConversationLaunch.value = null },
                        requestedLocalAppLaunch = requestedLocalAppLaunch,
                        onLocalAppLaunchHandled = { pendingLocalAppLaunch.value = null },
                        openLocalAppsRequest = requestedOpenLocalApps,
                        onOpenLocalAppsHandled = { pendingOpenLocalApps.value = false },
                    )
                    AnimatedVisibility(
                        visible = settingsOpen,
                        enter = slideInVertically(initialOffsetY = { it }),
                        exit = slideOutVertically(targetOffsetY = { it }),
                    ) {
                        SettingsHost(
                            engineSource = activeConversationSource,
                            projectStore = desktopProjectStore,
                            // 「在这里换项目」要真的把引擎换过去，而不是只挪一个记号：
                            // 项目层写进哪个目录由引擎进程的 cwd 决定。关掉设置页、把
                            // 请求交给 RootScreen 的 switchEngineScope，与抽屉里换项目
                            // 走的是同一条路径。
                            onSwitchProject = { projectId ->
                                pendingProjectSwitch = projectId
                                settingsOpen = false
                            },
                            appearanceStore = store,
                            isDark = darkTheme,
                            accentId = prefs.accentId,
                            store = settingsStore,
                            onPermissionModeChanged = { mode ->
                                val source = activeConversationSource
                                    ?: error("engine is not connected")
                                source.setPermissionMode(mode)
                            },
                            onTypescriptLspModeChanged = { mode ->
                                val source = activeConversationSource
                                    ?: error("engine is not connected")
                                source.submitClientCommand(
                                    com.lingxi.code.bindings.ClientCommand.SetTypescriptLspMode(mode),
                                )
                            },
                            onSetLocalAppPluginEnabled = { pluginId, enabled ->
                                activeConversationSource?.submitClientCommand(
                                    com.lingxi.code.bindings.ClientCommand.PluginCommand(
                                        com.lingxi.code.bindings.PluginCommandDto.SetEnabled(
                                            pluginId = pluginId,
                                            enabled = enabled,
                                        ),
                                    ),
                                )
                            },
                            initialRoute = settingsInitialRoute,
                            onClose = {
                                settingsOpen = false
                                settingsInitialRoute = SettingsRoutes.MAIN
                            },
                            // 关于 → 重新观看引导: clear setupDone (replays the wizard)
                            // and drop back to the conversation behind it.
                            onReplayOnboarding = {
                                scope.launch { store.setSetupDone(false) }
                                settingsOpen = false
                            },
                            // 重新连接引擎: rebuild the engine against the just-saved key
                            // and drop back to the (now-real) conversation.
                            onReconnectEngine = {
                                if (activeConversationBusy) {
                                    android.widget.Toast.makeText(applicationContext, getString(R.string.voice_session_busy_retry), android.widget.Toast.LENGTH_SHORT).show()
                                } else {
                                    engineReconnect += 1
                                    settingsOpen = false
                                }
                            },
                            onOpenTerminal = {
                                openTerminal(it.sessionId, it.initCommand)
                            },
                        )
                    }

                    // First-run setup wizard — shown until onboarding completes;
                    // sits above settings so the replay path covers it too. On
                    // finish it persists the chosen profile + flips setupDone.
                    SetupWizardOverlay(
                        visible = !prefs.setupDone,
                        initialAssistantName = prefs.assistantName,
                        initialUserName = prefs.userName,
                        initialVoiceprint = prefs.voiceprint,
                        initialModelId = prefs.defaultModelId,
                        initialVoiceLang = prefs.voiceLang,
                        onFinish = { assistantName, userName, voiceprint, modelId, voiceLang ->
                            scope.launch {
                                val migratedVoiceLanguage = when (voiceLang.lowercase()) {
                                    "zh" -> "zh-CN"
                                    "en" -> "en-US"
                                    else -> settingsState.voice.language
                                }
                                store.setAssistantName(assistantName)
                                store.setUserName(userName)
                                store.setVoiceprint(voiceprint)
                                store.setDefaultModel(modelId)
                                store.setVoiceLang(voiceLang)
                                settingsStore.setVoice(
                                    settingsState.voice.copy(language = migratedVoiceLanguage),
                                )
                                store.setSetupDone(true)
                            }
                        },
                    )

                    AnimatedVisibility(
                        visible = terminalOpen,
                        enter = slideInVertically(initialOffsetY = { it }),
                        exit = slideOutVertically(targetOffsetY = { it }),
                    ) {
                        key(terminalLaunchGeneration) {
                            val terminalGateway = remember(
                                terminalLaunchGeneration,
                                settingsState.linuxRuntime.selectedMode,
                            ) {
                                createTerminalGateway(
                                    applicationContext,
                                    settingsState.linuxRuntime.selectedMode,
                                )
                            }
                            TerminalRoute(
                                args = TerminalRouteArgs(
                                    sessionId = terminalSessionId,
                                    initCommand = terminalInitCommand,
                                ),
                                instanceKey = terminalLaunchGeneration.toString(),
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

    override fun onDestroy() {
        // Drop the launcher references so a finishing Activity can't be leaked by
        // the process-global controller and any in-flight capture is cancelled.
        CameraController.detach()
        ShareController.detach()
        RecorderController.detach()
        NotificationController.detach()
        ClipboardController.detach()
        AndroidDeviceControlController.detach()
        LocationController.detach()
        NativeOffloadRuntime.detach()
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        pendingCronRunId.value = intent.getStringExtra(
            com.lingxi.code.cron.CronNotifications.EXTRA_CRON_RUN_ID,
        )
        pendingTerminalArgs.value = intent.terminalRouteArgs()
        pendingConversationLaunch.value = ConversationNotificationRoute.parse(intent)
        pendingLocalAppLaunch.value = LocalAppWidgetDeepLink.parse(intent)
        pendingOpenLocalApps.value = intent.getBooleanExtra(
            LocalAppWidgetDeepLink.EXTRA_OPEN_LOCAL_APPS,
            false,
        )
    }
}

private fun Intent.terminalRouteArgs(): TerminalRouteArgs? {
    val uri = data ?: return null
    if (uri.scheme != "lingxi" || uri.host != "open_terminal") return null
    return TerminalRouteArgs(
        sessionId = uri.getQueryParameter("sessionId")
            ?.takeIf(String::isNotBlank)
            ?: "interactive",
        initCommand = uri.getQueryParameter("initCommand"),
    )
}
