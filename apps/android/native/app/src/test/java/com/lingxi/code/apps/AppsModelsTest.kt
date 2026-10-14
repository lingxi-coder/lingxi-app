package com.lingxi.code.apps

import com.lingxi.code.market.InstalledApp
import com.lingxi.code.market.MarketOperation
import com.lingxi.code.market.MarketUpdate
import com.lingxi.code.market.MarketVersion
import com.lingxi.code.market.OperationPhase
import com.lingxi.code.market.UpdateStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AppsModelsTest {
    private fun installed(appId: String, sequence: Long = 1) =
        InstalledApp("https://market.test/api/v1", appId, "$appId-v$sequence", sequence, "install-$appId", "dir-$appId")

    private fun version(appId: String, sequence: Long) =
        MarketVersion("$appId-v$sequence", appId, sequence, "html", "index.html", 10, null, "", null)

    @Test
    fun myAppsPutsTheLastOpenedFirstThenSortsByName() {
        val info = mapOf(
            "a" to InstalledAppInfo("a", "Beta", lastOpenedAtMillis = 10),
            "b" to InstalledAppInfo("b", "alpha"),
            "c" to InstalledAppInfo("c", "Gamma", lastOpenedAtMillis = 20),
        )
        val apps = myApps(
            installed = listOf(installed("a"), installed("b"), installed("c"), installed("d")),
            info = info,
            updates = emptyList(),
            operations = emptyMap(),
        )
        assertEquals(listOf("c", "a", "b", "d"), apps.map(MyApp::appId))
        // Nothing known about "d" yet: it is shown by its id.
        assertEquals("d", apps.last().name)
    }

    @Test
    fun appsMadeInChatJoinTheListUnderTheirOwnKeys() {
        val info = mapOf(
            "a" to InstalledAppInfo("a", "Market app", lastOpenedAtMillis = 10),
            "chat:a1" to InstalledAppInfo("chat:a1", "chat:a1", lastOpenedAtMillis = 20),
        )
        val apps = myApps(
            installed = listOf(installed("a")),
            info = info,
            updates = emptyList(),
            operations = emptyMap(),
            chatApps = listOf(
                ChatApp("a1", "Water", "", revision = 3, updatedAtMillis = 5),
                ChatApp("b2", "Abacus", "", revision = 1, updatedAtMillis = 6),
            ),
        )
        assertEquals(listOf("chat:a1", "a", "chat:b2"), apps.map(MyApp::appId))
        // The library's name wins over what the pages recorded when it was opened.
        assertEquals("Water", apps.first().name)
        assertEquals(listOf(AppSource.CHAT, AppSource.MARKET, AppSource.CHAT), apps.map(MyApp::source))
        assertEquals(false, apps.first().updateAvailable)
    }

    @Test
    fun chatAppKeysRoundTripAndNeverMatchMarketIds() {
        assertEquals("chat:a1", chatAppKey("a1"))
        assertEquals("a1", chatAppIdOf(chatAppKey("a1")))
        assertNull(chatAppIdOf("a1"))
        assertNull(chatAppIdOf("chat:"))
    }

    @Test
    fun myAppsMarksUpdatesAndOnlyRunningOperations() {
        val apps = myApps(
            installed = listOf(installed("a"), installed("b"), installed("c")),
            info = emptyMap(),
            updates = listOf(
                MarketUpdate("a", UpdateStatus.UPDATE_AVAILABLE, "a-v2", 2),
                MarketUpdate("b", UpdateStatus.UP_TO_DATE, "b-v1", 1),
            ),
            operations = mapOf(
                "b" to MarketOperation(OperationPhase.DOWNLOADING, 5, 10),
                "c" to MarketOperation(OperationPhase.FAILED, error = "HTTP_503"),
            ),
        ).associateBy(MyApp::appId)
        assertTrue(apps.getValue("a").updateAvailable)
        assertEquals(false, apps.getValue("b").updateAvailable)
        assertEquals(OperationPhase.DOWNLOADING, apps.getValue("b").activeOperation?.phase)
        assertNull(apps.getValue("c").activeOperation)
    }

    @Test
    fun recentAppsAreTheLastThreeOpened() {
        val apps = listOf(
            MyApp("a", "A", "", 1, false, null),
            MyApp("b", "B", "", null, false, null),
            MyApp("c", "C", "", 3, false, null),
            MyApp("d", "D", "", 4, false, null),
            MyApp("e", "E", "", 2, false, null),
        )
        assertEquals(listOf("d", "c", "e"), recentApps(apps).map(MyApp::appId))
    }

    @Test
    fun matchingAppsSearchesNamesIgnoringCase() {
        val apps = listOf(MyApp("a", "Water Tracker", "", null, false, null), MyApp("b", "记账本", "", null, false, null))
        assertEquals(emptyList<MyApp>(), matchingApps(apps, "  "))
        assertEquals(listOf("a"), matchingApps(apps, "water").map(MyApp::appId))
        assertEquals(listOf("b"), matchingApps(apps, "账").map(MyApp::appId))
    }

    @Test
    fun appInitialIsTheFirstCharacter() {
        assertEquals("记", appInitial("记账本"))
        assertEquals("C", appInitial("  calculator"))
        assertEquals("😀", appInitial("😀 Fun"))
        assertEquals("?", appInitial(" "))
    }

    @Test
    fun appActionFollowsInstallStateAndVersions() {
        assertEquals(AppAction.Get, appAction(version("a", 1), installed = null, update = null, operation = null))
        assertEquals(AppAction.Unavailable, appAction(null, installed = null, update = null, operation = null))
        assertEquals(AppAction.Open, appAction(version("a", 1), installed("a", 1), update = null, operation = null))
        // The listing knows a newer version.
        assertEquals(AppAction.Update("a-v2"), appAction(version("a", 2), installed("a", 1), update = null, operation = null))
        // The update check wins over the listing, and works without one.
        assertEquals(
            AppAction.Update("a-v3"),
            appAction(version("a", 2), installed("a", 1), MarketUpdate("a", UpdateStatus.UPDATE_AVAILABLE, "a-v3", 3), null),
        )
        assertEquals(
            AppAction.Update("a-v3"),
            appAction(null, installed("a", 1), MarketUpdate("a", UpdateStatus.UPDATE_AVAILABLE, "a-v3", 3), null),
        )
        assertEquals(
            AppAction.Open,
            appAction(null, installed("a", 1), MarketUpdate("a", UpdateStatus.UNAVAILABLE, null, null), null),
        )
    }

    @Test
    fun appActionShowsOperationsInFlight() {
        val v1 = version("a", 1)
        assertEquals(
            AppAction.Downloading(0.25f),
            appAction(v1, null, null, MarketOperation(OperationPhase.DOWNLOADING, downloadedBytes = 25, totalBytes = 100)),
        )
        assertEquals(AppAction.Downloading(null), appAction(v1, null, null, MarketOperation(OperationPhase.DOWNLOADING)))
        assertEquals(AppAction.Installing, appAction(v1, null, null, MarketOperation(OperationPhase.COMMITTING)))
        assertEquals(AppAction.Removing, appAction(v1, installed("a"), null, MarketOperation(OperationPhase.UNINSTALLING)))
        // A failed or finished operation leaves the button as it was before.
        assertEquals(AppAction.Get, appAction(v1, null, null, MarketOperation(OperationPhase.FAILED, error = "HASH_MISMATCH")))
        assertEquals(AppAction.Open, appAction(v1, installed("a"), null, MarketOperation(OperationPhase.COMPLETE)))
    }

    @Test
    fun failedOperationCodeOnlyForFailures() {
        assertEquals("HTTP_503", failedOperationCode(MarketOperation(OperationPhase.FAILED, error = "HTTP_503")))
        assertEquals("UNKNOWN_ERROR", failedOperationCode(MarketOperation(OperationPhase.FAILED)))
        assertNull(failedOperationCode(MarketOperation(OperationPhase.CANCELLED)))
        assertNull(failedOperationCode(null))
    }

    @Test
    fun installedAppInfoRoundTrips() {
        val items = listOf(
            InstalledAppInfo("b", "记账本", "每天记一笔", "tools", lastOpenedAtMillis = 1_700_000_000_000),
            InstalledAppInfo("a", "Timer"),
        )
        val decoded = decodeInstalledAppInfo(encodeInstalledAppInfo(items))
        assertEquals(items.associateBy(InstalledAppInfo::appId), decoded)
    }
}
