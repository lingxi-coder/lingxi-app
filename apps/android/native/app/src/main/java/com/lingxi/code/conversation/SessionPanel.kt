package com.lingxi.code.conversation

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.modeColor
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.settings.SettingsRow
import com.lingxi.code.settings.SettingsSection
import com.lingxi.code.theme.LingXiTheme

/** Everything the session panel shows about the open conversation. */
@Immutable
internal data class SessionPanelModel(
    val title: String,
    val mode: SessionMode,
    val workspaceName: String,
    val modelName: String,
    /** Effective permission mode, already localized; null when unknown. */
    val permissionLabel: String?,
    val pinned: Boolean,
    /** False for scheduled-task run sessions, which are managed from the task page. */
    val canArchive: Boolean = true,
)

/**
 * ONE panel for the open conversation's settings, opened by tapping the top
 * bar's title. Replaces the scattered per-session controls: mode (changing it
 * forks the conversation), model, permission, pin, make-scheduled, terminal,
 * archive.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SessionPanel(
    model: SessionPanelModel,
    onDismiss: () -> Unit,
    onContinueInOtherMode: () -> Unit,
    onOpenPermissionSettings: () -> Unit,
    onTogglePinned: () -> Unit,
    onMakeScheduled: () -> Unit,
    onOpenTerminal: () -> Unit,
    onArchive: () -> Unit,
) {
    val t = LingXiTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val otherMode = when (model.mode) {
        SessionMode.Chat -> SessionMode.Code
        SessionMode.Code -> SessionMode.Chat
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = t.windowBg,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 16.dp)
                .testTag(UiTags.SESSION_PANEL),
        ) {
            Text(
                text = model.title,
                color = t.text,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = "${sessionModeLabel(model.mode)} · ${model.workspaceName}",
                color = t.modeColor(model.mode),
                fontSize = 12.5f.sp,
                modifier = Modifier.padding(top = 3.dp, bottom = 14.dp),
            )

            SettingsSection {
                SettingsRow(
                    label = stringResource(R.string.cron_mode_label),
                    sub = stringResource(R.string.session_panel_mode_hint),
                    value = sessionModeLabel(model.mode),
                    valueColor = t.modeColor(model.mode),
                    onTap = onContinueInOtherMode,
                    modifier = Modifier.testTag(UiTags.SESSION_PANEL_MODE),
                )
                SettingsRow(
                    label = stringResource(R.string.session_details_model),
                    value = model.modelName,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.settings_parity_permissions),
                    value = model.permissionLabel,
                    isLast = true,
                    onTap = onOpenPermissionSettings,
                )
            }
            SettingsSection {
                SettingsRow(
                    label = stringResource(
                        if (model.pinned) R.string.drawer_session_unpin else R.string.drawer_session_pin,
                    ),
                    icon = LXIconName.Pin,
                    chevron = false,
                    onTap = onTogglePinned,
                    modifier = Modifier.testTag(UiTags.SESSION_PANEL_PIN),
                )
                SettingsRow(
                    label = stringResource(R.string.session_panel_make_scheduled),
                    icon = LXIconName.Clock,
                    onTap = onMakeScheduled,
                    modifier = Modifier.testTag(UiTags.SESSION_PANEL_SCHEDULE),
                )
                SettingsRow(
                    label = stringResource(R.string.settings_linux_section_terminal),
                    icon = LXIconName.Terminal,
                    onTap = onOpenTerminal,
                )
                SettingsRow(
                    label = stringResource(
                        R.string.drawer_session_continue_in,
                        sessionModeLabel(otherMode),
                    ),
                    icon = LXIconName.Share,
                    isLast = true,
                    onTap = onContinueInOtherMode,
                )
            }
            if (model.canArchive) {
                SettingsSection {
                    SettingsRow(
                        label = stringResource(R.string.settings_parity_archive),
                        icon = LXIconName.Folder,
                        danger = true,
                        isLast = true,
                        chevron = false,
                        onTap = onArchive,
                        modifier = Modifier.testTag(UiTags.SESSION_PANEL_ARCHIVE),
                    )
                }
            }
        }
    }
}

/** Localized name of an engine permission-mode id, or null when it is not a known mode. */
@Composable
internal fun permissionModeLabel(mode: String): String? = when (mode) {
    "default" -> stringResource(R.string.settings_permission_mode_default)
    "acceptEdits" -> stringResource(R.string.settings_permission_mode_accept_edits)
    "plan" -> stringResource(R.string.settings_permission_mode_plan)
    "auto" -> stringResource(R.string.settings_permission_mode_auto)
    "dontAsk" -> stringResource(R.string.settings_permission_mode_dont_ask)
    "bypassPermissions" -> stringResource(R.string.settings_permission_mode_bypass)
    else -> null
}

/** Where a new conversation should run: the global workspace or one of the projects. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun WorkspacePickerSheet(
    projects: List<Project>,
    currentScope: ConversationScope,
    onDismiss: () -> Unit,
    onPick: (ConversationScope) -> Unit,
) {
    val t = LingXiTheme.palette
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = t.windowBg,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 16.dp),
        ) {
            Text(
                text = stringResource(R.string.new_chat_pick_workspace_title),
                color = t.text,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.padding(bottom = 14.dp),
            )
            SettingsSection {
                SettingsRow(
                    label = stringResource(R.string.common_global),
                    icon = LXIconName.Message,
                    value = if (currentScope == ConversationScope.Global) "✓" else null,
                    chevron = false,
                    isLast = projects.isEmpty(),
                    onTap = { onPick(ConversationScope.Global) },
                )
                projects.forEachIndexed { index, project ->
                    val target = ConversationScope.Project(project.id)
                    SettingsRow(
                        label = project.name,
                        sub = project.desc.takeIf(String::isNotBlank),
                        icon = LXIconName.Folder,
                        iconColor = project.color,
                        value = if (currentScope == target) "✓" else null,
                        chevron = false,
                        isLast = index == projects.lastIndex,
                        onTap = { onPick(target) },
                    )
                }
            }
        }
    }
}
