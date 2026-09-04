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
import com.lingxi.code.drawer.DrawerLocalAppWorkspace
import com.lingxi.code.drawer.DrawerProductionData
import com.lingxi.code.drawer.DrawerSection
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionCatalogPhase
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.model.persistedSessionTarget
import com.lingxi.code.model.sessionStateKey
import com.lingxi.code.model.toDto
import com.lingxi.code.model.toUi
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
import com.lingxi.code.model.ManagedLocalAppMcpSource
import com.lingxi.code.model.ManagedLocalAppToolSchema
import com.lingxi.code.model.LocalAppPluginStatus
import com.lingxi.code.bindings.AppEventDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.LocalAppPluginInventoryDto
import com.lingxi.code.bindings.ManagedLocalAppMcpServerDto
import com.lingxi.code.bindings.PluginActivationStateDto
import com.lingxi.code.bindings.PluginCommandDto
import com.lingxi.code.theme.LingXiTheme
import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.share.rememberShare
import com.lingxi.code.vision.rememberCameraCapture
import com.lingxi.code.model.Role
import com.lingxi.code.localapps.LocalAppsAction
import com.lingxi.code.localapps.LocalAppApprovalSheetDialog
import com.lingxi.code.localapps.LocalAppDetailsTab
import com.lingxi.code.localapps.LocalAppsDestination
import com.lingxi.code.localapps.LocalAppSessionPage
import com.lingxi.code.localapps.LocalAppsRoute
import com.lingxi.code.localapps.LocalAppsViewModel
import com.lingxi.code.localapps.LocalAppWorkflow
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
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
 * How long a created-app landing waits for a STREAMING TURN to finish before it
 * even attempts the scope switch.
 *
 * Separate from [LANDING_SWITCH_ATTEMPTS] x [LANDING_SWITCH_RETRY_MS] because
 * the two refusals `switchWorkspaceSource` raises have different timescales: a
 * competing switch clears in milliseconds, while a turn routinely runs for
 * minutes. Spending the 10 s switch budget on a streaming turn is what silently
 * dropped the hand-off. Still bounded — an unbounded wait would suspend inside
 * `collect` and strand every later landing.
 */
private const val LANDING_STREAM_WAIT_MS = 180_000L

/**
 * A conversation notification is usually tapped on a COLD start, so the first
 * attempt can land before the engine source is bound. Retry on the same budget
 * shape as the created-app landing, then report instead of dropping the route.
 */
private const val CONVERSATION_LAUNCH_ATTEMPTS = 40
private const val CONVERSATION_LAUNCH_RETRY_MS = 250L
private const val FORK_SESSION_EVENT_TIMEOUT_MS = 15_000L

private data class PendingForkRequest(
    val scope: ConversationScope,
    val sourceMode: SessionMode,
    val targetMode: SessionMode,
    val sourceSessionId: String,
    val sourceSessionTitle: String,
)

internal enum class SessionCatalogLookupState {
    Pending,
    Ready,
}

internal data class SessionCatalogLookup<T>(
    val state: SessionCatalogLookupState,
    val value: T? = null,
)

internal fun isBoundToScopeMode(
    boundScope: ConversationScope,
    boundMode: SessionMode,
    targetScope: ConversationScope,
    targetMode: SessionMode,
): Boolean = boundScope == targetScope && boundMode == targetMode

internal fun <T> localAppSessionCatalogLookup(
    appsLoading: Boolean,
    appPresent: Boolean,
    page: LocalAppSessionPage?,
    value: T?,
): SessionCatalogLookup<T> = when {
    value != null -> SessionCatalogLookup(SessionCatalogLookupState.Ready, value)
    appsLoading -> SessionCatalogLookup(SessionCatalogLookupState.Pending)
    page?.loaded == true || !appPresent -> SessionCatalogLookup(SessionCatalogLookupState.Ready)
    else -> SessionCatalogLookup(SessionCatalogLookupState.Pending)
}

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
    var activeSessionMode by rememberSaveable { mutableStateOf(SessionMode.Code) }
    val chatViewModel: ChatViewModel = viewModel ?: viewModel(
        key = "chat",
        factory = viewModelFactory {
            initializer {
                ChatViewModel(
                    source = EngineConversationSource.create(
                        context = appContext,
                        projectWorkspace = projectState.activeProject?.workspace,
                        workspaceKey = projectState.activeProject?.record?.id
                            ?.let { "project.$it" }
                            ?: "global",
                        sessionMode = activeSessionMode,
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
                        workspaceKey = projectState.activeProject?.record?.id
                            ?.let { "project.$it" }
                            ?: "global",
                        sessionMode = activeSessionMode,
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
            currentConversationId = {
                chatViewModel.state.value.session.id.takeUnless { it.isBlank() || it == "new" }
            },
        ),
    )
    val localAppsState by localAppsViewModel.uiState.collectAsStateWithLifecycle()
    val scopeStore = remember(appContext) { ScopeStateStore(appContext) }
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var drawerBootstrapComplete by rememberSaveable { mutableStateOf(false) }

    fun drawerSectionFor(mode: SessionMode): DrawerSection = when (mode) {
        SessionMode.Chat -> DrawerSection.Chat
        SessionMode.Code -> DrawerSection.Code
    }

    fun conversationModeFor(section: DrawerSection): SessionMode? = when (section) {
        DrawerSection.Chat -> SessionMode.Chat
        DrawerSection.Code -> SessionMode.Code
        DrawerSection.Cron -> null
    }

    fun setConversationMode(mode: SessionMode) {
        activeSessionMode = mode
        drawerUi.section = drawerSectionFor(mode)
    }

    fun projectSnapshotForScope(scope: ConversationScope): ProjectSnapshot? = when (scope) {
        ConversationScope.Global -> null
        is ConversationScope.Project -> projectState.projects.firstOrNull { it.record.id == scope.projectId }
        is ConversationScope.LocalApp -> null
    }

    fun workspaceForScope(scope: ConversationScope) = when (scope) {
        ConversationScope.Global -> null
        is ConversationScope.Project -> projectSnapshotForScope(scope)?.workspace
        is ConversationScope.LocalApp -> localAppWorkspace(
            appFilesRoot = appContext.filesDir,
            appId = scope.appId,
            workspaceRel = localAppsViewModel.uiState.value.apps
                .firstOrNull { it.id == scope.appId }
                ?.workspaceRel,
        )
    }

    fun restorableSession(
        scope: ConversationScope,
        sessionId: String,
        mode: SessionMode,
    ): SessionRow? = when (scope) {
        ConversationScope.Global -> projectState.globalSessions
            .firstOrNull { it.sessionId == sessionId && it.mode == mode }
            ?.let {
                SessionRow(
                    uuid = it.sessionId,
                    title = it.title,
                    messageCount = it.messageCount,
                    relativeTime = it.relativeTime,
                    mode = it.mode,
                    modifiedAtEpochSeconds = it.updatedAtEpochMillis / 1000L,
                )
            }
        is ConversationScope.Project -> projectState.projects
            .firstOrNull { it.record.id == scope.projectId }
            ?.sessions
            ?.firstOrNull { it.sessionId == sessionId && it.mode == mode }
            ?.let {
                SessionRow(
                    uuid = it.sessionId,
                    title = it.title,
                    messageCount = it.messageCount,
                    relativeTime = it.relativeTime,
                    mode = it.mode,
                    modifiedAtEpochSeconds = it.updatedAtEpochMillis / 1000L,
                )
            }
        is ConversationScope.LocalApp -> localAppsState.appSessions[scope.appId]
            ?.rows
            ?.firstOrNull { it.uuid == sessionId && it.mode == mode }
            ?.let {
                SessionRow(
                    uuid = it.uuid,
                    title = it.title,
                    messageCount = it.messageCount,
                    relativeTime = it.relativeTime,
                    mode = it.mode,
                    modifiedAtEpochSeconds = it.modifiedAtEpochSeconds,
                )
            }
    }

    fun localAppWorkspaceStatus(workflow: LocalAppWorkflow): String = when (workflow) {
        LocalAppWorkflow.Draft -> context.getString(R.string.local_apps_workflow_draft)
        LocalAppWorkflow.PublishedUnverified ->
            context.getString(R.string.local_apps_verification_status_unverified)
        LocalAppWorkflow.PublishedVerified ->
            context.getString(R.string.local_apps_verification_status_passed)
    }

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
    LaunchedEffect(requestedLocalAppLaunch, localAppsState.loading) {
        val request = requestedLocalAppLaunch ?: return@LaunchedEffect
        if (localAppsState.loading) return@LaunchedEffect
        setConversationMode(SessionMode.Code)
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
    // Local-app modal channels whose page-local presenters would disappear on a
    // page switch: raise the cover for the ones still owned by `LocalAppsScreen`.
    //
    // Tokenized approval receipts moved to a ROOT presenter in Phase 8, so they
    // survive both route switches and the drawer-driven create path. Only the
    // remaining screen-local prompts need this "show the apps surface" assist.
    LaunchedEffect(
        localAppsState.pendingAuthorization,
        localAppsState.pendingUiAction,
        localAppsState.pendingDependencyChangeConfirmation,
    ) {
        if (
            localAppsState.pendingAuthorization != null ||
            localAppsState.pendingUiAction != null ||
            localAppsState.pendingDependencyChangeConfirmation != null
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

    LaunchedEffect(scopeStore) {
        val restoredMode = scopeStore.readActiveMode() ?: SessionMode.Code
        activeSessionMode = restoredMode
        drawerUi.section = drawerSectionFor(restoredMode)
        val presentation = scopeStore.readWorkspacePresentation()
        drawerUi.replaceWorkspacePresentation(
            collapsed = presentation.filterValues { it.collapsed }.keys,
            pinned = presentation.mapNotNull { (key, state) ->
                state.pinnedAtEpochMillis?.let { key to it }
            }.toMap(),
        )
        drawerBootstrapComplete = true
    }

    LaunchedEffect(drawerUi.section, drawerBootstrapComplete) {
        if (!drawerBootstrapComplete) return@LaunchedEffect
        conversationModeFor(drawerUi.section)?.let { nextMode ->
            if (activeSessionMode != nextMode) activeSessionMode = nextMode
            scopeStore.persistActiveMode(nextMode)
        }
    }

    LaunchedEffect(sourceScope) {
        drawerUi.selectWorkspace(sourceScope.persistenceKey())
    }

    // Refresh the session catalog whenever the drawer transitions to open, so the
    // list is fresh each time the user reaches for it (the engine re-reports via
    // SessionList). `isOpen` flips on the open animation's start, so this fires
    // once per open, not per frame. An active app scope also refreshes its
    // workspace catalogs so every app group is complete, including apps that
    // have never been opened in this process.
    LaunchedEffect(drawerState.isOpen) {
        if (drawerState.isOpen) {
            chatViewModel.refreshSessions()
            cronRepository.refresh()
            localAppsState.apps.forEach {
                localAppsViewModel.onAction(LocalAppsAction.LoadAppSessions(it.id, null))
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
    val currentEngineSource by chatViewModel.engineSource.collectAsStateWithLifecycle()
    var pendingForkRequest by remember { mutableStateOf<PendingForkRequest?>(null) }
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
                } else if (event is ClientEvent.ProviderModelCatalog) {
                    resolvedSettingsStore.setLlmCatalogEntries(event.providers)
                } else if (event is ClientEvent.TypescriptLspModeChanged) {
                    resolvedSettingsStore.setTypescriptLspState(
                        requested = event.requested,
                        effective = event.effective,
                        available = event.available,
                    )
                } else if (event is ClientEvent.AppEvent) {
                    when (val appEvent = event.event) {
                        is AppEventDto.PluginStatusChanged -> {
                            resolvedSettingsStore.setLocalAppPluginEnabled(
                                appEvent.status.state == PluginActivationStateDto.LOADED,
                            )
                        }
                        is AppEventDto.PluginInventoryChanged -> {
                            resolvedSettingsStore.setLocalAppPluginStatus(
                                appEvent.inventory.toSettingsModel(
                                    prior = resolvedSettingsStore.state.value.localAppPlugin,
                                ),
                            )
                        }
                        is AppEventDto.ManagedMcpInventoryChanged -> {
                            resolvedSettingsStore.setManagedLocalAppInventory(
                                appEvent.servers.map { server ->
                                    server.toSettingsModel(context)
                                },
                            )
                        }
                        else -> Unit
                    }
                }
            }
    }
    val llmCatalogByProfileId = remember(settingsState.llmCatalogEntries) {
        settingsState.llmCatalogEntries.associateBy(LlmProviderCatalogEntry::profileId)
    }
    val modelProviderStatuses = remember(
        settingsState.llmProviders,
        settingsState.llmCatalogLoaded,
        llmCatalogByProfileId,
    ) {
        settingsState.llmProviders.map { provider ->
            val profileId = ProviderSettingsRepository.profileNameFor(provider)
            val catalogEntry = llmCatalogByProfileId[profileId]
            ModelProviderStatus(
                profileId = profileId,
                settingsId = provider.id,
                name = provider.name,
                status = provider.status,
                enabled = provider.enabled,
                credentialConfigured = provider.credentialConfigured,
                catalogModelIds = when {
                    !settingsState.llmCatalogLoaded -> null
                    catalogEntry == null -> emptyList()
                    else -> (
                        catalogEntry.modelIds +
                            catalogEntry.modelDetails.map(CatalogModelDetails::reference)
                        ).distinct()
                },
                showInModelPicker = provider.showInModelPicker,
                visibleModelIds = provider.visibleModelIds,
            )
        }
    }
    val engineMcp by chatViewModel.mcpServers.collectAsState()
    LaunchedEffect(chatViewModel, reconnectToken) { chatViewModel.refreshMcpServers() }
    LaunchedEffect(engineMcp) {
        resolvedSettingsStore.setMcpServers(engineMcp)
    }
    LaunchedEffect(currentEngineSource, reconnectToken, settingsState.localAppPlugin.pluginId) {
        val pluginId = settingsState.localAppPlugin.pluginId
        runCatching {
            currentEngineSource.submitClientCommand(
                com.lingxi.code.bindings.ClientCommand.PluginCommand(
                    PluginCommandDto.GetStatus(pluginId = pluginId),
                ),
            )
            currentEngineSource.submitClientCommand(
                com.lingxi.code.bindings.ClientCommand.PluginCommand(
                    PluginCommandDto.GetInventory(pluginId = pluginId),
                ),
            )
            currentEngineSource.submitClientCommand(
                com.lingxi.code.bindings.ClientCommand.PluginCommand(
                    PluginCommandDto.GetManagedMcpInventory,
                ),
            )
        }
    }

    fun currentEngineMode(): SessionMode = currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code

    fun sourceMatches(scope: ConversationScope, mode: SessionMode): Boolean = isBoundToScopeMode(
        boundScope = sourceScope,
        boundMode = currentEngineMode(),
        targetScope = scope,
        targetMode = mode,
    )

    // The composer draft is hoisted here so a voice transcription (the
    // hold-to-talk release) can route its recognized text straight into the
    // input the user is about to send. ScopeStateStore is authoritative for the
    // `(workspace, mode)` draft; the legacy SavedStateHandle value is consulted
    // only once for a Code-mode process restore.
    var draft by remember { mutableStateOf(chatViewModel.restoredDraft) }
    var voiceDraftBase by remember { mutableStateOf("") }

    // Which scope's draft the composer is currently showing — guards the
    // restore effect below against unrelated recompositions.
    var draftScopeKey by remember { mutableStateOf<String?>(null) }

    // Swap the visible draft whenever workspace OR mode changes. Keyed on the
    // durable session-state key so Chat and Code can never overwrite each
    // other, even inside the same workspace.
    LaunchedEffect(sourceScope, activeSessionMode) {
        val key = sourceScope.sessionStateKey(activeSessionMode)
        if (key == draftScopeKey) return@LaunchedEffect
        val firstBind = draftScopeKey == null
        draftScopeKey = key
        val restored = scopeStore.read(key)?.draft
            ?: if (firstBind && activeSessionMode == SessionMode.Code) {
                chatViewModel.restoredDraft
            } else {
                ""
            }
        // The very first bind after process start must not wipe a draft the
        // user already restored (remember { } above) — only apply when the
        // stored value differs and this is a REAL scope change.
        if (!firstBind || restored.isNotEmpty() || activeSessionMode != SessionMode.Code) {
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
        allowInactiveWaitingRecovery: Boolean = false,
        sessionModeOverride: SessionMode = activeSessionMode,
    ): Boolean {
        val destination = target ?: SessionRef("new", context.getString(R.string.chat_new_conversation))
        var persisted: ProjectStoreState? = null
        return chatViewModel.switchWorkspaceSource(
            projectId = (engineScope as? ConversationScope.Project)?.projectId,
            target = destination,
            newSession = newSession,
            resumeEmpty = resumeEmpty,
            replacePendingTransition = replacePendingTransition,
            allowInactiveWaitingRecovery = allowInactiveWaitingRecovery,
            scope = engineScope,
            createSource = {
                EngineConversationSource.create(
                    context = appContext,
                    projectWorkspace = when (engineScope) {
                        ConversationScope.Global -> null
                        is ConversationScope.Project -> project?.workspace
                        is ConversationScope.LocalApp -> workspaceForScope(engineScope)
                    },
                    workspaceKey = engineScope.persistenceKey(),
                    sessionMode = sessionModeOverride,
                    linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                    reuseProcessSource = true,
                )
            },
            persistSelection = {
                persisted = projectStore.persistActive((engineScope as? ConversationScope.Project)?.projectId)
                scopeStore.persistActiveScope(engineScope.persistenceKey())
                scopeStore.persistActiveMode(sessionModeOverride)
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
        allowInactiveWaitingRecovery: Boolean = false,
        sessionModeOverride: SessionMode = activeSessionMode,
    ): Boolean = switchEngineScope(
        engineScope = project?.let { ConversationScope.Project(it.record.id) } ?: ConversationScope.Global,
        project = project,
        target = target,
        newSession = newSession,
        resumeEmpty = resumeEmpty,
        replacePendingTransition = replacePendingTransition,
        allowInactiveWaitingRecovery = allowInactiveWaitingRecovery,
        sessionModeOverride = sessionModeOverride,
    )

    LaunchedEffect(
        requestedConversationLaunch,
        projectState.loading,
        localAppsState.loading,
        sourceScope,
        currentEngineSource,
    ) {
        val request = requestedConversationLaunch ?: return@LaunchedEffect
        val targetScope = request.workspaceKey?.let(::conversationScopeFromKey) ?: sourceScope
        if (request.workspaceKey != null && conversationScopeFromKey(request.workspaceKey) == null) {
            chatViewModel.reportConversationLaunchFailed()
            onConversationLaunchHandled()
            return@LaunchedEffect
        }
        if (targetScope is ConversationScope.Project && projectState.loading) return@LaunchedEffect
        if (targetScope is ConversationScope.LocalApp && localAppsState.loading) return@LaunchedEffect
        if (targetScope is ConversationScope.Project && projectSnapshotForScope(targetScope) == null) {
            chatViewModel.reportConversationLaunchFailed()
            onConversationLaunchHandled()
            return@LaunchedEffect
        }

        showingApps = false
        val targetMode = request.sessionMode
        val boundMode = currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code
        val alreadyBound = sourceScope == targetScope && boundMode == targetMode
        var routed = false
        var attempt = 0
        while (!routed && attempt < CONVERSATION_LAUNCH_ATTEMPTS) {
            if (attempt > 0) delay(CONVERSATION_LAUNCH_RETRY_MS)
            attempt += 1
            routed = if (alreadyBound) {
                chatViewModel.openSessionFromNotification(
                    ref = SessionRef(request.sessionId, ""),
                    turnId = request.turnId,
                )
            } else {
                switchEngineScope(
                    engineScope = targetScope,
                    project = projectSnapshotForScope(targetScope),
                    target = SessionRef(request.sessionId, ""),
                    newSession = false,
                    replacePendingTransition = true,
                    allowInactiveWaitingRecovery = true,
                    sessionModeOverride = targetMode,
                )
            }
        }
        if (routed) {
            setConversationMode(targetMode)
        } else {
            chatViewModel.reportConversationLaunchFailed()
        }
        onConversationLaunchHandled()
    }

    suspend fun completeForkTransition(
        request: PendingForkRequest,
        forkedSessionId: String,
    ) {
        pendingForkRequest = null
        val switched = switchEngineScope(
            engineScope = request.scope,
            project = projectSnapshotForScope(request.scope),
            target = SessionRef(forkedSessionId, request.sourceSessionTitle),
            newSession = false,
            replacePendingTransition = true,
            sessionModeOverride = request.targetMode,
        )
        if (switched) {
            setConversationMode(request.targetMode)
            closeDrawer()
        }
    }

    suspend fun continueSessionInMode(
        sessionScope: ConversationScope,
        row: SessionRow,
        targetMode: SessionMode,
    ) {
        val request = PendingForkRequest(
            scope = sessionScope,
            sourceMode = row.mode,
            targetMode = targetMode,
            sourceSessionId = row.uuid,
            sourceSessionTitle = row.title,
        )
        if (pendingForkRequest != null) {
            chatViewModel.reportHostError(context.getString(R.string.drawer_continue_session_in_progress))
            return
        }
        val activeSourceMatchesScope =
            sourceScope == sessionScope &&
                (currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code) == row.mode
        if (activeSourceMatchesScope) {
            pendingForkRequest = request
            runCatching {
                coroutineScope {
                    val awaitEvent = async {
                        withTimeoutOrNull(FORK_SESSION_EVENT_TIMEOUT_MS) {
                            currentEngineSource.clientEvents.first { event ->
                                event is ClientEvent.SessionForked &&
                                    event.sourceSessionId == row.uuid &&
                                    event.mode.toUi() == targetMode
                            }
                        } as? ClientEvent.SessionForked
                    }
                    currentEngineSource.submitClientCommand(
                        ClientCommand.ForkSession(
                            sessionId = row.uuid,
                            targetMode = targetMode.toDto(),
                        ),
                    )
                    val event = awaitEvent.await()
                    if (event == null) {
                        pendingForkRequest = null
                        chatViewModel.reportHostError(context.getString(R.string.drawer_continue_session_timeout))
                    } else {
                        completeForkTransition(request, event.sessionId)
                    }
                }
            }.onFailure {
                pendingForkRequest = null
                chatViewModel.reportHostError(it.message ?: it::class.simpleName.orEmpty())
            }
            return
        }
        pendingForkRequest = request
        val forkSource = EngineConversationSource.create(
            context = appContext,
            projectWorkspace = workspaceForScope(sessionScope),
            workspaceKey = sessionScope.persistenceKey(),
            sessionMode = row.mode,
            linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
            reuseProcessSource = false,
        )
        if (forkSource is com.lingxi.code.conversation.UnavailableConversationSource) {
            pendingForkRequest = null
            forkSource.close()
            chatViewModel.reportHostError(forkSource.reason)
            return
        }
        try {
            coroutineScope {
                val awaitEvent = async {
                    withTimeoutOrNull(FORK_SESSION_EVENT_TIMEOUT_MS) {
                        forkSource.clientEvents.first { event ->
                            event is ClientEvent.SessionForked &&
                                event.sourceSessionId == row.uuid &&
                                event.mode.toUi() == targetMode
                        }
                    } as? ClientEvent.SessionForked
                }
                forkSource.submitClientCommand(
                    ClientCommand.ForkSession(
                        sessionId = row.uuid,
                        targetMode = targetMode.toDto(),
                    ),
                )
                val event = awaitEvent.await()
                if (event == null) {
                    pendingForkRequest = null
                    chatViewModel.reportHostError(context.getString(R.string.drawer_continue_session_timeout))
                    return@coroutineScope
                }
                completeForkTransition(request, event.sessionId)
            }
        } catch (error: Throwable) {
            pendingForkRequest = null
            chatViewModel.reportHostError(error.message ?: error::class.simpleName.orEmpty())
        } finally {
            forkSource.close()
        }
    }

    LaunchedEffect(
        activeSessionMode,
        sourceScope,
        currentEngineSource,
        drawerBootstrapComplete,
        state.streaming,
        state.sessionTransitioning,
        state.session.id,
        state.sessionReady,
        projectState.loading,
        projectState.projects,
        projectState.globalSessions,
        localAppsState.loading,
        localAppsState.appSessions,
    ) {
        if (!drawerBootstrapComplete || state.streaming || state.sessionTransitioning) return@LaunchedEffect
        val engineMode = currentEngineMode()
        if (engineMode == activeSessionMode) return@LaunchedEffect
        val savedSessionId = scopeStore.read(sourceScope.sessionStateKey(activeSessionMode))
            ?.lastActiveSessionId
        val target = persistedSessionTarget(
            sessionId = savedSessionId,
            catalogRow = savedSessionId?.let {
                restorableSession(sourceScope, it, activeSessionMode)
            },
        )
        switchEngineScope(
            engineScope = sourceScope,
            project = projectSnapshotForScope(sourceScope),
            target = target?.ref,
            newSession = target == null,
            resumeEmpty = target?.resumeEmpty == true,
            replacePendingTransition = true,
            sessionModeOverride = activeSessionMode,
        )
    }

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
              // The whole body is wrapped so a cancellation MID-BODY re-arms
              // the landing instead of losing it. `receiveAsFlow()` hands the
              // element to this body before it runs, and `repeatOnLifecycle
              // (RESUMED)` cancels this coroutine the instant the lifecycle
              // drops below RESUMED — entirely normal if the user backgrounds
              // the app while this body is suspended in the retry loop's
              // `delay` or either `withTimeoutOrNull` wait below, which can
              // together span several seconds. Without this, that ordinary
              // backgrounding permanently drops the just-created app's landing
              // with no retry and no error.
              try {
                setConversationMode(SessionMode.Code)
                showingApps = false
                // The result is load-bearing, not decoration: `switchWorkspaceSource`
                // REFUSES while a turn is streaming (or another switch is pending)
                // and returns false after only raising a banner — it does not begin
                // a transition. Ignoring that left the wait below satisfied by the
                // CURRENT project conversation, and the kickoff was then sent into
                // it: the app's agent rooted in the wrong directory, which is the
                // exact failure the comment above says was observed on device.
                // Retry, the way iOS's `openCreatedAppSession` does.
                //
                // But the two refusals need two different waits. A competing
                // switch clears in milliseconds; a STREAMING TURN does not, and
                // routinely outlives the whole 40 x 250 ms budget below. Paying
                // for it out of that budget is what dropped the hand-off into
                // the (previously empty) `else` at the bottom: the user was left
                // with an orphan draft app and only the unrelated
                // 「请先停止当前任务，再切换项目。」 chat banner, which says nothing
                // about the app that was just created. iOS parks the landing
                // until the turn ends; wait for it here on a turn-sized budget
                // before spending the switch budget at all.
                if (chatViewModel.state.value.streaming) {
                    withTimeoutOrNull(LANDING_STREAM_WAIT_MS) {
                        chatViewModel.state.first { !it.streaming }
                    }
                }
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
                    val ready = withTimeoutOrNull(SESSION_READY_TIMEOUT_MS) {
                        chatViewModel.state.first {
                            it.sessionReady && !it.sessionTransitioning && it.session.id.isNotBlank()
                        }
                    }
                    if (ready != null) {
                        // Readiness says WHEN, never WHICH. The predicate above
                        // is satisfied by whatever conversation becomes ready —
                        // including the user's previous one if the transition was
                        // abandoned or a competing switch won — so re-check the
                        // scope, and that the transcript is still empty, before
                        // firing. Without this the first message of the interview
                        // can land in the wrong conversation, rooting the agent
                        // outside the app workspace. iOS checks scope, target
                        // session id and emptiness at its own send site.
                        val landed = chatViewModel.sourceScope.value ==
                            ConversationScope.LocalApp(landing.appId)
                        if (landed && chatViewModel.state.value.messages.isEmpty()) {
                            // Placeholder-free copy: a shell has no brief to
                            // interpolate, and the old text told the agent the
                            // shape and the name were "already fixed", which is
                            // exactly what the conversation now exists to decide.
                            chatViewModel.send(context.getString(R.string.local_apps_kickoff))
                        }
                    } else {
                        // The scope switch itself succeeded, but the session
                        // never reported ready within budget: without this, the
                        // kickoff is silently dropped and nothing tells the user
                        // anything happened.
                        //
                        // Reusing the switch-exhausted branch's banner here is a
                        // KNOWN imprecision, deliberately accepted, not an
                        // oversight. By this point the app exists and the user is
                        // already sitting inside its conversation (the cover was
                        // closed at the top of this body), so 「创建结果未知，
                        // 请在应用库确认。」 both overstates the damage and stays
                        // silent about the one thing that actually went wrong:
                        // the interview kickoff was never sent, so the user has
                        // to type the first message themselves.
                        //
                        // Saying that needs its OWN string key, and every
                        // local_apps_ string is generated — one entry per locale
                        // under clients/translations plus a generate.py run,
                        // never a hand-edit of the generated Android or iOS
                        // catalogs. An approximate banner is still strictly
                        // better than the silent drop this branch replaces;
                        // sharpen the copy the next time the catalogs are
                        // touched for other reasons.
                        localAppsViewModel.reportCreatedAppLandingExhausted()
                    }
                } else {
                    // The switch budget is spent and the landing came off a
                    // one-shot channel, so nothing else will carry the user in.
                    // Say so against the app, rather than leaving only the
                    // switch's own banner, which never names it.
                    localAppsViewModel.reportCreatedAppLandingExhausted()
                }
              } catch (c: CancellationException) {
                localAppsViewModel.rearmCreatedAppLanding(landing)
                throw c
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
              // See the created-app landing above for why this is wrapped:
              // same one-shot channel, same `repeatOnLifecycle(RESUMED)`
              // cancellation hazard.
              try {
                setConversationMode(SessionMode.Code)
                val target = landing.sessionId?.let { SessionRef(it, "") }
                if (chatViewModel.sourceScope.value == ConversationScope.LocalApp(landing.appId)) {
                    // Already inside this app: an in-place session switch, so
                    // the engine source is not needlessly rebound. This branch
                    // cannot be refused, so closing the cover up front is safe.
                    showingApps = false
                    if (target == null) {
                        chatViewModel.startNewSession()
                    } else {
                        drawerUi.selectSession(target.id)
                        chatViewModel.openSession(target, empty = true)
                    }
                } else {
                    // Unlike the in-place branch, this CAN be refused (a
                    // streaming turn, or a pending transition) — closing the
                    // cover before knowing that strands the user outside the
                    // library with only the switch's own generic banner and no
                    // way back to the draft they just tapped. Close it only on
                    // success. `replacePendingTransition = true` matches every
                    // drawer call site: a transient pending-transition refusal
                    // should not out-rank a fresh tap.
                    if (
                        switchEngineScope(
                            engineScope = ConversationScope.LocalApp(landing.appId),
                            project = null,
                            target = target,
                            newSession = target == null,
                            resumeEmpty = true,
                            replacePendingTransition = true,
                        )
                    ) {
                        showingApps = false
                    }
                }
              } catch (c: CancellationException) {
                localAppsViewModel.rearmDraftSessionLanding(landing)
                throw c
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
        activeSessionMode,
    ) {
        val project = projectState.activeProject
        if (
            !projectState.loading &&
                project != null &&
                sourceProjectId == null &&
                chatViewModel.sourceScope.value !is ConversationScope.LocalApp
        ) {
            val scopedLastActiveSessionId = scopeStore.read(
                ConversationScope.Project(project.record.id).sessionStateKey(activeSessionMode),
            )?.lastActiveSessionId
            val legacyLastActiveSessionId = project.record.lastActiveSessionId
                .takeIf { activeSessionMode == SessionMode.Code }
            val lastSummary = (scopedLastActiveSessionId ?: legacyLastActiveSessionId)
                ?.let { id -> project.sessions.firstOrNull { it.sessionId == id } }
                ?.takeIf { it.mode == activeSessionMode }
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
    LaunchedEffect(projectState.loading, sourceScope, activeSessionMode) {
        if (projectState.loading || appScopeRestoreAttempted) return@LaunchedEffect
        if (sourceScope != ConversationScope.Global) return@LaunchedEffect
        val persistedScope = conversationScopeFromKey(scopeStore.readActiveScopeKey())
        if (persistedScope is ConversationScope.LocalApp) {
            appScopeRestoreAttempted = true
            val lastSessionId = scopeStore.read(
                persistedScope.sessionStateKey(activeSessionMode),
            )?.lastActiveSessionId
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

    // Persist the mode-scoped last-active session once the engine confirms it.
    LaunchedEffect(
        sourceScope,
        activeSessionMode,
        currentEngineSource,
        state.session.id,
        state.sessionReady,
    ) {
        val active = sourceScope
        val engineMode = currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code
        if (
            engineMode == activeSessionMode &&
            state.sessionReady &&
            state.session.id != "new"
        ) {
            runCatching {
                scopeStore.persistLastActiveSession(
                    active.sessionStateKey(activeSessionMode),
                    state.session.id,
                )
            }
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
        sourceScope,
    ) {
        if (
            sessionState.phase == SessionCatalogPhase.Ready &&
            sourceScope !is ConversationScope.LocalApp
        ) {
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
        sourceScope,
        currentEngineSource,
    ) {
        val engineMode = currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code
        if (
            engineMode == activeSessionMode &&
            state.sessionReady &&
            state.isNew &&
            state.session.id != "new" &&
            sourceScope !is ConversationScope.LocalApp
        ) {
            runCatching {
                projectStore.recordStartedSession(
                    projectId = sourceProjectId,
                    sessionId = state.session.id,
                    title = state.session.title,
                    mode = activeSessionMode,
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
            mode = cached.mode,
            modifiedAtEpochSeconds = cached.updatedAtEpochMillis / 1000L,
        )
    }
    val globalDrawerSessions = if (sourceScope == ConversationScope.Global) {
        sessionState.withCachedRows(cachedGlobalRows)
    } else {
        EngineSessionState.ready(cachedGlobalRows)
    }
    fun sessionRowLookup(
        scope: ConversationScope,
        sessionId: String,
        mode: SessionMode,
    ): SessionCatalogLookup<SessionRow> {
        val row = when (scope) {
            ConversationScope.Global -> globalDrawerSessions.rows
                .firstOrNull { it.uuid == sessionId && it.mode == mode }
            is ConversationScope.Project -> projectState.projects
                .firstOrNull { it.record.id == scope.projectId }
                ?.sessions
                ?.firstOrNull { it.sessionId == sessionId && it.mode == mode }
                ?.let {
                    SessionRow(
                        uuid = it.sessionId,
                        title = it.title,
                        messageCount = it.messageCount,
                        relativeTime = it.relativeTime,
                        mode = it.mode,
                        modifiedAtEpochSeconds = it.updatedAtEpochMillis / 1000L,
                    )
                }
            is ConversationScope.LocalApp -> localAppsState.appSessions[scope.appId]
                ?.rows
                ?.firstOrNull { it.uuid == sessionId && it.mode == mode }
                ?.let {
                    SessionRow(
                        uuid = it.uuid,
                        title = it.title,
                        messageCount = it.messageCount,
                        relativeTime = it.relativeTime,
                        mode = it.mode,
                        modifiedAtEpochSeconds = it.modifiedAtEpochSeconds,
                    )
                }
        }
        return when (scope) {
            ConversationScope.Global -> if (row != null || globalDrawerSessions.phase != SessionCatalogPhase.Loading) {
                SessionCatalogLookup(SessionCatalogLookupState.Ready, row)
            } else {
                SessionCatalogLookup(SessionCatalogLookupState.Pending)
            }
            is ConversationScope.Project -> if (row != null || !projectState.loading) {
                SessionCatalogLookup(SessionCatalogLookupState.Ready, row)
            } else {
                SessionCatalogLookup(SessionCatalogLookupState.Pending)
            }
            is ConversationScope.LocalApp -> localAppSessionCatalogLookup(
                appsLoading = localAppsState.loading,
                appPresent = localAppsState.apps.any { it.id == scope.appId },
                page = localAppsState.appSessions[scope.appId],
                value = row,
            )
        }
    }
    var restoredModeSessionKey by rememberSaveable { mutableStateOf<String?>(null) }
    LaunchedEffect(
        activeSessionMode,
        sourceScope,
        drawerBootstrapComplete,
        state.streaming,
        state.sessionTransitioning,
        currentEngineSource,
        globalDrawerSessions.phase,
        globalDrawerSessions.rows,
        projectState.loading,
        projectState.projects,
        localAppsState.loading,
        localAppsState.appSessions,
    ) {
        if (!drawerBootstrapComplete || state.streaming || state.sessionTransitioning) return@LaunchedEffect
        val engineMode = currentEngineMode()
        if (engineMode != activeSessionMode) return@LaunchedEffect
        val restoreKey = sourceScope.sessionStateKey(activeSessionMode)
        if (restoreKey == restoredModeSessionKey) return@LaunchedEffect
        val sessionId = scopeStore.read(restoreKey)?.lastActiveSessionId
        if (sessionId == null) {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        if (sessionId == state.session.id || sessionId == "new") {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        val restoredRowLookup = sessionRowLookup(sourceScope, sessionId, activeSessionMode)
        if (restoredRowLookup.state == SessionCatalogLookupState.Pending) return@LaunchedEffect
        val restoredRow = restoredRowLookup.value ?: run {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        drawerUi.selectSession(restoredRow.uuid)
        chatViewModel.openSession(
            SessionRef(restoredRow.uuid, restoredRow.title),
            empty = restoredRow.messageCount == 0,
        )
        restoredModeSessionKey = restoreKey
    }
    val drawerProductionData = DrawerProductionData(
        workspaces = listOf(LocalProjectWorkspace),
        projects = projectState.projects.map { it.toDrawerProject() },
        localAppWorkspaces = localAppsState.apps.map { app ->
            DrawerLocalAppWorkspace(
                appId = app.id,
                name = localAppDisplayName(
                    app,
                    draftTitle = context.getString(R.string.local_apps_draft_card_title),
                    fallback = app.id,
                ),
                status = localAppWorkspaceStatus(app.workflow),
                updatedAtEpochSeconds = app.updatedAtMs / 1000L,
                sessions = localAppsState.appSessions[app.id]?.rows.orEmpty(),
            )
        },
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
        if (drawerUi.activeWs.isBlank()) drawerUi.selectWorkspace(ConversationScope.Global.persistenceKey())
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
                            val row = globalDrawerSessions.rows.firstOrNull { it.uuid == ref.id }
                            val targetMode = row?.mode ?: activeSessionMode
                            val targetScope = ConversationScope.Global
                            val switched = if (sourceMatches(targetScope, targetMode)) {
                                drawerUi.selectSession(ref.id)
                                chatViewModel.openSession(ref, empty = row?.messageCount == 0)
                                true
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            engineScope = targetScope,
                                            project = null,
                                            target = ref,
                                            newSession = false,
                                            resumeEmpty = row?.messageCount == 0,
                                            replacePendingTransition = true,
                                            sessionModeOverride = targetMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            }
                            if (switched) closeDrawer()
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
                        onNewGlobalSession = {
                            showingApps = false
                            if (sourceMatches(ConversationScope.Global, activeSessionMode)) {
                                chatViewModel.startNewSession()
                                closeDrawer()
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            project = null,
                                            target = null,
                                            newSession = true,
                                            replacePendingTransition = true,
                                            sessionModeOverride = activeSessionMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                            }
                        },
                        onResumeSession = { uuid ->
                            showingApps = false
                            globalDrawerSessions.rows.firstOrNull { it.uuid == uuid }?.let { row ->
                                // Resume directly only when the live engine IS
                                // the global scope — a Project OR LocalApp
                                // scope must rebind first, or the session would
                                // resume against the wrong cwd.
                                if (sourceMatches(ConversationScope.Global, row.mode)) {
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
                                                replacePendingTransition = true,
                                                sessionModeOverride = row.mode,
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
                                        mode = row.mode,
                                        modifiedAtEpochSeconds = row.modifiedAtEpochSeconds,
                                        isInit = row.isInit,
                                    )
                                },
                            )
                        },
                        onSelectAppScopeSession = { ref ->
                            // Same engine scope (the section only renders for
                            // the ACTIVE app), so a plain in-place resume.
                            showingApps = false
                            val appId = (sourceScope as? ConversationScope.LocalApp)?.appId
                            val row = appId?.let { id ->
                                localAppsState.appSessions[id]?.rows?.firstOrNull { it.uuid == ref.id }
                            }
                            val targetMode = row?.mode ?: activeSessionMode
                            val targetScope = appId?.let(ConversationScope::LocalApp)
                            val switched = if (targetScope != null && sourceMatches(targetScope, targetMode)) {
                                drawerUi.selectSession(ref.id)
                                chatViewModel.openSession(ref, empty = row?.messageCount == 0)
                                true
                            } else if (targetScope != null) {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            engineScope = targetScope,
                                            project = null,
                                            target = ref,
                                            newSession = false,
                                            resumeEmpty = row?.messageCount == 0,
                                            replacePendingTransition = true,
                                            sessionModeOverride = targetMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            } else {
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onNewAppScopeSession = {
                            showingApps = false
                            val appId = (sourceScope as? ConversationScope.LocalApp)?.appId
                            val targetScope = appId?.let(ConversationScope::LocalApp)
                            val switched = if (
                                targetScope != null &&
                                    sourceMatches(targetScope, activeSessionMode)
                            ) {
                                chatViewModel.startNewSession()
                                true
                            } else if (targetScope != null) {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            engineScope = targetScope,
                                            project = null,
                                            target = null,
                                            newSession = true,
                                            replacePendingTransition = true,
                                            sessionModeOverride = activeSessionMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            } else {
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onSelectLocalAppSession = { appId, ref ->
                            showingApps = false
                            val row = localAppsState.appSessions[appId]?.rows?.firstOrNull { it.uuid == ref.id }
                            val targetMode = row?.mode ?: activeSessionMode
                            val targetScope = ConversationScope.LocalApp(appId)
                            val switched = if (sourceMatches(targetScope, targetMode)) {
                                drawerUi.selectSession(ref.id)
                                chatViewModel.openSession(ref, empty = row?.messageCount == 0)
                                true
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            engineScope = ConversationScope.LocalApp(appId),
                                            project = null,
                                            target = ref,
                                            newSession = false,
                                            resumeEmpty = row?.messageCount == 0,
                                            replacePendingTransition = true,
                                            sessionModeOverride = targetMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onNewLocalAppSession = { appId ->
                            showingApps = false
                            val targetScope = ConversationScope.LocalApp(appId)
                            val switched = if (sourceMatches(targetScope, activeSessionMode)) {
                                chatViewModel.startNewSession()
                                true
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            engineScope = ConversationScope.LocalApp(appId),
                                            project = null,
                                            target = null,
                                            newSession = true,
                                            replacePendingTransition = true,
                                            sessionModeOverride = activeSessionMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
                                }
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onContinueSession = { sessionScope, row, targetMode ->
                            showingApps = false
                            scope.launch { continueSessionInMode(sessionScope, row, targetMode) }
                        },
                        onOpenLocalAppDetails = { appId ->
                            showingApps = true
                            closeDrawer()
                            localAppsViewModel.onAction(LocalAppsAction.OpenApp(appId))
                            localAppsViewModel.onAction(LocalAppsAction.SelectDetailsTab(LocalAppDetailsTab.Sessions))
                        },
                        onSelectSection = { section ->
                            drawerUi.section = section
                            conversationModeFor(section)?.let(::setConversationMode)
                        },
                        isWorkspaceCollapsed = { mode, workspaceKey -> drawerUi.isWorkspaceCollapsed(mode, workspaceKey) },
                        onToggleWorkspaceCollapsed = { mode, workspaceKey ->
                            drawerUi.toggleWorkspaceCollapsed(mode, workspaceKey)
                            scope.launch {
                                scopeStore.persistWorkspaceCollapsed(
                                    "${mode.wireKey}:$workspaceKey",
                                    drawerUi.isWorkspaceCollapsed(mode, workspaceKey),
                                )
                            }
                        },
                        pinnedAtEpochMillis = { mode, workspaceKey -> drawerUi.pinnedAt(mode, workspaceKey) },
                        onToggleWorkspacePinned = { mode, workspaceKey ->
                            drawerUi.toggleWorkspacePinned(mode, workspaceKey)
                            scope.launch {
                                scopeStore.persistWorkspacePinned(
                                    workspaceKey,
                                    drawerUi.pinnedAt(mode, workspaceKey),
                                )
                            }
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
                            val targetScope = ConversationScope.Project(projectId)
                            val switched = if (sourceMatches(targetScope, session.mode)) {
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
                                            replacePendingTransition = true,
                                            sessionModeOverride = session.mode,
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
                            val targetScope = ConversationScope.Project(projectId)
                            val switched = if (sourceMatches(targetScope, activeSessionMode)) {
                                chatViewModel.startNewSession()
                                true
                            } else {
                                scope.launch {
                                    if (
                                        switchEngineScope(
                                            project = project,
                                            target = null,
                                            newSession = true,
                                            replacePendingTransition = true,
                                            sessionModeOverride = activeSessionMode,
                                        )
                                    ) {
                                        closeDrawer()
                                    }
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
                            // Deliberately no conversation-mode switch here:
                            // this row only opens the library to LOOK at it, and
                            // forcing one as a side effect would swap the
                            // user's live Chat conversation to a Code session
                            // just from browsing — iOS's equivalent entry
                            // changes no mode, and the sibling
                            // `onOpenLocalAppDetails` handler elsewhere in this
                            // file already opens the apps cover with no mode
                            // flip.
                            showingApps = true
                            closeDrawer()
                            // A library-origin create ("+" on a library card)
                            // parks the cover on the new shell's Details page
                            // (see `createShellApp` / `openApp`), and nothing
                            // else resets it. Without this, browsing here after
                            // such a create reopens onto that armed Details
                            // destination — an unscaffolded shell with nothing
                            // to show — instead of the library the user tapped
                            // into. `Refresh` alone does not touch destination.
                            localAppsViewModel.openLibrary()
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
                            val targetMode = sessionRow?.mode ?: SessionMode.Code
                            setConversationMode(targetMode)
                            showingApps = false
                            val target = sessionRow?.let { SessionRef(it.uuid, it.title) }
                            val targetScope = ConversationScope.LocalApp(appId)
                            if (sourceMatches(targetScope, targetMode)) {
                                if (target == null) {
                                    chatViewModel.startNewSession()
                                } else {
                                    drawerUi.selectSession(target.id)
                                    chatViewModel.openSession(target, empty = sessionRow.messageCount == 0)
                                }
                            } else {
                                scope.launch {
                                    switchEngineScope(
                                        engineScope = targetScope,
                                        project = null,
                                        target = target,
                                        newSession = target == null,
                                        resumeEmpty = sessionRow?.messageCount == 0,
                                        replacePendingTransition = true,
                                        sessionModeOverride = targetMode,
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
                            val active = sourceScope
                            scope.launch {
                                scopeStore.persistDraft(
                                    active.sessionStateKey(activeSessionMode),
                                    it,
                                )
                            }
                            // Keep only the legacy Code slot mirrored for old
                            // process-state restores; mode-scoped storage above
                            // is authoritative for every workspace.
                            if (activeSessionMode == SessionMode.Code) {
                                chatViewModel.onDraftChanged(it)
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
        localAppsState.pendingApprovalSheet?.let { sheet ->
            LocalAppApprovalSheetDialog(
                sheet = sheet,
                onAction = localAppsViewModel::onAction,
            )
        }
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

private fun LocalAppPluginInventoryDto.toSettingsModel(
    prior: LocalAppPluginStatus,
): LocalAppPluginStatus = LocalAppPluginStatus(
    pluginId = pluginId,
    displayName = displayName,
    version = version,
    bundleDigest = bundleSha256.take(12),
    enabled = state == PluginActivationStateDto.LOADED,
    skillsCount = counts.skills.toInt(),
    agentsCount = counts.agents.toInt(),
    workflowsCount = counts.workflows.toInt(),
    templatesCount = counts.templates.toInt(),
    validationError = validationError ?: prior.validationError,
)

private fun ManagedLocalAppMcpServerDto.toSettingsModel(context: Context): ManagedLocalAppMcpSource =
    ManagedLocalAppMcpSource(
        stableServerName = serverName,
        appName = appName,
        appId = appId,
        buildDigestSummary = buildId.take(12),
        catalogDigestSummary = catalogSha256.take(12),
        toolCount = toolCount.toInt(),
        authoringRevision = "r${authoringRevision}",
        uiVerification = uiVerification.summary,
        mcpVerification = mcpVerification.summary,
        schemaSummary = toolSurfaceSha256.take(12),
        annotationSummary = tools.firstNotNullOfOrNull { it.annotationsJson }
            ?: context.getString(R.string.common_none),
        permissionCeiling = tools.firstOrNull()?.permissionCeiling
            ?: context.getString(R.string.common_none),
        toolSchemas = tools.map { tool ->
            ManagedLocalAppToolSchema(
                name = tool.name,
                permissionSummary = tool.permissionCeiling,
            )
        },
    )

@Preview(showBackground = true)
@Composable
private fun RootScreenPreview() {
    LingXiTheme {
        RootScreen(isDark = true, onToggleTheme = {})
    }
}
