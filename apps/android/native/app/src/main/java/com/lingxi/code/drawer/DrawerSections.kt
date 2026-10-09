package com.lingxi.code.drawer

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.model.Chat
import com.lingxi.code.model.Cron
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme

/**
 * The 对话 / 项目 / 定时 section bodies and their row/card composables — the
 * scrolling middle of the drawer panel. Ported 1:1 from the iOS `Drawer`'s
 * `chatsSection` / `projectsSection` / `cronsSection`, adapted to idiomatic
 * Compose (state hoisted into [DrawerUiState]).
 */

// MARK: - 对话 (chats) -------------------------------------------------------

@Composable
internal fun ChatsSection(
    chats: List<Chat>,
    activeSession: String,
    onSelectSession: (SessionRef) -> Unit,
) {
    val t = LingXiTheme.palette
    val grouped = chats.groupBy { it.group }
    Column(Modifier.fillMaxWidth()) {
        ChatGroupOrder.filter { grouped[it] != null }.forEach { group ->
            Text(
                text = group.uppercase(),
                color = t.text4,
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = 0.6.sp,
                modifier = Modifier
                    .padding(horizontal = 14.dp)
                    .padding(top = 8.dp, bottom = 4.dp),
            )
            grouped[group].orEmpty().forEach { chat ->
                ChatRow(
                    chat = chat,
                    active = chat.id == activeSession,
                    onClick = { onSelectSession(SessionRef(chat.id, chat.title)) },
                )
            }
            Spacer(Modifier.height(8.dp))
        }
        Text(
            text = stringResource(R.string.drawer_chats_temp_notice),
            color = t.text4,
            fontSize = 11.5f.sp,
            lineHeight = (11.5f + 4f).sp,
            modifier = Modifier
                .padding(horizontal = 14.dp)
                .padding(top = 6.dp, bottom = 4.dp),
        )
    }
}

@Composable
private fun ChatRow(chat: Chat, active: Boolean, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 2.dp)
            .height(IntrinsicSize.Min)
            .clip(RoundedCornerShape(10.dp))
            .background(if (active) t.surfaceActive else Color.Transparent)
            .clickable(onClick = onClick),
    ) {
        AccentBar(visible = active, color = t.accent, inset = 12.dp)
        Column(modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
            Text(
                text = chat.title,
                color = if (active) t.text else t.text2,
                fontSize = 14.sp,
                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = "${chat.activity} · ${chat.preview}",
                color = t.text4,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

// MARK: - 对话 (engine sessions) ---------------------------------------------

/**
 * The 对话 tab rendered from the engine's REAL resumable-session catalog
 * ([SessionRow]s), replacing [ChatsSection] when the engine has reported a
 * `SessionList`. One flat, newest-first list (the engine already sorts by mtime
 * desc) — no MockData grouping headers, since the wire rows carry no group.
 * Tapping a row resumes that session by its wire uuid.
 */
@Composable
internal fun EngineSessionsSection(
    state: EngineSessionState,
    activeSession: String,
    onSelectSession: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth()) {
        when {
            state.isLoading -> {
                Text(
                    text = stringResource(R.string.drawer_sessions_loading),
                    color = t.text4,
                    fontSize = 12.sp,
                    modifier = Modifier
                        .padding(horizontal = 14.dp)
                        .padding(top = 8.dp),
                )
                return@Column
            }
            state.isError -> {
                Text(
                    text = state.errorMessage ?: stringResource(R.string.drawer_sessions_load_failed),
                    color = t.statusError,
                    fontSize = 12.sp,
                    lineHeight = 18.sp,
                    modifier = Modifier
                        .padding(horizontal = 14.dp)
                        .padding(top = 8.dp),
                )
                return@Column
            }
            state.isEmpty -> {
                Text(
                    text = stringResource(R.string.drawer_no_sessions),
                    color = t.text4,
                    fontSize = 12.sp,
                    modifier = Modifier
                        .padding(horizontal = 14.dp)
                        .padding(top = 8.dp),
                )
                return@Column
            }
        }
        state.rows.forEach { row ->
            EngineSessionRow(
                row = row,
                active = row.uuid == activeSession,
                onClick = { onSelectSession(row.uuid) },
            )
        }
        if (state.rows.isEmpty()) {
            // The catalog was reported but the search filtered everything out.
            Text(
                text = stringResource(R.string.drawer_no_matching_sessions),
                color = t.text4,
                fontSize = 12.sp,
                modifier = Modifier
                    .padding(horizontal = 14.dp)
                    .padding(top = 8.dp),
            )
        }
    }
}

@Composable
private fun EngineSessionRow(row: SessionRow, active: Boolean, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 2.dp)
            .height(IntrinsicSize.Min)
            .clip(RoundedCornerShape(10.dp))
            .background(if (active) t.surfaceActive else Color.Transparent)
            .clickable(onClick = onClick),
    ) {
        AccentBar(visible = active, color = t.accent, inset = 12.dp)
        Column(modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
            Text(
                text = row.title,
                color = if (active) t.text else t.text2,
                fontSize = 14.sp,
                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = stringResource(R.string.drawer_session_meta, row.relativeTime, row.messageCount),
                color = t.text4,
                fontSize = 12.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * The 2.5dp accent bar pinned to the leading edge of an active row. Reserves its
 * width even when hidden so selecting a row doesn't shift its text — the bar
 * stretches to the row's intrinsic height (caller wraps in
 * `Modifier.height(IntrinsicSize.Min)`).
 */
@Composable
private fun AccentBar(visible: Boolean, color: Color, inset: Dp) {
    Box(
        modifier = Modifier
            .padding(vertical = inset)
            .width(2.5.dp)
            .fillMaxHeight()
            .clip(RoundedCornerShape(2.dp))
            .background(if (visible) color else Color.Transparent),
    )
}

// MARK: - 项目 (projects) ----------------------------------------------------

@Composable
internal fun ProjectsSection(
    projects: List<Project>,
    activeSession: String,
    openProjects: Set<String>,
    onToggleProject: (String) -> Unit,
    onSelectSession: (String, SessionRef) -> Unit,
    onNewSession: (String) -> Unit,
    onCreateProject: () -> Unit,
    onReimportProject: (String) -> Unit,
    onExportProject: (String) -> Unit,
    onReauthorizeProject: (String) -> Unit,
    statusMessage: String? = null,
) {
    Column(
        verticalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
    ) {
        if (statusMessage != null) {
            Text(
                text = statusMessage,
                color = LingXiTheme.palette.accent,
                fontSize = 11.5f.sp,
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp),
            )
        }
        projects.forEach { project ->
            ProjectRow(
                project = project,
                isOpen = project.id in openProjects,
                activeSession = activeSession,
                onToggle = { onToggleProject(project.id) },
                onSelectSession = onSelectSession,
                onNewSession = { onNewSession(project.id) },
                onReimport = { onReimportProject(project.id) },
                onExport = { onExportProject(project.id) },
                onReauthorize = { onReauthorizeProject(project.id) },
            )
        }
        if (projects.isEmpty()) {
            Text(
                text = stringResource(R.string.drawer_no_projects_detail),
                color = LingXiTheme.palette.text4,
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
        DashedButton(label = stringResource(R.string.drawer_new_or_import_project), onClick = onCreateProject)
    }
}

@Composable
private fun ProjectRow(
    project: Project,
    isOpen: Boolean,
    activeSession: String,
    onToggle: () -> Unit,
    onSelectSession: (String, SessionRef) -> Unit,
    onNewSession: () -> Unit,
    onReimport: () -> Unit,
    onExport: () -> Unit,
    onReauthorize: () -> Unit,
) {
    val t = LingXiTheme.palette
    val hasActive = project.sessions.any { it.id == activeSession }
    val chevronRotation by animateFloatAsState(if (isOpen) 90f else 0f, label = "chevron")

    Column {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(10.dp))
                .background(if (hasActive && !isOpen) t.surfaceActive else Color.Transparent)
                .clickable(onClick = onToggle)
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            LXIcon(
                name = LXIconName.ChevronR,
                size = 12.dp,
                color = t.text4,
                stroke = 2f,
                modifier = Modifier.rotate(chevronRotation),
            )
            ProjectIcon(project)
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = project.name,
                    color = t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = project.desc,
                    color = t.text4,
                    fontSize = 11.5f.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                text = project.sessions.size.toString(),
                color = t.text4,
                fontSize = 11.sp,
                fontWeight = FontWeight.Medium,
            )
        }

        AnimatedVisibility(
            visible = isOpen,
            enter = expandVertically() + fadeIn(),
            exit = shrinkVertically() + fadeOut(),
        ) {
            // height(IntrinsicSize.Min) lets the 1dp guide line fillMaxHeight()
            // stretch to the session column's height.
            Row(modifier = Modifier.height(IntrinsicSize.Min)) {
                Box(
                    modifier = Modifier
                        .padding(start = 22.dp, top = 4.dp, bottom = 4.dp)
                        .width(1.dp)
                        .fillMaxHeight()
                        .background(t.border),
                )
                Column {
                    project.sessions.forEach { session ->
                        SessionRow(
                            project = project,
                            session = session,
                            active = session.id == activeSession,
                            onClick = {
                                onSelectSession(project.id, SessionRef(session.id, session.title))
                            },
                        )
                    }
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(5.dp),
                        modifier = Modifier
                            .clickable(onClick = onNewSession)
                            .padding(start = 16.dp, end = 12.dp)
                            .padding(vertical = 7.dp),
                    ) {
                        LXIcon(name = LXIconName.Plus, size = 11.dp, color = t.text4, stroke = 2f)
                        Text(stringResource(R.string.drawer_project_new_session_short), color = t.text4, fontSize = 12.5f.sp)
                    }
                    if (project.storageKind == "saf-mirror") {
                        Row(
                            horizontalArrangement = Arrangement.spacedBy(14.dp),
                            modifier = Modifier.padding(start = 16.dp, end = 12.dp, bottom = 8.dp),
                        ) {
                            // `project.syncState` is compared against the untranslated
                            // enum label `ProjectSyncState.AuthorizationLost.label`
                            // (project/ProjectModels.kt, outside this task's scope) —
                            // translating only this side of the comparison would break
                            // reauthorize/reimport branching in every non-zh locale, so
                            // the comparison literal stays Chinese; only the rendered
                            // button text below is localized.
                            val authorizationLost = project.syncState == "外部目录授权失效"
                            Text(
                                text = if (authorizationLost) {
                                    stringResource(R.string.drawer_reauthorize)
                                } else {
                                    stringResource(R.string.drawer_reimport_short)
                                },
                                color = t.accent,
                                fontSize = 11.5f.sp,
                                modifier = Modifier.clickable(
                                    onClick = if (authorizationLost) onReauthorize else onReimport,
                                ),
                            )
                            Text(
                                text = stringResource(R.string.drawer_export_back_short),
                                color = t.accent,
                                fontSize = 11.5f.sp,
                                modifier = Modifier.clickable(onClick = onExport),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ProjectIcon(project: Project) {
    val t = LingXiTheme.palette
    Box(
        modifier = Modifier
            .size(28.dp)
            .clip(RoundedCornerShape(7.dp))
            .background(project.color.mix(t.surface, amount = 0.18f))
            .border(0.5.dp, project.color.tint(0.28f), RoundedCornerShape(7.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Text(project.icon, color = project.color, fontSize = 13.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
private fun SessionRow(
    project: Project,
    session: ProjectSession,
    active: Boolean,
    onClick: () -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 4.dp)
            .height(IntrinsicSize.Min)
            .clip(RoundedCornerShape(8.dp))
            .background(if (active) t.surfaceActive else Color.Transparent)
            .clickable(onClick = onClick),
    ) {
        AccentBar(visible = active, color = project.color, inset = 8.dp)
        Column(modifier = Modifier.padding(start = 13.5.dp, end = 12.dp).padding(vertical = 8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                if (session.pinned) {
                    LXIcon(name = LXIconName.Pin, size = 11.dp, color = project.color, stroke = 2.2f)
                }
                Text(
                    text = session.title,
                    color = if (active) t.text else t.text2,
                    fontSize = 13.5f.sp,
                    fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                text = stringResource(R.string.drawer_project_session_meta, session.activity, session.msgs),
                color = t.text4,
                fontSize = 11.5f.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

// MARK: - 定时 (crons) -------------------------------------------------------

@Composable
internal fun CronsSection(
    crons: List<Cron>,
    onOpenCron: (String) -> Unit,
    onCreateCron: () -> Unit,
) {
    Column(
        verticalArrangement = Arrangement.spacedBy(6.dp),
        modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
    ) {
        if (crons.isEmpty()) {
            Text(
                text = stringResource(R.string.drawer_no_crons),
                color = LingXiTheme.palette.text4,
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
        crons.forEach { cron ->
            CronCard(cron = cron, onClick = { onOpenCron(cron.id) })
        }
        DashedButton(label = stringResource(R.string.cron_new_task_button), onClick = onCreateCron)
    }
}

@Composable
private fun CronCard(cron: Cron, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
            .alpha(if (cron.enabled) 1f else 0.55f)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            StatusDot(enabled = cron.enabled)
            Text(
                text = cron.title,
                color = t.text,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
        }
        Spacer(Modifier.height(5.dp))
        Text(
            text = cron.desc,
            color = t.text3,
            fontSize = 12.sp,
            lineHeight = (12f + 2f).sp,
        )
        Spacer(Modifier.height(7.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
            Text(
                text = cron.cron,
                color = t.text2,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.Medium,
                modifier = Modifier
                    .clip(RoundedCornerShape(5.dp))
                    .background(t.surfaceActive)
                    .padding(horizontal = 7.dp, vertical = 3.dp),
            )
            Text("→", color = t.text4, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
            Text(
                text = cron.next,
                color = if (cron.enabled) t.accent else t.text4,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.Medium,
            )
        }
    }
}

/** The status dot — accent + a soft halo ring when enabled, else neutral text4. */
@Composable
private fun StatusDot(enabled: Boolean) {
    val t = LingXiTheme.palette
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(14.dp)) {
        if (enabled) {
            Box(
                modifier = Modifier
                    .size(13.dp)
                    .clip(RoundedCornerShape(50))
                    .background(t.accent.tint(0.18f)),
            )
        }
        Box(
            modifier = Modifier
                .size(8.dp)
                .clip(RoundedCornerShape(50))
                .background(if (enabled) t.accent else t.text4),
        )
    }
}

// MARK: - shared ------------------------------------------------------------

/** The dashed "新建…" footer button at the bottom of projects/crons. */
@Composable
internal fun DashedButton(label: String, onClick: () -> Unit = {}) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(10.dp)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 4.dp, vertical = 0.dp)
            .padding(top = 10.dp)
            .clip(shape)
            .dashedBorder(t.border, cornerRadius = 10f)
            .clickable(onClick = onClick)
            .padding(11.dp),
    ) {
        LXIcon(name = LXIconName.Plus, size = 13.dp, color = t.text3, stroke = 2f)
        Text(label, color = t.text3, fontSize = 13.sp)
    }
}

/** Dashed-stroke border drawn behind content (matches the iOS dashed overlay). */
private fun Modifier.dashedBorder(color: Color, cornerRadius: Float): Modifier =
    drawBehind {
        val stroke = Stroke(
            width = 1.dp.toPx(),
            pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx())),
        )
        val r = cornerRadius * density
        drawRoundRect(
            color = color,
            cornerRadius = CornerRadius(r, r),
            style = stroke,
        )
    }
