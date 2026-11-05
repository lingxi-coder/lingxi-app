package com.lingxi.code.tabs

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import com.lingxi.code.settings.SettingsRoutes
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow

/**
 * Everything the app shell remembers: which tab is selected, how deep each tab is, the requests
 * deep links and shortcuts leave for a tab body, and the Chats chrome that decides whether the bar
 * shows. [MainTabShell] draws it; `MainActivity` and the tab bodies drive it through [TabHost].
 *
 * Only the selected tab (and, while a jump from another tab is pending, Settings' origin) is saved
 * across rotation and process death; a tab's own stack is saved by the tab through
 * `rememberSaveable` inside the shell's `SaveableStateHolder`.
 */
@Stable
class TabShellState(selected: AppTab = AppTab.Chats, settingsOrigin: AppTab? = null) : TabHost {
    private val selectionSequence = java.util.concurrent.atomic.AtomicLong()
    val selectionRevision: Long get() = selectionSequence.get()
    private var selectedState: AppTab by mutableStateOf(selected)
    var selected: AppTab
        get() = selectedState
        private set(value) {
            if (selectedState != value) {
                selectionSequence.incrementAndGet()
                selectedState = value
            }
        }

    private val depths = mutableStateMapOf<AppTab, Int>()

    /** Pushed screens above [tab]'s root as last reported by that tab. */
    fun depthOf(tab: AppTab): Int = depths[tab] ?: 0

    /** The Chats drawer is open or being dragged open (phone only; reported by the drawer). */
    var chatsDrawerOpen: Boolean by mutableStateOf(false)
        private set

    /** A full-screen overlay owned by Chats (an app runtime, the voice flow) is on screen. */
    var chatsFullScreenOverlay: Boolean by mutableStateOf(false)
        private set

    /** What `RootScreen` reports whenever its pushed pages, drawer or full-screen overlays change. */
    fun reportChatsChrome(pushedPages: Int, drawerOpen: Boolean, fullScreenOverlay: Boolean) {
        reportDepth(AppTab.Chats, pushedPages)
        if (chatsDrawerOpen != drawerOpen) chatsDrawerOpen = drawerOpen
        if (chatsFullScreenOverlay != fullScreenOverlay) chatsFullScreenOverlay = fullScreenOverlay
    }

    private var requestSequence = 0L
    private fun nextId(): Long = ++requestSequence

    // --- Settings ---------------------------------------------------------------------------

    /** The pending request for the Settings body; it answers with [onSettingsJumpHandled]. */
    var settingsJump: SettingsJump? by mutableStateOf(null)
        private set

    /** The Settings stack (mirrored from the body) and the tab a jump came from. */
    internal var settingsNav: SettingsJumpState by mutableStateOf(SettingsJumpState(origin = settingsOrigin))
        private set

    fun onSettingsJumpHandled(id: Long) {
        if (settingsJump?.id == id) settingsJump = null
    }

    /** The Settings body reports the pages above its root so that "Back" knows whether it leaves. */
    internal fun mirrorSettingsStack(stack: List<String>) {
        if (settingsNav.stack != stack) settingsNav = settingsNav.copy(stack = stack)
    }

    /**
     * Back inside Settings when a jump remembered its origin: the tab to return to (already
     * selected), or null when the press stays inside Settings. The caller clears the nav stack.
     */
    internal fun settingsBack(): AppTab? {
        val reduction = settingsNav.reduce(SettingsJumpState.Event.Back, SettingsRoutes.MAIN)
        settingsNav = reduction.state
        reduction.leaveTo?.let { selected = it }
        return reduction.leaveTo
    }

    /** "Close" at the Settings root (the X today, a system Back): back to the origin, else to Chats. */
    internal fun closeSettings() {
        val origin = settingsNav.origin
        settingsNav = SettingsJumpState()
        selected = origin ?: AppTab.Chats
    }

    // --- Bots and Teams ---------------------------------------------------------------------

    var botsRequest: BotsRequest? by mutableStateOf(null)
        private set

    var teamsJoinCode: String? by mutableStateOf(null)
        private set

    /** Select Bots and open the computer (or the session on it) a link or a notification names. */
    fun requestBots(hostId: String, sessionId: String?) {
        botsRequest = BotsRequest(nextId(), hostId, sessionId)
        selected = AppTab.Bots
    }

    fun onBotsRequestHandled(id: Long) {
        if (botsRequest?.id == id) botsRequest = null
    }

    /** Select Teams; a [joinCode] prefills the join form (it never joins by itself). */
    fun requestTeams(joinCode: String?) {
        teamsJoinCode = joinCode
        selected = AppTab.Teams
    }

    fun onTeamsJoinCodeHandled() {
        teamsJoinCode = null
    }

    // --- TabHost ----------------------------------------------------------------------------

    override fun select(tab: AppTab) {
        selected = tab
    }

    override fun openSettings(route: String, origin: AppTab?) {
        settingsNav = settingsNav.reduce(SettingsJumpState.Event.Jump(route, origin), SettingsRoutes.MAIN).state
        settingsJump = SettingsJump(nextId(), route, origin)
        selected = AppTab.Settings
    }

    override fun reportDepth(tab: AppTab, depth: Int) {
        val clamped = depth.coerceAtLeast(0)
        if (depths[tab] != clamped) depths[tab] = clamped
    }

    // --- The bar ----------------------------------------------------------------------------

    private val _reselects = MutableSharedFlow<TabReselect>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** Taps on the tab that is already selected; the tab bodies pop to root or scroll to top. */
    val reselects: Flow<TabReselect> get() = _reselects

    /** A tap on the bar or the rail. Whatever jump brought the user to Settings no longer applies. */
    fun onTabClick(tab: AppTab) {
        settingsNav = settingsNav.reduce(SettingsJumpState.Event.UserSelectedTab, SettingsRoutes.MAIN).state
        if (tab == selected) _reselects.tryEmit(TabReselect(tab, depthOf(tab))) else selected = tab
    }

    companion object {
        val Saver: Saver<TabShellState, Any> = listSaver(
            save = { listOf(it.selected.key, it.settingsNav.origin?.key.orEmpty()) },
            restore = { TabShellState(AppTab.fromKey(it[0]) ?: AppTab.Chats, AppTab.fromKey(it[1])) },
        )
    }
}

/** The shell state, saved across configuration changes and process death. */
@Composable
fun rememberTabShellState(): TabShellState = rememberSaveable(saver = TabShellState.Saver) { TabShellState() }
