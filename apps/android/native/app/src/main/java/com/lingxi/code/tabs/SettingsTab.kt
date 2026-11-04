package com.lingxi.code.tabs

import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.cron.CronRunRecord
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.settings.LinuxRuntimeTerminalLaunchRequest
import com.lingxi.code.settings.SettingsHost
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.settings.SettingsStore
import com.lingxi.code.theme.AppearanceStore
import kotlinx.coroutines.flow.first

/**
 * The Settings tab body: today's [SettingsHost], driven by the shell.
 *
 * This is the single place that knows how the shell talks to `SettingsHost` (SPEC-mobile-team-bot-ui
 * section 5). Until the A-SETTINGS slice gives the host its own `jump` / `onOriginBack` /
 * `onDepthChanged` parameters, the shell owns the host's `NavHostController` and does the same
 * things from outside:
 *
 * - a jump replaces the stack: `navigate(route) { popUpTo(MAIN) }`;
 * - the pages above the root are mirrored into the shell (depth for the bar, stack for the origin rule);
 * - system Back on a jumped page (or the root) returns to the tab the jump came from;
 * - closing the root (the X, a Back with nothing to pop) goes to the origin, else to Chats;
 * - a re-tap of the tab pops to the root.
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
    onOpenScheduledSession: (CronRunRecord) -> Unit,
    onSwitchProject: (String) -> Unit,
    onReplayOnboarding: () -> Unit,
    onReconnectEngine: () -> Boolean,
    onOpenTerminal: (LinuxRuntimeTerminalLaunchRequest) -> Unit,
    onPermissionModeChanged: suspend (String) -> Unit,
    onTypescriptLspModeChanged: suspend (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val navController = rememberNavController()

    // How many pages sit above the root: 0 on the root, 1 when the root is right below, 2 beyond that.
    // That is all the shell needs (the bar shows at 0; Back leaves a jump's origin at <= 1).
    val entry by navController.currentBackStackEntryAsState()
    val route = entry?.destination?.route
    val depth = when {
        route == null || route == SettingsRoutes.MAIN -> 0
        navController.previousBackStackEntry?.destination?.route == SettingsRoutes.MAIN -> 1
        else -> 2
    }
    LaunchedEffect(route, depth) {
        state.mirrorSettingsStack(List(depth) { route.orEmpty() })
        state.reportDepth(AppTab.Settings, depth)
    }

    // A jump (deep link, shortcut, account row): replace the stack with the page.
    val jump = state.settingsJump
    LaunchedEffect(navController, jump?.id) {
        if (jump == null) return@LaunchedEffect
        // The host composes its NavHost after measuring its width: wait for the graph first.
        navController.currentBackStackEntryFlow.first()
        if (jump.route == SettingsRoutes.MAIN) {
            navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
        } else {
            navController.navigate(jump.route) {
                launchSingleTop = true
                popUpTo(SettingsRoutes.MAIN) { inclusive = false }
            }
        }
        state.onSettingsJumpHandled(jump.id)
    }

    // Re-tapping the tab pops to the root (scroll-to-top arrives with the A-SETTINGS body).
    OnTabReselected(AppTab.Settings) { pushed ->
        if (pushed > 0) navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
    }

    SettingsHost(
        appearanceStore = appearanceStore,
        engineSource = engineSource,
        projectStore = projectStore,
        isDark = isDark,
        accentId = accentId,
        modifier = modifier,
        navController = navController,
        store = store,
        onClose = state::closeSettings,
        onOpenScheduledSession = onOpenScheduledSession,
        onReplayOnboarding = onReplayOnboarding,
        onReconnectEngine = onReconnectEngine,
        onOpenTerminal = onOpenTerminal,
        onPermissionModeChanged = onPermissionModeChanged,
        onTypescriptLspModeChanged = onTypescriptLspModeChanged,
        onSwitchProject = onSwitchProject,
    )

    // Registered after the host's own handler, so it wins: Back on the page a jump opened (or at the
    // root after a jump to it) leaves for the tab the jump came from, with the stack cleared. Every
    // other press is the host's: pop a page, or `onClose` at the root.
    BackHandler(enabled = state.settingsNav.origin != null && depth <= 1) {
        navController.popBackStack(SettingsRoutes.MAIN, inclusive = false)
        state.settingsBack()
    }
}
