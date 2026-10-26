package com.lingxi.code.settings

import androidx.activity.compose.BackHandler
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.remember
import com.lingxi.code.conversation.ConversationSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.offline.VoiceModelDownloader
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first

/** Native settings navigation shares device repositories and the active engine connection. */
@Composable
fun SettingsHost(
    appearanceStore: AppearanceStore,
    engineSource: ConversationSource? = null,
    deepLinkImport: com.lingxi.code.navigation.DeepLink? = null,
    onDeepLinkImportDismiss: () -> Unit = {},
    projectStore: com.lingxi.code.project.ProjectStore? = null,
    isDark: Boolean,
    accentId: String,
    modifier: Modifier = Modifier,
    navController: NavHostController = rememberNavController(),
    store: SettingsStore? = null,
    initialRoute: String = SettingsRoutes.MAIN,
    onClose: () -> Unit = {},
    onOpenScheduledSession: (com.lingxi.code.cron.CronRunRecord) -> Unit = {},
    onReplayOnboarding: () -> Unit = {},
    onReconnectEngine: () -> Boolean = { false },
    onOpenTerminal: (LinuxRuntimeTerminalLaunchRequest) -> Unit = {},
    onPermissionModeChanged: suspend (String) -> Unit = {},
    onTypescriptLspModeChanged: suspend (String) -> Unit = {},
    // 把当前会话切到另一个项目。设置页自己做不了：项目层/本地层写到哪个目录由引擎
    // 进程的 cwd 决定，换 cwd 就要重建会话源，那套状态机住在 RootScreen 里。
    // null 表示本宿主不提供切换能力，此时层选择器下不画切换入口。
    onSwitchProject: ((String) -> Unit)? = null,
) {
    val engineBridge = remember { SettingsEngineBridge() }
    val draftRegistry = remember(engineSource) { SettingsDraftRegistry() }
    val engineSettingsState by engineBridge.state.collectAsStateWithLifecycle()
    LaunchedEffect(engineSource) { engineBridge.bind(engineSource) }
    val context = LocalContext.current
    val resolvedStore: SettingsStore =
        store ?: viewModel(factory = SettingsStore.factory(context))
    LaunchedEffect(resolvedStore, engineSettingsState.generation, engineSettingsState.credentialRevision) {
        if (engineSettingsState.credentialRevision > 0) resolvedStore.refreshAfterProviderCredentialMutation()
    }
    val reconnectSafely: () -> Unit = {
        if (draftRegistry.hasUnsavedDrafts || engineSettingsState.savingSettings || engineSettingsState.credentialSaving || engineSettingsState.pending != null) {
            draftRegistry.reconnectBlocked = true
        } else {
            resolvedStore.applyLlmConfiguration(onReconnectEngine)
        }
    }
    val t = LingXiTheme.palette
    val state by resolvedStore.state.collectAsStateWithLifecycle()
    val modelStates by VoiceModelDownloader.states.collectAsStateWithLifecycle()
    val audioRoutingSession = engineSource?.activeSessionState?.collectAsStateWithLifecycle()?.value
    val audioRoutingModel = engineSource?.modelState?.collectAsStateWithLifecycle()?.value
    val runtimeMcpServers = engineSource?.mcpServers?.collectAsStateWithLifecycle()?.value.orEmpty()
    val probedVoiceCapability by produceState(
        initialValue = state.voiceCapability,
        key1 = context.applicationContext,
        key2 = state.voice,
        key3 = listOf(modelStates, state.llmProviders, audioRoutingSession, audioRoutingModel),
    ) {
        value = probeVoiceCapabilitySnapshot(
            context = context.applicationContext,
            preferences = state.voice,
            modelStates = modelStates,
        )
    }
    val backEntry by navController.currentBackStackEntryAsState()
    val route = backEntry?.destination?.route
    val atRoot = route == null || route == SettingsRoutes.MAIN
    // The scheduled-task pages are opened from the drawer, not from settings:
    // they take the place of the settings home so back closes the surface.
    val standalone = SettingsRoutes.isStandalone(initialRoute)

    LaunchedEffect(navController, initialRoute) {
        if (initialRoute != SettingsRoutes.MAIN) {
            // The adaptive host composes its NavHost after measuring the
            // available width. Await the graph before handling a deep link.
            navController.currentBackStackEntryFlow.first()
            navController.navigate(initialRoute) {
                launchSingleTop = true
                if (standalone) popUpTo(SettingsRoutes.MAIN) { inclusive = true }
            }
        }
    }
    LaunchedEffect(probedVoiceCapability) {
        resolvedStore.setVoiceCapability(probedVoiceCapability)
    }

    // Pop one page, or close the whole surface when there is no page below.
    // Checked before popping so the host never empties its own back stack.
    val goBack: () -> Unit = {
        if (navController.previousBackStackEntry != null) navController.popBackStack() else onClose()
    }
    // System back.
    BackHandler(enabled = true) { if (deepLinkImport != null) onDeepLinkImportDismiss() else goBack() }

    if (deepLinkImport != null) {
        DeepLinkImportPage(deepLinkImport, engineBridge, onDeepLinkImportDismiss)
        return
    }

    androidx.compose.material3.Surface(
        color = t.windowBg,
        contentColor = t.text,
        modifier = modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.systemBars),
    ) {
        // 经一个恒定存在的 flow 收集，而不是 `projectStore?.state?.collect…()`：
        // 后者会让一次 @Composable 调用变成条件调用（projectStore 可空），而直接读
        // `.value` 又不会在项目列表变化时重组。
        val projectsFlow = remember(projectStore) {
            projectStore?.state
                ?: kotlinx.coroutines.flow.MutableStateFlow(com.lingxi.code.project.ProjectStoreState())
        }
        val projectsForSwitching by projectsFlow.collectAsStateWithLifecycle()
        val projectSwitching = SettingsProjectSwitching(
            projects = projectsForSwitching.projects.map {
                SettingsProjectChoice(id = it.record.id, name = it.record.name)
            },
            activeProjectId = projectsForSwitching.activeProjectId,
            switchTo = onSwitchProject,
        )
        androidx.compose.runtime.CompositionLocalProvider(
            LocalSettingsDraftRegistry provides draftRegistry,
            LocalSettingsProjectSwitching provides projectSwitching,
        ) {
        Column(Modifier.fillMaxSize()) {
            SettingsTopBar(
                title = titleFor(backEntry, state),
                atRoot = atRoot,
                onBack = goBack,
                onClose = onClose,
                // 完成: back to the settings home, or close when it is not underneath.
                onReset = { if (!navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)) onClose() },
            )

            SettingsConnectionBanner(engineSettingsState, reconnectSafely)
            BoxWithConstraints(Modifier.fillMaxSize()) {
            val wide = maxWidth >= 840.dp
            Row(Modifier.fillMaxSize()) {
            if (wide && !standalone) Box(Modifier.width(280.dp).verticalScroll(rememberScrollState()).padding(16.dp)) {
                DesktopSettingsNavigation(onNavigate = { navController.navigate(it) { launchSingleTop = true } }, selected = route)
            }
            NavHost(
                navController = navController,
                startDestination = SettingsRoutes.MAIN,
                modifier = Modifier.fillMaxSize(),
            ) {
                page(SettingsRoutes.MAIN) {
                    if (wide) GeneralSettingsPage(onNavigate = { navController.navigate(it) }, onReplayOnboarding = onReplayOnboarding)
                    else SettingsHomePage(onNavigate = { navController.navigate(it) })
                }
                page(SettingsRoutes.GROUP) { entry ->
                    SettingsGroup.fromKey(entry.arguments?.getString("group"))?.let { group ->
                        SettingsGroupPage(
                            group = group,
                            onNavigate = { navController.navigate(it) },
                            onReplayOnboarding = onReplayOnboarding,
                        )
                    }
                }
                page(SettingsRoutes.FUSION) { Text(stringResource(R.string.settings_parity_fusion_unavailable)) }
                page(SettingsRoutes.GENERAL) { GeneralSettingsPage(onNavigate = { navController.navigate(it) }, onReplayOnboarding = onReplayOnboarding) }
                page(SettingsRoutes.ACCOUNT) { NativeAccountPage(state, onCredentials = { navController.navigate(SettingsRoutes.CREDENTIALS) }) }
                page(SettingsRoutes.CREDENTIALS) { EngineCredentialsPage(engineBridge, onMobileProfiles = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Llm.name)) }) }
                page(SettingsRoutes.ABOUT) { NativeAboutPage(onLicenses = { navController.navigate(SettingsRoutes.OPEN_SOURCE) }) }
                composable(SettingsRoutes.ARCHIVED) {
                    if (projectStore != null) {
                        val projects by projectStore.state.collectAsStateWithLifecycle()
                        var archiveError by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf<String?>(null) }
                        val archiveScope = androidx.compose.runtime.rememberCoroutineScope()
                        Column {
                            archiveError?.let { Text(it, color = androidx.compose.material3.MaterialTheme.colorScheme.error) }
                            Box(Modifier.weight(1f)) {
                                com.lingxi.code.project.ArchivedSettingsPage(
                                    state = projects,
                                    onRestore = { projectId, sessionId ->
                                        archiveScope.launch {
                                            runCatching { projectStore.setSessionArchived(projectId, sessionId, false) }
                                                .onSuccess { archiveError = null }
                                                .onFailure { archiveError = "Could not restore the conversation. Please retry." }
                                        }
                                    },
                                    onArchive = { projectId, sessionId ->
                                        archiveScope.launch {
                                            runCatching { projectStore.setSessionArchived(projectId, sessionId, true) }
                                                .onSuccess { archiveError = null }
                                                .onFailure { archiveError = "Could not archive the conversation. Please retry." }
                                        }
                                    },
                                )
                            }
                        }
                    } else Text(settingsLabel("Project storage is unavailable. Reopen settings after the application finishes loading."))
                }
                page(SettingsRoutes.PROJECTS) {
                    Column {
                        if (projectStore != null) {
                            val projects by projectStore.state.collectAsStateWithLifecycle()
                            if (projects.projects.isEmpty()) Text(settingsLabel("No projects saved on this device."))
                            projects.projects.forEach { project ->
                                SettingsSection(label = project.record.name) {
                                    Text(project.workspace.hostPath)
                                    Text(settingsLabel("${project.sessions.size} conversations · ${project.record.syncState.label}"))
                                }
                            }
                        }
                        EngineSettingsPage(SettingsRoutes.PROJECTS, engineBridge)
                    }
                }
                page(SettingsRoutes.ENGINE_MCP) { McpDirectoryPage(engineBridge, runtimeMcpServers) }
                (layeredPageKeys.keys.filter { it != SettingsRoutes.PROJECTS && it != SettingsRoutes.PLUGINS } + SettingsRoutes.DIAGNOSTICS).forEach { engineRoute ->
                    page(engineRoute) { EngineSettingsPage(engineRoute, engineBridge, reconnectSafely) }
                }
                page(SettingsRoutes.PLUGINS) {
                    PluginsPage(
                        bridge = engineBridge,
                        onDetail = { id -> navController.navigate(SettingsRoutes.pluginDetail(id)) },
                        onAdvanced = { navController.navigate(SettingsRoutes.PLUGINS_ADVANCED) },
                    )
                }
                page(SettingsRoutes.PLUGIN_DETAIL) {
                    PluginDetailPage(
                        bridge = engineBridge,
                        pluginId = it.arguments?.getString("id").orEmpty(),
                        onReconnect = reconnectSafely,
                        onPop = { navController.popBackStack() },
                    )
                }
                page(SettingsRoutes.PLUGINS_ADVANCED) { EngineSettingsPage(SettingsRoutes.PLUGINS, engineBridge, reconnectSafely) }

                // Device settings
                page(SettingsRoutes.APPEARANCE) {
                    AppearancePage(store = appearanceStore, isDark = isDark, accentId = accentId)
                }
                page(SettingsRoutes.LANGUAGE) {
                    LanguagePage(language = state.language, onSelect = resolvedStore::setLanguage)
                }
                page(SettingsRoutes.NOTIFICATIONS) {
                    NativeNotificationsPage(
                        config = state.notifs,
                        onChange = resolvedStore::setNotifs,
                    )
                }
                page(SettingsRoutes.INPUT) { NativeSystemSettingsPage(input = true) }

                // Security and runtime preferences
                page(SettingsRoutes.PRIVACY) { NativeSystemSettingsPage(input = false) }
                page(SettingsRoutes.PERMISSION_MODE) {
                    PermissionModePage(
                        selected = state.permissionMode,
                        effective = state.effectivePermissionMode,
                        error = state.permissionModeError,
                        onSelect = { mode ->
                            resolvedStore.setPermissionMode(mode, onPermissionModeChanged)
                        },
                    )
                }
                page(SettingsRoutes.TYPESCRIPT_LSP) {
                    TypeScriptLspModePage(
                        selected = state.typescriptLspMode,
                        effective = state.effectiveTypescriptLspMode,
                        available = state.typescriptLspAvailable,
                        error = state.typescriptLspError,
                        onSelect = { mode ->
                            resolvedStore.setTypescriptLspMode(mode, onTypescriptLspModeChanged)
                        },
                    )
                }
                // This page owns a LazyColumn and must receive bounded height.
                composable(SettingsRoutes.OPEN_SOURCE) {
                    OpenSourceLicensesPage(Modifier.fillMaxSize().padding(top = 16.dp))
                }

                // Providers and voice
                page(SettingsRoutes.VOICE) {
                    VoicePage(
                        voice = state.voice,
                        capability = state.voiceCapability,
                        revision = state.voiceRevision,
                        saving = state.voiceSaving,
                        saveError = state.voiceSaveError,
                        onProviderSettings = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Llm.name)) },
                        onChange = resolvedStore::updateVoice,
                    )
                }
                page(SettingsRoutes.PROVIDER_LIST) {
                    val kind = providerKindArg(it)
                    ProviderListPage(
                        kind = kind,
                        state = state,
                        store = resolvedStore,
                        onEdit = { id -> navController.navigate(SettingsRoutes.providerEdit(kind.name, id)) },
                        onAdd = { navController.navigate(SettingsRoutes.providerPicker(kind.name)) },
                        onReconnectEngine = reconnectSafely,
                        onNavigate = { navController.navigate(it) },
                    )
                }
                page(SettingsRoutes.PROVIDER_PICKER) {
                    val kind = providerKindArg(it)
                    ProviderPickerPage(
                        kind = kind,
                        state = state,
                        store = resolvedStore,
                        // Replace the picker with the edit page so the back stack is
                        // list → edit (the iOS `replaceTopTwo` behavior).
                        onPicked = { newId ->
                            navController.navigate(SettingsRoutes.providerEdit(kind.name, newId)) {
                                popUpTo(SettingsRoutes.PROVIDER_PICKER) { inclusive = true }
                            }
                        },
                    )
                }
                page(SettingsRoutes.PROVIDER_EDIT) {
                    val kind = providerKindArg(it)
                    val id = it.arguments?.getString("id") ?: ""
                    ProviderEditPage(
                        kind = kind,
                        providerId = id,
                        state = state,
                        store = resolvedStore,
                        onReconnectEngine = reconnectSafely,
                        onPop = { navController.popBackStack() },
                    )
                }
                // Mobile capability routes
                page(SettingsRoutes.SKILLS) {
                    SkillsPage(
                        state = state,
                        store = resolvedStore,
                        onDetail = { id -> navController.navigate(SettingsRoutes.skillDetail(id)) },
                    )
                }
                page(SettingsRoutes.SKILL_DETAIL) {
                    SkillManager(engineBridge, initialId = it.arguments?.getString("id"))
                }
                // Preserve the legacy route while directing it to authoritative configuration.
                page(SettingsRoutes.MCP_LIST) { McpDirectoryPage(engineBridge, runtimeMcpServers) }
                page(SettingsRoutes.LINUX_RUNTIME) {
                    LinuxRuntimePage(
                        state = state,
                        store = resolvedStore,
                        onOpenTerminal = onOpenTerminal,
                    )
                }
                page(SettingsRoutes.COMPUTER_USE) {
                    ComputerUseSettingsPage(
                        voice = state.voice,
                        capability = state.voiceCapability,
                        onOpenAudioSettings = {
                            navController.navigate(SettingsRoutes.VOICE)
                        },
                    )
                }
                page(SettingsRoutes.MCP_EDIT) { entry -> McpConfigurationPage(engineBridge, initialName = entry.arguments?.getString("id"), initialScope = entry.arguments?.getString("scope")?.takeUnless { it == "_" }) }
                page(SettingsRoutes.DREAM) { GeneralSettingsPage(onNavigate = { navController.navigate(it) }, onReplayOnboarding = onReplayOnboarding) }
                page(SettingsRoutes.CRON) {
                    com.lingxi.code.cron.CronScreen(engineSource = engineSource, projectStore = projectStore, onOpenSession = onOpenScheduledSession)
                }
                page(SettingsRoutes.CRON_TASK) { entry ->
                    com.lingxi.code.cron.CronScreen(
                        engineSource = engineSource, projectStore = projectStore, onOpenSession = onOpenScheduledSession,
                        initialTaskKey = entry.arguments?.getString("task"),
                    )
                }
                page(SettingsRoutes.CRON_RUN) { entry ->
                    com.lingxi.code.cron.CronScreen(
                        engineSource = engineSource, projectStore = projectStore, onOpenSession = onOpenScheduledSession,
                        initialRunId = entry.arguments?.getString("run"),
                    )
                }
                page(SettingsRoutes.CRON_NEW) { entry ->
                    com.lingxi.code.cron.CronScreen(
                        engineSource = engineSource, projectStore = projectStore, onOpenSession = onOpenScheduledSession,
                        initialPrompt = entry.arguments?.getString("prompt"),
                    )
                }
            }
            }
            }
        }
        }
    }
}

/** Registers a destination whose body scrolls inside the standard page padding. */
private fun androidx.navigation.NavGraphBuilder.page(
    route: String,
    content: @Composable (androidx.navigation.NavBackStackEntry) -> Unit,
) {
    composable(route) { entry ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(top = 16.dp, bottom = 28.dp),
        ) { content(entry) }
    }
}

/** Parse the `{kind}` route argument into a [ProviderKind] (defaults to LLM). */
private fun providerKindArg(entry: androidx.navigation.NavBackStackEntry): ProviderKind =
    runCatching { ProviderKind.valueOf(entry.arguments?.getString("kind") ?: "") }
        .getOrDefault(ProviderKind.Llm)

/** Resolve the top-bar title for a back-stack entry (+ data-derived titles). */
@Composable
private fun titleFor(entry: androidx.navigation.NavBackStackEntry?, state: SettingsUiState): String =
    when (val route = entry?.destination?.route) {
        in desktopSettingsEntries.map { it.route }.filter { it != SettingsRoutes.ACCOUNT && it != SettingsRoutes.APPEARANCE && it != SettingsRoutes.VOICE } -> settingsLabel(desktopSettingsEntries.first { it.route == route }.title)
        null, SettingsRoutes.MAIN -> stringResource(SettingsTitles.MAIN)
        SettingsRoutes.FUSION -> stringResource(R.string.settings_parity_fusion)
        SettingsRoutes.ACCOUNT -> stringResource(SettingsTitles.ACCOUNT)
        SettingsRoutes.VOICE -> stringResource(SettingsTitles.VOICE)
        SettingsRoutes.APPEARANCE -> stringResource(SettingsTitles.APPEARANCE)
        SettingsRoutes.LANGUAGE -> stringResource(SettingsTitles.LANGUAGE)
        SettingsRoutes.NOTIFICATIONS -> stringResource(SettingsTitles.NOTIFICATIONS)
        SettingsRoutes.INPUT -> stringResource(SettingsTitles.INPUT)
        SettingsRoutes.PRIVACY -> stringResource(SettingsTitles.PRIVACY)
        SettingsRoutes.PERMISSION_MODE -> stringResource(SettingsTitles.PERMISSION_MODE)
        SettingsRoutes.TYPESCRIPT_LSP -> stringResource(SettingsTitles.TYPESCRIPT_LSP)
        SettingsRoutes.OPEN_SOURCE -> stringResource(SettingsTitles.OPEN_SOURCE)
        SettingsRoutes.SKILLS -> stringResource(SettingsTitles.SKILLS)
        SettingsRoutes.PLUGIN_DETAIL -> stringResource(R.string.plugins_detail_title)
        SettingsRoutes.PLUGINS_ADVANCED -> stringResource(R.string.plugins_advanced)
        SettingsRoutes.MCP_LIST -> stringResource(SettingsTitles.MCP)
        SettingsRoutes.LINUX_RUNTIME -> stringResource(SettingsTitles.LINUX_RUNTIME)
        SettingsRoutes.COMPUTER_USE -> stringResource(SettingsTitles.COMPUTER_USE)
        SettingsRoutes.DREAM -> stringResource(SettingsTitles.DREAM)
        SettingsRoutes.CRON,
        SettingsRoutes.CRON_TASK,
        SettingsRoutes.CRON_RUN,
        SettingsRoutes.CRON_NEW,
        -> stringResource(SettingsTitles.CRON)
        SettingsRoutes.GROUP ->
            SettingsGroup.fromKey(entry.arguments?.getString("group"))
                ?.let { stringResource(it.title) }
                ?: stringResource(SettingsTitles.MAIN)
        SettingsRoutes.PROVIDER_LIST -> providerKindArg(entry).let {
            if (it == ProviderKind.Llm) stringResource(R.string.settings_group_models_hub) else stringResource(it.titleRes)
        }
        SettingsRoutes.PROVIDER_PICKER ->
            stringResource(R.string.provider_add_kind_fmt, stringResource(providerKindArg(entry).titleRes))
        SettingsRoutes.PROVIDER_EDIT -> {
            val kind = providerKindArg(entry)
            val id = entry.arguments?.getString("id")
            state.providers(kind).firstOrNull { it.id == id }?.name ?: stringResource(kind.titleRes)
        }
        SettingsRoutes.SKILL_DETAIL -> {
            val id = entry.arguments?.getString("id")
            state.skills.firstOrNull { it.id == id }?.name ?: stringResource(SettingsTitles.SKILLS)
        }
        SettingsRoutes.MCP_EDIT -> {
            val id = entry.arguments?.getString("id")
            state.mcpServers.firstOrNull { it.id == id }?.name ?: stringResource(SettingsTitles.MCP)
        }
        else -> stringResource(SettingsTitles.MAIN)
    }

/**
 * Brand settings top bar — chevron-back (or close 'x' at the root) on the left,
 * centered title, and a 完成 action that resets to the root when nested.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun SettingsTopBar(
    title: String, atRoot: Boolean, onBack: () -> Unit,
    onClose: () -> Unit, onReset: () -> Unit,
) {
    androidx.compose.material3.TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            androidx.compose.material3.IconButton(onClick = { if (atRoot) onClose() else onBack() }) {
                if (atRoot) LXIcon(LXIconName.X, contentDescription = stringResource(R.string.settings_close_accessibility))
                else androidx.compose.material3.Icon(
                    androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.onboarding_back))
            }
        },
        actions = {
            if (!atRoot) androidx.compose.material3.TextButton(onClick = onReset) {
                Text(stringResource(R.string.settings_done))
            }
        },
        windowInsets = androidx.compose.foundation.layout.WindowInsets(0, 0, 0, 0),
    )
}
