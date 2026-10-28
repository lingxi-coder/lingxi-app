package com.lingxi.code.remote

import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotThreadsStateFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteFailureKindFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionStatusFfi
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteBotsTest {
    private fun bot(id: String, waiting: UInt = 0u, status: RemoteBotStatusFfi = RemoteBotStatusFfi.IDLE) =
        RemoteBotSummaryFfi(id, "Bot $id", "does $id", status, waiting, null)

    private fun chat(id: String) = RemoteSessionSummaryFfi(id, "Chat $id", "", RemoteSessionStatusFfi.IDLE, null, null, 0u)

    private fun bots(hostId: String?, vararg listed: RemoteBotSummaryFfi) = RemoteBotsState(hostId, RemoteBotsStateFfi.Bots(listed.toList()))

    private fun chats(hostId: String, botId: String, threads: List<RemoteSessionSummaryFfi>?, error: RemoteSessionErrorFfi? = null) =
        RemoteBotChatsState(hostId, mapOf(botId to RemoteBotThreadsStateFfi(threads, error)))

    private fun page(bots: RemoteBotsState, chats: RemoteBotChatsState = RemoteBotChatsState()) =
        remoteBotPageUi("h", "b1", bots, chats)

    // --- the host screen's section ---------------------------------------

    @Test fun theHostScreenListsBotsOnlyWhenTheComputerHasSomeToShow() {
        assertTrue(RemoteBotsState().listFor("h").isEmpty())
        assertTrue(RemoteBotsState("h", RemoteBotsStateFfi.Unknown).listFor("h").isEmpty())
        assertTrue(RemoteBotsState("h", RemoteBotsStateFfi.Unsupported).listFor("h").isEmpty())
        assertTrue(bots("h").listFor("h").isEmpty())
        assertEquals(listOf("b1", "b2"), bots("h", bot("b1"), bot("b2")).listFor("h").map { it.botId })
    }

    @Test fun anotherComputersBotsAreNeverListed() {
        val held = bots("other", bot("b1"))
        assertTrue(held.listFor("h").isEmpty())
        assertNull(held.nameOf("h", "b1"))
        assertEquals("Bot b1", held.nameOf("other", "b1"))
    }

    @Test fun aBotsNameIsFoundAmongTheListedOnes() {
        val held = bots("h", bot("b1"), bot("b2"))
        assertEquals("Bot b2", held.nameOf("h", "b2"))
        assertNull(held.nameOf("h", "missing"))
    }

    // --- a Bot's chats, found by session ---------------------------------

    @Test fun aChatIsFoundAmongTheChatsOfAnyBotOfItsComputer() {
        val held = RemoteBotChatsState(
            "h",
            mapOf(
                "b1" to RemoteBotThreadsStateFfi(listOf(chat("s1")), null),
                "b2" to RemoteBotThreadsStateFfi(listOf(chat("s2")), null),
                "b3" to RemoteBotThreadsStateFfi(null, RemoteSessionErrorFfi("internal", "x")),
            ),
        )
        assertEquals("Chat s2", held.summaryOf("h", "s2")?.title)
        assertNull(held.summaryOf("h", "s3"))
        assertNull(held.summaryOf("other", "s2"))
    }

    // --- the Bot page ----------------------------------------------------

    @Test fun theBotPageWaitsWhileTheComputersBotsAreNotKnown() {
        assertEquals(RemoteBotPageUi.Loading, page(RemoteBotsState()))
        assertEquals(RemoteBotPageUi.Loading, page(RemoteBotsState("h", RemoteBotsStateFfi.Unknown)))
        // The held Bots are another computer's: nothing is known of this one's yet.
        assertEquals(RemoteBotPageUi.Loading, page(bots("other", bot("b1"))))
    }

    @Test fun theBotPageSaysTheBotIsGoneWhenTheComputerDoesNotKnowBots() {
        assertEquals(RemoteBotPageUi.Gone, page(RemoteBotsState("h", RemoteBotsStateFfi.Unsupported)))
    }

    @Test fun theBotPageSaysTheBotIsGoneWhenItIsNotInTheList() {
        assertEquals(RemoteBotPageUi.Gone, page(bots("h")))
        assertEquals(RemoteBotPageUi.Gone, page(bots("h", bot("b2"))))
    }

    @Test fun theBotPageShowsTheBotWhileItsChatsLoad() {
        val shown = page(bots("h", bot("b1"), bot("b2"))) as RemoteBotPageUi.Shown
        assertEquals("b1", shown.bot.botId)
        assertTrue(shown.chats.loading)
        assertFalse(shown.chats.empty)
        assertFalse(shown.chats.failed)
        // Another Bot's chats, or another computer's, say nothing of this one's.
        assertTrue((page(bots("h", bot("b1")), chats("h", "b2", emptyList())) as RemoteBotPageUi.Shown).chats.loading)
        assertTrue((page(bots("h", bot("b1")), chats("other", "b1", emptyList())) as RemoteBotPageUi.Shown).chats.loading)
    }

    @Test fun theBotPageShowsNoChatsYetAndTheChatsNewestFirstAsTheyCome() {
        val none = page(bots("h", bot("b1")), chats("h", "b1", emptyList())) as RemoteBotPageUi.Shown
        assertTrue(none.chats.empty)
        assertFalse(none.chats.loading)
        val some = page(bots("h", bot("b1")), chats("h", "b1", listOf(chat("new"), chat("old")))) as RemoteBotPageUi.Shown
        assertEquals(listOf("new", "old"), some.chats.threads?.map { it.sessionId })
        assertFalse(some.chats.empty)
        assertFalse(some.chats.loading)
    }

    @Test fun aFailedListingKeepsWhatWasHeldAndOffersARetry() {
        val failed = page(
            bots("h", bot("b1")),
            chats("h", "b1", listOf(chat("s1")), RemoteSessionErrorFfi("internal", "offline")),
        ) as RemoteBotPageUi.Shown
        assertTrue(failed.chats.failed)
        assertFalse(failed.chats.loading)
        assertEquals(listOf("s1"), failed.chats.threads?.map { it.sessionId })
        // Failing before anything was listed is a failure, not a spinner.
        val first = page(bots("h", bot("b1")), chats("h", "b1", null, RemoteSessionErrorFfi("timeout", "slow"))) as RemoteBotPageUi.Shown
        assertTrue(first.chats.failed)
        assertFalse(first.chats.loading)
    }

    @Test fun aRefusalThatSaysTheBotIsUnreachableIsTheBotBeingGone() {
        for (code in listOf("unknown_session", "not_remote_enabled")) {
            assertEquals(
                code,
                RemoteBotPageUi.Gone,
                page(bots("h", bot("b1")), chats("h", "b1", null, RemoteSessionErrorFfi(code, "x"))),
            )
        }
        assertTrue(page(bots("h", bot("b1")), chats("h", "b1", null, RemoteSessionErrorFfi("capacity", "x"))) is RemoteBotPageUi.Shown)
    }

    // --- words -----------------------------------------------------------

    @Test fun aBotsStatusReusesTheSessionLabels() {
        assertEquals(R.string.remote_status_idle, botStatusText(RemoteBotStatusFfi.IDLE))
        assertEquals(R.string.remote_status_running, botStatusText(RemoteBotStatusFfi.RUNNING))
        assertEquals(R.string.remote_status_requires_action, botStatusText(RemoteBotStatusFfi.REQUIRES_ACTION))
        assertEquals(sessionStatusText(RemoteSessionStatusFfi.IDLE), botStatusText(RemoteBotStatusFfi.IDLE))
        assertEquals(sessionStatusText(RemoteSessionStatusFfi.RUNNING), botStatusText(RemoteBotStatusFfi.RUNNING))
        assertEquals(sessionStatusText(RemoteSessionStatusFfi.REQUIRES_ACTION), botStatusText(RemoteBotStatusFfi.REQUIRES_ACTION))
    }

    @Test fun aChatThatCouldNotBeStartedIsExplainedByWhyTheComputerRefused() {
        assertEquals(R.string.remote_bot_gone, botCreateErrorText(RemoteFfiException.Rpc("unknown_session", "")))
        assertEquals(R.string.remote_bot_gone, botCreateErrorText(RemoteFfiException.Rpc("not_remote_enabled", "")))
        assertEquals(R.string.remote_err_capacity, botCreateErrorText(RemoteFfiException.Rpc("capacity", "")))
        assertEquals(R.string.remote_err_forbidden, botCreateErrorText(RemoteFfiException.Rpc("forbidden", "")))
        assertEquals(R.string.remote_err_generic, botCreateErrorText(RemoteFfiException.Rpc("closed", "")))
        assertEquals(R.string.remote_err_generic, botCreateErrorText(RemoteFfiException.NotConnected()))
        assertEquals(R.string.remote_err_generic, botCreateErrorText(IllegalStateException("remote computer changed")))
    }

    @Test fun theBotPageSaysSoWhileTheComputerIsNotConnected() {
        assertNull(botConnectionNote(RemoteConnectionUi.Ready(RemoteTransportFfi.WEBRTC)))
        assertEquals(R.string.remote_reconnecting, botConnectionNote(RemoteConnectionUi.Idle))
        assertEquals(R.string.remote_reconnecting, botConnectionNote(RemoteConnectionUi.Connecting(everReady = true)))
        assertEquals(R.string.remote_reconnecting, botConnectionNote(RemoteConnectionUi.Reconnecting(null, null)))
        assertEquals(R.string.remote_pairing_waiting, botConnectionNote(RemoteConnectionUi.Pairing("123456")))
        val failed = RemoteConnectionUi.Failed(R.string.remote_fail_revoked, RemoteFailureKindFfi.REVOKED, canRetry = false, repair = true, forget = false)
        assertEquals(R.string.remote_fail_revoked, botConnectionNote(failed))
    }
}
