package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.createSavedStateHandle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.conversation.PermissionPromptDialog
import com.lingxi.code.computeruse.ComputerUseApprovalDialog
import com.lingxi.code.computeruse.ComputerUseFeatureProvider
import com.lingxi.code.connectivity.rememberOnlineState
import com.lingxi.code.connectivity.shouldShowOfflineBanner
import com.lingxi.code.drawer.DrawerContent
import com.lingxi.code.drawer.DrawerProductionData
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.model.SessionCatalogPhase
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.project.ConflictResolution
import com.lingxi.code.project.CreateProjectDialog
import com.lingxi.code.project.LocalProjectWorkspace
import com.lingxi.code.project.ProjectConflictDialog
import com.lingxi.code.project.ProjectErrorDialog
import com.lingxi.code.project.ProjectOperationKind
import com.lingxi.code.project.ProjectSnapshot
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.project.ProjectStoreState
import com.lingxi.code.project.toDrawerProject
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.offline.SherpaVoice
import android.Manifest
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import com.lingxi.code.share.rememberShare
import com.lingxi.code.vision.rememberCameraCapture
import com.lingxi.code.model.Role
import com.lingxi.code.voice.FlowModeOverlay
import com.lingxi.code.voice.VoiceFlowOverlay
import com.lingxi.code.voice.rememberOrbVoiceListen
import com.lingxi.code.voice.rememberVoiceCapture
import android.graphics.BitmapFactory
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.launch

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
fun RootScreen(
    isDark: Boolean,
    onToggleTheme: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenSettings: () -> Unit = {},
    onOpenModelSettings: () -> Unit = {},
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    modelSetupRequired: Boolean = false,
    // FlowMode (心流) profile bits, read from the persisted AppearancePrefs and
    // passed down so the voice-orb overlay can label itself + gate its text input.
    assistantName: String = "灵犀",
    inputDialog: Boolean = true,
    // The chosen offline voice-pack language ("zh"/"en"/""). When its sherpa pack
    // is downloaded, the orb uses on-device STT/TTS instead of the system voice.
    voiceLang: String = "",
    // Bumped from Settings → 重新连接引擎. The retained ChatViewModel replaces
    // its owned engine source so provider changes take effect without restart.
    reconnectToken: Int = 0,
    settingsStore: SettingsStore? = null,
    viewModel: ChatViewModel? = null,
) {
    val context = LocalContext.current
    val projectStore: ProjectStore = viewModel(
        key = "projects",
        factory = ProjectStore.factory(context),
    )
    val projectState by projectStore.state.collectAsState()

    // The Activity-scoped ViewModel owns the one native engine source. This is
    // important across configuration changes: Compose is recreated, while the
    // ViewModel and its live engine remain. Provider reconnects replace the
    // source inside that same ViewModel instead of accumulating keyed VMs.
    val appContext = context.applicationContext
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
    }
    val state by chatViewModel.state.collectAsState()
    DisposableEffect(chatViewModel, context) {
        ComputerUseFeatureProvider.attach(context) { chatViewModel.cancel() }
        onDispose { }
    }
    val computerUseApproval by ComputerUseFeatureProvider.pendingApproval.collectAsState()
    val pendingPermission by chatViewModel.pendingPermission.collectAsState()
    // The engine's REAL resumable-session catalog (out-of-band, sibling of the
    // model catalog). The drawer renders its loading / empty / error states
    // directly and never falls back to mock sessions.
    val sessionState by chatViewModel.sessions.collectAsState()
    val sourceProjectId by chatViewModel.sourceProjectId.collectAsState()
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    // Refresh the session catalog whenever the drawer transitions to open, so the
    // list is fresh each time the user reaches for it (the engine re-reports via
    // SessionList). `isOpen` flips on the open animation's start, so this fires
    // once per open, not per frame.
    LaunchedEffect(drawerState.isOpen) {
        if (drawerState.isOpen) chatViewModel.refreshSessions()
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

    // FlowMode orb voice driver: a one-shot tap-to-talk listener, plus the live
    // assistant reply text derived from the same conversation state ChatScreen
    // renders (the orb is just another view of the real session).
    // The orb's listen path: prefer the OFFLINE sherpa STT when a language pack
    // is downloaded AND mic permission is already granted; otherwise fall back to
    // the system SpeechRecognizer (which also drives the permission request).
    val orbSystemListen = rememberOrbVoiceListen()
    val orbListen: (onResult: (String?) -> Unit) -> Unit = { cb ->
        val canSherpa = voiceLang.isNotBlank() && SherpaVoice.sttReady(voiceLang) &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
        if (canSherpa) scope.launch { cb(SherpaVoice.transcribe(voiceLang)) } else orbSystemListen(cb)
    }
    val orbAssistantText = state.messages.lastOrNull()
        ?.let { if (it.role == Role.Ai) it.text else "" } ?: ""

    // Mirror the engine's REAL MCP listing into the activity-scoped SettingsStore
    // (the same instance SettingsHost renders). RefreshListings runs again after
    // provider reconnect; an empty reply remains an explicit empty catalog.
    val resolvedSettingsStore: SettingsStore =
        settingsStore ?: viewModel(factory = SettingsStore.factory(context))
    val settingsState by resolvedSettingsStore.state.collectAsState()
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
    // handle (see onDraftChange below) and `send` clears it.
    var draft by remember { mutableStateOf(chatViewModel.restoredDraft) }
    var voiceDraftBase by remember { mutableStateOf("") }

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

    suspend fun switchEngineScope(
        project: ProjectSnapshot?,
        target: SessionRef?,
        newSession: Boolean,
        replacePendingTransition: Boolean = false,
    ): Boolean {
        val destination = target ?: SessionRef("new", "新对话")
        var persisted: ProjectStoreState? = null
        return chatViewModel.switchWorkspaceSource(
            projectId = project?.record?.id,
            target = destination,
            newSession = newSession,
            replacePendingTransition = replacePendingTransition,
            createSource = {
                EngineConversationSource.create(
                    context = appContext,
                    projectWorkspace = project?.workspace,
                    linuxRuntimeMode = settingsState.linuxRuntime.selectedMode,
                )
            },
            persistSelection = {
                persisted = projectStore.persistActive(project?.record?.id)
            },
            onCommitted = {
                projectStore.publishActive(checkNotNull(persisted))
                drawerUi.selectSession(destination.id)
            },
        )
    }

    // Recover the last active Project after process start. The Activity-scoped
    // ChatViewModel survives rotation, so sourceProjectId prevents a needless
    // rebuild on configuration changes. A process-restored global Resume is
    // explicitly superseded only after the Project Source and active index are
    // both ready, so the wrong cwd never becomes authoritative.
    LaunchedEffect(
        projectState.loading,
        projectState.activeProjectId,
        sourceProjectId,
    ) {
        val project = projectState.activeProject
        if (
            !projectState.loading &&
                project != null &&
                sourceProjectId == null
        ) {
            val last = project.record.lastActiveSessionId
                ?.let { id -> project.sessions.firstOrNull { it.sessionId == id } }
                ?.let { SessionRef(it.sessionId, it.title) }
            switchEngineScope(
                project = project,
                target = last,
                newSession = last == null,
                replacePendingTransition = true,
            )
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
                sourceProjectId != null &&
                    state.isNew &&
                    (
                        state.session.id == "new" ||
                            sessionState.rows.none { it.uuid == state.session.id }
                        )
            if (!provisionalSessionMayNotBeListed) {
                runCatching { projectStore.syncEngineSessions(sourceProjectId, sessionState.rows) }
                    .onFailure { chatViewModel.reportHostError("会话索引保存失败：${it.message}") }
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
        val projectId = sourceProjectId
        if (
            projectId != null &&
            state.sessionReady &&
            state.isNew &&
            state.session.id != "new"
        ) {
            runCatching {
                projectStore.recordStartedSession(
                    projectId = projectId,
                    sessionId = state.session.id,
                    title = state.session.title,
                )
            }.onFailure {
                chatViewModel.reportHostError("新会话索引保存失败：${it.message}")
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

    val globalDrawerSessions = if (sourceProjectId == null) {
        sessionState
    } else {
        EngineSessionState.ready(
            projectState.globalSessions.map { cached ->
                SessionRow(
                    uuid = cached.sessionId,
                    title = cached.title,
                    messageCount = cached.messageCount,
                    relativeTime = cached.relativeTime,
                )
            },
        )
    }
    val drawerProductionData = DrawerProductionData(
        workspaces = listOf(LocalProjectWorkspace),
        projects = projectState.projects.map { it.toDrawerProject() },
        crons = emptyList(),
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
            chatViewModel.reportHostError("项目正在同步，请等待同步完成。")
        } else {
            action()
        }
    }
    fun runProjectSync(projectId: String, action: () -> Unit): Boolean {
        val activeProjectIsExecuting =
            projectId == sourceProjectId && (state.streaming || state.sessionTransitioning)
        if (activeProjectIsExecuting) {
            chatViewModel.reportHostError("请先停止当前任务，再同步项目。")
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
                            drawerUi.selectSession(ref.id)
                            chatViewModel.openSession(ref)
                            closeDrawer()
                        },
                        onOpenSettings = {
                            closeDrawer()
                            onOpenSettings()
                        },
                        onClose = { closeDrawer() },
                        engineSessions = globalDrawerSessions,
                        onResumeSession = { uuid ->
                            globalDrawerSessions.rows.firstOrNull { it.uuid == uuid }?.let { row ->
                                if (sourceProjectId == null) {
                                    drawerUi.selectSession(uuid)
                                    chatViewModel.resumeSession(row)
                                } else {
                                    scope.launch {
                                        if (
                                            switchEngineScope(
                                                null,
                                                SessionRef(row.uuid, row.title),
                                                false,
                                            )
                                        ) {
                                            closeDrawer()
                                        }
                                    }
                                }
                                if (sourceProjectId == null) closeDrawer()
                            }
                        },
                        productionData = drawerProductionData,
                        onCreateProject = { showCreateProject = true },
                        onSelectProjectSession = { projectId, ref ->
                            val project = projectState.projects.firstOrNull {
                                it.record.id == projectId
                            } ?: return@DrawerContent
                            val switched = if (sourceProjectId == projectId) {
                                drawerUi.selectSession(ref.id)
                                chatViewModel.openSession(ref)
                                true
                            } else {
                                scope.launch {
                                    if (switchEngineScope(project, ref, false)) closeDrawer()
                                }
                                false
                            }
                            if (switched) closeDrawer()
                        },
                        onNewProjectSession = { projectId ->
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
                ChatScreen(
                    state = state,
                    onSend = { text -> runConversationAction { chatViewModel.send(text) } },
                    // "新对话": reset the local transcript immediately AND tell the
                    // engine to begin a new session (NewSession). For the mock the
                    // engine call is a no-op, so this still behaves like newChat.
                    onNewChat = { runConversationAction(chatViewModel::startNewSession) },
                    onSelectModel = chatViewModel::selectModel,
                    isDark = isDark,
                    onToggleTheme = onToggleTheme,
                    onOpenDrawer = { scope.launch { drawerState.open() } },
                    // TAP the mic → FlowMode orb; press-and-hold → STT (below).
                    onMicClick = { flowActive = true },
                    onMicHoldStart = {
                        voiceActive = true
                        onVoiceHoldStart()
                    },
                    onMicHoldRelease = {
                        voiceActive = false
                        onVoiceHoldRelease()
                    },
                    draft = draft,
                    onDraftChange = {
                        draft = it
                        chatViewModel.onDraftChanged(it) // mirror into SavedStateHandle
                    },
                    onCameraClick = onCameraClick,
                    attachment = attachment,
                    onRemoveAttachment = { attachment = null },
                    onShare = onShare,
                    onStop = chatViewModel::cancel,
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
                )
            }
        }

        // Immersive voice overlay (full-screen, above everything else).
        VoiceFlowOverlay(visible = voiceActive)

        // FlowMode (心流) voice-orb overlay — interactive living-orb experience
        // opened by a mic tap; dismissed by its own close button.
        FlowModeOverlay(
            visible = flowActive,
            assistantName = assistantName,
            inputDialog = inputDialog,
            voiceLang = voiceLang,
            streaming = state.streaming,
            assistantText = orbAssistantText,
            onSend = { text -> runConversationAction { chatViewModel.send(text) } },
            onCancel = { chatViewModel.cancel() },
            onListen = orbListen,
            onClose = { flowActive = false },
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
    }
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
