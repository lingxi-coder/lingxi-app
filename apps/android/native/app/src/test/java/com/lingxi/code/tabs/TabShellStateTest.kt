package com.lingxi.code.tabs

import androidx.compose.runtime.saveable.SaverScope
import com.lingxi.code.settings.SettingsRoutes
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the shell remembers (section 2.3 and 2.6), without a UI: the selection, the per-tab depth, the
 * requests a link leaves for a tab body, the Settings jump and its origin, and the re-tap events.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class TabShellStateTest {
    @Test
    fun startsOnTheChatsRoot() {
        val state = TabShellState()

        assertEquals(AppTab.Chats, state.selected)
        AppTab.entries.forEach { assertEquals(0, state.depthOf(it)) }
        assertNull(state.settingsJump)
        assertNull(state.botsRequest)
        assertNull(state.teamsJoinCode)
        assertFalse(state.chatsDrawerOpen)
        assertFalse(state.chatsFullScreenOverlay)
    }

    @Test
    fun depthIsPerTabAndNeverNegative() {
        val state = TabShellState()

        state.reportDepth(AppTab.Bots, 2)
        state.reportDepth(AppTab.Teams, -3)

        assertEquals(2, state.depthOf(AppTab.Bots))
        assertEquals(0, state.depthOf(AppTab.Teams))
        assertEquals(0, state.depthOf(AppTab.Settings))
    }

    @Test
    fun chatsReportsItsPagesDrawerAndOverlayTogether() {
        val state = TabShellState()

        state.reportChatsChrome(pushedPages = 2, drawerOpen = true, fullScreenOverlay = true)
        assertEquals(2, state.depthOf(AppTab.Chats))
        assertTrue(state.chatsDrawerOpen)
        assertTrue(state.chatsFullScreenOverlay)

        state.reportChatsChrome(pushedPages = 0, drawerOpen = false, fullScreenOverlay = false)
        assertEquals(0, state.depthOf(AppTab.Chats))
        assertFalse(state.chatsDrawerOpen)
        assertFalse(state.chatsFullScreenOverlay)
    }

    // --- Settings jumps ---------------------------------------------------------------------

    @Test
    fun openSettingsSelectsSettingsReplacesTheStackAndRemembersTheOrigin() {
        val state = TabShellState()

        state.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots)

        assertEquals(AppTab.Settings, state.selected)
        val jump = state.settingsJump
        assertNotNull(jump)
        assertEquals(SettingsRoutes.ACCOUNT, jump!!.route)
        assertEquals(AppTab.Bots, jump.origin)
        assertEquals(listOf(SettingsRoutes.ACCOUNT), state.settingsNav.stack)
        assertEquals(AppTab.Bots, state.settingsNav.origin)
    }

    @Test
    fun aJumpToTheRootLeavesTheStackEmpty() {
        val state = TabShellState()

        state.openSettings(SettingsRoutes.MAIN, origin = AppTab.Chats)

        assertEquals(emptyList<String>(), state.settingsNav.stack)
        assertEquals(AppTab.Chats, state.settingsNav.origin)
    }

    @Test
    fun aLinkJumpHasNoOrigin() {
        val state = TabShellState()

        state.openSettings(SettingsRoutes.ACCOUNT)

        assertNull(state.settingsNav.origin)
        assertNull(state.settingsJump!!.origin)
    }

    @Test
    fun everyJumpHasItsOwnIdAndAStaleHandledCallIsIgnored() {
        val state = TabShellState()
        state.openSettings(SettingsRoutes.ACCOUNT)
        val first = state.settingsJump!!
        state.openSettings(SettingsRoutes.VOICE)
        val second = state.settingsJump!!
        assertNotEquals(first.id, second.id)

        state.onSettingsJumpHandled(first.id)
        assertEquals(second, state.settingsJump)

        state.onSettingsJumpHandled(second.id)
        assertNull(state.settingsJump)
    }

    @Test
    fun backOnAJumpedPageSelectsTheOriginAndClearsTheStack() {
        val state = TabShellState()
        state.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Teams)
        state.mirrorSettingsStack(listOf(SettingsRoutes.ACCOUNT))

        val leftTo = state.settingsBack()

        assertEquals(AppTab.Teams, leftTo)
        assertEquals(AppTab.Teams, state.selected)
        assertEquals(emptyList<String>(), state.settingsNav.stack)
        assertNull(state.settingsNav.origin)
    }

    @Test
    fun backDeeperThanOnePagePopsInsideSettings() {
        val state = TabShellState()
        state.openSettings("provider/llm/edit/a", origin = AppTab.Chats)
        state.mirrorSettingsStack(listOf("provider/llm/edit/a", "provider/llm/model/b"))

        val leftTo = state.settingsBack()

        assertNull(leftTo)
        assertEquals(AppTab.Settings, state.selected)
        assertEquals(listOf("provider/llm/edit/a"), state.settingsNav.stack)
        assertEquals(AppTab.Chats, state.settingsNav.origin)
    }

    @Test
    fun closingSettingsGoesToTheOriginElseToChats() {
        val withOrigin = TabShellState()
        withOrigin.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots)
        withOrigin.closeSettings()
        assertEquals(AppTab.Bots, withOrigin.selected)
        assertNull(withOrigin.settingsNav.origin)

        val withoutOrigin = TabShellState(selected = AppTab.Settings)
        withoutOrigin.closeSettings()
        assertEquals(AppTab.Chats, withoutOrigin.selected)
    }

    @Test
    fun tappingATabForgetsWhereTheJumpCameFrom() {
        val state = TabShellState()
        state.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots)

        state.onTabClick(AppTab.Settings)

        assertNull(state.settingsNav.origin)
        // The page the jump opened is still there; only the way back changed.
        assertEquals(listOf(SettingsRoutes.ACCOUNT), state.settingsNav.stack)
    }

    // --- requests ---------------------------------------------------------------------------

    @Test
    fun aBotsRequestSelectsBotsAndIsConsumedOnce() {
        val state = TabShellState()

        state.requestBots("host-1", "session-9")
        assertEquals(AppTab.Bots, state.selected)
        val first = state.botsRequest!!
        assertEquals("host-1", first.hostId)
        assertEquals("session-9", first.sessionId)

        state.requestBots("host-2", null)
        val second = state.botsRequest!!
        assertNotEquals(first.id, second.id)

        state.onBotsRequestHandled(first.id)
        assertEquals(second, state.botsRequest)
        state.onBotsRequestHandled(second.id)
        assertNull(state.botsRequest)
    }

    @Test
    fun aTeamsRequestSelectsTeamsAndKeepsTheJoinCodeUntilHandled() {
        val state = TabShellState()

        state.requestTeams("CODE42")
        assertEquals(AppTab.Teams, state.selected)
        assertEquals("CODE42", state.teamsJoinCode)

        state.onTeamsJoinCodeHandled()
        assertNull(state.teamsJoinCode)

        // A link without a code (`team/inbox`) just opens the tab.
        state.select(AppTab.Chats)
        state.requestTeams(null)
        assertEquals(AppTab.Teams, state.selected)
        assertNull(state.teamsJoinCode)
    }

    // --- taps -------------------------------------------------------------------------------

    @Test
    fun tappingAnotherTabSelectsItWithoutAReselect() = runTest {
        val state = TabShellState()
        val reselects = mutableListOf<TabReselect>()
        val job = launch(UnconfinedTestDispatcher(testScheduler)) { state.reselects.collect { reselects += it } }

        state.onTabClick(AppTab.Teams)

        assertEquals(AppTab.Teams, state.selected)
        assertTrue(reselects.isEmpty())
        job.cancel()
    }

    @Test
    fun tappingTheSelectedTabEmitsAReselectCarryingItsDepth() = runTest {
        val state = TabShellState(selected = AppTab.Bots)
        state.reportDepth(AppTab.Bots, 2)
        val reselects = mutableListOf<TabReselect>()
        val job = launch(UnconfinedTestDispatcher(testScheduler)) { state.reselects.collect { reselects += it } }

        state.onTabClick(AppTab.Bots)
        state.reportDepth(AppTab.Bots, 0)
        state.onTabClick(AppTab.Bots)

        assertEquals(listOf(TabReselect(AppTab.Bots, 2), TabReselect(AppTab.Bots, 0)), reselects)
        assertEquals(AppTab.Bots, state.selected)
        job.cancel()
    }

    // --- saved state ------------------------------------------------------------------------

    @Test
    fun savedStateKeepsTheSelectedTabAndTheJumpOrigin() {
        val state = TabShellState(selected = AppTab.Settings, settingsOrigin = AppTab.Bots)

        val saved = with(TabShellState.Saver) { SaverScope { true }.save(state) }
        assertNotNull(saved)
        val restored = TabShellState.Saver.restore(saved!!)

        assertNotNull(restored)
        assertEquals(AppTab.Settings, restored!!.selected)
        assertEquals(AppTab.Bots, restored.settingsNav.origin)
    }

    @Test
    fun anUnknownSavedTabFallsBackToChats() {
        val restored = TabShellState.Saver.restore(listOf("renamed-tab", ""))

        assertEquals(AppTab.Chats, restored!!.selected)
        assertNull(restored.settingsNav.origin)
    }
}
