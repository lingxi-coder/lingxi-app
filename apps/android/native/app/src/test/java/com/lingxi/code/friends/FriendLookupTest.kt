package com.lingxi.code.friends

import com.lingxi.code.teams.TeamMessageFailure
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException

/**
 * The "look somebody up, then ask" flow (docs/FRIENDS-CLIENTS.md D6, D7, D8, D15, D18): the lookup only fills the card, the confirming tap
 * picks `redeem`, `accept` or `request`, and the request id lives exactly as long as one confirmation that may have to be retried.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class FriendLookupTest {
    private val code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-"

    private fun found(relation: String) = JSONObject("""{"uid":"u2","displayName":"Ann","handle":"ann","relation":"$relation"}""")

    private class Harness(scope: CoroutineScope) {
        val calls = mutableListOf<Map<String, Any>>()
        val events = mutableListOf<String>()
        val outcomes = mutableListOf<FriendOutcome>()
        var answer: suspend (Map<String, Any>) -> JSONObject = { throw AssertionError("unexpected call $it") }
        var arrived: suspend (String) -> Unit = {}
        val lookup = FriendLookup(
            scope,
            post = { body -> calls += body; answer(body) },
            awaitFriend = { uid -> events += "await:$uid"; arrived(uid) },
            announce = { outcomes += it; events += "announce:$it" },
        )
        val state get() = lookup.state.value
        val confirmations get() = calls.filter { it["action"] != "lookup" }
    }

    private fun Harness.answers(relation: String, confirmation: JSONObject = JSONObject("""{"uid":"u2","state":"outgoing"}""")) {
        answer = { body -> if (body["action"] == "lookup") found(relation) else confirmation }
    }

    // --- looking up -------------------------------------------------------------------------------------------

    @Test fun aLookupOnlyFillsTheCardAndSendsNothingElse() = runTest {
        val h = Harness(this)
        h.answers("none")
        h.lookup.lookup(FriendProof.Handle("ann"))
        assertTrue("the lookup is in flight as soon as it starts", h.state.looking)
        advanceUntilIdle()
        assertEquals(listOf(mapOf("action" to "lookup", "handle" to "ann")), h.calls)
        assertEquals(LookupResult("u2", "Ann", "ann", FriendRelation.None), h.state.result)
        assertEquals(FriendProof.Handle("ann"), h.state.proof)
        assertFalse(h.state.looking)
        assertNull(h.state.error)
        assertNull(h.state.outcome)
    }

    @Test fun aLookupShowsItsProgressAndAFailureShowsItsError() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<JSONObject>()
        h.answer = { gate.await() }
        h.lookup.lookup(FriendProof.Email("a@b.co"))
        runCurrent()
        assertTrue(h.state.looking)
        assertEquals(FriendProof.Email("a@b.co"), h.state.proof)
        gate.completeExceptionally(TeamMessageFailure("NOT_FOUND"))
        advanceUntilIdle()
        assertFalse(h.state.looking)
        assertEquals(FriendsError.NotFound, h.state.error)
        assertNull(h.state.result)
    }

    @Test fun aFailureOfTheTransportIsANetworkErrorAndTheProofStaysForRetry() = runTest {
        val h = Harness(this)
        h.answer = { throw IOException("offline") }
        h.lookup.lookup(FriendProof.Code(code))
        advanceUntilIdle()
        assertEquals(FriendsError.Network, h.state.error)
        assertFalse(h.state.error!!.definitive)
        assertEquals(FriendProof.Code(code), h.state.proof)
        h.answers("none")
        h.lookup.lookup(h.state.proof!!)
        advanceUntilIdle()
        assertNull("a new lookup drops the old error", h.state.error)
        assertEquals(FriendRelation.None, h.state.result?.relation)
    }

    @Test fun aNewLookupReplacesTheCardAndDropsTheRequestId() = runTest {
        val h = Harness(this)
        h.answer = { body -> if (body["action"] == "lookup") found("none") else throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("hi")
        advanceUntilIdle()
        val first = h.confirmations.single()["requestId"]
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("hi")
        advanceUntilIdle()
        assertNotEquals("the id belongs to one lookup", first, h.confirmations.last()["requestId"])
    }

    // --- confirming ---------------------------------------------------------------------------------------------

    @Test fun aHandleRequestCarriesTheRequestIdAndTheTrimmedNote() = runTest {
        val h = Harness(this)
        h.answers("none")
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("  hello  ")
        advanceUntilIdle()
        val body = h.confirmations.single()
        assertEquals("request", body["action"])
        assertEquals("handle", body["via"])
        assertEquals("ann", body["handle"])
        assertEquals("hello", body["note"])
        assertTrue(FriendsLogic.isRequestId(body["requestId"] as String))
        assertEquals(FriendOutcome.RequestSent("Ann"), h.state.outcome)
        assertFalse(h.state.confirming)
        assertEquals(listOf<FriendOutcome>(FriendOutcome.RequestSent("Ann")), h.outcomes)
        assertTrue("a request waits for the other person: no card is awaited", h.events.none { it.startsWith("await") })
    }

    @Test fun aTeamMemberIsRequestedThroughTheTeamProof() = runTest {
        val h = Harness(this)
        h.answers("none")
        h.lookup.lookup(FriendProof.Team("t1", "u2"))
        advanceUntilIdle()
        assertEquals(mapOf("action" to "lookup", "teamId" to "t1", "uid" to "u2"), h.calls.first())
        h.lookup.confirm("")
        advanceUntilIdle()
        val body = h.confirmations.single()
        assertEquals("team", body["via"])
        assertEquals("t1", body["teamId"])
        assertEquals("u2", body["uid"])
        assertFalse("an empty note is not sent", body.containsKey("note"))
    }

    @Test fun aFriendLinkRedeemsWithoutANoteAndTheChatWaitsForTheFriendsCard() = runTest {
        val h = Harness(this)
        h.answers("none", JSONObject("""{"uid":"u2","state":"friends","teamId":"f_abc"}"""))
        val card = CompletableDeferred<Unit>()
        h.arrived = { card.await() }
        h.lookup.lookup(FriendProof.Code(code))
        advanceUntilIdle()
        h.lookup.confirm("ignored")
        runCurrent()
        val body = h.confirmations.single()
        assertEquals(mapOf("action" to "redeem", "code" to code), body - "requestId")
        assertTrue(FriendsLogic.isRequestId(body["requestId"] as String))
        assertEquals("the card has not arrived: still confirming, nothing announced", listOf("await:u2"), h.events)
        assertTrue(h.state.confirming)
        assertNull(h.state.outcome)
        card.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("await:u2", "announce:${FriendOutcome.NowFriends("u2", "Ann")}"), h.events)
        assertEquals(FriendOutcome.NowFriends("u2", "Ann"), h.state.outcome)
        assertFalse(h.state.confirming)
    }

    @Test fun anIncomingRequestIsAcceptedNotRequestedAgain() = runTest {
        val h = Harness(this)
        h.answers("incoming", JSONObject("""{"state":"friends","teamId":"f_abc"}"""))
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("hi")
        advanceUntilIdle()
        assertEquals(listOf(mapOf("action" to "accept", "otherUid" to "u2")), h.confirmations)
        assertEquals("an answer without a uid names the person that was looked up", FriendOutcome.NowFriends("u2", "Ann"), h.state.outcome)
        assertEquals(listOf("await:u2"), h.events.filter { it.startsWith("await") })
    }

    @Test fun aMutualRequestCountsAsAcceptedAndOpensTheChat() = runTest {
        val h = Harness(this)
        h.answers("none", JSONObject("""{"uid":"u2","state":"friends","teamId":"f_abc"}"""))
        h.lookup.lookup(FriendProof.Email("a@b.co"))
        advanceUntilIdle()
        h.lookup.confirm("")
        advanceUntilIdle()
        assertEquals(FriendOutcome.NowFriends("u2", "Ann"), h.state.outcome)
        assertEquals(listOf("await:u2"), h.events.filter { it.startsWith("await") })
    }

    @Test fun aFriendOrASentRequestHasNothingToConfirm() = runTest {
        for (relation in listOf("friends", "outgoing")) {
            val h = Harness(this)
            h.answers(relation)
            h.lookup.lookup(FriendProof.Handle("ann"))
            advanceUntilIdle()
            h.lookup.confirm("hi")
            advanceUntilIdle()
            assertTrue(relation, h.confirmations.isEmpty())
            assertNull(h.state.outcome)
        }
    }

    @Test fun nothingIsConfirmedBeforeALookupHasAnswered() = runTest {
        val h = Harness(this)
        h.lookup.confirm("hi")
        assertTrue(h.calls.isEmpty())
        val gate = CompletableDeferred<JSONObject>()
        h.answer = { gate.await() }
        h.lookup.lookup(FriendProof.Handle("ann"))
        runCurrent()
        h.lookup.confirm("hi")
        assertEquals("only the lookup was sent", 1, h.calls.size)
        gate.complete(found("none"))
        advanceUntilIdle()
    }

    @Test fun aSecondTapWhileConfirmingIsIgnored() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<JSONObject>()
        h.answer = { body -> if (body["action"] == "lookup") found("none") else gate.await() }
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("hi")
        runCurrent()
        h.lookup.confirm("hi")
        h.lookup.confirm("other")
        runCurrent()
        assertEquals(1, h.confirmations.size)
        assertTrue(h.state.confirming)
        gate.complete(JSONObject("""{"uid":"u2","state":"outgoing"}"""))
        advanceUntilIdle()
        assertEquals(1, h.confirmations.size)
    }

    // --- the request id (D18) -----------------------------------------------------------------------------------

    @Test fun theRequestIdIsReusedAfterAnUnknownOutcomeOnly() = runTest {
        for (unknown in listOf<Throwable>(TeamMessageFailure("SERVICE_UNAVAILABLE"), IOException("timeout"))) {
            val h = Harness(this)
            var failures = 1
            h.answer = { body -> if (body["action"] == "lookup") found("none") else if (failures-- > 0) throw unknown else JSONObject("""{"uid":"u2","state":"outgoing"}""") }
            h.lookup.lookup(FriendProof.Handle("ann"))
            advanceUntilIdle()
            h.lookup.confirm("hi")
            advanceUntilIdle()
            assertEquals(FriendsError.Network, h.state.error)
            assertFalse(h.state.confirming)
            assertNull(h.state.outcome)
            h.lookup.confirm("hi")
            advanceUntilIdle()
            val ids = h.confirmations.map { it["requestId"] }
            assertEquals("$unknown", 2, ids.size)
            assertEquals("the same confirmation is retried with the same id", ids[0], ids[1])
            assertNull("a retry starts without the old error", h.state.error)
            assertEquals(FriendOutcome.RequestSent("Ann"), h.state.outcome)
        }
    }

    @Test fun aDefinitiveFailureDropsTheRequestId() = runTest {
        val h = Harness(this)
        h.answer = { body -> if (body["action"] == "lookup") found("none") else throw TeamMessageFailure("STALE_REQUEST") }
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("hi")
        advanceUntilIdle()
        assertEquals(FriendsError.StaleRequest, h.state.error)
        h.lookup.confirm("hi")
        advanceUntilIdle()
        val ids = h.confirmations.map { it["requestId"] }
        assertEquals(2, ids.size)
        assertNotEquals("a definitive answer ends the confirmation", ids[0], ids[1])
    }

    @Test fun aChangedNoteIsANewConfirmationWithANewId() = runTest {
        val h = Harness(this)
        h.answer = { body -> if (body["action"] == "lookup") found("none") else throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.lookup.lookup(FriendProof.Handle("ann"))
        advanceUntilIdle()
        h.lookup.confirm("first")
        advanceUntilIdle()
        h.lookup.confirm("second")
        advanceUntilIdle()
        h.lookup.confirm("second")
        advanceUntilIdle()
        val ids = h.confirmations.map { it["requestId"] }
        assertEquals(3, ids.size)
        assertNotEquals(ids[0], ids[1])
        assertEquals(ids[1], ids[2])
    }

    @Test fun aRedeemRetriedAfterAnUnknownOutcomeKeepsItsId() = runTest {
        val h = Harness(this)
        var failures = 2
        h.answer = { body -> if (body["action"] == "lookup") found("none") else if (failures-- > 0) throw IOException("offline") else JSONObject("""{"uid":"u2","state":"friends"}""") }
        h.lookup.lookup(FriendProof.Code(code))
        advanceUntilIdle()
        repeat(3) { h.lookup.confirm(""); advanceUntilIdle() }
        val ids = h.confirmations.map { it["requestId"] }
        assertEquals(3, ids.size)
        assertEquals(1, ids.toSet().size)
        assertEquals(FriendOutcome.NowFriends("u2", "Ann"), h.state.outcome)
    }

    // --- clearing -----------------------------------------------------------------------------------------------

    @Test fun clearingDropsTheCardTheCodeAndAnyAnswerStillOnItsWay() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<JSONObject>()
        h.answer = { gate.await() }
        h.lookup.lookup(FriendProof.Code(code))
        runCurrent()
        assertTrue(h.state.looking)
        h.lookup.clear()
        assertEquals(LookupState(), h.state)
        gate.complete(found("none"))
        advanceUntilIdle()
        assertEquals("a late answer never shows up", LookupState(), h.state)
    }

    @Test fun clearingWhileConfirmingNeverPublishesTheOutcome() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<JSONObject>()
        h.answer = { body -> if (body["action"] == "lookup") found("none") else gate.await() }
        h.lookup.lookup(FriendProof.Code(code))
        advanceUntilIdle()
        h.lookup.confirm("")
        runCurrent()
        h.lookup.clear()
        gate.complete(JSONObject("""{"uid":"u2","state":"friends"}"""))
        advanceUntilIdle()
        assertEquals(LookupState(), h.state)
        assertTrue("nothing is announced for a flow that was dismissed", h.outcomes.isEmpty())
    }
}
