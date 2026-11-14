package com.lingxi.code.tabs

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarDefaults
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailDefaults
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.navigationevent.compose.LocalNavigationEventDispatcherOwner
import androidx.navigationevent.compose.rememberNavigationEventDispatcherOwner
import com.lingxi.code.R
import com.lingxi.code.bots.BotsBadge
import com.lingxi.code.bots.BotsTab
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.friends.FriendsBadge
import com.lingxi.code.friends.FriendsTab
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.teams.LocalOpenVoiceSettings
import com.lingxi.code.teams.TeamsBadge
import com.lingxi.code.teams.TeamsTab
import kotlinx.coroutines.flow.StateFlow

/** The one wide breakpoint (decision D3): the rail, the persistent drawer and every two-pane tab switch here. */
internal val WideBreakpoint = 840.dp

/**
 * The app shell: five tabs behind a Material 3 bottom bar (a rail from [WideBreakpoint] up).
 *
 * - **Chats** ([chats], the existing `RootScreen`) is always composed: it owns the live engine, the
 *   transcript and the drawer. When another tab is selected it is still measured but not placed,
 *   has no semantics, and its `BackHandler`s are parked on an inert dispatcher.
 * - **Bots / Teams / Friends / Settings** are composed while selected and restored through a
 *   `SaveableStateHolder`, so each keeps its own stack.
 * - The bar shows by [TabLogic.tabBarVisible]; its height is subtracted from the bottom system-bar
 *   inset that the content below would otherwise pad a second time.
 * - Back: the selected tab's own handlers first, then "select Chats"; Chats keeps its behaviour.
 *
 * [overlayOpen] is true while a full-screen overlay drawn above the shell (onboarding, the
 * terminal, the import preview) covers the bar.
 */
@Composable
fun MainTabShell(
    state: TabShellState,
    overlayOpen: Boolean,
    chats: @Composable () -> Unit,
    settings: @Composable (Modifier) -> Unit,
    modifier: Modifier = Modifier,
    bots: @Composable (Modifier) -> Unit = { BotsTab(state.botsRequest, state::onBotsRequestHandled, it) },
    teams: @Composable (Modifier) -> Unit = { TeamsTab(state.teamsJoinCode, state::onTeamsJoinCodeHandled, it) },
    friends: @Composable (Modifier) -> Unit = { FriendsTab(state.friendsRequest, state::onFriendsRequestHandled, it) },
    botsBadge: StateFlow<Int> = BotsBadge.count,
    teamsBadge: StateFlow<Int> = TeamsBadge.count,
    friendsBadge: StateFlow<Int> = FriendsBadge.count,
) {
    val botsCount by botsBadge.collectAsState()
    val teamsCount by teamsBadge.collectAsState()
    val friendsCount by friendsBadge.collectAsState()
    BoxWithConstraints(modifier.fillMaxSize()) {
        val wide = maxWidth >= WideBreakpoint
        val density = LocalDensity.current
        val ime = WindowInsets.ime
        val keyboardVisible by remember(ime, density) { derivedStateOf { ime.getBottom(density) > 0 } }
        val selected = state.selected
        val fullScreenOverlay = overlayOpen || (selected == AppTab.Chats && state.chatsFullScreenOverlay)
        val barVisible = TabLogic.tabBarVisible(
            wide = wide,
            depth = state.depthOf(selected),
            keyboardVisible = keyboardVisible,
            drawerOpen = state.chatsDrawerOpen,
            selectedTab = selected,
            overlayOpen = fullScreenOverlay,
        )

        // The bar pads the bottom system-bar inset itself, so the content above it must not pad it
        // again: it consumes as much of that inset as the bar currently covers (a smooth hand-over
        // while the bar slides in or out). Next to the rail the content consumes the start inset.
        var barHeightPx by remember { mutableIntStateOf(0) }
        val navigationBarInsets by rememberUpdatedState(NavigationBarDefaults.windowInsets)
        val railStartInsets by rememberUpdatedState(NavigationRailDefaults.windowInsets.only(WindowInsetsSides.Start))
        val phoneLayout by rememberUpdatedState(!wide)
        val railShown by rememberUpdatedState(wide && barVisible)
        val barConsumed = remember {
            BarConsumedInsets(navigationBar = { navigationBarInsets }, barHeightPx = { if (phoneLayout) barHeightPx else 0 })
        }
        val railConsumed = remember { RailConsumedInsets(railStart = { railStartInsets }, shown = { railShown }) }

        CompositionLocalProvider(
            LocalWideLayout provides wide,
            LocalTabHost provides state,
            LocalTabReselects provides state.reselects,
        ) {
            // Only roots leave their tab. Pushed screens and overlays own Back independently of
            // callback registration order across recompositions.
            BackHandler(enabled = selected != AppTab.Chats && state.depthOf(selected) == 0 && !fullScreenOverlay) {
                state.select(AppTab.Chats)
            }
            Row(Modifier.fillMaxSize()) {
                if (wide && barVisible) {
                    TabRail(selected, botsCount, teamsCount, friendsCount, state::onTabClick)
                }
                Column(Modifier.weight(1f).fillMaxHeight()) {
                    TabBodies(
                        state = state,
                        chats = chats,
                        settings = settings,
                        bots = bots,
                        teams = teams,
                        friends = friends,
                        modifier = Modifier
                            .weight(1f)
                            .fillMaxWidth()
                            .consumeWindowInsets(railConsumed)
                            .consumeWindowInsets(barConsumed),
                    )
                    if (!wide) {
                        PhoneBar(
                            visible = barVisible,
                            keyboardVisible = keyboardVisible,
                            selected = selected,
                            botsCount = botsCount,
                            teamsCount = teamsCount,
                            friendsCount = friendsCount,
                            onSelect = state::onTabClick,
                            onHeightChanged = { barHeightPx = it },
                        )
                    }
                }
            }
        }
    }
}

// --- bodies -------------------------------------------------------------------------------------

@Composable
private fun TabBodies(
    state: TabShellState,
    chats: @Composable () -> Unit,
    settings: @Composable (Modifier) -> Unit,
    bots: @Composable (Modifier) -> Unit,
    teams: @Composable (Modifier) -> Unit,
    friends: @Composable (Modifier) -> Unit,
    modifier: Modifier,
) {
    val selected = state.selected
    val saveable = rememberSaveableStateHolder()
    val focusManager = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    // A hidden composer must not keep the keyboard (or the focus) alive behind another tab.
    LaunchedEffect(selected) {
        if (selected != AppTab.Chats) {
            focusManager.clearFocus(force = true)
            keyboard?.hide()
        }
    }
    Box(modifier) {
        ChatsHost(active = selected == AppTab.Chats, content = chats)
        if (selected != AppTab.Chats) {
            saveable.SaveableStateProvider(selected.key) {
                val body = Modifier.fillMaxSize()
                when (selected) {
                    AppTab.Bots -> bots(body)
                    // A Team or friend conversation's voice input opens the voice settings and comes back to its tab.
                    AppTab.Teams, AppTab.Friends -> CompositionLocalProvider(
                        LocalOpenVoiceSettings provides { state.openSettings(SettingsRoutes.VOICE, selected) },
                    ) { if (selected == AppTab.Teams) teams(body) else friends(body) }
                    AppTab.Settings -> settings(body)
                    AppTab.Chats -> Unit
                }
            }
        }
    }
}

/**
 * Keeps the Chats tab composed while it is not the selected tab. Not placed, no semantics, and its
 * navigation dispatcher is disabled while hidden, so Back never closes a Chats page the user cannot see.
 */
@Composable
private fun ChatsHost(active: Boolean, content: @Composable () -> Unit) {
    val navigation = rememberNavigationEventDispatcherOwner(enabled = active)
    // Keep a real Layout parent so its measure policy resets the Box child's measuredByParent
    // even after Chats was visible. Hidden streaming updates cannot schedule independent remeasure.
    Layout(
        content = {
            CompositionLocalProvider(LocalNavigationEventDispatcherOwner provides navigation) {
                Box(Modifier.fillMaxSize()) { content() }
            }
        },
        modifier = Modifier.fillMaxSize().then(if (active) Modifier else Modifier.clearAndSetSemantics { }),
    ) { children, constraints ->
        if (active) {
            val child = children.single().measure(constraints)
            layout(child.width, child.height) { child.placeRelative(0, 0) }
        } else {
            layout(constraints.minWidth, constraints.minHeight) { }
        }
    }
}

// --- insets -------------------------------------------------------------------------------------

/** The bottom inset the bar covers: its current height, never more than the navigation-bar inset it pads. */
private class BarConsumedInsets(
    private val navigationBar: () -> WindowInsets,
    private val barHeightPx: () -> Int,
) : WindowInsets {
    override fun getLeft(density: Density, layoutDirection: LayoutDirection): Int = 0
    override fun getTop(density: Density): Int = 0
    override fun getRight(density: Density, layoutDirection: LayoutDirection): Int = 0
    override fun getBottom(density: Density): Int = minOf(barHeightPx(), navigationBar().getBottom(density))
}

/** The start inset the rail pads, while the rail is on screen. */
private class RailConsumedInsets(
    private val railStart: () -> WindowInsets,
    private val shown: () -> Boolean,
) : WindowInsets {
    override fun getLeft(density: Density, layoutDirection: LayoutDirection): Int =
        if (shown()) railStart().getLeft(density, layoutDirection) else 0
    override fun getTop(density: Density): Int = 0
    override fun getRight(density: Density, layoutDirection: LayoutDirection): Int =
        if (shown()) railStart().getRight(density, layoutDirection) else 0
    override fun getBottom(density: Density): Int = 0
}

// --- bar and rail -------------------------------------------------------------------------------

@Composable
private fun PhoneBar(
    visible: Boolean,
    keyboardVisible: Boolean,
    selected: AppTab,
    botsCount: Int,
    teamsCount: Int,
    friendsCount: Int,
    onSelect: (AppTab) -> Unit,
    onHeightChanged: (Int) -> Unit,
) {
    Box(Modifier.fillMaxWidth().onSizeChanged { onHeightChanged(it.height) }) {
        AnimatedVisibility(
            visible = visible,
            enter = expandVertically(expandFrom = Alignment.Top) + fadeIn(),
            // The keyboard takes the bottom edge at once; nothing else should linger beside it.
            exit = if (keyboardVisible) ExitTransition.None else shrinkVertically(shrinkTowards = Alignment.Top) + fadeOut(),
        ) {
            val label = stringResource(R.string.tabs_navigation_a11y)
            NavigationBar(Modifier.testTag(UiTags.TABS_BAR).semantics { contentDescription = label }) {
                AppTab.entries.forEach { tab ->
                    NavigationBarItem(
                        selected = tab == selected,
                        onClick = { onSelect(tab) },
                        icon = { TabIcon(tab, badgeCount(tab, botsCount, teamsCount, friendsCount)) },
                        label = { TabLabel(tab) },
                        alwaysShowLabel = true,
                        modifier = Modifier.testTag(UiTags.tab(tab.key)).tabBadgeState(tab, badgeCount(tab, botsCount, teamsCount, friendsCount)),
                    )
                }
            }
        }
    }
}

@Composable
private fun TabRail(selected: AppTab, botsCount: Int, teamsCount: Int, friendsCount: Int, onSelect: (AppTab) -> Unit) {
    val label = stringResource(R.string.tabs_navigation_a11y)
    NavigationRail(Modifier.width(80.dp).testTag(UiTags.TABS_RAIL).semantics { contentDescription = label }) {
        AppTab.entries.forEach { tab ->
            NavigationRailItem(
                selected = tab == selected,
                onClick = { onSelect(tab) },
                icon = { TabIcon(tab, badgeCount(tab, botsCount, teamsCount, friendsCount)) },
                label = { TabLabel(tab) },
                alwaysShowLabel = true,
                modifier = Modifier.testTag(UiTags.tab(tab.key)).tabBadgeState(tab, badgeCount(tab, botsCount, teamsCount, friendsCount)),
            )
        }
    }
}

@Composable
private fun TabLabel(tab: AppTab) {
    Text(stringResource(tab.labelRes()), minLines = if (LocalDensity.current.fontScale > 1.3f) 2 else 1,
        maxLines = 2, overflow = TextOverflow.Ellipsis)
}

/** Teams shows the unread total, Friends its requests plus unread, Bots the chats waiting for the user; Chats and Settings never badge. */
private fun badgeCount(tab: AppTab, botsCount: Int, teamsCount: Int, friendsCount: Int): Int = when (tab) {
    AppTab.Bots -> botsCount
    AppTab.Teams -> teamsCount
    AppTab.Friends -> friendsCount
    AppTab.Chats, AppTab.Settings -> 0
}

@Composable
private fun Modifier.tabBadgeState(tab: AppTab, badge: Int): Modifier {
    if (badge <= 0) return this
    val description = when (tab) {
        AppTab.Teams, AppTab.Friends -> stringResource(R.string.tabs_badge_unread_fmt, badge)
        AppTab.Bots -> stringResource(R.string.remote_bot_waiting_fmt, badge)
        AppTab.Chats, AppTab.Settings -> ""
    }
    return semantics { stateDescription = description }
}

@Composable
private fun TabIcon(tab: AppTab, badge: Int) {
    val text = TabLogic.tabBadgeText(badge)
    BadgedBox(
        badge = {
            if (text != null) {
                // The tab announces its full count as state; the visual 99+ badge stays silent.
                Badge(Modifier.clearAndSetSemantics { }) { Text(text) }
            }
        },
    ) {
        LXIcon(tab.icon(), size = 24.dp, color = LocalContentColor.current)
    }
}

private fun AppTab.labelRes(): Int = when (this) {
    AppTab.Chats -> R.string.tabs_chats
    AppTab.Bots -> R.string.remote_bots
    AppTab.Teams -> R.string.teams_title
    AppTab.Friends -> R.string.friends_title
    AppTab.Settings -> R.string.settings_title_main
}

private fun AppTab.icon(): LXIconName = when (this) {
    AppTab.Chats -> LXIconName.Message
    AppTab.Bots -> LXIconName.Bot
    AppTab.Teams -> LXIconName.Users
    AppTab.Friends -> LXIconName.User
    AppTab.Settings -> LXIconName.Cog
}
