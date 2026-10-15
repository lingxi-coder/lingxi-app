package com.lingxi.code.project

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.PageTopBar
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.components.modeColor
import com.lingxi.code.components.tint
import com.lingxi.code.drawer.RecentsFilter
import com.lingxi.code.model.Cron
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.settings.SettingsRow
import com.lingxi.code.settings.SettingsSection
import com.lingxi.code.theme.LingXiTheme

/**
 * Pinned projects first (most recently pinned on top), then by latest activity
 * — the newest session, or the project's own update time when it has none —
 * then by name. PURE, so the order is unit-testable on the plain JVM.
 *
 * [pinnedAtEpochMillis] is keyed by the workspace key (`project.<id>`) the
 * drawer used for workspace pins, so pins made before this page existed keep
 * their meaning.
 */
internal fun sortProjectsForList(
    projects: List<Project>,
    pinnedAtEpochMillis: (String) -> Long?,
): List<Project> = projects.sortedWith(
    compareByDescending<Project> { pinnedAtEpochMillis(projectWorkspaceKey(it.id)) != null }
        .thenByDescending { pinnedAtEpochMillis(projectWorkspaceKey(it.id)) ?: Long.MIN_VALUE }
        .thenByDescending { project ->
            maxOf(
                project.updatedAtEpochMillis,
                project.sessions.maxOfOrNull(ProjectSession::updatedAtEpochMillis) ?: 0L,
            )
        }
        .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name }
        .thenBy { it.id },
)

internal fun projectWorkspaceKey(projectId: String): String = "project.$projectId"

/** The project's sessions for one filter chip, newest first. */
internal fun projectSessionsFor(project: Project, filter: RecentsFilter): List<ProjectSession> =
    project.sessions
        .filter { filter.accepts(it.mode) }
        .sortedByDescending(ProjectSession::updatedAtEpochMillis)

/**
 * The project list page: every project, pinned first, with a ＋ to create or import one.
 *
 * [pinnedWorkspaces] is the pin time per workspace key (`project.<id>`). It is a
 * value, not a lookup function: the sort is remembered against it, and a function
 * capturing the same state holder keeps its identity when a pin changes.
 */
@Composable
fun ProjectsPage(
    projects: List<Project>,
    pinnedWorkspaces: Map<String, Long>,
    statusMessage: String?,
    onBack: () -> Unit,
    onOpenProject: (String) -> Unit,
    onTogglePinned: (String) -> Unit,
    onCreateProject: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val sorted = remember(projects, pinnedWorkspaces) {
        sortProjectsForList(projects) { key -> pinnedWorkspaces[key] }
    }
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .testTag(UiTags.PROJECTS_PAGE),
    ) {
        PageTopBar(
            title = stringResource(R.string.drawer_entry_projects),
            onBack = onBack,
            trailing = {
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(44.dp)
                        .clip(CircleShape)
                        .testTag(UiTags.PROJECTS_CREATE)
                        .clickable(onClick = onCreateProject),
                ) {
                    LXIcon(
                        name = LXIconName.Plus,
                        size = 18.dp,
                        color = t.accent,
                        stroke = 2f,
                        contentDescription = stringResource(R.string.drawer_new_or_import_project),
                    )
                }
            },
        )
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
        ) {
            if (statusMessage != null) {
                Text(
                    text = statusMessage,
                    color = t.accent,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(bottom = 12.dp),
                )
            }
            if (sorted.isEmpty()) {
                Text(
                    text = stringResource(R.string.drawer_no_projects_detail),
                    color = t.text4,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(vertical = 12.dp),
                )
            } else {
                SettingsSection {
                    sorted.forEachIndexed { index, project ->
                        val pinned = projectWorkspaceKey(project.id) in pinnedWorkspaces
                        SettingsRow(
                            label = project.name,
                            sub = project.desc.takeIf(String::isNotBlank),
                            icon = LXIconName.Folder,
                            iconColor = project.color,
                            value = project.sessions.size.toString(),
                            isLast = index == sorted.lastIndex,
                            onTap = { onOpenProject(project.id) },
                            modifier = Modifier.testTag(UiTags.projectRow(project.id)),
                            trailing = {
                                Box(
                                    contentAlignment = Alignment.Center,
                                    modifier = Modifier
                                        .size(40.dp)
                                        .clip(CircleShape)
                                        .clickable { onTogglePinned(project.id) },
                                ) {
                                    LXIcon(
                                        name = LXIconName.Pin,
                                        size = 14.dp,
                                        color = if (pinned) t.accent else t.text4,
                                        stroke = 1.8f,
                                        contentDescription = stringResource(
                                            if (pinned) R.string.drawer_workspace_unpin else R.string.drawer_workspace_pin,
                                            project.name,
                                        ),
                                    )
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

/**
 * One project's page: start a conversation here, the project's conversations
 * (filterable by mode), its scheduled tasks, and the project management actions
 * that used to live on the drawer's workspace cards.
 */
@Composable
fun ProjectPage(
    project: Project,
    crons: List<Cron>,
    activeSession: String,
    onBack: () -> Unit,
    /** Start a conversation here; the mode is the filter chip's, or null to keep the current one. */
    onNewSession: (SessionMode?) -> Unit,
    onOpenSession: (SessionRef) -> Unit,
    onOpenCron: (String) -> Unit,
    onReimport: () -> Unit,
    onExport: () -> Unit,
    onReauthorize: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    var filter by remember { mutableStateOf(RecentsFilter.All) }
    val sessions = remember(project, filter) { projectSessionsFor(project, filter) }
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .testTag(UiTags.PROJECT_PAGE),
    ) {
        PageTopBar(title = project.name, onBack = onBack)
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
        ) {
            if (project.desc.isNotBlank()) {
                Text(project.desc, color = t.text4, style = androidx.compose.material3.MaterialTheme.typography.bodySmall, modifier = Modifier.padding(bottom = 12.dp))
            }
            // The one primary action: a new conversation in this project.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(12.dp))
                    .background(t.accent)
                    .testTag(UiTags.PROJECT_NEW_SESSION)
                    .clickable { onNewSession(filter.mode) }
                    .padding(vertical = 13.dp),
            ) {
                LXIcon(name = LXIconName.Edit, size = 16.dp, color = androidx.compose.material3.MaterialTheme.colorScheme.onPrimary, stroke = 1.9f)
                Text(
                    stringResource(R.string.chat_new_chat),
                    color = androidx.compose.material3.MaterialTheme.colorScheme.onPrimary,
                    style = androidx.compose.material3.MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.SemiBold,
                )
            }

            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.padding(top = 18.dp, bottom = 10.dp),
            ) {
                RecentsFilter.entries.forEach { option ->
                    val tint = when (option) {
                        RecentsFilter.All -> t.text
                        RecentsFilter.Chat -> t.modeColor(SessionMode.Chat)
                        RecentsFilter.Code -> t.modeColor(SessionMode.Code)
                    }
                    val active = option == filter
                    val shape = RoundedCornerShape(50)
                    Text(
                        text = when (option) {
                            RecentsFilter.All -> stringResource(R.string.drawer_filter_all)
                            RecentsFilter.Chat -> stringResource(R.string.session_mode_chat)
                            RecentsFilter.Code -> stringResource(R.string.session_mode_code)
                        },
                        color = if (active) tint else t.text3,
                        style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                        fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                        modifier = Modifier
                            .clip(shape)
                            .background(if (active) tint.copy(alpha = 0.16f) else t.surface)
                            .border(0.5.dp, if (active) tint.copy(alpha = 0.4f) else t.border, shape)
                            .clickable { filter = option }
                            .padding(horizontal = 14.dp, vertical = 7.dp),
                    )
                }
            }

            if (sessions.isEmpty()) {
                Text(
                    text = stringResource(R.string.drawer_no_sessions),
                    color = t.text4,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(vertical = 10.dp),
                )
            } else {
                SettingsSection {
                    sessions.forEachIndexed { index, session ->
                        SettingsRow(
                            label = session.title,
                            sub = stringResource(R.string.drawer_project_session_meta, session.activity, session.msgs),
                            chevron = false,
                            isLast = index == sessions.lastIndex,
                            valueColor = t.modeColor(session.mode),
                            value = when (session.mode) {
                                SessionMode.Chat -> stringResource(R.string.session_mode_chat)
                                SessionMode.Code -> stringResource(R.string.session_mode_code)
                            },
                            onTap = { onOpenSession(SessionRef(session.id, session.title)) },
                            modifier = Modifier
                                .testTag(UiTags.drawerSession(session.id))
                                .background(if (session.id == activeSession) t.surfaceActive else androidx.compose.ui.graphics.Color.Transparent),
                        )
                    }
                }
            }

            if (crons.isNotEmpty()) {
                Spacer16()
                SettingsSection(label = stringResource(R.string.project_page_crons)) {
                    crons.forEachIndexed { index, cron ->
                        SettingsRow(
                            label = cron.title,
                            sub = cron.desc.takeIf(String::isNotBlank),
                            icon = LXIconName.Clock,
                            isLast = index == crons.lastIndex,
                            onTap = { onOpenCron(cron.id) },
                        )
                    }
                }
            }

            if (project.storageKind == "saf-mirror") {
                Spacer16()
                // `syncState` is the untranslated enum label of the sync state;
                // only the rendered button text is localized.
                val authorizationLost = project.syncState == "外部目录授权失效"
                SettingsSection(label = stringResource(R.string.project_page_manage)) {
                    SettingsRow(
                        label = stringResource(
                            if (authorizationLost) R.string.drawer_reauthorize else R.string.drawer_reimport,
                        ),
                        icon = LXIconName.Folder,
                        onTap = if (authorizationLost) onReauthorize else onReimport,
                    )
                    SettingsRow(
                        label = stringResource(R.string.drawer_export_back),
                        icon = LXIconName.Share,
                        isLast = true,
                        onTap = onExport,
                    )
                }
            }
        }
    }
}

@Composable
private fun Spacer16() {
    Box(Modifier.size(16.dp))
}
