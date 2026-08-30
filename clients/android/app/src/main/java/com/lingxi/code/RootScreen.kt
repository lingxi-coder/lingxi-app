package com.lingxi.code

import android.content.pm.PackageManager
import android.util.Base64
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.createSavedStateHandle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.BackHandler
import androidx.activity.result.contract.ActivityResultContracts
import com.lingxi.code.conversation.AndroidConversationBackgroundExecution
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ConversationTurnOrigin
import com.lingxi.code.conversation.ConversationTurnOutcome
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.ConversationLaunchRequest
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.conversation.ComputerUseSetupStatus
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.toImageRef
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.conversation.conversationStrings
import com.lingxi.code.conversation.PermissionPromptDialog
import com.lingxi.code.computeruse.ComputerUseApprovalDialog
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseUiState
import com.lingxi.code.connectivity.rememberOnlineState
import com.lingxi.code.connectivity.shouldShowOfflineBanner
import com.lingxi.code.cron.AndroidCronRepository
import com.lingxi.code.cron.CronRunStatus
import com.lingxi.code.cron.CronSchedulingMode
import com.lingxi.code.drawer.DrawerAppScope
import com.lingxi.code.drawer.DrawerContent
import com.lingxi.code.drawer.DrawerProductionData
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.model.SessionCatalogPhase
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.model.withCachedRows
import com.lingxi.code.project.ConflictResolution
import com.lingxi.code.project.CreateProjectDialog
import com.lingxi.code.project.LocalProjectWorkspace
import com.lingxi.code.project.ProjectConflictDialog
import com.lingxi.code.project.ProjectErrorDialog
import com.lingxi.code.project.ProjectOperationKind
import com.lingxi.code.project.ProjectSnapshot
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.project.ProjectStoreState
import com.lingxi.code.project.ScopeStateStore
import com.lingxi.code.project.toDrawerProject
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.theme.LingXiTheme
import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.share.rememberShare
import com.lingxi.code.vision.rememberCameraCapture
import com.lingxi.code.model.Role
import com.lingxi.code.localapps.LocalAppsAction
import com.lingxi.code.localapps.LocalAppsDestination
import com.lingxi.code.localapps.LocalAppsRoute
import com.lingxi.code.localapps.LocalAppsViewModel
import com.lingxi.code.localapps.localAppDisplayName
import com.lingxi.code.localapps.localAppWorkspace
import com.lingxi.code.localapps.localAppsStrings
import com.lingxi.code.localapps.widget.AndroidLocalAppWidgetSnapshotSync
import com.lingxi.code.localapps.widget.LocalAppLaunchRequest
import com.lingxi.code.localapps.widget.LocalAppWidgetPinRequester
import com.lingxi.code.model.sessionCatalogStrings
import com.lingxi.code.voice.FlowModeOverlay
import com.lingxi.code.voice.VoiceFlowOverlay
import com.lingxi.code.voice.cancelActiveHeldVoiceSession
import com.lingxi.code.voice.rememberOrbVoiceListen
import com.lingxi.code.voice.rememberVoiceCapture
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import android.graphics.BitmapFactory
import android.widget.Toast
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.launch
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withTimeoutOrNull

/**
 * How long the created-app hand-off retries a refused scope switch, and how
 * long it then waits for the app's session to come up. Both are BOUNDED: the
 * landing is a one-shot Channel element, so an unbounded wait strands it and
 * every later landing behind it.
 */
private const val LANDING_SWITCH_ATTEMPTS = 40
private const val LANDING_SWITCH_RETRY_MS = 250L
private const val SESSION_READY_TIMEOUT_MS = 20_000L

/**
 * A conversation notification is usually tapped on a COLD start, so the first
 * attempt can land before the engine source is bound. Retry on the same budget
 * shape as the created-app landing, then report instead of dropping the route.
 */
private const val CONVERSATION_LAUNCH_ATTEMPTS = 40
private const val CONVERSATION_LAUNCH_RETRY_MS = 250L

/**
 * Root composable for the app shell.
 *
 * Wraps the conversation surface in a Material 3 [ModalNavigationDrawer] that
 * hosts the 对话/项目/定时 [DrawerContent] (the Android analog of the iOS
 * `RootView`, which overlaid the `Drawer` over `ChatView`). The conversation's
 * menu button opens the drawer; selecting a chat/session closes it and switches
 * the [ChatViewModel] to that session; the account row invokes [onOpenSettings]
 * (the settings nav graph is wired in a later phase). Drawer + conversation read
 * the same hoisted state, and the system back gesture closes an open drawer
 * (handled by [ModalNavigationDrawer]).
 *
 * @param onOpenSettings opens the settings surface — a stub until A6 lands.
 */
@Composable
@OptIn(ExperimentalCoroutinesApi::class)
fun RootScreen(
    isDark: Boolean,
    onToggleTheme: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenSettings: () -> Unit = {},
    onOpenModelSettings: () -> Unit = {},
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    onOpenComputerUseSettings: () -> Unit = { onOpenSettings() },
    onOpenCronSettings: (String?) -> Unit = { onOpenSettings() },
    onOpenTerminal: (sessionId: String, initCommand: String?) -> Unit = { _, _ -> },
    modelSetupRequired: Boolean = false,
    // FlowMode (心流) profile bits, read from the persisted AppearancePrefs and
    // passed down so the voice-orb overlay can label itself + gate its text input.
    // The `"灵犀"` default here is unreachable in production: MainActivity's one
    // real call site always passes `prefs.assistantName` explicitly (itself
    // sourced from the localized `AppearanceStore.prefs`, see ThemeState.kt).
    // Only `@Preview`/tests that omit the argument would ever see it.
    assistantName: String = "灵犀",
    inputDialog: Boolean = true,
    // Bumped from Settings → 重新连接引擎. The retained ChatViewModel replaces
    // its owned engine source so provider changes take effect without restart.
    reconnectToken: Int = 0,
    settingsStore: SettingsStore? = null,
    onConversationSourceChanged: (ConversationSource) -> Unit = {},
    viewModel: ChatViewModel? = null,
    requestedConversationLaunch: ConversationLaunchRequest? = null,
    onConversationLaunchHandled: () -> Unit = {},
    requestedLocalAppLaunch: LocalAppLaunchRequest? = null,
    onLocalAppLaunchHandled: () -> Unit = {},
    openLocalAppsRequest: Boolean = false,
    onOpenLocalAppsHandled: () -> Unit = {},
) {
    val context = LocalContext.current
    val voiceSpeechPlayer = remember(context) { VoiceSpeechPlayer(context) }
    DisposableEffect(voiceSpeechPlayer) {
        onDispose { voiceSpeechPlayer.stop() }
    }
    val projectStore: ProjectStore = viewModel(
        key = "projects",
        factory = ProjectStore.factory(context),
    )
    val projectState by projectStore.state.collectAsState()
    val cronRepository = remember(context) { AndroidCronRepository.get(context) }
    val cronState by cronRepository.state.collectAsState()
    val projectScopeSignature = remember(projectState.activeProjectId, projectState.projects) {
        buildString {
            append(projectState.activeProjectId.orEmpty())
            projectState.projects.forEach { append('|').append(it.record.id) }
        }
    }
    LaunchedEffect(projectState.loading, projectScopeSignature) {
        if (!projectState.loading) {
            cronRepository.requestReconcile("project-change")
        }
    }

    // The Activity-scoped ViewModel owns the one native engine source. This is
    // important across configuration changes: Compose is recreated, while the
    // ViewModel and its live engine remain. Provider reconnects replace the
    // source inside that same ViewModel instead of accumulating keyed VMs.
    val appContext = context.applicationContext
    val lifecycleOwner = LocalLifecycleOwner.current
    val chatViewModel: ChatViewModel = viewModel ?: viewModel(
        key = "chat",
        factory = viewModelFactory {
            initializer {
                ChatViewModel(
                    source = EngineConversationSource.create(
                        context = appContext,
                        projectWorkspace = projectState.activeProject?.workspace,
                        linuxRuntimeMode = settingsStore?.state?.value?.linuxRuntime?.selectedMode
                            ?: com.lingxi.code.settings.LinuxRuntimeMode.Legacy,
                    ),
                    savedState = createSavedStateHandle(),
                    sourceGeneration = reconnectToken,
                    strings = conversationStrings(appContext),
                    backgroundExecution = AndroidConversationBackgroundExecution(appContext),
                )
            }
        },
    )
    LaunchedEffect(chatViewModel, reconnectToken) {
        if (viewModel == null) {
            chatViewModel.ensureSource(reconnectToken) {
                EngineConversationSource.create(
                    context = appContext,
                    projectWorkspace = projectState.activeProject?.workspace,
                    linuxRuntimeMode = settingsStore?.state?.value?.linuxRuntime?.selectedMode
                        ?: com.lingxi.code.settings.LinuxRuntimeMode.Legacy,
                )
            }
        }
        onConversationSourceChanged(chatViewModel.engineSource.value)
    }
    LaunchedEffect(chatViewModel) {
        chatViewModel.engineSource.collect { onConversationSourceChanged(it) }
    }
    DisposableEffect(chatViewModel, lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                chatViewModel.refreshExecutionStatus()
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    val selectedLinuxMode = settingsStore?.state?.value?.linuxRuntime?.selectedMode
        ?: LinuxRuntimeMode.Legacy
    LaunchedEffect(chatViewModel, selectedLinuxMode, reconnectToken) {
        if (selectedLinuxMode != LinuxRuntimeMode.MobileLinux) return@LaunchedEffect
        var cursor = runCatching {
            LinuxRuntimeBridge.readEvents(
                context = appContext,
                mode = selectedLinuxMode,
                limit = 256u,
            ).maxOfOrNull { it.sequence }
        }.getOrNull()
        while (currentCoroutineContext().isActive) {
            val eventResult = runCatching {
                LinuxRuntimeBridge.readEvents(
                    context = appContext,
                    mode = selectedLinuxMode,
                    afterSequence = cursor,
                    limit = 256u,
                )
            }
            if (eventResult.isFailure) {
                delay(250)
                continue
            }
            val events = eventResult.getOrThrow()
            for (event in events.sortedBy { it.sequence }) {
                val snapshot = event.taskId?.let { taskId ->
                    runCatching {
                        LinuxRuntimeBridge.taskStatus(
                            context = appContext,
                            mode = selectedLinuxMode,
                            taskId = taskId,
                        )
                    }.getOrNull()
                }
                chatViewModel.reduceMobileLinuxEvent(event, snapshot)
                cursor = maxOf(cursor ?: 0u, event.sequence)
            }
            delay(if (events.isEmpty()) 80 else 10)
        }
    }
    val state by chatViewModel.state.collectAsState()
    val localAppsViewModel: LocalAppsViewModel = viewModel(
        key = "local-apps",
        factory = LocalAppsViewModel.factory(
            chatViewModel.engineSource,
            strings = localAppsStrings(context),
            sessionStrings = sessionCatalogStrings(context),
            webStorageCleanup = com.lingxi.code.localapps.AndroidLocalAppWebStorageCleanup.get(appContext),
            widgetSnapshotSync = AndroidLocalAppWidgetSnapshotSync.get(appContext),
        ),
    )
    val localAppsState by localAppsViewModel.uiState.collectAsStateWithLifecycle()
    var showingApps by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(localAppsViewModel, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            localAppsViewModel.widgetPinRequests.collect { appId ->
                if (!LocalAppWidgetPinRequester.request(context, appId)) {
                    Toast.makeText(
                        context,
                        context.getString(R.string.local_apps_widget_pin_unavailable),
                        Toast.LENGTH_LONG,
                    ).show()
                }
            }
        }
    }
    LaunchedEffect(openLocalAppsRequest) {
        if (!openLocalAppsRequest) return@LaunchedEffect
        showingApps = true
        localAppsViewModel.openLibrary()
        onOpenLocalAppsHandled()
    }
    LaunchedEffect(requestedConversationLaunch) {
        val request = requestedConversationLaunch ?: return@LaunchedEffect
        showingApps = false
        // NOT `openSession`: every one of these notifications announces a
        // PARKED durable turn, and `openSession` refuses exactly that state.
        // The tap therefore did nothing, and `onConversationLaunchHandled()`
        // below then threw the request away — no retry, no feedback. Route
        // through the entry point that is allowed to cross the parked-turn
        // guard, carry the announced `turnId` so the checkpoint the user was
        // sent to look at is preserved, and retry the way the created-app
        // landing below does (the engine source may not be bound yet on a cold
        // start from the notification).
        var routed = false
        var attempt = 0
        while (!routed && attempt < CONVERSATION_LAUNCH_ATTEMPTS) {
            if (attempt > 0) delay(CONVERSATION_LAUNCH_RETRY_MS)
            attempt += 1
            routed = chatViewModel.openSessionFromNotification(
                ref = SessionRef(request.sessionId, ""),
                turnId = request.turnId,
            )
        }
        if (!routed) chatViewModel.reportConversationLaunchFailed()
        onConversationLaunchHandled()
    }
    LaunchedEffect(requestedLocalAppLaunch, localAppsState.loading) {
        val request = requestedLocalAppLaunch ?: return@LaunchedEffect
        if (localAppsState.loading) return@LaunchedEffect
        showingApps = true
        localAppsViewModel.openFromWidget(
            appId = request.appId,
            autostart = request.autostart,
        )
        onLocalAppLaunchHandled()
    }
    // Modal channels whose ONLY presenter lives inside `LocalAppsScreen`: raise
    // the cover so the user can answer them.
    //
    // `pendingProfileProposal` joins the other two here. Its dialog
    // (`ProfileProposalDialog`) is private to `LocalAppsScreen` and composed only
    // under `if (showingApps)`, and the per-app MCP tool
    // `<app>_agent_profile_propose_update` returns `approval_required: true` with
    // the engine holding the approval token until it is answered. That was
    // survivable while the cover was the only way to reach a local app; with the
    // drawer's 「创建应用」 the default path now ends in the app's CONVERSATION
    // with the cover never mounted, so the agent would wait forever on an
    // approval the user was never shown. iOS rehomed the same sheet to its root
    // for the same reason (`RootView.localAppProfileProposalItem`); Android
    // already answers this class of request by raising the cover, so it does
    // that rather than growing a second presenter.
    LaunchedEffect(
        localAppsState.pendingAuthorization,
        localAppsState.pendingUiAction,
        localAppsState.pendingProfileProposal,
    ) {
        if (
            localAppsState.pendingAuthorization != null ||
            localAppsState.pendingUiAction != null ||
            localAppsState.pendingProfileProposal != null
        ) {
            showingApps = true
        }
    }
    DisposableEffect(chatViewModel, context) {
        ComputerUseFeatureProvider.attach(context) { chatViewModel.cancel() }
        onDispose { }
    }
    val computerUseApproval by ComputerUseFeatureProvider.pendingApproval.collectAsState()
    val computerUseState by ComputerUseFeatureProvider.state.collectAsState()
    val computerUseConfiguration by ComputerUseFeatureProvider.configuration.collectAsState()
    val authorizedComputerUsePackages =
        computerUseConfiguration.appSelections.keys +
            computerUseState.grants.map { it.packageName }
    val computerUseBrowserPackages = remember(appContext, authorizedComputerUsePackages) {
        if (ComputerUseFeatureProvider.available) {
            resolveBrowserPackages(
                context = appContext,
                candidatePackages = authorizedComputerUsePackages,
            )
        } else {
            emptySet()
        }
    }
    val computerUseReadiness = if (ComputerUseFeatureProvider.available) {
        computerUseSetupStatus(
            state = computerUseState,
            configuredPackages = computerUseConfiguration.appSelections.keys,
            browserPackages = computerUseBrowserPackages,
        )
    } else {
        null
    }
    // Computer Use authorization is app-scoped, so a manual dismissal remains
    // quiet across chat switches. A fresh android_use request below deliberately
    // overrides it when the capability is actually needed.
    var computerUseSetupDismissed by rememberSaveable { mutableStateOf(false) }
    var handledComputerUseRequestKey by rememberSaveable { mutableStateOf<String?>(null) }
    val computerUseRequestKey = state.computerUseRequestKey
    LaunchedEffect(computerUseReadiness?.ready) {
        if (computerUseReadiness?.ready != false) {
            computerUseSetupDismissed = false
        }
    }
    LaunchedEffect(computerUseRequestKey) {
        if (
            shouldReshowComputerUseSetup(
                readiness = computerUseReadiness,
                requestKey = computerUseRequestKey,
                handledRequestKey = handledComputerUseRequestKey,
            )
        ) {
            handledComputerUseRequestKey = computerUseRequestKey
            computerUseSetupDismissed = false
        }
    }
    val computerUseSetup = computerUseReadiness?.takeIf {
        shouldShowComputerUseSetup(it, computerUseSetupDismissed)
    }
    val pendingPermission by chatViewModel.pendingPermission.collectAsState()
    // The engine's REAL resumable-session catalog (out-of-band, sibling of the
    // model catalog). The drawer renders its loading / empty / error states
    // directly and never falls back to mock sessions.
    val sessionState by chatViewModel.sessions.collectAsState()
    val sourceProjectId by chatViewModel.sourceProjectId.collectAsState()
    // Which workspace the live engine is bound to (Global / Project / LocalApp)
    // — the generalization of sourceProjectId for the local-app scopes.
    val sourceScope by chatViewModel.sourceScope.collectAsState()
    // Durable per-scope conversation state (last-active session + draft),
    // keyed `global` / `project.<id>` / `app.<id>`. Project/global last-active
    // stays with ProjectStore; this store carries the app scopes and records
    // which scope was active across process death.
    val scopeStore = remember(appContext) { ScopeStateStore(appContext) }
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    // Refresh the session catalog whenever the drawer transitions to open, so the
    // list is fresh each time the user reaches for it (the engine re-reports via
    // SessionList). `isOpen` flips on the open animation's start, so this fires
    // once per open, not per frame. An active app scope also refreshes its
    // workspace catalog so the drawer's app section is current.
    LaunchedEffect(drawerState.isOpen) {
        if (drawerState.isOpen) {
            chatViewModel.refreshSessions()
            cronRepository.refresh()
            (sourceScope as? ConversationScope.LocalApp)?.let {
                localAppsViewModel.onAction(LocalAppsAction.LoadAppSessions(it.appId, null))
            }
        }
    }

    // Voice-flow overlay visibility, hoisted here (the Android analog of the iOS
    // RootView's `voiceActive` flag): the composer's mic long-press flips it on,
    // releasing the held finger flips it off. The overlay renders above the
    // drawer + conversation.
    var voiceActive by remember { mutableStateOf(false) }

    // FlowMode (心流) voice-orb overlay visibility — a TAP on the composer mic
    // flips it on (the long-press STT path still drives `voiceActive`). The
    // overlay renders above the drawer + conversation + voice-flow overlay.
    var flowActive by remember { mutableStateOf(false) }
    var appInForeground by remember {
        mutableStateOf(lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED))
    }

    // FlowMode orb voice driver: a one-shot tap-to-talk listener, plus the live
    // assistant reply text derived from the same conversation state ChatScreen
    // renders (the orb is just another view of the real session).
    val orbListen = rememberOrbVoiceListen()
    val orbAssistantText = (state.streamingMessage ?: state.messages.lastOrNull())
        ?.let { if (it.role == Role.Ai) it.text else "" } ?: ""
    DisposableEffect(lifecycleOwner, orbListen, voiceSpeechPlayer) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START, Lifecycle.Event.ON_RESUME -> appInForeground = true
                Lifecycle.Event.ON_STOP -> {
                    appInForeground = false
                    voiceSpeechPlayer.stop()
                    cancelActiveHeldVoiceSession()
                    orbListen.cancel()
                    // Keep Flow Mode open but paused. Returning to the app shows
                    // the latest assistant result; another explicit tap is
                    // required before either microphone starts again.
                    voiceActive = false
                }
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    // Mirror the engine's REAL MCP listing into the activity-scoped SettingsStore
    // (the same instance SettingsHost renders). RefreshListings runs again after
    // provider reconnect; an empty reply remains an explicit empty catalog.
    val resolvedSettingsStore: SettingsStore =
        settingsStore ?: viewModel(factory = SettingsStore.factory(context))
    val settingsState by resolvedSettingsStore.state.collectAsState()
    val currentAutoPlayReplies = rememberUpdatedState(settingsState.voice.autoPlayReplies)
    val currentFlowActive = rememberUpdatedState(flowActive)
    val currentAppInForeground = rememberUpdatedState(appInForeground)
    LaunchedEffect(chatViewModel, voiceSpeechPlayer) {
        chatViewModel.turnCompletions.collect { completion ->
            if (completion.origin != ConversationTurnOrigin.Ordinary) return@collect
            if (completion.outcome != ConversationTurnOutcome.Completed) return@collect
            if (!currentAppInForeground.value) return@collect
            if (!currentAutoPlayReplies.value || currentFlowActive.value) return@collect
            val text = completion.finalAssistantText.trim()
            if (text.isEmpty()) return@collect
            runCatching { voiceSpeechPlayer.speak(text) }
        }
    }
    LaunchedEffect(chatViewModel, resolvedSettingsStore) {
        chatViewModel.engineSource
            .flatMapLatest { it.clientEvents }
            .collect { event ->
                if (event is ClientEvent.PermissionModeChanged) {
                    resolvedSettingsStore.setEffectivePermissionMode(event.mode)
                }
            }
    }
    val modelProviderStatuses = remember(settingsState.llmProviders) {
        settingsState.llmProviders.map { provider ->
            ModelProviderStatus(
                profileId = ProviderSettingsRepository.profileNameFor(provider),
                settingsId = provider.id,
                name = provider.name,
                status = provider.status,
                enabled = provider.enabled,
                credentialConfigured = provider.credentialConfigured,
            )
        }
    }
    val engineMcp by chatViewModel.mcpServers.collectAsState()
    LaunchedEffect(chatViewModel, reconnectToken) { chatViewModel.refreshMcpServers() }
    LaunchedEffect(engineMcp) {
        resolvedSettingsStore.setMcpServers(engineMcp)
    }

    // The composer draft is hoisted here so a voice transcription (the
    // hold-to-talk release) can route its recognized text straight into the
    // input the user is about to send. Seeded from the ViewModel's SavedStateHandle
    // so an unsent draft survives process death; every edit mirrors back into the
    // handle (see onDraftChange below) and `send` clears it. App scopes mirror
    // into the per-scope store instead — see the scope-restore effect below.
    var draft by remember { mutableStateOf(chatViewModel.restoredDraft) }
    var voiceDraftBase by remember { mutableStateOf("") }

    // Which scope's draft the composer is currently showing — guards the
    // restore effect below against unrelated recompositions.
    var draftScopeKey by remember { mutableStateOf<String?>(null) }

    // Swap the visible draft when the SCOPE changes: an app scope's draft
    // comes from the durable scope store; project/global keep today's
    // SavedStateHandle slot. Keyed on the persistence key so rotation (same
    // scope, new composition) never clobbers what the user is typing.
    LaunchedEffect(sourceScope) {
        val key = sourceScope.persistenceKey()
        if (key == draftScopeKey) return@LaunchedEffect
        val firstBind = draftScopeKey == null
        draftScopeKey = key
        val restored = when (sourceScope) {
            is ConversationScope.LocalApp -> scopeStore.read(key)?.draft.orEmpty()
            else -> chatViewModel.restoredDraft
        }
        // The very first bind after process start must not wipe a draft the
        // user already restored (remember { } above) — only apply when the
        // stored value differs and this is a REAL scope change.
        if (!firstBind || sourceScope is ConversationScope.LocalApp) {
            draft = restored
        }
    }

    // Hold-to-talk → live transcription, gated on RECORD_AUDIO. The recognized
    // partial utterance replaces the live voice suffix while recognition is
    // running; the final result replaces that same suffix on release.
    val (startVoiceCapture, onVoiceHoldRelease) = rememberVoiceCapture(
        onTranscript = { transcript ->
            draft = appendVoiceTranscript(voiceDraftBase, transcript)
        },
        onPartialTranscript = { partial ->
            draft = appendVoiceTranscript(voiceDraftBase, partial)
        },
    )
    val onVoiceHoldStart: () -> Unit = {
        voiceDraftBase = draft
        startVoiceCapture()
    }

    // The captured-photo attachment is hoisted here exactly like the voice draft:
    // tapping the composer's camera affordance drives an on-device capture through
    // the same CameraController the engine bridges onto `traits::CameraControl`,
    // and the resulting JPEG surfaces as a composer thumbnail (the device-vision
    // analog of how onTranscript surfaces a recognized utterance).
    var attachment by remember { mutableStateOf<ComposerAttachment?>(null) }

    val onCameraClick = rememberCameraCapture(
        onCaptured = { image ->
            val bmp = BitmapFactory.decodeByteArray(image.jpegBytes, 0, image.jpegBytes.size)
            if (bmp != null) {
                attachment = ComposerAttachment(
                    thumb = bmp.asImageBitmap(),
                    width = image.width,
                    height = image.height,
                    mediaType = "image/jpeg",
                    base64 = Base64.encodeToString(image.jpegBytes, Base64.NO_WRAP),
                )
            }
        },
    )

    // Device-share: tapping a message bubble's share affordance surfaces the
    // native chooser through the same ShareController the engine bridges onto
    // `traits::SharingService` (the device-share analog of how onCameraClick
    // reuses CameraController for both the UI affordance and `tool-camera`).
    val onShare = rememberShare()

    // Connectivity: a dismissible offline banner driven by ConnectivityManager's
    // NetworkCallback (rememberOnlineState). It only INFORMS — the conversation
    // is never hard-blocked. `dismissedWhileOffline` hides the banner after the
    // user dismisses the current offline episode; coming back online resets it
    // so the next disconnect re-shows it.
    val isOnline by rememberOnlineState()
    var dismissedWhileOffline by remember { mutableStateOf(false) }
    LaunchedEffect(isOnline) {
        if (isOnline) dismissedWhileOffline = false
    }

    fun closeDrawer() = scope.launch { drawerState.close() }

    /**
     * Transactionally rebind the conversation engine to [engineScope]. For a
     * Project scope [project] supplies the workspace snapshot (as before); a
     * LocalApp scope resolves its `apps/<id>/workspace` directory against the
     * engine data root exactly the way the code browser does; Global binds no
     * workspace. The active scope is persisted per-scope alongside the
     * project store's active-project index.
     */
    suspend fun switchEngineScope(
        engineScope: ConversationScope,
        project: ProjectSnapshot?,
        target: SessionRef?,
        newSession: Boolean,
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
    ): Boolean {
        val destination = target ?: SessionRef("new", context.getString(R.string.chat_new_conversation))
        var persisted: ProjectStoreState? = null
        return chatViewModel.switchWorkspaceSource(
            projectId = (engineScope as? ConversationScope.Project)?.projectId,
            target = destination,
            newSession = newSession,
            resumeEmpty = resumeEmpty,
            replacePendingTransition = replacePendingTransition,
            scope = engineScope,
            createSource = {
                EngineConversationSource.create(
                    context = appContext,
                    projectWorkspace = when (engineScope) {
                        ConversationScope.Global -> null
                        is ConversationScope.Project -> project?.workspace
                        is ConversationScope.LocalApp -> localAppWorkspace(
                            appFilesRoot = appContext.filesDir,
                            appId = engineScope.appId,
                            workspaceRel = localAppsViewModel.uiState.value.apps
                                .firstOrNull { it.id == engineScope.appId }
                            ?.workspaceRel,
                        )
                    },
                    linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                    reuseProcessSource = true,
                )
            },
            persistSelection = {
                persisted = projectStore.persistActive((engineScope as? ConversationScope.Project)?.projectId)
                scopeStore.persistActiveScope(engineScope.persistenceKey())
            },
            onCommitted = {
                projectStore.publishActive(checkNotNull(persisted))
                drawerUi.selectSession(destination.id)
            },
        )
    }

    /** Project-flow convenience: derive the scope from the snapshot. */
    suspend fun switchEngineScope(
        project: ProjectSnapshot?,
        target: SessionRef?,
        newSession: Boolean,
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
    ): Boolean = switchEngineScope(
        engineScope = project?.let { ConversationScope.Project(it.record.id) } ?: ConversationScope.Global,
        project = project,
        target = target,
        newSession = newSession,
        resumeEmpty = resumeEmpty,
        replacePendingTransition = replacePendingTransition,
    )

    // A freshly created app hands the conversation off into its OWN scope.
    //
    // The app is created by the sheet, BEFORE any conversation exists, so this
    // switch is the first thing that happens rather than the tail of an intake
    // turn. That is what makes the app's first message already rooted in the
    // app workspace: the SCOPE is what sets the session cwd, and an agent that
    // starts anywhere else writes its source into the wrong directory (observed
    // on device — it hand-rolled a package.json/vite.config.js by copying
    // another app, and every build after that failed on the workspace).
    //
    // `initSessionId` is null only when the engine's best-effort init-session
    // mint failed; a fresh conversation in the same scope is still correct.
    LaunchedEffect(localAppsViewModel, chatViewModel, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            localAppsViewModel.createdAppLandings.collect { landing ->
                showingApps = false
                // The result is load-bearing, not decoration: `switchWorkspaceSource`
                // REFUSES while a turn is streaming (or another switch is pending)
                // and returns false after only raising a banner — it does not begin
                // a transition. Ignoring that left the wait below satisfied by the
                // CURRENT project conversation, and the kickoff was then sent into
                // it: the app's agent rooted in the wrong directory, which is the
                // exact failure the comment above says was observed on device.
                // Retry, the way iOS's `openCreatedAppSession` does.
                var switched = false
                var attempt = 0
                while (!switched && attempt < LANDING_SWITCH_ATTEMPTS) {
                    if (attempt > 0) delay(LANDING_SWITCH_RETRY_MS)
                    attempt += 1
                    switched = switchEngineScope(
                        engineScope = ConversationScope.LocalApp(landing.appId),
                        project = null,
                        target = landing.initSessionId?.let { SessionRef(it, "") },
                        newSession = landing.initSessionId == null,
                        resumeEmpty = true,
                    )
                }
                if (switched) {
                    // Bounded: a failed transition clears `sessionTransitioning`
                    // WITHOUT setting `sessionReady`, so an unbounded wait would
                    // suspend forever inside `collect` and strand every later
                    // landing too.
                    withTimeoutOrNull(SESSION_READY_TIMEOUT_MS) {
                        chatViewModel.state.first {
                            it.sessionReady && !it.sessionTransitioning && it.session.id.isNotBlank()
                        }
                    }?.let {
                        // Placeholder-free copy: a shell has no brief to
                        // interpolate, and the old text told the agent the
                        // shape and the name were "already fixed", which is
                        // exactly what the conversation now exists to decide.
                        chatViewModel.send(context.getString(R.string.local_apps_kickoff))
                    }
                }
            }
        }
    }

    // Tapping a DRAFT card in the library resumes that shell's pinned init
    // conversation instead of opening a details/preview page it has nothing to
    // show on (design §D.3, and what iOS's `open(_:)` already does). A user who
    // leaves the interview half-finished and taps back in must land in the SAME
    // conversation.
    //
    // NOT the created-app landing above: no kickoff is sent here. That prompt
    // opens the interview exactly once; re-sending it on every re-entry would
    // restart an interview already in progress.
    //
    // `resumeEmpty = true` rather than a guessed message count. The library row
    // carries no transcript length, and the engine's `resume_empty_session`
    // replays a real transcript normally while ALSO bootstrapping the
    // zero-message case that a plain `resume_session` rejects outright — so it
    // is correct for a shell whose kickoff landed and for one whose did not.
    LaunchedEffect(localAppsViewModel, chatViewModel, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            localAppsViewModel.draftSessionLandings.collect { landing ->
                showingApps = false
                val target = landing.sessionId?.let { SessionRef(it, "") }
                if (chatViewModel.sourceScope.value == ConversationScope.LocalApp(landing.appId)) {
                    // Already inside this app: an in-place session switch, so
                    // the engine source is not needlessly rebound.
                    if (target == null) {
                        chatViewModel.startNewSession()
                    } else {
                        drawerUi.selectSession(target.id)
                        chatViewModel.openSession(target, empty = true)
                    }
                } else {
                    switchEngineScope(
                        engineScope = ConversationScope.LocalApp(landing.appId),
                        project = null,
                        target = target,
                        newSession = target == null,
                        resumeEmpty = true,
                    )
                }
            }
        }
    }
    // Recover the last active Project after process start. The Activity-scoped
    // ChatViewModel survives rotation, so sourceProjectId prevents a needless
    // rebuild on configuration changes. A process-restored global Resume is
    // explicitly superseded only after the Project Source and active index are
    // both ready, so the wrong cwd never becomes authoritative. An app scope
    // never trips this: entering one persists activeProject = null, and the
    // sourceScope guard covers the in-flight transition window.
    LaunchedEffect(
        projectState.loading,
        projectState.activeProjectId,
        sourceProjectId,
    ) {
        val project = projectState.activeProject
        if (
            !projectState.loading &&
                project != null &&
                sourceProjectId == null &&
                chatViewModel.sourceScope.value !is ConversationScope.LocalApp
        ) {
            val lastSummary = project.record.lastActiveSessionId
                ?.let { id -> project.sessions.firstOrNull { it.sessionId == id } }
            val last = lastSummary?.let {
                SessionRef(id = it.sessionId, title = it.title)
            }
            switchEngineScope(
                project = project,
                target = last,
                newSession = last == null,
                resumeEmpty = lastSummary?.messageCount == 0,
                replacePendingTransition = true,
            )
        }
    }

    // Recover the last active LOCAL-APP scope after process start — the app
    // analog of the Project recovery above, driven by the scope store's
    // persisted active-scope key + the app's own last-active session. Runs at
    // most once per process; entering an app scope persists
    // activeProject = null, so the two restore effects never race each other.
    var appScopeRestoreAttempted by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(projectState.loading, sourceScope) {
        if (projectState.loading || appScopeRestoreAttempted) return@LaunchedEffect
        if (sourceScope != ConversationScope.Global) return@LaunchedEffect
        val persistedScope = conversationScopeFromKey(scopeStore.readActiveScopeKey())
        if (persistedScope is ConversationScope.LocalApp) {
            appScopeRestoreAttempted = true
            val lastSessionId = scopeStore.read(persistedScope.persistenceKey())?.lastActiveSessionId
            // A draft restores under its localized placeholder title, never
            // under the engine's `"untitled"` — same predicate as the library
            // card and the drawer header.
            val appName = localAppsViewModel.uiState.value.apps
                .firstOrNull { it.id == persistedScope.appId }
                ?.let {
                    localAppDisplayName(
                        it,
                        draftTitle = context.getString(R.string.local_apps_draft_card_title),
                        fallback = persistedScope.appId,
                    )
                }
            switchEngineScope(
                engineScope = persistedScope,
                project = null,
                target = lastSessionId?.let {
                    SessionRef(it, appName ?: context.getString(R.string.chat_new_conversation))
                },
                newSession = lastSessionId == null,
                replacePendingTransition = true,
            )
        }
    }

    // Persist an app scope's last-active session once the engine confirms it —
    // the durable half of "each app remembers where its conversation left
    // off". Project/global equivalents already flow through ProjectStore.
    LaunchedEffect(sourceScope, state.session.id, state.sessionReady) {
        val active = sourceScope
        if (active is ConversationScope.LocalApp && state.sessionReady && state.session.id != "new") {
            runCatching { scopeStore.persistLastActiveSession(active.persistenceKey(), state.session.id) }
        }
    }

    // The current Source's SessionList is authoritative for its own scope.
    // Cache global rows separately so the 对话 tab remains available while a
    // Project engine is active.
    LaunchedEffect(
        sourceProjectId,
        sessionState.phase,
        sessionState.rows,
        state.session.id,
        state.sessionReady,
        state.isNew,
    ) {
        if (sessionState.phase == SessionCatalogPhase.Ready) {
            val provisionalSessionMayNotBeListed =
                state.isNew &&
                    (
                        state.session.id == "new" ||
                            sessionState.rows.none { it.uuid == state.session.id }
                        )
            if (!provisionalSessionMayNotBeListed) {
                runCatching { projectStore.syncEngineSessions(sourceProjectId, sessionState.rows) }
                    .onFailure { chatViewModel.reportHostError(context.getString(R.string.session_index_save_failed_fmt, it.message.orEmpty())) }
            }
        }
    }
    // SessionStarted is emitted before an empty session necessarily has a
    // file-backed SessionList row. Persist that confirmed id immediately, then
    // wait until the first turn finishes before asking the authoritative catalog
    // to replace the provisional row.
    LaunchedEffect(
        sourceProjectId,
        state.session.id,
        state.sessionReady,
        state.isNew,
    ) {
        if (
            state.sessionReady &&
            state.isNew &&
            state.session.id != "new"
        ) {
            runCatching {
                projectStore.recordStartedSession(
                    projectId = sourceProjectId,
                    sessionId = state.session.id,
                    title = state.session.title,
                )
            }.onFailure {
                chatViewModel.reportHostError(context.getString(R.string.session_index_save_new_failed_fmt, it.message.orEmpty()))
            }
        }
    }
    LaunchedEffect(
        sourceProjectId,
        state.session.id,
        state.sessionReady,
        state.isNew,
        state.streaming,
    ) {
        if (
            state.sessionReady &&
            state.session.id != "new" &&
            !state.isNew &&
            !state.streaming
        ) {
            chatViewModel.refreshSessions()
        }
    }
    LaunchedEffect(
        sourceProjectId,
        state.session.id,
        state.sessionReady,
        projectState.projects,
    ) {
        val projectId = sourceProjectId ?: return@LaunchedEffect
        if (
            state.sessionReady &&
            projectState.projects.firstOrNull { it.record.id == projectId }
                ?.sessions
                ?.any { it.sessionId == state.session.id } == true
        ) {
            runCatching { projectStore.markActiveSession(projectId, state.session.id) }
        }
    }

    val cachedGlobalRows = projectState.globalSessions.map { cached ->
        SessionRow(
            uuid = cached.sessionId,
            title = cached.title,
            messageCount = cached.messageCount,
            relativeTime = cached.relativeTime,
        )
    }
    val globalDrawerSessions = if (sourceProjectId == null) {
        sessionState.withCachedRows(cachedGlobalRows)
    } else {
        EngineSessionState.ready(cachedGlobalRows)
    }
    val drawerProductionData = DrawerProductionData(
        workspaces = listOf(LocalProjectWorkspace),
        projects = projectState.projects.map { it.toDrawerProject() },
        crons = cronState.tasks.map { cron ->
            val status = cron.activeRun?.status ?: cron.lastRun?.status
            Cron(
                id = "${cron.scope.scopeId}:${cron.task.id}",
                wsId = LocalProjectWorkspace.id,
                title = cron.task.prompt.lineSequence().firstOrNull()
                    ?.take(42)
                    ?.ifBlank { cron.task.id }
                    ?: cron.task.id,
                cron = cron.task.cron,
                next = cron.task.nextFireMs?.toLong()?.let(::formatCronTime)
                    ?: context.getString(R.string.cron_no_next_fire),
                desc = buildString {
                    append(cron.scope.projectName)
                    append(" · ")
                    append(
                        when {
                            cron.schedulingMode == CronSchedulingMode.Unsupported ->
                                cron.unsupportedReason ?: context.getString(R.string.cron_unsupported_period_fallback)
                            status != null -> cronStatusLabel(status, context)
                            cron.schedulingMode == CronSchedulingMode.FifteenMinuteFallback ->
                                context.getString(R.string.cron_fifteen_minute_patrol_short)
                            else -> context.getString(R.string.cron_exact_alarm_short)
                        },
                    )
                },
                enabled = cron.schedulingMode != CronSchedulingMode.Unsupported,
            )
        },
        projectStatusMessage = projectState.operation?.message,
    )
    LaunchedEffect(Unit) {
        if (drawerUi.activeWs.isBlank()) drawerUi.selectWorkspace(LocalProjectWorkspace.id)
    }

    var showCreateProject by remember { mutableStateOf(false) }
    var pendingImportName by remember { mutableStateOf<String?>(null) }
    var pendingReauthorizeProjectId by remember { mutableStateOf<String?>(null) }
    var dismissedConflictSignature by remember { mutableStateOf<String?>(null) }
    val activeProjectSyncing =
        projectState.operation?.let { operation ->
            operation.projectId == sourceProjectId &&
                operation.kind in setOf(
                    ProjectOperationKind.Reimport,
                    ProjectOperationKind.Export,
                    ProjectOperationKind.ResolveConflicts,
                )
        } == true
    fun runConversationAction(action: () -> Unit) {
        if (activeProjectSyncing) {
            chatViewModel.reportHostError(context.getString(R.string.project_sync_in_progress_notice))
        } else {
            action()
        }
    }
    fun runProjectSync(projectId: String, action: () -> Unit): Boolean {
        val activeProjectIsExecuting =
            projectId == sourceProjectId && (state.streaming || state.sessionTransitioning)
        if (activeProjectIsExecuting) {
            chatViewModel.reportHostError(context.getString(R.string.project_sync_stop_task_first_notice))
            return false
        }
        action()
        return true
    }
    val conflictSignature = projectState.conflicts
        .takeIf { it.isNotEmpty() }
        ?.joinToString("|") { "${it.projectId}:${it.relativePath}" }
    val visibleProjectConflicts =
        if (conflictSignature != null && conflictSignature != dismissedConflictSignature) {
            projectState.conflicts
        } else {
            emptyList()
        }
    LaunchedEffect(conflictSignature) {
        if (conflictSignature == null) dismissedConflictSignature = null
    }
    val importProjectLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        val name = pendingImportName
        val reauthorizeProjectId = pendingReauthorizeProjectId
        pendingImportName = null
        pendingReauthorizeProjectId = null
        if (uri != null && name != null) {
            scope.launch {
                runCatching { projectStore.importSaf(name, uri) }
                    .onSuccess { created ->
                        if (switchEngineScope(created, null, true)) closeDrawer()
                }
            }
        } else if (uri != null && reauthorizeProjectId != null) {
            runProjectSync(reauthorizeProjectId) {
                projectStore.reauthorize(reauthorizeProjectId, uri)
            }
        }
    }

    Box(modifier = modifier.fillMaxSize()) {
        ModalNavigationDrawer(
            modifier = Modifier.fillMaxSize(),
            drawerState = drawerState,
            scrimColor = Color.Black.copy(alpha = 0.4f),
            drawerContent = {
                ModalDrawerSheet(
                    drawerContainerColor = LingXiTheme.palette.sidebarBg,
                    drawerTonalElevation = 0.dp,
                    modifier = Modifier.width(320.dp),
                ) {
                    DrawerContent(
                        ui = drawerUi,
                        onSelectSession = { ref ->
                            showingApps = false
                            drawerUi.selectSession(ref.id)
                            chatViewModel.openSession(ref)
                            closeDrawer()
                        },
                        onOpenSettings = {
                            closeDrawer()
                            onOpenSettings()
                        },
                        onOpenTerminal = {
                            closeDrawer()
                            onOpenTerminal("interactive", null)
                        },
                        onClose = { closeDrawer() },
                        engineSessions = globalDrawerSessions,
                        onResumeSession = { uuid ->
                            showingApps = false
                            globalDrawerSessions.rows.firstOrNull { it.uuid == uuid }?.let { row ->
                                // Resume directly only when the live engine IS
                                // the global scope — a Project OR LocalApp
                                // scope must rebind first, or the session would
                                // resume against the wrong cwd.
                                if (sourceScope == ConversationScope.Global) {
                                    drawerUi.selectSession(uuid)
                                    chatViewModel.resumeSession(row)
                                    closeDrawer()
                                } else {
                                    scope.launch {
                                        if (
                                            switchEngineScope(
                                                null,
                                                SessionRef(row.uuid, row.title),
                                                false,
                                                resumeEmpty = row.messageCount == 0,
                                            )
                                        ) {
                                            closeDrawer()
                                        }
                                    }
                                }
                            }
                        },
                        appScope = (sourceScope as? ConversationScope.LocalApp)?.let { active ->
                            DrawerAppScope(
                                appId = active.appId,
                                appName = localAppDisplayName(
                                    localAppsState.apps.firstOrNull { it.id == active.appId },
                                    draftTitle = context.getString(R.string.local_apps_draft_card_title),
                                    fallback = active.appId,
                                ),
                                sessions = localAppsState.appSessions[active.appId]?.rows.orEmpty().map { row ->
                                    SessionRow(
                                        uuid = row.uuid,
                                        title = row.title,
                                        messageCount = row.messageCount,
                                        relativeTime = row.relativeTime,
                                    )
                                },
                            )
                        },
                        onSelectAppScopeSession = { ref ->
                            // Same engine scope (the section only renders for
                            // the ACTIVE app), so a plain in-place resume.
                            showingApps = false
                            drawerUi.selectSession(ref.id)
                            val appId = (sourceScope as? ConversationScope.LocalApp)?.appId
                            val row = appId?.let { id ->
                                localAppsState.appSessions[id]?.rows?.firstOrNull { it.uuid == ref.id }
                            }
                            chatViewModel.openSession(ref, empty = row?.messageCount == 0)
                            closeDrawer()
                        },
                        onNewAppScopeSession = {
                            showingApps = false
                            chatViewModel.startNewSession()
                            closeDrawer()
                        },
                        productionData = drawerProductionData,
                        onCreateProject = { showCreateProject = true },
                        onSelectProjectSession = { projectId, ref ->
                            showingApps = false
                            val project = projectState.projects.firstOrNull {
                                it.record.id == projectId
                            } ?: return@DrawerContent
                            val session = project.sessions
                                .firstOrNull { it.sessionId == ref.id }
                                ?: return@DrawerContent
                            val resumeTarget = SessionRef(session.sessionId, session.title)
                            val switched = if (sourceProjectId == projectId) {
                                drawerUi.selectSession(resumeTarget.id)
                                chatViewModel.openSession(
                                    resumeTarget,
                                    empty = session.messageCount == 0,
                                )
                                true
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            project = project,
                                            target = resumeTarget,
                                            newSession = false,
                                            resumeEmpty = session.messageCount == 0,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onNewProjectSession = { projectId ->
                            showingApps = false
                            val project = projectState.projects.firstOrNull {
                                it.record.id == projectId
                            } ?: return@DrawerContent
                            val switched = if (sourceProjectId == projectId) {
                                chatViewModel.startNewSession()
                                true
                            } else {
                                scope.launch {
                                    if (switchEngineScope(project, null, true)) closeDrawer()
                                }
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onReimportProject = { projectId ->
                            runProjectSync(projectId) { projectStore.reimport(projectId) }
                        },
                        onExportProject = { projectId ->
                            runProjectSync(projectId) { projectStore.export(projectId) }
                        },
                        onReauthorizeProject = { projectId ->
                            runProjectSync(projectId) {
                                pendingReauthorizeProjectId = projectId
                                importProjectLauncher.launch(null)
                            }
                        },
                        onOpenCron = { taskKey ->
                            closeDrawer()
                            onOpenCronSettings(taskKey)
                        },
                        onCreateCron = {
                            closeDrawer()
                            onOpenCronSettings(null)
                        },
                        appsCount = localAppsState.apps.size,
                        // Create, then land in the new app's own conversation.
                        //
                        // `closeDrawer()` here rather than on the landing: the
                        // landing collector below closes the apps cover, but
                        // nothing on that path touches the drawer, and it fires
                        // seconds later — or never, if the create fails. The
                        // drawer must not sit open over either outcome.
                        //
                        // The apps cover is deliberately NOT opened. That is the
                        // whole point of this row: the user ends up in a
                        // conversation, not on a library page. It is also why
                        // `createAppFromDrawer` passes `armLibraryFallback =
                        // false`, and why `LocalAppsErrorDialog` (at the bottom
                        // of this file) has to exist at all.
                        onCreateApp = {
                            closeDrawer()
                            localAppsViewModel.createAppFromDrawer()
                        },
                        onOpenApps = {
                            showingApps = true
                            closeDrawer()
                            localAppsViewModel.onAction(LocalAppsAction.Refresh)
                        },
                    )
                }
            },
        ) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .windowInsetsPadding(WindowInsets.systemBars)
                    .imePadding(),
            ) {
                if (showingApps) {
                    BackHandler {
                        if (localAppsState.destination == LocalAppsDestination.Library) {
                            showingApps = false
                        } else {
                            localAppsViewModel.onAction(LocalAppsAction.Back)
                        }
                    }
                    LocalAppsRoute(
                        viewModel = localAppsViewModel,
                        onOpenDrawer = { scope.launch { drawerState.open() } },
                        onExternalNavigation = { url ->
                            runCatching {
                                context.startActivity(
                                    Intent(Intent.ACTION_VIEW, Uri.parse(url))
                                        .addCategory(Intent.CATEGORY_BROWSABLE),
                                )
                            }
                        },
                        onOpenAppSession = { appId, sessionRow ->
                            // Leave the apps surface and drop the conversation
                            // into the app's scope: resume the tapped catalog
                            // row, or start fresh for 「新会话」. Already in
                            // this app's scope → plain in-place session switch.
                            showingApps = false
                            val target = sessionRow?.let { SessionRef(it.uuid, it.title) }
                            if (sourceScope == ConversationScope.LocalApp(appId)) {
                                if (target == null) {
                                    chatViewModel.startNewSession()
                                } else {
                                    drawerUi.selectSession(target.id)
                                    chatViewModel.openSession(target, empty = sessionRow.messageCount == 0)
                                }
                            } else {
                                scope.launch {
                                    switchEngineScope(
                                        engineScope = ConversationScope.LocalApp(appId),
                                        project = null,
                                        target = target,
                                        newSession = target == null,
                                        resumeEmpty = sessionRow?.messageCount == 0,
                                    )
                                }
                            }
                        },
                        modifier = Modifier.fillMaxSize(),
                    )
                } else {
                    ChatScreen(
                        state = state,
                        onSend = {
                            text ->
                            voiceSpeechPlayer.stop()
                            runConversationAction {
                                chatViewModel.send(
                                    text,
                                    origin = ConversationTurnOrigin.Ordinary,
                                )
                            }
                        },
                        onSendWithAttachment = { text, attachment ->
                            voiceSpeechPlayer.stop()
                            runConversationAction {
                                chatViewModel.send(
                                    text,
                                    images = attachment?.toImageRef()?.let(::listOf).orEmpty(),
                                    origin = ConversationTurnOrigin.Ordinary,
                                )
                            }
                        },
                        // "新对话": reset the local transcript immediately AND tell the
                        // engine to begin a new session (NewSession). For the mock the
                        // engine call is a no-op, so this still behaves like newChat.
                        onNewChat = {
                            voiceSpeechPlayer.stop()
                            runConversationAction(chatViewModel::startNewSession)
                        },
                        onSelectModel = chatViewModel::selectModel,
                        isDark = isDark,
                        onToggleTheme = onToggleTheme,
                        onOpenDrawer = { scope.launch { drawerState.open() } },
                        // Ordinary mic and Flow Mode are separate controls.
                        onMicClick = {
                            if (!voiceActive) voiceSpeechPlayer.stop()
                            voiceActive = !voiceActive
                            if (voiceActive) onVoiceHoldStart() else onVoiceHoldRelease()
                        },
                        onMicHoldStart = {
                            voiceSpeechPlayer.stop()
                            voiceActive = true
                            onVoiceHoldStart()
                        },
                        onMicHoldRelease = {
                            voiceActive = false
                            onVoiceHoldRelease()
                        },
                        onFlowModeClick = {
                            flowActive = !flowActive
                            if (flowActive) voiceSpeechPlayer.stop()
                        },
                        flowModeActive = flowActive,
                        flowModePanel = {
                            FlowModeOverlay(
                                visible = flowActive,
                                assistantName = assistantName,
                                inputDialog = inputDialog,
                                streaming = state.streaming,
                                assistantText = orbAssistantText,
                                onSend = {
                                    text ->
                                    runConversationAction {
                                        chatViewModel.send(
                                            text,
                                            origin = ConversationTurnOrigin.Flow,
                                        )
                                    }
                                },
                                onCancel = { chatViewModel.cancel() },
                                onListen = orbListen,
                                onClose = { flowActive = false },
                            )
                        },
                        draft = draft,
                        onDraftChange = {
                            draft = it
                            when (val active = sourceScope) {
                                // App scopes persist into the per-scope store
                                // (IO-dispatched inside), NOT the SavedState
                                // slot — the global/project draft must survive
                                // an app-scope visit untouched.
                                is ConversationScope.LocalApp ->
                                    scope.launch { scopeStore.persistDraft(active.persistenceKey(), it) }
                                else -> chatViewModel.onDraftChanged(it) // mirror into SavedStateHandle
                            }
                        },
                        onCameraClick = onCameraClick,
                        attachment = attachment,
                        onRemoveAttachment = { attachment = null },
                        onShare = onShare,
                        onStop = chatViewModel::cancel,
                        onDiscardRecoveredTurn = chatViewModel::discardRecoveredTurn,
                        onDismissError = chatViewModel::dismissError,
                        showOfflineBanner = shouldShowOfflineBanner(isOnline, dismissedWhileOffline),
                        onDismissOffline = { dismissedWhileOffline = true },
                        // "重试" re-sends the last user turn through the same path the
                        // composer uses; the ConnectivityManager callback keeps the
                        // banner's visibility honest (it auto-clears once a validated
                        // network returns, regardless of this tap).
                        onRetryOffline = {
                            runConversationAction { chatViewModel.resendLast() }
                        },
                        modelSetupRequired = modelSetupRequired,
                        onOpenModelSettings = onOpenModelSettings,
                        modelProviderStatuses = modelProviderStatuses,
                        onOpenProviderSettings = onOpenProviderSettings,
                        computerUseSetup = computerUseSetup,
                        onOpenComputerUseSettings = onOpenComputerUseSettings,
                        onDismissComputerUseSetup = { computerUseSetupDismissed = true },
                        onOpenTerminal = { sessionId, command ->
                            onOpenTerminal(sessionId, command)
                        },
                        onAnswerQuestion = chatViewModel::answerQuestion,
                        onCancelQuestion = chatViewModel::cancelQuestion,
                        onResumeWorkflow = chatViewModel::resumeWorkflow,
                        // Tool-call expansion and the plan panel keep their state in
                        // the ViewModel, not in the recycled rows that render them.
                        onToggleToolCall = chatViewModel::toggleToolCall,
                        onTogglePlan = chatViewModel::togglePlanExpanded,
                    )
                }
            }
        }

        // Immersive voice overlay (full-screen, above everything else).
        VoiceFlowOverlay(visible = voiceActive)

        // Permission prompt (above everything): renders the head parked request
        // and resolves it by submitting Approve/DenyPermission through the source,
        // which forwards to MobileEngineHandle.submit. SHIP-BLOCKER #3: this is
        // the round-trip that unparks a write/Bash turn the engine is waiting on.
        PermissionPromptDialog(
            state = pendingPermission,
            onApprove = { requestId, response ->
                scope.launch { chatViewModel.approvePermission(requestId, response) }
            },
            onDeny = { requestId ->
                scope.launch { chatViewModel.denyPermission(requestId) }
            },
        )
        ComputerUseApprovalDialog(
            approval = computerUseApproval,
            onResolve = ComputerUseFeatureProvider::resolveApproval,
        )

        CreateProjectDialog(
            visible = showCreateProject,
            onDismiss = { showCreateProject = false },
            onCreateInternal = { name ->
                showCreateProject = false
                scope.launch {
                    runCatching { projectStore.createInternal(name) }
                        .onSuccess { created ->
                            if (switchEngineScope(created, null, true)) closeDrawer()
                        }
                }
            },
            onChooseExternal = { name ->
                showCreateProject = false
                pendingImportName = name
                importProjectLauncher.launch(null)
            },
        )
        ProjectConflictDialog(
            conflicts = visibleProjectConflicts,
            onKeepExternal = {
                val projectId = projectState.conflicts.first().projectId
                if (
                    runProjectSync(projectId) {
                        projectStore.resolveConflicts(projectId, ConflictResolution.KeepExternal)
                    }
                ) {
                    dismissedConflictSignature = conflictSignature
                }
            },
            onKeepInternal = {
                val projectId = projectState.conflicts.first().projectId
                if (
                    runProjectSync(projectId) {
                        projectStore.resolveConflicts(projectId, ConflictResolution.KeepInternal)
                    }
                ) {
                    dismissedConflictSignature = conflictSignature
                }
            },
            onDismiss = {
                dismissedConflictSignature = conflictSignature
            },
        )
        ProjectErrorDialog(
            message = projectState.errorMessage,
            onDismiss = projectStore::clearError,
        )
        // The local-apps error channel needs a presenter with the apps cover
        // DOWN. `uiState.error`'s only renderer is the dialog inside
        // `LocalAppsScreen`, which is composed only under `if (showingApps)`
        // — so a create started from the drawer (the cover is never opened on
        // that path) failed silently: "已有一个本地应用正在创建中",
        // "此构建未包含本地应用引擎", "创建结果未知，请在应用库确认" all landed on
        // state nobody drew. Same hole iOS plugged with its root-level alert
        // (`RootView.localAppErrorPresented`).
        //
        // Gated on `!showingApps`, which makes the two presenters PROVABLY
        // exclusive rather than merely unlikely to collide: `showingApps` is
        // the single boolean that decides whether `LocalAppsRoute` — and with
        // it the cover's own dialog — is in the composition at all. Yielding
        // costs nothing, because the cover coming up does not clear `error`;
        // the message is simply drawn by the other presenter.
        //
        // Same title and dismiss action as that dialog, so the copy does not
        // depend on which surface happened to be up.
        LocalAppsErrorDialog(
            message = if (showingApps) null else localAppsState.error,
            onDismiss = { localAppsViewModel.onAction(LocalAppsAction.DismissError) },
        )
    }
}

/**
 * Root-level presenter for `LocalAppsUiState.error`, for the states in which no
 * apps cover is mounted to show it. Deliberately shaped like
 * [ProjectErrorDialog] — the existing root-level error idiom — but titled with
 * the local-apps string, which is why it is not that composable reused.
 */
@Composable
private fun LocalAppsErrorDialog(
    message: String?,
    onDismiss: () -> Unit,
) {
    if (message == null) return
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.local_apps_action_failed_title)) },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_got_it)) }
        },
    )
}

internal fun computerUseSetupStatus(
    state: ComputerUseUiState,
    configuredPackages: Set<String> = emptySet(),
    browserPackages: Set<String>,
): ComputerUseSetupStatus =
    ComputerUseSetupStatus(
        accessibilityEnabled = state.serviceEnabled,
        browserAuthorized =
            state.grants.any { it.packageName in browserPackages } ||
                configuredPackages.any { it in browserPackages },
        sessionActive = state.sessionState in setOf(
            ComputerUseSessionState.Starting,
            ComputerUseSessionState.Active,
            ComputerUseSessionState.AwaitingApproval,
        ),
    )

/**
 * Returns the selected Computer Use apps that Android confirms can open web
 * links. Resolving each already-visible package explicitly avoids assuming a
 * vendor-specific browser package such as Chrome or MIUI Browser.
 */
internal fun resolveBrowserPackages(
    context: Context,
    candidatePackages: Set<String>,
): Set<String> {
    if (candidatePackages.isEmpty()) return emptySet()

    val webIntent = Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com"))
        .addCategory(Intent.CATEGORY_BROWSABLE)
    return candidatePackages.filterTo(linkedSetOf()) { packageName ->
        runCatching {
            context.packageManager.resolveActivity(
                Intent(webIntent).setPackage(packageName),
                PackageManager.MATCH_DEFAULT_ONLY,
            ) != null
        }.getOrDefault(false)
    }
}

internal fun shouldShowComputerUseSetup(
    readiness: ComputerUseSetupStatus?,
    dismissed: Boolean,
): Boolean = readiness?.ready == false && !dismissed

internal fun shouldReshowComputerUseSetup(
    readiness: ComputerUseSetupStatus?,
    requestKey: String?,
    handledRequestKey: String?,
): Boolean =
    readiness?.ready == false &&
        requestKey != null &&
        requestKey != handledRequestKey

private fun formatCronTime(epochMs: Long): String =
    java.time.Instant.ofEpochMilli(epochMs)
        .atZone(java.time.ZoneId.systemDefault())
        .format(java.time.format.DateTimeFormatter.ofPattern("MM-dd HH:mm"))

private fun cronStatusLabel(status: CronRunStatus, context: Context): String = when (status) {
    // Bare "已排队"/"运行中"/"已取消"/"已跳过" reuse the exact same copy already
    // extracted for the full Cron screen's per-run badge (cron/CronScreen.kt).
    // The "最近…" ("recently…") variants are this compact drawer summary's own,
    // distinct copy, so they get their own keys rather than reusing
    // cron_status_succeeded/chat_status_failed/chat_status_timed_out.
    CronRunStatus.Queued -> context.getString(R.string.cron_status_queued)
    CronRunStatus.Running -> context.getString(R.string.chat_status_running)
    CronRunStatus.Succeeded -> context.getString(R.string.cron_status_recent_succeeded)
    CronRunStatus.Failed -> context.getString(R.string.cron_status_recent_failed)
    CronRunStatus.TimedOut -> context.getString(R.string.cron_status_recent_timed_out)
    CronRunStatus.Cancelled -> context.getString(R.string.chat_status_cancelled)
    CronRunStatus.Skipped -> context.getString(R.string.cron_status_skipped)
}

internal fun appendVoiceTranscript(base: String, transcript: String): String = when {
    transcript.isBlank() -> base
    base.isBlank() -> transcript
    else -> "$base $transcript"
}

@Preview(showBackground = true)
@Composable
private fun RootScreenPreview() {
    LingXiTheme {
        RootScreen(isDark = true, onToggleTheme = {})
    }
}
