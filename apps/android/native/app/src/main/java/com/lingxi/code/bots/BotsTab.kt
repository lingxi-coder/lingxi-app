package com.lingxi.code.bots

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.lingxi.code.remote.RemoteOverlay
import com.lingxi.code.remote.RemoteRoute
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.BotsRequest
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.OnTabReselected
import com.lingxi.code.theme.LocalPalette

/**
 * The Bots tab.
 *
 * Shell stub (SPEC-mobile-team-bot-ui section 9.5 step 1): today's Remote Control screens, re-hosted
 * as the tab body. The A-BOTS slice replaces the body with the computers / Bots tree and keeps this
 * signature. [request] is a computer or session a link or a notification named; it is consumed once
 * through [onRequestHandled].
 */
@Composable
fun BotsTab(request: BotsRequest?, onRequestHandled: (Long) -> Unit, modifier: Modifier = Modifier) {
    val host = LocalTabHost.current
    var pending by remember { mutableStateOf<RemoteRoute?>(null) }
    LaunchedEffect(request?.id) {
        if (request != null) {
            pending = request.sessionId?.let { RemoteRoute.Session(request.hostId, it) } ?: RemoteRoute.Host(request.hostId)
            onRequestHandled(request.id)
        }
    }
    // Re-tapping the tab pops to the computers list (scroll-to-top arrives with the real body).
    OnTabReselected(AppTab.Bots) { depth -> if (depth > 0) pending = RemoteRoute.Hosts }
    Box(modifier) {
        RemoteOverlay(
            requestedRoute = pending,
            onRouteHandled = { pending = null },
            isDark = LocalPalette.current.isDark,
            onOpenAccount = { host.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots) },
            onClose = { host.select(AppTab.Chats) },
            onDepthChanged = { host.reportDepth(AppTab.Bots, it) },
        )
    }
}
