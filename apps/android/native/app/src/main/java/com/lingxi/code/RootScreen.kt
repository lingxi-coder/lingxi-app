package com.lingxi.code

import com.lingxi.code.conversation.blocksEngineReconnect
import com.lingxi.code.apps.LocalAppAuthoringPrompt

import android.util.Base64
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.Snackbar
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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
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
import com.lingxi.code.conversation.SessionHost
import com.lingxi.code.conversation.sourceText
import com.lingxi.code.conversation.bindAudioOwner
import com.lingxi.code.conversation.ConversationLaunchRequest
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.conversation.ComputerUseSetupStatus
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.toImageRef
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.conversation.verifiedAuthoringSource
import com.lingxi.code.conversation.pendingAuthoringSourceIdentity
import com.lingxi.code.conversation.reserveAuthoringSelection
import com.lingxi.code.conversation.authoritativeAuthoringWorkspace
import com.lingxi.code.conversation.releaseAuthoringSelection
import com.lingxi.code.conversation.reserveValidatedAuthoringSelection
import com.lingxi.code.conversation.conversationStrings
import com.lingxi.code.conversation.PermissionPromptDialog
import com.lingxi.code.computeruse.ComputerUseApprovalDialog
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseUiState
import com.lingxi.code.computeruse.resolveBrowserPackages
import com.lingxi.code.connectivity.rememberOnlineState
import com.lingxi.code.connectivity.shouldShowOfflineBanner
import com.lingxi.code.cron.AndroidCronRepository
import com.lingxi.code.cron.CronPage
import com.lingxi.code.cron.CronPageRoute
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.OnTabReselected
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
import com.lingxi.code.apps.matchesOwner
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
import com.lingxi.code.project.globalGuestCwd
import com.lingxi.code.project.projectGrantRoots
import com.lingxi.code.project.workspaceDirectoryRepository
import com.lingxi.code.project.workspaceOwnerStoreMatches
import com.lingxi.code.project.toDrawerProject
import com.lingxi.code.settings.ProviderSettingsRepository
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

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
private data class PendingHandoffRequest(
    val scope: ConversationScope,
    val sourceMode: SessionMode,
    val targetMode: SessionMode,
    val sourceSessionId: String,
    val sourceSessionTitle: String,
)

internal fun isBoundToScope(
    boundScope: ConversationScope,
    targetScope: ConversationScope,
): Boolean = boundScope == targetScope

internal fun isRestorableSession(
    expectedId: String,
    candidateId: String,
): Boolean = canonicalSessionId(expectedId) == canonicalSessionId(candidateId)

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
    engineMode: SessionMode?,
    activeMode: SessionMode,
    activeKey: String,
    restoredKey: String?,
): Boolean = ready && sessionId != "new" && engineMode == activeMode && restoredKey == activeKey

internal fun sessionLinkModeMatchesOwner(
    requested: SessionMode?, indexed: SessionMode?, current: SessionMode?, isCurrent: Boolean,
): Boolean = requested != null && (indexed == null || indexed == requested) && (!isCurrent || current == requested)

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
 * the [ChatViewModel] to that session; the account row invokes [onOpenAccount]
 * (Settings is a tab of the shell). Drawer + conversation read
 * the same hoisted state, and the system back gesture closes an open drawer
 * (handled by [ModalNavigationDrawer]).
 *
 * This is the Chats tab of the app shell: it is always composed (it owns the live engine), reports
 * its pushed pages, drawer and full-screen overlays through [onChromeChanged] so the shell can
 * decide whether its tab bar shows, and asks the shell for every other tab through the callbacks.
 *
 * @param onOpenSettings opens the Settings tab (the shell remembers Chats as the way back).
 */
@Composable
@OptIn(ExperimentalCoroutinesApi::class)
fun RootScreen(
    isDark: Boolean,
    tabState: com.lingxi.code.tabs.TabShellState? = null,
    modifier: Modifier = Modifier,
    onOpenSettings: () -> Unit = {},
    /** The drawer's account row: Settings / Account, with Back returning to Chats. */
    onOpenAccount: () -> Unit = { onOpenSettings() },
    onOpenModelSettings: () -> Unit = {},
    onOpenPermissionSettings: () -> Unit = { onOpenSettings() },
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    onOpenComputerUseSettings: () -> Unit = { onOpenSettings() },
    onOpenVoiceSettings: () -> Unit = { onOpenSettings() },
    onOpenTerminal: (sessionId: String, initCommand: String?, cwd: String, ownerKey: String) -> Unit = { _, _, _, _ -> },
    /** Select the Bots tab (the Apps page's "browse apps shared by a computer"). */
    onOpenBotsTab: () -> Unit = {},
    /** A scheduled run's session was chosen on the scheduled-task page: the host opens it in the chat. */
    onOpenScheduledSession: (com.lingxi.code.cron.CronRunRecord) -> Unit = {},
    /**
     * What the tab shell needs for its bar rule: how many pages are pushed over the chat, whether the
     * phone drawer is open or opening, and whether a full-screen overlay (a running app, the voice flow) covers it.
     */
    onChromeChanged: (pushedPages: Int, drawerOpen: Boolean, fullScreenOverlay: Boolean) -> Unit = { _, _, _ -> },
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
    onDeepLinkNativeReady: (com.lingxi.code.navigation.DeepLink, com.lingxi.code.conversation.ConversationSource) -> Unit = { _, _ -> },
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
    val projectStore: ProjectStore = viewModel(
        key = "projects",
        factory = ProjectStore.factory(context),
    )
    val projectState by projectStore.state.collectAsState()
    val appContext = context.applicationContext
    val scopeStore = remember(appContext) { ScopeStateStore(appContext) }
    var scopeStateValid by remember(scopeStore) { mutableStateOf<Boolean?>(null) }
    var scopeStateValidationRetry by remember(scopeStore) { mutableIntStateOf(0) }
    val sessionHost = remember(appContext, projectStore, scopeStore) {
        SessionHost.get(appContext, reconnectToken) {
            scopeStateValid == true && !projectStore.state.value.loading && !projectStore.state.value.storageUnavailable
        }
    }
    val selectedOwner by sessionHost.selected.collectAsState()
    val chatViewModel: ChatViewModel = viewModel ?: selectedOwner
    fun reportScopeStateFailure(failure: Exception) {
        scopeStateValid = false
        projectStore.reportOwnerStateFailure(failure)
    }
    suspend fun validateOwnerIndexes(): Boolean {
        if (scopeStateValid != true) return false
        try {
            scopeStore.validate()
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            reportScopeStateFailure(failure)
            return false
        }
        return try {
            projectStore.validateOwnerIndexes()
            true
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            false
        }
    }
    LaunchedEffect(scopeStore, projectState.storageUnavailable, scopeStateValidationRetry) {
        scopeStateValid = null
        val ownerStateValid = try {
            scopeStore.validate()
            true
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            reportScopeStateFailure(failure)
            false
        }
        scopeStateValid = if (!ownerStateValid) false else try {
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                com.lingxi.code.conversation.ConversationServiceOwners.initialize(appContext)
            }
            true
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            projectStore.reportOwnerStateFailure(failure, "conversation-recovery")
            false
        }
    }
    if (com.lingxi.code.conversation.shouldBlockForUnavailableStorage(
            projectState.loading || projectState.storageUnavailable, sessionHost.hasRegisteredOwner(chatViewModel))) {
        ConversationStorageUnavailable(context, modifier, projectState.loading, projectState.unavailableScopes, projectStore::reload)
        return
    }
    val cronRepository = remember(context) { AndroidCronRepository.get(context) }
    val cronState by cronRepository.state.collectAsState()
    val cronAdmissionRevision by com.lingxi.code.conversation.SessionAdmissionGate.revision.collectAsState()
    LaunchedEffect(cronAdmissionRevision) {
        if (com.lingxi.code.conversation.SessionAdmissionGate.hasBarrierFailure()) {
            projectStore.reportOwnerStateFailure(IllegalStateException("Scheduled owner admission could not be verified"), "scheduled")
            return@LaunchedEffect
        }
        if (com.lingxi.code.conversation.SessionAdmissionGate.needsOwnerScan()) {
            runCatching { projectStore.validateOwnerIndexes() }
                .onFailure { if (it is kotlinx.coroutines.CancellationException) throw it }
        }
    }
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
    val lifecycleOwner = LocalLifecycleOwner.current
    var activeSessionMode by rememberSaveable { mutableStateOf(chatViewModel.state.value.sessionMode ?: SessionMode.Code) }
    // Only a deliberately started session may inherit the temporary new-chat draft.
    var newDraftScope by remember { mutableStateOf<ConversationScope?>(null) }
    if (com.lingxi.code.conversation.shouldBlockForOwnerStateValidation(
            scopeStateValid, sessionHost.hasRegisteredOwner(chatViewModel))) {
        ConversationStorageUnavailable(context, modifier, scopeStateValid == null, emptyList()) {
            scopeStateValidationRetry++
            projectStore.reload()
        }
        return
    }
    var restoredModeSessionKey by rememberSaveable { mutableStateOf<String?>(null) }
    var drawerBootstrapComplete by remember { mutableStateOf(false) }
    var voiceCaptureOwner by remember { mutableStateOf<ChatViewModel?>(null) }
    val voiceSpeechPlayer = remember(chatViewModel) {
        if (viewModel == null) sessionHost.speechPlayer(chatViewModel) else VoiceSpeechPlayer(appContext)
    }
    DisposableEffect(voiceSpeechPlayer, viewModel) {
        onDispose { if (viewModel != null) voiceSpeechPlayer.dispose() }
    }
    val navigationSequence = remember { java.util.concurrent.atomic.AtomicLong() }
    fun nextNavigationIntent(): Long = com.lingxi.code.conversation.nextNavigationIntent(navigationSequence)
    val appPageSequence = remember { java.util.concurrent.atomic.AtomicLong() }
    val rootCompositionAlive = remember { java.util.concurrent.atomic.AtomicBoolean(true) }
    DisposableEffect(rootCompositionAlive) {
        onDispose { rootCompositionAlive.set(false) }
    }
    var pendingModeNavigation by remember { mutableStateOf<Pair<SessionMode, Long>?>(null) }
    val navigationMutex = remember { Mutex() }
    LaunchedEffect(chatViewModel, reconnectToken, drawerBootstrapComplete, projectState.loading, projectState.activeProjectId, projectState.storageUnavailable, scopeStateValid) {
        if (!drawerBootstrapComplete || projectState.loading || projectState.storageUnavailable || scopeStateValid != true) return@LaunchedEffect
        if (!validateOwnerIndexes()) return@LaunchedEffect
        try {
            if (viewModel == null) {
                    val initialBinding = chatViewModel.engineSource.value is com.lingxi.code.conversation.ConnectingConversationSource &&
                        sessionHost.needsBootstrap(chatViewModel)
                    val launchScope = if (initialBinding) {
                        val restored = conversationScopeFromKey(scopeStore.readActiveScopeKey(activeSessionMode))
                        restored?.takeIf { it !is ConversationScope.Project || projectState.projects.any { p -> p.record.id == it.projectId } }
                            ?: ConversationScope.Global
                    } else chatViewModel.sourceScope.value
                    val launchWorkspaceKey = launchScope.persistenceKey()
                    val launchSessionId = if (initialBinding) {
                        val slotId = scopeStore.read(launchScope.sessionStateKey(activeSessionMode))?.lastActiveSessionId
                            ?.takeUnless { it.isBlank() || it == "new" }
                        newDraftScope = launchScope.takeIf { slotId == null }
                        // This startup target has already been selected. Waiting for
                        // its confirmation must not reopen an older mode-slot owner.
                        restoredModeSessionKey = launchScope.sessionStateKey(activeSessionMode)
                        slotId
                    } else chatViewModel.state.value.session.id.takeUnless { it == "new" }
                    if (!initialBinding && launchSessionId == null) newDraftScope = launchScope
                    val ownerMode = try {
                        com.lingxi.code.conversation.DurableConversationTurnClientStore.ownerModeForSession(
                            appContext, launchWorkspaceKey, launchSessionId,
                        ) ?: activeSessionMode
                    } catch (failure: IllegalStateException) {
                        chatViewModel.reportConversationLaunchFailed()
                        return@LaunchedEffect
                    }
                    if (launchSessionId != null && !scopeStore.matchesSessionOwner(launchSessionId, launchWorkspaceKey, ownerMode)) {
                        chatViewModel.reportConversationLaunchFailed()
                        return@LaunchedEffect
                    }
                    val launchWorkspace = when (launchScope) {
                        ConversationScope.Global -> null
                        ConversationScope.Scheduled -> com.lingxi.code.project.ProjectWorkspace(
                            projectId = "b51bca68-b85f-4caa-8881-07dd33eba24d",
                            hostPath = com.lingxi.code.cron.CronScope.global(appContext).workspacePath,
                            guestPath = com.lingxi.code.project.MobileUserStorage.directories(appContext).scheduledGuestCwd,
                        )
                        is ConversationScope.Project -> projectState.projects
                            .firstOrNull { it.record.id == launchScope.projectId }?.workspace
                    }
                    val launchLinuxMode = settingsStore?.state?.value?.linuxRuntime?.selectedMode
                        ?: com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux
                    val launchCronRows = if (launchScope == ConversationScope.Scheduled && launchSessionId != null)
                        cronRepository.state.first { !it.loading }.generatedSessions else cronRepository.state.value.generatedSessions
                    val launchTranscriptPath = launchSessionId?.let { id ->
                        com.lingxi.code.conversation.sessionTranscriptPath(launchScope, id, ownerMode,
                            projectStore.state.value, launchCronRows)
                    }
                    val createSource: suspend () -> ConversationSource = {
                        EngineConversationSource.createInBackground(
                            context = appContext,
                            projectWorkspace = launchWorkspace,
                            workspaceKey = launchWorkspaceKey,
                            sessionMode = ownerMode,
                            linuxRuntimeMode = launchLinuxMode,
                            reuseProcessSource = launchSessionId != null,
                            expectedSessionId = launchSessionId,
                            initialSessionTranscriptPath = launchTranscriptPath,
                        )
                    }
                    if (initialBinding) {
                        sessionHost.bootstrap(launchScope, ownerMode, launchSessionId?.let { SessionRef(it, "") }, reconnectToken, createSource)
                        val confirmed = withTimeoutOrNull(20_000L) {
                            chatViewModel.state.first {
                                chatViewModel.sessionOwnerRejected || (it.sessionReady && it.session.id != "new" && it.sessionMode == ownerMode &&
                                    (launchSessionId == null || canonicalSessionId(it.session.id) == canonicalSessionId(launchSessionId)))
                            }
                        }
                        if (confirmed != null && !chatViewModel.sessionOwnerRejected && validateOwnerIndexes() && !projectStore.state.value.storageUnavailable &&
                            scopeStore.claimSessionOwner(confirmed.session.id, launchWorkspaceKey, ownerMode) { scopeStateValid == true && !projectStore.state.value.storageUnavailable }) {
                            sessionHost.register(chatViewModel, launchScope, ownerMode)
                            if (!scopeStore.persistActiveSelection(launchWorkspaceKey, ownerMode) { scopeStateValid == true && !projectStore.state.value.storageUnavailable }) {
                                chatViewModel.reportConversationLaunchFailed()
                            }
                        } else {
                            chatViewModel.reportConversationLaunchFailed()
                        }
                    } else {
                        chatViewModel.ensureSourceInBackground(reconnectToken, awaitRelease = { sessionId ->
                            com.lingxi.code.bindings.runtime.waitForMobileSessionRelease(
                                lingxiHome = com.lingxi.code.project.MobileUserStorage.directories(appContext).configHome,
                                sessionId = sessionId,
                                timeoutMs = 8000u,
                            )
                        }, awaitMediaDrain = {
                            val status = com.lingxi.code.voice.audio.AndroidNativeMediaHostProvider.get(appContext)
                                .waitForAttachmentDrain(8000u)
                            if (status != com.lingxi.code.bindings.android.AndroidMediaDrainStatus.READY) {
                                android.util.Log.w("RootScreen", "Engine reconnect blocked by media attachment drain: $status")
                            }
                            status == com.lingxi.code.bindings.android.AndroidMediaDrainStatus.READY
                        }, canReplace = {
                            val owner = chatViewModel.engineSource.value.recoverySpec?.ownerKey
                            scopeStateValid == true && !projectStore.state.value.storageUnavailable &&
                                chatViewModel.engineSource.value.turnAdmissionReady &&
                                voiceCaptureOwner !== chatViewModel && !sessionHost.ownerHasVoiceWork(chatViewModel) &&
                                (owner == null || owner !in ComputerUseFeatureProvider.activeEngineOwners())
                        }, beginReplacement = { sessionHost.awaitSourceReplacement(chatViewModel) }) {
                            EngineConversationSource.create(
                                context = appContext,
                                projectWorkspace = launchWorkspace,
                                workspaceKey = launchWorkspaceKey,
                                sessionMode = ownerMode,
                                linuxRuntimeMode = launchLinuxMode,
                                replaceProcessSource = true,
                                expectedSessionId = launchSessionId,
                                initialSessionTranscriptPath = launchTranscriptPath,
                            )
                        }
                    }
            }
            onConversationSourceChanged(chatViewModel.engineSource.value)
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            projectStore.reload()
            chatViewModel.reportConversationLaunchFailed()
        }
    }
    LaunchedEffect(chatViewModel) {
        chatViewModel.engineSource.collect { onConversationSourceChanged(it) }
    }
    val catalogLifecycleScope = androidx.compose.runtime.rememberCoroutineScope()
    DisposableEffect(chatViewModel, lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                chatViewModel.refreshExecutionStatus()
                catalogLifecycleScope.launch { runCatching { chatViewModel.engineSource.value.submitClientCommand(ClientCommand.RefreshProviderCatalog("foreground")) } }
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    val state by chatViewModel.state.collectAsState()
    val authoringRetryAvailable by chatViewModel.authoringRetryAvailable.collectAsState()
    val pendingPermission by chatViewModel.pendingPermission.collectAsState()
    val sideChats by chatViewModel.sideChats.chats.collectAsState()
    val hostBusy by sessionHost.busy.collectAsState()
    val hasActiveEngineWork = if (viewModel == null) hostBusy else state.blocksEngineReconnect(hasPendingPermission = pendingPermission != null)
    LaunchedEffect(hasActiveEngineWork) { onConversationBusyChanged(hasActiveEngineWork) }
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    LaunchedEffect(sessionHost, chatViewModel, scopeStateValid, projectState.loading, projectState.storageUnavailable) {
        if (scopeStateValid == true && !projectState.loading && !projectState.storageUnavailable) {
            val owners = if (viewModel == null) sessionHost.allOwners() else listOf(chatViewModel)
            owners.forEach { it.retryConversationDraftWrites(scopeStore, ::reportScopeStateFailure) }
        }
    }

    fun queueScopeStateWrite(write: suspend () -> Unit) {
        scope.launch {
            try {
                write()
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                reportScopeStateFailure(failure)
                chatViewModel.reportHostError(failure.message.orEmpty())
            }
        }
    }

    // The conversation mode follows the session the user opens or starts; the
    // drawer's Chat/Code chips are only a list filter and never decide it.
    fun setConversationMode(mode: SessionMode) {
        activeSessionMode = mode
    }

    fun rememberModeSelection(mode: SessionMode) {
        if (scopeStateValid != true || projectStore.state.value.loading || projectStore.state.value.storageUnavailable) return
        if (viewModel == null) sessionHost.rememberModeSelection(scopeStore, mode)
        else chatViewModel.rememberModeSelection(scopeStore, mode)
    }

    fun selectSessionMode(mode: SessionMode) {
        if (scopeStateValid != true || projectStore.state.value.loading || projectStore.state.value.storageUnavailable) return
        rememberModeSelection(mode)
        val intent = nextNavigationIntent()
        pendingModeNavigation = if (mode != activeSessionMode) mode to intent else null
    }

    fun projectSnapshotForScope(scope: ConversationScope): ProjectSnapshot? = when (scope) {
        ConversationScope.Global, ConversationScope.Scheduled -> null
        is ConversationScope.Project -> projectState.projects.firstOrNull { it.record.id == scope.projectId }
    }

    fun resolveLinkedScope(key: String?): ConversationScope? {
        val scope = conversationScopeFromKey(key) ?: return null
        return scope.takeIf { it !is ConversationScope.Project || projectSnapshotForScope(it) != null }
    }

    fun workspaceForScope(scope: ConversationScope, sessionId: String? = null) = when (scope) {
        ConversationScope.Global -> null
        ConversationScope.Scheduled -> com.lingxi.code.project.MobileUserStorage.scheduledWorkspace(appContext, sessionId).copy(
            projectId = "b51bca68-b85f-4caa-8881-07dd33eba24d",
        )
        is ConversationScope.Project -> projectSnapshotForScope(scope)?.workspace
    }

    DisposableEffect(sessionHost, context, viewModel) {
        ComputerUseFeatureProvider.attach(context) {
            val owners = ComputerUseFeatureProvider.activeEngineOwners()
            if (viewModel == null) sessionHost.cancelComputerUseTurns(owners) else viewModel.cancel()
        }
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
    val rawSessionState by chatViewModel.sessions.collectAsState()
    val sourceProjectId by chatViewModel.sourceProjectId.collectAsState()
    // Which workspace the live engine is bound to (Global / Scheduled / Project)
    // — the generalization of sourceProjectId.
    val sourceScope by chatViewModel.sourceScope.collectAsState()
    var handoffReference by remember(chatViewModel, sourceScope, state.session.id) {
        mutableStateOf<ScopeStateStore.ConversationHandoff?>(null)
    }
    LaunchedEffect(sourceScope, state.session.id, chatViewModel, scopeStateValid, projectState.storageUnavailable) {
        if (scopeStateValid != true || projectState.storageUnavailable) return@LaunchedEffect
        try {
            handoffReference = scopeStore.readConversationHandoff(sourceScope.persistenceKey(), state.session.id)
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            projectStore.reload()
        }
    }
    var catalogOwners by remember(chatViewModel, sourceScope) { mutableStateOf<Map<String, ScopeStateStore.SessionOwner>?>(null) }
    val ownerRevision by scopeStore.ownerChanges.collectAsState()
    LaunchedEffect(rawSessionState.rows, sourceScope, state.session.id, state.sessionReady, scopeStateValid, projectState.storageUnavailable, ownerRevision) {
        if (scopeStateValid != true || projectState.storageUnavailable) return@LaunchedEffect
        try {
            catalogOwners = scopeStore.readSessionOwners()
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            projectStore.reload()
        }
    }
    LaunchedEffect(cronState.generatedSessions, projectState.loading, projectState.storageUnavailable, scopeStateValid, cronAdmissionRevision) {
        if (projectState.loading || projectState.storageUnavailable || scopeStateValid != true) return@LaunchedEffect
        if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwnersGlobally()) return@LaunchedEffect
        try {
            scopeStore.validate()
            projectStore.validateOwnerIndexes()
            val rows = cronState.generatedSessions.filter {
                (it.sessionScopeKey == "scheduled" || it.sessionScopeKey?.startsWith("project.") == true) && !it.sessionId.isNullOrBlank() &&
                    !it.sessionTranscriptPath.isNullOrBlank() &&
                    SessionMode.entries.any { mode -> mode.wireValue == it.sessionMode }
            }
            rows.groupBy { canonicalSessionId(it.sessionId!!) }.forEach { (id, group) ->
                if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner(id)) return@forEach
                val identities = group.map { Triple(it.sessionScopeKey, it.sessionMode, it.sessionTranscriptPath) }.distinct()
                check(identities.size == 1) { "Scheduled session $id has conflicting transcript identities" }
                val run = group.first()
                val mode = SessionMode.entries.single { it.wireValue == run.sessionMode }
                val key = checkNotNull(run.sessionScopeKey)
                val owner = ScopeStateStore.SessionOwner(key, mode)
                if (key.startsWith("project.")) {
                    val projectId = key.removePrefix("project.")
                    val project = projectStore.state.value.projects.firstOrNull { it.record.id == projectId }
                        ?: return@forEach
                    val existing = project.sessions.firstOrNull { canonicalSessionId(it.sessionId) == id }
                    check(existing?.transcriptPath == null || existing.transcriptPath == run.sessionTranscriptPath) {
                        "Project session $id has conflicting transcript identities"
                    }
                    if (existing == null || existing.transcriptPath != run.sessionTranscriptPath) {
                        projectStore.recordStartedSession(projectId, id, run.prompt.take(120), mode,
                            transcriptPath = run.sessionTranscriptPath, markActive = { false })
                    }
                } else {
                    val state = projectStore.state.value
                    val indexed = state.globalSessions.firstOrNull { canonicalSessionId(it.sessionId) == id }
                        ?.let { ScopeStateStore.SessionOwner("global", it.mode) }
                        ?: state.projects.firstNotNullOfOrNull { project ->
                            project.sessions.firstOrNull { canonicalSessionId(it.sessionId) == id }
                                ?.let { ScopeStateStore.SessionOwner("project.${project.record.id}", it.mode) }
                        }
                    if (indexed != null) {
                        projectStore.reportOwnerConflict(id, owner, indexed)
                        return@LaunchedEffect
                    }
                }
                if (!scopeStore.claimSessionOwner(id, owner.scopeKey, owner.mode) {
                        scopeStateValid == true && !projectStore.state.value.storageUnavailable
                    }) {
                    if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner(id)) return@forEach
                    projectStore.reportOwnerConflict(id, owner, scopeStore.readSessionOwners()[id])
                    return@LaunchedEffect
                }
            }
            if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwnersGlobally()) return@LaunchedEffect
            catalogOwners = scopeStore.readSessionOwners()
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            projectStore.reportOwnerStateFailure(failure, "scheduled")
        }
    }
    val sessionState = rawSessionState.copy(
        phase = if (catalogOwners == null) SessionCatalogPhase.Loading else rawSessionState.phase,
        rows = rawSessionState.rows.filter { row ->
            catalogOwners?.get(canonicalSessionId(row.uuid)) == ScopeStateStore.SessionOwner(sourceScope.persistenceKey(), row.mode)
        },
    )
    // Durable per-scope conversation state (last-active session + draft),
    // keyed `global` / `scheduled` / `project.<id>`. Project/global last-active
    // stays with ProjectStore; this store records which scope was active
    // across process death.

    var cronResultsSeenAt by remember { mutableLongStateOf(0L) }
    LaunchedEffect(scopeStore, projectState.loading, projectState.storageUnavailable, scopeStateValid, cronAdmissionRevision) {
        if (projectState.loading || projectState.storageUnavailable || scopeStateValid != true) return@LaunchedEffect
        if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwnersGlobally()) return@LaunchedEffect
        try {
            if (!validateOwnerIndexes()) return@LaunchedEffect
            // Seed owners only from an unambiguous existing logical index. A cwd
            // catalog alone cannot establish which workspace owns a session.
            val indexed = projectState.globalSessions.map { it.sessionId to ScopeStateStore.SessionOwner("global", it.mode) } +
                projectState.projects.flatMap { project -> project.sessions.map {
                    it.sessionId to ScopeStateStore.SessionOwner(ConversationScope.Project(project.record.id).persistenceKey(), it.mode)
                } }
            indexed.groupBy { canonicalSessionId(it.first) }.forEach { (id, claims) ->
                val owners = claims.map { it.second }.distinct()
                if (id.isBlank() || id == "new") return@forEach
                if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner(id)) return@forEach
                val owner = owners.first()
                if (owners.size != 1 || !scopeStore.claimSessionOwner(id, owner.scopeKey, owner.mode) { scopeStateValid == true && !projectStore.state.value.storageUnavailable }) {
                    if (projectStore.state.value.storageUnavailable) return@LaunchedEffect
                    if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner(id)) return@forEach
                    val recorded = if (owners.size != 1) owners.last() else scopeStore.readSessionOwners()[id]
                    projectStore.reportOwnerConflict(id, owner, recorded)
                    return@LaunchedEffect
                }
            }
            if (!com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwnersGlobally()) return@LaunchedEffect
            if (!drawerBootstrapComplete) {
                val restoredMode = scopeStore.readActiveMode() ?: SessionMode.Code
                activeSessionMode = restoredMode
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
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            projectStore.reload()
        }
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
        // The capture state is the app's, not this screen's: a failure while another tab is using the microphone is that tab's to explain.
        if (captureState.phase == com.lingxi.code.voice.VoiceCapturePhase.Failed && (tabState == null || tabState.selected == com.lingxi.code.tabs.AppTab.Chats)) {
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(message = captureState.errorMessage ?: captureState.statusText)
            voiceActive = false
        }
    }

    // FlowMode (心流) voice-orb overlay visibility — a TAP on the composer mic
    // flips it on (the long-press STT path still drives `voiceActive`). The
    // overlay renders above the drawer + conversation + voice-flow overlay.
    var flowActive by remember(chatViewModel) {
        if (viewModel == null) sessionHost.flowVisibleState(chatViewModel) else mutableStateOf(false)
    }
    var appInForeground by remember {
        mutableStateOf(lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED))
    }

    // Flow uses this actual Agent conversation; its audio controller pins each admitted route.
    val flowVoiceController = rememberFlowVoiceController(
        owner = chatViewModel,
        retained = viewModel == null,
        pendingPermissionOwner = sessionHost.pendingVoicePermission,
        audioAdmission = sessionHost.voiceAdmission,
        obtain = { create -> if (viewModel == null) sessionHost.flowController(chatViewModel, create) else create() },
    )
    val flowVoiceState by flowVoiceController.state.collectAsState()
    LaunchedEffect(flowVoiceState.error) {
        flowVoiceState.error?.let { error ->
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(error.kind.name, error.message)
        }
    }
    val orbAssistantText = (state.streamingMessage ?: state.messages.lastOrNull())
        ?.let { if (it.role == Role.Ai) it.text else "" } ?: ""
    LaunchedEffect(flowVoiceController, state.model.id, state.model.providerId) {
        flowVoiceController.updateSessionBinding(state.model.id, state.model.providerId)
    }
    DisposableEffect(lifecycleOwner, flowVoiceController, chatViewModel) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START, Lifecycle.Event.ON_RESUME -> {
                    appInForeground = true
                    if (viewModel == null) sessionHost.setAutomaticSpeechForeground(true)
                    chatViewModel.sideChats.resumeForeground()
                }
                Lifecycle.Event.ON_STOP -> {
                    chatViewModel.sideChats.cancelCurrentTurns()
                    appInForeground = false
                    if (viewModel == null) sessionHost.setAutomaticSpeechForeground(false)
                    catalogLifecycleScope.launch { runCatching { chatViewModel.engineSource.value.submitClientCommand(ClientCommand.RefreshProviderCatalog("background")) } }
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
    val providerAccountState by resolvedSettingsStore.providerAccounts.state.collectAsState()
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
    val currentWorkspace by currentEngineSource.workspaceState.collectAsStateWithLifecycle()
    LaunchedEffect(currentEngineSource, flowVoiceController, voiceSpeechPlayer) {
        currentEngineSource.bindAudioOwner(flowVoiceController.audioOwner)
        currentEngineSource.bindAudioOwner(voiceSpeechPlayer.audioOwner)
    }
    DisposableEffect(resolvedSettingsStore, currentEngineSource) {
        resolvedSettingsStore.bindProviderAccountSource(currentEngineSource)
        onDispose { resolvedSettingsStore.bindProviderAccountSource(null) }
    }
    LaunchedEffect(resolvedSettingsStore, currentEngineSource, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                resolvedSettingsStore.providerAccounts.refresh()
                delay(15_000)
            }
        }
    }
    LaunchedEffect(currentEngineSource, settingsState.voice, settingsState.llmProviders, state.session.id, state.model.id, appInForeground) {
        val audioSource = currentEngineSource as? com.lingxi.code.conversation.EngineConversationSource
        if (audioSource != null) audioSource.activateAudioContext() else com.lingxi.code.voice.audio.AndroidAudioSessionContext.clear()
        resolvedSettingsStore.setVoiceCapability(com.lingxi.code.settings.probeVoiceCapabilitySnapshot(context, settingsState.voice))
    }
    var pendingHandoffRequest by remember { mutableStateOf<PendingHandoffRequest?>(null) }
    var handoffJob by remember { mutableStateOf<Job?>(null) }
    var handoffCanCancel by remember { mutableStateOf(false) }
    val currentAutoPlayReplies = rememberUpdatedState(settingsState.voice.autoPlayReplies)
    val currentFlowActive = rememberUpdatedState(flowActive)
    val currentAppInForeground = rememberUpdatedState(appInForeground)
    LaunchedEffect(sessionHost, viewModel, settingsState.voice.autoPlayReplies, appInForeground) {
        if (viewModel == null) sessionHost.configureAutomaticSpeech(settingsState.voice.autoPlayReplies, appInForeground)
    }
    LaunchedEffect(sessionHost, viewModel, flowActive) {
        if (viewModel == null) sessionHost.automaticSpeechStateChanged()
    }
    DisposableEffect(sessionHost, viewModel) {
        onDispose { if (viewModel == null) sessionHost.setAutomaticSpeechForeground(false) }
    }
    LaunchedEffect(sessionHost, viewModel) {
        if (viewModel != null) return@LaunchedEffect
        sessionHost.automaticSpeechErrors.collect { (owner, error) ->
            if (sessionHost.selected.value === owner) {
                audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(error.kind.name, error.message)
            }
        }
    }
    LaunchedEffect(chatViewModel, voiceSpeechPlayer, viewModel) {
        if (viewModel == null) return@LaunchedEffect
        chatViewModel.turnCompletions.collect { completion ->
            if (completion.origin == ConversationTurnOrigin.Flow) {
                flowVoiceController.updateReply(
                    completion.finalAssistantText.takeIf { completion.outcome == ConversationTurnOutcome.Completed }.orEmpty(),
                    false,
                )
            }
            if (completion.origin != ConversationTurnOrigin.Ordinary) return@collect
            if (completion.outcome != ConversationTurnOutcome.Completed) return@collect
            if (!currentAppInForeground.value) return@collect
            if (!currentAutoPlayReplies.value || currentFlowActive.value) return@collect
            val text = completion.finalAssistantText.trim()
            if (text.isEmpty()) return@collect
            voiceSpeechPlayer.speakAsync(text, onError = { error ->
                if (viewModel != null || sessionHost.selected.value === chatViewModel) {
                    audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(error.kind.name, error.message)
                }
            })
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
    fun currentEngineMode(): SessionMode? = state.sessionMode ?: currentEngineSource.recoverySpec?.sessionMode

    fun sourceMatches(scope: ConversationScope): Boolean = isBoundToScope(sourceScope, scope)

    // The composer draft is hoisted here so a voice transcription (the
    // hold-to-talk release) can route its recognized text straight into the
    // input the user is about to send. Drafts follow the conversation owner,
    // including when its profile changes, rather than a workspace/mode slot.
    val initialSavedDraft = remember(chatViewModel) { chatViewModel.restoredDraft }
    val draftCell = remember(chatViewModel) { if (viewModel == null) sessionHost.draftState(chatViewModel) else mutableStateOf("") }
    var draft by draftCell
    val conversationSource by chatViewModel.engineSource.collectAsState()
    val visualizationHost = conversationSource.visualizationHost
    var voiceDraftBase by remember { mutableStateOf("") }
    var voiceGestureOwner by remember { mutableStateOf<ChatViewModel?>(null) }
    LaunchedEffect(captureState.phase) {
        if (captureState.phase == com.lingxi.code.voice.VoiceCapturePhase.PermissionRequired) {
            voiceCaptureOwner = null
            voiceGestureOwner = null
            voiceActive = false
        }
    }

    // The currently displayed draft and serialized writes belong to this owner.
    val draftOwnership = remember(chatViewModel) {
        if (viewModel == null) sessionHost.draftOwnership(chatViewModel)
        else com.lingxi.code.conversation.ConversationDraftOwnership()
    }
    var draftOwnerScope by draftOwnership.scope
    var draftOwnerSessionId by draftOwnership.sessionId
    var draftRevision by draftOwnership.revision
    var draftLoaded by draftOwnership.loaded
    var mayRestoreSavedDraft by draftOwnership.mayRestoreSaved

    fun queueDraftWrite(ownerScope: ConversationScope?, sessionId: String?, text: String): Job? {
        if (ownerScope == null || sessionId == null || sessionId == "new") return null
        return chatViewModel.persistConversationDraft(scopeStore, ownerScope, sessionId, text, ::reportScopeStateFailure)
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

    // All owners use their own ordered writer, including updates arriving while
    // they are hidden. Update the in-memory revision before any storage I/O so
    // a pending restore or an older write cannot replace newer user input.
    fun publishOwnerDraft(owner: ChatViewModel, ownerScope: ConversationScope, sessionId: String, text: String) {
        val ownership = sessionHost.draftOwnership(owner)
        ownership.scope.value = ownerScope
        ownership.sessionId.value = canonicalSessionId(sessionId)
        ownership.revision.longValue++
        ownership.loaded.value = true
        ownership.mayRestoreSaved.value = false
        sessionHost.draftState(owner).value = text
        owner.onDraftChanged(text, ownerScope, sessionId)
        if (canonicalSessionId(sessionId) != "new") {
            owner.persistConversationDraft(scopeStore, ownerScope, sessionId, text, ::reportScopeStateFailure)
        }
    }

    LaunchedEffect(drawerBootstrapComplete, sourceScope, state.session.id, state.sessionTransitioning, state.isNew,
        scopeStateValid, projectState.storageUnavailable) {
        if (!drawerBootstrapComplete || scopeStateValid != true || projectState.storageUnavailable) return@LaunchedEffect
        val nextId = canonicalSessionId(state.session.id)
        if (draftOwnerScope == sourceScope && draftOwnerSessionId == nextId && draftLoaded) return@LaunchedEffect
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
        scope.launch {
            try {
                chatViewModel.awaitConversationDraftWrites()
                val restored = scopeStore.readConversationDraft(owner.persistenceKey(), nextId) ?: saved.orEmpty()
                if (draftOwnerScope == owner && draftOwnerSessionId == nextId && draftRevision == revision) {
                    draft = restored
                    draftLoaded = true
                    chatViewModel.onDraftChanged(restored, owner, nextId)
                }
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                projectStore.reload()
            }
        }
    }

    // Hold-to-talk → live transcription, gated on RECORD_AUDIO. The recognized
    // partial utterance replaces the live voice suffix while recognition is
    // running; the final result replaces that same suffix on release.
    fun updateVoiceOwnerDraft(transcript: String) {
        val owner = voiceCaptureOwner ?: return
        val text = appendVoiceTranscript(voiceDraftBase, transcript)
        if (owner === chatViewModel) updateCurrentDraft(text)
        else if (viewModel == null) {
            val ownerScope = owner.sourceScope.value
            val ownerId = canonicalSessionId(owner.state.value.session.id)
            publishOwnerDraft(owner, ownerScope, ownerId, text)
        }
    }
    val (startVoiceCapture, stopVoiceCapture) = rememberVoiceCapture(
        onTranscript = { transcript ->
            updateVoiceOwnerDraft(transcript)
            voiceCaptureOwner = null
        },
        onPartialTranscript = { partial ->
            updateVoiceOwnerDraft(partial)
        },
        onError = { kind, message ->
            if (voiceCaptureOwner === chatViewModel) {
                audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.fromError(kind, message)
                voiceActive = false
            }
            voiceCaptureOwner = null
        },
        bindOwner = { owner -> chatViewModel.engineSource.value.bindAudioOwner(owner) },
        audioAdmission = if (viewModel == null) sessionHost.voiceAdmission else null,
    )
    val onVoiceHoldStart: () -> Unit = {
        if (voiceCaptureOwner != null) {
            voiceGestureOwner = null
            voiceActive = false
            audioGuidance = com.lingxi.code.voice.AudioSetupGuidance.Retry
        } else {
            audioGuidance = null
            voiceCaptureOwner = chatViewModel
            voiceGestureOwner = chatViewModel
            voiceDraftBase = draft
            startVoiceCapture()
            if (viewModel == null) sessionHost.yieldOtherOwnersAutomaticSpeech(chatViewModel)
        }
    }
    val onVoiceHoldRelease: () -> Unit = {
        if (voiceGestureOwner === chatViewModel) {
            voiceGestureOwner = null
            stopVoiceCapture()
        }
    }
    DisposableEffect(chatViewModel, flowVoiceController) {
        onDispose {
            flowVoiceController.navigationAway()
            if (voiceGestureOwner === chatViewModel) {
                voiceGestureOwner = null
                stopVoiceCapture()
                voiceActive = false
            }
        }
    }

    // The captured-photo attachment is hoisted here exactly like the voice draft:
    // tapping the composer's camera affordance drives an on-device capture through
    // the same CameraController the engine bridges onto `traits::CameraControl`,
    // and the resulting JPEG surfaces as a composer thumbnail (the device-vision
    // analog of how onTranscript surfaces a recognized utterance).
    val attachmentCell = remember(chatViewModel) { if (viewModel == null) sessionHost.attachmentState(chatViewModel) else mutableStateOf<ComposerAttachment?>(null) }
    var attachment by attachmentCell
    var pendingComposerRestore by remember(chatViewModel) { mutableStateOf<com.lingxi.code.conversation.ComposerRestoreGate?>(null) }

    var cameraRequestOwner by remember { mutableStateOf<ChatViewModel?>(null) }
    val launchCamera = rememberCameraCapture(
        onCaptured = { image ->
            val bmp = BitmapFactory.decodeByteArray(image.jpegBytes, 0, image.jpegBytes.size)
            if (bmp != null) {
                val captured = ComposerAttachment(
                    thumb = bmp.asImageBitmap(),
                    width = image.width,
                    height = image.height,
                    mediaType = "image/jpeg",
                    base64 = Base64.encodeToString(image.jpegBytes, Base64.NO_WRAP),
                )
                val owner = cameraRequestOwner
                if (viewModel == null && owner != null) {
                    sessionHost.attachmentState(owner).value = captured
                    sessionHost.draftOwnership(owner).attachmentRevision.longValue++
                } else {
                    attachment = captured
                    draftOwnership.attachmentRevision.longValue++
                }
                cameraRequestOwner = null
            }
        },
    )
    val onCameraClick = { cameraRequestOwner = chatViewModel; launchCamera() }

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
        runCatching { chatViewModel.engineSource.value.submitClientCommand(ClientCommand.RefreshProviderCatalog(if (isOnline) "network_restored" else "network_lost")) }
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
        sessionModeOverride: SessionMode = activeSessionMode,
        expectedRuntimeGeneration: String? = null,
        navigationIntent: Long = nextNavigationIntent(),
        selectedTranscriptPath: String? = null,
        requireSelectedTranscript: Boolean = false,
        preparedAuthoringGrant: com.lingxi.code.project.PreparedAuthoringGrant? = null,
        authoringIntent: com.lingxi.code.apps.AuthoringIntent? = null,
        onActivated: (ChatViewModel, Boolean) -> Unit = { _, _ -> },
        presentationIsCurrent: () -> Boolean = { true },
    ): Boolean {
        fun navigationIsCurrent() = navigationIntent == navigationSequence.get() && presentationIsCurrent()
        return try {
            val destination = target ?: SessionRef("new", resources.getString(R.string.chat_new_conversation))
            if (viewModel != null || !navigationIsCurrent()) return false
            val sessionId = destination.id.takeUnless { newSession || it == "new" }
            if (requireSelectedTranscript && (sessionId == null || selectedTranscriptPath?.startsWith('/') != true)) return false
            fun savedPathMatchesSelection(): Boolean = !requireSelectedTranscript || (sessionId != null &&
                com.lingxi.code.conversation.selectedTranscriptPathMatches(selectedTranscriptPath,
                    com.lingxi.code.conversation.sessionTranscriptPath(engineScope, sessionId, sessionModeOverride,
                        projectStore.state.value, cronRepository.state.value.generatedSessions)))
            if (!savedPathMatchesSelection()) return false
            if (sessionId != null) {
                val retained = sessionHost.acquireRetained(engineScope, sessionModeOverride, sessionId, expectedRuntimeGeneration,
                    selectedTranscriptPath.takeIf { requireSelectedTranscript })
                if (retained != null) {
                    var fencedSource: ConversationSource? = null
                    try {
                        fun retainedAuthoringMatches(): Boolean {
                            val grant = preparedAuthoringGrant ?: return true
                            val source = retained.viewModel.engineSource.value
                            val recovery = source.recoverySpec
                            val sourceIdentity = source.pendingAuthoringSourceIdentity()
                            return (fencedSource == null || source === fencedSource) &&
                                !retained.viewModel.state.value.blocksEngineReconnect(retained.viewModel.pendingPermission.value != null) &&
                                !retained.viewModel.sideChats.hasLiveChats && !sessionHost.ownerHasVoiceWork(retained.viewModel) &&
                                sourceIdentity != null && com.lingxi.code.project.sameAuthoringSource(sourceIdentity, grant.identity) &&
                                com.lingxi.code.project.retainedAuthoringSourceMatches(
                                grant.identity, sessionId, sessionModeOverride, source.activeSessionState.value?.sessionId,
                                source.workspaceState.value, recovery?.transcriptPath, recovery?.hostPath, recovery?.guestPath,
                                recovery?.projectId, recovery?.workspaceKey, recovery?.sessionMode)
                        }
                        if (!retainedAuthoringMatches()) return false
                        if (preparedAuthoringGrant != null) {
                            val source = retained.viewModel.engineSource.value
                            reserveValidatedAuthoringSelection(
                                reserve = { source.reserveAuthoringSelection().also { if (it) fencedSource = source } },
                                readWorkspace = { source.authoritativeAuthoringWorkspace() },
                                matches = { it == source.workspaceState.value && retainedAuthoringMatches() },
                                release = { source.releaseAuthoringSelection(); fencedSource = null },
                            ) ?: return false
                            val selection = checkNotNull(authoringIntent)
                            check(selection.projectId == preparedAuthoringGrant.identity.projectId &&
                                selection.sourceDirectory == preparedAuthoringGrant.identity.guestPath)
                            val retainedPath = retained.viewModel.engineSource.value.recoverySpec?.transcriptPath
                            check(!retainedPath.isNullOrBlank()) { "Authoring transcript is unavailable" }
                            com.lingxi.code.apps.AuthoringIntentStore(appContext).attach(selection.selectionId,
                                canonicalSessionId(sessionId), retainedPath, sessionModeOverride, trackLive = false)
                            com.lingxi.code.apps.AuthoringIntentStore(appContext).markCatalogued(canonicalSessionId(sessionId))
                        }
                        val projectId = (engineScope as? ConversationScope.Project)?.projectId
                        val stillSelected = {
                            navigationIsCurrent() && sessionHost.selected.value === retained.viewModel
                        }
                        val selected = projectStore.publishRetainedActive(projectId, allowCommit = {
                            navigationIsCurrent() && retainedAuthoringMatches() && savedPathMatchesSelection() && sessionHost.isRetainedLeaseCurrent(
                                retained, engineScope, sessionModeOverride, sessionId, expectedRuntimeGeneration,
                                selectedTranscriptPath.takeIf { requireSelectedTranscript })
                        }) {
                            sessionHost.activate(retained)
                            onActivated(retained.viewModel, true)
                            drawerUi.selectSession(sessionId)
                            restoredModeSessionKey = engineScope.sessionStateKey(sessionModeOverride)
                            newDraftScope = null
                            activeSessionMode = sessionModeOverride
                        }
                        if (selected) {
                            scope.launch {
                                if (!stillSelected() || scopeStateValid != true || projectStore.state.value.storageUnavailable) return@launch
                                runCatching {
                                    projectStore.persistActive(projectId) { stillSelected() && scopeStateValid == true }
                                    if (stillSelected()) scopeStore.persistActiveSelection(engineScope.persistenceKey(), sessionModeOverride) {
                                        stillSelected() && scopeStateValid == true && !projectStore.state.value.storageUnavailable
                                    }
                                }.onFailure { if (stillSelected()) retained.viewModel.reportHostError(it.message.orEmpty()) }
                            }
                            return true
                        }
                    } finally {
                        fencedSource?.releaseAuthoringSelection()
                        sessionHost.release(retained)
                    }
                    if (!navigationIsCurrent()) return false
                }
                // A mismatching retained owner must not be reacquired through the cold path.
                if (requireSelectedTranscript && sessionHost.find(engineScope, sessionId) != null) return false
            }
            if (projectStore.state.value.storageUnavailable) {
                projectStore.reload()
                return false
            }
            if (!validateOwnerIndexes()) return false
            if (sessionId != null && !scopeStore.matchesSessionOwner(sessionId, engineScope.persistenceKey(), sessionModeOverride)) return false
            var committed = false
            var ownerAdmitted = false
            data class PreparedTarget(val source: ConversationSource, val session: SessionRef, val path: String?)
            com.lingxi.code.conversation.withPreparedNavigation(
                mutex = navigationMutex,
                isCurrent = { navigationIsCurrent() },
                acquire = {
                    sessionHost.acquire(engineScope, sessionModeOverride, sessionId?.let { destination }, reconnectToken,
                        expectedRuntimeGeneration, isCurrent = { navigationIsCurrent() }) {
                        val indexedPath = sessionId?.let { id ->
                            com.lingxi.code.conversation.sessionTranscriptPath(engineScope, id, sessionModeOverride,
                                projectStore.state.value, cronRepository.state.value.generatedSessions)
                        }
                        val pathForConstruction = com.lingxi.code.conversation.selectedTranscriptPathForCreation(
                            selectedTranscriptPath, indexedPath, requireSelectedTranscript)
                        val pendingAuthoring = preparedAuthoringGrant?.let { grant ->
                            check(engineScope is ConversationScope.Project && sessionModeOverride == SessionMode.Code &&
                                grant.identity.projectId == engineScope.projectId &&
                                project?.workspace?.guestPath == grant.identity.guestPath &&
                                project?.workspace?.hostPath == grant.identity.hostPath) { "Authoring source changed" }
                            checkNotNull(grant.consume(grant.identity, navigationIntent, navigationIsCurrent())) {
                                "Authoring source selection expired"
                            }
                        }
                        EngineConversationSource.createInBackground(
                            context = appContext,
                            projectWorkspace = when (engineScope) {
                                ConversationScope.Global -> null
                                ConversationScope.Scheduled -> workspaceForScope(engineScope, sessionId)
                                is ConversationScope.Project -> project?.workspace
                            },
                            workspaceKey = engineScope.persistenceKey(),
                            sessionMode = sessionModeOverride,
                            linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                            reuseProcessSource = sessionId != null,
                            expectedSessionId = sessionId,
                            initialSessionTranscriptPath = pathForConstruction,
                            runtimeGeneration = expectedRuntimeGeneration ?: java.util.UUID.randomUUID().toString(),
                            strictRuntimeGeneration = expectedRuntimeGeneration != null,
                            pendingAuthoringAdmission = pendingAuthoring,
                        )
                    }
                },
                prepare = prepare@{ lease ->
                    if (projectStore.state.value.storageUnavailable || !savedPathMatchesSelection()) return@prepare null
                    val owner = lease.viewModel
                    val confirmed = kotlinx.coroutines.withTimeoutOrNull(20_000L) {
                        owner.state.first {
                            owner.sessionOwnerRejected || (it.sessionReady && it.session.id != "new" && it.sessionMode == sessionModeOverride &&
                                (sessionId == null || canonicalSessionId(it.session.id) == canonicalSessionId(sessionId)))
                        }
                    } ?: return@prepare null
                    if (owner.sessionOwnerRejected) {
                        sessionHost.selected.value.reportConversationLaunchFailed()
                        return@prepare null
                    }
                    val source = owner.engineSource.value
                    if (requireSelectedTranscript && !com.lingxi.code.conversation.selectedTranscriptPathMatches(
                            selectedTranscriptPath, source.recoverySpec?.transcriptPath)) return@prepare null
                    val preparedPath = if (newSession && engineScope != ConversationScope.Scheduled) {
                        kotlinx.coroutines.withTimeoutOrNull(20_000L) {
                            source.workspaceState.first { workspace -> workspace != null &&
                                canonicalSessionId(workspace.sessionId) == canonicalSessionId(confirmed.session.id) &&
                                workspace.transcriptPath.isNotBlank() }
                        }?.transcriptPath ?: return@prepare null
                    } else null
                    if (preparedAuthoringGrant != null && !source.turnAdmissionReady) {
                        val intent = checkNotNull(authoringIntent)
                        check(intent.projectId == (engineScope as? ConversationScope.Project)?.projectId &&
                            intent.sourceDirectory == preparedAuthoringGrant.identity.guestPath)
                        val sourcePath = preparedPath ?: source.recoverySpec?.transcriptPath
                        check(!sourcePath.isNullOrBlank()) { "Authoring transcript is unavailable" }
                        com.lingxi.code.apps.AuthoringIntentStore(appContext).attach(intent.selectionId, canonicalSessionId(confirmed.session.id),
                            sourcePath, checkNotNull(confirmed.sessionMode))
                    }
                    PreparedTarget(source, confirmed.session, preparedPath)
                },
                commit = commit@{ lease, prepared ->
                    val owner = lease.viewModel
                    val source = prepared.source
                    val confirmed = prepared.session
                    if (owner.engineSource.value !== source || !navigationIsCurrent() || !savedPathMatchesSelection()) return@commit false
                    if (requireSelectedTranscript && !com.lingxi.code.conversation.selectedTranscriptPathMatches(
                            selectedTranscriptPath, source.recoverySpec?.transcriptPath)) return@commit false
                    if (!navigationIsCurrent() || !validateOwnerIndexes()) return@commit false
                    if (projectStore.state.value.storageUnavailable ||
                        !scopeStore.claimSessionOwner(confirmed.id, engineScope.persistenceKey(), sessionModeOverride) {
                            scopeStateValid == true && !projectStore.state.value.storageUnavailable && navigationIsCurrent()
                        }) return@commit false
                    sessionHost.register(owner, engineScope, sessionModeOverride)
                    ownerAdmitted = true
                    kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                        if (projectStore.state.value.storageUnavailable || !navigationIsCurrent()) return@withContext
                        if (newSession && preparedAuthoringGrant != null) {
                            check(projectStore.recordStartedSession((engineScope as ConversationScope.Project).projectId,
                                confirmed.id, confirmed.title, sessionModeOverride, transcriptPath = prepared.path,
                                allowCommit = { navigationIsCurrent() }, markActive = { false })) {
                                "Unable to save the authoring session"
                            }
                        }
                        if (preparedAuthoringGrant != null && !source.turnAdmissionReady)
                            com.lingxi.code.apps.AuthoringIntentStore(appContext).markCatalogued(canonicalSessionId(confirmed.id))
                        val previousOwner = sessionHost.selected.value
                        val previousScope = previousOwner.sourceScope.value
                        val previousProject = (previousScope as? ConversationScope.Project)?.projectId
                        val previousMode = previousOwner.state.value.sessionMode ?: activeSessionMode
                        try {
                            val persisted = projectStore.persistActive((engineScope as? ConversationScope.Project)?.projectId) {
                                navigationIsCurrent()
                            }
                            if (!navigationIsCurrent()) {
                                projectStore.persistActive(previousProject) { sessionHost.selected.value === previousOwner }
                                return@withContext
                            }
                            if (!scopeStore.persistActiveSelection(engineScope.persistenceKey(), sessionModeOverride) {
                                    scopeStateValid == true && !projectStore.state.value.storageUnavailable && navigationIsCurrent()
                                }) {
                                runCatching { projectStore.persistActive(previousProject) { sessionHost.selected.value === previousOwner } }
                                return@withContext
                            }
                            if (!navigationIsCurrent()) {
                                scopeStore.persistActiveSelection(previousScope.persistenceKey(), previousMode) { sessionHost.selected.value === previousOwner }
                                projectStore.persistActive(previousProject) { sessionHost.selected.value === previousOwner }
                                return@withContext
                            }
                            val published = synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                                if (!projectStore.publishActive(persisted, allowCommit = { navigationIsCurrent() }) {
                                        sessionHost.activate(lease)
                                        onActivated(owner, false)
                                    }) false
                                else {
                                    drawerUi.selectSession(confirmed.id)
                                    restoredModeSessionKey = engineScope.sessionStateKey(sessionModeOverride)
                                    newDraftScope = engineScope.takeIf { newSession }
                                    activeSessionMode = sessionModeOverride
                                    committed = true
                                    true
                                }
                            }
                            if (!published) {
                                runCatching { scopeStore.persistActiveSelection(previousScope.persistenceKey(), previousMode) { sessionHost.selected.value === previousOwner } }
                                runCatching { projectStore.persistActive(previousProject) { sessionHost.selected.value === previousOwner } }
                                return@withContext
                            }
                        } catch (failure: Exception) {
                            runCatching { projectStore.persistActive(previousProject) { sessionHost.selected.value === previousOwner } }
                            runCatching { scopeStore.persistActiveSelection(previousScope.persistenceKey(), previousMode) { sessionHost.selected.value === previousOwner } }
                            throw failure
                        }
                    }
                    committed
                },
                afterCommit = { lease, prepared ->
                    val owner = lease.viewModel
                    val confirmed = prepared.session
                    kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                        // Publication has already selected this owner. A later storage
                        // error must be reported without making the caller believe the
                        // navigation failed or releasing the selected source.
                        if (newSession && engineScope != ConversationScope.Scheduled) {
                            runCatching {
                                projectStore.recordStartedSession((engineScope as? ConversationScope.Project)?.projectId,
                                    confirmed.id, confirmed.title, sessionModeOverride, transcriptPath = prepared.path,
                                    allowCommit = { navigationIsCurrent() || owner.state.value.requiresBackgroundExecution ||
                                        owner.state.value.messages.any { it.role == com.lingxi.code.model.Role.User } },
                                    markActive = { sessionHost.selected.value === owner })
                            }.onFailure { owner.reportHostError(it.message.orEmpty()) }
                        }
                        if (engineScope is ConversationScope.Project) {
                            runCatching { projectStore.addVisibleMode(engineScope.projectId, sessionModeOverride) }
                                .onFailure { owner.reportHostError(it.message.orEmpty()) }
                        }
                    }
                },
                release = { lease ->
                    val abandonedSession = lease.viewModel.engineSource.value.activeSessionState.value?.sessionId
                    sessionHost.release(lease, discard = !committed && (!ownerAdmitted || preparedAuthoringGrant != null))
                    if (!committed && preparedAuthoringGrant != null) {
                        abandonedSession?.let { session ->
                            com.lingxi.code.apps.AuthoringIntentStore(appContext).releaseLive(canonicalSessionId(session))
                            projectStore.reload()
                        }
                    }
                },
            )
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            projectStore.reload()
            chatViewModel.reportHostError(failure.message.orEmpty())
            false
        }
    }

    /** Project-flow convenience: derive the scope from the snapshot. */
    suspend fun switchEngineScope(
        project: ProjectSnapshot?,
        target: SessionRef?,
        newSession: Boolean,
        sessionModeOverride: SessionMode = activeSessionMode,
        navigationIntent: Long = nextNavigationIntent(),
        selectedTranscriptPath: String? = null,
        requireSelectedTranscript: Boolean = false,
    ): Boolean = switchEngineScope(
        engineScope = project?.let { ConversationScope.Project(it.record.id) } ?: ConversationScope.Global,
        project = project,
        target = target,
        newSession = newSession,
        sessionModeOverride = sessionModeOverride,
        navigationIntent = navigationIntent,
        selectedTranscriptPath = selectedTranscriptPath,
        requireSelectedTranscript = requireSelectedTranscript,
    )

    LaunchedEffect(pendingModeNavigation) {
        val (mode, intent) = pendingModeNavigation ?: return@LaunchedEffect
        try {
            // The prior mode's exact in-process owner is already admitted. Do
            // not make returning to it wait for an unrelated fsync or draft write.
            val retained = if (viewModel == null) sessionHost.retainedDestination(mode) else null
            val retainedArchived = retained?.let { target ->
                when (val targetScope = target.scope) {
                    is ConversationScope.Project -> projectSnapshotForScope(targetScope)?.sessions
                        ?.any { canonicalSessionId(it.sessionId) == canonicalSessionId(target.session.id) && it.isArchived } == true
                    ConversationScope.Global -> projectState.globalSessions
                        .any { canonicalSessionId(it.sessionId) == canonicalSessionId(target.session.id) && it.isArchived }
                    ConversationScope.Scheduled -> false
                }
            } ?: false
            if (retained != null && !retainedArchived) {
                queueCurrentDraft()
                if (switchEngineScope(retained.scope, projectSnapshotForScope(retained.scope), retained.session, false,
                        sessionModeOverride = mode, navigationIntent = intent)) return@LaunchedEffect
                if (intent != navigationSequence.get()) return@LaunchedEffect
            }
            if (!validateOwnerIndexes()) return@LaunchedEffect
            val targetScope = resolveLinkedScope(scopeStore.readActiveScopeKey(mode)) ?: ConversationScope.Global
            val slot = scopeStore.read(targetScope.sessionStateKey(mode))?.lastActiveSessionId
            val summary = when (targetScope) {
                is ConversationScope.Project -> projectSnapshotForScope(targetScope)?.sessions?.firstOrNull {
                    it.sessionId == slot && it.mode == mode && !it.isArchived
                }
                ConversationScope.Global -> projectState.globalSessions.firstOrNull {
                    it.sessionId == slot && it.mode == mode && !it.isArchived
                }
                ConversationScope.Scheduled -> null
            }
            val knownOwner = slot?.let { scopeStore.readSessionOwners()[canonicalSessionId(it)] }
            val isArchived = when (targetScope) {
                is ConversationScope.Project -> projectSnapshotForScope(targetScope)?.sessions?.any { it.sessionId == slot && it.isArchived } == true
                ConversationScope.Global -> projectState.globalSessions.any { it.sessionId == slot && it.isArchived }
                ConversationScope.Scheduled -> false
            }
            val resume = summary?.let { SessionRef(it.sessionId, it.title) } ?: slot
                ?.takeIf { !isArchived && knownOwner == ScopeStateStore.SessionOwner(targetScope.persistenceKey(), mode) }
                ?.let { SessionRef(it, "") }
            queueCurrentDraft()
            if (resume == null) {
                switchEngineScope(targetScope, projectSnapshotForScope(targetScope), null, true,
                    sessionModeOverride = mode, navigationIntent = intent)
            } else {
                switchEngineScope(targetScope, projectSnapshotForScope(targetScope),
                    resume, false, sessionModeOverride = mode, navigationIntent = intent)
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            projectStore.reload()
        } finally {
            if (pendingModeNavigation == (mode to intent)) pendingModeNavigation = null
        }
    }

    LaunchedEffect(requestedProjectSwitch, projectState.loading) {
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
        cronState.loading,
    ) {
        val request = requestedConversationLaunch ?: return@LaunchedEffect
        val targetScope = request.workspaceKey?.let(::conversationScopeFromKey) ?: sourceScope
        if (request.workspaceKey != null && conversationScopeFromKey(request.workspaceKey) == null) {
            chatViewModel.reportConversationLaunchFailed()
            onConversationLaunchHandled()
            return@LaunchedEffect
        }
        if (!com.lingxi.code.conversation.canResolveConversationNotification(projectState.loading, cronState.loading)) return@LaunchedEffect
        if (targetScope is ConversationScope.Project && projectSnapshotForScope(targetScope) == null) {
            chatViewModel.reportConversationLaunchFailed()
            onConversationLaunchHandled()
            return@LaunchedEffect
        }
        val targetMode = request.sessionMode
        val routed = switchEngineScope(
            engineScope = targetScope,
            project = projectSnapshotForScope(targetScope),
            target = SessionRef(request.sessionId, ""),
            newSession = false,
            sessionModeOverride = targetMode,
            expectedRuntimeGeneration = request.runtimeGeneration,
            selectedTranscriptPath = request.transcriptPath,
            requireSelectedTranscript = true,
        )
        if (!routed) chatViewModel.reportConversationLaunchFailed()
        onConversationLaunchHandled()
    }

    suspend fun continueSessionInMode(
        sessionScope: ConversationScope,
        row: SessionRow,
        targetMode: SessionMode,
        navigationIntent: Long,
    ) {
        if (pendingHandoffRequest != null) {
            chatViewModel.reportHostError(resources.getString(R.string.drawer_continue_session_in_progress))
            return
        }
        val request = PendingHandoffRequest(sessionScope, row.mode, targetMode, row.uuid, row.title)
        pendingHandoffRequest = request
        val requestId = java.util.UUID.randomUUID().toString()
        var summarySource: ConversationSource? = null
        var sourceLease: SessionHost.Lease? = null
        var targetLease: SessionHost.Lease? = null
        var targetIndexed = false
        var sourceHandoffAdmitted = false
        try {
            fun savedSourceMatches(): Boolean = com.lingxi.code.conversation.selectedTranscriptPathMatches(
                row.transcriptPath, com.lingxi.code.conversation.sessionTranscriptPath(
                    sessionScope, row.uuid, row.mode, projectStore.state.value, cronRepository.state.value.generatedSessions))
            projectStore.validateOwnerIndexes()
            check(!projectStore.state.value.storageUnavailable &&
                scopeStore.matchesSessionOwner(row.uuid, sessionScope.persistenceKey(), row.mode) && savedSourceMatches())
            sourceLease = sessionHost.acquire(sessionScope, row.mode, SessionRef(row.uuid, row.title), reconnectToken) {
                val indexedPath = com.lingxi.code.conversation.sessionTranscriptPath(
                    sessionScope, row.uuid, row.mode, projectStore.state.value, cronRepository.state.value.generatedSessions)
                val pathForConstruction = com.lingxi.code.conversation.selectedTranscriptPathForCreation(
                    row.transcriptPath, indexedPath, required = true)
                EngineConversationSource.createInBackground(
                    context = appContext,
                    projectWorkspace = workspaceForScope(sessionScope, row.uuid),
                    workspaceKey = sessionScope.persistenceKey(),
                    sessionMode = row.mode,
                    linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                    reuseProcessSource = true,
                    expectedSessionId = row.uuid,
                    initialSessionTranscriptPath = pathForConstruction,
                )
            } ?: return
            val owner = checkNotNull(sourceLease).viewModel
            val source = owner.engineSource.value
            summarySource = source
            if (source is com.lingxi.code.conversation.UnavailableConversationSource) error(source.reason)
            kotlinx.coroutines.withTimeout(20_000L) {
                owner.state.first { it.sessionReady && it.sessionMode == row.mode &&
                    canonicalSessionId(it.session.id) == canonicalSessionId(row.uuid) }
            }
            fun sourceStillMatches(): Boolean = owner.engineSource.value === source && savedSourceMatches() &&
                sessionHost.isRetainedLeaseCurrent(checkNotNull(sourceLease), sessionScope, row.mode, row.uuid,
                    expectedTranscriptPath = row.transcriptPath)
            check(sourceStillMatches()) { "Selected conversation source changed" }
            projectStore.validateOwnerIndexes()
            check(!projectStore.state.value.storageUnavailable &&
                scopeStore.claimSessionOwner(row.uuid, sessionScope.persistenceKey(), row.mode) {
                    sourceStillMatches() && scopeStateValid == true && !projectStore.state.value.storageUnavailable
                })
            check(sourceStillMatches()) { "Selected conversation source changed" }
            coroutineScope {
                val response = async(start = kotlinx.coroutines.CoroutineStart.UNDISPATCHED) {
                    withTimeoutOrNull(120_000L) {
                        source.clientEvents.first { event ->
                            (event is ClientEvent.SessionHandoffPrepared && event.requestId == requestId) ||
                                (event is ClientEvent.SessionHandoffFailed && event.requestId == requestId)
                        }
                    }
                }
                sourceHandoffAdmitted = true
                source.submitClientCommand(ClientCommand.PrepareSessionHandoff(requestId, targetMode.toDto()))
                handoffCanCancel = true
                when (val event = response.await()) {
                    is ClientEvent.SessionHandoffPrepared -> {
                        handoffCanCancel = false
                        check(sourceStillMatches()) { "Selected conversation source changed" }
                        check(canonicalSessionId(event.sourceSessionId) == canonicalSessionId(row.uuid) &&
                            event.sourceMode.toUi() == row.mode && event.targetMode.toUi() == targetMode)
                        val summary = event.summary.sourceText()
                        check(summary.isNotBlank()) { resources.getString(R.string.drawer_continue_session_timeout) }
                        projectStore.validateOwnerIndexes()
                        targetLease = sessionHost.acquire(ConversationScope.Global, targetMode, null, reconnectToken) {
                            EngineConversationSource.createInBackground(
                                context = appContext,
                                workspaceKey = ConversationScope.Global.persistenceKey(),
                                sessionMode = targetMode,
                                linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                                reuseProcessSource = false,
                            )
                        } ?: error(resources.getString(R.string.drawer_continue_session_timeout))
                        val targetVm = checkNotNull(targetLease).viewModel
                        val confirmed = kotlinx.coroutines.withTimeout(20_000L) {
                            targetVm.state.first { it.sessionReady && it.session.id != "new" && it.sessionMode == targetMode }
                        }
                        val targetId = canonicalSessionId(confirmed.session.id)
                        val targetSource = targetVm.engineSource.value
                        val targetWorkspace = kotlinx.coroutines.withTimeout(20_000L) {
                            targetSource.workspaceState.first { workspace -> workspace != null &&
                                canonicalSessionId(workspace.sessionId) == targetId && workspace.transcriptPath.isNotBlank() }
                        }
                        check(targetVm.engineSource.value === targetSource)
                        kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                            projectStore.validateOwnerIndexes()
                            val targetStillMatches = {
                                targetVm.engineSource.value === targetSource &&
                                    targetSource.workspaceState.value == targetWorkspace &&
                                    canonicalSessionId(targetVm.state.value.session.id) == targetId &&
                                    targetVm.state.value.sessionMode == targetMode
                            }
                            check(targetStillMatches())
                            check(!projectStore.state.value.storageUnavailable &&
                                scopeStore.claimSessionOwner(targetId, ConversationScope.Global.persistenceKey(), targetMode) {
                                    targetStillMatches() && scopeStateValid == true && !projectStore.state.value.storageUnavailable
                                })
                            sessionHost.register(targetVm, ConversationScope.Global, targetMode)
                            check(projectStore.recordStartedSession(null, targetId, confirmed.session.title, targetMode,
                                transcriptPath = checkNotNull(targetWorkspace).transcriptPath, allowCommit = targetStillMatches))
                            targetIndexed = true
                            val targetScope = ConversationScope.Global
                            com.lingxi.code.conversation.completeHandoffDraft(
                                summary = summary,
                                persistHandoff = { text -> scopeStore.persistHandoffDraft(targetScope.persistenceKey(), targetId, text,
                                    ScopeStateStore.ConversationHandoff(sessionScope.persistenceKey(), row.uuid,
                                        row.mode, targetMode, event.cutoff, requestId)) },
                                currentDraft = {
                                    check(targetStillMatches())
                                    val current = sessionHost.draftState(targetVm).value
                                    current.takeIf { it.isNotEmpty() || sessionHost.draftOwnership(targetVm).loaded.value }
                                },
                                publishDraft = { text -> publishOwnerDraft(targetVm, targetScope, targetId, text) },
                            )
                        }
                        if (navigationIntent == navigationSequence.get() && switchEngineScope(
                                ConversationScope.Global, null, SessionRef(targetId, confirmed.session.title), false,
                                sessionModeOverride = targetMode, navigationIntent = navigationIntent,
                            )) closeDrawer()
                    }
                    is ClientEvent.SessionHandoffFailed -> error(event.message)
                    else -> {
                        runCatching { source.submitClientCommand(ClientCommand.CancelSessionHandoff(requestId)) }
                        error(resources.getString(R.string.drawer_continue_session_timeout))
                    }
                }
            }
        } catch (error: Exception) {
            if (error is kotlinx.coroutines.CancellationException) {
                kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                    runCatching { summarySource?.submitClientCommand(ClientCommand.CancelSessionHandoff(requestId)) }
                }
                throw error
            }
            chatViewModel.reportHostError(error.message ?: error::class.simpleName.orEmpty())
        } finally {
            kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                targetLease?.let { sessionHost.release(it, discard = !targetIndexed) }
                sourceLease?.let { sessionHost.release(it, discard = !sourceHandoffAdmitted) }
            }
            handoffCanCancel = false
            pendingHandoffRequest = null
        }
    }

    fun launchHandoff(ownerScope: ConversationScope, row: SessionRow, mode: SessionMode) {
        if (handoffJob?.isActive == true) return
        val intent = nextNavigationIntent()
        handoffJob = scope.launch { continueSessionInMode(ownerScope, row, mode, intent) }
    }

    // Persist the mode-scoped last-active session once the engine confirms it.
    LaunchedEffect(
        sourceScope,
        activeSessionMode,
        currentEngineSource,
        state.session.id,
        state.sessionReady,
        restoredModeSessionKey,
        projectState.storageUnavailable,
        projectState.globalSessions,
        projectState.projects,
        cronState.generatedSessions,
    ) {
        val active = sourceScope
        val engineMode = currentEngineMode()
        val owner = chatViewModel
        val source = currentEngineSource
        val sessionId = canonicalSessionId(state.session.id)
        val intent = navigationSequence.get()
        val indexedPath = com.lingxi.code.conversation.sessionTranscriptPath(active, sessionId, activeSessionMode,
            projectStore.state.value, cronRepository.state.value.generatedSessions)
        if (indexedPath != null && !projectStore.state.value.storageUnavailable && shouldPersistActiveSession(state.sessionReady, state.session.id, engineMode, activeSessionMode,
                active.sessionStateKey(activeSessionMode), restoredModeSessionKey)) {
            runCatching {
                scopeStore.persistLastActiveSession(
                    active.sessionStateKey(activeSessionMode),
                    sessionId,
                    allowCommit = { navigationSequence.get() == intent && owner.engineSource.value === source &&
                        owner.sourceScope.value == active && owner.state.value.sessionMode == activeSessionMode &&
                        !projectStore.state.value.storageUnavailable && canonicalSessionId(owner.state.value.session.id) == sessionId },
                )
            }
        }
    }

    LaunchedEffect(currentEngineSource, sourceProjectId, sourceScope, state.session.id, state.sessionMode, cronAdmissionRevision) {
        if (sourceScope == ConversationScope.Scheduled) return@LaunchedEffect
        val owner = chatViewModel
        val source = currentEngineSource
        val scopeKey = sourceScope
        val mode = state.sessionMode
        val generation = source.recoverySpec?.runtimeGeneration
        source.workspaceState.collect { workspace ->
            if (workspace != null && mode != null && canonicalSessionId(workspace.sessionId) == canonicalSessionId(owner.state.value.session.id)) {
                val id = canonicalSessionId(workspace.sessionId)
                val stored = projectStore.state.value.let { current ->
                    if (sourceProjectId == null) current.globalSessions else current.projects.firstOrNull { it.record.id == sourceProjectId }?.sessions.orEmpty()
                }.firstOrNull { canonicalSessionId(it.sessionId) == id && it.mode == mode }
                runCatching { projectStore.recordWorkspace(sourceProjectId, workspace, expectedPreviousPath = stored?.transcriptPath, checkPreviousPath = true,
                    allowCommit = { owner.engineSource.value === source && owner.sourceScope.value == scopeKey &&
                        owner.state.value.sessionMode == mode && canonicalSessionId(owner.state.value.session.id) == id &&
                        source.recoverySpec?.runtimeGeneration == generation && source.workspaceState.value == workspace &&
                        (viewModel != null || sessionHost.hasRegisteredOwner(owner)) },
                    updateProjectDirectory = { viewModel != null || sessionHost.selected.value === owner }) }
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
        cronAdmissionRevision,
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
        currentWorkspace,
        cronAdmissionRevision,
    ) {
        val engineMode = currentEngineMode()
        if (
            engineMode == activeSessionMode &&
            state.sessionReady &&
            state.session.id != "new" &&
            currentWorkspace?.let { canonicalSessionId(it.sessionId) == canonicalSessionId(state.session.id) && it.transcriptPath.isNotBlank() } == true &&
            (if (sourceProjectId == null) projectState.globalSessions else projectState.projects.firstOrNull { it.record.id == sourceProjectId }?.sessions.orEmpty()).none {
                it.sessionId == state.session.id && !it.transcriptPath.isNullOrBlank() &&
                    (it.messageCount > 0 || state.messages.none { message -> message.role == com.lingxi.code.model.Role.User })
            } &&
            sourceScope != ConversationScope.Scheduled
        ) {
            val owner = chatViewModel
            val source = currentEngineSource
            val ownerScope = sourceScope
            val ownerId = canonicalSessionId(state.session.id)
            runCatching {
                projectStore.recordStartedSession(
                    projectId = sourceProjectId,
                    sessionId = state.session.id,
                    title = state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User }?.text?.take(120) ?: state.session.title,
                    mode = activeSessionMode,
                    initialMessageCount = if (state.messages.any { it.role == com.lingxi.code.model.Role.User }) 1 else 0,
                    transcriptPath = currentWorkspace?.transcriptPath,
                    allowCommit = { owner.engineSource.value === source && owner.sourceScope.value == ownerScope &&
                        owner.state.value.sessionMode == engineMode && canonicalSessionId(owner.state.value.session.id) == ownerId &&
                        source.workspaceState.value == currentWorkspace },
                    markActive = { viewModel != null || sessionHost.selected.value === owner },
                )
                // The first prompt can follow /cd, before there was any row
                // for WorkspaceChanged to annotate. Preserve the engine's
                // original root and exact transcript instead of guessing them.
                source.workspaceState.value?.takeIf {
                    canonicalSessionId(it.sessionId) == canonicalSessionId(state.session.id)
                }?.let { workspace ->
                    projectStore.recordWorkspace(sourceProjectId, workspace,
                        allowCommit = { owner.engineSource.value === source && owner.sourceScope.value == ownerScope &&
                            canonicalSessionId(owner.state.value.session.id) == ownerId && source.workspaceState.value == workspace },
                        updateProjectDirectory = { viewModel != null || sessionHost.selected.value === owner })
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
            transcriptPath = cached.transcriptPath,
        )
    }
    val scheduledSessionRows = cronState.generatedSessions.filter { it.sessionScopeKey == "scheduled" && it.sessionId != null && !it.sessionTranscriptPath.isNullOrBlank() }
        .distinctBy { it.sessionId }.mapNotNull { run ->
            val id = canonicalSessionId(run.sessionId!!)
            val owner = catalogOwners?.get(id)?.takeIf { it.scopeKey == ConversationScope.Scheduled.persistenceKey() && it.mode.wireValue == run.sessionMode }
                ?: return@mapNotNull null
            SessionRow(uuid = id, title = run.prompt.take(120), messageCount = 2, mode = owner.mode,
                relativeTime = formatCronTime(run.finishedAtMs ?: run.triggeredAtMs),
                modifiedAtEpochSeconds = (run.finishedAtMs ?: run.triggeredAtMs) / 1000L,
                transcriptPath = run.sessionTranscriptPath)
        }
    val globalDrawerSessions = (if (sourceScope == ConversationScope.Global) {
        sessionState.withCachedRows(cachedGlobalRows, projectState.globalSessions.filter { it.pendingCatalogConfirmation }.map { it.sessionId }.toSet()).let { catalog ->
            val archivedIds = projectState.globalSessions.filter { it.isArchived }.map { it.sessionId }.toSet()
            catalog.copy(rows = catalog.rows.filterNot { it.uuid in archivedIds })
        }
    } else {
        EngineSessionState.ready(cachedGlobalRows)
    }).let { it.copy(rows = (it.rows + scheduledSessionRows).distinctBy(SessionRow::uuid)) }
    val drawerProductionData = DrawerProductionData(
        workspaces = listOf(LocalProjectWorkspace.copy(name = stringResource(R.string.project_storage_local))),
        projects = projectState.projects.filter { activeSessionMode in it.record.visibleModes }.map { project ->
            project.copy(sessions = project.sessions.filter { it.mode == activeSessionMode })
                .toDrawerProject(com.lingxi.code.conversation.conversationStrings(context))
        },
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

    fun submitComposer(text: String, image: ComposerAttachment?) {
        val owner = chatViewModel
        val ownerScope = sourceScope
        val ownerId = canonicalSessionId(state.session.id)
        val ownership = draftOwnership
        val rejected = com.lingxi.code.conversation.RejectedComposerDraft(text, image, state.visualizationChip, ownerScope, ownerId)
        val gate = com.lingxi.code.conversation.ComposerRestoreGate(
            revision = { Triple(ownership.revision.longValue, ownership.attachmentRevision.longValue, owner.visualizationDraftRevision()) },
            restore = {
                if (owner.sourceScope.value == ownerScope && canonicalSessionId(owner.state.value.session.id) == ownerId &&
                    ownership.scope.value == ownerScope && canonicalSessionId(ownership.sessionId.value.orEmpty()) == ownerId
                ) {
                    if (viewModel == null) publishOwnerDraft(owner, ownerScope, ownerId, text)
                    else {
                        ownership.revision.longValue++
                        ownership.loaded.value = true
                        draftCell.value = text
                        owner.onDraftChanged(text, ownerScope, ownerId)
                    }
                    attachmentCell.value = image
                    ownership.attachmentRevision.longValue++
                    owner.restoreVisualizationDraft(rejected.visualization)
                } else ownership.rejected.add(rejected)
            },
            preserve = { ownership.rejected.add(rejected) },
        )
        pendingComposerRestore = gate
        if (activeProjectSyncing) {
            owner.reportHostError(resources.getString(R.string.project_sync_in_progress_notice))
            gate.rejected()
        } else {
            voiceSpeechPlayer.stop()
            owner.send(text, images = image?.toImageRef()?.let(::listOf).orEmpty(),
                origin = ConversationTurnOrigin.Ordinary, onNotAdmitted = gate::rejected)
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
    // 定时任务 is a page of the chat (decision D14), not a Settings page.
    var cronPage by rememberSaveable(stateSaver = CronPageRoute.Saver) { mutableStateOf<CronPageRoute?>(null) }
    var marketInstallId by rememberSaveable { mutableStateOf<String?>(null) }
    var localAppDetailsId by rememberSaveable { mutableStateOf<String?>(null) }
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
    LaunchedEffect(requestedDeepLink, projectState.loading, cronState.loading, state.sessionReady, state.sessionTransitioning, state.streaming, approvedDraftLink, sourceScope, currentEngineSource, runtimeAppId, flowActive, voiceActive, captureState.phase, attachment, showCreateProject, activeProjectSyncing, workspacePickerOpen, routingDeepLink, draftLinkReview, linkedTargetError, linkedMessageUuid) {
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
        if (cronState.loading && link.route.startsWith("session/")) return@LaunchedEffect
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
                        if (cronState.generatedSessions.any { it.sessionScopeKey == "scheduled" && it.sessionId?.let(::canonicalSessionId) == canonicalSessionId(id) }) add(ConversationScope.Scheduled)
                        projectState.projects.filter { project -> project.sessions.any { it.sessionId.removePrefix("sess:") == id.removePrefix("sess:") } }.forEach { add(ConversationScope.Project(it.record.id)) }
                    }
                    return matches.singleOrNull()
                }
                try {
                    if (parts.first() !in setOf("project", "apps", "market")) {
                        appPageSequence.incrementAndGet()
                        projectsPageOpen = false; projectPageId = null; appsPageOpen = false; marketInstallId = null; localAppDetailsId = null; sessionPanelOpen = false
                        projectsPageOpen = false; projectPageId = null; appsPageOpen = false; marketInstallId = null; localAppDetailsId = null; sessionPanelOpen = false; cronPage = null
                    }
                    if (parts.first() in setOf("settings", "provider", "extensions", "cron", "terminal", "runtime", "import") && link.params["workspace"] != null) {
                        val targetScope = resolveLinkedScope(link.params["workspace"])
                            ?: error(resources.getString(R.string.deep_link_workspace_missing))
                        if (sourceScope != targetScope) {
                            queueCurrentDraft()
                            check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope), null, true)) { resources.getString(R.string.deep_link_busy) }
                            return@launch
                        }
                    }
                    when (parts.first()) {
                        "settings", "provider", "extensions", "terminal" -> onDeepLinkNativeReady(link, currentEngineSource)
                        // cron/run/<id> and cron/job/<id> open the scheduled-task page over the chat.
                        "cron" -> cronPage = if (parts[1] == "run") CronPageRoute.Run(id) else CronPageRoute.task(id)
                        "session" -> {
                            val targetScope = resolveScope() ?: error(resources.getString(R.string.deep_link_conversation_ambiguous))
                            val rows = when (targetScope) {
                                is ConversationScope.Project -> projectSnapshotForScope(targetScope)?.sessions.orEmpty()
                                ConversationScope.Global -> projectState.globalSessions
                                ConversationScope.Scheduled -> emptyList()
                            }
                            val row = rows.firstOrNull { isRestorableSession(id, it.sessionId) }
                            val scheduled = if (targetScope == ConversationScope.Scheduled) cronState.generatedSessions.firstOrNull { it.sessionScopeKey == "scheduled" && it.sessionId?.let(::canonicalSessionId) == canonicalSessionId(id) } else null
                            check(row != null || scheduled != null) { resources.getString(R.string.deep_link_conversation_workspace_missing) }
                            val sameSession = targetScope == sourceScope && canonicalSessionId(state.session.id) == canonicalSessionId(id)
                            val mode = link.params["mode"]?.let { com.lingxi.code.model.sessionModeFromWireValue(it) }
                                ?: error(resources.getString(R.string.deep_link_mode_mismatch))
                            check(sessionLinkModeMatchesOwner(mode, row?.mode, state.sessionMode, sameSession)) {
                                resources.getString(R.string.deep_link_mode_mismatch)
                            }
                            val generation = link.params["generation"]
                            check(!sameSession || generation == null || currentEngineSource.recoverySpec?.runtimeGeneration == generation) {
                                resources.getString(R.string.deep_link_conversation_open_failed)
                            }
                            queueCurrentDraft()
                            if (!sameSession) {
                                check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope),
                                    SessionRef(id, row?.title ?: scheduled?.prompt.orEmpty().take(120)), false,
                                    sessionModeOverride = mode, expectedRuntimeGeneration = generation)) { resources.getString(R.string.deep_link_busy) }
                                setConversationMode(mode)
                            }
                            linkedSessionId = id.removePrefix("sess:")
                            linkedScopeModeKey = targetScope.persistenceKey()
                            linkedMessageSource = sessionHost.selected.value.engineSource.value
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
                            queueCurrentDraft()
                            val prompt = link.payload["prompt"] as? String ?: ""
                            val intent = nextNavigationIntent()
                            check(switchEngineScope(targetScope, projectSnapshotForScope(targetScope), null, true,
                                sessionModeOverride = mode, navigationIntent = intent) && intent == navigationSequence.get()) {
                                resources.getString(R.string.deep_link_busy)
                            }
                            setConversationMode(mode)
                            val owner = sessionHost.selected.value
                            val current = sessionHost.draftState(owner).value
                            val combined = if (prompt.isEmpty()) current else com.lingxi.code.conversation.mergeHandoffDraft(prompt, current)
                            publishOwnerDraft(owner, targetScope, owner.state.value.session.id, combined)
                            approvedDraftLink = null
                        }
                        "import" -> onDeepLinkImportReady(link)
                        "project" -> {
                            val project = (resolveLinkedScope(id) as? ConversationScope.Project)?.projectId
                                ?: projectState.projects.firstOrNull { it.record.id == id }?.record?.id
                                ?: error(resources.getString(R.string.deep_link_workspace_missing))
                            projectPageId = project
                        }
                        "market" -> {
                            appPageSequence.incrementAndGet()
                            projectsPageOpen = false; projectPageId = null; sessionPanelOpen = false
                            appsPageOpen = true; runtimeAppId = null; localAppDetailsId = null; marketInstallId = id
                        }
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
                runCatching { projectStore.importSaf(name, uri, activeSessionMode) }
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
        catalog.copy(rows = catalog.rows.filter { it.uuid !in scheduledIds && it.mode == activeSessionMode })
    }
    val newCronResults = cronState.generatedSessions.count { run ->
        run.sessionId != null && (run.finishedAtMs ?: 0L) > cronResultsSeenAt
    }
    fun markCronResultsSeen() {
        val now = System.currentTimeMillis()
        cronResultsSeenAt = now
        queueScopeStateWrite { scopeStore.persistCronResultsSeenAt(now) }
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
    val newChatScope = sourceScope
    val liveWorkspace = currentWorkspace
    val workspaceStorageHealthy = scopeStateValid == true && !projectState.loading && !projectState.storageUnavailable
    val verifiedWorkspaceProjects = if (workspaceStorageHealthy) projectState.projects else emptyList()
    val verifiedScheduledWorkspace = remember(context, sourceScope, state.session.id, currentEngineSource, liveWorkspace) {
        if (sourceScope == ConversationScope.Scheduled && liveWorkspace != null &&
            currentEngineSource.recoverySpec?.workspaceKey == "scheduled" &&
            canonicalSessionId(liveWorkspace.sessionId) == canonicalSessionId(state.session.id) &&
            currentEngineSource.activeSessionState.value?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(state.session.id)) {
            runCatching {
                com.lingxi.code.project.MobileUserStorage.verifiedScheduledWorkspace(context, canonicalSessionId(state.session.id))
            }.getOrNull()?.takeIf { it.guestPath == liveWorkspace.projectRoot }
        } else null
    }
    fun openCurrentOwnerTerminal(sessionId: String, command: String?) {
        val owner = chatViewModel.state.value.session.id
        val source = chatViewModel.engineSource.value
        val workspace = source.workspaceState.value
        if (source !== currentEngineSource || owner.isBlank() || owner == "new" || workspace == null ||
            canonicalSessionId(workspace.sessionId) != canonicalSessionId(owner) || workspace.cwd.isBlank() ||
            (sessionId != "interactive" && canonicalSessionId(sessionId) != canonicalSessionId(owner))) {
            chatViewModel.reportHostError(resources.getString(R.string.workspace_session_changed))
            return
        }
        onOpenTerminal(sessionId, command, workspace.cwd, "conversation:${canonicalSessionId(owner)}")
    }
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
    fun openProjectSession(projectId: String, selected: com.lingxi.code.model.ProjectSession) {
        val project = projectState.projects.firstOrNull { it.record.id == projectId } ?: return
        val resumeTarget = SessionRef(selected.id, selected.title)
        val intent = nextNavigationIntent()
        scope.launch {
            if (switchEngineScope(project = project, target = resumeTarget, newSession = false,
                    sessionModeOverride = selected.mode, navigationIntent = intent,
                    selectedTranscriptPath = selected.transcriptPath, requireSelectedTranscript = true)) {
                closeDrawer()
                closeProjectPages()
            }
        }
    }

    /**
     * Start an independent conversation in [target] and [mode]. [carryDraft]
     * moves what the user already typed into the new conversation.
     */
    fun startNewSessionIn(
        target: ConversationScope,
        mode: SessionMode = activeSessionMode,
        carryDraft: Boolean = false,
    ) {
        val project = projectSnapshotForScope(target)
        if (target is ConversationScope.Project && project == null) return
        val carried = draft
        val intent = nextNavigationIntent()
        scope.launch {
            queueCurrentDraft()
            if (switchEngineScope(engineScope = target, project = project, target = null,
                    newSession = true, sessionModeOverride = mode, navigationIntent = intent) && intent == navigationSequence.get()) {
                if (carryDraft) {
                    val owner = sessionHost.selected.value
                    val current = sessionHost.draftState(owner).value
                    val combined = if (carried.isEmpty()) current else com.lingxi.code.conversation.mergeHandoffDraft(carried, current)
                    publishOwnerDraft(owner, target, owner.state.value.session.id, combined)
                }
                closeDrawer()
                closeProjectPages()
            }
        }
    }

    /** Start a new conversation in a project, in the current or explicitly selected mode. */
    fun startProjectSession(projectId: String, mode: SessionMode = activeSessionMode) =
        startNewSessionIn(ConversationScope.Project(projectId), mode)

    fun authoringEntryMode(): SessionMode? {
        val owner = chatViewModel.state.value
        val source = chatViewModel.engineSource.value
        val activation = source.activeSessionState.value
        val confirmed = owner.sessionReady && !owner.sessionTransitioning && activation != null &&
            activation.mode == owner.sessionMode && canonicalSessionId(activation.sessionId) == canonicalSessionId(owner.session.id)
        return if (com.lingxi.code.apps.authoringEntryAllowed(activeSessionMode, owner.sessionMode,
                source.recoverySpec?.sessionMode, confirmed)) owner.sessionMode else null
    }

    fun captureAppAuthoringOperation(pageIsVisible: () -> Boolean): com.lingxi.code.apps.AppAuthoringOperationHost? {
        if (authoringEntryMode() == null) return null
        val owner = chatViewModel
        val navigation = navigationSequence.get()
        val pageNavigation = appPageSequence.get()
        val tabRevision = tabState?.selectionRevision
        return object : com.lingxi.code.apps.AppAuthoringOperationHost {
            override fun <T> launch(work: suspend () -> T, complete: (Result<T>, Boolean) -> Unit) {
                sessionHost.launchAuthoringOperation(work,
                    isCurrent = { rootCompositionAlive.get() && navigation == navigationSequence.get() && pageNavigation == appPageSequence.get() &&
                        sessionHost.selected.value === owner &&
                        (tabState == null || (tabState.selected == com.lingxi.code.tabs.AppTab.Chats && tabRevision == tabState.selectionRevision)) &&
                        pageIsVisible() },
                    hiddenFailure = { failure -> owner.reportHostError(if (com.lingxi.code.apps.isAppDevelopmentCodeRequired(failure))
                        resources.getString(R.string.local_app_development_requires_code) else failure.message ?: "App operation failed") }, complete = complete)
            }
        }
    }

    fun openAppAuthoring(appId: String?, name: String, createdDirectory: com.lingxi.code.apps.AuthoringDirectory?,
        recoverySelection: com.lingxi.code.apps.AuthoringIntent? = null,
        recoveryRequest: com.lingxi.code.apps.AuthoringRequest? = null,
        sourceOperation: String? = null, sourceRevision: Long? = null) {
        val entryMode = authoringEntryMode() ?: run {
            chatViewModel.reportHostError(resources.getString(R.string.local_app_development_requires_code))
            return
        }
        val intent = nextNavigationIntent()
        val tabRevision = tabState?.selectionRevision
        fun presentationIsCurrent() = intent == navigationSequence.get() &&
            (tabState == null || (tabState.selected == com.lingxi.code.tabs.AppTab.Chats && tabRevision == tabState.selectionRevision))
        scope.launch {
            var activatedOwner: ChatViewModel? = null
            var activatedRetained = false
            try {
                val intentStore = com.lingxi.code.apps.AuthoringIntentStore(appContext)
                val newOperation = sourceOperation ?: if (appId == null && createdDirectory == null && recoverySelection == null) "create" else null
                val sourceRequest = recoveryRequest ?: newOperation?.let { operation ->
                    com.lingxi.code.apps.AuthoringRequest(java.util.UUID.randomUUID().toString(), operation, name,
                        java.util.UUID.randomUUID().toString(), java.util.UUID.randomUUID().toString(),
                        appId.takeUnless { operation == "create" }, revision = sourceRevision).also {
                        intentStore.saveRequest(it, entryMode)
                        projectStore.createAuthoringPlaceholder(it.name, SessionMode.Code, it.projectId, it.selectionId)
                    }
                }
                var generatedApp: com.lingxi.code.apps.AppRecord? = null
                var generated = sourceRequest?.let { request ->
                    if (!presentationIsCurrent()) return@launch
                    check(request.operation in setOf("create", "restore", "remix", "bind") &&
                        projectStore.state.value.projects.any { it.record.id == request.projectId && it.record.authoringIntentId == request.selectionId }) {
                        "应用源码选择已失效。"
                    }
                    com.lingxi.code.apps.awaitAdmittedAuthoringSource(
                        submit = {
                            when (request.operation) {
                                "create" -> localApps.createAuthoringDirectory(request.name, request.selectionId, entryMode)
                                "bind" -> localApps.bindAuthoringSource(request, entryMode)
                                else -> localApps.restoreAuthoringDirectory(request, entryMode).let { restored ->
                                    generatedApp = restored.record
                                    restored.directory
                                }
                            }
                        }, navigationIsCurrent = { presentationIsCurrent() },
                    ) ?: return@launch
                }
                val effectiveAppId = com.lingxi.code.apps.authoringResultAppId(generatedApp, appId)
                val appRecord = if (recoverySelection == null && sourceRequest == null) effectiveAppId?.let { localApps.get(it) } else null
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
                val resolution = if (recoverySelection == null && sourceRequest == null) effectiveAppId?.let { localApps.prepareAuthoring(it, sessionBindings) } else null
                check(resolution?.action != "sourceMissing") { "源码目录已不可用，请从应用管理中恢复源码或创建获许可的副本。" }
                var effectiveRequest = sourceRequest
                if (resolution != null) {
                    val preparedSource = checkNotNull(resolution.sourceId)
                    val directory = resolution.sourceDirectory
                    val host = checkNotNull(resolution.sourcePath)
                    val existing = boundProject ?: projectStore.state.value.projects.firstOrNull { it.record.directory == directory }
                    val roots = com.lingxi.code.project.MobileUserStorage.directories(appContext).projectGrantRoots()
                    check(host == localApps.hostFile(directory).absolutePath && isCanonicalUuidValue(preparedSource) &&
                        com.lingxi.code.project.isSafeProjectWorkspace(roots, directory, host, existing, preparedAppDirectory = directory)) {
                        "应用源码身份不匹配。"
                    }
                    val projectId = existing?.record?.id ?: java.util.UUID.randomUUID().toString()
                    val workspaceId = oldBinding?.workspaceId?.takeIf { boundProject?.record?.id == projectId && oldBinding.sourceDirectory == directory }
                        ?: java.util.UUID.randomUUID().toString()
                    val targetRow = resolution.authoring?.authoringSessionId?.takeIf { resolution.action == "resumeSession" }
                        ?.let { id -> existing?.sessions?.firstOrNull { canonicalSessionId(it.sessionId) == canonicalSessionId(id) && it.mode == SessionMode.Code } }
                    if (resolution.action == "resumeSession") check(targetRow?.transcriptPath?.startsWith('/') == true)
                    val request = com.lingxi.code.apps.AuthoringRequest(java.util.UUID.randomUUID().toString(), "bind", name,
                        projectId, workspaceId, checkNotNull(effectiveAppId), sourceId = preparedSource, sourceDirectory = directory,
                        targetSessionId = targetRow?.sessionId?.let(::canonicalSessionId), targetTranscriptPath = targetRow?.transcriptPath)
                    intentStore.saveRequest(request, entryMode)
                    if (existing == null) projectStore.createAuthoringPlaceholder(name, SessionMode.Code, projectId, request.selectionId)
                    else projectStore.registerAuthoringWorkspace(name, directory, SessionMode.Code, projectId, request.selectionId) {
                        presentationIsCurrent()
                    }
                    check(presentationIsCurrent()) { "应用源码选择已失效。" }
                    generated = com.lingxi.code.apps.awaitAdmittedAuthoringSource(
                        submit = { localApps.bindAuthoringSource(request, entryMode) },
                        navigationIsCurrent = { presentationIsCurrent() },
                    ) ?: return@launch
                    effectiveRequest = request
                }
                val selectedDirectory = checkNotNull(generated ?: createdDirectory)
                val directory = resolution?.sourceDirectory ?: selectedDirectory.path
                val sourceId = resolution?.sourceId ?: selectedDirectory.sourceId
                val sourceRequestId = selectedDirectory.requestId
                val nativeSourcePath = resolution?.sourcePath ?: selectedDirectory.sourcePath
                check(selectedDirectory.path == directory && selectedDirectory.sourceId == sourceId &&
                    selectedDirectory.sourcePath == nativeSourcePath) { "应用源码身份已变化。" }
                check(!projectStore.state.value.storageUnavailable && presentationIsCurrent()) {
                    "应用源码选择已失效。"
                }
                val roots = com.lingxi.code.project.MobileUserStorage.directories(appContext).projectGrantRoots()
                val existingProject = effectiveRequest?.let { request -> projectStore.state.value.projects.firstOrNull { it.record.id == request.projectId } }
                    ?: recoverySelection?.let { selection -> projectStore.state.value.projects.firstOrNull {
                        it.record.id == selection.projectId && it.record.authoringIntentId == selection.selectionId
                    } } ?: boundProject ?: projectStore.state.value.projects.firstOrNull { it.record.directory == directory }
                if (recoverySelection != null) check(existingProject?.record?.authoringIntentId == recoverySelection.selectionId) {
                    "应用源码选择已变化。"
                }
                val hostPath = localApps.hostFile(directory).absolutePath
                check(nativeSourcePath == hostPath && isCanonicalUuidValue(sourceId) && isCanonicalUuidValue(sourceRequestId)) { "应用源码身份不匹配。" }
                check(com.lingxi.code.project.isSafeProjectWorkspace(roots, directory, hostPath, existingProject,
                    preparedAppDirectory = directory)) { "应用源码目录不可用或未获授权。" }
                val projectId = recoverySelection?.projectId ?: effectiveRequest?.projectId ?: existingProject?.record?.id ?: java.util.UUID.randomUUID().toString()
                val workspaceId = recoverySelection?.workspaceId ?: effectiveRequest?.workspaceId ?: oldBinding?.workspaceId
                    ?.takeIf { boundProject?.record?.id == projectId && oldBinding.sourceDirectory == directory }
                    ?: java.util.UUID.randomUUID().toString()
                val targetSessionId = recoverySelection?.sessionId ?: effectiveRequest?.targetSessionId ?: resolution?.authoring?.authoringSessionId
                    ?.takeIf { resolution.action == "resumeSession" && boundProject?.record?.id == projectId }
                val previousSelection = targetSessionId?.let { intentStore.read(canonicalSessionId(it)) }
                if (previousSelection != null) check(previousSelection.sourceId == sourceId && previousSelection.projectId == projectId &&
                    previousSelection.sourceDirectory == directory &&
                    previousSelection.workspaceId == workspaceId) { "应用源码会话身份已变化。" }
                val selection = recoverySelection ?: previousSelection ?: com.lingxi.code.apps.AuthoringIntent(
                    effectiveRequest?.selectionId ?: java.util.UUID.randomUUID().toString(), sourceId, sourceRequestId, directory, projectId, workspaceId, effectiveAppId)
                check(selection.sourceId == sourceId && (previousSelection != null || selection.sourceRequestId == sourceRequestId) &&
                    selection.sourceDirectory == directory && selection.projectId == projectId &&
                    selection.workspaceId == workspaceId) { "应用源码选择身份不匹配。" }
                if (recoverySelection == null && previousSelection == null) intentStore.savePrepared(selection, entryMode)
                // A rebind may reuse an existing owner while its new request still needs recovery.
                val projectSelectionId = effectiveRequest?.selectionId ?: selection.selectionId
                val ownedProject = projectStore.registerAuthoringWorkspace(name, directory, SessionMode.Code, projectId, projectSelectionId) {
                    presentationIsCurrent() && (recoverySelection == null || projectStore.state.value.projects.any {
                        it.record.id == projectId && it.record.authoringIntentId == selection.selectionId
                    })
                }
                val project = ownedProject.copy(workspace = com.lingxi.code.project.ProjectWorkspace(ownedProject.record.id, hostPath, directory))
                val sourceIdentity = com.lingxi.code.project.AuthoringSourceIdentity(effectiveAppId, project.record.id, hostPath, directory)
                fun freshAuthoringGrant() = com.lingxi.code.project.PreparedAuthoringGrant(sourceIdentity, intent)
                val target = targetSessionId?.let { id -> project.sessions.firstOrNull { row ->
                    canonicalSessionId(row.sessionId) == canonicalSessionId(id) && row.mode == SessionMode.Code &&
                        (row.workspaceDirectory ?: project.record.activeDirectory) == directory
                } }
                check(targetSessionId == null || target != null) { "应用源码会话目录已变化，请重新打开。" }
                // App authoring explicitly creates or resumes a Code session.
                val mode = SessionMode.Code
                queueCurrentDraft()
                val authoringScope = ConversationScope.Project(project.record.id)
                val requestedTranscript = effectiveRequest?.targetTranscriptPath ?: recoverySelection?.transcriptPath
                fun captureAuthoringOwner(owner: ChatViewModel, fresh: Boolean) {
                    activatedOwner = owner
                    val ownership = sessionHost.draftOwnership(owner)
                    val ownerId = canonicalSessionId(owner.state.value.session.id)
                    val sameOwner = sameDraftOwner(ownership.scope.value, ownership.sessionId.value, authoringScope, ownerId)
                    if ((ownership.loaded.value && sameOwner) ||
                        (fresh && !ownership.loaded.value && ownership.revision.longValue == 0L &&
                            sessionHost.draftState(owner).value.isEmpty() && sessionHost.attachmentState(owner).value == null)) {
                        val current = sessionHost.draftState(owner).value
                        publishOwnerDraft(owner, authoringScope, ownerId, LocalAppAuthoringPrompt.compose(name, effectiveAppId, current))
                    }
                }
                check(switchEngineScope(authoringScope, project,
                    target?.let { SessionRef(it.sessionId, it.title) }, target == null,
                    sessionModeOverride = mode,
                    navigationIntent = intent, presentationIsCurrent = ::presentationIsCurrent, selectedTranscriptPath = requestedTranscript,
                    requireSelectedTranscript = requestedTranscript != null,
                    preparedAuthoringGrant = freshAuthoringGrant(), authoringIntent = selection,
                    onActivated = { owner, retained ->
                        activatedRetained = retained
                        captureAuthoringOwner(owner, target == null)
                    })) { "无法切换到应用源码会话。" }
                val authoringVm = checkNotNull(activatedOwner)
                val authoringSource = authoringVm.engineSource.value
                val firstRegistration = java.util.concurrent.atomic.AtomicBoolean(recoverySelection == null && recoveryRequest == null)
                val bindOwner: suspend () -> Unit = {
                    check(authoringVm.engineSource.value === authoringSource && authoringVm.sourceScope.value == authoringScope)
                    val confirmed = authoringVm.awaitAppAuthoringSession(directory, target?.sessionId)
                    val ownerMode = checkNotNull(authoringVm.state.value.sessionMode)
                    val activation = checkNotNull(authoringSource.activeSessionState.value)
                    check(ownerMode == SessionMode.Code && activation.mode == ownerMode &&
                        canonicalSessionId(activation.sessionId) == canonicalSessionId(confirmed.sessionId) &&
                        (target == null || activatedRetained || activation.kind == com.lingxi.code.conversation.SessionActivationKind.Resumed) &&
                        authoringSource.recoverySpec?.sessionMode == ownerMode)
                    val savedIntent = checkNotNull(intentStore.readSelection(selection.selectionId))
                    check(savedIntent.sourceId == sourceId && savedIntent.projectId == project.record.id &&
                        savedIntent.sourceDirectory == directory && savedIntent.workspaceId == workspaceId &&
                        savedIntent.sessionId == canonicalSessionId(confirmed.sessionId) && savedIntent.sessionMode == ownerMode &&
                        savedIntent.transcriptPath == confirmed.transcriptPath) { "应用源码会话来源已变化。" }
                    val authoringStillMatches = {
                        authoringVm.engineSource.value === authoringSource &&
                            authoringVm.sourceScope.value == authoringScope &&
                            authoringSource.activeSessionState.value?.let { active -> active.mode == ownerMode &&
                                canonicalSessionId(active.sessionId) == canonicalSessionId(confirmed.sessionId) &&
                                (target == null || activatedRetained || active.kind == com.lingxi.code.conversation.SessionActivationKind.Resumed) } == true &&
                            authoringSource.workspaceState.value == confirmed &&
                            canonicalSessionId(authoringVm.state.value.session.id) == canonicalSessionId(confirmed.sessionId)
                    }
                    check(authoringStillMatches()) { "开发会话已切换，请重新打开应用源码。" }
                    check(projectStore.recordStartedSession(project.record.id, confirmed.sessionId, name, mode,
                        transcriptPath = confirmed.transcriptPath, allowCommit = authoringStillMatches,
                        markActive = { sessionHost.selected.value === authoringVm }))
                    projectStore.recordWorkspace(project.record.id, confirmed, allowCommit = authoringStillMatches,
                        updateProjectDirectory = { sessionHost.selected.value === authoringVm })
                    check(authoringStillMatches()) { "开发目录已变化。" }
                    val authoring = org.json.JSONObject()
                        .put("workspaceId", workspaceId)
                        .put("sourceDirectory", directory)
                        .put("projectRef", AppProjectRef("mobileProject", projectId = project.record.id).toJson())
                        .put("authoringSessionId", canonicalSessionId(confirmed.sessionId))
                    effectiveAppId?.let { authoring.put("appId", it) }
                    kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                        localApps.withAuthoringCurrentLock(effectiveAppId) {
                            com.lingxi.code.apps.registerAuthoringWithReceipt(
                                submit = {
                                    val setCurrent = firstRegistration.getAndSet(false) && presentationIsCurrent()
                                    localApps.registerAuthoring(sourceId, sourceRequestId, setCurrent, ownerMode, authoring); Unit
                                },
                                lookup = { localApps.getAuthoringGrant(directory, project.record.id, canonicalSessionId(confirmed.sessionId), ownerMode) },
                                isNotFound = { it is com.lingxi.code.apps.AppServiceException && it.code == "notFound" },
                                matches = { saved -> saved.matchesOwner(sourceId, directory, project.record.id, workspaceId,
                                    canonicalSessionId(confirmed.sessionId)) },
                                lookupBeforeSubmit = !firstRegistration.get(),
                                receiptProvesSubmission = effectiveRequest?.operation != "bind",
                            )
                        }
                        check(authoringStillMatches()) { "开发目录或会话已变化。" }
                        intentStore.markBindingConfirmed(selection.selectionId, canonicalSessionId(confirmed.sessionId))
                        effectiveRequest?.let { intentStore.markRequestCompleted(it.selectionId) }
                        check(authoringStillMatches()) { "开发目录或会话已变化。" }
                        if (!authoringSource.turnAdmissionReady) {
                            val admitted = when (authoringSource) {
                                is EngineConversationSource -> authoringSource.acknowledgeAuthoringBinding(sourceIdentity, confirmed.sessionId)
                                is com.lingxi.code.conversation.RecoveringConversationSource ->
                                    authoringSource.acknowledgeAuthoringBinding(sourceIdentity, confirmed.sessionId)
                                else -> false
                            }
                            check(admitted) { "应用源码会话绑定与来源不匹配。" }
                        }
                    }
                }
                authoringVm.installAuthoringRetry(bindOwner)
                try {
                    bindOwner()
                    authoringVm.finishAuthoringBinding()
                } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
                catch (failure: Exception) {
                    authoringVm.reportAuthoringBindingFailure(failure)
                }
                if (sessionHost.selected.value === authoringVm && presentationIsCurrent()) {
                    appsPageOpen = false; marketInstallId = null; localAppDetailsId = null; runtimeAppId = null
                    closeDrawer(); closeProjectPages()
                }
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (error: Exception) {
                val message = when {
                    error is com.lingxi.code.apps.InvalidAuthoringNameException -> resources.getString(R.string.local_app_authoring_name_invalid)
                    com.lingxi.code.apps.isAppDevelopmentCodeRequired(error) -> resources.getString(R.string.local_app_development_requires_code)
                    else -> error.message ?: "无法打开应用源码会话。"
                }
                (activatedOwner ?: chatViewModel).reportHostError(message)
            }
        }
    }

    fun recoverProjectAuthoring(projectId: String, selectionId: String) {
        val entryMode = authoringEntryMode() ?: run {
            chatViewModel.reportHostError(resources.getString(R.string.local_app_development_requires_code))
            return
        }
        val recoveryNavigation = nextNavigationIntent()
        scope.launch {
            try {
                fun currentProject() = projectStore.state.value.projects.singleOrNull {
                    !projectStore.state.value.storageUnavailable && it.record.id == projectId && it.record.authoringIntentId == selectionId
                }
                var project = checkNotNull(currentProject()) { "应用源码选择已变化。" }
                val intents = com.lingxi.code.apps.AuthoringIntentStore(appContext)
                val selection = intents.readSelection(selectionId)
                if (selection == null) {
                    val request = checkNotNull(intents.readRequest(selectionId)) { "应用源码请求不可用。" }
                    check(request.projectId == projectId && recoveryNavigation == navigationSequence.get())
                    openAppAuthoring(request.appId, request.name, null, recoveryRequest = request)
                    return@launch
                }
                check(selection.projectId == projectId)
                if (selection.sessionId != null) {
                    if (project.sessions.none { canonicalSessionId(it.sessionId) == selection.sessionId }) {
                        projectStore.recoverStagedAuthoringIntents()
                        project = checkNotNull(currentProject())
                    }
                    check(project.sessions.any { canonicalSessionId(it.sessionId) == selection.sessionId &&
                        it.mode == SessionMode.Code && it.transcriptPath == selection.transcriptPath }) { "应用源码会话不可用。" }
                }
                val proof = localApps.prepareAuthoringSource(selection.sourceId, selection.sourceDirectory, selection.sourceRequestId, entryMode)
                check(currentProject() != null && recoveryNavigation == navigationSequence.get() &&
                    proof.path == selection.sourceDirectory && proof.sourceId == selection.sourceId &&
                    proof.requestId == selection.sourceRequestId &&
                    proof.sourcePath == localApps.hostFile(selection.sourceDirectory).absolutePath) { "应用源码身份已变化。" }
                openAppAuthoring(selection.appId, project.record.name, proof, recoverySelection = selection)
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (failure: Exception) { chatViewModel.reportHostError(if (com.lingxi.code.apps.isAppDevelopmentCodeRequired(failure))
                resources.getString(R.string.local_app_development_requires_code) else failure.message ?: "无法恢复应用源码。") }
        }
    }

    // The shell's bar rule: pages pushed over the chat, the phone drawer being open or opening, and
    // full-screen overlays (a running app, the voice flow) that cover everything.
    var drawerOpenOrOpening by remember { mutableStateOf(false) }
    val pushedPages = listOf(
        projectsPageOpen, projectPageId != null, appsPageOpen, marketInstallId != null,
        localAppDetailsId != null, runtimeAppId != null, cronPage != null,
    ).count { it }
    val latestOnChromeChanged by rememberUpdatedState(onChromeChanged)
    LaunchedEffect(pushedPages, drawerOpenOrOpening, runtimeAppId != null, voiceActive) {
        latestOnChromeChanged(pushedPages, drawerOpenOrOpening, runtimeAppId != null || voiceActive)
    }
    // Tapping the Chats tab again with pages pushed (the wide rail stays reachable) pops back to the chat.
    OnTabReselected(AppTab.Chats) { pushed ->
        if (pushed > 0) {
            projectsPageOpen = false; projectPageId = null; appsPageOpen = false; marketInstallId = null
            localAppDetailsId = null; runtimeAppId = null; cronPage = null
        }
    }

    Box(modifier = modifier.fillMaxSize()) {
        AdaptiveConversationDrawer(
            modifier = Modifier.fillMaxSize(),
            drawerState = drawerState,
            scrimColor = Color.Black.copy(alpha = 0.4f),
            onOpenOrOpeningChange = { drawerOpenOrOpening = it },
            drawerContent = {
                ModalDrawerSheet(
                    drawerContainerColor = LingXiTheme.palette.sidebarBg,
                    drawerTonalElevation = 0.dp,
                    modifier = Modifier.width(320.dp),
                ) {
                    DrawerContent(
                        ui = drawerUi,
                        onSelectSession = selectSession@ { ref ->
                            val row = globalDrawerSessions.rows.firstOrNull { it.uuid == ref.id } ?: return@selectSession
                            val targetScope = if (scheduledSessionRows.any { it.uuid == ref.id }) ConversationScope.Scheduled else ConversationScope.Global
                            scope.launch {
                                if (switchEngineScope(targetScope, null, ref, false,
                                        sessionModeOverride = row.mode)) closeDrawer()
                            }
                        },
                        onOpenAccount = {
                            closeDrawer()
                            onOpenAccount()
                        },
                        onOpenTerminal = {
                            closeDrawer()
                            openCurrentOwnerTerminal("interactive", null)
                        },
                        onClose = { closeDrawer() },
                        engineSessions = drawerEngineSessions,
                        onResumeSession = { uuid ->
                            globalDrawerSessions.rows.firstOrNull { it.uuid == uuid }?.let { row ->
                                val targetScope = if (scheduledSessionRows.any { it.uuid == uuid }) ConversationScope.Scheduled else ConversationScope.Global
                                scope.launch {
                                    if (switchEngineScope(targetScope, null, SessionRef(row.uuid, row.title), false,
                                            sessionModeOverride = row.mode)) closeDrawer()
                                }
                            }
                        },
                        onContinueSession = { sessionScope, row, targetMode ->
                            launchHandoff(sessionScope, row, targetMode)
                        },
                        onToggleSessionPinned = { sessionId ->
                            drawerUi.toggleSessionPinned(sessionId)
                            queueScopeStateWrite { scopeStore.persistSessionPinned(sessionId, drawerUi.sessionPinnedAt(sessionId)) }
                        },
                        productionData = drawerProductionData,
                        onOpenRecentSession = { row ->
                            val intent = nextNavigationIntent()
                            val targetScope = row.scope
                            val project = projectSnapshotForScope(targetScope)
                            scope.launch {
                                if (switchEngineScope(targetScope, project, SessionRef(row.uuid, row.title), false,
                                        sessionModeOverride = row.mode, navigationIntent = intent,
                                        selectedTranscriptPath = row.transcriptPath, requireSelectedTranscript = true)) {
                                    closeDrawer()
                                    closeProjectPages()
                                }
                            }
                        },
                        onOpenCron = { taskKey ->
                            markCronResultsSeen()
                            closeDrawer()
                            cronPage = CronPageRoute.task(taskKey)
                        },
                        onOpenCronList = {
                            markCronResultsSeen()
                            closeDrawer()
                            cronPage = CronPageRoute.Tasks
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
                            appPageSequence.incrementAndGet()
                            appsPageOpen = true
                        },
                        onOpenApp = { appId ->
                            closeDrawer()
                            appPageSequence.incrementAndGet()
                            runtimeAppId = appId
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
                    onRetryAuthoring = if (authoringRetryAvailable) chatViewModel::retryAuthoringBinding else null,
                    onSend = { text -> submitComposer(text, null) },
                    onSendPreflight = { text, attachment ->
                        if (activeProjectSyncing) {
                            chatViewModel.reportHostError(resources.getString(R.string.project_sync_in_progress_notice))
                            false
                        } else chatViewModel.admitComposerSend(text, attachment != null)
                    },
                    onSendWithAttachment = ::submitComposer,
                    // "新对话": reset the local transcript immediately AND tell the
                    // engine to begin a new session (NewSession). For the mock the
                    // engine call is a no-op, so this still behaves like newChat.
                    onNewChat = {
                        val (targetScope, targetMode) = com.lingxi.code.model.defaultNewChatTarget(activeSessionMode)
                        startNewSessionIn(targetScope, targetMode)
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
                    modeSelectionEnabled = !state.sessionTransitioning,
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
                        if (!flowActive && !chatViewModel.engineSource.value.turnAdmissionReady) {
                            chatViewModel.reportHostError(resources.getString(R.string.chat_turn_preparing_retry))
                            return@ChatScreen
                        }
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
                        if (flowActive) {
                            if (viewModel == null) sessionHost.yieldOtherOwnersAutomaticSpeech(chatViewModel)
                            voiceSpeechPlayer.stop()
                        }
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
                            assistantText = orbAssistantText,
                            onSend = {
                                text ->
                                val owner = chatViewModel
                                val ownerScope = sourceScope
                                val ownerId = canonicalSessionId(state.session.id)
                                val ownership = draftOwnership
                                val controller = flowVoiceController
                                val voiceGeneration = controller.submissionGeneration()
                                val rejected = com.lingxi.code.conversation.RejectedComposerDraft(
                                    text, null, state.visualizationChip, ownerScope, ownerId,
                                )
                                val gate = com.lingxi.code.conversation.ComposerRestoreGate(
                                    revision = { Triple(ownership.revision.longValue, ownership.attachmentRevision.longValue, owner.visualizationDraftRevision()) },
                                    preserve = { ownership.rejected.add(rejected) },
                                    restore = {
                                        if (owner.sourceScope.value == ownerScope && canonicalSessionId(owner.state.value.session.id) == ownerId &&
                                            ownership.scope.value == ownerScope && canonicalSessionId(ownership.sessionId.value.orEmpty()) == ownerId &&
                                            draftCell.value.isEmpty() && attachmentCell.value == null && owner.state.value.visualizationChip == null
                                        ) {
                                            if (viewModel == null) publishOwnerDraft(owner, ownerScope, ownerId, text)
                                            else {
                                                ownership.revision.longValue++
                                                ownership.loaded.value = true
                                                draftCell.value = text
                                                owner.onDraftChanged(text, ownerScope, ownerId)
                                            }
                                            owner.restoreVisualizationDraft(rejected.visualization)
                                        } else ownership.rejected.add(rejected)
                                    },
                                )
                                val fail = {
                                    gate.rejected()
                                    controller.rejectSubmission(voiceGeneration)
                                }
                                if (activeProjectSyncing) {
                                    owner.reportHostError(resources.getString(R.string.project_sync_in_progress_notice))
                                    fail()
                                } else {
                                    owner.send(text, origin = ConversationTurnOrigin.Flow, onNotAdmitted = fail)
                                }
                                gate.cleared()
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
                    hasUnsentDraft = draftOwnership.rejected.isNotEmpty(),
                    canRestoreUnsentDraft = draft.isEmpty() && attachment == null && state.visualizationChip == null &&
                        draftOwnership.rejected.firstOrNull()?.let { failed ->
                            failed.scope == sourceScope && canonicalSessionId(failed.sessionId.orEmpty()) == canonicalSessionId(state.session.id)
                        } == true,
                    onRestoreUnsentDraft = {
                        val failed = draftOwnership.rejected.firstOrNull()
                        if (failed != null && draft.isEmpty() && attachment == null && state.visualizationChip == null &&
                            failed.scope == sourceScope && canonicalSessionId(failed.sessionId.orEmpty()) == canonicalSessionId(state.session.id)
                        ) {
                            updateCurrentDraft(failed.text)
                            attachment = failed.attachment
                            draftOwnership.attachmentRevision.longValue++
                            chatViewModel.restoreVisualizationDraft(failed.visualization)
                            draftOwnership.rejected.removeAt(0)
                        }
                    },
                    onRemoveAttachment = {
                        attachment = null
                        draftOwnership.attachmentRevision.longValue++
                        pendingComposerRestore?.cleared()
                        pendingComposerRestore = null
                    },
                    onShare = onShare,
                    onStop = chatViewModel::cancel,
                    onDiscardRecoveredTurn = chatViewModel::discardRecoveredTurn,
                    onDismissError = chatViewModel::dismissError,
                    onDismissCommandResult = chatViewModel::dismissCommandResult,
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
                    modelAvailabilityNotice = providerAccountState.availabilityProblem?.let {
                        com.lingxi.code.settings.accountAvailabilityProblemLabel(it)
                    },
                    onOpenModelSettings = onOpenModelSettings,
                    modelProviderStatuses = modelProviderStatuses,
                    accountModelAvailability = providerAccountState.modelAvailability,
                    onOpenProviderSettings = onOpenProviderSettings,
                    computerUseSetup = computerUseSetup,
                    onOpenComputerUseSettings = onOpenComputerUseSettings,
                    onDismissComputerUseSetup = { computerUseSetupDismissed = true },
                    onOpenTerminal = { sessionId, command ->
                        openCurrentOwnerTerminal(sessionId, command)
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

        if (pendingHandoffRequest != null && handoffCanCancel) {
            Snackbar(
                modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp),
                action = { TextButton(onClick = { handoffJob?.cancel() }) { Text(stringResource(R.string.common_cancel)) } },
            ) { Text(stringResource(R.string.session_handoff_preparing)) }
        }

        // Immersive voice overlay (full-screen, above everything else).
        VoiceFlowOverlay(visible = voiceActive)

        com.lingxi.code.conversation.SideChatSheet(
            chatViewModel.sideChats, isDark, onCreate = chatViewModel::createSideChat,
            modelProviderStatuses = modelProviderStatuses,
            accountModelAvailability = providerAccountState.modelAvailability,
        )

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
                pinnedWorkspaces = drawerUi.pinnedWorkspaces.filterKeys { it.startsWith("${activeSessionMode.wireValue}:") }
                    .mapKeys { it.key.removePrefix("${activeSessionMode.wireValue}:") },
                statusMessage = drawerProductionData.projectStatusMessage,
                onBack = { projectsPageOpen = false },
                onOpenProject = { projectPageId = it },
                onTogglePinned = { projectId ->
                    val key = "project.$projectId"
                    drawerUi.toggleWorkspacePinned(activeSessionMode, key)
                    queueScopeStateWrite { scopeStore.persistWorkspacePinned("${activeSessionMode.wireValue}:$key", drawerUi.pinnedAt(activeSessionMode, key)) }
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
                        startProjectSession(shown.id, mode ?: activeSessionMode)
                    },
                    onOpenSession = { selected -> openProjectSession(shown.id, selected) },
                    onOpenCron = { taskKey ->
                        markCronResultsSeen()
                        closeProjectPages()
                        cronPage = CronPageRoute.task(taskKey)
                    },
                    onReimport = { runProjectSync(shown.id) { projectStore.reimport(shown.id) } },
                    onExport = { runProjectSync(shown.id) { projectStore.export(shown.id) } },
                    onReauthorize = {
                        runProjectSync(shown.id) {
                            pendingReauthorizeProjectId = shown.id
                            importProjectLauncher.launch(null)
                        }
                    },
                    onRecoverAuthoring = projectState.projects.firstOrNull { it.record.id == shown.id }
                        ?.record?.authoringIntentId?.let { selection -> { recoverProjectAuthoring(shown.id, selection) } },
                    canStartAuthoring = { authoringEntryMode() != null },
                )
            }
        }

        val authoringRecoveryProjects = projectState.projects.mapNotNull { project ->
            project.record.authoringIntentId?.let { selection ->
                com.lingxi.code.apps.PendingAuthoringRecovery(project.record.id, selection, project.record.name)
            }
        }
        var pendingAuthoringRecoveries by remember { mutableStateOf(emptyList<com.lingxi.code.apps.PendingAuthoringRecovery>()) }
        LaunchedEffect(appsPageOpen, projectState.storageUnavailable, authoringRecoveryProjects) {
            pendingAuthoringRecoveries = if (!appsPageOpen || projectState.storageUnavailable) emptyList()
            else com.lingxi.code.apps.pendingAuthoringRecoveries(authoringRecoveryProjects,
                com.lingxi.code.apps.AuthoringIntentStore(appContext))
        }
        AnimatedVisibility(
            visible = appsPageOpen,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            BackHandler { appPageSequence.incrementAndGet(); appsPageOpen = false }
            UnifiedAppsPage(
                apps = localApps,
                onBack = { appPageSequence.incrementAndGet(); appsPageOpen = false },
                onRemoteApps = { appPageSequence.incrementAndGet(); appsPageOpen = false; onOpenBotsTab() },
                onOpen = { appPageSequence.incrementAndGet(); runtimeAppId = it },
                onDetails = { appPageSequence.incrementAndGet(); localAppDetailsId = it },
                onAuthoring = { appId, name, directory -> openAppAuthoring(appId, name, directory) },
                pendingAuthoring = pendingAuthoringRecoveries,
                onRecoverAuthoring = { recoverProjectAuthoring(it.projectId, it.selectionId) },
                canStartAuthoring = { authoringEntryMode() != null },
                captureImportTicket = { captureAppAuthoringOperation { appsPageOpen }?.let(sessionHost::retainAuthoringPickerTicket) },
                takeImportTicket = sessionHost::takeAuthoringPickerTicket,
            )
        }
        marketInstallId?.let { remoteId ->
            BackHandler { marketInstallId = null }
            com.lingxi.code.apps.UnifiedMarketInstallPage(
                apps = localApps, marketId = remoteId,
                onBack = { marketInstallId = null },
                onInstalled = { localId -> appPageSequence.incrementAndGet(); marketInstallId = null; localAppDetailsId = localId },
            )
        }
        // Like the project page, these keep their last app while sliding out.
        var lastLocalAppDetailsId by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(localAppDetailsId) { if (localAppDetailsId != null) lastLocalAppDetailsId = localAppDetailsId }
        AnimatedVisibility(
            visible = localAppDetailsId != null,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            val shownId = localAppDetailsId ?: lastLocalAppDetailsId
            if (shownId != null) {
                BackHandler { appPageSequence.incrementAndGet(); localAppDetailsId = null }
                UnifiedAppDetailsPage(
                    apps = localApps,
                    appId = shownId,
                    onBack = { appPageSequence.incrementAndGet(); localAppDetailsId = null },
                    onOpen = { appPageSequence.incrementAndGet(); runtimeAppId = it },
                    onAuthoring = { appId, name, directory -> openAppAuthoring(appId, name, directory) },
                    onRestoreAuthoring = { appId, name, remix, revision ->
                        openAppAuthoring(appId, name, null, sourceOperation = if (remix) "remix" else "restore", sourceRevision = revision)
                    },
                    canStartAuthoring = { authoringEntryMode() != null },
                    captureOperation = { captureAppAuthoringOperation { localAppDetailsId == shownId } },
                    captureNavigation = {
                        val tabRevision = tabState?.selectionRevision
                        val pageRevision = appPageSequence.get()
                        val current = {
                            rootCompositionAlive.get() && pageRevision == appPageSequence.get() && localAppDetailsId == shownId &&
                                (tabState == null || (tabState.selected == com.lingxi.code.tabs.AppTab.Chats && tabRevision == tabState.selectionRevision))
                        }
                        current
                    },
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
                    onClose = { appPageSequence.incrementAndGet(); runtimeAppId = null },
                    onShowDetails = { appId ->
                        appPageSequence.incrementAndGet()
                        runtimeAppId = null
                        localAppDetailsId = appId
                    },
                )
            }
        }
        // 定时任务: drawn over the chat like the Apps page; it keeps its last route while sliding out.
        var lastCronPage by remember { mutableStateOf<CronPageRoute?>(null) }
        LaunchedEffect(cronPage) { if (cronPage != null) lastCronPage = cronPage }
        AnimatedVisibility(
            visible = cronPage != null,
            enter = slideInHorizontally(initialOffsetX = { it }),
            exit = slideOutHorizontally(targetOffsetX = { it }),
        ) {
            val shown = cronPage ?: lastCronPage
            if (shown != null) {
                CronPage(
                    route = shown,
                    engineSource = currentEngineSource,
                    projectStore = projectStore,
                    onOpenSession = { run -> cronPage = null; onOpenScheduledSession(run) },
                    onClose = { cronPage = null },
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
                transcriptPath = currentEngineSource.recoverySpec?.transcriptPath,
            )
            val otherMode = when (activeSessionMode) {
                SessionMode.Chat -> SessionMode.Code
                SessionMode.Code -> SessionMode.Chat
            }
            SessionPanel(
                model = SessionPanelModel(
                    title = state.session.title,
                    mode = activeSessionMode,
                    canSwitchMode = !state.sessionTransitioning,
                    workspaceName = currentWorkspaceName,
                    modelName = state.model.name,
                    effortLabel = state.reasoning?.let { effortLabel(it.selection) },
                    permissionLabel = permissionModeLabel(settingsState.effectivePermissionMode),
                    pinned = drawerUi.sessionPinnedAt(state.session.id) != null,
                    canArchive = sourceScope != ConversationScope.Scheduled,
                    handoffSourceMode = handoffReference?.sourceMode,
                    handoffSourceId = handoffReference?.sourceSessionId,
                ),
                onDismiss = { sessionPanelOpen = false },
                onSelectMode = ::selectSessionMode,
                // Fork remains an explicit, separate action.
                onContinueInOtherMode = {
                    sessionPanelOpen = false
                    launchHandoff(sourceScope, panelRow, otherMode)
                },
                onOpenHandoffSource = {
                    sessionPanelOpen = false
                    val reference = handoffReference
                    val targetScope = resolveLinkedScope(reference?.sourceScope)
                    if (reference != null && targetScope != null) {
                        val intent = nextNavigationIntent()
                        scope.launch {
                            if (!switchEngineScope(targetScope, projectSnapshotForScope(targetScope),
                                    SessionRef(reference.sourceSessionId, ""), false,
                                    sessionModeOverride = reference.sourceMode, navigationIntent = intent)) {
                                chatViewModel.reportConversationLaunchFailed()
                            }
                        }
                    } else chatViewModel.reportConversationLaunchFailed()
                },
                onOpenPermissionSettings = {
                    sessionPanelOpen = false
                    onOpenPermissionSettings()
                },
                onTogglePinned = {
                    sessionPanelOpen = false
                    val id = state.session.id
                    drawerUi.toggleSessionPinned(id)
                    queueScopeStateWrite { scopeStore.persistSessionPinned(id, drawerUi.sessionPinnedAt(id)) }
                },
                onMakeScheduled = {
                    sessionPanelOpen = false
                    cronPage = CronPageRoute.new(
                        state.messages.firstOrNull { it.role == com.lingxi.code.model.Role.User }
                            ?.text.orEmpty().take(500),
                    )
                },
                onOpenTerminal = {
                    sessionPanelOpen = false
                    openCurrentOwnerTerminal("interactive", null)
                },
                onArchive = {
                    sessionPanelOpen = false
                    val id = state.session.id
                    val projectId = (sourceScope as? ConversationScope.Project)?.projectId
                    val owner = chatViewModel
                    val mode = activeSessionMode
                    val intent = nextNavigationIntent()
                    scope.launch {
                        runCatching { projectStore.setSessionArchived(projectId, id, true) }
                            .onSuccess {
                                if (navigationSequence.get() == intent && (viewModel != null || sessionHost.selected.value === owner) &&
                                    canonicalSessionId(owner.state.value.session.id) == canonicalSessionId(id)
                                ) startNewSessionIn(ConversationScope.Global, mode)
                            }
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
            val defaultWorkspace = filePaths.globalGuestCwd(activeSessionMode)
            val verifiedAppSource = currentEngineSource.verifiedAuthoringSource().takeIf { workspaceStorageHealthy }
            val filesRepository = remember(filePaths, defaultWorkspace, verifiedWorkspaceProjects, workspaceStorageHealthy, verifiedScheduledWorkspace, verifiedAppSource) {
                filePaths.workspaceDirectoryRepository(defaultWorkspace, verifiedWorkspaceProjects,
                    verifiedScheduledWorkspace.takeIf { workspaceStorageHealthy }, verifiedAppSource)
            }
            val browserSource = remember { currentEngineSource }
            val browserWorkspace = remember { browserSource.workspaceState.value }
            val browserSession = browserWorkspace?.sessionId
            if (browserWorkspace == null || browserWorkspace.cwd.isBlank() || browserSession.isNullOrBlank() ||
                browserSource !== chatViewModel.engineSource.value ||
                canonicalSessionId(browserSession) != canonicalSessionId(chatViewModel.state.value.session.id)) {
                LaunchedEffect(browserSource, browserWorkspace) {
                    filesBrowserOpen = false
                    chatViewModel.reportHostError(resources.getString(R.string.workspace_session_changed))
                }
            } else WorkspaceFilesDialog(filesRepository, browserWorkspace.cwd,
                protectedPaths = {
                    projectState.projects.flatMap { project -> listOf(project.record.directory, project.record.activeDirectory,
                        project.record.directory.substringBeforeLast('/')) + project.record.additionalDirectories + project.sessions.mapNotNull { it.workspaceDirectory } } +
                        listOfNotNull(currentEngineSource.workspaceState.value?.cwd, currentEngineSource.workspaceState.value?.projectRoot) +
                        currentEngineSource.workspaceState.value?.additionalDirectories.orEmpty() + sessionState.rows.mapNotNull { it.workspaceDirectory }
                }, canMutate = { workspaceOwnerStoreMatches(scopeStateValid == true, projectStore.state.value, verifiedWorkspaceProjects) &&
                    !chatViewModel.state.value.requiresBackgroundExecution && !chatViewModel.state.value.sessionTransitioning &&
                    !chatViewModel.state.value.cancellationInFlight && chatViewModel.engineSource.value === browserSource &&
                    browserSource.workspaceState.value?.sessionId == browserSession &&
                    canonicalSessionId(chatViewModel.state.value.session.id) == canonicalSessionId(browserSession) },
                onDismiss = { filesBrowserOpen = false })
        }
        if (workspacePickerOpen) {
            val pickerSource = remember { currentEngineSource }
            val pickerSessionId = remember { pickerSource.workspaceState.value?.sessionId?.takeIf(String::isNotBlank) }
            suspend fun requirePickerGrant(path: String) {
                check(pickerSource.turnAdmissionReady && chatViewModel.engineSource.value === pickerSource &&
                    workspaceOwnerStoreMatches(scopeStateValid == true, projectStore.state.value, verifiedWorkspaceProjects)) {
                    context.getString(R.string.workspace_session_changed)
                }
                val ownerProject = (sourceScope as? ConversationScope.Project)?.let { scope ->
                    projectStore.state.value.projects.singleOrNull { it.record.id == scope.projectId }
                }
                val taskWorkspace = if (sourceScope == ConversationScope.Scheduled) {
                    kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                        pickerSessionId?.let { com.lingxi.code.project.MobileUserStorage.verifiedScheduledWorkspace(appContext, it) }
                    }
                } else null
                val roots = com.lingxi.code.project.MobileUserStorage.directories(appContext).projectGrantRoots()
                check(com.lingxi.code.project.scheduledPickerPathAllowed(sourceScope == ConversationScope.Scheduled, taskWorkspace, path) &&
                    com.lingxi.code.project.isSafeProjectWorkspace(roots, path, null, ownerProject,
                    pickerSource.verifiedAuthoringSource(), verifiedScheduled = taskWorkspace)) {
                    context.getString(R.string.workspace_browser_operation_failed)
                }
                check(chatViewModel.engineSource.value === pickerSource &&
                    workspaceOwnerStoreMatches(scopeStateValid == true, projectStore.state.value, verifiedWorkspaceProjects)) {
                    context.getString(R.string.workspace_session_changed)
                }
            }
            WorkspaceDirectorySheet(
                projects = drawerProductionData.projects.orEmpty(),
                currentScope = newChatScope,
                workspaceGuestPath = com.lingxi.code.project.MobileUserStorage.directories(context).globalGuestCwd(activeSessionMode),
                verifiedProjects = verifiedWorkspaceProjects,
                verifiedScheduled = verifiedScheduledWorkspace.takeIf { workspaceStorageHealthy },
                verifiedAppSource = pickerSource.verifiedAuthoringSource().takeIf { workspaceStorageHealthy },
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
                canMutate = { pickerSessionId != null &&
                    workspaceOwnerStoreMatches(scopeStateValid == true, projectStore.state.value, verifiedWorkspaceProjects) &&
                    !chatViewModel.state.value.requiresBackgroundExecution && !chatViewModel.state.value.sessionTransitioning && !chatViewModel.state.value.cancellationInFlight &&
                    chatViewModel.engineSource.value === pickerSource &&
                    pickerSource.workspaceState.value?.sessionId == pickerSessionId &&
                    canonicalSessionId(chatViewModel.state.value.session.id) == canonicalSessionId(pickerSessionId) },
                onPickDirectory = { path ->
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                    requirePickerGrant(path)
                    pickerSource.changeWorkspace(path, checkNotNull(pickerSessionId))
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                },
                onAddDirectory = { path ->
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                    requirePickerGrant(path)
                    pickerSource.addWorkspaceDirectory(path, checkNotNull(pickerSessionId))
                    check(chatViewModel.engineSource.value === pickerSource) { context.getString(R.string.workspace_session_changed) }
                },
                onPick = { target ->
                    workspacePickerOpen = false
                    if (target != newChatScope) startNewSessionIn(target, activeSessionMode, carryDraft = true)
                },
            )
        }
        CreateProjectDialog(
            visible = showCreateProject,
            onDismiss = { showCreateProject = false },
            onCreateInternal = { name ->
                showCreateProject = false
                scope.launch {
                    runCatching { projectStore.createInternal(name, activeSessionMode) }
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
            onRetry = if (projectState.storageUnavailable) projectStore::reload else null,
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
        browserAuthorized = if (state.sessionState == ComputerUseSessionState.Inactive) {
            configuredPackages.any { it in browserPackages }
        } else {
            state.grants.any { it.packageName in browserPackages }
        },
        sessionActive = state.sessionState in setOf(
            ComputerUseSessionState.Active,
            ComputerUseSessionState.AwaitingApproval,
        ),
        sessionStarting = state.sessionState == ComputerUseSessionState.Starting,
    )

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
    CronRunStatus.OutcomeUnknown -> context.getString(R.string.cron_status_outcome_unknown)
}

internal fun appendVoiceTranscript(base: String, transcript: String): String = when {
    transcript.isBlank() -> base
    base.isBlank() -> transcript
    else -> "$base $transcript"
}

@Composable
private fun ConversationStorageUnavailable(
    context: Context,
    modifier: Modifier,
    loading: Boolean,
    scopes: List<String>,
    onRetry: () -> Unit,
) {
    var filesOpen by remember { mutableStateOf(false) }
    androidx.compose.foundation.layout.Column(modifier = modifier.fillMaxSize().padding(24.dp)) {
        Text(if (loading) stringResource(R.string.remote_session_loading) else
            listOf(stringResource(R.string.conversation_state_unavailable), scopes.joinToString())
                .filter(String::isNotBlank).joinToString("\n"))
        if (!loading) {
            TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
            TextButton(onClick = { filesOpen = true }) { Text(stringResource(R.string.remote_files)) }
        }
    }
    if (filesOpen) {
        val paths = remember(context) { com.lingxi.code.project.MobileUserStorage.directories(context) }
        val repository = remember(paths) {
            paths.workspaceDirectoryRepository(paths.guestHome)
        }
        WorkspaceFilesDialog(repository, paths.guestHome, protectedPaths = { emptyList() },
            canMutate = { false }, onDismiss = { filesOpen = false })
    }
}

@Preview(showBackground = true)
@Composable
private fun RootScreenPreview() {
    LingXiTheme {
        RootScreen(isDark = true)
    }
}
