package com.lingxi.code.bots

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import java.io.File
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the Bots tab keeps on the phone (SPEC-mobile-team-bot-ui.md sections 4.2 and 4.7): the snapshot codec, the
 * debounced and uid-scoped store, the tree's own choices, and the recorder that feeds them from the manager.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class BotsSnapshotStoreTest {
    private val ioScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** Where a store schedules its delayed writes when the test flushes by hand: a debounce this long never fires. */
    private val idleScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val dir = File.createTempFile("bots-store", "").let { it.delete(); it.mkdirs(); it }
    private val file = File(dir, "bots.preferences_pb")
    private val dataStore: DataStore<Preferences> = PreferenceDataStoreFactory.create(scope = ioScope) { file }

    @After fun cleanUp() {
        idleScope.cancel()
        ioScope.cancel()
        dir.deleteRecursively()
    }

    private fun store(over: DataStore<Preferences> = dataStore) = BotsSnapshotStore(over, idleScope, debounceMs = 600_000)

    private fun bot(id: String, chats: List<ChatItem>? = null, name: String = "Bot $id") =
        BotItem(id, name, "does $id", RemoteBotStatusFfi.IDLE, 0, null, chats)

    private fun chat(id: String) = ChatItem(id, "Chat $id")

    private fun snap(uid: String, hostId: String, at: Long, vararg bots: BotItem) = BotsSnapshot(uid, hostId, at, bots.toList())

    private suspend fun rawKeys(): Set<String> = dataStore.data.first().asMap().keys.map { it.name }.toSet()

    private suspend fun storedSnapshot(uid: String, hostId: String): BotsSnapshot? =
        dataStore.data.first()[stringPreferencesKey(snapshotKeyName(uid, hostId))]?.let { BotsSnapshotCodec.decode(it) }

    /** Waits, in real time, for [condition]: the work behind the store is on other threads. */
    private suspend fun eventually(what: String, condition: suspend () -> Boolean) = withContext(Dispatchers.Default) {
        try {
            withTimeout(10_000) {
                while (!condition()) delay(5)
            }
        } catch (_: TimeoutCancellationException) {
            throw AssertionError("timed out waiting for $what")
        }
    }

    private suspend fun awaitWrites(store: BotsSnapshotStore, count: Int) = withContext(Dispatchers.Default) {
        withTimeout(10_000) { while (store.writes < count) delay(5) }
    }

    // --- the stored form ------------------------------------------------------------------------------------------

    @Test fun aSnapshotRoundTripsThroughItsJsonForm() {
        val original = snap(
            "u1", "h1", 1_700_000_000_123L,
            BotItem(
                botId = "b-1_x", name = "机器人 \"quoted\"\nsecond line", description = "does things",
                status = RemoteBotStatusFfi.RUNNING, waitingThreads = 3, lastActivityAtMs = 1_700_000_000_000L,
                chats = listOf(
                    ChatItem("s1", "Title", RemoteSessionStatusFfi.RUNNING, null),
                    ChatItem("s2", "", RemoteSessionStatusFfi.REQUIRES_ACTION, RemotePendingActionFfi.QUESTION),
                ),
            ),
            bot("never-listed", chats = null),
            bot("no-chats", chats = emptyList()),
        )
        val decoded = BotsSnapshotCodec.decode(BotsSnapshotCodec.encode(original))
        assertEquals(original, decoded)
        // Never listed and listed-empty stay different things.
        assertNull(decoded!!.bots[1].chats)
        assertEquals(emptyList<ChatItem>(), decoded.bots[2].chats)
    }

    @Test fun textThatIsNoSnapshotReadsAsNone() {
        val valid = JSONObject(BotsSnapshotCodec.encode(snap("u1", "h1", 1L, bot("a"))))
        assertNotNull(BotsSnapshotCodec.decode(valid.toString()))
        assertNull(BotsSnapshotCodec.decode(""))
        assertNull(BotsSnapshotCodec.decode("{not json"))
        assertNull(BotsSnapshotCodec.decode("[]"))
        assertNull(BotsSnapshotCodec.decode(JSONObject(valid.toString()).put("v", 2).toString()))
        for (missing in listOf("v", "uid", "hostId", "savedAtMs", "bots")) {
            assertNull(missing, BotsSnapshotCodec.decode(JSONObject(valid.toString()).also { it.remove(missing) }.toString()))
        }
        // A Bot without an id is not a Bot; the whole snapshot is dropped rather than half read.
        val broken = JSONObject(valid.toString())
        broken.getJSONArray("bots").getJSONObject(0).remove("id")
        assertNull(BotsSnapshotCodec.decode(broken.toString()))
    }

    @Test fun valuesThisVersionDoesNotKnowReadAsTheirDefaults() {
        val json = JSONObject(BotsSnapshotCodec.encode(snap("u1", "h1", 1L, bot("a", chats = listOf(chat("s1"))))))
        val storedBot = json.getJSONArray("bots").getJSONObject(0)
        storedBot.put("status", "FROZEN").put("waiting", -5)
        storedBot.getJSONArray("chats").getJSONObject(0).put("status", "SLEEPING").put("pending", "TELEPORT")
        val decoded = BotsSnapshotCodec.decode(json.toString())!!
        assertEquals(RemoteBotStatusFfi.IDLE, decoded.bots[0].status)
        assertEquals(0, decoded.bots[0].waitingThreads)
        assertEquals(RemoteSessionStatusFfi.IDLE, decoded.bots[0].chats!![0].status)
        assertNull(decoded.bots[0].chats!![0].pendingAction)
    }

    @Test fun theKeyOfASnapshotCarriesNoAccountId() {
        val key = snapshotKeyName("user-123@example.com", "host-9")
        assertTrue(key.startsWith("bots.snap."))
        assertTrue(key.endsWith(".host-9"))
        assertFalse(key.contains("user-123"))
        assertEquals(key, snapshotKeyName("user-123@example.com", "host-9"))
        assertNotEquals(key, snapshotKeyName("user-456@example.com", "host-9"))
        assertNotEquals(key, snapshotKeyName("user-123@example.com", "host-8"))
    }

    // --- the store ------------------------------------------------------------------------------------------------

    @Test fun anUpdateIsReadBackForItsOwnUserOnly() = runBlocking {
        val store = store()
        store.update("u1", "h1") { snap("u1", "h1", 5L, bot("a")) }
        store.update("u2", "h1") { snap("u2", "h1", 6L, bot("b")) }
        assertEquals(setOf("h1"), store.snapshots("u1").first().keys)
        assertEquals(listOf("a"), store.snapshots("u1").first().getValue("h1").bots.map { it.botId })
        assertEquals(listOf("b"), store.snapshots("u2").first().getValue("h1").bots.map { it.botId })
        assertTrue(store.snapshots("u3").first().isEmpty())
        assertEquals("a", store.load("u1", "h1")!!.bots.single().botId)
        assertNull(store.load("u3", "h1"))
        assertNull(store.load("u1", "other"))
    }

    @Test fun theTransformSeesThePriorSnapshotOfThatUserAndComputerOnly() = runBlocking {
        val store = store()
        store.update("u1", "h1") { snap("u1", "h1", 5L, bot("a")) }
        var seen: BotsSnapshot? = snap("x", "x", 0L)
        store.update("u2", "h1") { prior -> seen = prior; null }
        assertNull(seen)
        store.update("u1", "h2") { prior -> seen = prior; null }
        assertNull(seen)
        store.update("u1", "h1") { prior -> seen = prior; prior }
        assertEquals("a", seen!!.bots.single().botId)
    }

    @Test fun aSnapshotSurvivesTheStoreBeingRecreated() = runBlocking {
        val first = store()
        first.update("u1", "h1") { snap("u1", "h1", 7L, bot("a", chats = listOf(chat("s1")))) }
        first.flush()
        assertEquals(1, first.writes)
        val second = store()
        assertEquals(snap("u1", "h1", 7L, bot("a", chats = listOf(chat("s1")))), second.load("u1", "h1"))
        assertEquals(setOf("h1"), second.snapshots("u1").first().keys)
    }

    @Test fun aBurstOfUpdatesBecomesOneWriteADebounceAfterTheLast() = runTest {
        val store = BotsSnapshotStore(dataStore, backgroundScope, debounceMs = 1_000)
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        advanceTimeBy(400)
        store.update("u1", "h1") { snap("u1", "h1", 2L, bot("a")) }
        advanceTimeBy(400)
        store.update("u1", "h1") { snap("u1", "h1", 3L, bot("a")) }
        // 999 ms after the last change nothing has been written; the earlier changes did not start the clock.
        advanceTimeBy(999)
        assertEquals(0, store.writes)
        advanceTimeBy(2)
        awaitWrites(store, 1)
        assertEquals(1, store.writes)
        assertEquals(3L, storedSnapshot("u1", "h1")!!.savedAtMs)
        advanceUntilIdle()
        assertEquals(1, store.writes)
    }

    @Test fun anUpdateThatChangesNothingWritesNothing() = runTest {
        val store = BotsSnapshotStore(dataStore, backgroundScope, debounceMs = 1_000)
        store.update("u1", "h1") { prior -> prior }
        store.update("u1", "h1") { null }
        advanceTimeBy(5_000)
        assertEquals(0, store.writes)
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        store.flush()
        assertEquals(1, store.writes)
        // The same value again, as an equal copy, changes nothing either.
        store.update("u1", "h1") { prior -> prior!!.copy() }
        advanceTimeBy(5_000)
        assertEquals(1, store.writes)
    }

    @Test fun aDeletionIsWrittenAtOnce() = runBlocking {
        val store = store()
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        store.update("u1", "h2") { snap("u1", "h2", 2L, bot("b")) }
        store.flush()
        assertEquals(setOf(snapshotKeyName("u1", "h1"), snapshotKeyName("u1", "h2")), rawKeys())
        store.delete("u1", "h1")
        assertEquals(setOf(snapshotKeyName("u1", "h2")), rawKeys())
        assertNull(store.load("u1", "h1"))
        assertEquals(setOf("h2"), store.snapshots("u1").first().keys)
    }

    @Test fun signingOutDropsEverySnapshotAndChangingAccountDropsTheOthers() = runBlocking {
        val store = store()
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        store.update("u2", "h2") { snap("u2", "h2", 2L, bot("b")) }
        store.flush()
        assertEquals(2, rawKeys().size)
        store.retainOnly("u2")
        assertEquals(setOf(snapshotKeyName("u2", "h2")), rawKeys())
        assertNull(store.load("u1", "h1"))
        assertNotNull(store.load("u2", "h2"))
        store.retainOnly(null)
        assertTrue(rawKeys().isEmpty())
        assertTrue(store.snapshots("u2").first().isEmpty())
    }

    @Test fun onlyTheTwentyNewestComputersOfAUserAreKept() = runBlocking {
        val store = store()
        for (i in 1..21) store.update("u1", "h$i") { snap("u1", "h$i", i.toLong(), bot("a")) }
        store.update("u2", "only") { snap("u2", "only", 0L, bot("a")) }
        store.flush()
        val kept = store.snapshots("u1").first().keys
        assertEquals(20, kept.size)
        assertFalse("h1" in kept)
        assertTrue("h2" in kept && "h21" in kept)
        // The other user's single computer is not the oldest of anybody's.
        assertEquals(setOf("only"), store.snapshots("u2").first().keys)
        assertFalse(snapshotKeyName("u1", "h1") in rawKeys())
        assertEquals(21, rawKeys().size)
    }

    @Test fun aSnapshotThatDoesNotReadBackIsDroppedOnLoad() = runBlocking {
        val mine = snap("u1", "h1", 1L, bot("a"))
        dataStore.edit { prefs ->
            prefs[stringPreferencesKey(snapshotKeyName("u1", "h1"))] = BotsSnapshotCodec.encode(mine)
            prefs[stringPreferencesKey("bots.snap.deadbeef.junk")] = "{not json"
            // Another user's snapshot filed under this user's key name: never read for the wrong user.
            prefs[stringPreferencesKey(snapshotKeyName("u1", "h2"))] = BotsSnapshotCodec.encode(snap("u2", "h2", 2L, bot("x")))
        }
        val store = store()
        assertEquals(mine, store.load("u1", "h1"))
        assertNull(store.load("u1", "h2"))
        assertTrue(store.snapshots("u1").first().keys == setOf("h1"))
        store.flush()
        assertEquals(setOf(snapshotKeyName("u1", "h1")), rawKeys())
    }

    @Test fun otherPreferencesInTheFileAreLeftAlone() = runBlocking {
        dataStore.edit { it[stringPreferencesKey("something.else")] = "kept" }
        val store = store()
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        store.retainOnly(null)
        assertEquals(setOf("something.else"), rawKeys())
    }

    private class FlakyDataStore(private val inner: DataStore<Preferences>) : DataStore<Preferences> {
        @Volatile var failures = 0
        override val data = inner.data
        override suspend fun updateData(transform: suspend (t: Preferences) -> Preferences): Preferences {
            if (failures > 0) {
                failures--
                throw IOException("disk full")
            }
            return inner.updateData(transform)
        }
    }

    @Test fun aFailedWriteIsTriedAgainWithTheNextFlush() = runBlocking {
        val flaky = FlakyDataStore(dataStore)
        val store = store(flaky)
        store.update("u1", "h1") { snap("u1", "h1", 1L, bot("a")) }
        flaky.failures = 1
        store.flush()
        assertEquals(0, store.writes)
        assertTrue(rawKeys().isEmpty())
        // The snapshot is still the one in memory, and the next flush writes it.
        assertNotNull(store.load("u1", "h1"))
        store.flush()
        assertEquals(1, store.writes)
        assertEquals(1L, storedSnapshot("u1", "h1")!!.savedAtMs)
    }

    // --- the tree's own choices -----------------------------------------------------------------------------------

    @Test fun choicesDefaultToNothingCollapsedAndNoChosenBots() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        assertEquals(BotsPrefs(emptySet(), null), prefs.prefs("u1").first())
        assertNull(prefs.lastHostId("u1"))
    }

    @Test fun collapsedComputersAreKeptPerUserUnderTheirKey() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        prefs.setCollapsed("u1", "h1", true)
        prefs.setCollapsed("u1", "h2", true)
        prefs.setCollapsed("u2", "h9", true)
        assertEquals(setOf("h1", "h2"), prefs.prefs("u1").first().collapsedHosts)
        assertEquals(setOf("h9"), prefs.prefs("u2").first().collapsedHosts)
        assertEquals(setOf("h1", "h2"), dataStore.data.first()[stringSetPreferencesKey("bots.collapsedHosts.u1")])
        prefs.setCollapsed("u1", "h1", false)
        assertEquals(setOf("h2"), prefs.prefs("u1").first().collapsedHosts)
        prefs.setCollapsed("u1", "h2", false)
        assertNull(dataStore.data.first()[stringSetPreferencesKey("bots.collapsedHosts.u1")])
        assertTrue(prefs.prefs("u1").first().collapsedHosts.isEmpty())
    }

    @Test fun openBotsAreKeptPerUserAndAnEmptyChoiceIsAChoice() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        prefs.setExpandedBots("u1", setOf("h1/a", "h1/b"))
        assertEquals(setOf("h1/a", "h1/b"), prefs.prefs("u1").first().expandedBots)
        assertEquals(setOf("h1/a", "h1/b"), dataStore.data.first()[stringSetPreferencesKey("bots.expandedBots.u1")])
        assertNull(prefs.prefs("u2").first().expandedBots)
        prefs.setExpandedBots("u1", emptySet())
        assertEquals(emptySet<String>(), prefs.prefs("u1").first().expandedBots)
    }

    @Test fun theLastUsedComputerIsKeptPerUser() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        prefs.saveLastHost("u1", "h1")
        prefs.saveLastHost("u2", "h2")
        assertEquals("h1", prefs.lastHostId("u1"))
        assertEquals("h2", prefs.lastHostId("u2"))
        assertEquals("h1", dataStore.data.first()[stringPreferencesKey("bots.lastHost.u1")])
        prefs.saveLastHost("u1", "h3")
        assertEquals("h3", prefs.lastHostId("u1"))
        assertNull(prefs.lastHostId("u3"))
    }

    @Test fun forgettingAComputerRemovesItFromTheChoicesToo() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        prefs.setCollapsed("u1", "h1", true)
        prefs.setCollapsed("u1", "h2", true)
        prefs.setExpandedBots("u1", setOf("h1/a", "h2/b", "h1/c"))
        prefs.saveLastHost("u1", "h1")
        prefs.forgetHost("u1", "h1")
        assertEquals(BotsPrefs(setOf("h2"), setOf("h2/b")), prefs.prefs("u1").first())
        assertNull(prefs.lastHostId("u1"))
        // Another computer's last-used mark stays.
        prefs.saveLastHost("u1", "h2")
        prefs.forgetHost("u1", "h1")
        assertEquals("h2", prefs.lastHostId("u1"))
        // Forgetting leaves the choice of nothing chosen as it was.
        val untouched = BotsPrefsStore(dataStore)
        assertNull(untouched.prefs("u2").first().expandedBots)
        untouched.forgetHost("u2", "h1")
        assertNull(untouched.prefs("u2").first().expandedBots)
    }

    @Test fun changingAccountDropsTheChoicesOfEveryoneElse() = runBlocking {
        val prefs = BotsPrefsStore(dataStore)
        for (uid in listOf("u1", "u2")) {
            prefs.setCollapsed(uid, "h1", true)
            prefs.setExpandedBots(uid, setOf("h1/a"))
            prefs.saveLastHost(uid, "h1")
        }
        dataStore.edit { it[stringPreferencesKey("something.else")] = "kept" }
        prefs.retainOnly("u1")
        assertEquals(BotsPrefs(setOf("h1"), setOf("h1/a")), prefs.prefs("u1").first())
        assertEquals(BotsPrefs(emptySet(), null), prefs.prefs("u2").first())
        assertNull(prefs.lastHostId("u2"))
        assertEquals("h1", prefs.lastHostId("u1"))
        prefs.retainOnly(null)
        assertEquals(setOf("something.else"), rawKeys())
    }

    // --- the recorder: the manager's traffic, kept ---------------------------------------------------------------

    private class Fixture(dataStore: DataStore<Preferences>, clockStart: Long = 1_000L) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val stores = BotsStores(dataStore, scope, debounceMs = 10)
        val now = AtomicLong(clockStart)
        val recorder = BotsRecorder(stores, scope) { now.get() }.also { it.onAccount("u1") }
        fun close() = scope.cancel()
    }

    private fun liveBots(vararg ids: String) = RemoteBotsStateFfi.Bots(ids.map { RemoteBotSummaryFfi(it, "Bot $it", "does $it", RemoteBotStatusFfi.IDLE, 2u, 77uL) })

    private fun threads(vararg ids: String) =
        RemoteBotThreadsStateFfi(ids.map { RemoteSessionSummaryFfi(it, "Chat $it", "", RemoteSessionStatusFfi.IDLE, null, null, 0u) }, null)

    @Test fun theBotsAComputerDeliversAreSavedAsItsSnapshot() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a", "b"))
            eventually("snapshot") { f.stores.snapshots.load("u1", "h1") != null }
            val saved = f.stores.snapshots.load("u1", "h1")!!
            assertEquals(1_000L, saved.savedAtMs)
            assertEquals(listOf("a", "b"), saved.bots.map { it.botId })
            assertEquals(BotItem("a", "Bot a", "does a", RemoteBotStatusFfi.IDLE, 2, 77L, null), saved.bots[0])
            // Written behind: it reaches the DataStore a debounce later.
            eventually("written") { storedSnapshot("u1", "h1") != null }
            assertEquals(saved, storedSnapshot("u1", "h1"))
        } finally {
            f.close()
        }
    }

    @Test fun aComputerThatDoesNotKnowBotsLeavesTheSnapshotAlone() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            eventually("snapshot") { f.stores.snapshots.load("u1", "h1") != null }
            f.recorder.onBots("u1", "h1", RemoteBotsStateFfi.Unsupported)
            f.recorder.onBots("u1", "h1", RemoteBotsStateFfi.Unknown)
            f.recorder.onConnected("u1", "marker")
            eventually("marker") { f.stores.prefs.lastHostId("u1") == "marker" }
            assertEquals(listOf("a"), f.stores.snapshots.load("u1", "h1")!!.bots.map { it.botId })
        } finally {
            f.close()
        }
    }

    @Test fun aBotsChatsAreSavedWithTheBotAndSurviveTheBotListChanging() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a", "b"))
            f.recorder.onBotThreads("u1", "h1", "a", threads("c1", "c2"))
            f.recorder.onBots("u1", "h1", liveBots("a", "c"))
            f.recorder.onConnected("u1", "marker")
            eventually("marker") { f.stores.prefs.lastHostId("u1") == "marker" }
            val saved = f.stores.snapshots.load("u1", "h1")!!
            assertEquals(listOf("a", "c"), saved.bots.map { it.botId })
            assertEquals(listOf("c1", "c2"), saved.bots[0].chats!!.map { it.sessionId })
            assertNull(saved.bots[1].chats)
        } finally {
            f.close()
        }
    }

    @Test fun chatsOfABotTheSnapshotDoesNotListAndChatsThatFailedAreIgnored() = runBlocking {
        val f = Fixture(dataStore)
        try {
            // No snapshot yet: chats alone never create one.
            f.recorder.onBotThreads("u1", "h1", "a", threads("c1"))
            f.recorder.onBots("u1", "h1", liveBots("a"))
            f.recorder.onBotThreads("u1", "h1", "zzz", threads("x"))
            f.recorder.onBotThreads("u1", "h1", "a", RemoteBotThreadsStateFfi(null, RemoteSessionErrorFfi("timeout", "slow")))
            f.recorder.onConnected("u1", "marker")
            eventually("marker") { f.stores.prefs.lastHostId("u1") == "marker" }
            val saved = f.stores.snapshots.load("u1", "h1")!!
            assertNull(saved.bots.single().chats)
        } finally {
            f.close()
        }
    }

    @Test fun chatsDeliveredAgainUnchangedDoNotMoveTheSaveTime() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            f.recorder.onBotThreads("u1", "h1", "a", threads("c1", "c2"))
            f.recorder.onConnected("u1", "m1")
            eventually("first") { f.stores.prefs.lastHostId("u1") == "m1" }
            val before = f.stores.snapshots.load("u1", "h1")!!.savedAtMs
            f.now.set(9_000L)
            f.recorder.onBotThreads("u1", "h1", "a", threads("c1", "c2"))
            f.recorder.onConnected("u1", "m2")
            eventually("second") { f.stores.prefs.lastHostId("u1") == "m2" }
            assertEquals(before, f.stores.snapshots.load("u1", "h1")!!.savedAtMs)
            // A different list does move it.
            f.recorder.onBotThreads("u1", "h1", "a", threads("c0", "c1", "c2"))
            f.recorder.onConnected("u1", "m3")
            eventually("third") { f.stores.prefs.lastHostId("u1") == "m3" }
            assertEquals(9_000L, f.stores.snapshots.load("u1", "h1")!!.savedAtMs)
        } finally {
            f.close()
        }
    }

    @Test fun theLastUsedComputerIsRememberedAndReadBack() = runBlocking {
        val f = Fixture(dataStore)
        try {
            assertNull(f.recorder.lastHostId("u1"))
            f.recorder.onConnected("u1", "h1")
            eventually("remembered") { f.recorder.lastHostId("u1") == "h1" }
            assertNull(f.recorder.lastHostId("u2"))
        } finally {
            f.close()
        }
    }

    @Test fun forgettingAComputerDeletesItsSnapshotAndChoices() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            f.recorder.onBots("u1", "h2", liveBots("b"))
            f.stores.prefs.setCollapsed("u1", "h1", true)
            f.recorder.onConnected("u1", "h1")
            eventually("saved") { f.stores.snapshots.load("u1", "h2") != null && f.recorder.lastHostId("u1") == "h1" }
            f.recorder.onForgotten("u1", "h1")
            eventually("forgotten") { f.stores.snapshots.load("u1", "h1") == null && f.recorder.lastHostId("u1") == null }
            assertNotNull(f.stores.snapshots.load("u1", "h2"))
            assertTrue(f.stores.prefs.prefs("u1").first().collapsedHosts.isEmpty())
            // The deletion reached the file at once, without waiting for a debounce.
            eventually("file") { snapshotKeyName("u1", "h1") !in rawKeys() }
        } finally {
            f.close()
        }
    }

    @Test fun signingOutClearsEverySnapshotAndChoice() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            f.recorder.onConnected("u1", "h1")
            f.stores.prefs.setExpandedBots("u1", setOf("h1/a"))
            eventually("saved") { f.stores.snapshots.load("u1", "h1") != null && f.recorder.lastHostId("u1") == "h1" }
            f.recorder.onAccount(null)
            eventually("cleared") { f.stores.snapshots.load("u1", "h1") == null && f.recorder.lastHostId("u1") == null }
            assertNull(f.stores.prefs.prefs("u1").first().expandedBots)
            eventually("file") { rawKeys().isEmpty() }
        } finally {
            f.close()
        }
    }

    @Test fun anotherAccountSigningInDropsTheFirstOnesSnapshotsAndKeepsItsOwn() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            eventually("saved") { f.stores.snapshots.load("u1", "h1") != null }
            f.recorder.onAccount("u2")
            f.recorder.onBots("u2", "h2", liveBots("b"))
            f.recorder.onConnected("u2", "marker")
            eventually("dropped") { f.recorder.lastHostId("u2") == "marker" && f.stores.snapshots.load("u1", "h1") == null }
            assertNotNull(f.stores.snapshots.load("u2", "h2"))
            // Never read for another account: u1 sees nothing of u2's.
            assertTrue(f.stores.snapshots.snapshots("u1").first().isEmpty())
        } finally {
            f.close()
        }
    }

    @Test fun lateCallbacksCannotRecreateThePreviousAccountsSnapshotsOrLastHost() = runBlocking {
        val f = Fixture(dataStore)
        try {
            f.recorder.onBots("u1", "h1", liveBots("a"))
            eventually("first snapshot") { f.stores.snapshots.load("u1", "h1") != null }
            f.recorder.onAccount("u2")
            // These callbacks were accepted by an old client just before the account changed.
            f.recorder.onBots("u1", "h1", liveBots("late"))
            f.recorder.onBotThreads("u1", "h1", "a", threads("private"))
            f.recorder.onConnected("u1", "h1")
            f.recorder.onBots("u2", "h2", liveBots("b"))
            f.recorder.onConnected("u2", "marker")
            eventually("queue drained") { f.recorder.lastHostId("u2") == "marker" }
            f.stores.snapshots.flush()
            assertNull(f.stores.snapshots.load("u1", "h1"))
            assertNull(f.recorder.lastHostId("u1"))
            assertNotNull(f.stores.snapshots.load("u2", "h2"))
            assertTrue(rawKeys().none { it == snapshotKeyName("u1", "h1") || it == "bots.lastHost.u1" })
        } finally { f.close() }
    }

    @Test fun anInFlightFlushCannotRestoreDataAfterAccountRetention() = runBlocking {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val blockFirst = AtomicBoolean(true)
        val gated = object : DataStore<Preferences> {
            override val data = dataStore.data
            override suspend fun updateData(transform: suspend (Preferences) -> Preferences): Preferences {
                if (blockFirst.getAndSet(false)) { entered.complete(Unit); release.await() }
                return dataStore.updateData(transform)
            }
        }
        val store = store(gated)
        store.update("u1", "h1") { snap("u1", "h1", 1, bot("private")) }
        val flush = async(start = CoroutineStart.UNDISPATCHED) { store.flush() }
        entered.await()
        val retention = async(start = CoroutineStart.UNDISPATCHED) { store.retainOnly(null) }
        release.complete(Unit)
        flush.await(); retention.await()
        assertNull(storedSnapshot("u1", "h1"))
    }

    // --- the badge -------------------------------------------------------------------------------------------------

    @Test fun theBadgeFollowsTheManagersCountAndAnotherSourceReplacesIt() {
        val scope = CoroutineScope(Job() + Dispatchers.Unconfined)
        try {
            val first = MutableStateFlow(3)
            BotsBadge.follow(first, scope)
            assertEquals(3, BotsBadge.count.value)
            first.value = 7
            assertEquals(7, BotsBadge.count.value)
            val second = MutableStateFlow(1)
            BotsBadge.follow(second, scope)
            assertEquals(1, BotsBadge.count.value)
            first.value = 50
            assertEquals(1, BotsBadge.count.value)
            second.value = 0
            assertEquals(0, BotsBadge.count.value)
        } finally {
            BotsBadge.follow(MutableStateFlow(0), scope)
            scope.cancel()
        }
    }
}
