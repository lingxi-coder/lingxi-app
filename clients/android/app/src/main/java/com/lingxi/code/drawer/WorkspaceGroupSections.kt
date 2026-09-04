package com.lingxi.code.drawer

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.theme.LingXiTheme

private fun SessionMode.forkTargetMode(): SessionMode = when (this) {
    SessionMode.Chat -> SessionMode.Code
    SessionMode.Code -> SessionMode.Chat
}

@Composable
internal fun WorkspaceGroupsSection(
    groups: List<WorkspaceGroup>,
    mode: SessionMode,
    activeWs: String,
    activeSession: String,
    onSelectWorkspace: (String) -> Unit,
    isWorkspaceCollapsed: (String) -> Boolean = { false },
    onToggleWorkspaceCollapsed: (String) -> Unit = {},
    isWorkspacePinned: (String) -> Boolean = { false },
    onToggleWorkspacePinned: (String) -> Unit = {},
    onSelectGlobalSession: (SessionRow) -> Unit,
    onSelectProjectSession: (String, SessionRef) -> Unit,
    onSelectLocalAppSession: (String, SessionRef) -> Unit,
    onNewGlobalSession: () -> Unit,
    onNewProjectSession: (String) -> Unit,
    onNewLocalAppSession: (String) -> Unit,
    onContinueSession: (ConversationScope, SessionRow, SessionMode) -> Unit = { _, _, _ -> },
    onOpenLocalAppLibrary: (String) -> Unit = {},
    onOpenLocalAppDetails: (String) -> Unit = {},
) {
    val t = LingXiTheme.palette
    if (groups.isEmpty()) {
        Text(
            text = if (mode == SessionMode.Chat) {
                stringResource(R.string.drawer_no_matching_sessions)
            } else {
                stringResource(R.string.drawer_no_projects_detail)
            },
            color = t.text4,
            fontSize = 12.sp,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 14.dp, vertical = 12.dp),
        )
        return
    }
    Column(
        verticalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 4.dp),
    ) {
        groups.forEach { group ->
            WorkspaceGroupCard(
                group = group,
                activeWs = activeWs,
                activeSession = activeSession,
                onSelectWorkspace = onSelectWorkspace,
                isCollapsed = isWorkspaceCollapsed(group.stableKey),
                onToggleCollapsed = { onToggleWorkspaceCollapsed(group.stableKey) },
                pinned = isWorkspacePinned(group.stableKey),
                onTogglePinned = { onToggleWorkspacePinned(group.stableKey) },
                onSelectGlobalSession = onSelectGlobalSession,
                onSelectProjectSession = onSelectProjectSession,
                onSelectLocalAppSession = onSelectLocalAppSession,
                onNewGlobalSession = onNewGlobalSession,
                onNewProjectSession = onNewProjectSession,
                onNewLocalAppSession = onNewLocalAppSession,
                onContinueSession = { row ->
                    onContinueSession(group.scope, row, mode.forkTargetMode())
                },
                onOpenLocalAppLibrary = onOpenLocalAppLibrary,
                onOpenLocalAppDetails = onOpenLocalAppDetails,
            )
        }
    }
}

@Composable
private fun WorkspaceGroupCard(
    group: WorkspaceGroup,
    activeWs: String,
    activeSession: String,
    onSelectWorkspace: (String) -> Unit,
    isCollapsed: Boolean,
    onToggleCollapsed: () -> Unit,
    pinned: Boolean,
    onTogglePinned: () -> Unit,
    onSelectGlobalSession: (SessionRow) -> Unit,
    onSelectProjectSession: (String, SessionRef) -> Unit,
    onSelectLocalAppSession: (String, SessionRef) -> Unit,
    onNewGlobalSession: () -> Unit,
    onNewProjectSession: (String) -> Unit,
    onNewLocalAppSession: (String) -> Unit,
    onContinueSession: (SessionRow) -> Unit,
    onOpenLocalAppLibrary: (String) -> Unit,
    onOpenLocalAppDetails: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    val selected = group.stableKey == activeWs
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(if (selected) t.surfaceActive else t.surface)
            .testTag(UiTags.drawerWorkspace(group.stableKey))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clickable { onSelectWorkspace(group.stableKey) },
        ) {
            LXIcon(
                name = when (group.kind) {
                    WorkspaceGroupKind.Global -> LXIconName.Message
                    WorkspaceGroupKind.Project -> LXIconName.Folder
                    WorkspaceGroupKind.LocalApp -> LXIconName.Workflow
                },
                size = 14.dp,
                color = if (selected) t.accent else t.text3,
                stroke = 1.8f,
            )
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = group.name,
                    color = t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                group.status?.takeIf(String::isNotBlank)?.let { status ->
                    Text(
                        text = status,
                        color = t.text4,
                        fontSize = 11.5f.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            if (group.kind == WorkspaceGroupKind.LocalApp) {
                Text(
                    text = stringResource(R.string.drawer_workspace_badge_local_app),
                    color = t.accent,
                    fontSize = 10.5f.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
            WorkspaceActionChip(
                tag = UiTags.drawerWorkspacePin(group.stableKey),
                icon = LXIconName.Pin,
                contentDescription = stringResource(
                    if (pinned) R.string.drawer_workspace_unpin else R.string.drawer_workspace_pin,
                    group.name,
                ),
                onClick = onTogglePinned,
            )
            WorkspaceActionChip(
                tag = UiTags.drawerWorkspaceCollapse(group.stableKey),
                icon = if (isCollapsed) LXIconName.ChevronR else LXIconName.Chevron,
                contentDescription = stringResource(
                    if (isCollapsed) R.string.drawer_workspace_expand else R.string.drawer_workspace_collapse,
                    group.name,
                ),
                onClick = onToggleCollapsed,
            )
            Text(text = group.sessions.size.toString(), color = t.text4, fontSize = 11.sp, fontWeight = FontWeight.Medium)
        }

        Spacer(Modifier.padding(top = 2.dp))

        if (isCollapsed) {
            Unit
        } else if (group.sessions.isEmpty()) {
            Text(
                text = stringResource(R.string.drawer_project_new_session_short),
                color = t.text4,
                fontSize = 12.sp,
                modifier = Modifier
                    .padding(top = 6.dp)
                    .clickable {
                        when (val scope = group.scope) {
                            ConversationScope.Global -> onNewGlobalSession()
                            is ConversationScope.Project -> onNewProjectSession(scope.projectId)
                            is ConversationScope.LocalApp -> onNewLocalAppSession(scope.appId)
                        }
                    },
            )
        } else {
            group.sessions.forEach { row ->
                WorkspaceSessionRow(
                    row = row,
                    active = row.uuid == activeSession,
                    continueTargetMode = row.mode.forkTargetMode(),
                    onClick = {
                        when (val scope = group.scope) {
                            ConversationScope.Global -> onSelectGlobalSession(row)
                            is ConversationScope.Project ->
                                onSelectProjectSession(scope.projectId, SessionRef(row.uuid, row.title))
                            is ConversationScope.LocalApp ->
                                onSelectLocalAppSession(scope.appId, SessionRef(row.uuid, row.title))
                        }
                    },
                    onContinue = { onContinueSession(row) },
                )
            }
            Text(
                text = stringResource(R.string.drawer_project_new_session_short),
                color = t.text4,
                fontSize = 12.sp,
                modifier = Modifier
                    .padding(top = 6.dp)
                    .clickable {
                        when (val scope = group.scope) {
                            ConversationScope.Global -> onNewGlobalSession()
                            is ConversationScope.Project -> onNewProjectSession(scope.projectId)
                            is ConversationScope.LocalApp -> onNewLocalAppSession(scope.appId)
                        }
                    },
            )
        }

        if (group.scope is ConversationScope.LocalApp) {
            LocalAppWorkspaceShortcuts(
                appId = group.scope.appId,
                onOpenLocalAppLibrary = onOpenLocalAppLibrary,
                onOpenLocalAppDetails = onOpenLocalAppDetails,
            )
        }
    }
}

@Composable
private fun WorkspaceActionChip(
    tag: String,
    icon: LXIconName,
    contentDescription: String,
    onClick: () -> Unit,
) {
    val t = LingXiTheme.palette
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(24.dp)
            .clip(CircleShape)
            .testTag(tag)
            .clickable(onClick = onClick),
    ) {
        LXIcon(
            name = icon,
            size = 12.dp,
            color = t.text3,
            stroke = 1.8f,
            contentDescription = contentDescription,
        )
    }
}

@Composable
private fun LocalAppWorkspaceShortcuts(
    appId: String,
    onOpenLocalAppLibrary: (String) -> Unit,
    onOpenLocalAppDetails: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.padding(top = 6.dp),
    ) {
        Text(
            text = stringResource(R.string.drawer_open_apps_library),
            color = t.accent,
            fontSize = 11.5f.sp,
            modifier = Modifier.clickable { onOpenLocalAppLibrary(appId) },
        )
        Text(
            text = stringResource(R.string.session_details_button),
            color = t.accent,
            fontSize = 11.5f.sp,
            modifier = Modifier.clickable { onOpenLocalAppDetails(appId) },
        )
    }
}

@Composable
private fun WorkspaceSessionRow(
    row: SessionRow,
    active: Boolean,
    continueTargetMode: SessionMode,
    onClick: () -> Unit,
    onContinue: () -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(if (active) t.surfaceActive else t.sidebarBg)
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Column {
            Text(
                // The pinned create-interview session carries no other marker
                // in this compact row (unlike the Local Apps screen's own
                // session list, which has room for a separate AssistChip via
                // `LocalAppSessionCard`), so it gets the same badge copy
                // suffixed onto the title instead.
                text = if (row.isInit) {
                    "${row.title} · ${stringResource(R.string.local_apps_session_init_badge)}"
                } else {
                    row.title
                },
                color = if (active) t.text else t.text2,
                fontSize = 13.5f.sp,
                fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = stringResource(R.string.drawer_session_meta, row.relativeTime, row.messageCount),
                color = t.text4,
                fontSize = 11.5f.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = stringResource(
                    R.string.drawer_session_continue_in,
                    stringResource(
                        when (continueTargetMode) {
                            SessionMode.Chat -> R.string.drawer_tab_chats
                            SessionMode.Code -> R.string.drawer_tab_projects
                        },
                    ),
                ),
                color = t.accent,
                fontSize = 11.5f.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .padding(top = 4.dp)
                    .testTag(UiTags.drawerSessionFork(row.uuid, continueTargetMode.wireKey))
                    .clickable(onClick = onContinue),
            )
        }
    }
}

@Composable
internal fun CronWorkspaceGroupsSection(
    groups: List<CronWorkspaceGroup>,
    onOpenCron: (String) -> Unit,
    onCreateCron: () -> Unit,
) {
    val t = LingXiTheme.palette
    Column(
        verticalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 4.dp),
    ) {
        if (groups.all { it.crons.isEmpty() }) {
            Text(
                text = stringResource(R.string.drawer_no_crons),
                color = t.text4,
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
        groups.forEach { group ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(12.dp))
                    .background(t.surface)
                    .padding(horizontal = 12.dp, vertical = 10.dp),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    LXIcon(
                        name = if (group.kind == WorkspaceGroupKind.Global) LXIconName.Clock else LXIconName.Folder,
                        size = 14.dp,
                        color = t.text3,
                        stroke = 1.8f,
                    )
                    Text(
                        text = group.name,
                        color = t.text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                group.crons.forEach { cron ->
                    Text(
                        text = cron.title,
                        color = t.text2,
                        fontSize = 12.5f.sp,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { onOpenCron(cron.id) }
                            .padding(top = 8.dp),
                    )
                }
            }
        }
        DashedButton(label = stringResource(R.string.cron_new_task_button), onClick = onCreateCron)
    }
}
