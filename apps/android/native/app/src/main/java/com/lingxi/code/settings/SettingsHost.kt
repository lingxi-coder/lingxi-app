package com.lingxi.code.settings

import androidx.activity.compose.BackHandler
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation.NavGraphBuilder
import androidx.navigation.NavBackStackEntry
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.lingxi.code.R
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.tabs.OnTabReselected
import com.lingxi.code.tabs.SettingsJump
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.offline.VoiceModelDownloader
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first

/**
 * The Settings tab (SPEC-mobile-team-bot-ui section 5): the settings home and every page below it in one
 * Navigation 2 `NavHost`, sharing the device repositories and the active engine connection.
 *
 * It is a tab body, not a surface the user closes: the root has no close action and no "Done", a page
 * below it has a back arrow only. The shell talks to it through five hooks and keeps nothing of the stack:
 *
 * - [jump] replaces the stack with one page (a deep link or an in-app shortcut) and is answered once
 *   through [onJumpHandled];
 * - [origin] is the tab that jump came from. Back, from the system or from the top bar's arrow, on the
 *   page the jump opened (or on the root after a jump to it) clears the stack and calls [onOriginBack];
 *   deeper pages pop first. The shell clears [origin] when the user taps a tab;
 * - [onDepthChanged] reports the pages above the root (0 = on the root), which is where the shell
 *   shows its bottom bar;
 * - at wide widths ([LocalWideLayout]) the 280 dp page list sits beside the page.
 *
 * Back on the root with no [origin] is not handled here: the shell sends it to Chats.
 */
@Composable
fun SettingsHost(
    appearanceStore: AppearanceStore,
    engineSource: ConversationSource? = null,
    projectStore: com.lingxi.code.project.ProjectStore? = null,
    isDark: Boolean,
    accentId: String,
    modifier: Modifier = Modifier,
    store: SettingsStore? = null,
    jump: SettingsJump? = null,
    onJumpHandled: (Long) -> Unit = {},
    origin: AppTab? = null,
    onOriginBack: (AppTab) -> Unit = {},
    onDepthChanged: (Int) -> Unit = {},
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
    val navController = rememberNavController()
    val catalogScope = androidx.compose.runtime.rememberCoroutineScope()
    val engineBridge = remember { SettingsEngineBridge() }
    val draftRegistry = remember(engineSource) { SettingsDraftRegistry() }
    val engineSettingsState by engineBridge.state.collectAsStateWithLifecycle()
    LaunchedEffect(engineSource) { engineBridge.bind(engineSource) }
    val context = LocalContext.current
    val resolvedStore: SettingsStore =
        store ?: viewModel(factory = SettingsStore.factory(context))
    val reconnectSafely: () -> Unit = {
        if (draftRegistry.hasUnsavedDrafts || engineSettingsState.savingSettings || engineSettingsState.pending != null) {
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
    val navigationStack by navController.currentBackStack.collectAsStateWithLifecycle()
    // The pages above the root, bottom first. The root is the start destination, so it is the first routed entry.
    val pagesAboveRoot = remember(navigationStack) { navigationStack.mapNotNull { it.destination.route }.drop(1) }
    val depth = pagesAboveRoot.size
    val wide = LocalWideLayout.current
    val rootScroll = rememberScrollState()
    val listScroll = rememberScrollState()
    val scope = rememberCoroutineScope()

    // A deep link or a shortcut replaces the stack with its page: [root, page].
    val latestOnJumpHandled by rememberUpdatedState(onJumpHandled)
    LaunchedEffect(navController, jump?.id) {
        val request = jump ?: return@LaunchedEffect
        // The graph is set while the NavHost first composes; wait for it before navigating.
        navController.currentBackStackEntryFlow.first()
        if (request.route == SettingsRoutes.MAIN) {
            navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
        } else {
            try {
                navController.navigate(request.route) {
                    launchSingleTop = true
                    popUpTo(SettingsRoutes.MAIN) { inclusive = false }
                }
            } catch (unknown: IllegalArgumentException) {
                // A page this build does not have: stay where we are rather than crash on a stale link.
            }
        }
        latestOnJumpHandled(request.id)
    }

    val latestOnDepthChanged by rememberUpdatedState(onDepthChanged)
    LaunchedEffect(depth) { latestOnDepthChanged(depth) }

    // Tapping the tab again: pop to the root, then scroll the root to its top (section 2.3).
    OnTabReselected(AppTab.Settings) { shown ->
        if (shown > 0) {
            navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
        } else {
            scope.launch { launch { rootScroll.animateScrollTo(0) }; listScroll.animateScrollTo(0) }
        }
    }

    LaunchedEffect(probedVoiceCapability) {
        resolvedStore.setVoiceCapability(probedVoiceCapability)
    }

    // Back from the top bar's arrow, or from a system press that ends a jump (see the handler below).
    val backAction = settingsBackAction(pagesAboveRoot, origin)
    val goBack: () -> Unit = {
        when (val action = backAction) {
            SettingsBackAction.Pop -> navController.popBackStack()
            is SettingsBackAction.LeaveTo -> {
                // Leave the stack cleared: coming back to Settings later starts at the root.
                navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
                onOriginBack(action.tab)
            }
            SettingsBackAction.ShellDecides -> Unit
        }
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
                showBack = depth > 0,
                onBack = goBack,
            )

            SettingsConnectionBanner(engineSettingsState, reconnectSafely)
            Row(Modifier.fillMaxSize()) {
            if (wide) Box(Modifier.width(280.dp).verticalScroll(listScroll).padding(16.dp).testTag(SettingsTags.DESKTOP_NAV)) {
                // The root's right pane is the General page, so that entry is the one to mark there.
                DesktopSettingsNavigation(
                    // Replace the page on the right rather than stacking one per tap: Back then ends a jump in one step.
                    onNavigate = { navController.navigate(it) { launchSingleTop = true; popUpTo(SettingsRoutes.MAIN) { inclusive = false } } },
                    selected = if (route == SettingsRoutes.MAIN) SettingsRoutes.GENERAL else route,
                )
            }
            NavHost(
                navController = navController,
                startDestination = SettingsRoutes.MAIN,
                modifier = Modifier.fillMaxSize(),
            ) {
                page(SettingsRoutes.MAIN, scroll = rootScroll) {
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
                page(SettingsRoutes.ACCOUNT) { NativeAccountPage(state, onCredentials = { navController.navigate(SettingsRoutes.PROVIDER_ACCOUNTS) }) }
                page(SettingsRoutes.PROVIDER_ACCOUNTS) { ProviderAccountsPage(engineBridge, resolvedStore.providerAccounts, state.llmCatalogEntries, onChanged = { resolvedStore.refreshAfterProviderAccountMutation() }, onMobileProfiles = { navController.navigate(SettingsRoutes.providerList(ProviderKind.Llm.name)) }) }
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
                    LaunchedEffect(kind, engineSource, engineSettingsState.connected) {
                        // Binding owns the initial refresh; a fresh app may open this page first.
                        if (kind == ProviderKind.Llm && engineSettingsState.connected) {
                            try { engineBridge.refresh() }
                            catch (error: Exception) {
                                if (error is kotlinx.coroutines.CancellationException) throw error
                                // Dispatch records the failure shown by ProviderListPage.
                            }
                        }
                    }
                    ProviderListPage(
                        kind = kind,
                        state = state,
                        store = resolvedStore,
                        onEdit = { id -> navController.navigate(SettingsRoutes.providerEdit(kind.name, id)) },
                        onAdd = { navController.navigate(SettingsRoutes.providerPicker(kind.name)) },
                        onRefreshCatalog = {
                            if (engineSettingsState.connected) catalogScope.launch {
                                try { engineBridge.refreshOfficialCatalog() }
                                catch (error: Exception) {
                                    if (error is kotlinx.coroutines.CancellationException) throw error
                                }
                            }
                        },
                        catalogRefreshEnabled = engineSettingsState.connected,
                        catalogRefreshNotice = engineSettingsState.notice,
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
            }
            }
        }
        }
    }

    // Registered after the NavHost (which pops pages with its own handler), so it wins over it, but only
    // for the press that ends a jump: everything else is the NavHost's, with its predictive-back animation.
    BackHandler(enabled = backAction is SettingsBackAction.LeaveTo, onBack = goBack)
}

/**
 * Registers a destination whose body scrolls inside the standard page padding. [scroll] is for a page
 * whose scroll position the host needs to reset (the root, when its tab is tapped again).
 */
private fun NavGraphBuilder.page(
    route: String,
    scroll: ScrollState? = null,
    content: @Composable (NavBackStackEntry) -> Unit,
) {
    composable(route) { entry ->
        val scrollState = scroll ?: rememberScrollState()
        Box(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(scrollState)
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
 * The Settings top bar: the title, and a back arrow only on a page below the root. The root has no
 * navigation icon (it is a tab; the shell owns leaving it) and no page has a close or "Done" action.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun SettingsTopBar(title: String, showBack: Boolean, onBack: () -> Unit) {
    androidx.compose.material3.TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        modifier = Modifier.testTag(SettingsTags.TOP_BAR),
        navigationIcon = {
            if (showBack) androidx.compose.material3.IconButton(onClick = onBack, modifier = Modifier.testTag(SettingsTags.BACK)) {
                androidx.compose.material3.Icon(
                    androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.onboarding_back))
            }
        },
        windowInsets = androidx.compose.foundation.layout.WindowInsets(0, 0, 0, 0),
    )
}
