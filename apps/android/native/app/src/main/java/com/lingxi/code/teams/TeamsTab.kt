package com.lingxi.code.teams

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.OnTabReselected

/**
 * The Teams tab.
 *
 * Shell stub (SPEC-mobile-team-bot-ui section 9.5 step 1): today's [TeamsScreen], re-hosted as the
 * tab body. The A-TEAMS slice replaces the body with the conversation list and keeps this signature.
 * [joinCode] prefills the join form (it never joins by itself) and is cleared through
 * [onJoinCodeHandled] once used or abandoned.
 */
@Composable
fun TeamsTab(joinCode: String?, onJoinCodeHandled: () -> Unit, modifier: Modifier = Modifier) {
    val host = LocalTabHost.current
    var reset by remember { mutableIntStateOf(0) }
    // Re-tapping the tab pops to the team list: a fresh screen has no open conversation.
    OnTabReselected(AppTab.Teams) { depth -> if (depth > 0) reset++ }
    Box(modifier) {
        key(reset) {
            TeamsScreen(
                initialInvite = joinCode,
                onInviteHandled = onJoinCodeHandled,
                onOpenAccount = { host.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Teams) },
                onClose = { onJoinCodeHandled(); host.select(AppTab.Chats) },
                onDepthChanged = { host.reportDepth(AppTab.Teams, it) },
            )
        }
    }
}
