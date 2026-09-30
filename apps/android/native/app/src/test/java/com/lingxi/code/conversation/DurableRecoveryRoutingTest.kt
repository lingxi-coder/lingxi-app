package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
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
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * The three ways a PARKED durable checkpoint used to trap the user, each of
 * which is invisible to a test that only asserts the fallback copy:
 *
 *  * the background notification's tap routed through `openSession`, which
 *    REFUSES while a checkpoint is parked — the state every one of those
 *    notifications announces;
 *  * `discardRecoveredTurn` latched on a terminal `TurnRecoveryState` the host
 *    is not obliged to emit (`cancel_active_turn`'s inactive branch maps
 *    NotFound/Terminal to `snapshot = None` and returns `Ok(())`);
 *  * every other entry point refused SILENTLY.
 *
 * Copy is asserted by RESOURCE ID, not by the fallback text: the fallback is
 * what `DefaultConversationStrings` returns and is therefore the one thing a
 * test sees even when the id beside it points at completely different copy.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class DurableRecoveryRoutingTest {

    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    @Test
    fun discardStatusAndFailureCopyNameTheDiscardResourcesNotTheStopOnes() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource()
        val vm = parkedViewModel(source, ids)

        vm.cancel() // parked checkpoint -> discardRecoveredTurn
        runCurrent()

        assertEquals(listOf(93L), source.discarded)
        assertTrue(
            "the discard status line must resolve chat_discarding, not chat_stopping " +
                "(\"正在停止…\"/\"Stopping…\") — the device renders the RESOURCE: $ids",
            ids.contains(R.string.chat_discarding),
        )
        assertFalse("chat_stopping must no longer back a discard", ids.contains(R.string.chat_stopping))
    }

    @Test
    fun discardFailureCopyNamesTheDiscardResourceNotCancelGeneration() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource(discardError = IllegalStateException("boom"))
        val vm = parkedViewModel(source, ids)

        vm.cancel()
        runCurrent()

        assertTrue(
            "a failed discard must resolve chat_error_discard_background_failed, not " +
                "chat_error_cancel_generation_failed (\"取消生成失败\"): $ids",
            ids.contains(R.string.chat_error_discard_background_failed),
        )
        assertFalse(ids.contains(R.string.chat_error_cancel_generation_failed))
    }

    @Test
    fun anAcceptedDiscardThatIsNeverConfirmedStopsBlockingTheComposer() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource()
        val vm = parkedViewModel(source, ids)
        assertTrue(vm.state.value.durableRecoveryBlocked)

        vm.cancel()
        runCurrent()
        assertEquals(listOf(93L), source.discarded)

        // The host accepted and emitted NOTHING. Before the watchdog this
        // latched forever: a second tap returned at the in-flight guard.
        vm.cancel()
        runCurrent()
        assertEquals("the in-flight latch must still suppress a duplicate", listOf(93L), source.discarded)
        assertTrue(vm.state.value.durableRecoveryBlocked)

        advanceTimeBy(ChatViewModel.DISCARD_CONFIRMATION_TIMEOUT_MS + 1)
        advanceUntilIdle()

        assertFalse(
            "the composer must not stay blocked on a confirmation the host never sends",
            vm.state.value.durableRecoveryBlocked,
        )
        assertNotNull(vm.state.value.error)
        assertTrue(
            "the release must be explained with chat_error_discard_unconfirmed: $ids",
            ids.contains(R.string.chat_error_discard_unconfirmed),
        )
        vm.send("now unblocked")
        runCurrent()
        assertEquals(listOf("now unblocked"), source.submitted)
    }

    @Test
    fun aTerminalConfirmationCancelsTheWatchdogSoItCannotFireLate() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource()
        val vm = parkedViewModel(source, ids)

        vm.cancel()
        runCurrent()
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.CANCELLED)))
        assertFalse(vm.state.value.durableRecoveryBlocked)
        val errorAfterTerminal = vm.state.value.error

        advanceTimeBy(ChatViewModel.DISCARD_CONFIRMATION_TIMEOUT_MS * 2)
        advanceUntilIdle()

        assertEquals(
            "a confirmed discard must not later raise the unconfirmed banner",
            errorAfterTerminal,
            vm.state.value.error,
        )
        assertFalse(ids.contains(R.string.chat_error_discard_unconfirmed))
    }

    @Test
    fun everyParkedRefusalRaisesAVisibleBannerInsteadOfReturningSilently() = runTest(dispatcher) {
        for (action in listOf<Pair<String, (ChatViewModel) -> Unit>>(
            "send" to { vm -> vm.send("hello") },
            "openSession" to { vm -> vm.openSession(SessionRef("session-b", "B")) },
            "newChat" to { vm -> vm.newChat() },
        )) {
            val ids = mutableListOf<Int>()
            val source = ParkedTurnSource()
            val vm = parkedViewModel(source, ids)
            assertEquals(null, vm.state.value.error)

            action.second(vm)
            runCurrent()

            assertNotNull(
                "${action.first} refused a parked checkpoint SILENTLY — no banner",
                vm.state.value.error,
            )
            assertTrue(
                "${action.first} must name chat_error_finish_background_turn_first: $ids",
                ids.contains(R.string.chat_error_finish_background_turn_first),
            )
            assertTrue("${action.first} must not reach the engine", source.submitted.isEmpty())
            assertTrue(source.sessionOperations.isEmpty())
        }
    }

    @Test
    fun switchWorkspaceSourceRefusalIsAlsoVisible() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource()
        val vm = parkedViewModel(source, ids)

        val switched = vm.switchWorkspaceSource(
            projectId = "project-b",
            createSource = { ParkedTurnSource() },
        )

        assertFalse(switched)
        assertNotNull(
            "the created-app landing loop retries on false and then sends its kickoff into " +
                "whatever conversation is open; the refusal has to be visible",
            vm.state.value.error,
        )
        assertTrue(ids.contains(R.string.chat_error_finish_background_turn_first))
    }

    @Test
    fun theNotificationRouteLandsOnTheAnnouncedSessionDespiteTheParkedCheckpoint() =
        runTest(dispatcher) {
            val ids = mutableListOf<Int>()
            val source = ParkedTurnSource()
            val vm = parkedViewModel(source, ids)
            assertTrue(vm.state.value.durableRecoveryBlocked)

            // The old route was `openSession`, which refuses this exact state.
            vm.openSession(SessionRef("session-b", ""))
            runCurrent()
            assertTrue(
                "precondition: openSession still refuses a parked checkpoint",
                source.sessionOperations.isEmpty(),
            )

            val routed = vm.openSessionFromNotification(SessionRef("session-b", ""), turnId = 77L)
            runCurrent()

            assertTrue(routed)
            assertEquals(listOf("resume:session-b"), source.sessionOperations)
            assertEquals("session-b", vm.state.value.session.id)
            assertFalse(
                "the checkpoint belonged to the session we just left; carrying its block " +
                    "across would hand the destination a permanently locked composer",
                vm.state.value.durableRecoveryBlocked,
            )
        }

    @Test
    fun theNotificationRouteIsANoOpWhenAlreadyOnTheAnnouncedTurn() = runTest(dispatcher) {
        val ids = mutableListOf<Int>()
        val source = ParkedTurnSource()
        val vm = parkedViewModel(source, ids)

        val routed = vm.openSessionFromNotification(SessionRef("session-a", ""), turnId = 93L)
        runCurrent()

        assertTrue(routed)
        assertTrue(
            "re-resuming the session already on screen would tear down the very transcript " +
                "the notification asked the user to look at",
            source.sessionOperations.isEmpty(),
        )
        assertTrue(
            "the announced checkpoint keeps its Discard affordance",
            vm.state.value.durableRecoveryBlocked,
        )
    }

    /**
     * REFUTATION PIN for "AttachTurn's terminal snapshot discards the replay".
     *
     * `host.rs` emits `TurnRecoveryState` FIRST and the `TurnEventReplay`
     * suffix after it, so a terminal snapshot arriving before the transcript
     * looks like it should null `recoveredTurnToken`/`recoveryReplayTurnId` and
     * drop everything that follows. It does not: the
     * `if (startsRecoveredAttach && terminal) { ... return }` guard sits AHEAD
     * of the `when (snapshot.state)`, so the COMPLETED arm is not reached on
     * the attach snapshot at all — settling is deferred to ResumeTurn's second
     * terminal state, which arrives after the replay. This test fails the day
     * that guard is removed.
     */
    @Test
    fun aTerminalAttachSnapshotStillRendersTheReplayedTranscript() = runTest(dispatcher) {
        val source = ParkedTurnSource()
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )

        // AttachTurn: terminal state first ...
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.COMPLETED)),
        )
        // ... then the retained suffix it announced.
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 93u,
                sequence = 1u,
                eventJson = """{"type":"text_delta","text":"real output"}""",
            ),
        )
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 93u,
                sequence = 2u,
                eventJson = """{"type":"turn_ended","outcome":{"type":"end_turn"}}""",
            ),
        )
        runCurrent()

        assertTrue(
            "the replayed transcript must survive a terminal attach snapshot; " +
                "messages=${vm.state.value.messages.map { it.text }} " +
                "streaming=${vm.state.value.streamingMessage?.text}",
            vm.state.value.messages.any { it.text.contains("real output") } ||
                vm.state.value.streamingMessage?.text?.contains("real output") == true,
        )
    }

    // --- harness ----------------------------------------------------------

    /** Records every resource id the ViewModel resolves, then returns the fallback. */
    private fun recordingStrings(ids: MutableList<Int>) = ConversationStrings { id, fallback, args ->
        ids += id
        if (args.isEmpty()) {
            fallback
        } else {
            String.format(java.util.Locale.ROOT, fallback, *args)
        }
    }

    /** A ViewModel holding one inactive `WaitingForUser` checkpoint on `session-a`/turn 93. */
    private fun parkedViewModel(source: ParkedTurnSource, ids: MutableList<Int>): ChatViewModel {
        val vm = ChatViewModel(source, strings = recordingStrings(ids))
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        // Attach's snapshot arms the recovered token; ResumeTurn's repeat is what
        // marks the checkpoint inactive-and-parked.
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

    private class ParkedTurnSource(
        private val discardError: Throwable? = null,
    ) : ConversationSource {
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
            discardError?.let { throw it }
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
