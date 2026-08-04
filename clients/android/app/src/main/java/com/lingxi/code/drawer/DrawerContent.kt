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
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.model.Chat
import com.lingxi.code.model.Cron
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Project
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
    onOpenApps: () -> Unit = {},
) {
    val t = LingXiTheme.palette

    // Live search query — filters the active workspace's chats / projects / crons
    // (the real editable analog of the prototype's static search pill). Kept as a
    // plain `remember` (transient, like a search box that resets when the drawer
    // closes); the workspace-scoped lists below recompute on every keystroke.
    var query by remember { mutableStateOf("") }

    val filteredEngineSessions = remember(engineSessions.rows, query) {
        filterSessions(engineSessions.rows, query)
    }
    val projects = remember(productionData.projects, ui.activeWs, query) {
        productionData.projects
            ?.let { rows ->
                if (ui.activeWs.isBlank()) rows else rows.filter { it.wsId == ui.activeWs }
            }
            ?.let { filterProjects(it, query) }
    }
    val crons = remember(productionData.crons, ui.activeWs, query) {
        productionData.crons
            ?.let { rows ->
                if (ui.activeWs.isBlank()) rows else rows.filter { it.wsId == ui.activeWs }
            }
            ?.let { filterCrons(it, query) }
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.sidebarBg)
            .windowInsetsPadding(WindowInsets.statusBars),
    ) {
        DrawerHeader(onClose = onClose)
        WorkspaceSource(
            workspaces = productionData.workspaces,
            activeWs = ui.activeWs,
            onSelect = ui::selectWorkspace,
        )
        if (ui.section != DrawerSection.Apps) {
            SearchBar(query = query, onQueryChange = { query = it })
        }
        SectionTabs(
            section = ui.section,
            chats = filteredEngineSessions.size,
            projects = projects?.size ?: 0,
            crons = crons?.size ?: 0,
            apps = appsCount,
            onSelect = { ui.section = it },
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
                DrawerSection.Chats -> EngineSessionsSection(
                    state = engineSessions.copy(rows = filteredEngineSessions),
                    activeSession = ui.activeSession,
                    onSelectSession = { onResumeSession(it) },
                )

                DrawerSection.Projects -> when {
                    projects == null -> DrawerCollectionState("项目数据源尚未接入")
                    else -> ProjectsSection(
                        projects = projects,
                        activeSession = ui.activeSession,
                        openProjects = ui.openProjects,
                        onToggleProject = ui::toggleProject,
                        onSelectSession = onSelectProjectSession,
                        onNewSession = onNewProjectSession,
                        onCreateProject = onCreateProject,
                        onReimportProject = onReimportProject,
                        onExportProject = onExportProject,
                        onReauthorizeProject = onReauthorizeProject,
                        statusMessage = productionData.projectStatusMessage,
                    )
                }

                DrawerSection.Crons -> when {
                    crons == null -> DrawerCollectionState("定时任务数据源尚未接入")
                    else -> CronsSection(
                        crons = crons,
                        onOpenCron = onOpenCron,
                        onCreateCron = onCreateCron,
                    )
                }

                DrawerSection.Apps -> AppsSection(
                    appsCount = appsCount,
                    onOpenApps = onOpenApps,
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
        Text("灵犀", color = t.text, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        Box(
            modifier = Modifier
                .size(36.dp)
                .clip(CircleShape)
                .clickable(onClick = onClose),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.X, size = 20.dp, color = t.text3, stroke = 1.8f, contentDescription = "关闭侧栏")
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
        WorkspaceUnavailableMessage("工作区数据源尚未接入")
        return
    }
    if (workspaces.isEmpty()) {
        WorkspaceUnavailableMessage("暂无工作区")
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
                Text("搜索会话", color = t.text4, fontSize = 14.sp)
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
                LXIcon(name = LXIconName.X, size = 13.dp, color = t.text4, stroke = 2f, contentDescription = "清除搜索")
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
    apps: Int,
    onSelect: (DrawerSection) -> Unit,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(bottom = 8.dp),
    ) {
        SectionTab(DrawerSection.Chats, LXIconName.Message, "对话", chats, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Projects, LXIconName.Folder, "项目", projects, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Crons, LXIconName.Clock, "定时", crons, section, onSelect, Modifier.weight(1f))
        SectionTab(DrawerSection.Apps, LXIconName.Workflow, "应用", apps, section, onSelect, Modifier.weight(1f))
    }
}

@Composable
private fun AppsSection(appsCount: Int, onOpenApps: () -> Unit) {
    val t = LingXiTheme.palette
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 24.dp),
    ) {
        LXIcon(name = LXIconName.Workflow, size = 34.dp, color = t.accent, stroke = 1.7f)
        Text(
            if (appsCount == 0) "还没有本地应用" else "本地应用 $appsCount 个",
            color = t.text,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            "从模板创建、设计、生成并运行本地应用。",
            color = t.text3,
            fontSize = 12.sp,
        )
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(10.dp))
                .background(t.surfaceActive)
                .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
                .clickable(onClick = onOpenApps)
                .padding(horizontal = 14.dp, vertical = 11.dp),
        ) {
            LXIcon(name = LXIconName.Plus, size = 15.dp, color = t.accent, stroke = 1.8f)
            Text("打开应用库", color = t.text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
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
            Text("终端", color = t.text, fontSize = 14.sp, fontWeight = FontWeight.Medium)
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
                Text("Pro · 5.5 / 8 段", color = t.text4, fontSize = 11.5f.sp)
            }
            LXIcon(name = LXIconName.Cog, size = 18.dp, color = t.text3, stroke = 1.6f, contentDescription = "设置")
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
