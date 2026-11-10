package com.lingxi.code.tabs

/**
 * Where a deep link lands. [Stay] is a link that opens a shell-level dialog on whatever tab is
 * showing (the import preview); [None] is a route the app does not know.
 */
internal enum class LinkTarget(val key: String) {
    Chats("chats"),
    Bots("bots"),
    Teams("teams"),
    Friends("friends"),
    Settings("settings"),
    Stay("stay"),
    None("none");

    /** The tab to select, or null when the link leaves the selection alone. */
    val tab: AppTab?
        get() = when (this) {
            Chats -> AppTab.Chats
            Bots -> AppTab.Bots
            Teams -> AppTab.Teams
            Friends -> AppTab.Friends
            Settings -> AppTab.Settings
            Stay, None -> null
        }
}

/**
 * The shell's pure rules (SPEC-mobile-team-bot-ui.md section 8.6): no Compose, no Android, so the
 * JVM suite runs them against the table shared with iOS (`mobile-team-bot-vectors.json`).
 */
internal object TabLogic {
    /** Badges stop counting here; the text then reads `99+`. */
    const val BADGE_CAP = 99

    /**
     * Whether the bottom bar (phone) or the rail (wide) is on screen.
     *
     * A full-screen overlay hides both; otherwise the rail is always there, and the bar is there only
     * on a tab's root, without the keyboard, and not while the Chats drawer is open or opening.
     */
    fun tabBarVisible(
        wide: Boolean,
        depth: Int,
        keyboardVisible: Boolean,
        drawerOpen: Boolean,
        selectedTab: AppTab,
        overlayOpen: Boolean,
    ): Boolean = when {
        overlayOpen -> false
        wide -> true
        else -> depth == 0 && !keyboardVisible && !(selectedTab == AppTab.Chats && drawerOpen)
    }

    /** The tab a link route opens, by its first path segment. */
    fun tabForLink(route: String): LinkTarget = when (route.substringBefore('/')) {
        "session", "new", "project", "apps", "market", "runtime", "terminal", "cron" -> LinkTarget.Chats
        "remote" -> LinkTarget.Bots
        "team" -> LinkTarget.Teams
        "friend" -> LinkTarget.Friends
        "settings", "provider", "extensions" -> LinkTarget.Settings
        "import" -> LinkTarget.Stay
        else -> LinkTarget.None
    }

    /** The number a tab badge shows; null hides the badge. */
    fun tabBadgeText(count: Int): String? = when {
        count <= 0 -> null
        count > BADGE_CAP -> "$BADGE_CAP+"
        else -> count.toString()
    }

    /** Chats waiting for the user: the sum over the connected computer's Bots, a negative count counting as 0. */
    fun botsBadgeCount(waitingThreads: Iterable<Int>): Int = waitingThreads.sumOf { it.coerceAtLeast(0) }
}

/**
 * The Settings tab's stack and where Back goes when a jump came from another tab (section 2.6).
 * [stack] holds the pages above the Settings root; [origin] is the tab a jump came from.
 *
 * The state machine is a pure value: the shell mirrors the real navigation stack into it and asks
 * it what a Back press means.
 */
internal data class SettingsJumpState(
    val stack: List<String> = emptyList(),
    val origin: AppTab? = null,
) {
    sealed interface Event {
        /** A shortcut or a link: replaces the stack with [page] and remembers where it came from. */
        data class Jump(val page: String, val origin: AppTab?) : Event

        /** The user opened a page from inside Settings. */
        data class Push(val page: String) : Event

        data object Back : Event

        /** The user tapped a tab: whatever jump brought them here no longer applies. */
        data object UserSelectedTab : Event
    }

    /** [leaveTo] is the tab Back selects (and the stack is then empty); null = stay in Settings. */
    data class Reduction(val state: SettingsJumpState, val leaveTo: AppTab?)

    /**
     * [mainPage] names the Settings root: a jump to it leaves the stack empty. Back at the root with
     * no origin changes nothing: the shell decides (Android selects Chats).
     */
    fun reduce(event: Event, mainPage: String = MAIN_PAGE): Reduction = when (event) {
        is Event.Jump -> Reduction(
            SettingsJumpState(stack = if (event.page == mainPage) emptyList() else listOf(event.page), origin = event.origin),
            leaveTo = null,
        )
        is Event.Push -> Reduction(copy(stack = stack + event.page), leaveTo = null)
        Event.Back -> when {
            stack.size >= 2 -> Reduction(copy(stack = stack.dropLast(1)), leaveTo = null)
            origin != null -> Reduction(SettingsJumpState(), leaveTo = origin)
            else -> Reduction(copy(stack = emptyList()), leaveTo = null)
        }
        Event.UserSelectedTab -> Reduction(copy(origin = null), leaveTo = null)
    }

    companion object {
        /** The root page name used by the shared vectors. */
        const val MAIN_PAGE = "main"
    }
}
