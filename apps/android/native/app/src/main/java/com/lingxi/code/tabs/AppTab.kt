package com.lingxi.code.tabs

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import com.lingxi.code.settings.SettingsRoutes
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow

/*
 * The contracts every slice of the Bot / Team redesign compiles against
 * (SPEC-mobile-team-bot-ui.md section 9.2). The shell slice owns this file; a
 * slice that needs more asks for it here instead of editing the shell.
 */

/** The four top-level destinations of the app shell. [key] is stable: it names the saved state and the test tags. */
enum class AppTab(val key: String) {
    Chats("chats"),
    Bots("bots"),
    Teams("teams"),
    Settings("settings");

    companion object {
        fun fromKey(key: String?): AppTab? = entries.firstOrNull { it.key == key }
    }
}

/** True at >= 840 dp. Measured once, by [MainTabShell] (decision D3); nobody else measures the window for this. */
val LocalWideLayout = staticCompositionLocalOf { false }

/** What a screen can ask of the shell; implemented by [TabShellState], read through [LocalTabHost]. */
interface TabHost {
    /** Programmatic selection (a closer that used to dismiss an overlay). A tap on the bar goes through the shell. */
    fun select(tab: AppTab)

    /**
     * The Jump API of section 5: select Settings, replace its stack with [route] (the root for
     * [SettingsRoutes.MAIN]) and remember [origin] (the caller's tab) so that Back returns there.
     */
    fun openSettings(route: String = SettingsRoutes.MAIN, origin: AppTab? = null)

    /** Pushed screens above [tab]'s root; 0 = on the root. The bar is only shown on roots. */
    fun reportDepth(tab: AppTab, depth: Int)
}

val LocalTabHost = staticCompositionLocalOf<TabHost> { error("TabHost not provided") }

/** A request to land on a Settings page; the Settings body consumes it once through `onJumpHandled(id)`. */
data class SettingsJump(val id: Long, val route: String, val origin: AppTab?)

/** A request to open a computer, or one of its sessions, in the Bots tab; consumed once through `onRequestHandled(id)`. */
data class BotsRequest(val id: Long, val hostId: String, val sessionId: String?)

/** The user tapped the already selected tab; [depth] is how many screens were pushed on it at that moment. */
data class TabReselect(val tab: AppTab, val depth: Int)

/** Re-tap events of the shell; the default is a flow that never emits (a screen hosted outside the shell). */
val LocalTabReselects = staticCompositionLocalOf<Flow<TabReselect>> { emptyFlow() }

/**
 * Runs [action] when the user taps [tab] while it is already selected: pop to the root when [depth] > 0,
 * otherwise scroll to the top (Bots / Teams / Settings; Chats ignores it). Section 2.3.
 */
@Composable
fun OnTabReselected(tab: AppTab, action: (depth: Int) -> Unit) {
    val reselects = LocalTabReselects.current
    val latest = rememberUpdatedState(action)
    LaunchedEffect(reselects, tab) {
        reselects.collect { if (it.tab == tab) latest.value(it.depth) }
    }
}
