package com.lingxi.code

import com.lingxi.code.conversation.blocksEngineReconnect

import android.content.pm.PackageManager
import android.util.Base64
import androidx.compose.foundation.background
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
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
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
import com.lingxi.code.conversation.effortLabel
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
import com.lingxi.code.account.AccountViewModel
import com.lingxi.code.drawer.DrawerAccount
import com.lingxi.code.drawer.DrawerContent
import com.lingxi.code.drawer.DrawerProductionData
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.conversation.SessionPanel
import com.lingxi.code.conversation.SessionPanelModel
import com.lingxi.code.conversation.WorkspaceDirectorySheet
import com.lingxi.code.conversation.WorkspaceFilesDialog
import com.lingxi.code.conversation.permissionModeLabel
import com.lingxi.code.project.ProjectPage
import com.lingxi.code.project.ProjectsPage
import com.lingxi.code.apps.UnifiedAppRuntimePage
import com.lingxi.code.apps.UnifiedAppsPage
import com.lingxi.code.apps.UnifiedLocalApps
import com.lingxi.code.apps.UnifiedAppDetailsPage
import com.lingxi.code.apps.AppAuthoringSessionContext
import com.lingxi.code.apps.AppProjectRef
import com.lingxi.code.apps.isCanonicalUuidValue
import com.lingxi.code.conversation.LocalAppCardLinks
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionCatalogPhase
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
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
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.theme.LingXiTheme
import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lingxi.code.share.rememberShare
import com.lingxi.code.vision.rememberCameraCapture
import com.lingxi.code.model.Role
import com.lingxi.code.model.sessionCatalogStrings
import com.lingxi.code.voice.FlowModeOverlay
import com.lingxi.code.voice.rememberFlowVoiceController
import com.lingxi.code.voice.VoiceFlowOverlay
import com.lingxi.code.voice.cancelActiveHeldVoiceSession
import com.lingxi.code.voice.rememberVoiceCapture
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import android.graphics.BitmapFactory
import android.widget.Toast
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
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

internal fun isBoundToScope(
    boundScope: ConversationScope,
    targetScope: ConversationScope,
): Boolean = boundScope == targetScope

internal fun isRestorableSession(
    expectedId: String,
    candidateId: String,
    candidateDirectory: String? = null,
    activeDirectory: String? = null,
): Boolean = canonicalSessionId(expectedId) == canonicalSessionId(candidateId) &&
    (activeDirectory == null || candidateDirectory == activeDirectory)

internal fun shouldAttemptModeRestore(
    ready: Boolean,
    transitioning: Boolean,
    streaming: Boolean,
    engineMode: SessionMode,
    activeMode: SessionMode,
    restoreKey: String,
    completedKey: String?,
): Boolean = ready && !transitioning && !streaming && engineMode == activeMode && restoreKey != completedKey

internal fun shouldPersistActiveSession(
    ready: Boolean,
    sessionId: String,
    engineMode: SessionMode,
    activeMode: SessionMode,
    activeKey: String,
    restoredKey: String?,
): Boolean = ready && sessionId != "new" && engineMode == activeMode && restoredKey == activeKey

internal fun sameDraftOwner(
    previousScope: ConversationScope?,
    previousSessionId: String?,
    scope: ConversationScope,
    sessionId: String,
): Boolean = previousScope == scope && previousSessionId != null && sessionId != "new" &&
    canonicalSessionId(previousSessionId) == canonicalSessionId(sessionId)

/** A temporary draft is carried only into the new session that owns it. */
internal fun shouldTransferNewDraft(
    previousScope: ConversationScope?, previousSessionId: String?,
    scope: ConversationScope, sessionId: String, transitioning: Boolean, isNew: Boolean,
    newDraftScope: ConversationScope?,
): Boolean = previousScope == scope && previousSessionId == "new" && sessionId != "new" &&
    !transitioning && isNew && newDraftScope == scope

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
    modifier: Modifier = Modifier,
    onOpenSettings: () -> Unit = {},
    onOpenModelSettings: () -> Unit = {},
    onOpenPermissionSettings: () -> Unit = { onOpenSettings() },
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    onOpenComputerUseSettings: () -> Unit = { onOpenSettings() },
    onOpenVoiceSettings: () -> Unit = { onOpenSettings() },
    onOpenCronSettings: (String?) -> Unit = { onOpenSettings() },
    /** Open the scheduled-task page on a new task prefilled with the given prompt. */
    onOpenCronCreate: (prompt: String) -> Unit = { onOpenCronSettings(null) },
    onOpenTerminal: (sessionId: String, initCommand: String?) -> Unit = { _, _ -> },
    /** Open the Remote Control screens, which the host draws above this one. */
    onOpenRemoteControl: () -> Unit = {},
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
    onConversationBusyChanged: (Boolean) -> Unit = {},
    viewModel: ChatViewModel? = null,
    requestedDeepLink: com.lingxi.code.navigation.DeepLink? = null,
    onDeepLinkHandled: (com.lingxi.code.navigation.DeepLink) -> Unit = {},
    onDeepLinkBlockingChanged: (Boolean) -> Unit = {},
    onDeepLinkImportReady: (com.lingxi.code.navigation.DeepLink) -> Unit = {},
    onDeepLinkNativeReady: (com.lingxi.code.navigation.DeepLink) -> Unit = {},
    requestedConversationLaunch: ConversationLaunchRequest? = null,
    onConversationLaunchHandled: () -> Unit = {},
    // 设置页请求切换到的项目 id。设置页自己做不了这件事：项目层/本地层写到哪个目录
    // 由引擎的 cwd 决定，换 cwd 就要重建会话源，而那套状态机（switchEngineScope）
    // 住在这里。走的是和 requestedConversationLaunch 完全相同的「宿主持有请求、
    // RootScreen 消费后回调清除」形状，而不是另起一套只给设置页用的切换逻辑。
    requestedProjectSwitch: String? = null,
    onProjectSwitchHandled: () -> Unit = {},
) {
    val context = LocalContext.current
    val resources by rememberUpdatedState(androidx.compose.ui.platform.LocalResources.current)
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
            projectState.projects.forEach { append('|').append(it.record.id).append('@').append(it.record.activeDirectory) }
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
    var lastSelectedMode by rememberSaveable { mutableStateOf(SessionMode.Code) }
    // Only a deliberately started session may inherit the temporary new-chat draft.
    var newDraftScope by remember { mutableStateOf<ConversationScope?>(null) }
    val scopeStore = remember(appContext) { ScopeStateStore(appContext) }
    var restoredModeSessionKey by rememberSaveable { mutableStateOf<String?>(null) }
    var drawerBootstrapComplete by remember { mutableStateOf(false) }
    val chatViewModel: ChatViewModel = viewModel ?: viewModel(
        key = "chat",
        factory = viewModelFactory {
            initializer {
                ChatViewModel(
                    source = com.lingxi.code.conversation.ConnectingConversationSource(conversationStrings(appContext)),
                    savedState = createSavedStateHandle(),
                    sourceGeneration = reconnectToken - 1,
                    strings = conversationStrings(appContext),
                    backgroundExecution = AndroidConversationBackgroundExecution(appContext),
                )
            }
        },
    )
    LaunchedEffect(chatViewModel, reconnectToken, drawerBootstrapComplete, projectState.loading, projectState.activeProjectId) {
        if (!drawerBootstrapComplete || projectState.loading) return@LaunchedEffect
        if (viewModel == null) {
                val initialBinding = chatViewModel.engineSource.value is com.lingxi.code.conversation.ConnectingConversationSource
                val launchScope = if (initialBinding) {
                    projectState.activeProject?.record?.id?.let { ConversationScope.Project(it) }
                        ?: if (scopeStore.readActiveScopeKey() == ConversationScope.Scheduled.persistenceKey()) {
                            ConversationScope.Scheduled
                        } else ConversationScope.Global
                } else chatViewModel.sourceScope.value
                val launchWorkspaceKey = launchScope.persistenceKey()
                val launchSessionId = if (initialBinding) {
                    val slotId = scopeStore.read(launchScope.sessionStateKey(activeSessionMode))?.lastActiveSessionId
                        ?.takeUnless { it.isBlank() || it == "new" }
                    val selected = chatViewModel.prepareInitialSessionRestore(slotId?.let { SessionRef(it, "") }, launchScope)
                    newDraftScope = launchScope.takeIf { selected == null }
                    // This startup target has already been selected. Waiting for
                    // its confirmation must not reopen an older mode-slot owner.
                    restoredModeSessionKey = launchScope.sessionStateKey(activeSessionMode)
                    selected?.id
                } else chatViewModel.state.value.session.id.takeUnless { it == "new" }
                if (!initialBinding && launchSessionId == null) newDraftScope = launchScope
                val ownerMode = com.lingxi.code.conversation.DurableConversationTurnClientStore.ownerModeForSession(
                    appContext, launchWorkspaceKey, launchSessionId,
                ) ?: activeSessionMode
                val replacing = chatViewModel.engineSource.value !is com.lingxi.code.conversation.ConnectingConversationSource
                chatViewModel.ensureSourceInBackground(reconnectToken) {
                    EngineConversationSource.create(
                        context = appContext,
                        projectWorkspace = when (launchScope) {
                            ConversationScope.Global -> null
                            ConversationScope.Scheduled -> com.lingxi.code.project.ProjectWorkspace(
                                projectId = "b51bca68-b85f-4caa-8881-07dd33eba24d",
                                hostPath = com.lingxi.code.cron.CronScope.global(appContext).workspacePath,
                                guestPath = com.lingxi.code.project.MobileUserStorage.directories(appContext).scheduledGuestCwd,
                            )
                            is ConversationScope.Project -> projectState.projects
                                .firstOrNull { it.record.id == launchScope.projectId }?.workspace
                        },
                        workspaceKey = launchWorkspaceKey,
                        sessionMode = ownerMode,
                        linuxRuntimeMode = settingsStore?.state?.value?.linuxRuntime?.selectedMode
                            ?: com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux,
                        replaceProcessSource = replacing,
                        expectedSessionId = launchSessionId,
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
        ?: LinuxRuntimeMode.MobileLinux
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
    val pendingPermission by chatViewModel.pendingPermission.collectAsState()
    val sideChats by chatViewModel.sideChats.chats.collectAsState()
    val hasActiveEngineWork = state.blocksEngineReconnect(hasPendingPermission = pendingPermission != null)
    LaunchedEffect(hasActiveEngineWork) { onConversationBusyChanged(hasActiveEngineWork) }
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    // The conversation mode follows the session the user opens or starts; the
    // drawer's Chat/Code chips are only a list filter and never decide it.
    fun setConversationMode(mode: SessionMode) {
        activeSessionMode = mode
    }

    fun rememberModeSelection(mode: SessionMode) {
        lastSelectedMode = mode
        chatViewModel.rememberModeSelection(scopeStore, mode)
    }

    fun selectSessionMode(mode: SessionMode) {
        rememberModeSelection(mode)
        chatViewModel.setSessionMode(mode)
    }

    fun projectSnapshotForScope(scope: ConversationScope): ProjectSnapshot? = when (scope) {
        ConversationScope.Global, ConversationScope.Scheduled -> null
        is ConversationScope.Project -> projectState.projects.firstOrNull { it.record.id == scope.projectId }
    }

    fun resolveLinkedScope(key: String?): ConversationScope? {
        val scope = conversationScopeFromKey(key) ?: return null
        return scope.takeIf { it !is ConversationScope.Project || projectSnapshotForScope(it) != null }
    }

    fun workspaceForScope(scope: ConversationScope) = when (scope) {
        ConversationScope.Global -> null
        ConversationScope.Scheduled -> com.lingxi.code.project.ProjectWorkspace(
            projectId = "b51bca68-b85f-4caa-8881-07dd33eba24d",
            hostPath = com.lingxi.code.cron.CronScope.global(appContext).workspacePath,
            guestPath = com.lingxi.code.project.MobileUserStorage.directories(appContext).scheduledGuestCwd,
        )
        is ConversationScope.Project -> projectSnapshotForScope(scope)?.workspace
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
    // The engine's REAL resumable-session catalog (out-of-band, sibling of the
    // model catalog). The drawer renders its loading / empty / error states
    // directly and never falls back to mock sessions.
    val sessionState by chatViewModel.sessions.collectAsState()
    val sourceProjectId by chatViewModel.sourceProjectId.collectAsState()
    // Which workspace the live engine is bound to (Global / Scheduled / Project)
    // — the generalization of sourceProjectId.
    val sourceScope by chatViewModel.sourceScope.collectAsState()
    // Durable per-scope conversation state (last-active session + draft),
    // keyed `global` / `scheduled` / `project.<id>`. Project/global last-active
    // stays with ProjectStore; this store records which scope was active
    // across process death.

    var cronResultsSeenAt by remember { mutableLongStateOf(0L) }
    LaunchedEffect(scopeStore) {
        val restoredMode = scopeStore.readActiveMode() ?: SessionMode.Code
        activeSessionMode = restoredMode
        lastSelectedMode = chatViewModel.readSelectedMode(scopeStore) ?: SessionMode.Code
        drawerUi.replaceSessionPins(scopeStore.readPinnedSessions())
        // Results that already exist when this first runs are not "new".
        cronResultsSeenAt = scopeStore.readCronResultsSeenAt().takeIf { it > 0L }
            ?: System.currentTimeMillis().also { scopeStore.persistCronResultsSeenAt(it) }
        val presentation = scopeStore.readWorkspacePresentation()
        drawerUi.replaceWorkspacePresentation(
            collapsed = presentation.filterValues { it.collapsed }.keys,
            pinned = presentation.mapNotNull { (key, state) ->
                state.pinnedAtEpochMillis?.let { key to it }
            }.toMap(),
        )
        drawerBootstrapComplete = true
    }

    LaunchedEffect(activeSessionMode, drawerBootstrapComplete) {
        if (!drawerBootstrapComplete) return@LaunchedEffect
        scopeStore.persistActiveMode(activeSessionMode)
    }

    LaunchedEffect(sourceScope) {
        drawerUi.selectWorkspace(sourceScope.persistenceKey())
    }

    // Refresh the session catalog whenever the drawer transitions to open, so the
    // list is fresh each time the user reaches for it (the engine re-reports via
    // SessionList). `isOpen` flips on the open animation's start, so this fires
    // once per open, not per frame.
    LaunchedEffect(drawerState.isOpen) {
        if (drawerState.isOpen) {
            chatViewModel.refreshSessions()
            cronRepository.refresh()
        }
    }

    // Voice-flow overlay visibility, hoisted here (the Android analog of the iOS
    // RootView's `voiceActive` flag): the composer's mic long-press flips it on,
    // releasing the held finger flips it off. The overlay renders above the
    // drawer + conversation.
    var voiceActive by remember { mutableStateOf(false) }
    var audioGuidance by remember { mutableStateOf<com.lingxi.code.voice.AudioSetupGuidance?>(null) }
    val captureState by com.lingxi.code.voice.VoiceCaptureStore.state.collectAsState()
    LaunchedEffect(captureState.phase, captureState.errorMessage) {
        if (captureState.phase == com.lingxi.code.voice.VoiceCapturePhase.Failed) {
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(message = captureState.errorMessage ?: captureState.statusText)
            voiceActive = false
        }
    }

    // FlowMode (心流) voice-orb overlay visibility — a TAP on the composer mic
    // flips it on (the long-press STT path still drives `voiceActive`). The
    // overlay renders above the drawer + conversation + voice-flow overlay.
    var flowActive by remember { mutableStateOf(false) }
    var appInForeground by remember {
        mutableStateOf(lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED))
    }

    // Flow uses this actual Agent conversation; its audio controller pins each admitted route.
    val flowVoiceController = rememberFlowVoiceController()
    val flowVoiceState by flowVoiceController.state.collectAsState()
    LaunchedEffect(flowVoiceState.error) {
        flowVoiceState.error?.let { error ->
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(error.kind.name, error.message)
            flowActive = false
        }
    }
    val orbAssistantText = (state.streamingMessage ?: state.messages.lastOrNull())
        ?.let { if (it.role == Role.Ai) it.text else "" } ?: ""
    LaunchedEffect(chatViewModel, state.session.id, activeSessionMode, sourceScope) {
        flowVoiceController.pause()
        voiceSpeechPlayer.stop()
        audioGuidance = null
    }
    LaunchedEffect(flowVoiceController, state.model.id, state.model.providerId) {
        flowVoiceController.updateSessionBinding(state.model.id, state.model.providerId)
    }
    DisposableEffect(lifecycleOwner, flowVoiceController) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START, Lifecycle.Event.ON_RESUME -> {
                    appInForeground = true
                    chatViewModel.sideChats.resumeForeground()
                }
                Lifecycle.Event.ON_STOP -> {
                    chatViewModel.sideChats.cancelCurrentTurns()
                    appInForeground = false
                    flowVoiceController.pause()
                    cancelActiveHeldVoiceSession()
                    scope.launch { runCatching { AndroidAudioServiceProvider.invalidate(context) } }
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
    LaunchedEffect(settingsState.voice, settingsState.llmProviders) {
        audioGuidance = null
    }
    // The notifier holds ARMED TIMERS (upstream's 60s idle delay, 6s permission
    // delay), so a preference change has to be pushed at it rather than polled.
    // Keyed on the value, not just the ViewModel, so every edit lands.
    LaunchedEffect(chatViewModel, settingsState.notifs) {
        chatViewModel.setNotificationPreferences(settingsState.notifs)
    }
    val currentEngineSource by chatViewModel.engineSource.collectAsStateWithLifecycle()
    DisposableEffect(resolvedSettingsStore, currentEngineSource) {
        resolvedSettingsStore.bindProviderCredentialSource(currentEngineSource)
        onDispose { resolvedSettingsStore.bindProviderCredentialSource(null) }
    }
    LaunchedEffect(currentEngineSource, settingsState.voice, settingsState.llmProviders, state.session.id, state.model.id, appInForeground) {
        val audioSource = currentEngineSource as? com.lingxi.code.conversation.EngineConversationSource
        if (audioSource != null) audioSource.activateAudioContext() else com.lingxi.code.voice.audio.AndroidAudioSessionContext.clear()
        resolvedSettingsStore.setVoiceCapability(com.lingxi.code.settings.probeVoiceCapabilitySnapshot(context, settingsState.voice))
    }
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
            runCatching {
                voiceSpeechPlayer.speak(text, onError = { error ->
                    audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(error.kind.name, error.message)
                })
            }.onFailure { error ->
                if (error !is kotlinx.coroutines.CancellationException) {
                    audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(message = error.message.orEmpty())
                }
            }
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
    fun currentEngineMode(): SessionMode = state.sessionMode ?: currentEngineSource.recoverySpec?.sessionMode ?: SessionMode.Code

    fun sourceMatches(scope: ConversationScope): Boolean = isBoundToScope(sourceScope, scope)

    // The composer draft is hoisted here so a voice transcription (the
    // hold-to-talk release) can route its recognized text straight into the
    // input the user is about to send. Drafts follow the conversation owner,
    // including when its profile changes, rather than a workspace/mode slot.
    val initialSavedDraft = remember(chatViewModel) { chatViewModel.restoredDraft }
    var draft by remember { mutableStateOf("") }
    val conversationSource by chatViewModel.engineSource.collectAsState()
    val visualizationHost = conversationSource.visualizationHost
    var voiceDraftBase by remember { mutableStateOf("") }

    // The currently displayed draft and serialized writes belong to this owner.
    var draftOwnerScope by remember { mutableStateOf<ConversationScope?>(null) }
    var draftOwnerSessionId by remember { mutableStateOf<String?>(null) }
    var draftRevision by remember { mutableLongStateOf(0L) }
    var draftLoaded by remember { mutableStateOf(false) }
    var mayRestoreSavedDraft by remember { mutableStateOf(true) }

    fun queueDraftWrite(ownerScope: ConversationScope?, sessionId: String?, text: String): Job? {
        if (ownerScope == null || sessionId == null || sessionId == "new") return null
        return chatViewModel.persistConversationDraft(scopeStore, ownerScope, sessionId, text)
    }

    fun replaceOwnedDraft(ownerScope: ConversationScope, sessionId: String, text: String) {
        draftOwnerScope = ownerScope
        draftOwnerSessionId = canonicalSessionId(sessionId)
        draftRevision++
        draftLoaded = true
        draft = text
        chatViewModel.onDraftChanged(text, ownerScope, sessionId)
        queueDraftWrite(ownerScope, sessionId, text)
    }

    fun queueCurrentDraft(): Job? = if (draftLoaded) queueDraftWrite(draftOwnerScope, draftOwnerSessionId, draft) else null

    fun updateCurrentDraft(text: String) {
        draft = text
        draftRevision++
        draftLoaded = true
        queueCurrentDraft()
        val ownerScope = draftOwnerScope
        val ownerId = draftOwnerSessionId
        if (ownerScope != null && ownerId != null) chatViewModel.onDraftChanged(text, ownerScope, ownerId)
    }

    LaunchedEffect(chatViewModel) {
        chatViewModel.sideChats.failedDraft.collect { failed ->
            if (failed != null && draft.isEmpty()) updateCurrentDraft(failed)
        }
    }

    fun startCurrentScopeSession(mode: SessionMode) {
        val previousKey = restoredModeSessionKey
        val previousSessionId = chatViewModel.state.value.session.id
        queueCurrentDraft()
        restoredModeSessionKey = sourceScope.sessionStateKey(mode)
        chatViewModel.startNewSession(mode)
        if (!chatViewModel.state.value.sessionTransitioning && chatViewModel.state.value.session.id == previousSessionId) {
            restoredModeSessionKey = previousKey
        } else {
            val sessionId = chatViewModel.state.value.session.id
            newDraftScope = sourceScope.takeIf { sessionId == "new" }
            replaceOwnedDraft(sourceScope, sessionId, "")
        }
    }

    // Mode changes do not replace the source, session or its draft.
    LaunchedEffect(drawerBootstrapComplete, state.sessionMode, state.pendingSessionMode, state.session.id, state.sessionReady, state.sessionTransitioning, sourceScope) {
        if (!drawerBootstrapComplete || !state.sessionReady || state.sessionTransitioning) return@LaunchedEffect
        val mode = state.sessionMode ?: return@LaunchedEffect
        val sameOwner = sameDraftOwner(draftOwnerScope, draftOwnerSessionId, sourceScope, state.session.id)
        state.pendingSessionMode?.takeIf { sameOwner }?.let { pending ->
            val key = sourceScope.sessionStateKey(pending)
            if (state.session.id != "new") scopeStore.persistLastActiveSession(key, state.session.id)
        }
        if (mode != activeSessionMode) {
            val key = sourceScope.sessionStateKey(mode)
            if (state.session.id != "new") scopeStore.persistLastActiveSession(key, state.session.id)
            restoredModeSessionKey = key
            activeSessionMode = mode
            chatViewModel.refreshSessions()
        }
    }

    LaunchedEffect(drawerBootstrapComplete, sourceScope, state.session.id, state.sessionTransitioning, state.isNew) {
        if (!drawerBootstrapComplete) return@LaunchedEffect
        val nextId = canonicalSessionId(state.session.id)
        if (draftOwnerScope == sourceScope && draftOwnerSessionId == nextId) return@LaunchedEffect
        val oldScope = draftOwnerScope
        val oldId = draftOwnerSessionId
        val transferNewDraft = shouldTransferNewDraft(
            oldScope, oldId, sourceScope, nextId, state.sessionTransitioning, state.isNew, newDraftScope,
        )
        if (transferNewDraft) {
            replaceOwnedDraft(sourceScope, nextId, draft)
            newDraftScope = null
            mayRestoreSavedDraft = false
            return@LaunchedEffect
        }
        queueCurrentDraft()
        val saved = if (mayRestoreSavedDraft && chatViewModel.restoredDraftFor(sourceScope, nextId) != null) initialSavedDraft else null
        if (nextId != "new") {
            newDraftScope = null
            mayRestoreSavedDraft = false
        }
        draftOwnerScope = sourceScope
        draftOwnerSessionId = nextId
        draftRevision++
        val revision = draftRevision
        draftLoaded = nextId == "new"
        draft = if (nextId == "new") saved.orEmpty() else ""
        if (nextId == "new") return@LaunchedEffect
        val owner = sourceScope
        val mode = state.sessionMode ?: activeSessionMode
        scope.launch {
            chatViewModel.awaitConversationDraftWrites()
            val restored = scopeStore.readConversationDraft(owner.persistenceKey(), nextId, mode) ?: saved.orEmpty()
            if (draftOwnerScope == owner && draftOwnerSessionId == nextId && draftRevision == revision) {
                draft = restored
                draftLoaded = true
                chatViewModel.onDraftChanged(restored, owner, nextId)
            }
        }
    }

    // Hold-to-talk → live transcription, gated on RECORD_AUDIO. The recognized
    // partial utterance replaces the live voice suffix while recognition is
    // running; the final result replaces that same suffix on release.
    val (startVoiceCapture, onVoiceHoldRelease) = rememberVoiceCapture(
        onTranscript = { transcript ->
            updateCurrentDraft(appendVoiceTranscript(voiceDraftBase, transcript))
        },
        onPartialTranscript = { partial ->
            updateCurrentDraft(appendVoiceTranscript(voiceDraftBase, partial))
        },
        onError = { kind, message ->
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(kind, message)
            voiceActive = false
        },
    )
    val onVoiceHoldStart: () -> Unit = {
        audioGuidance = null
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
     * Project scope [project] supplies the workspace snapshot (as before);
     * Global binds no workspace. The active scope is persisted per-scope alongside the
     * project store's active-project index.
     */
    suspend fun switchEngineScope(
        engineScope: ConversationScope,
        project: ProjectSnapshot?,
        target: SessionRef?,
        newSession: Boolean,
        newSessionDirectory: String? = null,
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
        allowInactiveWaitingRecovery: Boolean = false,
        sessionModeOverride: SessionMode = activeSessionMode,
    ): Boolean {
        val destination = target ?: SessionRef("new", resources.getString(R.string.chat_new_conversation))
        var persisted: ProjectStoreState? = null
        var previousProjectId: String? = null
        var previousScopeKey: String? = null
        var previousStoredScopeKey: String? = null
        var previousMode: SessionMode? = null
        var selectionOwnerScope: ConversationScope? = null
        var selectionOwnerMode: SessionMode? = null
        return chatViewModel.switchWorkspaceSource(
            projectId = (engineScope as? ConversationScope.Project)?.projectId,
            target = destination,
            newSession = newSession,
            newSessionDirectory = newSessionDirectory,
            newSessionMode = sessionModeOverride.takeIf { newSession },
            resumeEmpty = resumeEmpty,
            replacePendingTransition = replacePendingTransition,
            allowInactiveWaitingRecovery = allowInactiveWaitingRecovery,
            scope = engineScope,
            createSource = {
                val sessionId = if (newSession) null else target?.id
                val ownerMode = com.lingxi.code.conversation.DurableConversationTurnClientStore.ownerModeForSession(
                    appContext, engineScope.persistenceKey(), sessionId,
                ) ?: sessionModeOverride
                EngineConversationSource.createInBackground(
                    context = appContext,
                    projectWorkspace = when (engineScope) {
                        ConversationScope.Global -> null
                        ConversationScope.Scheduled -> workspaceForScope(engineScope)
                        is ConversationScope.Project -> project?.workspace
                    },
                    workspaceKey = engineScope.persistenceKey(),
                    sessionMode = ownerMode,
                    linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                    reuseProcessSource = true,
                    currentSource = chatViewModel.engineSource.value,
                    expectedSessionId = sessionId,
                )
            },
            persistSelection = {
                selectionOwnerScope = chatViewModel.sourceScope.value
                selectionOwnerMode = chatViewModel.engineSource.value.recoverySpec?.sessionMode
                previousProjectId = projectStore.state.value.activeProjectId
                previousStoredScopeKey = scopeStore.readActiveScopeKey()
                previousScopeKey = previousStoredScopeKey ?: chatViewModel.sourceScope.value.persistenceKey()
                previousMode = scopeStore.readActiveMode() ?: activeSessionMode
                persisted = projectStore.persistActive((engineScope as? ConversationScope.Project)?.projectId)
                scopeStore.persistActiveScope(engineScope.persistenceKey())
                scopeStore.persistActiveMode(sessionModeOverride)
            },
            rollbackSelection = {
                // Workspace transactions are serialized by the ViewModel.
                // A later workspace/mode owner must keep its own selection;
                // a same-workspace provider reconnect can still undo our write.
                if (selectionOwnerScope == chatViewModel.sourceScope.value &&
                    selectionOwnerMode == chatViewModel.engineSource.value.recoverySpec?.sessionMode &&
                    scopeStore.readActiveScopeKey() in setOf(previousStoredScopeKey, engineScope.persistenceKey()) &&
                    scopeStore.readActiveMode() in setOf(previousMode, sessionModeOverride)
                ) {
                    projectStore.persistActive(previousProjectId)
                    previousScopeKey?.let { scopeStore.persistActiveScope(it) }
                    previousMode?.let { scopeStore.persistActiveMode(it) }
                }
            },
            onCommitted = {
                projectStore.publishActive(checkNotNull(persisted))
                drawerUi.selectSession(destination.id)
                if (newSession) {
                    queueCurrentDraft()
                    newDraftScope = engineScope
                    replaceOwnedDraft(engineScope, "new", "")
                    restoredModeSessionKey = engineScope.sessionStateKey(sessionModeOverride)
                } else {
                    newDraftScope = null
                }
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

    LaunchedEffect(requestedProjectSwitch, projectState.loading, projectState.projects) {
        val projectId = requestedProjectSwitch ?: return@LaunchedEffect
        // 项目目录还没加载完就先不动：此时 projects 是空的，会把一个存在的项目
        // 误判成「不存在」然后把请求丢掉。
        if (projectState.loading) return@LaunchedEffect
        val project = projectState.projects.firstOrNull { it.record.id == projectId }
        if (project != null) {
            switchEngineScope(project = project, target = null, newSession = true)
        }
        onProjectSwitchHandled()
    }

    LaunchedEffect(
        requestedConversationLaunch,
        projectState.loading,
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
        if (targetScope is ConversationScope.Project && projectSnapshotForScope(targetScope) == null) {
            chatViewModel.reportConversationLaunchFailed()
            onConversationLaunchHandled()
            return@LaunchedEffect
        }
        val targetMode = request.sessionMode
        val alreadyBound = sourceScope == targetScope
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
            if (!alreadyBound) setConversationMode(targetMode)
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
        if (sourceScope == request.scope) {
            chatViewModel.openSession(SessionRef(forkedSessionId, request.sourceSessionTitle))
            closeDrawer()
            return
        }
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
            chatViewModel.reportHostError(resources.getString(R.string.drawer_continue_session_in_progress))
            return
        }
        val activeSourceMatchesScope = sourceScope == sessionScope
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
                        chatViewModel.reportHostError(resources.getString(R.string.drawer_continue_session_timeout))
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
        val forkSource = EngineConversationSource.createInBackground(
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
                    chatViewModel.reportHostError(resources.getString(R.string.drawer_continue_session_timeout))
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
        drawerBootstrapComplete,
        currentEngineSource,
    ) {
        val project = projectState.activeProject
        if (
            drawerBootstrapComplete &&
                currentEngineSource !is com.lingxi.code.conversation.ConnectingConversationSource &&
            !projectState.loading &&
                project != null &&
                sourceProjectId == null
        ) {
            val scopedLastActiveSessionId = scopeStore.read(
                ConversationScope.Project(project.record.id).sessionStateKey(activeSessionMode),
            )?.lastActiveSessionId
            val lastSummary = scopedLastActiveSessionId
                ?.let { id -> project.sessions.firstOrNull {
                    isRestorableSession(id, it.sessionId, it.workspaceDirectory, project.record.activeDirectory)
                } }
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

    // Persist the mode-scoped last-active session once the engine confirms it.
    LaunchedEffect(
        sourceScope,
        activeSessionMode,
        currentEngineSource,
        state.session.id,
        state.sessionReady,
        restoredModeSessionKey,
    ) {
        val active = sourceScope
        val engineMode = currentEngineMode()
        if (shouldPersistActiveSession(state.sessionReady, state.session.id, engineMode, activeSessionMode,
                active.sessionStateKey(activeSessionMode), restoredModeSessionKey)) {
            runCatching {
                scopeStore.persistLastActiveSession(
                    active.sessionStateKey(activeSessionMode),
                    state.session.id,
                )
            }
        }
    }

    LaunchedEffect(currentEngineSource, sourceProjectId) {
        val projectId = currentEngineSource.recoverySpec?.projectId ?: return@LaunchedEffect
        currentEngineSource.workspaceState.collect { workspace ->
            if (workspace != null) {
                runCatching { projectStore.recordWorkspace(projectId, workspace) }
                    .onFailure { chatViewModel.reportHostError(it.message.orEmpty()) }
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
        state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User },
        sourceScope,
    ) {
        if (
            sessionState.phase == SessionCatalogPhase.Ready &&
            sourceScope != ConversationScope.Scheduled
        ) {
            val provisionalSessionMayNotBeListed =
                state.isNew &&
                    (
                        state.session.id == "new" ||
                            sessionState.rows.none { it.uuid == state.session.id }
                        )
            if (!provisionalSessionMayNotBeListed) {
                runCatching { projectStore.syncEngineSessions(sourceProjectId, sessionState.rows) }
                    .onFailure { chatViewModel.reportHostError(resources.getString(R.string.session_index_save_failed_fmt, it.message.orEmpty())) }
            }
        }
    }
    // Publish the confirmed session as soon as the first local user message exists.
    // Pending rows survive catalog refresh until the engine persists them.
    LaunchedEffect(
        sourceProjectId,
        state.session.id,
        state.sessionReady,
        state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User }?.id,
        sourceScope,
        currentEngineSource,
    ) {
        val engineMode = currentEngineMode()
        if (
            engineMode == activeSessionMode &&
            state.sessionReady &&
            state.messages.any { it.role == com.lingxi.code.model.Role.User } &&
            state.session.id != "new" &&
            (if (sourceProjectId == null) projectState.globalSessions else projectState.projects.firstOrNull { it.record.id == sourceProjectId }?.sessions.orEmpty()).none { it.sessionId == state.session.id && it.messageCount > 0 } &&
            sourceScope != ConversationScope.Scheduled
        ) {
            runCatching {
                projectStore.recordStartedSession(
                    projectId = sourceProjectId,
                    sessionId = state.session.id,
                    title = state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User }?.text?.take(120) ?: state.session.title,
                    mode = activeSessionMode,
                    initialMessageCount = 1,
                )
                // The first prompt can follow /cd, before there was any row
                // for WorkspaceChanged to annotate. Preserve the engine's
                // original root and exact transcript instead of guessing them.
                currentEngineSource.workspaceState.value?.takeIf {
                    canonicalSessionId(it.sessionId) == canonicalSessionId(state.session.id)
                }?.let { workspace ->
                    sourceProjectId?.let { projectStore.recordWorkspace(it, workspace) }
                }
            }.onFailure {
                chatViewModel.reportHostError(resources.getString(R.string.session_index_save_new_failed_fmt, it.message.orEmpty()))
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

    val cachedGlobalRows = projectState.globalSessions.filterNot { it.isArchived }.map { cached ->
        SessionRow(
            uuid = cached.sessionId,
            title = cached.title,
            messageCount = cached.messageCount,
            relativeTime = cached.relativeTime,
            mode = cached.mode,
            modifiedAtEpochSeconds = cached.updatedAtEpochMillis / 1000L,
        )
    }
    val scheduledSessionRows = cronState.generatedSessions.filter { it.projectId == null && it.sessionId != null }
        .distinctBy { it.sessionId }.map { run ->
            SessionRow(uuid = run.sessionId!!, title = run.prompt.take(120), messageCount = 2,
                relativeTime = formatCronTime(run.finishedAtMs ?: run.triggeredAtMs),
                modifiedAtEpochSeconds = (run.finishedAtMs ?: run.triggeredAtMs) / 1000L)
        }
    val globalDrawerSessions = (if (sourceScope == ConversationScope.Global) {
        sessionState.withCachedRows(cachedGlobalRows, projectState.globalSessions.filter { it.pendingCatalogConfirmation }.map { it.sessionId }.toSet()).let { catalog ->
            val archivedIds = projectState.globalSessions.filter { it.isArchived }.map { it.sessionId }.toSet()
            catalog.copy(rows = catalog.rows.filterNot { it.uuid in archivedIds })
        }
    } else {
        EngineSessionState.ready(cachedGlobalRows)
    }).let { it.copy(rows = (it.rows + scheduledSessionRows).distinctBy(SessionRow::uuid)) }
    fun sessionRowLookup(
        scope: ConversationScope,
        sessionId: String,
    ): SessionCatalogLookup<SessionRow> {
        val row = when (scope) {
            ConversationScope.Scheduled -> scheduledSessionRows.firstOrNull { isRestorableSession(sessionId, it.uuid) }
            ConversationScope.Global -> globalDrawerSessions.rows
                .firstOrNull { isRestorableSession(sessionId, it.uuid) }
            is ConversationScope.Project -> projectState.projects
                .firstOrNull { it.record.id == scope.projectId }
                ?.let { project -> project.sessions.firstOrNull {
                    isRestorableSession(sessionId, it.sessionId, it.workspaceDirectory, project.record.activeDirectory)
                } }
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
        }
        return when (scope) {
            ConversationScope.Scheduled -> SessionCatalogLookup(SessionCatalogLookupState.Ready, row)
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
        }
    }
    LaunchedEffect(
        activeSessionMode,
        sourceScope,
        drawerBootstrapComplete,
        state.streaming,
        state.sessionTransitioning,
        state.sessionReady,
        state.sessionMode,
        currentEngineSource,
        globalDrawerSessions.phase,
        globalDrawerSessions.rows,
        projectState.loading,
        projectState.projects,
    ) {
        if (!drawerBootstrapComplete) return@LaunchedEffect
        val restoreKey = sourceScope.sessionStateKey(activeSessionMode)
        if (!shouldAttemptModeRestore(state.sessionReady, state.sessionTransitioning, state.streaming,
                currentEngineMode(), activeSessionMode, restoreKey, restoredModeSessionKey)) return@LaunchedEffect
        val sessionId = scopeStore.read(restoreKey)?.lastActiveSessionId
        if (sessionId == null) {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        if (sessionId == state.session.id || sessionId == "new") {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        val project = projectSnapshotForScope(sourceScope)
        if (project != null && project.sessions.none { isRestorableSession(sessionId, it.sessionId, it.workspaceDirectory, project.record.activeDirectory) }) {
            restoredModeSessionKey = restoreKey
            return@LaunchedEffect
        }
        val restoredRowLookup = sessionRowLookup(sourceScope, sessionId)
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
        workspaces = listOf(LocalProjectWorkspace.copy(name = stringResource(R.string.project_storage_local))),
        projects = projectState.projects.map { it.toDrawerProject(com.lingxi.code.conversation.conversationStrings(context)) },
        crons = cronState.tasks.map { cron ->
            val automation = com.lingxi.code.cron.CronAutomation.from(cron.task)
            val status = cron.activeRun?.status ?: cron.lastRun?.status
            Cron(
                id = "${cron.scope.scopeId}:${cron.task.id}",
                wsId = LocalProjectWorkspace.id,
                title = automation.name.ifBlank { cron.task.prompt.lineSequence().firstOrNull().orEmpty() }
                    ?.take(42)
                    ?.ifBlank { cron.task.id }
                    ?: cron.task.id,
                cron = cron.task.cron,
                next = cron.task.nextFireMs?.toLong()?.let(::formatCronTime)
                    ?: resources.getString(R.string.cron_no_next_fire),
                desc = buildString {
                    append(cron.scope.projectName)
                    append(" · ")
                    append(
                        when {
                            automation.status != "active" -> automation.status.replaceFirstChar { it.uppercase() }
                            cron.schedulingMode == CronSchedulingMode.Unsupported ->
                                cron.unsupportedReason ?: resources.getString(R.string.cron_unsupported_period_fallback)
                            status != null -> cronStatusLabel(status, context)
                            cron.schedulingMode == CronSchedulingMode.FifteenMinuteFallback ->
                                resources.getString(R.string.cron_fifteen_minute_patrol_short)
                            else -> resources.getString(R.string.cron_exact_alarm_short)
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
            chatViewModel.reportHostError(resources.getString(R.string.project_sync_in_progress_notice))
        } else {
            action()
        }
    }
    fun runProjectSync(projectId: String, action: () -> Unit): Boolean {
        val activeProjectIsExecuting =
            projectId == sourceProjectId && (state.streaming || state.sessionTransitioning)
        if (activeProjectIsExecuting) {
            chatViewModel.reportHostError(resources.getString(R.string.project_sync_stop_task_first_notice))
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
    var projectsPageOpen by rememberSaveable { mutableStateOf(false) }
    var projectPageId by rememberSaveable { mutableStateOf<String?>(null) }
    // 应用: the page, one app's page and the running app, each drawn over the last.
    var appsPageOpen by rememberSaveable { mutableStateOf(false) }
    var marketAppId by rememberSaveable { mutableStateOf<String?>(null) }
    var runtimeAppId by rememberSaveable { mutableStateOf<String?>(null) }
    val localApps = remember(appContext) { UnifiedLocalApps.get(appContext) }
    val appLibrary by localApps.state.collectAsState()
    val drawerApps = remember(appLibrary.apps, appLibrary.openedRevision) { localApps.drawerApps(appLibrary.apps) }
    var workspacePickerOpen by rememberSaveable { mutableStateOf(false) }
    var filesBrowserOpen by rememberSaveable { mutableStateOf(false) }
    var sessionPanelOpen by rememberSaveable { mutableStateOf(false) }
    var linkedDetailKey by rememberSaveable { mutableStateOf<String?>(null) }
    var linkedMessageUuid by rememberSaveable { mutableStateOf<String?>(null) }
    var linkedSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var linkedScopeModeKey by rememberSaveable { mutableStateOf<String?>(null) }
    var linkedMessageRequest by rememberSaveable { mutableLongStateOf(0L) }
    var linkedMessageSource by remember { mutableStateOf<com.lingxi.code.conversation.ConversationSource?>(null) }
    var linkedTargetError by rememberSaveable { mutableStateOf<String?>(null) }
    var draftLinkReview by remember { mutableStateOf<com.lingxi.code.navigation.DeepLink?>(null) }
    var approvedDraftLink by remember { mutableStateOf<com.lingxi.code.navigation.DeepLink?>(null) }
    var routingDeepLink by remember { mutableStateOf(false) }
    val latestRequestedDeepLink by rememberUpdatedState(requestedDeepLink)
    val latestScopeModeKey by rememberUpdatedState(sourceScope.persistenceKey())
    val latestSessionId by rememberUpdatedState(state.session.id.removePrefix("sess:"))
    androidx.compose.runtime.SideEffect {
        onDeepLinkBlockingChanged(linkedTargetError != null || linkedMessageUuid != null || draftLinkReview != null || routingDeepLink)
    }
    LaunchedEffect(sourceScope, activeSessionMode, currentEngineSource, state.session.id, state.sessionReady, state.sessionTransitioning, linkedMessageUuid) {
        if (linkedMessageUuid == null || !state.sessionReady || state.sessionTransitioning) return@LaunchedEffect
        if (linkedScopeModeKey != latestScopeModeKey || linkedSessionId != latestSessionId || (linkedMessageSource != null && linkedMessageSource !== currentEngineSource)) {
            linkedMessageUuid = null
            linkedMessageSource = null
        } else if (linkedMessageSource == null) {
            linkedMessageSource = currentEngineSource
        }
    }
    LaunchedEffect(requestedDeepLink, projectState.loading, state.sessionReady, state.sessionTransitioning, state.streaming, approvedDraftLink, sourceScope, currentEngineSource, runtimeAppId, flowActive, voiceActive, captureState.phase, attachment, showCreateProject, activeProjectSyncing, workspacePickerOpen, routingDeepLink, draftLinkReview, linkedTargetError, linkedMessageUuid) {
        val link = requestedDeepLink ?: return@LaunchedEffect
        if (runtimeAppId != null && link.route == "apps/$runtimeAppId" && link.params["review"] == "agent") {
            try {
                val appId = runtimeAppId!!
                val app = localApps.get(appId)
                val expectedRevision = link.params["revision"]?.toLongOrNull()
                check(expectedRevision == null || expectedRevision == app.currentRevision) {
                    "This Agent request belongs to a different application revision. Reopen the current app and retry."
                }
                localApps.setBrowserReviewRequest(appId, link.params["runId"])
                onDeepLinkHandled(link)
            } catch (error: Exception) {
                if (error is kotlinx.coroutines.CancellationException) throw error
                linkedTargetError = error.message ?: "Unable to open this Agent request."
                onDeepLinkBlockingChanged(true)
                chatViewModel.reportHostError(linkedTargetError!!)
                onDeepLinkHandled(link)
            }
            return@LaunchedEffect
        }
        if (routingDeepLink || draftLinkReview != null || linkedTargetError != null || linkedMessageUuid != null) return@LaunchedEffect
        if (projectState.loading || !state.sessionReady || state.sessionTransitioning || state.streaming || runtimeAppId != null || flowActive || voiceActive || attachment != null || showCreateProject || activeProjectSyncing || workspacePickerOpen) return@LaunchedEffect
        if (captureState.phase in setOf(com.lingxi.code.voice.VoiceCapturePhase.Starting, com.lingxi.code.voice.VoiceCapturePhase.Listening, com.lingxi.code.voice.VoiceCapturePhase.Stopping)) return@LaunchedEffect
        routingDeepLink = true
        scope.launch {
            val routingSource = chatViewModel.engineSource.value
            try {
                val parts = link.route.split('/')
                val id = parts.last()
                fun resolveScope(): ConversationScope? {
                    val key = link.params["workspace"]
                    if (key != null) return resolveLinkedScope(key)
                    if (parts.first() == "new") return sourceScope
                    val matches = buildList {
                        if (projectState.globalSessions.any { it.sessionId.removePrefix("sess:") == id.removePrefix("sess:") }) add(ConversationScope.Global)
                        if (cronState.generatedSessions.any { it.projectId == null && it.sessionId?.removePrefix("sess:") == id.removePrefix("sess:") }) add(ConversationScope.Scheduled)
                        projectState.projects.filter { project -> project.sessions.any { it.sessionId.removePrefix("sess:") == id.removePrefix("sess:") } }.forEach { add(ConversationScope.Project(it.record.id)) }
                    }
                    return matches.singleOrNull()
                }
                try {
                    if (parts.first() !in setOf("project", "apps", "market")) {
                        projectsPageOpen = false; projectPageId = null; appsPageOpen = false; marketAppId = null; sessionPanelOpen = false
                    }
                    if (parts.first() in setOf("settings", "provider", "extensions", "cron", "terminal", "runtime", "import") && link.params["workspace"] != null) {
                        val targetScope = resolveLinkedScope(link.params["workspace"])
                            ?: error(resources.getString(R.string.deep_link_workspace_missing))
                        if (sourceScope != targetScope) {
                            queueCurrentDraft()?.join()
                            check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope), null, true)) { resources.getString(R.string.deep_link_busy) }
                            return@launch
                        }
                    }
                    when (parts.first()) {
                        "settings", "provider", "extensions", "cron", "terminal" -> onDeepLinkNativeReady(link)
                        "session" -> {
                            val targetScope = resolveScope() ?: error(resources.getString(R.string.deep_link_conversation_ambiguous))
                            val rows = when (targetScope) {
                                is ConversationScope.Project -> projectSnapshotForScope(targetScope)?.sessions.orEmpty()
                                ConversationScope.Global -> projectState.globalSessions
                                ConversationScope.Scheduled -> emptyList()
                            }
                            val row = rows.firstOrNull { it.sessionId.removePrefix("sess:") == id.removePrefix("sess:") }
                            val scheduled = if (targetScope == ConversationScope.Scheduled) cronState.generatedSessions.firstOrNull { it.projectId == null && it.sessionId?.removePrefix("sess:") == id.removePrefix("sess:") } else null
                            check(row != null || scheduled != null) { resources.getString(R.string.deep_link_conversation_workspace_missing) }
                            val storedMode = row?.mode ?: SessionMode.Code
                            // A link may retain the mode at the time it was made.
                            // The indexed session and current owner determine the mode now.
                            val sameSession = targetScope == sourceScope && canonicalSessionId(state.session.id) == canonicalSessionId(id)
                            val mode = if (sameSession) activeSessionMode else storedMode
                            queueCurrentDraft()?.join()
                            if (!sameSession) {
                                check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope), SessionRef(id, row?.title ?: scheduled?.prompt.orEmpty().take(120)), false, sessionModeOverride = mode)) { resources.getString(R.string.deep_link_busy) }
                                setConversationMode(mode)
                            }
                            linkedSessionId = id.removePrefix("sess:")
                            linkedScopeModeKey = targetScope.persistenceKey()
                            linkedMessageSource = chatViewModel.engineSource.value
                            linkedMessageRequest += 1
                            linkedMessageUuid = link.params["message"]
                            if (linkedMessageUuid != null) onDeepLinkBlockingChanged(true)
                        }
                        "new" -> {
                            val targetScope = resolveScope() ?: error(resources.getString(R.string.deep_link_workspace_missing))
                            val mode = link.params["mode"]?.let { com.lingxi.code.model.sessionModeFromWireValue(it) } ?: activeSessionMode
                            val destinationDraft = if (targetScope == sourceScope) draft else ""
                            if (destinationDraft.isNotBlank() && approvedDraftLink != link) {
                                draftLinkReview = link
                                return@launch
                            }
                            queueCurrentDraft()?.join()
                            val prompt = link.payload["prompt"] as? String ?: ""
                            check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope), null, true, sessionModeOverride = mode)) { resources.getString(R.string.deep_link_busy) }
                            setConversationMode(mode)
                            replaceOwnedDraft(targetScope, chatViewModel.state.value.session.id, prompt)
                            approvedDraftLink = null
                        }
                        "import" -> onDeepLinkImportReady(link)
                        "project" -> {
                            val project = (resolveLinkedScope(id) as? ConversationScope.Project)?.projectId
                                ?: projectState.projects.firstOrNull { it.record.id == id }?.record?.id
                                ?: error(resources.getString(R.string.deep_link_workspace_missing))
                            projectPageId = project
                        }
                        "market" -> { appsPageOpen = true; marketAppId = id }
                        "apps" -> {
                            // Initial library refresh can still be in flight
                            // when a cold-start link reaches this route.
                            // Resolve against the durable owner, not UI cache.
                            val app = localApps.get(id)
                            if (link.params["review"] == "agent") {
                                val expectedRevision = link.params["revision"]?.toLongOrNull()
                                check(expectedRevision == null || expectedRevision == app.currentRevision) { resources.getString(R.string.deep_link_invalid) }
                                localApps.setBrowserReviewRequest(id, link.params["runId"])
                            }
                            runtimeAppId = id
                        }
                        "runtime" -> {
                            when (parts[1]) {
                                "activity" -> { check(id.removePrefix("sess:") == state.session.id.removePrefix("sess:")) { resources.getString(R.string.deep_link_runtime_missing) }; sessionPanelOpen = true }
                                "agent" -> { check(state.sessionAgents.any { it.agentId == id }) { resources.getString(R.string.deep_link_runtime_missing) }; linkedDetailKey = "agent:$id" }
                                "task" -> { check(state.backgroundTasks.containsKey(id)) { resources.getString(R.string.deep_link_runtime_missing) }; linkedDetailKey = "task:$id" }
                                "plan", "review" -> {
                                    val tools = state.messages.flatMap { it.blocks }.filterIsInstance<com.lingxi.code.conversation.MessageContent.Tool>().map { it.call } + state.agentRun?.tools.orEmpty().map { it.toToolCall() }
                                    check(tools.any { it.id == id && (if (parts[1] == "plan") it.planMarkdown != null else it.tool.contains("review", ignoreCase = true)) }) { resources.getString(R.string.deep_link_runtime_missing) }
                                    linkedDetailKey = "tool:$id"
                                }
                            }
                        }
                    }
                } catch (error: Exception) {
                    if (error is kotlinx.coroutines.CancellationException) throw error
                    if (latestRequestedDeepLink == link && chatViewModel.engineSource.value === routingSource) {
                        val message = error.message ?: resources.getString(R.string.deep_link_unable_open)
                        linkedTargetError = message
                        onDeepLinkBlockingChanged(true)
                        chatViewModel.reportHostError(message)
                    }
                }
                if (latestRequestedDeepLink == link) onDeepLinkHandled(link)
            } finally {
                routingDeepLink = false
            }
        }
    }
    linkedTargetError?.let { message ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { linkedTargetError = null },
            title = { androidx.compose.material3.Text(stringResource(R.string.deep_link_unable_open)) },
            text = { androidx.compose.material3.Text(message) },
            confirmButton = { androidx.compose.material3.TextButton(onClick = { linkedTargetError = null }) { androidx.compose.material3.Text(stringResource(R.string.common_ok)) } },
        )
    }
    if (draftLinkReview != null) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { val reviewed = draftLinkReview; draftLinkReview = null; reviewed?.let(onDeepLinkHandled) },
            title = { androidx.compose.material3.Text(stringResource(R.string.deep_link_replace_draft_title)) },
            text = { androidx.compose.material3.Text(stringResource(R.string.deep_link_replace_draft_body) + "\n\n" + (draftLinkReview?.payload?.get("prompt") as? String).orEmpty()) },
            confirmButton = { androidx.compose.material3.TextButton(onClick = { approvedDraftLink = draftLinkReview; draftLinkReview = null }) { androidx.compose.material3.Text(stringResource(R.string.deep_link_replace_draft)) } },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { val reviewed = draftLinkReview; draftLinkReview = null; reviewed?.let(onDeepLinkHandled) }) { androidx.compose.material3.Text(stringResource(R.string.deep_link_keep_draft)) } },
        )
    }
    val localAppCardLinks = remember(localApps, appLibrary.apps) {
        LocalAppCardLinks(
            open = { id ->
                runtimeAppId = id
                scope.launch { localApps.refresh() }
            },
            exists = { id -> localApps.refresh(); localApps.state.value.apps.any { it.id == id } },
            generation = appLibrary.apps,
        )
    }
    // Leaves the projects pages, e.g. once a project was created or imported from
    // them and the conversation has moved into it.
    fun closeProjectPages() {
        projectPageId = null
        projectsPageOpen = false
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
                        if (switchEngineScope(created, null, true)) {
                            closeDrawer()
                            closeProjectPages()
                        }
                }
            }
        } else if (uri != null && reauthorizeProjectId != null) {
            runProjectSync(reauthorizeProjectId) {
                projectStore.reauthorize(reauthorizeProjectId, uri)
            }
        }
    }

    // Scheduled-task runs are reached from the scheduled-task page, never from
    // the recents list.
    val drawerEngineSessions = globalDrawerSessions.let { catalog ->
        val scheduledIds = scheduledSessionRows.mapTo(mutableSetOf()) { it.uuid }
        catalog.copy(rows = catalog.rows.filterNot { it.uuid in scheduledIds })
    }
    val newCronResults = cronState.generatedSessions.count { run ->
        run.sessionId != null && (run.finishedAtMs ?: 0L) > cronResultsSeenAt
    }
    fun markCronResultsSeen() {
        val now = System.currentTimeMillis()
        cronResultsSeenAt = now
        scope.launch { scopeStore.persistCronResultsSeenAt(now) }
    }
    val accountModel: AccountViewModel = viewModel()
    val accountState by accountModel.state.collectAsStateWithLifecycle()
    val accountFallbackName = stringResource(R.string.settings_title_account)
    val drawerAccount = accountState.user?.let { user ->
        DrawerAccount(
            displayName = user.name ?: user.email ?: accountFallbackName,
            email = user.email,
        )
    }
    // A scheduled-task workspace is not a place to start chatting: new chats from
    // there go to the global workspace.
    val newChatScope = if (sourceScope == ConversationScope.Scheduled) ConversationScope.Global else sourceScope
    val liveWorkspace by currentEngineSource.workspaceState.collectAsStateWithLifecycle()
    val currentWorkspaceName = liveWorkspace?.cwd?.takeIf { it.isNotBlank() }?.let { path ->
        if (path == com.lingxi.code.project.MobileUserStorage.directories(context).guestHome) "~"
        else path.substringAfterLast('/').ifBlank { path }
    } ?: when (val current = newChatScope) {
        ConversationScope.Global, ConversationScope.Scheduled -> stringResource(R.string.common_global)
        is ConversationScope.Project ->
            drawerProductionData.projects.orEmpty().firstOrNull { it.id == current.projectId }?.name
                ?: stringResource(R.string.drawer_entry_projects)
    }

    /**
     * Open an existing project session. The recents list and the project page
     * mix modes, so the opened session decides the conversation mode: after a
     * rebind the mode is set explicitly, or the mode effect would bounce the
     * engine back to the previous mode.
     */
    fun openProjectSession(projectId: String, ref: SessionRef) {
        val project = projectState.projects.firstOrNull { it.record.id == projectId } ?: return
        val session = project.sessions.firstOrNull { it.sessionId == ref.id } ?: return
        val resumeTarget = SessionRef(session.sessionId, session.title)
        val targetScope = ConversationScope.Project(projectId)
        if (sourceMatches(targetScope)) {
            drawerUi.selectSession(resumeTarget.id)
            chatViewModel.openSession(resumeTarget, empty = session.messageCount == 0)
            closeDrawer()
            closeProjectPages()
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
                    setConversationMode(session.mode)
                    closeDrawer()
                    closeProjectPages()
                }
            }
        }
    }

    /**
     * Start a new conversation in [target] in [mode]. A workspace change
     * rebinds the engine; a mode change keeps the current workspace source.
     * [carryDraft] moves what the user already typed into the new conversation.
     */
    fun startNewSessionIn(
        target: ConversationScope,
        mode: SessionMode = lastSelectedMode,
        carryDraft: Boolean = false,
    ) {
        val project = projectSnapshotForScope(target)
        if (target is ConversationScope.Project && project == null) return
        val carried = draft
        if (sourceMatches(target)) {
            queueCurrentDraft()
            startCurrentScopeSession(mode)
            if (carryDraft && chatViewModel.state.value.sessionTransitioning) {
                replaceOwnedDraft(target, chatViewModel.state.value.session.id, carried)
            }
            closeDrawer()
            closeProjectPages()
        } else {
            scope.launch {
                queueCurrentDraft()?.join()
                if (
                    switchEngineScope(
                        engineScope = target,
                        project = project,
                        target = null,
                        newSession = true,
                        replacePendingTransition = true,
                        sessionModeOverride = mode,
                    )
                ) {
                    setConversationMode(mode)
                    if (carryDraft) replaceOwnedDraft(target, chatViewModel.state.value.session.id, carried)
                    closeDrawer()
                    closeProjectPages()
                }
            }
        }
    }

    /** Start a new conversation in a project, in the last explicitly selected mode. */
    fun startProjectSession(projectId: String, mode: SessionMode = lastSelectedMode) =
        startNewSessionIn(ConversationScope.Project(projectId), mode)

    fun openAppAuthoring(appId: String?, name: String, createdDirectory: String) {
        scope.launch {
            try {
                check(!state.streaming && !state.sessionTransitioning) { "当前会话正在执行，请先停止或等待完成。" }
                val appRecord = appId?.let { localApps.get(it) }
                val oldBinding = appRecord?.authoring
                val oldProjectRef = oldBinding?.projectRef?.takeIf { it.kind == "mobileProject" }?.projectId
                val boundProject = if (oldBinding != null && appRecord != null && isCanonicalUuidValue(oldProjectRef) &&
                    oldBinding.isBoundTo(appRecord.id, oldProjectRef!!, oldBinding.sourceDirectory)) {
                    projectStore.state.value.projects.firstOrNull { project ->
                        project.record.id == oldProjectRef && project.record.directory == oldBinding.sourceDirectory
                    }
                } else null
                val sessionBindings = if (boundProject != null && oldBinding != null) {
                    boundProject.sessions.filter { row ->
                        row.mode == SessionMode.Code &&
                            (row.workspaceDirectory ?: boundProject.record.activeDirectory) == oldBinding.sourceDirectory
                    }.associate { row ->
                        canonicalSessionId(row.sessionId) to AppAuthoringSessionContext(
                            workspaceId = oldBinding.workspaceId!!,
                            sourceDirectory = oldBinding.sourceDirectory,
                            projectRef = oldBinding.projectRef!!,
                            mode = "code",
                        )
                    }
                } else emptyMap()
                val resolution = appId?.let { localApps.prepareAuthoring(it, sessionBindings) }
                check(resolution?.action != "sourceMissing") { "源码目录已不可用，请从应用管理中恢复源码或创建获许可的副本。" }
                val directory = resolution?.sourceDirectory ?: createdDirectory
                val ownedProject = boundProject?.takeIf { it.record.directory == directory }
                    ?: projectStore.registerAuthoringWorkspace(name, directory)
                val project = ownedProject.copy(workspace = com.lingxi.code.project.ProjectWorkspace(ownedProject.record.id, localApps.hostFile(directory).absolutePath, directory))
                val targetSessionId = resolution?.authoring?.authoringSessionId
                    ?.takeIf { resolution.action == "resumeSession" && boundProject?.record?.id == ownedProject.record.id }
                val target = targetSessionId?.let { id -> project.sessions.firstOrNull { row ->
                    canonicalSessionId(row.sessionId) == canonicalSessionId(id) && row.mode == SessionMode.Code &&
                        (row.workspaceDirectory ?: project.record.activeDirectory) == directory
                } }
                val mode = target?.mode ?: SessionMode.Code
                val workspaceId = oldBinding?.workspaceId
                    ?.takeIf { boundProject?.record?.id == ownedProject.record.id && oldBinding.sourceDirectory == directory }
                    ?: java.util.UUID.randomUUID().toString()
                queueCurrentDraft()?.join()
                val authoringScope = ConversationScope.Project(project.record.id)
                check(switchEngineScope(authoringScope, project,
                    target?.let { SessionRef(it.sessionId, it.title) }, target == null,
                    newSessionDirectory = directory.takeIf { target == null }, sessionModeOverride = mode)) { "无法切换到应用源码会话。" }
                setConversationMode(mode)
                val attemptedAuthoringSource = chatViewModel.engineSource.value
                var confirmed = try {
                    chatViewModel.awaitAppAuthoringSession(directory, target?.sessionId)
                } catch (resumeFailure: Exception) {
                    if (resumeFailure is kotlinx.coroutines.CancellationException) throw resumeFailure
                    if (target == null) throw resumeFailure
                    // The stored catalogue can lag an engine-side cwd change.
                    // A failed real-cwd check replaces that session in the same
                    // App-owned Project; it never falls back to a global project.
                    check(chatViewModel.engineSource.value === attemptedAuthoringSource &&
                        chatViewModel.sourceScope.value == authoringScope) { "开发会话已切换，请重新打开应用源码。" }
                    check(switchEngineScope(authoringScope, project, null, true, newSessionDirectory = directory,
                        sessionModeOverride = SessionMode.Code)) { "无法恢复应用源码会话。" }
                    setConversationMode(SessionMode.Code)
                    chatViewModel.awaitAppAuthoringSession(directory)
                }
                val authoringSource = chatViewModel.engineSource.value
                check(chatViewModel.engineSource.value === authoringSource &&
                    chatViewModel.sourceScope.value == ConversationScope.Project(project.record.id)) { "开发会话已切换，请重新打开应用源码。" }
                // Empty confirmed sessions also need the existing catalog
                // entry so reopening the app can reuse this recovery UUID.
                projectStore.recordStartedSession(project.record.id, confirmed.sessionId, name, mode)
                projectStore.recordWorkspace(project.record.id, confirmed)
                check(chatViewModel.engineSource.value === authoringSource &&
                    authoringSource.workspaceState.value == confirmed &&
                    canonicalSessionId(chatViewModel.state.value.session.id) == canonicalSessionId(confirmed.sessionId)) { "开发目录已变化，没有插入草稿。" }
                val authoring = org.json.JSONObject()
                    .put("workspaceId", workspaceId)
                    .put("sourceDirectory", directory)
                    .put("projectRef", AppProjectRef("mobileProject", projectId = project.record.id).toJson())
                    .put("authoringSessionId", canonicalSessionId(confirmed.sessionId))
                if (appId != null) {
                    localApps.request("bindAuthoring", appId) {
                        put("authoring", org.json.JSONObject(authoring.toString()).put("appId", appId))
                    }
                } else {
                    val saved = localApps.registerAuthoring(authoring)
                    check(saved.appId == null && saved.sourceDirectory == directory &&
                        saved.workspaceId == workspaceId && saved.projectRef == AppProjectRef("mobileProject", projectId = project.record.id) &&
                        saved.authoringSessionId?.let(::canonicalSessionId) == canonicalSessionId(confirmed.sessionId)) {
                        "应用源码会话绑定没有保存，未插入草稿。"
                    }
                }
                check(chatViewModel.engineSource.value === authoringSource &&
                    authoringSource.workspaceState.value == confirmed &&
                    canonicalSessionId(chatViewModel.state.value.session.id) == canonicalSessionId(confirmed.sessionId)) {
                    "开发目录已变化，没有插入草稿。"
                }
                if (appId == null) {
                    val prompt = "请在这个源码项目中创建应用：$name。使用 local-app skill 和 LocalApp schema 2，完成后发布到本机应用库。"
                    replaceOwnedDraft(authoringScope, confirmed.sessionId, prompt)
                }
                appsPageOpen = false; marketAppId = null; runtimeAppId = null
                closeDrawer(); closeProjectPages()
            } catch (error: Exception) { chatViewModel.reportHostError(error.message ?: "无法打开应用源码会话。") }
        }
    }

    Box(modifier = modifier.fillMaxSize()) {
        AdaptiveConversationDrawer(
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
                            val row = globalDrawerSessions.rows.firstOrNull { it.uuid == ref.id }
                            val targetMode = row?.mode ?: activeSessionMode
                            val targetScope = ConversationScope.Global
                            val switched = if (sourceMatches(targetScope)) {
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
                                        setConversationMode(targetMode)
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
                        engineSessions = drawerEngineSessions,
                        onResumeSession = { uuid ->
                            globalDrawerSessions.rows.firstOrNull { it.uuid == uuid }?.let { row ->
                                val targetScope = if (scheduledSessionRows.any { it.uuid == uuid }) ConversationScope.Scheduled else ConversationScope.Global
                                // Resume directly only when the live engine IS
                                // the global scope — a Project
                                // scope must rebind first, or the session would
                                // resume against the wrong cwd.
                                if (sourceMatches(targetScope)) {
                                    drawerUi.selectSession(uuid)
                                    chatViewModel.resumeSession(row)
                                    closeDrawer()
                                } else {
                                    scope.launch {
                                        if (
                                            switchEngineScope(
                                                project = null,
                                                target = SessionRef(row.uuid, row.title),
                                                newSession = false,
                                                engineScope = targetScope,
                                                resumeEmpty = row.messageCount == 0,
                                                replacePendingTransition = true,
                                                sessionModeOverride = row.mode,
                                            )
                                        ) {
                                            // The recents list mixes modes: the
                                            // opened session decides the mode.
                                            setConversationMode(row.mode)
                                            closeDrawer()
                                        }
                                    }
                                }
                            }
                        },
                        onContinueSession = { sessionScope, row, targetMode ->
                            val targetScope = if (sessionScope == ConversationScope.Global && scheduledSessionRows.any { it.uuid == row.uuid }) {
                                ConversationScope.Scheduled
                            } else sessionScope
                            scope.launch { continueSessionInMode(targetScope, row, targetMode) }
                        },
                        onToggleSessionPinned = { sessionId ->
                            drawerUi.toggleSessionPinned(sessionId)
                            scope.launch {
                                scopeStore.persistSessionPinned(sessionId, drawerUi.sessionPinnedAt(sessionId))
                            }
                        },
                        productionData = drawerProductionData,
                        onSelectProjectSession = ::openProjectSession,
                        onOpenCron = { taskKey ->
                            markCronResultsSeen()
                            closeDrawer()
                            onOpenCronSettings(taskKey)
                        },
                        onOpenCronList = {
                            markCronResultsSeen()
                            closeDrawer()
                            onOpenCronSettings(null)
                        },
                        onOpenProjects = {
                            closeDrawer()
                            projectsPageOpen = true
                        },
                        onOpenProject = { projectId ->
                            closeDrawer()
                            projectPageId = projectId
                        },
                        runningSessionId = state.session.id.takeIf { it != "new" && state.isStreaming },
                        awaitingSessionId = state.session.id.takeIf {
                            it != "new" && (pendingPermission != null || state.pendingQuestions.isNotEmpty())
                        },
                        newCronResults = newCronResults,
                        account = drawerAccount,
                        apps = drawerApps,
                        onOpenApps = {
                            closeDrawer()
                            appsPageOpen = true
                        },
                        onOpenApp = { appId ->
                            closeDrawer()
                            runtimeAppId = appId
                        },
                        onOpenRemoteControl = {
                            closeDrawer()
                            onOpenRemoteControl()
                        },
                    )
                }
            },
        ) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(com.lingxi.code.theme.LingXiTheme.palette.windowBg)
                    .windowInsetsPadding(WindowInsets.systemBars)
                    .imePadding(),
            ) {
                val requestedMessageUuid = linkedMessageUuid.takeIf { linkedSessionId == state.session.id.removePrefix("sess:") }
                val messageRequest = linkedMessageRequest
                ChatScreen(
                    onOpenFiles = { filesBrowserOpen = true },
                    linkedDetailKey = linkedDetailKey,
                    onLinkedDetailHandled = { linkedDetailKey = null },
                    linkedMessageUuid = requestedMessageUuid,
                    onMessageLinkHandled = messageHandled@ { found ->
                        if (requestedMessageUuid == null || linkedMessageUuid != requestedMessageUuid || messageRequest != linkedMessageRequest || linkedSessionId != latestSessionId || linkedScopeModeKey != latestScopeModeKey || linkedMessageSource !== chatViewModel.engineSource.value) return@messageHandled
                        if (!found) {
                            val message = resources.getString(R.string.deep_link_message_missing)
                            linkedTargetError = message
                            onDeepLinkBlockingChanged(true)
                            chatViewModel.reportHostError(message)
                        }
                        linkedMessageUuid = null
                        linkedMessageSource = null
                    },
                    onSuggestionContext = chatViewModel::updatePromptSuggestionContext,
                    onSuggestionFeedback = chatViewModel::promptSuggestionFeedback,
                    pendingPermission = pendingPermission != null,
                    effectivePermissionMode = settingsState.effectivePermissionMode,
                    onOpenPermissionSettings = onOpenPermissionSettings,
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
                    onSendPreflight = { text, attachment -> chatViewModel.admitComposerSend(text, attachment != null) },
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
                        runConversationAction {
                            // A scheduled-task run is not a place to start chatting
                            // (see `newChatScope`): the new chat goes to Global, so
                            // it shows up in the recents and matches the workspace
                            // the new-chat bar names.
                            if (sourceScope == ConversationScope.Scheduled) {
                                startNewSessionIn(newChatScope)
                            } else {
                                startCurrentScopeSession(lastSelectedMode)
                            }
                        }
                    },
                    onSelectModel = chatViewModel::selectModel,
                    onSelectReasoning = chatViewModel::selectReasoning,
                    isDark = isDark,
                    sessionMode = activeSessionMode,
                    workspaceName = currentWorkspaceName,
                    onOpenSessionPanel = { sessionPanelOpen = true },
                    onOpenSideChats = if (chatViewModel.supportsSideChats) chatViewModel::openSideChats else null,
                    onSelectNewChatMode = { mode ->
                        selectSessionMode(mode)
                    },
                    modeSelectionEnabled = currentEngineSource.supportsSessionModeChanges &&
                        (state.sessionReady || state.awaitingNewSessionMode) && !state.sessionTransitioning,
                    onPickWorkspace = { workspacePickerOpen = true },
                    onOpenDrawer = { scope.launch { drawerState.open() } },
                    // Ordinary mic and Flow Mode are separate controls.
                    onMicClick = {
                        if (!voiceActive && (!settingsState.voiceCapability.recognitionSupported ||
                            settingsState.voiceCapability.recognitionReadiness == com.lingxi.code.voice.audio.AudioReadiness.UNAVAILABLE)) {
                            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.Configuration
                            return@ChatScreen
                        }
                        if (!voiceActive) voiceSpeechPlayer.stop()
                        voiceActive = !voiceActive
                        if (voiceActive) onVoiceHoldStart() else onVoiceHoldRelease()
                    },
                    onMicHoldStart = {
                        if (!settingsState.voiceCapability.recognitionSupported ||
                            settingsState.voiceCapability.recognitionReadiness == com.lingxi.code.voice.audio.AudioReadiness.UNAVAILABLE) {
                            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.Configuration
                            return@ChatScreen
                        }
                        voiceSpeechPlayer.stop()
                        voiceActive = true
                        onVoiceHoldStart()
                    },
                    onMicHoldRelease = {
                        voiceActive = false
                        onVoiceHoldRelease()
                    },
                    onFlowModeClick = {
                        val realtime = settingsState.voice.conversation.mode == "realtime"
                        if (!flowActive && ((realtime && settingsState.voiceCapability.providerRealtime?.readiness != "ready") ||
                            (!realtime && (settingsState.voiceCapability.recognitionReadiness == com.lingxi.code.voice.audio.AudioReadiness.UNAVAILABLE ||
                            settingsState.voiceCapability.speechReadiness == com.lingxi.code.voice.audio.AudioReadiness.UNAVAILABLE ||
                            !settingsState.voiceCapability.recognitionSupported || !settingsState.voiceCapability.speechSupported)))) {
                            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.Configuration
                            return@ChatScreen
                        }
                        audioGuidance = null
                        flowActive = !flowActive
                        if (flowActive) voiceSpeechPlayer.stop()
                    },
                    flowModeActive = flowActive,
                    // Unavailable actions stay reachable to explain how to configure them.
                    voiceInputSupported = true,
                    voiceConversationSupported = true,
                    audioGuidance = audioGuidance,
                    onOpenVoiceSettings = onOpenVoiceSettings,
                    onDismissAudioGuidance = { audioGuidance = null },
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
                            controller = flowVoiceController,
                            onClose = { flowActive = false },
                        )
                    },
                    draft = draft,
                    onDraftChange = ::updateCurrentDraft,
                    onCameraClick = onCameraClick,
                    attachment = attachment,
                    onRemoveAttachment = { attachment = null },
                    onShare = onShare,
                    onStop = chatViewModel::cancel,
                    onDiscardRecoveredTurn = chatViewModel::discardRecoveredTurn,
                    onDismissError = chatViewModel::dismissError,
                    onDismissCompactRejection = chatViewModel::dismissCompactRejection,
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
                    visualizationHost = visualizationHost,
                    localAppCardLinks = localAppCardLinks,
                    onVisualizationFollowup = chatViewModel::offerVisualizationFollowup,
                    onAcceptVisualizationFollowup = chatViewModel::acceptVisualizationFollowup,
                    onRemoveVisualizationChip = chatViewModel::clearVisualizationChip,
                    // Tool-call expansion and the plan panel keep their state in
                    // the ViewModel, not in the recycled rows that render them.
                    onToggleToolCall = chatViewModel::toggleToolCall,
                    onTogglePlan = chatViewModel::togglePlanExpanded,
                )
            }
        }

        // Immersive voice overlay (full-screen, above everything else).
        VoiceFlowOverlay(visible = voiceActive)

        com.lingxi.code.conversation.SideChatSheet(chatViewModel.sideChats, isDark, onCreate = chatViewModel::createSideChat)

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

        // The list stays underneath an opened project, so a project page slides
        // over it and back reveals it, like any pushed page.
        AnimatedVisibility(
            visible = projectsPageOpen,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            BackHandler { projectsPageOpen = false }
            ProjectsPage(
                projects = drawerProductionData.projects.orEmpty(),
                pinnedWorkspaces = drawerUi.pinnedWorkspaces,
                statusMessage = drawerProductionData.projectStatusMessage,
                onBack = { projectsPageOpen = false },
                onOpenProject = { projectPageId = it },
                onTogglePinned = { projectId ->
                    val key = "project.$projectId"
                    drawerUi.toggleWorkspacePinned(activeSessionMode, key)
                    scope.launch { scopeStore.persistWorkspacePinned(key, drawerUi.pinnedWorkspaces[key]) }
                },
                onCreateProject = { showCreateProject = true },
            )
        }
        val openedProject = projectPageId?.let { id ->
            drawerProductionData.projects.orEmpty().firstOrNull { it.id == id }
        }
        // `openedProject` is already null while the page slides out; the page
        // keeps drawing the last project until the exit animation is done.
        // Read only inside the page, so updating it never recomposes this screen.
        var lastOpenedProject by remember { mutableStateOf<Project?>(null) }
        LaunchedEffect(openedProject) { if (openedProject != null) lastOpenedProject = openedProject }
        AnimatedVisibility(
            visible = openedProject != null,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            val shown = openedProject ?: lastOpenedProject
            if (shown != null) {
                BackHandler { projectPageId = null }
                ProjectPage(
                    project = shown,
                    crons = drawerProductionData.crons.orEmpty().filter { it.id.startsWith("${shown.id}:") },
                    activeSession = drawerUi.activeSession,
                    onBack = { projectPageId = null },
                    onNewSession = { mode ->
                        if (mode != null) {
                            rememberModeSelection(mode)
                        }
                        startProjectSession(shown.id, mode ?: lastSelectedMode)
                    },
                    onOpenSession = { ref -> openProjectSession(shown.id, ref) },
                    onOpenCron = { taskKey ->
                        markCronResultsSeen()
                        closeProjectPages()
                        onOpenCronSettings(taskKey)
                    },
                    onReimport = { runProjectSync(shown.id) { projectStore.reimport(shown.id) } },
                    onExport = { runProjectSync(shown.id) { projectStore.export(shown.id) } },
                    onReauthorize = {
                        runProjectSync(shown.id) {
                            pendingReauthorizeProjectId = shown.id
                            importProjectLauncher.launch(null)
                        }
                    },
                )
            }
        }

        AnimatedVisibility(
            visible = appsPageOpen,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            BackHandler { appsPageOpen = false }
            UnifiedAppsPage(
                apps = localApps,
                onBack = { appsPageOpen = false },
                onOpen = { runtimeAppId = it },
                onDetails = { marketAppId = it },
                onAuthoring = ::openAppAuthoring,
            )
        }
        // Like the project page, these keep their last app while sliding out.
        var lastMarketAppId by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(marketAppId) { if (marketAppId != null) lastMarketAppId = marketAppId }
        AnimatedVisibility(
            visible = marketAppId != null,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            val shownId = marketAppId ?: lastMarketAppId
            if (shownId != null) {
                BackHandler { marketAppId = null }
                UnifiedAppDetailsPage(
                    apps = localApps,
                    appId = shownId,
                    onBack = { marketAppId = null },
                    onOpen = { runtimeAppId = it },
                    onAuthoring = ::openAppAuthoring,
                )
            }
        }
        var lastRuntimeAppId by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(runtimeAppId) { if (runtimeAppId != null) lastRuntimeAppId = runtimeAppId }
        AnimatedVisibility(
            visible = runtimeAppId != null,
            enter = slideInVertically(initialOffsetY = { it }),
            exit = slideOutVertically(targetOffsetY = { it }),
        ) {
            val shownId = runtimeAppId ?: lastRuntimeAppId
            if (shownId != null) {
                // Its own back handler goes back inside the app first.
                UnifiedAppRuntimePage(
                    apps = localApps,
                    appId = shownId,
                    onClose = { runtimeAppId = null },
                    onShowDetails = { appId ->
                        runtimeAppId = null
                        marketAppId = appId
                    },
                )
            }
        }
        if (sessionPanelOpen) {
            val panelRow = SessionRow(
                uuid = state.session.id,
                title = state.session.title,
                messageCount = state.messages.size,
                relativeTime = "",
                mode = activeSessionMode,
            )
            val otherMode = when (activeSessionMode) {
                SessionMode.Chat -> SessionMode.Code
                SessionMode.Code -> SessionMode.Chat
            }
            SessionPanel(
                model = SessionPanelModel(
                    title = state.session.title,
                    mode = activeSessionMode,
                    pendingMode = state.pendingSessionMode,
                    canSwitchMode = currentEngineSource.supportsSessionModeChanges && (state.sessionReady || state.awaitingNewSessionMode) && !state.sessionTransitioning,
                    workspaceName = currentWorkspaceName,
                    modelName = state.model.name,
                    effortLabel = state.reasoning?.let { effortLabel(it.selection) },
                    permissionLabel = permissionModeLabel(settingsState.effectivePermissionMode),
                    pinned = drawerUi.sessionPinnedAt(state.session.id) != null,
                    canArchive = sourceScope != ConversationScope.Scheduled,
                ),
                onDismiss = { sessionPanelOpen = false },
                onSelectMode = ::selectSessionMode,
                // Fork remains an explicit, separate action.
                onContinueInOtherMode = {
                    sessionPanelOpen = false
                    scope.launch { continueSessionInMode(sourceScope, panelRow, otherMode) }
                },
                onOpenPermissionSettings = {
                    sessionPanelOpen = false
                    onOpenPermissionSettings()
                },
                onTogglePinned = {
                    sessionPanelOpen = false
                    val id = state.session.id
                    drawerUi.toggleSessionPinned(id)
                    scope.launch { scopeStore.persistSessionPinned(id, drawerUi.sessionPinnedAt(id)) }
                },
                onMakeScheduled = {
                    sessionPanelOpen = false
                    onOpenCronCreate(
                        state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User }
                            ?.text.orEmpty().take(500),
                    )
                },
                onOpenTerminal = {
                    sessionPanelOpen = false
                    onOpenTerminal("interactive", null)
                },
                onArchive = {
                    sessionPanelOpen = false
                    val id = state.session.id
                    val projectId = (sourceScope as? ConversationScope.Project)?.projectId
                    scope.launch {
                        runCatching { projectStore.setSessionArchived(projectId, id, true) }
                            .onSuccess { startCurrentScopeSession(lastSelectedMode) }
                            .onFailure {
                                chatViewModel.reportHostError(
                                    resources.getString(R.string.session_index_save_failed_fmt, it.message.orEmpty()),
                                )
                            }
                    }
                },
            )
        }
        if (filesBrowserOpen) {
            val filePaths = remember { com.lingxi.code.project.MobileUserStorage.directories(context) }
            val filesRepository = remember { com.lingxi.code.project.WorkspaceDirectoryRepository(java.io.File(filePaths.home),
                filePaths.guestHome, filePaths.defaultGuestCwd, filePaths.projectsGuestPath, java.io.File(filePaths.configHome)) }
            val browserSource = remember { currentEngineSource }
            val browserSession = remember { liveWorkspace?.sessionId }
            WorkspaceFilesDialog(filesRepository, liveWorkspace?.cwd ?: filePaths.defaultGuestCwd,
                protectedPaths = {
                    projectState.projects.flatMap { project -> listOf(project.record.directory, project.record.activeDirectory,
                        project.record.directory.substringBeforeLast('/')) + project.record.additionalDirectories + project.sessions.mapNotNull { it.workspaceDirectory } } +
                        listOfNotNull(currentEngineSource.workspaceState.value?.cwd, currentEngineSource.workspaceState.value?.projectRoot) +
                        currentEngineSource.workspaceState.value?.additionalDirectories.orEmpty() + sessionState.rows.mapNotNull { it.workspaceDirectory }
                }, canMutate = { !chatViewModel.state.value.requiresBackgroundExecution && !chatViewModel.state.value.sessionTransitioning &&
                    !chatViewModel.state.value.cancellationInFlight && chatViewModel.engineSource.value === browserSource &&
                    browserSource.workspaceState.value?.sessionId == browserSession },
                onDismiss = { filesBrowserOpen = false })
        }
        if (workspacePickerOpen) {
            val pickerSource = remember { currentEngineSource }
            val pickerSessionId = remember { pickerSource.workspaceState.value?.sessionId.orEmpty() }
            WorkspaceDirectorySheet(
                projects = drawerProductionData.projects.orEmpty(),
                currentScope = newChatScope,
                onDismiss = { workspacePickerOpen = false },
                currentDirectory = liveWorkspace?.cwd,
                runtimeStatus = stringResource(com.lingxi.code.settings.linuxRuntimeStateLabel(settingsState.linuxRuntime.rootfsState)),
                directories = liveWorkspace?.additionalDirectories.orEmpty(),
                protectedPaths = {
                    projectState.projects.flatMap { project ->
                        listOf(project.record.directory, project.record.activeDirectory,
                            project.record.directory.substringBeforeLast('/')) +
                            project.record.additionalDirectories + project.sessions.mapNotNull { it.workspaceDirectory }
                    } + listOfNotNull(pickerSource.workspaceState.value?.cwd, pickerSource.workspaceState.value?.projectRoot) +
                        pickerSource.workspaceState.value?.additionalDirectories.orEmpty() +
                        sessionState.rows.mapNotNull { it.workspaceDirectory }
                },
                canMutate = { !chatViewModel.state.value.requiresBackgroundExecution && !chatViewModel.state.value.sessionTransitioning && !chatViewModel.state.value.cancellationInFlight &&
                    chatViewModel.engineSource.value === pickerSource &&
                    pickerSource.workspaceState.value?.sessionId.orEmpty() == pickerSessionId },
                onPickDirectory = { path ->
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                    pickerSource.changeWorkspace(path, pickerSessionId)
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                },
                onAddDirectory = { path ->
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                    pickerSource.addWorkspaceDirectory(path, pickerSessionId)
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                },
                onPick = { target ->
                    workspacePickerOpen = false
                    if (target != newChatScope) startNewSessionIn(target, lastSelectedMode, carryDraft = true)
                },
            )
        }
        CreateProjectDialog(
            visible = showCreateProject,
            onDismiss = { showCreateProject = false },
            onCreateInternal = { name ->
                showCreateProject = false
                scope.launch {
                    runCatching { projectStore.createInternal(name) }
                        .onSuccess { created ->
                            if (switchEngineScope(created, null, true)) {
                                closeDrawer()
                                closeProjectPages()
                            }
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
    }
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
    CronRunStatus.Interrupted -> "Interrupted"
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
        RootScreen(isDark = true)
    }
}
