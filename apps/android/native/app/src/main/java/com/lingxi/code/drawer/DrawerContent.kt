package com.lingxi.code.drawer

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
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
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.modeColor
import com.lingxi.code.model.Chat
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.Workspace
import com.lingxi.code.theme.LingXiTheme
import java.util.TimeZone

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

/** The signed-in account shown on the drawer's bottom row; `null` means signed out. */
@Immutable
data class DrawerAccount(val displayName: String, val email: String?)

/**
 * The mobile drawer: search, two fixed entries (定时任务 · 项目), then ONE
 * recents list that is not split by mode — Chat / Code are only filter chips.
 * Running and awaiting-approval sessions carry a marker at the row's end. The
 * bottom row holds the account, the terminal and settings.
 *
 * Rendered as the `drawerContent` of a Material 3 `ModalNavigationDrawer` (the
 * gesture, scrim and slide are owned by the M3 component, so this is just the
 * 320dp panel body).
 *
 * State is hoisted into [DrawerUiState]. Opening a session calls
 * [onResumeSession] / [onSelectProjectSession], which the root shell uses to
 * rebind the engine to the session's own scope AND mode and to close the
 * drawer. Search matches sessions, projects and scheduled tasks.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DrawerContent(
    ui: DrawerUiState,
    onSelectSession: (SessionRef) -> Unit,
    onOpenSettings: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenTerminal: () -> Unit = {},
    /**
     * The engine's REAL resumable sessions (out-of-band catalog). The recents
     * list renders this state directly and never falls back to mock sessions in
     * production.
     */
    engineSessions: EngineSessionState = EngineSessionState.loading(),
    /** Resume a real engine session by its wire uuid. */
    onResumeSession: (String) -> Unit = { onSelectSession(SessionRef(it, it)) },
    /** Real project/cron/workspace collections. Null collections render unavailable. */
    productionData: DrawerProductionData = DrawerProductionData(),
    onSelectProjectSession: (String, SessionRef) -> Unit = { _, session -> onSelectSession(session) },
    onOpenCron: (String) -> Unit = {},
    onOpenCronList: () -> Unit = {},
    onOpenProjects: () -> Unit = {},
    onOpenProject: (String) -> Unit = {},
    onContinueSession: (ConversationScope, SessionRow, SessionMode) -> Unit = { _, _, _ -> },
    onToggleSessionPinned: (String) -> Unit = {},
    /**
     * The engine is bound to one session at a time, so only the visible session
     * can be running or waiting for the user's approval.
     */
    runningSessionId: String? = null,
    awaitingSessionId: String? = null,
    /** Scheduled-task results the user has not looked at yet. */
    newCronResults: Int = 0,
    account: DrawerAccount? = null,
) {
    val t = LingXiTheme.palette

    // Live search query — filters sessions, projects and scheduled tasks. A plain
    // `remember` (transient, like a search box that resets when the drawer
    // closes); the lists below recompute on every keystroke.
    var query by remember { mutableStateOf("") }
    val searching = query.isNotBlank()

    val globalName = stringResource(R.string.drawer_workspace_global)
    val projects = productionData.projects.orEmpty()
    val nowSeconds = System.currentTimeMillis() / 1000L
    val utcOffsetSeconds = TimeZone.getDefault().getOffset(nowSeconds * 1000L) / 1000
    // The local calendar day is part of the key so "今天" rolls over at midnight
    // even if no session data changed while the drawer stayed composed.
    val localDay = Math.floorDiv(nowSeconds + utcOffsetSeconds, 86_400L)
    val groups = remember(
        engineSessions.rows,
        projects,
        ui.filter,
        query,
        ui.pinnedSessions,
        runningSessionId,
        awaitingSessionId,
        localDay,
    ) {
        buildRecentGroups(
            globalName = globalName,
            globalSessions = engineSessions.rows,
            projects = projects,
            filter = ui.filter,
            query = query,
            pinnedSessions = ui.pinnedSessions,
            statusFor = { id ->
                when (id) {
                    awaitingSessionId -> RecentStatus.AwaitingConfirmation
                    runningSessionId -> RecentStatus.Running
                    else -> RecentStatus.Idle
                }
            },
            nowEpochSeconds = nowSeconds,
            utcOffsetSeconds = utcOffsetSeconds,
        )
    }
    val matchingProjects = remember(projects, query) {
        if (query.isBlank()) emptyList() else projects.filter { project ->
            project.name.contains(query.trim(), ignoreCase = true) ||
                project.desc.contains(query.trim(), ignoreCase = true)
        }
    }
    val matchingCrons = remember(productionData.crons, query) {
        if (query.isBlank()) emptyList() else filterCrons(productionData.crons.orEmpty(), query)
    }

    fun open(session: RecentSession) {
        ui.selectSession(session.uuid)
        when (val scope = session.scope) {
            ConversationScope.Global, ConversationScope.Scheduled -> onResumeSession(session.uuid)
            is ConversationScope.Project ->
                onSelectProjectSession(scope.projectId, SessionRef(session.uuid, session.title))
        }
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.sidebarBg)
            .windowInsetsPadding(WindowInsets.statusBars),
    ) {
        DrawerHeader(onClose = onClose)
        SearchBar(query = query, onQueryChange = { query = it })

        if (!searching) {
            EntryRow(
                icon = LXIconName.Clock,
                label = stringResource(R.string.settings_title_cron),
                badge = newCronResults.takeIf { it > 0 }
                    ?.let { stringResource(R.string.drawer_cron_new_results, it) },
                tag = UiTags.DRAWER_ENTRY_CRON,
                onClick = onOpenCronList,
            )
            EntryRow(
                icon = LXIconName.Folder,
                label = stringResource(R.string.drawer_entry_projects),
                count = projects.size,
                tag = UiTags.DRAWER_ENTRY_PROJECTS,
                onClick = onOpenProjects,
            )
            FilterChips(selected = ui.filter, onSelect = { ui.filter = it })
        }

        // Scrolling body — fills the space above the bottom bar.
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp)
                .padding(top = 4.dp, bottom = 8.dp),
        ) {
            if (searching) {
                SearchResults(
                    groups = groups,
                    projects = matchingProjects,
                    crons = matchingCrons,
                    activeSession = ui.activeSession,
                    onOpenSession = ::open,
                    onOpenProject = onOpenProject,
                    onOpenCron = onOpenCron,
                    isPinned = { ui.sessionPinnedAt(it) != null },
                    onTogglePinned = onToggleSessionPinned,
                    onContinue = onContinueSession,
                )
            } else {
                RecentList(
                    groups = groups,
                    state = engineSessions,
                    activeSession = ui.activeSession,
                    onOpenSession = ::open,
                    isPinned = { ui.sessionPinnedAt(it) != null },
                    onTogglePinned = onToggleSessionPinned,
                    onContinue = onContinueSession,
                )
            }
        }

        BottomBar(
            account = account,
            onOpenAccount = onOpenSettings,
            onOpenTerminal = onOpenTerminal,
            onOpenSettings = onOpenSettings,
        )
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

// MARK: - search ------------------------------------------------------------

/**
 * The drawer's real search field — an editable [BasicTextField] whose [query]
 * filters sessions, projects and scheduled tasks live as the user types.
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
            .padding(bottom = 10.dp)
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .padding(horizontal = 14.dp, vertical = 11.dp),
    ) {
        LXIcon(name = LXIconName.Search, size = 16.dp, color = t.text4, stroke = 2f)
        Box(modifier = Modifier.weight(1f)) {
            if (query.isEmpty()) {
                Text(stringResource(R.string.drawer_search_all_placeholder), color = t.text4, fontSize = 14.sp, maxLines = 1)
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

// MARK: - entries + filter --------------------------------------------------

/** One of the fixed destinations above the recents list. */
@Composable
private fun EntryRow(
    icon: LXIconName,
    label: String,
    tag: String,
    onClick: () -> Unit,
    count: Int? = null,
    badge: String? = null,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 10.dp)
            .clip(RoundedCornerShape(10.dp))
            .testTag(tag)
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 11.dp),
    ) {
        LXIcon(name = icon, size = 18.dp, color = t.text2, stroke = 1.7f)
        Text(label, color = t.text, fontSize = 15.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
        if (badge != null) {
            Text(
                badge,
                color = t.accent,
                fontSize = 11.5f.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(50))
                    .background(t.accent.copy(alpha = 0.14f))
                    .padding(horizontal = 8.dp, vertical = 2.dp),
            )
        } else if (count != null) {
            Text(count.toString(), color = t.text4, fontSize = 12.sp, fontWeight = FontWeight.Medium)
        }
    }
}

@Composable
private fun FilterChips(selected: RecentsFilter, onSelect: (RecentsFilter) -> Unit) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 18.dp)
            .padding(top = 10.dp, bottom = 6.dp),
    ) {
        RecentsFilter.entries.forEach { filter ->
            FilterChip(
                label = when (filter) {
                    RecentsFilter.All -> stringResource(R.string.drawer_filter_all)
                    RecentsFilter.Chat -> stringResource(R.string.session_mode_chat)
                    RecentsFilter.Code -> stringResource(R.string.session_mode_code)
                },
                tint = when (filter) {
                    RecentsFilter.All -> LingXiTheme.palette.text
                    RecentsFilter.Chat -> LingXiTheme.palette.modeColor(SessionMode.Chat)
                    RecentsFilter.Code -> LingXiTheme.palette.modeColor(SessionMode.Code)
                },
                active = filter == selected,
                onClick = { onSelect(filter) },
                modifier = Modifier.testTag(UiTags.drawerFilter(filter.key)),
            )
        }
    }
}

@Composable
private fun FilterChip(
    label: String,
    tint: Color,
    active: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(50)
    Text(
        text = label,
        color = if (active) tint else t.text3,
        fontSize = 13.sp,
        fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
        modifier = modifier
            .clip(shape)
            .background(if (active) tint.copy(alpha = 0.16f) else t.surface)
            .border(0.5.dp, if (active) tint.copy(alpha = 0.4f) else t.border, shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 7.dp),
    )
}

// MARK: - recents -----------------------------------------------------------

@Composable
private fun bucketTitle(bucket: RecentBucket): String = stringResource(
    when (bucket) {
        RecentBucket.Pinned -> R.string.drawer_recents_pinned
        RecentBucket.Today -> R.string.drawer_recents_today
        RecentBucket.Yesterday -> R.string.session_relative_yesterday
        RecentBucket.LastWeek -> R.string.drawer_recents_week
        RecentBucket.Earlier -> R.string.drawer_recents_earlier
    },
)

@Composable
private fun RecentList(
    groups: List<RecentGroup>,
    state: EngineSessionState,
    activeSession: String,
    onOpenSession: (RecentSession) -> Unit,
    isPinned: (String) -> Boolean,
    onTogglePinned: (String) -> Unit,
    onContinue: (ConversationScope, SessionRow, SessionMode) -> Unit,
) {
    val t = LingXiTheme.palette
    if (groups.isEmpty()) {
        Text(
            text = when {
                state.isLoading -> stringResource(R.string.drawer_sessions_loading)
                state.isError -> state.errorMessage ?: stringResource(R.string.drawer_sessions_load_failed)
                else -> stringResource(R.string.drawer_no_sessions)
            },
            color = t.text4,
            fontSize = 12.sp,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 10.dp, vertical = 12.dp),
        )
        return
    }
    groups.forEach { group ->
        SectionHeader(bucketTitle(group.bucket))
        group.sessions.forEach { session ->
            RecentRow(
                session = session,
                active = session.uuid == activeSession,
                pinned = isPinned(session.uuid),
                onClick = { onOpenSession(session) },
                onTogglePinned = { onTogglePinned(session.uuid) },
                onContinue = onContinue,
            )
        }
    }
}

@Composable
private fun SectionHeader(title: String) {
    val t = LingXiTheme.palette
    Text(
        text = title,
        color = t.text4,
        fontSize = 11.5f.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(start = 10.dp, top = 12.dp, bottom = 4.dp),
    )
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun RecentRow(
    session: RecentSession,
    active: Boolean,
    pinned: Boolean,
    onClick: () -> Unit,
    onTogglePinned: () -> Unit,
    onContinue: (ConversationScope, SessionRow, SessionMode) -> Unit,
) {
    val t = LingXiTheme.palette
    var menuOpen by remember { mutableStateOf(false) }
    val otherMode = when (session.mode) {
        SessionMode.Chat -> SessionMode.Code
        SessionMode.Code -> SessionMode.Chat
    }
    Box {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(10.dp))
                .background(if (active) t.surfaceActive else Color.Transparent)
                .testTag(UiTags.drawerSession(session.uuid))
                .combinedClickable(onClick = onClick, onLongClick = { menuOpen = true })
                .padding(horizontal = 10.dp, vertical = 9.dp),
        ) {
            // Mode dot: the session's mode, in the mode's palette color.
            Box(
                modifier = Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(t.modeColor(session.mode)),
            )
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = session.title,
                    color = if (active) t.text else t.text2,
                    fontSize = 14.sp,
                    fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = listOf(session.workspaceName, session.relativeTime)
                        .filter(String::isNotBlank)
                        .joinToString(" · "),
                    color = t.text4,
                    fontSize = 11.5f.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            when (session.status) {
                RecentStatus.Running -> StatusPill(stringResource(R.string.chat_status_running), t.accent)
                RecentStatus.AwaitingConfirmation ->
                    StatusPill(stringResource(R.string.drawer_status_awaiting), t.statusTesting)
                RecentStatus.Idle -> if (pinned) {
                    LXIcon(name = LXIconName.Pin, size = 12.dp, color = t.text4, stroke = 1.8f)
                }
            }
        }
        DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
            DropdownMenuItem(
                text = {
                    Text(
                        stringResource(if (pinned) R.string.drawer_session_unpin else R.string.drawer_session_pin),
                    )
                },
                onClick = {
                    menuOpen = false
                    onTogglePinned()
                },
            )
            DropdownMenuItem(
                text = {
                    Text(
                        stringResource(
                            R.string.drawer_session_continue_in,
                            stringResource(
                                when (otherMode) {
                                    SessionMode.Chat -> R.string.session_mode_chat
                                    SessionMode.Code -> R.string.session_mode_code
                                },
                            ),
                        ),
                    )
                },
                modifier = Modifier.testTag(UiTags.drawerSessionFork(session.uuid, otherMode.wireKey)),
                onClick = {
                    menuOpen = false
                    onContinue(
                        session.scope,
                        SessionRow(
                            uuid = session.uuid,
                            title = session.title,
                            messageCount = session.messageCount,
                            relativeTime = session.relativeTime,
                            mode = session.mode,
                            modifiedAtEpochSeconds = session.modifiedAtEpochSeconds,
                        ),
                        otherMode,
                    )
                },
            )
        }
    }
}

@Composable
private fun StatusPill(label: String, color: Color) {
    Text(
        text = label,
        color = color,
        fontSize = 10.5f.sp,
        fontWeight = FontWeight.SemiBold,
        maxLines = 1,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(color.copy(alpha = 0.14f))
            .padding(horizontal = 7.dp, vertical = 2.dp),
    )
}

// MARK: - search results ----------------------------------------------------

@Composable
private fun SearchResults(
    groups: List<RecentGroup>,
    projects: List<Project>,
    crons: List<Cron>,
    activeSession: String,
    onOpenSession: (RecentSession) -> Unit,
    onOpenProject: (String) -> Unit,
    onOpenCron: (String) -> Unit,
    isPinned: (String) -> Boolean,
    onTogglePinned: (String) -> Unit,
    onContinue: (ConversationScope, SessionRow, SessionMode) -> Unit,
) {
    val t = LingXiTheme.palette
    if (groups.isEmpty() && projects.isEmpty() && crons.isEmpty()) {
        Text(
            text = stringResource(R.string.drawer_no_match),
            color = t.text4,
            fontSize = 12.sp,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 10.dp, vertical = 12.dp),
        )
        return
    }
    if (groups.isNotEmpty()) {
        SectionHeader(stringResource(R.string.drawer_search_section_sessions))
        groups.flatMap { it.sessions }.forEach { session ->
            RecentRow(
                session = session,
                active = session.uuid == activeSession,
                pinned = isPinned(session.uuid),
                onClick = { onOpenSession(session) },
                onTogglePinned = { onTogglePinned(session.uuid) },
                onContinue = onContinue,
            )
        }
    }
    if (projects.isNotEmpty()) {
        SectionHeader(stringResource(R.string.drawer_entry_projects))
        projects.forEach { project ->
            ResultRow(
                icon = LXIconName.Folder,
                title = project.name,
                subtitle = project.desc,
                onClick = { onOpenProject(project.id) },
            )
        }
    }
    if (crons.isNotEmpty()) {
        SectionHeader(stringResource(R.string.settings_title_cron))
        crons.forEach { cron ->
            ResultRow(
                icon = LXIconName.Clock,
                title = cron.title,
                subtitle = cron.next,
                onClick = { onOpenCron(cron.id) },
            )
        }
    }
}

@Composable
private fun ResultRow(icon: LXIconName, title: String, subtitle: String, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 9.dp),
    ) {
        LXIcon(name = icon, size = 15.dp, color = t.text3, stroke = 1.7f)
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = t.text2, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle.isNotBlank()) {
                Text(subtitle, color = t.text4, fontSize = 11.5f.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

// MARK: - bottom bar --------------------------------------------------------

/** Account (real Firebase profile), terminal and settings. */
@Composable
private fun BottomBar(
    account: DrawerAccount?,
    onOpenAccount: () -> Unit,
    onOpenTerminal: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        modifier = Modifier
            .fillMaxWidth()
            .topBorder(t.border)
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .weight(1f)
                .clip(RoundedCornerShape(12.dp))
                .testTag(UiTags.DRAWER_ACCOUNT)
                .clickable(onClick = onOpenAccount)
                .padding(horizontal = 8.dp, vertical = 6.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(Brush.linearGradient(listOf(t.accent, t.accent2))),
                contentAlignment = Alignment.Center,
            ) {
                val initial = account?.displayName?.trim()?.firstOrNull()?.uppercase()
                if (initial != null) {
                    Text(initial, color = Color.White, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                } else {
                    LXIcon(name = LXIconName.Sparkle, size = 16.dp, color = Color.White, stroke = 1.8f)
                }
            }
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = account?.displayName ?: stringResource(R.string.drawer_account_signed_out_title),
                    color = t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = account?.email ?: stringResource(R.string.drawer_account_signed_out_subtitle),
                    color = t.text4,
                    fontSize = 11.5f.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        BottomIconButton(
            icon = LXIconName.Terminal,
            description = stringResource(R.string.settings_linux_section_terminal),
            tag = UiTags.DRAWER_TERMINAL,
            onClick = onOpenTerminal,
        )
        BottomIconButton(
            icon = LXIconName.Cog,
            description = stringResource(R.string.settings_title_main),
            tag = UiTags.DRAWER_SETTINGS,
            onClick = onOpenSettings,
        )
    }
}

@Composable
private fun BottomIconButton(icon: LXIconName, description: String, tag: String, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .testTag(tag)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(name = icon, size = 20.dp, color = t.text2, stroke = 1.7f, contentDescription = description)
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
