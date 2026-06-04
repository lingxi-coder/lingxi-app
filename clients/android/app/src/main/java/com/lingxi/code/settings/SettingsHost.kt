package com.lingxi.code.settings

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
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
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme

/**
 * Settings surface host.
 *
 * Mirrors the iOS `SettingsHost` push-navigation stack with idiomatic
 * Navigation-Compose: a nested [NavHost] is the page stack; each destination is
 * pushed/popped on the [NavHostController]. A brand top bar shows the current
 * page title, a chevron-back affordance that pops the stack (or [onClose] at the
 * root), and a 完成 / close action. System back is bridged: at the root it calls
 * [onClose] (return to the conversation); deeper it pops one page.
 *
 * Theme + accent are owned by the DataStore-backed [appearanceStore] so the
 * Appearance page mutates the live, persisted source of truth. The mutable mock
 * settings (providers / skills / notifications / …) live in the hoisted
 * [store]; A7/A8 build the deeper editors on the same NavHost + store.
 *
 * @param onClose return to the conversation (root system-back + close button).
 */
@Composable
fun SettingsHost(
    appearanceStore: AppearanceStore,
    isDark: Boolean,
    accentId: String,
    modifier: Modifier = Modifier,
    navController: NavHostController = rememberNavController(),
    store: SettingsStore = viewModel(),
    onClose: () -> Unit = {},
) {
    val t = LingXiTheme.palette
    val state by store.state.collectAsState()
    val backEntry by navController.currentBackStackEntryAsState()
    val route = backEntry?.destination?.route
    val atRoot = route == null || route == SettingsRoutes.MAIN

    // System back: pop one page, or close the whole surface at the root.
    BackHandler(enabled = true) {
        if (!navController.popBackStack()) onClose()
    }

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .windowInsetsPadding(WindowInsets.systemBars),
    ) {
        Column(Modifier.fillMaxSize()) {
            SettingsTopBar(
                title = titleFor(backEntry, state),
                atRoot = atRoot,
                onBack = { if (!navController.popBackStack()) onClose() },
                onClose = onClose,
                onReset = { navController.popBackStack(SettingsRoutes.MAIN, inclusive = false) },
            )

            NavHost(
                navController = navController,
                startDestination = SettingsRoutes.MAIN,
                modifier = Modifier.fillMaxSize(),
            ) {
                page(SettingsRoutes.MAIN) {
                    MainSettingsPage(state = state, isDark = isDark, navController = navController)
                }
                page(SettingsRoutes.ACCOUNT) { AccountPage() }

                // 记忆与知识 (A6)
                page(SettingsRoutes.KNOWLEDGE) { KnowledgePage() }
                page(SettingsRoutes.MEMORY) { MemoryPage() }
                page(SettingsRoutes.WORKFLOWS) { WorkflowsPage() }

                // 应用 (A6)
                page(SettingsRoutes.APPEARANCE) {
                    AppearancePage(store = appearanceStore, isDark = isDark, accentId = accentId)
                }
                page(SettingsRoutes.LANGUAGE) {
                    LanguagePage(language = state.language, onSelect = store::setLanguage)
                }
                page(SettingsRoutes.NOTIFICATIONS) {
                    NotificationsPage(notifs = state.notifs, onChange = store::setNotifs)
                }
                page(SettingsRoutes.INPUT) { InputPage() }

                // 隐私与安全 (A6)
                page(SettingsRoutes.PRIVACY) { PrivacyPage() }

                // 智能 — providers (A7) + voice TTS editor
                page(SettingsRoutes.VOICE) {
                    VoicePage(voice = state.voice, onChange = store::setVoice)
                }
                page(SettingsRoutes.PROVIDER_LIST) {
                    val kind = providerKindArg(it)
                    ProviderListPage(
                        kind = kind,
                        state = state,
                        store = store,
                        onEdit = { id -> navController.navigate(SettingsRoutes.providerEdit(kind.name, id)) },
                        onAdd = { navController.navigate(SettingsRoutes.providerPicker(kind.name)) },
                    )
                }
                page(SettingsRoutes.PROVIDER_PICKER) {
                    val kind = providerKindArg(it)
                    ProviderPickerPage(
                        kind = kind,
                        store = store,
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
                        store = store,
                        onPop = { navController.popBackStack() },
                    )
                }
                // 能力扩展 — Skills / MCP / Dream (A8)
                page(SettingsRoutes.SKILLS) {
                    SkillsPage(
                        state = state,
                        store = store,
                        onDetail = { id -> navController.navigate(SettingsRoutes.skillDetail(id)) },
                    )
                }
                page(SettingsRoutes.SKILL_DETAIL) {
                    val id = it.arguments?.getString("id") ?: ""
                    SkillDetailPage(
                        skillId = id,
                        state = state,
                        store = store,
                        onPop = { navController.popBackStack() },
                    )
                }
                page(SettingsRoutes.MCP_LIST) {
                    MCPListPage(
                        state = state,
                        store = store,
                        onEdit = { id -> navController.navigate(SettingsRoutes.mcpEdit(id)) },
                    )
                }
                page(SettingsRoutes.MCP_EDIT) {
                    val id = it.arguments?.getString("id") ?: ""
                    MCPEditPage(
                        mcpId = id,
                        state = state,
                        store = store,
                        onPop = { navController.popBackStack() },
                    )
                }
                page(SettingsRoutes.DREAM) { DreamPage(state = state, store = store) }
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
private fun titleFor(entry: androidx.navigation.NavBackStackEntry?, state: SettingsUiState): String =
    when (val route = entry?.destination?.route) {
        null, SettingsRoutes.MAIN -> SettingsTitles.MAIN
        SettingsRoutes.ACCOUNT -> SettingsTitles.ACCOUNT
        SettingsRoutes.VOICE -> SettingsTitles.VOICE
        SettingsRoutes.KNOWLEDGE -> SettingsTitles.KNOWLEDGE
        SettingsRoutes.MEMORY -> SettingsTitles.MEMORY
        SettingsRoutes.WORKFLOWS -> SettingsTitles.WORKFLOWS
        SettingsRoutes.APPEARANCE -> SettingsTitles.APPEARANCE
        SettingsRoutes.LANGUAGE -> SettingsTitles.LANGUAGE
        SettingsRoutes.NOTIFICATIONS -> SettingsTitles.NOTIFICATIONS
        SettingsRoutes.INPUT -> SettingsTitles.INPUT
        SettingsRoutes.PRIVACY -> SettingsTitles.PRIVACY
        SettingsRoutes.SKILLS -> SettingsTitles.SKILLS
        SettingsRoutes.MCP_LIST -> SettingsTitles.MCP
        SettingsRoutes.DREAM -> SettingsTitles.DREAM
        SettingsRoutes.PROVIDER_LIST -> providerKindArg(entry).title
        SettingsRoutes.PROVIDER_PICKER -> "添加${providerKindArg(entry).title}"
        SettingsRoutes.PROVIDER_EDIT -> {
            val kind = providerKindArg(entry)
            val id = entry.arguments?.getString("id")
            state.providers(kind).firstOrNull { it.id == id }?.name ?: kind.title
        }
        SettingsRoutes.SKILL_DETAIL -> {
            val id = entry.arguments?.getString("id")
            state.skills.firstOrNull { it.id == id }?.name ?: SettingsTitles.SKILLS
        }
        SettingsRoutes.MCP_EDIT -> {
            val id = entry.arguments?.getString("id")
            state.mcpServers.firstOrNull { it.id == id }?.name ?: SettingsTitles.MCP
        }
        else -> SettingsTitles.MAIN
    }

/**
 * Brand settings top bar — chevron-back (or close 'x' at the root) on the left,
 * centered title, and a 完成 action that resets to the root when nested.
 */
@Composable
private fun SettingsTopBar(
    title: String,
    atRoot: Boolean,
    onBack: () -> Unit,
    onClose: () -> Unit,
    onReset: () -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .drawBehind {
                drawLine(
                    color = t.border,
                    start = Offset(0f, size.height),
                    end = Offset(size.width, size.height),
                    strokeWidth = 0.5.dp.toPx(),
                )
            }
            .padding(horizontal = 8.dp)
            .padding(top = 6.dp, bottom = 12.dp),
    ) {
        // Leading: back chevron + "关闭" at the root, or chevron only when nested.
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(2.dp),
            modifier = Modifier
                .widthIn(max = 120.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable { if (atRoot) onClose() else onBack() }
                .padding(horizontal = 4.dp, vertical = 6.dp),
        ) {
            LXIcon(
                name = LXIconName.ChevronR,
                size = 17.dp,
                color = t.text2,
                stroke = 2.2f,
                modifier = Modifier.rotate(180f),
                contentDescription = if (atRoot) "关闭设置" else "返回",
            )
            if (atRoot) {
                Text("关闭", color = t.text2, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1)
            }
        }
        Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
            Text(
                text = title,
                color = t.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
            )
        }
        // Trailing: 完成 (reset to root) when nested, otherwise a close 'x'.
        if (!atRoot) {
            Text(
                "完成",
                color = t.accent,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(onClick = onReset)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )
        } else {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(34.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(onClick = onClose),
            ) {
                LXIcon(name = LXIconName.X, size = 18.dp, color = t.text3, stroke = 1.8f, contentDescription = "关闭设置")
            }
        }
    }
}
