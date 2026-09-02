package com.lingxi.code.drawer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.model.Chat
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.Workspace
import com.lingxi.code.theme.LingXiTheme

/**
 * Production drawer collections supplied by real repositories.
 *
 * `null` means that capability has not been connected yet; an empty list means
 * the real source loaded successfully but currently has no rows. Defaults are
 * deliberately unavailable so preview/prototype [com.lingxi.code.model.MockData]
 * can never leak into a production session or conversation callback.
 */
data class DrawerProductionData(
    val workspaces: List<Workspace>? = null,
    val projects: List<Project>? = null,
    val crons: List<Cron>? = null,
    val projectStatusMessage: String? = null,
    val localAppWorkspaces: List<DrawerLocalAppWorkspace> = emptyList(),
)

/**
 * The active local-app conversation scope, surfaced at the top of the 对话
 * tab: the app's display name plus its workspace-scoped session catalog (the
 * cached `ListAppSessions` reply, init session first). Non-null only while
 * the engine source is bound to a local app's workspace.
 */
data class DrawerAppScope(
    val appId: String,
    val appName: String,
    val sessions: List<SessionRow>,
)

/**
 * The 对话 / 项目 / 定时 drawer content — the Android analog of the iOS `Drawer`,
 * rendered as the `drawerContent` of a Material 3 `ModalNavigationDrawer` (the
 * gesture, scrim and slide are owned by the M3 component, so this is just the
 * 320dp panel body).
 *
 * State is hoisted into [DrawerUiState]; selecting a chat/session calls
 * [onSelectSession] (which the root shell uses to close the drawer and switch
 * the conversation), the terminal shortcut calls [onOpenTerminal], and the
 * account row calls [onOpenSettings].
 *
 * Layout mirrors the prototype top-to-bottom: status-bar offset → header →
 * workspace pills → search → section tabs → scrolling section body → account
 * row.
 */
@Composable
fun DrawerContent(
    ui: DrawerUiState,
    onSelectSession: (SessionRef) -> Unit,
    onOpenSettings: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenTerminal: () -> Unit = {},
    /**
     * The engine's REAL resumable sessions (out-of-band catalog). The 对话 tab
     * renders this state directly, including loading / empty / error, and never
     * falls back to mock sessions in production.
     */
    engineSessions: EngineSessionState = EngineSessionState.loading(),
    /** Resume a real engine session by its wire uuid. */
    onResumeSession: (String) -> Unit = { onSelectSession(SessionRef(it, it)) },
    onNewGlobalSession: () -> Unit = { onSelectSession(SessionRef("new", "")) },
    /** Real project/cron/workspace collections. Null collections render unavailable. */
    productionData: DrawerProductionData = DrawerProductionData(),
    onCreateProject: () -> Unit = {},
    onSelectProjectSession: (String, SessionRef) -> Unit = { _, session -> onSelectSession(session) },
    onNewProjectSession: (String) -> Unit = {},
    onReimportProject: (String) -> Unit = {},
    onExportProject: (String) -> Unit = {},
    onReauthorizeProject: (String) -> Unit = {},
    onOpenCron: (String) -> Unit = {},
    onCreateCron: () -> Unit = {},
    appsCount: Int = 0,
    /**
     * Create a local app and land the conversation in it. The drawer only
     * announces the intent; the create is asynchronous and the hand-off arrives
     * later on `LocalAppsViewModel.createdAppLandings` (`RootScreen.kt`), so
     * this callback closes the drawer itself rather than waiting for a landing
     * that may be seconds away — or, if the create fails, never come.
     */
    onCreateApp: () -> Unit = {},
    /** Browse the app library. */
    onOpenApps: () -> Unit = {},
    /** The active local-app scope's name + sessions, or null outside app scope. */
    appScope: DrawerAppScope? = null,
    /** Resume one of the active app's sessions (same engine scope). */
    onSelectAppScopeSession: (SessionRef) -> Unit = {},
    /** Start a fresh session in the active app's workspace. */
    onNewAppScopeSession: () -> Unit = {},
    onSelectLocalAppSession: (String, SessionRef) -> Unit = { _, session -> onSelectAppScopeSession(session) },
    onNewLocalAppSession: (String) -> Unit = { onNewAppScopeSession() },
    onContinueSession: (ConversationScope, SessionRow, SessionMode) -> Unit = { _, _, _ -> },
    onOpenLocalAppDetails: (String) -> Unit = {},
    onSelectSection: (DrawerSection) -> Unit = { ui.section = it },
    isWorkspaceCollapsed: (SessionMode, String) -> Boolean = { _, _ -> false },
    onToggleWorkspaceCollapsed: (SessionMode, String) -> Unit = { _, _ -> },
    pinnedAtEpochMillis: (SessionMode, String) -> Long? = { _, _ -> null },
    onToggleWorkspacePinned: (SessionMode, String) -> Unit = { _, _ -> },
) {
    val t = LingXiTheme.palette

    // Live search query — filters the active workspace's chats / projects / crons
    // (the real editable analog of the prototype's static search pill). Kept as a
    // plain `remember` (transient, like a search box that resets when the drawer
    // closes); the workspace-scoped lists below recompute on every keystroke.
    var query by remember { mutableStateOf("") }

    val globalName = stringResource(R.string.drawer_workspace_global)
    val projects = productionData.projects.orEmpty()
    val localAppWorkspaces = remember(productionData.localAppWorkspaces, appScope) {
        val activeScopeWorkspace = appScope?.let { scope ->
            DrawerLocalAppWorkspace(
                appId = scope.appId,
                name = scope.appName,
                sessions = scope.sessions.map { row ->
                    com.lingxi.code.localapps.LocalAppSessionRow(
                        uuid = row.uuid,
                        title = row.title,
                        relativeTime = row.relativeTime,
                        messageCount = row.messageCount,
                        mode = row.mode,
                        modifiedAtEpochSeconds = row.modifiedAtEpochSeconds,
                        isInit = false,
                    )
                },
            )
        }
        val merged = productionData.localAppWorkspaces.toMutableList()
        if (activeScopeWorkspace != null && merged.none { it.appId == activeScopeWorkspace.appId }) {
            merged += activeScopeWorkspace
        }
        merged.toList()
    }
    val chatWorkspaceGroups = remember(engineSessions.rows, projects, localAppWorkspaces, query) {
        filterWorkspaceGroups(
            buildWorkspaceGroups(
                mode = SessionMode.Chat,
                globalName = globalName,
                globalSessions = engineSessions.rows,
                projects = projects,
                localApps = localAppWorkspaces,
                pinnedAtEpochMillis = { workspaceKey -> pinnedAtEpochMillis(SessionMode.Chat, workspaceKey) },
            ),
            query,
        )
    }
    val codeWorkspaceGroups = remember(engineSessions.rows, projects, localAppWorkspaces, query) {
        filterWorkspaceGroups(
            buildWorkspaceGroups(
                mode = SessionMode.Code,
                globalName = globalName,
                globalSessions = engineSessions.rows,
                projects = projects,
                localApps = localAppWorkspaces,
                pinnedAtEpochMillis = { workspaceKey -> pinnedAtEpochMillis(SessionMode.Code, workspaceKey) },
            ),
            query,
        )
    }
    val cronWorkspaceGroups = remember(productionData.crons, projects, query) {
        filterCronWorkspaceGroups(
            buildCronWorkspaceGroups(
                globalName = globalName,
                crons = productionData.crons.orEmpty(),
                projects = projects,
            ),
            query,
        )
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.sidebarBg)
            .windowInsetsPadding(WindowInsets.statusBars),
    ) {
        DrawerHeader(onClose = onClose)
        SearchBar(query = query, onQueryChange = { query = it })
        SectionTabs(
            section = ui.section,
            chats = chatWorkspaceGroups.sumOf { it.sessions.size },
            projects = codeWorkspaceGroups.sumOf { it.sessions.size },
            crons = cronWorkspaceGroups.sumOf { it.crons.size },
            onSelect = onSelectSection,
        )

        // Scrolling section body — fills the space above shortcuts/account.
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp)
                .padding(top = 4.dp, bottom = 8.dp),
        ) {
            when (ui.section) {
                DrawerSection.Chat -> {
                    DrawerAppQuickActions(onCreateApp = onCreateApp, onOpenApps = onOpenApps)
                    WorkspaceGroupsSection(
                        groups = chatWorkspaceGroups,
                        mode = SessionMode.Chat,
                        activeWs = ui.activeWs,
                        activeSession = ui.activeSession,
                        onSelectWorkspace = ui::selectWorkspace,
                        isWorkspaceCollapsed = { isWorkspaceCollapsed(SessionMode.Chat, it) },
                        onToggleWorkspaceCollapsed = { onToggleWorkspaceCollapsed(SessionMode.Chat, it) },
                        isWorkspacePinned = { pinnedAtEpochMillis(SessionMode.Chat, it) != null },
                        onToggleWorkspacePinned = { onToggleWorkspacePinned(SessionMode.Chat, it) },
                        onSelectGlobalSession = { onResumeSession(it.uuid) },
                        onSelectProjectSession = onSelectProjectSession,
                        onSelectLocalAppSession = onSelectLocalAppSession,
                        onNewGlobalSession = onNewGlobalSession,
                        onNewProjectSession = onNewProjectSession,
                        onNewLocalAppSession = onNewLocalAppSession,
                        onContinueSession = onContinueSession,
                        onOpenLocalAppLibrary = { onOpenApps() },
                        onOpenLocalAppDetails = onOpenLocalAppDetails,
                    )
                }

                DrawerSection.Code -> {
                    DrawerAppQuickActions(onCreateApp = onCreateApp, onOpenApps = onOpenApps)
                    WorkspaceGroupsSection(
                        groups = codeWorkspaceGroups,
                        mode = SessionMode.Code,
                        activeWs = ui.activeWs,
                        activeSession = ui.activeSession,
                        onSelectWorkspace = ui::selectWorkspace,
                        isWorkspaceCollapsed = { isWorkspaceCollapsed(SessionMode.Code, it) },
                        onToggleWorkspaceCollapsed = { onToggleWorkspaceCollapsed(SessionMode.Code, it) },
                        isWorkspacePinned = { pinnedAtEpochMillis(SessionMode.Code, it) != null },
                        onToggleWorkspacePinned = { onToggleWorkspacePinned(SessionMode.Code, it) },
                        onSelectGlobalSession = { onResumeSession(it.uuid) },
                        onSelectProjectSession = onSelectProjectSession,
                        onSelectLocalAppSession = onSelectLocalAppSession,
                        onNewGlobalSession = onNewGlobalSession,
                        onNewProjectSession = onNewProjectSession,
                        onNewLocalAppSession = onNewLocalAppSession,
                        onContinueSession = onContinueSession,
                        onOpenLocalAppLibrary = { onOpenApps() },
                        onOpenLocalAppDetails = onOpenLocalAppDetails,
                    )
                }

                DrawerSection.Cron -> CronWorkspaceGroupsSection(
                    groups = cronWorkspaceGroups,
                    onOpenCron = onOpenCron,
                    onCreateCron = onCreateCron,
                )
            }
        }

        TerminalShortcut(onClick = onOpenTerminal)
        AccountRow(onClick = onOpenSettings)
    }
}

// MARK: - header ------------------------------------------------------------

@Composable
private fun DrawerHeader(onClose: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(top = 8.dp, bottom = 12.dp),
    ) {
        Text(stringResource(R.string.app_name), color = t.text, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Box(
            modifier = Modifier
                .size(36.dp)
                .clip(CircleShape)
                .clickable(onClick = onClose),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.X,
                size = 20.dp,
                color = t.text3,
                stroke = 1.8f,
                contentDescription = stringResource(R.string.drawer_close_sidebar_a11y),
            )
        }
    }
}

// MARK: - workspace pills ---------------------------------------------------

@Composable
private fun WorkspaceSource(
    workspaces: List<Workspace>?,
    activeWs: String,
    onSelect: (String) -> Unit,
) {
    if (workspaces == null) {
        WorkspaceUnavailableMessage(stringResource(R.string.drawer_workspaces_unavailable))
        return
    }
    if (workspaces.isEmpty()) {
        WorkspaceUnavailableMessage(stringResource(R.string.drawer_no_workspaces))
        return
    }
    Row(
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 18.dp)
            .padding(bottom = 12.dp),
    ) {
        workspaces.forEach { ws ->
            WorkspacePill(ws = ws, active = ws.id == activeWs, onClick = { onSelect(ws.id) })
        }
    }
}

@Composable
private fun WorkspaceUnavailableMessage(message: String) {
    val t = LingXiTheme.palette
    Text(
        message,
        color = t.text4,
        fontSize = 12.sp,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(bottom = 12.dp),
    )
}

@Composable
private fun WorkspacePill(ws: Workspace, active: Boolean, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(50)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier
            .clip(shape)
            .background(if (active) ws.color.tint(0.20f) else t.surface)
            .border(0.5.dp, if (active) ws.color.tint(0.35f) else t.border, shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 7.dp),
    ) {
        Text(ws.icon, color = if (active) ws.color else t.text3, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text(ws.name, color = if (active) ws.color else t.text3, fontSize = 13.sp, fontWeight = FontWeight.Medium)
    }
}

// MARK: - search ------------------------------------------------------------

/**
 * The drawer's real search field — an editable [BasicTextField] whose [query]
 * filters the section lists (chats / projects / crons) live as the user types.
 * Hoisted so the filtering logic lives in [DrawerContent]; a trailing × clears
 * the query when non-empty.
 */
@Composable
private fun SearchBar(query: String, onQueryChange: (String) -> Unit) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(bottom = 14.dp)
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .padding(horizontal = 14.dp, vertical = 11.dp),
    ) {
        LXIcon(name = LXIconName.Search, size = 16.dp, color = t.text4, stroke = 2f)
        Box(modifier = Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text(stringResource(R.string.drawer_search_chats_placeholder), color = t.text4, fontSize = 14.sp)
            }
            BasicTextField(
                value = query,
                onValueChange = onQueryChange,
                singleLine = true,
                textStyle = TextStyle(color = t.text, fontSize = 14.sp),
                cursorBrush = SolidColor(t.accent),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag(UiTags.DRAWER_SEARCH),
            )
        }
        if (query.isNotEmpty()) {
            Box(
                modifier = Modifier
                    .size(20.dp)
                    .clip(CircleShape)
                    .clickable { onQueryChange("") },
                contentAlignment = Alignment.Center,
            ) {
                LXIcon(name = LXIconName.X, size = 13.dp, color = t.text4, stroke = 2f, contentDescription = stringResource(R.string.drawer_clear_search_a11y))
            }
        }
    }
}

// MARK: - section tabs ------------------------------------------------------

@Composable
private fun SectionTabs(
    section: DrawerSection,
    chats: Int,
    projects: Int,
    crons: Int,
    onSelect: (DrawerSection) -> Unit,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(bottom = 8.dp),
    ) {
        SectionTab(DrawerSection.Chat, LXIconName.Message, stringResource(R.string.drawer_tab_chats), chats, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Code, LXIconName.Folder, stringResource(R.string.drawer_tab_projects), projects, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Cron, LXIconName.Clock, stringResource(R.string.drawer_tab_crons), crons, section, onSelect, Modifier.weight(1f))
    }
}

/**
 * The minimum height of a tappable drawer row.
 *
 * 48dp is Android's documented minimum touch target, the same number the local
 * app run surface states for its own controls (`RunPillTapTarget`,
 * LocalAppsScreen.kt). Stated as a height rather than left to the rows'
 * padding: both rows below size themselves from `fontSize` plus a small
 * vertical inset, which lands them near 38dp and 32dp — comfortably legible and
 * comfortably under the minimum.
 */
private val DrawerRowTapTarget = 48.dp

/**
 * The create-app quick actions rendered above the workspace list in the
 * Chat/Code drawer tabs — the Android analog of iOS's `conversationActions`
 * (`Drawer.swift:637-655`), which renders for every section except cron.
 *
 * Before this, the only in-app create/browse entry lived inside the orphaned
 * `DrawerSection.Apps` tab, whose enum case `47d92dc28` deleted, leaving these
 * two rows with zero call sites — a first-time user with no local apps yet had
 * no in-app way to create one. Reusing them here (rather than resurrecting the
 * dedicated tab) mirrors iOS, which never had a standalone Apps tab either.
 *
 * Two affordances, deliberately unequal — the same split iOS's drawer makes
 * (`Drawer.swift`, `dashedButton(drawer_create_app)` beside the library button):
 *
 * - [onCreateApp] is PRIMARY and keeps the filled, accented row. Creating an app
 *   is what a user opens this tab to do, and the create now finishes in the
 *   app's own conversation rather than on a library page.
 * - [onOpenApps] is the browse affordance and stays wired to the library.
 *
 * Both rows are held to [DrawerRowTapTarget]; neither reaches it on its own.
 */
@Composable
private fun DrawerAppQuickActions(onCreateApp: () -> Unit, onOpenApps: () -> Unit) {
    val t = LingXiTheme.palette
    Column(
        verticalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
            // 11dp of vertical padding around a 13sp label measured about 38dp
            // — under the 48dp minimum, and this is the row a user opens this
            // tab to press. [DrawerRowTapTarget] raises the whole node, so the
            // background, the border and the `clickable` hit rect all grow with
            // it (the constraint is applied OUTSIDE them in this chain); the
            // padding stays as the visual inset for anything taller.
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = DrawerRowTapTarget)
                .clip(RoundedCornerShape(10.dp))
                .background(t.surfaceActive)
                .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
                .clickable(onClick = onCreateApp)
                .testTag("tag.drawerCreateApp")
                .padding(horizontal = 14.dp, vertical = 11.dp),
        ) {
            LXIcon(name = LXIconName.Plus, size = 15.dp, color = t.accent, stroke = 1.8f)
            Text(stringResource(R.string.drawer_create_app), color = t.text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        }
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
            // The smaller of the two — 8dp of padding around a 12sp label, about
            // 32dp — and the one with no background at all, so nothing on screen
            // hints at where it can be pressed. Same [DrawerRowTapTarget] as the
            // create row above and as the run surface's `RunPillTapTarget`
            // (LocalAppsScreen.kt): 48dp is Android's documented minimum, and a
            // secondary affordance is not a reason to fall under it.
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = DrawerRowTapTarget)
                .clip(RoundedCornerShape(10.dp))
                .clickable(onClick = onOpenApps)
                .testTag("tag.drawerOpenAppsLibrary")
                .padding(horizontal = 14.dp, vertical = 8.dp),
        ) {
            LXIcon(name = LXIconName.Book, size = 13.dp, color = t.text3, stroke = 1.8f)
            Text(stringResource(R.string.drawer_open_apps_library), color = t.text3, fontSize = 12.sp)
        }
    }
}

@Composable
private fun SectionTab(
    id: DrawerSection,
    icon: LXIconName,
    label: String,
    count: Int,
    current: DrawerSection,
    onSelect: (DrawerSection) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val active = id == current
    val shape = RoundedCornerShape(9.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp, Alignment.CenterHorizontally),
        modifier = modifier
            .clip(shape)
            .background(if (active) t.surfaceActive else Color.Transparent)
            .border(0.5.dp, if (active) t.border else Color.Transparent, shape)
            .clickable { onSelect(id) }
            .testTag(UiTags.drawerTab(id.key))
            .padding(horizontal = 4.dp, vertical = 9.dp),
    ) {
        LXIcon(name = icon, size = 14.dp, color = if (active) t.text else t.text3, stroke = 1.8f)
        Text(
            label,
            color = if (active) t.text else t.text3,
            fontSize = 13.sp,
            fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
        )
        Text(
            count.toString(),
            color = if (active) t.accent else t.text4,
            fontSize = 10.5f.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
private fun DrawerCollectionState(message: String) {
    val t = LingXiTheme.palette
    Text(
        message,
        color = t.text4,
        fontSize = 12.sp,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 12.dp),
    )
}

// MARK: - shortcuts / account -----------------------------------------------

@Composable
private fun TerminalShortcut(onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .testTag(UiTags.DRAWER_TERMINAL)
            .clickable(onClick = onClick)
            .padding(horizontal = 26.dp, vertical = 12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(36.dp)
                .clip(RoundedCornerShape(10.dp))
                .background(t.accent.copy(alpha = 0.14f)),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.Terminal,
                size = 20.dp,
                color = t.accent,
                stroke = 1.7f,
            )
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(stringResource(R.string.settings_linux_section_terminal), color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text("Android Shell", color = t.text4, fontSize = 11.5f.sp)
        }
        LXIcon(
            name = LXIconName.ChevronR,
            size = 16.dp,
            color = t.text4,
            stroke = 1.6f,
        )
    }
}

@Composable
private fun AccountRow(onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .topBorder(t.border)
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .clickable(onClick = onClick)
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
                contentAlignment = Alignment.Center,
            ) {
                Text("Y", color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
            Column(modifier = Modifier.weight(1f)) {
                Text("Yuxin Yang", color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                Text(stringResource(R.string.drawer_account_subtitle_mock), color = t.text4, fontSize = 11.5f.sp)
            }
            LXIcon(
                name = LXIconName.Cog,
                size = 18.dp,
                color = t.text3,
                stroke = 1.6f,
                contentDescription = stringResource(R.string.settings_title_main),
            )
        }
    }
}

// MARK: - search filtering (pure) -------------------------------------------

/**
 * Case-insensitive, whitespace-trimmed drawer search filters — PURE so they are
 * unit-testable on the plain JVM (see `DrawerSearchTest`). An empty/blank query
 * returns the list unchanged; otherwise a row matches if the query is a
 * substring of any of its user-visible text fields.
 *
 * Projects match on their own name/desc OR any contained session's title — a
 * matching project keeps only the sessions that also match (or all sessions when
 * the project name itself matched), so a query never surfaces a project with an
 * empty body.
 */
internal fun filterChats(chats: List<Chat>, query: String): List<Chat> {
    val q = query.trim().lowercase()
    if (q.isEmpty()) return chats
    return chats.filter {
        it.title.lowercase().contains(q) ||
            it.preview.lowercase().contains(q) ||
            it.group.lowercase().contains(q)
    }
}

/**
 * Filter the engine's REAL resumable sessions by the drawer search query —
 * the [SessionRow] analog of [filterChats]. Matches the session title or its
 * relative-time label (so "昨天" / a month-day narrows the list). Empty/blank
 * query is a pass-through. PURE so it is unit-testable on the plain JVM.
 */
internal fun filterSessions(sessions: List<SessionRow>, query: String): List<SessionRow> {
    val q = query.trim().lowercase()
    if (q.isEmpty()) return sessions
    return sessions.filter {
        it.title.lowercase().contains(q) ||
            it.relativeTime.lowercase().contains(q)
    }
}

internal fun filterCrons(crons: List<Cron>, query: String): List<Cron> {
    val q = query.trim().lowercase()
    if (q.isEmpty()) return crons
    return crons.filter {
        it.title.lowercase().contains(q) ||
            it.desc.lowercase().contains(q) ||
            it.cron.lowercase().contains(q)
    }
}

internal fun filterProjects(projects: List<Project>, query: String): List<Project> {
    val q = query.trim().lowercase()
    if (q.isEmpty()) return projects
    return projects.mapNotNull { project ->
        val projectMatches = project.name.lowercase().contains(q) ||
            project.desc.lowercase().contains(q)
        val matchingSessions = project.sessions.filter { s ->
            s.title.lowercase().contains(q) ||
                s.preview.lowercase().contains(q)
        }
        when {
            // Project header matched → keep it with all its sessions.
            projectMatches -> project
            // Only some sessions matched → keep the project narrowed to those.
            matchingSessions.isNotEmpty() -> project.copy(sessions = matchingSessions)
            // No match anywhere → drop the project.
            else -> null
        }
    }
}

/** A 0.5dp hairline drawn along the top edge (the iOS top-`overlay` divider). */
private fun Modifier.topBorder(color: Color): Modifier =
    drawBehind {
        drawLine(
            color = color,
            start = Offset(0f, 0f),
            end = Offset(size.width, 0f),
            strokeWidth = 0.5.dp.toPx(),
        )
    }
