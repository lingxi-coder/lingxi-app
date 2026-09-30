package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.SessionRef
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Break cases the durable-recovery findings did NOT imply, written by the
 * verification pass rather than the implementer. Each one is a way the new
 * machinery could be wrong while every test the implementer wrote stays green.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class DurableRecoveryAdversarialTest {

    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    /**
     * The discard watchdog is armed against the session the checkpoint lives
     * in. If the user then follows a notification to ANOTHER session, the
     * watchdog must die with the checkpoint it was guarding — otherwise the
     * timeout fires minutes later and raises "the discard was never confirmed"
     * on a conversation that never had a discard.
     */
    @Test
    fun aWatchdogArmedBeforeRoutingAwayCannotFireOnTheDestinationSession() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = FakeSource()
        val vm = parkedViewModel(source, ids)

        vm.cancel() // submits the discard, arms the watchdog
        runCurrent()
        assertEquals(listOf(93L), source.discarded)

        val routed = vm.openSessionFromNotification(SessionRef("session-b", ""), turnId = 77L)
        runCurrent()
        assertTrue(routed)
        assertEquals(listOf("resume:session-b"), source.sessionOperations)
        val errorOnArrival = vm.state.value.error

        advanceTimeBy(ChatViewModel.DISCARD_CONFIRMATION_TIMEOUT_MS * 3)
        advanceUntilIdle()

        assertEquals(
            "the watchdog for the session we LEFT fired on the destination",
            errorOnArrival,
            vm.state.value.error,
        )
        assertFalse(
            "chat_error_discard_unconfirmed was raised after the checkpoint was released " +
                "by the session change: $ids",
            ids.contains(R.string.chat_error_discard_unconfirmed),
        )
        assertEquals("session-b", vm.state.value.session.id)
    }

    /**
     * `holdingAnnouncedTurn` must be scoped to the announced SESSION. Durable
     * turn ids are not a globally unique namespace the client can rely on, so
     * dropping the `onAnnouncedSession &&` conjunct would silently swallow a
     * notification for another conversation whose turn id happens to collide —
     * the exact "tap does nothing" failure A1 was filed for, restored.
     */
    @Test
    fun aCollidingTurnIdOnAnotherSessionStillRoutes() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = FakeSource()
        val vm = parkedViewModel(source, ids) // parked on session-a / turn 93

        val routed = vm.openSessionFromNotification(SessionRef("session-b", ""), turnId = 93L)
        runCurrent()

        assertTrue(routed)
        assertEquals(
            "turn 93 is session-a's checkpoint; the notification named session-b",
            listOf("resume:session-b"),
            source.sessionOperations,
        )
        assertEquals("session-b", vm.state.value.session.id)
    }

    /**
     * The notification route crosses the parked guard through
     * `allowInactiveWaitingRecovery`. That flag is checked BEFORE the refusal,
     * and it has to stay that way: reordering it to `refuse... && !allow` would
     * still route (the banner-raiser returns true only to say "refused"), but
     * would paint a spurious "finish the background turn first" error over the
     * conversation the user was just sent to.
     */
    @Test
    fun theNotificationRouteRaisesNoRefusalBanner() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = FakeSource()
        val vm = parkedViewModel(source, ids)

        vm.openSessionFromNotification(SessionRef("session-b", ""), turnId = 77L)
        runCurrent()

        assertNull("the notification route must not raise a refusal banner", vm.state.value.error)
        assertFalse(
            "chat_error_finish_background_turn_first was resolved on a route that succeeded: $ids",
            ids.contains(R.string.chat_error_finish_background_turn_first),
        )
    }

    /**
     * The same reconnect path that has always been allowed to cross the parked
     * guard (`switchWorkspaceSource`'s inner transition / provider reconnect)
     * must also stay silent. This pins the `!allowInactiveWaitingRecovery &&`
     * short-circuit rather than the notification caller.
     */
    @Test
    fun aRecoveryAllowedTransitionDoesNotResolveTheRefusalCopy() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = FakeSource()
        val vm = parkedViewModel(source, ids)

        // The engine reconnect path (`ensureSource`) re-establishes the visible
        // session with allowInactiveWaitingRecovery = true precisely BECAUSE a
        // parked checkpoint must not block it. The refusal must stay behind
        // that flag's short-circuit: evaluating it first still routes, but
        // paints "finish the background turn first" over a reconnect the user
        // never asked for.
        val replacement = FakeSource()
        vm.ensureSource(generation = 1) { replacement }
        runCurrent()

        assertFalse(
            "the reconnect resolved the parked-refusal copy: $ids",
            ids.contains(R.string.chat_error_finish_background_turn_first),
        )
        assertEquals(
            "the reconnect must still re-establish the visible session",
            listOf("resume-empty:session-a"),
            replacement.sessionOperations,
        )
        assertTrue(
            "the announced session's own checkpoint must survive its own resume",
            vm.state.value.durableRecoveryBlocked,
        )
    }

    /**
     * A malformed/absent tag must not resurrect the old defect in the other
     * direction: `outcome` present but JSON null, or carrying an unknown tag,
     * must decode to "not a terminal reply" — never throw, never settle.
     */
    @Test
    fun aNullOrUnknownOutcomeTagNeitherThrowsNorSettlesTheTurn() {
        assertNull(retainedTurnEventToReply("""{"type":"turn_ended","outcome":null}"""))
        assertNull(retainedTurnEventToReply("""{"type":"turn_ended","outcome":{}}"""))
        assertNull(
            retainedTurnEventToReply("""{"type":"turn_ended","outcome":{"type":"future_outcome"}}"""),
        )
        assertNull(retainedTurnEventToReply("""{"type":"turn_ended","outcome":123}"""))
        // The tag is read from `outcome`, not from the sibling `stop_reason`
        // the blessed snapshot also carries.
        assertNull(
            retainedTurnEventToReply(
                """{"type":"turn_ended","stop_reason":"end_turn","outcome":{"type":"cancelled"}}""",
            ),
        )
        assertEquals(
            ReplyEvent.End,
            retainedTurnEventToReply(
                """{"type":"turn_ended","stop_reason":"end_turn","outcome":{"type":"end_turn"}}""",
            ),
        )
    }

    // --- harness ----------------------------------------------------------

    private fun recordingStrings(ids: MutableList<Int>) = ConversationStrings { id, fallback, args ->
        ids += id
        if (args.isEmpty()) fallback else String.format(java.util.Locale.ROOT, fallback, *args)
    }

    private fun parkedViewModel(source: FakeSource, ids: MutableList<Int>): ChatViewModel {
        val vm = ChatViewModel(source, strings = recordingStrings(ids))
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.WAITING_FOR_USER)),
        )
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.WAITING_FOR_USER)),
        )
        ids.clear()
        return vm
    }

    private fun snapshot(state: TurnRecoveryStateDto) = TurnRecoverySnapshotDto(
        sessionId = "session-a",
        turnId = 93u,
        state = state,
        firstSequence = 0u,
        lastSequence = 1u,
        safeToResume = false,
        reason = null,
    )

    private class FakeSource : ConversationSource {
        val active = MutableStateFlow<ActivatedSession?>(null)
        val submitted = mutableListOf<String>()
        val discarded = mutableListOf<Long>()
        val sessionOperations = mutableListOf<String>()

        override val activeSessionState = active.asStateFlow()

        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return emptyFlow()
        }

        override suspend fun discardDurableTurn(turnId: Long) {
            discarded += turnId
        }

        override suspend fun resumeSession(uuid: String) {
            sessionOperations += "resume:$uuid"
        }

        override suspend fun resumeEmptySession(uuid: String, title: String) {
            sessionOperations += "resume-empty:$uuid"
        }

        override suspend fun newSession() {
            sessionOperations += "new"
        }
    }
}
