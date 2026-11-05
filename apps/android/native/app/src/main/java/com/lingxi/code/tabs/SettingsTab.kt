package com.lingxi.code.tabs

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.settings.LinuxRuntimeTerminalLaunchRequest
import com.lingxi.code.settings.SettingsHost
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.theme.AppearanceStore

/**
 * The Settings tab body: [SettingsHost] wired to the shell's state (SPEC-mobile-team-bot-ui section 5).
 *
 * The host owns its navigation stack, its jumps, its depth and the meaning of Back; the shell only
 * answers it:
 *
 * - [TabShellState.settingsJump] is the pending request (a link or a shortcut), cleared once the host took it;
 * - [TabShellState.settingsNav]'s origin is the tab a jump came from. The shell clears it when the user
 *   taps a tab; the host asks to leave for it ([TabShellState.closeSettings] selects it and forgets it);
 * - the host's depth is the shell's depth for the Settings tab, which is what shows or hides the bar.
 */
@Composable
internal fun SettingsTab(
    state: TabShellState,
    appearanceStore: AppearanceStore,
    engineSource: ConversationSource?,
    projectStore: ProjectStore?,
    store: SettingsStore,
    isDark: Boolean,
    accentId: String,
    onSwitchProject: (String) -> Unit,
    onReplayOnboarding: () -> Unit,
    onReconnectEngine: () -> Boolean,
    onOpenTerminal: (LinuxRuntimeTerminalLaunchRequest) -> Unit,
    onPermissionModeChanged: suspend (String) -> Unit,
    onTypescriptLspModeChanged: suspend (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    SettingsHost(
        appearanceStore = appearanceStore,
        engineSource = engineSource,
        projectStore = projectStore,
        isDark = isDark,
        accentId = accentId,
        modifier = modifier,
        store = store,
        jump = state.settingsJump,
        onJumpHandled = state::onSettingsJumpHandled,
        origin = state.settingsNav.origin,
        onOriginBack = { state.closeSettings() },
        onDepthChanged = { state.reportDepth(AppTab.Settings, it) },
        onReplayOnboarding = onReplayOnboarding,
        onReconnectEngine = onReconnectEngine,
        onOpenTerminal = onOpenTerminal,
        onPermissionModeChanged = onPermissionModeChanged,
        onTypescriptLspModeChanged = onTypescriptLspModeChanged,
        onSwitchProject = onSwitchProject,
    )
}
