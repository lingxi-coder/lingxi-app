package com.lingxi.code.tabs

import com.lingxi.code.friends.FriendsVectors
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Section 8.6 "Shell": every row of the shared tables, plus the edges the tables do not spell out. */
class TabLogicTest {
    @Test fun tabBarVisibleMatchesTheSharedTable() {
        val rows = SharedVectors.mobile.getJSONArray("tabBarVisible").objects()
        assertEquals(8, rows.size)
        rows.forEach { row ->
            val tab = requireNotNull(AppTab.fromKey(row.getString("selectedTab")))
            assertEquals(
                row.toString(),
                row.getBoolean("expect"),
                TabLogic.tabBarVisible(
                    wide = row.getBoolean("wide"),
                    depth = row.getInt("depth"),
                    keyboardVisible = row.getBoolean("keyboardVisible"),
                    drawerOpen = row.getBoolean("drawerOpen"),
                    selectedTab = tab,
                    overlayOpen = row.getBoolean("overlayOpen"),
                ),
            )
        }
    }

    @Test fun tabForLinkMatchesTheSharedTable() {
        val rows = SharedVectors.mobile.getJSONArray("tabForLink").objects()
        assertEquals(21, rows.size)
        rows.forEach { row ->
            assertEquals(row.getString("route"), row.getString("expect"), TabLogic.tabForLink(row.getString("route")).key)
        }
    }

    /** D16: Friends links open the Friends tab (`links.tabForLink` of `friends-vectors.json`); `friends/...` is no route. */
    @Test fun tabForLinkMatchesTheFriendsRows() {
        val rows = FriendsVectors.rows("links", "tabForLink")
        assertEquals(14, rows.size)
        rows.forEach { row ->
            assertEquals("route '${row.getString("route")}'", row.getString("expect"), TabLogic.tabForLink(row.getString("route")).key)
        }
    }

    @Test fun tabBadgeTextMatchesTheSharedTable() {
        val rows = SharedVectors.mobile.getJSONArray("tabBadgeText").objects()
        assertEquals(7, rows.size)
        rows.forEach { row ->
            assertEquals("count ${row.getInt("n")}", row.stringOrNull("expect"), TabLogic.tabBadgeText(row.getInt("n")))
        }
    }

    @Test fun botsBadgeCountMatchesTheSharedVector() {
        val vector = SharedVectors.mobile.getJSONObject("botsBadgeCount")
        val waiting = vector.getJSONArray("input").objects().map { it.getInt("waitingThreads") }
        assertEquals(listOf(2, 0, 3), waiting)
        assertEquals(vector.getInt("expect"), TabLogic.botsBadgeCount(waiting))
    }

    @Test fun aNegativeWaitingCountNeverReducesTheBadge() {
        assertEquals(3, TabLogic.botsBadgeCount(listOf(-4, 3)))
        assertEquals(0, TabLogic.botsBadgeCount(emptyList()))
    }

    @Test fun theRailStaysOnEveryWideDepthUntilAnOverlayCoversIt() {
        for (tab in AppTab.entries) for (depth in 0..3) {
            assertTrue(TabLogic.tabBarVisible(true, depth, keyboardVisible = true, drawerOpen = true, selectedTab = tab, overlayOpen = false))
            assertEquals(false, TabLogic.tabBarVisible(true, depth, keyboardVisible = false, drawerOpen = false, selectedTab = tab, overlayOpen = true))
        }
    }

    @Test fun onlyTheChatsDrawerHidesThePhoneBar() {
        for (tab in AppTab.entries) {
            assertEquals(
                tab != AppTab.Chats,
                TabLogic.tabBarVisible(false, 0, keyboardVisible = false, drawerOpen = true, selectedTab = tab, overlayOpen = false),
            )
        }
    }

    @Test fun aLinkTargetNamesItsTabOrNone() {
        assertEquals(AppTab.Chats, LinkTarget.Chats.tab)
        assertEquals(AppTab.Bots, LinkTarget.Bots.tab)
        assertEquals(AppTab.Teams, LinkTarget.Teams.tab)
        assertEquals(AppTab.Friends, LinkTarget.Friends.tab)
        assertEquals(AppTab.Settings, LinkTarget.Settings.tab)
        assertNull(LinkTarget.Stay.tab)
        assertNull(LinkTarget.None.tab)
    }

    @Test fun tabKeysAreTheSavedStateAndTagNames() {
        assertEquals(listOf("chats", "bots", "teams", "friends", "settings"), AppTab.entries.map { it.key })
        AppTab.entries.forEach { assertEquals(it, AppTab.fromKey(it.key)) }
        assertNull(AppTab.fromKey("home"))
        assertNull(AppTab.fromKey(null))
    }
}
