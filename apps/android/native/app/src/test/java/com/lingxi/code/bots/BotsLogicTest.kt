package com.lingxi.code.bots

import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteConnectionPhaseFfi
import com.lingxi.code.bindings.android.RemoteConnectionStateFfi
import com.lingxi.code.bindings.android.RemoteFailureFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteHostFfi
import com.lingxi.code.bindings.android.RemotePendingActionFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.remote.RemoteBotChatsState
import com.lingxi.code.remote.RemoteBotsState
import com.lingxi.code.remote.RemoteConnectionUi
import com.lingxi.code.remote.RemoteHostsState
import com.lingxi.code.tabs.SharedVectors
import com.lingxi.code.tabs.TabLogic
import com.lingxi.code.tabs.objects
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The Bots tab's rules (SPEC-mobile-team-bot-ui.md sections 4 and 8.6): the shared vectors first, then every state of
 * section 4.5 and every cell of section 4.3 as the rows [buildTree] makes and the plans the gestures follow.
 */
class BotsLogicTest {
    // --- builders -------------------------------------------------------------------------------------------------

    private fun host(id: String, online: Boolean = true, paired: Boolean = true, name: String = "Host $id", lastSeen: Long = 0L) =
        RemoteHostFfi(id, name, "macos", "1.0", online, lastSeen.toULong(), paired)

    private fun liveBot(id: String, waiting: Int = 0, status: RemoteBotStatusFfi = RemoteBotStatusFfi.IDLE, at: Long? = null, name: String = "Bot $id") =
        RemoteBotSummaryFfi(id, name, "does $id", status, waiting.toUInt(), at?.toULong())

    private fun liveBots(hostId: String, vararg bots: RemoteBotSummaryFfi) = RemoteBotsState(hostId, RemoteBotsStateFfi.Bots(bots.toList()))

    private fun liveChat(id: String, title: String = "Chat $id", status: RemoteSessionStatusFfi = RemoteSessionStatusFfi.IDLE, pending: RemotePendingActionFfi? = null) =
        RemoteSessionSummaryFfi(id, title, "", status, pending, null, 0u)

    private fun heldChats(hostId: String, botId: String, threads: List<RemoteSessionSummaryFfi>?, error: RemoteSessionErrorFfi? = null) =
        RemoteBotChatsState(hostId, mapOf(botId to RemoteBotThreadsStateFfi(threads, error)))

    private fun connection(
        hostId: String,
        phase: RemoteConnectionPhaseFfi,
        everReady: Boolean = false,
        pairingCode: String? = null,
        failure: RemoteFailureFfi? = null,
    ) = RemoteConnectionStateFfi(hostId, phase, null, pairingCode, failure, 0u, null, null, everReady)

    private fun ready(hostId: String) = connection(hostId, RemoteConnectionPhaseFfi.READY, everReady = true)

    private fun savedBot(id: String, chats: List<ChatItem>? = null, name: String = "Bot $id") = BotItem(id, name, "does $id", chats = chats)

    private fun saved(hostId: String, vararg bots: BotItem, at: Long = 1_000L) = BotsSnapshot("u1", hostId, at, bots.toList())

    private fun savedChat(id: String) = ChatItem(id, "Saved $id")

    private fun tree(
        hosts: List<RemoteHostFfi>,
        connection: RemoteConnectionStateFfi? = null,
        bots: RemoteBotsState = RemoteBotsState(),
        chats: RemoteBotChatsState = RemoteBotChatsState(),
        snapshots: List<BotsSnapshot> = emptyList(),
        collapsed: Set<String> = emptySet(),
        expanded: Set<String>? = emptySet(),
        showAll: Set<String> = emptySet(),
    ) = buildTree(TreeInput(hosts, connection, bots, chats, snapshots.associateBy { it.hostId }, collapsed, expanded, showAll))

    /** The rows as short lines, so a whole section reads at a glance. */
    private fun lines(rows: List<TreeRow>): List<String> = rows.map { row ->
        when (row) {
            is TreeRow.HostHeader -> "host ${row.host.hostId}" + (if (row.connected) " connected" else "") + (if (row.collapsed) " collapsed" else "")
            is TreeRow.Connection -> "connection ${row.hostId} ${row.ui::class.simpleName}"
            is TreeRow.Note -> "note ${row.hostId} ${row.note.key}"
            is TreeRow.State -> "state ${row.hostId} ${row.kind.key}"
            is TreeRow.Bot -> "bot ${row.bot.botId}" + (if (row.expanded) " open" else "") + (if (row.dimmed) " dim" else "")
            is TreeRow.NewChat -> "newChat ${row.botId}" + (if (row.enabled) "" else " disabled")
            is TreeRow.BotState -> "botState ${row.botId} ${row.state}"
            is TreeRow.Chat -> "chat ${row.chat.sessionId}" + (if (row.dimmed) " dim" else "")
            is TreeRow.ShowMore -> "more ${row.botId} ${row.hidden}"
            is TreeRow.ShowLess -> "less ${row.botId}"
        }
    }

    private fun sectionOf(rows: List<TreeRow>, hostId: String): List<TreeRow> {
        val start = rows.indexOfFirst { it is TreeRow.HostHeader && it.host.hostId == hostId }
        val end = rows.drop(start + 1).indexOfFirst { it is TreeRow.HostHeader }.let { if (it < 0) rows.size else start + 1 + it }
        return rows.subList(start, end)
    }

    // --- the shared vectors (section 8.6) --------------------------------------------------------------------------

    private fun JSONObject.longOrNull(key: String): Long? = if (isNull(key)) null else getLong(key)

    @Test fun chatWindowMatchesTheSharedVectors() {
        val cases = SharedVectors.mobile.getJSONArray("chatWindow").objects()
        assertEquals(7, cases.size)
        for (case in cases) {
            val window = BotsLogic.chatWindow(case.getInt("total"), case.getBoolean("expanded"), case.getInt("first"))
            val expect = case.getJSONObject("expect")
            val label = case.toString()
            assertEquals(label, expect.getInt("shown"), window.shown)
            assertEquals(label, expect.getInt("hidden"), window.hidden)
            assertEquals(label, expect.getBoolean("showMore"), window.showMore)
            assertEquals(label, expect.getBoolean("showLess"), window.showLess)
        }
    }

    @Test fun chatWindowShowsFiveChatsByDefault() {
        assertEquals(5, BotsLogic.FIRST_CHATS)
        assertEquals(BotsLogic.ChatWindow(shown = 5, hidden = 3, showMore = true, showLess = false), BotsLogic.chatWindow(8, expanded = false))
        // A negative count (which a host cannot send) shows nothing rather than failing.
        assertEquals(BotsLogic.ChatWindow(0, 0, showMore = false, showLess = false), BotsLogic.chatWindow(-4, expanded = false))
    }

    @Test fun sortBotsMatchesTheSharedVector() {
        val vector = SharedVectors.mobile.getJSONObject("sortBots")
        val bots = vector.getJSONArray("input").objects().map {
            BotItem(it.getString("botId"), it.getString("name"), lastActivityAtMs = it.longOrNull("lastActivityAtMs"))
        }
        val expect = vector.getJSONArray("expect").let { array -> (0 until array.length()).map { array.getString(it) } }
        assertEquals(expect, BotsLogic.sortBots(bots).map { it.botId })
    }

    @Test fun sortHostsMatchesTheSharedVector() {
        val vector = SharedVectors.mobile.getJSONObject("sortHosts")
        val hosts = vector.getJSONArray("input").objects().map {
            host(it.getString("hostId"), it.getBoolean("online"), true, it.getString("name"), it.getLong("lastSeenMs"))
        }
        val expect = vector.getJSONArray("expect").let { array -> (0 until array.length()).map { array.getString(it) } }
        assertEquals(expect, BotsLogic.sortHosts(hosts, vector.getString("connected")).map { it.hostId })
        // Nothing connected: the same order, less the connected computer's lead.
        assertEquals(listOf("h4", "h5", "h1", "h2", "h3"), BotsLogic.sortHosts(hosts, null).map { it.hostId })
    }

    @Test fun hostBodyMatchesTheSharedVectors() {
        val cases = SharedVectors.mobile.getJSONArray("hostBody").objects()
        assertEquals(13, cases.size)
        for (case in cases) {
            val input = case.getJSONObject("input")
            val live = when (val raw = input.get("live")) {
                "unknown" -> LiveBots.Unknown
                "unsupported" -> LiveBots.Unsupported
                is Int -> LiveBots.Count(raw)
                else -> error("unknown live value $raw")
            }
            val body = BotsLogic.hostBody(
                connected = input.getBoolean("connected"),
                online = input.getBoolean("online"),
                paired = input.getBoolean("paired"),
                live = live,
                snapshotBots = if (input.isNull("snapshot")) null else input.getInt("snapshot"),
            )
            val expect = case.getJSONObject("expect")
            val label = case.getString("name")
            assertEquals(label, expect.getString("kind"), body.kind.key)
            assertEquals(label, expect.getString("source"), body.source.key)
            assertEquals(label, expect.getBoolean("dimmed"), body.dimmed)
            assertEquals(label, expect.getBoolean("actionable"), body.actionable)
            assertEquals(label, expect.getJSONArray("notes").let { array -> (0 until array.length()).map { array.getString(it) } }, body.notes.map { it.key })
        }
    }

    private fun snapshotOf(json: JSONObject): BotsSnapshot = BotsSnapshot(
        uid = "u1",
        hostId = json.getString("hostId"),
        savedAtMs = json.getLong("savedAtMs"),
        bots = json.getJSONArray("bots").objects().map { bot ->
            BotItem(
                botId = bot.getString("botId"),
                name = bot.getString("botId"),
                chats = if (bot.isNull("chats")) null else bot.getJSONArray("chats").strings().map { ChatItem(it, it) },
            )
        },
    )

    private fun JSONArray.strings(): List<String> = (0 until length()).map { getString(it) }

    /** (bot id, chat ids or null) per Bot, which is what the vectors state. */
    private fun shape(snapshot: BotsSnapshot) = snapshot.bots.map { it.botId to it.chats?.map { chat -> chat.sessionId } }

    private fun shapeOf(json: JSONObject) = json.getJSONArray("bots").objects().map { bot ->
        bot.getString("botId") to (if (bot.isNull("chats")) null else bot.getJSONArray("chats").strings())
    }

    @Test fun applyBotsMatchesTheSharedVectors() {
        val vector = SharedVectors.mobile.getJSONObject("snapshot")
        assertEquals(100, vector.getJSONObject("caps").getInt("botsPerHost"))
        val cases = vector.getJSONArray("applyBots").objects()
        assertEquals(3, cases.size)
        for (case in cases) {
            val label = case.getString("name")
            val prior = if (case.isNull("prior")) null else snapshotOf(case.getJSONObject("prior"))
            val live = case.getJSONArray("live").strings().map { BotItem(it, it) }
            val result = BotsLogic.applyBots(prior, "u1", case.getString("hostId"), live, case.getLong("now"))
            if (case.has("expect")) {
                val expect = case.getJSONObject("expect")
                assertEquals(label, expect.getString("hostId"), result.hostId)
                assertEquals(label, expect.getLong("savedAtMs"), result.savedAtMs)
                assertEquals(label, shapeOf(expect), shape(result))
            } else {
                assertEquals(label, case.getInt("expectCount"), result.bots.size)
                assertEquals(label, (1..100).map { "b$it" }, result.bots.map { it.botId })
                assertTrue(label, result.bots.all { it.chats == null })
            }
        }
    }

    @Test fun applyChatsMatchesTheSharedVectors() {
        val vector = SharedVectors.mobile.getJSONObject("snapshot")
        assertEquals(50, vector.getJSONObject("caps").getInt("chatsPerBot"))
        val cases = vector.getJSONArray("applyChats").objects()
        assertEquals(4, cases.size)
        for (case in cases) {
            val label = case.getString("name")
            val prior = if (case.isNull("prior")) null else snapshotOf(case.getJSONObject("prior"))
            val chats = case.getJSONArray("chats").strings().map { ChatItem(it, it) }
            val result = BotsLogic.applyChats(prior, case.getString("botId"), chats, case.getLong("now"))
            when {
                case.has("expect") && case.isNull("expect") -> assertNull(label, result)
                case.has("expect") -> {
                    val expect = case.getJSONObject("expect")
                    assertEquals(label, expect.getString("hostId"), result!!.hostId)
                    assertEquals(label, expect.getLong("savedAtMs"), result.savedAtMs)
                    assertEquals(label, shapeOf(expect), shape(result))
                }
                else -> {
                    val kept = result!!.bots.first { it.botId == case.getString("botId") }.chats!!.map { it.sessionId }
                    assertEquals(label, case.getInt("expectCount"), kept.size)
                    val firstLast = case.getJSONArray("expectFirstLast").strings()
                    assertEquals(label, firstLast, listOf(kept.first(), kept.last()))
                }
            }
        }
    }

    @Test fun unknownBotLeavesTheSnapshotTheSameInstance() {
        val prior = saved("h1", savedBot("a"))
        assertTrue(BotsLogic.applyChats(prior, "zzz", listOf(savedChat("x")), 99L) === prior)
    }

    @Test fun evictHostsKeepsTheTwentyNewest() {
        val vector = SharedVectors.mobile.getJSONObject("snapshot").getJSONObject("evictHosts")
        val times = vector.getJSONObject("savedAtMs")
        val snapshots = times.keys().asSequence().map { BotsSnapshot("u1", it, times.getLong(it), emptyList()) }.toList()
        assertEquals(22, snapshots.size)
        val expect = vector.getJSONArray("expect").strings()
        assertEquals(expect, BotsLogic.evictHosts(snapshots).map { it.hostId })
        assertEquals(BotsLogic.MAX_HOSTS, expect.size)
    }

    @Test fun autoConnectTargetMatchesTheSharedVectors() {
        val cases = SharedVectors.mobile.getJSONArray("autoConnectTarget").objects()
        assertEquals(7, cases.size)
        for (case in cases) {
            val input = case.getJSONObject("input")
            val hosts = input.getJSONArray("hosts").objects().map { host(it.getString("hostId"), it.getBoolean("online"), it.getBoolean("paired")) }
            val target = BotsLogic.autoConnectTarget(
                signedIn = input.getBoolean("signedIn"),
                connectedHostId = if (input.isNull("connectedHostId")) null else input.getString("connectedHostId"),
                lastHostId = if (input.isNull("lastHostId")) null else input.getString("lastHostId"),
                hosts = hosts,
            )
            val expect = if (case.isNull("expect")) null else case.getString("expect")
            assertEquals(case.getString("name"), expect, target)
        }
    }

    @Test fun botsBadgeCountMatchesTheSharedVector() {
        val vector = SharedVectors.mobile.getJSONObject("botsBadgeCount")
        val waiting = vector.getJSONArray("input").objects().map { it.getInt("waitingThreads") }
        assertEquals(vector.getInt("expect"), TabLogic.botsBadgeCount(waiting))
    }

    // --- sorting details ------------------------------------------------------------------------------------------

    @Test fun namesCompareByNumberAndIgnoreCase() {
        assertTrue(BotsLogic.compareNames("beta 2", "beta 10") < 0)
        assertTrue(BotsLogic.compareNames("beta 10", "beta 2") > 0)
        assertEquals(0, BotsLogic.compareNames("Alpha", "alpha"))
        assertTrue(BotsLogic.compareNames("a9", "a10") < 0)
        assertTrue(BotsLogic.compareNames("item 007", "item 7") == 0)
        assertTrue(BotsLogic.compareNames("item 7", "item 8") < 0)
        // A prefix comes before the longer name; a number before a letter at the same place.
        assertTrue(BotsLogic.compareNames("beta", "beta 2") < 0)
        assertTrue(BotsLogic.compareNames("1", "a") < 0)
        assertTrue(BotsLogic.compareNames("", "a") < 0)
        assertEquals(0, BotsLogic.compareNames("", ""))
    }

    @Test fun botsWithTheSameActivityAndNameKeepAStableOrderById() {
        val bots = listOf(BotItem("z", "Same"), BotItem("a", "same"), BotItem("m", "SAME"))
        assertEquals(listOf("a", "m", "z"), BotsLogic.sortBots(bots).map { it.botId })
    }

    @Test fun aBotWithNoActivitySortsAfterOneWithSome() {
        val bots = listOf(BotItem("a", "A", lastActivityAtMs = null), BotItem("b", "B", lastActivityAtMs = 1L), BotItem("c", "C", lastActivityAtMs = 0L))
        assertEquals(listOf("b", "a", "c"), BotsLogic.sortBots(bots).map { it.botId })
    }

    // --- the connection -------------------------------------------------------------------------------------------

    @Test fun onlyAReadyOrReconnectingOrRedialledLinkIsConnected() {
        fun connected(phase: RemoteConnectionPhaseFfi, everReady: Boolean = false) = BotsLogic.connectedHostId(connection("h1", phase, everReady))
        assertNull(BotsLogic.connectedHostId(null))
        assertNull(connected(RemoteConnectionPhaseFfi.IDLE))
        assertNull(connected(RemoteConnectionPhaseFfi.CONNECTING))
        assertEquals("h1", connected(RemoteConnectionPhaseFfi.CONNECTING, everReady = true))
        assertNull(connected(RemoteConnectionPhaseFfi.PAIRING))
        assertEquals("h1", connected(RemoteConnectionPhaseFfi.READY))
        assertEquals("h1", connected(RemoteConnectionPhaseFfi.RECONNECTING))
        assertNull(connected(RemoteConnectionPhaseFfi.FAILED))
    }

    @Test fun waitingChatsAreTheConnectedComputersBotsSummed() {
        val bots = liveBots("h1", liveBot("a", waiting = 2), liveBot("b"), liveBot("c", waiting = 3))
        assertEquals(5, BotsLogic.waitingChats(ready("h1"), bots))
        assertEquals(5, BotsLogic.waitingChats(connection("h1", RemoteConnectionPhaseFfi.RECONNECTING, everReady = true), bots))
    }

    @Test fun nothingWaitsWhileNothingIsConnectedOrTheBotsAreAnotherComputers() {
        val bots = liveBots("h1", liveBot("a", waiting = 2))
        assertEquals(0, BotsLogic.waitingChats(null, bots))
        assertEquals(0, BotsLogic.waitingChats(connection("h1", RemoteConnectionPhaseFfi.CONNECTING), bots))
        assertEquals(0, BotsLogic.waitingChats(connection("h1", RemoteConnectionPhaseFfi.FAILED), bots))
        assertEquals(0, BotsLogic.waitingChats(ready("h2"), bots))
        assertEquals(0, BotsLogic.waitingChats(ready("h1"), RemoteBotsState()))
        assertEquals(0, BotsLogic.waitingChats(ready("h1"), RemoteBotsState("h1", RemoteBotsStateFfi.Unsupported)))
    }

    @Test fun theLiveBotsOfAComputerAreOnlyThatComputers() {
        val held = liveBots("h1", liveBot("a", waiting = 3, status = RemoteBotStatusFfi.RUNNING, at = 77L), liveBot("b"))
        assertEquals(LiveBots.Count(2), held.liveFor("h1"))
        assertEquals(LiveBots.Unknown, held.liveFor("h2"))
        assertEquals(LiveBots.Unknown, RemoteBotsState().liveFor("h1"))
        assertEquals(LiveBots.Unsupported, RemoteBotsState("h1", RemoteBotsStateFfi.Unsupported).liveFor("h1"))
        assertTrue(held.itemsFor("h2").isEmpty())
        val a = held.itemsFor("h1").first()
        assertEquals(BotItem("a", "Bot a", "does a", RemoteBotStatusFfi.RUNNING, waitingThreads = 3, lastActivityAtMs = 77L), a)
    }

    @Test fun theLastUsedComputerIsOnlyDialledWhenNothingElseIs() {
        val hosts = listOf(host("h1"), host("h2", online = false))
        assertEquals("h1", BotsLogic.autoConnectTarget(true, null, "h1", hosts))
        assertNull(BotsLogic.autoConnectTarget(true, null, "h1", emptyList()))
        assertNull(BotsLogic.autoConnectTarget(true, "h2", "h1", hosts))
    }

    // --- the tab's whole-page states (section 4.5) ------------------------------------------------------------------

    @Test fun signedOutShowsTheGateWhateverElseIsHeld() {
        assertEquals(RootState.SignedOut, BotsLogic.rootState(false, RemoteHostsState(listOf(host("h1")), loaded = true)))
        assertEquals(RootState.SignedOut, BotsLogic.rootState(false, RemoteHostsState()))
    }

    @Test fun hostsThatAreNotLoadedShowTheSpinner() {
        assertEquals(RootState.Loading, BotsLogic.rootState(true, RemoteHostsState()))
    }

    @Test fun hostsThatFailedWithNothingToShowShowTheError() {
        assertEquals(RootState.Error, BotsLogic.rootState(true, RemoteHostsState(error = "offline", loaded = true)))
        // The error arrives before `loaded` does when the first reading fails; it still wins over the spinner.
        assertEquals(RootState.Error, BotsLogic.rootState(true, RemoteHostsState(error = "offline")))
    }

    @Test fun hostsThatFailedAfterBeingListedKeepTheTree() {
        assertEquals(RootState.Tree, BotsLogic.rootState(true, RemoteHostsState(listOf(host("h1")), loaded = true, error = "offline")))
    }

    @Test fun noComputersShowsTheHelp() {
        assertEquals(RootState.NoComputers, BotsLogic.rootState(true, RemoteHostsState(emptyList(), loaded = true)))
    }

    @Test fun listedComputersShowTheTree() {
        assertEquals(RootState.Tree, BotsLogic.rootState(true, RemoteHostsState(listOf(host("h1")), loaded = true)))
    }

    // --- the tree: one test per state of section 4.5 --------------------------------------------------------------

    @Test fun aConnectingComputerShowsItsSavedRowsAndTheConnectingNote() {
        val rows = tree(listOf(host("h1")), connection("h1", RemoteConnectionPhaseFfi.CONNECTING))
        assertEquals(listOf("host h1", "connection h1 Connecting", "state h1 connectToLoad"), lines(rows))
        assertEquals(RemoteConnectionUi.Connecting(everReady = false), (rows[1] as TreeRow.Connection).ui)
        assertEquals("Host h1", (rows[1] as TreeRow.Connection).hostName)
    }

    @Test fun dialingAnotherComputerKeepsThePreviousOnesSavedRowsVisible() {
        val rows = tree(
            listOf(host("h1"), host("h2")),
            connection("h2", RemoteConnectionPhaseFfi.CONNECTING),
            bots = liveBots("h1", liveBot("a")),
            snapshots = listOf(saved("h1", savedBot("a")), saved("h2", savedBot("x"))),
        )
        // The dial moved the connection: h1 is no longer connected, so it shows what was saved of it (dimmed), and
        // the computer being dialled shows its connecting note over its own saved rows.
        assertEquals(
            listOf(
                "host h1", "note h1 savedNote", "bot a dim",
                "host h2", "connection h2 Connecting", "note h2 savedNote", "bot x dim",
            ),
            lines(rows),
        )
    }

    @Test fun aReconnectingComputerStaysConnectedAndShowsTheReconnectingNote() {
        val rows = tree(
            listOf(host("h1")),
            connection("h1", RemoteConnectionPhaseFfi.RECONNECTING, everReady = true),
            bots = liveBots("h1", liveBot("a")),
        )
        assertEquals(listOf("host h1 connected", "connection h1 Reconnecting", "bot a"), lines(rows))
    }

    @Test fun aComputerThatWaitsForApprovalShowsTheCodeInItsSection() {
        val rows = tree(listOf(host("h1", paired = false)), connection("h1", RemoteConnectionPhaseFfi.PAIRING, pairingCode = "123456"))
        assertEquals(listOf("host h1", "connection h1 Pairing", "state h1 notPaired"), lines(rows))
        assertEquals("123456", ((rows[1] as TreeRow.Connection).ui as RemoteConnectionUi.Pairing).code)
    }

    @Test fun aFailedConnectionShowsItsMessageAndWhatWasSaved() {
        val failure = RemoteFailureFfi(RemoteFailureKindFfi.NO_ANSWER, null, false)
        val rows = tree(
            listOf(host("h1")),
            connection("h1", RemoteConnectionPhaseFfi.FAILED, failure = failure),
            snapshots = listOf(saved("h1", savedBot("a"))),
        )
        assertEquals(listOf("host h1", "connection h1 Failed", "note h1 savedNote", "bot a dim"), lines(rows))
        val failed = (rows[1] as TreeRow.Connection).ui as RemoteConnectionUi.Failed
        assertTrue(failed.canRetry)
    }

    @Test fun aReadyConnectionAddsNoNote() {
        val rows = tree(listOf(host("h1")), ready("h1"), bots = liveBots("h1", liveBot("a")))
        assertTrue(rows.none { it is TreeRow.Connection })
    }

    @Test fun aConnectionToAnotherComputerAddsNoNoteHere() {
        val rows = tree(listOf(host("h1"), host("h2")), connection("h2", RemoteConnectionPhaseFfi.FAILED))
        assertEquals(1, rows.count { it is TreeRow.Connection })
        assertEquals("h2", (rows.first { it is TreeRow.Connection } as TreeRow.Connection).hostId)
    }

    @Test fun connectedWithBotsNotDeliveredYetShowsLoading() {
        assertEquals(listOf("host h1 connected", "state h1 loading"), lines(tree(listOf(host("h1")), ready("h1"))))
        // The held Bots are another computer's: still nothing known about this one.
        assertEquals(
            listOf("host h1 connected", "state h1 loading"),
            lines(tree(listOf(host("h1")), ready("h1"), bots = liveBots("h2", liveBot("x")))),
        )
    }

    @Test fun connectedToAnOldDesktopShowsUnsupported() {
        val rows = tree(listOf(host("h1")), ready("h1"), bots = RemoteBotsState("h1", RemoteBotsStateFfi.Unsupported))
        assertEquals(listOf("host h1 connected", "state h1 unsupported"), lines(rows))
    }

    @Test fun connectedWithNoBotsShowsNone() {
        assertEquals(listOf("host h1 connected", "state h1 noBots"), lines(tree(listOf(host("h1")), ready("h1"), bots = liveBots("h1"))))
    }

    @Test fun anExpandedBotWhoseChatsLoadShowsLoadingAndNewChat() {
        val rows = tree(listOf(host("h1")), ready("h1"), bots = liveBots("h1", liveBot("a")), expanded = setOf("h1/a"))
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "botState a ChatsLoading"), lines(rows))
    }

    @Test fun anExpandedBotWhoseChatsFailedOffersARetry() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", null, RemoteSessionErrorFfi("timeout", "slow")),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "botState a ChatsFailed"), lines(rows))
    }

    @Test fun aFailedListingKeepsTheChatsThatWereHeld() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", listOf(liveChat("c1")), RemoteSessionErrorFfi("internal", "offline")),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "botState a ChatsFailed", "chat c1"), lines(rows))
    }

    @Test fun anExpandedBotWithNoChatsShowsTheNoChatsLineAndNewChat() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", emptyList()),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "botState a NoChats"), lines(rows))
    }

    @Test fun aBotThatIsNoLongerReachableSaysGoneAndOffersNoNewChat() {
        for (code in listOf("unknown_session", "not_remote_enabled")) {
            val rows = tree(
                listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
                chats = heldChats("h1", "a", null, RemoteSessionErrorFfi(code, "x")),
                expanded = setOf("h1/a"),
            )
            assertEquals(code, listOf("host h1 connected", "bot a open", "botState a Gone"), lines(rows))
        }
    }

    @Test fun aBotThatLeftTheListIsNoRowAtAll() {
        val rows = tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("b")), expanded = setOf("h1/a", "h1/b"))
        assertEquals(listOf("host h1 connected", "bot b open", "newChat b", "botState b ChatsLoading"), lines(rows))
    }

    @Test fun anOfflineComputerWithASnapshotShowsDimmedRowsAndBothNotes() {
        val rows = tree(
            listOf(host("h1", online = false, lastSeen = 5L)),
            snapshots = listOf(saved("h1", savedBot("a", chats = listOf(savedChat("s1"), savedChat("s2"))), at = 4_242L)),
            expanded = setOf("h1/a"),
        )
        assertEquals(
            listOf("host h1", "note h1 savedNote", "note h1 offlineChatsNote", "bot a open dim", "newChat a disabled", "chat s1 dim", "chat s2 dim"),
            lines(rows),
        )
        assertEquals(4_242L, (rows[1] as TreeRow.Note).savedAtMs)
        assertEquals(false, (rows.first { it is TreeRow.NewChat } as TreeRow.NewChat).enabled)
    }

    @Test fun anOfflineComputerWithoutASnapshotSaysOffline() {
        assertEquals(listOf("host h1", "state h1 offlineNoSnapshot"), lines(tree(listOf(host("h1", online = false)))))
    }

    @Test fun anOfflineAndUnpairedComputerSaysOfflineToo() {
        assertEquals(listOf("host h1", "state h1 offlineNoSnapshot"), lines(tree(listOf(host("h1", online = false, paired = false)))))
    }

    @Test fun anUnpairedOnlineComputerOffersToConnectAndIgnoresAnySnapshot() {
        val rows = tree(listOf(host("h1", paired = false)), snapshots = listOf(saved("h1", savedBot("a"))))
        assertEquals(listOf("host h1", "state h1 notPaired"), lines(rows))
    }

    @Test fun anOnlinePairedComputerThatNeverConnectedOffersToConnect() {
        assertEquals(listOf("host h1", "state h1 connectToLoad"), lines(tree(listOf(host("h1")))))
    }

    @Test fun aSavedComputerWithNoBotsSaysNoneWithItsNote() {
        val rows = tree(listOf(host("h1")), snapshots = listOf(saved("h1")))
        assertEquals(listOf("host h1", "note h1 savedNote", "state h1 noBots"), lines(rows))
    }

    @Test fun anOnlineComputerThatIsNotConnectedShowsDimmedRowsOnlyWithTheSavedNote() {
        val rows = tree(listOf(host("h1")), snapshots = listOf(saved("h1", savedBot("a"), savedBot("b"))))
        assertEquals(listOf("host h1", "note h1 savedNote", "bot a dim", "bot b dim"), lines(rows))
    }

    @Test fun expandingABotOnAnotherOnlineComputerShowsItsSavedChatsAndAllowsNewChat() {
        val rows = tree(
            listOf(host("h1")),
            snapshots = listOf(saved("h1", savedBot("a", chats = listOf(savedChat("s1"))))),
            expanded = setOf("h1/a"),
        )
        // New chat connects first, so it stays enabled; the saved chat is dimmed until the live list replaces it.
        assertEquals(listOf("host h1", "note h1 savedNote", "bot a open dim", "newChat a", "chat s1 dim"), lines(rows))
    }

    @Test fun aSavedBotWhoseChatsWereNeverListedLoadsThemOnlyWhileTheComputerIsOnline() {
        val bots = listOf(saved("h1", savedBot("a")))
        assertEquals(
            listOf("host h1", "note h1 savedNote", "bot a open dim", "newChat a", "botState a ChatsLoading"),
            lines(tree(listOf(host("h1")), snapshots = bots, expanded = setOf("h1/a"))),
        )
        assertEquals(
            listOf("host h1", "note h1 savedNote", "note h1 offlineChatsNote", "bot a open dim", "newChat a disabled"),
            lines(tree(listOf(host("h1", online = false)), snapshots = bots, expanded = setOf("h1/a"))),
        )
    }

    @Test fun aSavedBotWithNoSavedChatsSaysSo() {
        val rows = tree(listOf(host("h1")), snapshots = listOf(saved("h1", savedBot("a", chats = emptyList()))), expanded = setOf("h1/a"))
        assertEquals(listOf("host h1", "note h1 savedNote", "bot a open dim", "newChat a", "botState a NoChats"), lines(rows))
    }

    @Test fun theLiveListReplacesTheCacheOnceTheComputerIsConnected() {
        val snapshot = saved("h1", savedBot("a", chats = listOf(savedChat("old"))), savedBot("gone"))
        val before = tree(listOf(host("h1")), snapshots = listOf(snapshot), expanded = setOf("h1/a"))
        assertEquals(
            listOf("host h1", "note h1 savedNote", "bot a open dim", "newChat a", "chat old dim", "bot gone dim"),
            lines(before),
        )
        // Connected: the Bots the computer delivers, undimmed, with no saved note; its chats not yet delivered keep the saved ones dimmed.
        val during = tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a"), liveBot("fresh")), snapshots = listOf(snapshot), expanded = setOf("h1/a"))
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "chat old dim", "bot fresh"), lines(during))
        // The live chats arrive and replace the saved ones.
        val after = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a"), liveBot("fresh")),
            chats = heldChats("h1", "a", listOf(liveChat("new1"), liveChat("new2"))),
            snapshots = listOf(snapshot), expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "chat new1", "chat new2", "bot fresh"), lines(after))
    }

    @Test fun anEmptyLiveListReplacesSavedChatsToo() {
        val snapshot = saved("h1", savedBot("a", chats = listOf(savedChat("old"))))
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", emptyList()), snapshots = listOf(snapshot), expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "botState a NoChats"), lines(rows))
    }

    @Test fun aCollapsedComputerShowsOnlyItsHeader() {
        val rows = tree(
            listOf(host("h1"), host("h2")), ready("h1"), liveBots("h1", liveBot("a")),
            collapsed = setOf("h1"),
        )
        assertEquals(listOf("host h1 connected collapsed", "host h2", "state h2 connectToLoad"), lines(rows))
    }

    @Test fun aCollapsedComputerHidesItsConnectionNoteToo() {
        val rows = tree(listOf(host("h1")), connection("h1", RemoteConnectionPhaseFfi.CONNECTING), collapsed = setOf("h1"))
        assertEquals(listOf("host h1 collapsed"), lines(rows))
    }

    // --- ordering and windows ------------------------------------------------------------------------------------

    @Test fun computersAreOrderedConnectedFirstThenOnlineThenLastSeen() {
        val rows = tree(
            listOf(host("old", online = false, lastSeen = 10L), host("on", lastSeen = 1L), host("c", online = false, lastSeen = 2L), host("recent", online = false, lastSeen = 99L)),
            connection = ready("c"),
            bots = liveBots("c"),
        )
        assertEquals(listOf("c", "on", "recent", "old"), rows.filterIsInstance<TreeRow.HostHeader>().map { it.host.hostId })
    }

    @Test fun botsAreOrderedByActivityThenName() {
        val rows = tree(
            listOf(host("h1")), ready("h1"),
            liveBots("h1", liveBot("quiet", name = "Quiet"), liveBot("busy", at = 9_000L, name = "Zed"), liveBot("mid", at = 5_000L, name = "Mid"), liveBot("alpha", name = "alpha")),
        )
        assertEquals(listOf("busy", "mid", "alpha", "quiet"), rows.filterIsInstance<TreeRow.Bot>().map { it.bot.botId })
    }

    @Test fun chatsKeepTheHostsOrderAndAreNeverSorted() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", listOf(liveChat("c3", title = "Zulu"), liveChat("c1", title = "Alpha"), liveChat("c2", title = "Mike"))),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("c3", "c1", "c2"), rows.filterIsInstance<TreeRow.Chat>().map { it.chat.sessionId })
    }

    @Test fun savedChatsKeepTheHostsOrderToo() {
        val rows = tree(
            listOf(host("h1")),
            snapshots = listOf(saved("h1", savedBot("a", chats = listOf(savedChat("z"), savedChat("a"), savedChat("m"))))),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("z", "a", "m"), rows.filterIsInstance<TreeRow.Chat>().map { it.chat.sessionId })
    }

    private fun manyChats(count: Int) = (1..count).map { liveChat("c$it") }

    @Test fun anExpandedBotShowsFiveChatsThenAskToShowTheRest() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", manyChats(8)), expanded = setOf("h1/a"),
        )
        assertEquals(
            listOf("host h1 connected", "bot a open", "newChat a", "chat c1", "chat c2", "chat c3", "chat c4", "chat c5", "more a 3"),
            lines(rows),
        )
    }

    @Test fun showingAllChatsOffersToShowFewer() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", manyChats(8)), expanded = setOf("h1/a"), showAll = setOf("h1/a"),
        )
        assertEquals((1..8).map { "chat c$it" } + "less a", lines(rows).drop(3))
    }

    @Test fun fiveChatsOrFewerOfferNothingToExpand() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", manyChats(5)), expanded = setOf("h1/a"),
        )
        assertTrue(rows.none { it is TreeRow.ShowMore || it is TreeRow.ShowLess })
        assertEquals(5, rows.count { it is TreeRow.Chat })
    }

    @Test fun theWindowIsPerBot() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a"), liveBot("b")),
            chats = RemoteBotChatsState("h1", mapOf("a" to RemoteBotThreadsStateFfi(manyChats(8), null), "b" to RemoteBotThreadsStateFfi(manyChats(8), null))),
            expanded = setOf("h1/a", "h1/b"), showAll = setOf("h1/a"),
        )
        assertEquals(8, sectionOf(rows, "h1").takeWhile { !(it is TreeRow.Bot && it.bot.botId == "b") }.count { it is TreeRow.Chat })
        assertEquals(5, rows.dropWhile { !(it is TreeRow.Bot && it.bot.botId == "b") }.count { it is TreeRow.Chat })
        assertEquals(1, rows.count { it is TreeRow.ShowMore })
        assertEquals(1, rows.count { it is TreeRow.ShowLess })
    }

    // --- expansion defaults ---------------------------------------------------------------------------------------

    @Test fun withNoChoiceTheFirstBotOfTheConnectedComputerIsOpen() {
        val bots = liveBots("h1", liveBot("late", name = "Late"), liveBot("first", at = 9L, name = "First"))
        assertEquals(setOf("h1/first"), BotsLogic.defaultExpanded("h1", bots))
        val rows = tree(listOf(host("h1")), ready("h1"), bots, expanded = null)
        assertEquals(listOf("first"), rows.filterIsInstance<TreeRow.Bot>().filter { it.expanded }.map { it.bot.botId })
    }

    @Test fun anExplicitChoiceOverridesTheDefaultEvenWhenEmpty() {
        val bots = liveBots("h1", liveBot("a"), liveBot("b"))
        assertTrue(tree(listOf(host("h1")), ready("h1"), bots, expanded = emptySet()).filterIsInstance<TreeRow.Bot>().none { it.expanded })
        assertEquals(listOf("b"), tree(listOf(host("h1")), ready("h1"), bots, expanded = setOf("h1/b")).filterIsInstance<TreeRow.Bot>().filter { it.expanded }.map { it.bot.botId })
    }

    @Test fun theDefaultNeedsAConnectedComputerWithBots() {
        assertTrue(BotsLogic.defaultExpanded(null, liveBots("h1", liveBot("a"))).isEmpty())
        assertTrue(BotsLogic.defaultExpanded("h1", RemoteBotsState()).isEmpty())
        assertTrue(BotsLogic.defaultExpanded("h1", liveBots("h1")).isEmpty())
        assertTrue(BotsLogic.defaultExpanded("h2", liveBots("h1", liveBot("a"))).isEmpty())
        val rows = tree(listOf(host("h1"), host("h2")), ready("h1"), liveBots("h1", liveBot("a")), snapshots = listOf(saved("h2", savedBot("x"))), expanded = null)
        assertEquals(listOf("a"), rows.filterIsInstance<TreeRow.Bot>().filter { it.expanded }.map { it.bot.botId })
    }

    @Test fun aKeyNamesTheComputerAndTheBot() {
        assertEquals("h1/b1", BotsLogic.botKey("h1", "b1"))
    }

    // --- New chat while the link is not ready -----------------------------------------------------------------------

    @Test fun newChatWaitsForTheLinkToBeReady() {
        for (phase in listOf(RemoteConnectionPhaseFfi.RECONNECTING, RemoteConnectionPhaseFfi.CONNECTING)) {
            val rows = tree(
                listOf(host("h1")), connection("h1", phase, everReady = true), liveBots("h1", liveBot("a")),
                chats = heldChats("h1", "a", emptyList()), expanded = setOf("h1/a"),
            )
            assertFalse(phase.name, (rows.first { it is TreeRow.NewChat } as TreeRow.NewChat).enabled)
        }
        val ready = tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")), expanded = setOf("h1/a"))
        assertTrue((ready.first { it is TreeRow.NewChat } as TreeRow.NewChat).enabled)
    }

    // --- chat markers ---------------------------------------------------------------------------------------------

    @Test fun aChatKeepsItsStatusAndPendingAction() {
        val rows = tree(
            listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("a")),
            chats = heldChats("h1", "a", listOf(liveChat("c1", status = RemoteSessionStatusFfi.RUNNING), liveChat("c2", pending = RemotePendingActionFfi.QUESTION, status = RemoteSessionStatusFfi.REQUIRES_ACTION))),
            expanded = setOf("h1/a"),
        )
        val chats = rows.filterIsInstance<TreeRow.Chat>().map { it.chat }
        assertEquals(RemoteSessionStatusFfi.RUNNING, chats[0].status)
        assertNull(chats[0].pendingAction)
        assertEquals(RemotePendingActionFfi.QUESTION, chats[1].pendingAction)
        assertEquals("c1", chats[0].sessionId)
    }

    @Test fun aBlankTitleIsKeptAsItIsForTheRowToName() {
        val item = liveChat("c1", title = "").toItem()
        assertEquals("", item.title)
    }

    // --- robustness -------------------------------------------------------------------------------------------------

    @Test fun everyRowOfAMixedTreeHasItsOwnKey() {
        val snapshot = saved("h2", savedBot("a", chats = listOf(savedChat("s1"), savedChat("s2"))), savedBot("b"))
        val rows = tree(
            listOf(host("h1"), host("h2", online = false), host("h3", paired = false), host("h4")),
            ready("h1"),
            liveBots("h1", liveBot("a"), liveBot("b")),
            chats = heldChats("h1", "a", manyChats(7)),
            snapshots = listOf(snapshot),
            expanded = setOf("h1/a", "h1/b", "h2/a", "h2/b"),
            collapsed = setOf("h4"),
        )
        val keys = rows.map { it.key }
        assertEquals(keys.size, keys.toSet().size)
        assertTrue(keys.isNotEmpty())
    }

    @Test fun keysOfDifferentComputersNeverCollide() {
        val rows = tree(
            listOf(host("h1"), host("h2")),
            snapshots = listOf(saved("h1", savedBot("a", chats = listOf(savedChat("s")))), saved("h2", savedBot("a", chats = listOf(savedChat("s"))))),
            expanded = setOf("h1/a", "h2/a"),
        )
        assertEquals(rows.size, rows.map { it.key }.toSet().size)
    }

    @Test fun idsASourceRepeatsNeverMakeTwoRowsWithOneKey() {
        val rows = tree(
            listOf(host("h1"), host("h1")), ready("h1"),
            liveBots("h1", liveBot("a"), liveBot("a")),
            chats = heldChats("h1", "a", listOf(liveChat("c1"), liveChat("c1"), liveChat("c2"))),
            expanded = setOf("h1/a"),
        )
        assertEquals(listOf("host h1 connected", "bot a open", "newChat a", "chat c1", "chat c2"), lines(rows))
        assertEquals(rows.size, rows.map { it.key }.toSet().size)
    }

    @Test fun aSavedAndALiveBotListNeverMixInOneSection() {
        val snapshot = saved("h1", savedBot("old", name = "Old"))
        val live = tree(listOf(host("h1")), ready("h1"), liveBots("h1", liveBot("now")), snapshots = listOf(snapshot))
        assertEquals(listOf("now"), live.filterIsInstance<TreeRow.Bot>().map { it.bot.botId })
        val cached = tree(listOf(host("h1")), snapshots = listOf(snapshot))
        assertEquals(listOf("old"), cached.filterIsInstance<TreeRow.Bot>().map { it.bot.botId })
    }

    // --- the gestures (section 4.3) ---------------------------------------------------------------------------------

    private fun facts(connected: Boolean, online: Boolean, paired: Boolean) = HostFacts("h1", connected, online, paired)

    @Test fun tappingABotOnTheConnectedComputerOnlyToggles() {
        val connected = facts(connected = true, online = true, paired = true)
        assertEquals(BotTapPlan(expanded = true, connect = false), BotsLogic.botTapPlan(connected, wasExpanded = false))
        assertEquals(BotTapPlan(expanded = false, connect = false), BotsLogic.botTapPlan(connected, wasExpanded = true))
        // Presence may lag behind the link; a connected computer never dials again.
        assertEquals(BotTapPlan(expanded = true, connect = false), BotsLogic.botTapPlan(facts(true, online = false, paired = true), false))
    }

    @Test fun expandingABotOnAnotherOnlinePairedComputerStartsOneConnection() {
        val other = facts(connected = false, online = true, paired = true)
        assertEquals(BotTapPlan(expanded = true, connect = true), BotsLogic.botTapPlan(other, wasExpanded = false))
        // Collapsing it again connects nothing.
        assertEquals(BotTapPlan(expanded = false, connect = false), BotsLogic.botTapPlan(other, wasExpanded = true))
    }

    @Test fun expandingABotOnAnOfflineComputerNeverConnects() {
        for (paired in listOf(true, false)) {
            val offline = facts(connected = false, online = false, paired = paired)
            assertEquals(BotTapPlan(expanded = true, connect = false), BotsLogic.botTapPlan(offline, wasExpanded = false))
            assertEquals(BotTapPlan(expanded = false, connect = false), BotsLogic.botTapPlan(offline, wasExpanded = true))
        }
    }

    @Test fun expandingABotOnAnUnpairedComputerOnlyToggles() {
        val unpaired = facts(connected = false, online = true, paired = false)
        assertEquals(BotTapPlan(expanded = true, connect = false), BotsLogic.botTapPlan(unpaired, wasExpanded = false))
    }

    @Test fun tappingAChatFollowsTheComputersState() {
        assertEquals(OpenChatPlan.OpenNow, BotsLogic.openChatPlan(facts(true, true, true)))
        assertEquals(OpenChatPlan.OpenNow, BotsLogic.openChatPlan(facts(true, online = false, paired = true)))
        assertEquals(OpenChatPlan.ConnectThenOpen, BotsLogic.openChatPlan(facts(false, true, true)))
        assertEquals(OpenChatPlan.NoAnswer, BotsLogic.openChatPlan(facts(false, online = false, paired = true)))
        assertEquals(OpenChatPlan.NoAnswer, BotsLogic.openChatPlan(facts(false, online = false, paired = false)))
        assertEquals(OpenChatPlan.Pair, BotsLogic.openChatPlan(facts(false, online = true, paired = false)))
    }

    @Test fun newChatFollowsTheComputersState() {
        assertEquals(NewChatPlan.CreateNow, BotsLogic.newChatPlan(facts(true, true, true)))
        assertEquals(NewChatPlan.CreateNow, BotsLogic.newChatPlan(facts(true, online = false, paired = true)))
        assertEquals(NewChatPlan.ConnectThenCreate, BotsLogic.newChatPlan(facts(false, true, true)))
        assertEquals(NewChatPlan.Disabled, BotsLogic.newChatPlan(facts(false, online = false, paired = true)))
        assertEquals(NewChatPlan.Disabled, BotsLogic.newChatPlan(facts(false, online = true, paired = false)))
        assertEquals(NewChatPlan.Disabled, BotsLogic.newChatPlan(facts(false, online = false, paired = false)))
    }

    // --- relative times -------------------------------------------------------------------------------------------

    @Test fun ageStepsFollowTheSessionListsBoundaries() {
        val now = 10_000_000_000L
        fun age(seconds: Long) = ageOf(now - seconds * 1000, now)
        assertEquals(Age.JustNow, age(0))
        assertEquals(Age.JustNow, age(59))
        assertEquals(Age.Minutes(1), age(60))
        assertEquals(Age.Minutes(59), age(3_599))
        assertEquals(Age.Hours(1), age(3_600))
        assertEquals(Age.Hours(23), age(86_399))
        assertEquals(Age.Yesterday, age(86_400))
        assertEquals(Age.Yesterday, age(172_799))
        assertEquals(Age.Days(2), age(172_800))
        assertEquals(Age.Days(6), age(604_799))
        assertEquals(Age.Date, age(604_800))
        assertEquals(Age.Date, age(86_400L * 400))
    }

    @Test fun aTimeInTheFutureReadsAsJustNow() {
        assertEquals(Age.JustNow, ageOf(10_000L, 5_000L))
    }

    // --- the conversions from the FFI types -----------------------------------------------------------------------

    @Test fun aLiveBotBecomesAnItemWithoutChats() {
        val item = liveBot("a", waiting = 4, status = RemoteBotStatusFfi.REQUIRES_ACTION, at = 123L).toItem()
        assertEquals(BotItem("a", "Bot a", "does a", RemoteBotStatusFfi.REQUIRES_ACTION, 4, 123L, null), item)
        assertNull(liveBot("b").toItem().lastActivityAtMs)
    }

    @Test fun aLiveChatBecomesAnItemKeepingItsMarkers() {
        val item = liveChat("c", status = RemoteSessionStatusFfi.REQUIRES_ACTION, pending = RemotePendingActionFfi.PERMISSION).toItem()
        assertEquals(ChatItem("c", "Chat c", RemoteSessionStatusFfi.REQUIRES_ACTION, RemotePendingActionFfi.PERMISSION), item)
    }
}
