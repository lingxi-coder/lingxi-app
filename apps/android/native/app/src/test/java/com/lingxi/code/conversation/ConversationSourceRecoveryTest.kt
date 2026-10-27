package com.lingxi.code.conversation

import android.content.SharedPreferences
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.bindings.client.QueuedTurnStatusDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.test.runCurrent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.Collections

class ConversationSourceRecoveryTest {

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test
    fun pendingRecoveryForwardsPromptModeAndCompleteSubmission() = kotlinx.coroutines.test.runTest {
        val pending = CompletableDeferred<ConversationSource?>()
        val source = RecoveringConversationSource(
            pending,
            DefaultConversationStrings,
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )
        val delegate = RecordingRecoveredSource()
        val prompt = RecoveredPrompt(
            text = "echo \uD800",
            images = listOf(ImageRefDto("image/png", "AA==")),
            turnId = 42L,
            visualization = VisualizationRef("chart", 3u),
            mode = PromptModeDto.BASH,
        )
        try {
            val replies = async {
                source.submit(prompt.text, prompt.images, prompt.turnId, prompt.visualization, prompt.mode).toList()
            }
            runCurrent()
            assertFalse(replies.isCompleted)
            assertTrue(delegate.prompts.isEmpty())

            pending.complete(delegate)

            assertEquals(listOf(ReplyEvent.End), replies.await())
            assertEquals(listOf(prompt), delegate.prompts)
            source.submit("follow up", emptyList(), 43L, prompt.visualization).toList()
            assertEquals(RecoveredPrompt("follow up", emptyList(), 43L, prompt.visualization, null), delegate.prompts.last())
        } finally {
            source.close()
        }
    }

    @Test
    fun recoveredSourcePreservesExactSlashIdentityAndResult() = kotlinx.coroutines.test.runTest {
        val delegate = RecordingRecoveredSource()
        val source = RecoveringConversationSource(
            CompletableDeferred<ConversationSource?>(delegate),
            DefaultConversationStrings,
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )
        val raw = "/skill\uD800 argument".toClientTextDto()
        val images = listOf(ImageRefDto(mediaType = "image/png", base64 = "cG5nLWltYWdl"))
        try {
            val commandSource: SlashCommandConversationSource = source
            assertEquals(
                listOf(ReplyEvent.CommandOutput("done", false, 44uL)),
                commandSource.submitSlashCommand(raw, images, 44L).toList(),
            )
            assertEquals(listOf(Triple(raw, images, 44L)), delegate.slashCommands)
            assertTrue(delegate.prompts.isEmpty())
        } finally {
            source.close()
        }
    }

    @Test
    fun failedRecoveryTerminatesPromptAndSlashStreams() = kotlinx.coroutines.test.runTest {
        val source = RecoveringConversationSource(
            CompletableDeferred<ConversationSource?>().apply { complete(null) },
            DefaultConversationStrings,
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )
        try {
            val prompt = source.submit("echo hi", emptyList(), 45L, null, PromptModeDto.BASH).toList()
            val slash = source.submitSlashCommand("/help".toClientTextDto(), emptyList(), 46L).toList()
            for (replies in listOf(prompt, slash)) {
                assertEquals(2, replies.size)
                assertTrue(replies.first() is ReplyEvent.Error)
                assertEquals(ReplyEvent.End, replies.last())
            }
        } finally {
            source.close()
        }
    }

    @Test
    fun failedRecoveryRejectsModelSelection() = kotlinx.coroutines.test.runTest {
        val source = RecoveringConversationSource(
            kotlinx.coroutines.CompletableDeferred<EngineConversationSource?>().apply { complete(null) },
            DefaultConversationStrings,
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )
        try {
            source.setModel("openai/gpt-5.6-sol")
            fail("expected unavailable engine to reject model selection")
        } catch (expected: IllegalStateException) {
            assertEquals("engine is not connected", expected.message)
        } finally {
            source.close()
        }
    }

    @Test
    fun recoveryScopeKeyUsesStableWorkspaceIdentityAndMode() {
        val chat = ConversationRecoverySpec(
            projectId = "weather",
            hostPath = "/same/path",
            sessionMode = SessionMode.Chat,
            linuxRuntimeMode = LinuxRuntimeMode.MobileLinux,
            workspaceKey = "app.weather",
        )
        val code = chat.copy(sessionMode = SessionMode.Code)

        assertEquals("app.weather#chat", chat.scopeKey)
        assertEquals("app.weather#code", code.scopeKey)
    }

    @Test
    fun durableResumePolicyDistinguishesFreshUiRetainedTakeoverAndHeadless() {
        // A newly built UI source has no existing executor and must Resume.
        assertTrue(shouldResumeUiAttach(hadHeadlessExecutor = false, wasAlreadyUiOwned = false))
        assertTrue(shouldResumeDurableTurn(forceAttach = true, resumeOnUiAttach = true))
        // A retained source already has the process/headless executor; UI only
        // requests Attach from its current cursor and must not Resume again.
        assertFalse(shouldResumeUiAttach(hadHeadlessExecutor = true, wasAlreadyUiOwned = false))
        assertFalse(shouldResumeDurableTurn(forceAttach = true, resumeOnUiAttach = false))
        // A second Activity claim must not turn an existing UI owner into a
        // second Resume caller either.
        assertFalse(shouldResumeUiAttach(hadHeadlessExecutor = false, wasAlreadyUiOwned = true))
        // Cold headless recovery always has to Resume after Attach.
        assertTrue(shouldResumeDurableTurn(forceAttach = false, resumeOnUiAttach = false))
    }

    @Test
    fun pendingUiClaimKeepsColdResumeUntilHeadlessSourceRegisters() {
        val owner = ConversationHeadlessRecovery.RecoveryOwner(
            ConversationRecoverySpec(
                projectId = null,
                hostPath = null,
                sessionMode = SessionMode.Code,
                linuxRuntimeMode = LinuxRuntimeMode.MobileLinux,
            ),
        )

        owner.claimForUi()

        // No service executor has succeeded yet. A source registered after
        // this pending claim must therefore retain the UI's first Resume.
        assertFalse(owner.headlessExecutorActive)
        assertTrue(owner.uiAttachResumeRequired)
    }

    @Test
    fun secondUiClaimCannotOverwriteFirstInFlightResumeDisposition() {
        val owner = ConversationHeadlessRecovery.RecoveryOwner(
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )

        owner.claimForUi()
        // Models the first Activity's Attach suspended at the engine barrier.
        owner.claimForUi()
        assertTrue(owner.uiAttachResumeRequired)

        // Once the first Resume succeeds, later Activity claims are allowed
        // to observe the consumed requirement rather than replaying it.
        owner.uiAttachResumeRequired = false
        owner.claimForUi()
        assertFalse(owner.uiAttachResumeRequired)
    }

    @Test
    fun retainedLiveAndColdWaitingUseDifferentUiResumeDisposition() {
        val retainedOwner = ConversationHeadlessRecovery.RecoveryOwner(
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        ).apply {
            // monitorExisting hands off a live parked question from the UI.
            headlessExecutorActive = true
        }
        retainedOwner.claimForUi()
        assertFalse(retainedOwner.uiAttachResumeRequired)

        val coldOwner = ConversationHeadlessRecovery.RecoveryOwner(
            ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
        )
        coldOwner.claimForUi()
        assertTrue(coldOwner.uiAttachResumeRequired)
    }

    @Test
    fun failedCancelPreservesDurableTurnIdentity() {
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        store.begin(sessionId = "session-a", turnId = 42L)

        try {
            submitTurnCancellation(PermissionIngress(MutableStateFlow(null))) {
                throw IllegalStateException("transport down")
            }
            fail("expected cancellation submission failure")
        } catch (expected: IllegalStateException) {
            assertEquals("transport down", expected.message)
        }

        assertEquals(42L, store.load()?.turnId)
    }

    @Test
    fun durableCheckpointsAreIndependentAcrossConcurrentScopesAndLegacyMigration() {
        val preferences = FakeSharedPreferences()
        preferences.edit().putString("scope", "project-a#code").putString("session_id", "legacy-a")
            .putLong("turn_id", 11L).commit()
        val other = DurableConversationTurnClientStore(preferences, "project-b#code")
        other.begin("session-b", 22L)
        assertEquals("project-a#code", preferences.getString("scope", null))
        val first = DurableConversationTurnClientStore(preferences, "project-a#code")
        assertEquals("legacy-a", first.load()?.sessionId)
        assertEquals("session-b", other.load()?.sessionId)

        val start = CountDownLatch(1)
        val threadA = Thread { start.await(); first.begin("new-a", 31L) }
        val threadB = Thread { start.await(); other.begin("new-b", 32L) }
        threadA.start()
        threadB.start()
        start.countDown()
        threadA.join(5_000)
        threadB.join(5_000)
        val reopenedA = DurableConversationTurnClientStore(preferences, "project-a#code")
        val reopenedB = DurableConversationTurnClientStore(preferences, "project-b#code")
        assertEquals("new-a", reopenedA.load()?.sessionId)
        assertEquals("new-b", reopenedB.load()?.sessionId)
        reopenedA.clear("new-a", 31L)
        assertEquals(null, reopenedA.load())
        assertEquals("new-b", reopenedB.load()?.sessionId)
    }

    @Test
    fun acceptedWrongTurnCancelPreservesActiveDurableTurnIdentity() {
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        store.begin(sessionId = "session-a", turnId = 42L)

        submitTurnCancellation(PermissionIngress(MutableStateFlow(null))) {
            // Simulates a stale notification route targeting some other turn.
        }

        assertEquals(42L, store.load()?.turnId)
    }

    @Test
    fun cancellationSubmissionAndCorrelatedTerminalClearPreserveIdentityUntilMatchingEvent() {
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        store.begin(sessionId = "session-a", turnId = 42L)

        val submitted = mutableListOf<ClientCommand>()
        submitTurnCancellation(PermissionIngress(MutableStateFlow(null))) {
            submitted += ClientCommand.Cancel(sessionId = "session-a", turnId = 42u)
        }

        assertEquals(listOf(ClientCommand.Cancel(sessionId = "session-a", turnId = 42u)), submitted)
        // A stale terminal from another session or turn must not erase the
        // checkpoint that the notification Stop just submitted against.
        store.clear(sessionId = "session-b", turnId = 42L)
        store.clear(sessionId = "session-a", turnId = 41L)
        assertEquals(42L, store.load()?.turnId)

        // Only the correlated terminal event clears the durable identity.
        store.clear(sessionId = "session-a", turnId = 42L)
        assertEquals(null, store.load())
    }

    @Test
    fun displayOnlySlashResultClearsOnlyItsMatchingDurableCheckpoint() {
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        store.begin(sessionId = "session-a", turnId = 42L)

        clearDurableSlashCommandCheckpoint(
            store,
            ClientEvent.SlashCommandResult(
                turnId = 41uL,
                display = "stale".toClientTextDto(),
                isError = true,
            ),
        )
        assertEquals(42L, store.load()?.turnId)
        clearDurableSlashCommandCheckpoint(
            store,
            ClientEvent.SlashCommandResult(
                turnId = null,
                display = "uncorrelated".toClientTextDto(),
                isError = true,
            ),
        )
        assertEquals(42L, store.load()?.turnId)

        clearDurableSlashCommandCheckpoint(
            store,
            ClientEvent.SlashCommandResult(
                turnId = 42uL,
                display = "handled".toClientTextDto(),
                isError = false,
            ),
        )
        assertEquals(null, store.load())
    }

    @Test
    fun attachOwnershipCoalescesSameCursorAndAllowsUiTakeover() {
        val coordinator = DurableAttachCoordinator()
        val request = DurableAttachRequest("session-a", 42L, 0L)

        assertTrue(coordinator.reserve(request, DurableAttachOwner.Headless))
        assertFalse(coordinator.reserve(request, DurableAttachOwner.Headless))

        coordinator.claimForUi()
        // A UI takeover is a distinct owner: it must attach its own collector
        // even when the service previously attached at cursor zero.
        assertTrue(coordinator.reserve(request, DurableAttachOwner.Ui))
        assertFalse(coordinator.reserve(request, DurableAttachOwner.Ui))
        assertFalse(coordinator.reserve(request, DurableAttachOwner.Headless))

        // A later UI cursor is a distinct replay request, while a released UI
        // owner lets service redelivery reclaim the same source.
        assertTrue(coordinator.reserve(request.copy(afterSequence = 3L), DurableAttachOwner.Ui))
        coordinator.releaseToHeadless()
        assertTrue(coordinator.reserve(request.copy(afterSequence = 3L), DurableAttachOwner.Headless))
    }

    @Test
    fun coldResumedTerminalAttachSuppressesDuplicateAssistantAndToolReplay() {
        val gate = DurableTurnReplayGate()
        gate.noteAttachRequested(
            sessionId = "session-a",
            turnId = 88L,
            activation = ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed),
            afterSequence = 0L,
        )

        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.COMPLETED, 88u, 3u))))
        assertFalse(
            gate.shouldForward(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 1u,
                    eventJson = """{"type":"text_delta","text":"duplicate assistant"}""",
                ),
            ),
        )
        assertFalse(
            gate.shouldForward(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 2u,
                    eventJson = """{"type":"tool_use_result","id":"tool-1","tool":"shell","result_json":"{}","is_error":false}""",
                ),
            ),
        )
        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.COMPLETED, 88u, 3u))))
        assertTrue(
            gate.shouldForward(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 4u,
                    eventJson = """{"type":"text_delta","text":"future"}""",
                ),
            ),
        )
    }

    @Test
    fun coldResumedRunningAttachStillForwardsRetainedReplay() {
        val gate = DurableTurnReplayGate()
        gate.noteAttachRequested(
            sessionId = "session-a",
            turnId = 88L,
            activation = ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed),
            afterSequence = 0L,
        )

        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.RUNNING, 88u, 2u))))
        assertTrue(
            gate.shouldForward(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 1u,
                    eventJson = """{"type":"text_delta","text":"partial"}""",
                ),
            ),
        )
    }

    @Test
    fun sameVmCursorAttachDoesNotSuppressTerminalReplay() {
        val gate = DurableTurnReplayGate()
        gate.noteAttachRequested(
            sessionId = "session-a",
            turnId = 88L,
            activation = ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed),
            afterSequence = 2L,
        )

        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.COMPLETED, 88u, 3u))))
        assertTrue(
            gate.shouldForward(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 3u,
                    eventJson = """{"type":"text_delta","text":"live-tail"}""",
                ),
            ),
        )
    }

    @Test
    fun anotherSessionsTerminalCannotEndReplaySuppressionForSameTurnId() {
        val gate = DurableTurnReplayGate()
        gate.noteAttachRequested(
            sessionId = "session-a",
            turnId = 88L,
            activation = ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed),
            afterSequence = 0L,
        )
        val terminal = snapshot(TurnRecoveryStateDto.COMPLETED, 88u, 3u)
        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(terminal)))
        assertTrue(gate.shouldForward(ClientEvent.TurnRecoveryState(terminal.copy(sessionId = "session-b"))))
        assertFalse(gate.shouldForward(ClientEvent.TurnEventReplay(
            sessionId = "session-a", turnId = 88u, sequence = 1u,
            eventJson = """{"type":"text_delta","text":"already restored"}""",
        )))
    }

    @Test
    fun recoveryIngressKeepsTerminalAcrossSourcesAndSeparatesIdentities() {
        val ingress = DurableRecoveryIngress()
        val oldSource = ingress.newSource()
        val replacement = ingress.newSource()
        oldSource.acquireInterest("session-a", 88uL)
        replacement.acquireInterest("session-a", 88uL)
        replacement.acquireInterest("session-b", 88uL)
        replacement.acquireInterest("session-a", 89uL)
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        store.begin("session-a", 88L)
        val terminal = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 3u, revision = 2uL)
        val effects = mutableListOf<String>()

        assertTrue(oldSource.dispatch(ClientEvent.TurnRecoveryState(terminal)) {
            effects += "terminal"
            store.clear(terminal.sessionId, terminal.turnId.toLong())
        })
        assertEquals(null, store.load())
        store.begin("session-b", 88L)
        val paused = snapshot(TurnRecoveryStateDto.PAUSED_RECOVERABLE, 88u, 1u, revision = 1uL)
        assertFalse(replacement.dispatch(ClientEvent.TurnRecoveryState(paused)) { effects += "stale" })
        assertFalse(replacement.dispatch(ClientEvent.TurnRecoveryState(paused.copy(revision = 2uL))) {
            effects += "conflicting state"
        })
        assertEquals("session-b", store.load()?.sessionId)

        // Equal state versions are still delivered for Attach/Resume and can
        // carry a different reason or retained window.
        assertTrue(replacement.dispatch(ClientEvent.TurnRecoveryState(
            terminal.copy(reason = "replay_history_truncated", firstSequence = 2u, lastSequence = 9u),
        )) { effects += "repeat" })
        assertTrue(replacement.dispatch(ClientEvent.TurnRecoveryState(
            paused.copy(sessionId = "session-b"),
        )) { effects += "other session" })
        assertTrue(replacement.dispatch(ClientEvent.TurnRecoveryState(
            paused.copy(turnId = 89u),
        )) { effects += "other turn" })
        assertEquals(listOf("terminal", "repeat", "other session", "other turn"), effects)

        oldSource.retire()
        assertFalse(oldSource.dispatch(ClientEvent.TurnRecoveryState(terminal)) { effects += "retired" })
    }

    @Test
    fun ingressSerializesCheckSideEffectsAndRetirement() {
        val ingress = DurableRecoveryIngress()
        val oldSource = ingress.newSource()
        val replacement = ingress.newSource()
        oldSource.acquireInterest("session-a", 88uL)
        replacement.acquireInterest("session-a", 88uL)
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val nextStarted = CountDownLatch(1)
        val nextApplied = CountDownLatch(1)
        val effects = Collections.synchronizedList(mutableListOf<String>())
        val paused = snapshot(TurnRecoveryStateDto.PAUSED_RECOVERABLE, 88u, 1u, revision = 1uL)
        val terminal = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 1u, revision = 2uL)
        val first = Thread {
            oldSource.dispatch(ClientEvent.TurnRecoveryState(paused)) {
                entered.countDown()
                check(release.await(5, TimeUnit.SECONDS))
                effects += "old relay"
            }
        }
        val second = Thread {
            nextStarted.countDown()
            replacement.dispatch(ClientEvent.TurnRecoveryState(terminal)) {
                effects += "new relay"
                nextApplied.countDown()
            }
        }
        first.start()
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        second.start()
        assertTrue(nextStarted.await(5, TimeUnit.SECONDS))
        assertFalse("new side effects must wait for the old callback", nextApplied.await(100, TimeUnit.MILLISECONDS))
        release.countDown()
        first.join(5_000)
        second.join(5_000)
        assertEquals(listOf("old relay", "new relay"), effects)
        assertFalse(replacement.dispatch(ClientEvent.TurnRecoveryState(paused)) { effects += "stale" })

        // A close cannot retire an ingress while an accepted callback is still
        // between its revision check and relay side effect.
        val active = ingress.newSource()
        active.acquireInterest("session-a", 88uL)
        val applying = CountDownLatch(1)
        val finishApply = CountDownLatch(1)
        val retireStarted = CountDownLatch(1)
        val retired = CountDownLatch(1)
        val callback = Thread {
            active.dispatch(ClientEvent.TurnRecoveryState(terminal)) {
                applying.countDown()
                check(finishApply.await(5, TimeUnit.SECONDS))
                effects += "final relay"
            }
        }
        val closer = Thread {
            retireStarted.countDown()
            active.retire()
            retired.countDown()
        }
        callback.start()
        assertTrue(applying.await(5, TimeUnit.SECONDS))
        closer.start()
        assertTrue(retireStarted.await(5, TimeUnit.SECONDS))
        assertFalse(retired.await(100, TimeUnit.MILLISECONDS))
        finishApply.countDown()
        callback.join(5_000)
        closer.join(5_000)
        assertTrue(retired.await(5, TimeUnit.SECONDS))
        assertFalse(active.dispatch(ClientEvent.TurnRecoveryState(terminal)) { effects += "after close" })
        assertEquals(listOf("old relay", "new relay", "final relay"), effects)
    }

    @Test
    fun recoveryRevisionUsesUnsignedComparisonAndCanonicalSessionIdentity() {
        val gate = DurableRecoveryIngress().newSource()
        val session = "12345678-1234-1234-1234-123456789abc"
        gate.acquireInterest(session, 8uL)
        val high = snapshot(TurnRecoveryStateDto.CANCELLED, 8u, 0u,
            revision = Long.MAX_VALUE.toULong() + 1uL).copy(sessionId = session)
        assertTrue(gate.dispatch(ClientEvent.TurnRecoveryState(high)) {})
        assertFalse(gate.dispatch(ClientEvent.TurnRecoveryState(high.copy(sessionId = "sess:${session.uppercase()}",
            revision = Long.MAX_VALUE.toULong(), state = TurnRecoveryStateDto.PAUSED_RECOVERABLE))) {})
    }

    @Test
    fun turnIdsSkipOutstandingPersistedRecordsAndNeverWrap() {
        val allocator = DurableTurnIdAllocator(Long.MAX_VALUE - 3, reserved = { setOf(Long.MAX_VALUE - 2) })
        assertEquals(Long.MAX_VALUE - 1, allocator.next())
        assertEquals(Long.MAX_VALUE, allocator.next())
        assertNull(allocator.next())
    }

    @Test
    fun turnIdIsNotIssuedWhenItsReservationCannotBePersisted() {
        var succeeds = false
        val allocator = DurableTurnIdAllocator(10L, reserved = { emptySet() }, persist = { succeeds })
        assertNull(allocator.next())
        succeeds = true
        assertEquals(11L, allocator.next())
    }

    @Test
    fun pendingAdmissionAndSeveralQueuedOwnersSurviveReopenWithoutCrossingScopes() {
        val preferences = FakeSharedPreferences()
        val first = DurableConversationTurnClientStore(preferences, "scope-a")
        val other = DurableConversationTurnClientStore(preferences, "scope-b")
        assertTrue(first.begin("session-a", 10L, admissionPending = true))
        assertTrue(first.enqueue("session-a", 11L))
        assertTrue(first.enqueue("session-a", 12L))
        assertTrue(first.enqueue("another-session", 11L))
        assertTrue(other.begin("session-b", 20L, admissionPending = true))
        assertTrue(other.enqueue("session-b", 21L))

        val reopened = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(reopened.load()!!.admissionPending)
        assertEquals(listOf(11L, 12L), reopened.queued("session-a").map { it.turnId })
        assertEquals(listOf(11L), reopened.queued("another-session").map { it.turnId })
        assertEquals(listOf(21L), other.queued("session-b").map { it.turnId })
        assertFalse(shouldResumeDurableTurn(forceAttach = true, resumeOnUiAttach = true, admissionPending = true))
        assertFalse(shouldResumeDurableTurn(forceAttach = false, resumeOnUiAttach = true, admissionPending = true))
        reopened.confirmAdmission("session-b", 10L) // the wrong owner cannot clear the marker
        assertTrue(reopened.load()!!.admissionPending)
        reopened.confirmAdmission("session-a", 10L)
        assertFalse(reopened.load()!!.admissionPending)
        reopened.clear("session-a", 10L)
        assertTrue(reopened.promoteQueued("session-a", 11L))
        assertEquals(11L, reopened.load()!!.turnId)
        assertEquals(listOf(12L), reopened.queued("session-a").map { it.turnId })
        assertEquals(20L, other.load()!!.turnId)
        assertEquals(listOf(21L), other.queued("session-b").map { it.turnId })
    }

    @Test
    fun terminalEvidenceSurvivesCrashBeforeAndAfterUiConsumptionWhileNextTurnCanStart() {
        val preferences = FakeSharedPreferences()
        val source = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(source.begin("session-a", 10L))
        assertTrue(source.enqueue("session-a", 11L))
        assertTrue(source.recordTerminalEvidence("session-a", 10L)) // before relay or UI
        val beforeUi = DurableConversationTurnClientStore(preferences, "scope-a")
        assertEquals(10L, beforeUi.load()?.turnId)
        assertEquals(listOf(10L), beforeUi.terminalReceipts("session-a").map { it.turnId })
        assertTrue(beforeUi.finishTerminal("session-a", 10L))
        assertTrue(beforeUi.promoteQueued("session-a", 11L))
        val afterUi = DurableConversationTurnClientStore(preferences, "scope-a")
        assertEquals(11L, afterUi.load()?.turnId)
        assertEquals(listOf(10L), afterUi.terminalReceipts("session-a").map { it.turnId })
        assertTrue(afterUi.acknowledgeTerminals("session-b", listOf(10L)))
        assertEquals(listOf(10L), afterUi.terminalReceipts("session-a").map { it.turnId })
        assertTrue(afterUi.acknowledgeTerminals("session-a", listOf(10L)))
        assertTrue(afterUi.terminalReceipts("session-a").isEmpty())
    }

    @Test
    fun cancelledQueuedReceiptSurvivesReopenUntilExplicitReview() {
        val preferences = FakeSharedPreferences()
        val store = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(store.enqueue("session-a", 11L))
        assertTrue(store.enqueue("session-b", 11L))
        assertTrue(store.finishQueued("session-a", 11L, cancelledBeforeStart = true))
        val reopened = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(reopened.queued("session-a").isEmpty())
        assertEquals(listOf(11L), reopened.queued("session-b").map { it.turnId })
        assertEquals(listOf(DurableTerminalReceipt("session-a", 11L, cancelledBeforeStart = true)),
            reopened.terminalReceipts("session-a"))
        assertTrue(reopened.finishQueued("session-a", 11L, cancelledBeforeStart = true))
        assertFalse(reopened.finishQueued("session-a", 11L, cancelledBeforeStart = false))
        assertTrue(reopened.acknowledgeTerminals("session-b", listOf(11L)))
        assertEquals(1, reopened.terminalReceipts("session-a").size)
        assertTrue(reopened.acknowledgeTerminals("session-a", listOf(11L)))
        assertTrue(DurableConversationTurnClientStore(preferences, "scope-a").terminalReceipts("session-a").isEmpty())
    }

    @Test
    fun alreadyDeliveredQueueRemovalIsIdempotentButStorageFailureStillBlocksIt() {
        val preferences = FakeSharedPreferences()
        val store = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(store.enqueue("session-a", 11L))
        assertTrue(store.enqueue("session-a", 12L))
        assertTrue(store.enqueue("session-b", 11L))
        assertTrue(listOf(11L, 12L).all { store.removeDeliveredQueued("session-a", it) })
        assertTrue(listOf(11L, 12L).all { store.removeDeliveredQueued("session-a", it) })
        assertEquals(listOf(11L), store.queued("session-b").map { it.turnId })
        preferences.failNextCommit = true
        assertFalse(store.enqueue("session-a", 13L))
        assertFalse(store.removeDeliveredQueued("session-a", 11L))
    }

    @Test
    fun switchingSessionsParksEachUnfinishedOwnerWithoutOverwritingIt() {
        val preferences = FakeSharedPreferences()
        val source = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(source.begin("session-a", 10L, admissionPending = true))
        assertTrue(source.enqueue("session-a", 11L))
        assertTrue(source.restoreParked("session-b"))
        assertNull(source.load())
        assertTrue(source.begin("session-b", 20L, admissionPending = true))
        assertTrue(source.restoreParked("session-a"))
        assertEquals(10L, source.load()?.turnId)
        assertTrue(source.load()!!.admissionPending)
        assertEquals(listOf(11L), source.queued("session-a").map { it.turnId })
        val reopened = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(reopened.restoreParked("session-b"))
        assertEquals(20L, reopened.load()?.turnId)
        assertTrue(reopened.load()!!.admissionPending)
        assertTrue(reopened.restoreParked("session-a"))
        assertEquals(10L, reopened.load()?.turnId)
    }

    @Test
    fun lateTerminalForParkedSessionPreservesCurrentOwner() {
        val preferences = FakeSharedPreferences()
        val source = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(source.begin("session-a", 10L, admissionPending = true))
        assertTrue(source.restoreParked("session-b"))
        assertTrue(source.begin("session-b", 20L))
        assertTrue(source.recordTerminalEvidence("session-a", 10L))
        assertTrue(source.finishTerminal("session-a", 10L))
        assertEquals(20L, source.load()?.turnId)
        assertEquals(listOf(10L), source.terminalReceipts("session-a").map { it.turnId })
        assertTrue(source.restoreParked("session-a"))
        assertNull(source.load())
        assertTrue(source.restoreParked("session-b"))
        assertEquals(20L, source.load()?.turnId)
    }

    @Test
    fun deliveredQueueMembersRequireTheExactFullyProjectedTranscriptUuid() {
        val dto = MessageDto(transcriptUuid = "row-a", role = "user",
            blocks = listOf(MessageBlockDto.Text("same text".toClientTextDto())))
        val rows = transcriptFromDtos(listOf(dto, dto.copy(transcriptUuid = "row-b")))
        val proven = fullyVisibleUserTranscriptUuids(listOf(dto, dto.copy(transcriptUuid = "row-b")), rows)
        val delivered = QueuedTurnStatusDto.Delivered(batchOwnerTurnId = 11uL,
            memberTurnIds = listOf(11uL, 12uL), transcriptUuid = "row-a")
        assertEquals(listOf(11L, 12L), confirmedQueuedMembers(11uL, delivered, proven, rows))
        assertTrue(confirmedQueuedMembers(99uL, delivered, proven, rows).isEmpty())
        assertTrue(confirmedQueuedMembers(11uL, delivered.copy(transcriptUuid = "missing"), proven, rows).isEmpty())
        assertTrue(confirmedQueuedMembers(11uL, delivered.copy(memberTurnIds = listOf(11uL, 11uL)), proven, rows).isEmpty())
        val folded = rows + Message(role = Role.Ai, text = "fold", loopFoldedItemIds = setOf(rows.first().id))
        assertTrue(confirmedQueuedMembers(11uL, delivered, proven, folded).isEmpty())
        val filtered = dto.copy(blocks = listOf(MessageBlockDto.Text("  ".toClientTextDto())))
        assertTrue(fullyVisibleUserTranscriptUuids(listOf(filtered), transcriptFromDtos(listOf(filtered))).isEmpty())
        val unpaired = dto.copy(blocks = listOf(MessageBlockDto.Text("\uD800".toClientTextDto())))
        assertTrue(fullyVisibleUserTranscriptUuids(listOf(unpaired), transcriptFromDtos(listOf(unpaired))).isEmpty())
        val emoji = dto.copy(blocks = listOf(MessageBlockDto.Text("\uD83D\uDE00".toClientTextDto())))
        assertEquals(setOf("row-a"), fullyVisibleUserTranscriptUuids(listOf(emoji), transcriptFromDtos(listOf(emoji))))
    }

    @Test
    fun failedQueuedPersistenceIsReportedBeforeACommandCanBeSubmitted() {
        val preferences = FakeSharedPreferences()
        val store = DurableConversationTurnClientStore(preferences, "scope-a")
        preferences.failNextCommit = true
        assertFalse(store.enqueue("session-a", 11L))
    }

    @Test
    fun failedCommitPoisonsEveryScopeEvenWhenMemoryWasAlreadyMutated() {
        val preferences = FakeSharedPreferences()
        val first = DurableConversationTurnClientStore(preferences, "scope-a")
        val other = DurableConversationTurnClientStore(preferences, "scope-b")
        assertTrue(first.begin("session-a", 10L))
        preferences.failNextCommit = true
        assertFalse(first.finishTerminal("session-a", 10L))
        assertNull(first.load()) // fake matches SharedPreferences: memory changed before disk failure
        val commits = preferences.commitCalls
        assertFalse(first.begin("session-a", 11L))
        assertFalse(other.begin("session-b", 20L))
        assertEquals(commits, preferences.commitCalls) // no later write can flush the failed mutation
    }

    @Test
    fun userRequestedExitKeepsCurrentAndParkedOwnersWithoutAutoResume() {
        val preferences = FakeSharedPreferences()
        val store = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(store.begin("session-a", 10L))
        assertTrue(store.restoreParked("session-b"))
        assertTrue(store.begin("session-b", 20L))
        assertTrue(store.markUncertainAfterUserExit())
        assertTrue(store.load()!!.admissionPending)
        assertTrue(store.restoreParked("session-a"))
        assertTrue(store.load()!!.admissionPending)
    }

    @Test
    fun reentrantRecoveryCallbackRunsAfterTheCurrentProjection() {
        val ingress = DurableRecoveryIngress().newSource()
        ingress.acquireInterest("session-a", 88uL)
        val paused = snapshot(TurnRecoveryStateDto.PAUSED_RECOVERABLE, 88u, 0u, revision = 1uL)
        val cancelled = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 0u, revision = 2uL)
        val effects = mutableListOf<String>()

        assertTrue(ingress.dispatch(ClientEvent.TurnRecoveryState(paused)) {
            assertTrue(ingress.dispatch(ClientEvent.TurnRecoveryState(cancelled)) { effects += "cancelled" })
            effects += "paused"
        })
        assertEquals(listOf("paused", "cancelled"), effects)
        assertFalse(ingress.dispatch(ClientEvent.TurnRecoveryState(paused)) { effects += "stale" })
    }

    @Test
    fun unownedSnapshotDoesNotAdvanceTheSharedRevision() {
        val source = DurableRecoveryIngress().newSource()
        source.acquireInterest("session-a", 88uL)
        val terminal = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 0u, revision = 2uL)
        assertFalse(source.dispatch(ClientEvent.TurnRecoveryState(terminal), admit = { false }) { error("unowned") })
        assertTrue(source.dispatch(ClientEvent.TurnRecoveryState(terminal.copy(
            state = TurnRecoveryStateDto.PAUSED_RECOVERABLE, revision = 1uL,
        ))) {})
    }

    @Test
    fun watermarksAreSharedUntilAllLeasesDrainAndRemainBoundedAcrossTurns() {
        val ingress = DurableRecoveryIngress()
        val old = ingress.newSource("store-a")
        val current = ingress.newSource("store-a")
        val unrelated = ingress.newSource("store-b")
        old.acquireInterest("session-a", 88uL)
        current.acquireInterest("session-a", 88uL)
        unrelated.acquireInterest("session-a", 88uL)
        val terminal = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 0u, revision = 8uL)
        assertTrue(old.dispatch(ClientEvent.TurnRecoveryState(terminal)) {})
        val fence = old.fence("session-a", 88uL)
        val oldLease = old.detachInterest("session-a", 88uL)!!
        assertTrue(fence()) // an already queued consumer still owns it
        assertFalse(current.dispatch(ClientEvent.TurnRecoveryState(terminal.copy(
            revision = 7uL, state = TurnRecoveryStateDto.PAUSED_RECOVERABLE,
        ))) {})
        assertTrue(unrelated.dispatch(ClientEvent.TurnRecoveryState(terminal.copy(
            revision = 7uL, state = TurnRecoveryStateDto.PAUSED_RECOVERABLE,
        ))) {})
        old.releaseInterest(oldLease)
        assertFalse(fence())
        current.retire()
        unrelated.retire()
        assertEquals(0, ingress.watermarkCountForTesting())

        val reentered = ingress.newSource("store-a")
        reentered.acquireInterest("session-a", 88uL)
        assertTrue(reentered.dispatch(ClientEvent.TurnRecoveryState(terminal)) {})
        val oldGeneration = reentered.fence("session-a", 88uL)
        val detached = reentered.detachInterest("session-a", 88uL)!!
        assertTrue(oldGeneration())
        reentered.acquireInterest("session-a", 88uL)
        assertTrue(oldGeneration()) // existing consumers may still drain the old batch
        assertFalse(reentered.dispatch(ClientEvent.TurnRecoveryState(terminal.copy(
            revision = 7uL, state = TurnRecoveryStateDto.PAUSED_RECOVERABLE,
        ))) {})
        reentered.releaseInterest(detached)
        assertFalse(oldGeneration())
        reentered.retire()
        assertEquals(0, ingress.watermarkCountForTesting())

        val source = ingress.newSource("store-a")
        repeat(10_000) { index ->
            val turn = index.toULong()
            source.acquireInterest("session-a", turn)
            assertTrue(source.dispatch(ClientEvent.TurnRecoveryState(terminal.copy(turnId = turn))) {})
            source.releaseInterest(source.detachInterest("session-a", turn)!!)
            assertEquals(0, ingress.watermarkCountForTesting())
        }
    }

    @Test
    fun timedOutAttachRetainsBootstrapFenceAndDropsTheOldBatch() {
        val ingress = DurableAttachIngress()
        val snapshot = snapshot(TurnRecoveryStateDto.PAUSED_RECOVERABLE, 88u, 1u, revision = 1uL)
        val old = ingress.begin("session-a", 88uL, "old-request", uiOwner = true, expectsResume = true)
        ingress.end(old) // timeout before the old Started reaches this source
        assertTrue(ingress.bufferSnapshot(ClientEvent.TurnRecoveryState(snapshot)))
        assertFalse(ingress.started("old-request", snapshot, belongsToSource = true))
        assertFalse(ingress.mayForwardEnvelope("session-a", 88uL))
        assertEquals(null, ingress.finished("old-request", "session-a", 88uL))
        assertFalse(ingress.mayForwardEnvelope("session-a", 88uL)) // still needs authoritative bootstrap
        val next = ingress.begin("session-a", 88uL, "new-request", uiOwner = true, expectsResume = true)
        assertTrue(ingress.started("new-request", snapshot, belongsToSource = true))
        ingress.confirmStarted(snapshot)
        assertTrue(ingress.mayForwardEnvelope("session-a", 88uL))
        assertEquals(next, ingress.finished("new-request", "session-a", 88uL))
    }

    @Test
    fun failedSecondAttachKeepsTheEstablishedOwnersLiveIngress() {
        val ingress = DurableAttachIngress()
        val snapshot = snapshot(TurnRecoveryStateDto.RUNNING, 88u, 1u, revision = 1uL)
        ingress.noteLiveOwnership("session-a", 88uL)
        val second = ingress.begin("session-a", 88uL, "second", uiOwner = true, expectsResume = false,
            existingOwner = ingress.hasEstablishedOwner("session-a", 88uL))
        assertFalse(ingress.bufferSnapshot(ClientEvent.TurnRecoveryState(snapshot)))
        assertEquals("second", ingress.routeEnvelope("session-a", 88uL).excludedRequestId)
        ingress.end(second)
        assertFalse(ingress.bufferSnapshot(ClientEvent.TurnRecoveryState(snapshot)))
        assertTrue(ingress.mayForwardEnvelope("session-a", 88uL))
        assertFalse(ingress.started("second", snapshot, belongsToSource = true))
        assertFalse(ingress.mayForwardEnvelope("session-a", 88uL))
        assertNull(ingress.finished("second", "session-a", 88uL))
        assertTrue(ingress.mayForwardEnvelope("session-a", 88uL))
    }

    @Test
    fun queuedTurnWaitsForCancellationDrainAndMissingTerminalStaysBlocked() {
        val interest = DurableTurnIngressInterest()
        interest.begin("session-a", 1)
        interest.enqueue("session-a", 2uL)
        interest.beginCancellation("session-a", 1uL)
        interest.markSettled("session-a", 1uL, recoverySnapshot = true)
        assertFalse(interest.canAdopt("session-a", 2uL, "session-a"))
        assertFalse(interest.isQueued("session-b", 2uL, "session-a"))
        assertTrue(interest.finishCancellation("session-a", 1uL))
        assertTrue(interest.canAdopt("session-a", 2uL, "session-a"))

        interest.begin("session-a", 3)
        interest.enqueue("session-a", 4uL)
        interest.beginCancellation("session-a", 3uL)
        assertFalse(interest.finishCancellation("session-a", 3uL))
        assertFalse(interest.canAdopt("session-a", 4uL, "session-a"))

        interest.begin("session-a", 5)
        interest.enqueue("session-a", 6uL)
        interest.beginCancellation("session-a", 5uL)
        assertEquals(false to false, interest.abortCancellation("session-a", 5uL))
        interest.markSettled("session-a", 5uL, recoverySnapshot = true)
        assertTrue(interest.canAdopt("session-a", 6uL, "session-a"))

        interest.begin("session-a", 7)
        interest.enqueue("session-a", 8uL)
        interest.beginCancellation("session-a", 7uL)
        interest.markSettled("session-a", 7uL, recoverySnapshot = true)
        assertEquals(true to false, interest.abortCancellation("session-a", 7uL))
        assertFalse(interest.canAdopt("session-a", 8uL, "session-a"))
        assertTrue(interest.finishFailedCancellationAfterTurnEnd("session-a", 7uL))
        assertTrue(interest.canAdopt("session-a", 8uL, "session-a"))
    }

    @Test
    fun reentrantSourceRetirementStopsLaterCheckpointAndRelayEffects() {
        val source = DurableRecoveryIngress().newSource()
        source.acquireInterest("session-a", 88uL)
        val effects = mutableListOf<String>()
        val event = ClientEvent.TurnRecoveryState(snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 0u, revision = 2uL))
        assertTrue(source.dispatch(event) {
            effects += "stateflow"
            source.retire()
            if (source.isActive()) effects += "checkpoint"
            if (source.isActive()) effects += "relay"
        })
        assertEquals(listOf("stateflow"), effects)
        assertFalse(source.dispatch(event) { effects += "late" })
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test
    fun audioContextRefreshesStayOrderedAndLateRegistrationIsRemovedOnClose() = kotlinx.coroutines.test.runTest {
        val queue = OrderedSuspendQueue(backgroundScope)
        val firstMayFinish = CompletableDeferred<Unit>()
        val lateMayFinish = CompletableDeferred<Unit>()
        val order = mutableListOf<String>()
        val removed = mutableListOf<Pair<String, ULong>>()
        val bindings = MediaAudioSessionContextBindings { session, generation -> removed += session to generation }

        queue.offer {
            order += "first-start"
            firstMayFinish.await()
            bindings.register("session-a", 1uL)
            order += "first-finish"
        }
        queue.offer {
            order += "second-start"
            bindings.register("session-a", 2uL)
        }
        runCurrent()
        assertEquals(listOf("first-start"), order)
        firstMayFinish.complete(Unit)
        runCurrent()
        assertEquals(listOf("first-start", "first-finish", "second-start"), order)

        queue.offer {
            lateMayFinish.await()
            bindings.register("session-b", 3uL)
        }
        runCurrent()
        bindings.close()
        lateMayFinish.complete(Unit)
        runCurrent()
        assertEquals(listOf("session-a" to 2uL, "session-b" to 3uL), removed)
    }

    @Test
    fun backgroundRecoveryRequiresBothSessionAndTurn() {
        val background = ConversationBackgroundSnapshot("session-a", 88L, null)
        val matching = snapshot(TurnRecoveryStateDto.CANCELLED, 88u, 0u)
        assertTrue(recoveryMatchesBackgroundTurn(matching, background))
        assertFalse(recoveryMatchesBackgroundTurn(matching.copy(sessionId = "session-b"), background))
        assertFalse(recoveryMatchesBackgroundTurn(matching.copy(turnId = 89u), background))
    }

    @Test
    fun workspaceBootstrapBindsFirstPromptBeforeAsynchronousListing() {
        val owner = bootstrapWorkspaceSession(null, "session-a", SessionMode.Code) ?: error("missing owner")
        val store = DurableConversationTurnClientStore(FakeSharedPreferences(), "scope")
        val interest = DurableTurnIngressInterest()
        interest.begin(owner.sessionId, 91L)
        store.begin(owner.sessionId, 91L)
        assertEquals("session-a", store.load()?.sessionId)
        assertEquals(91L, store.load()?.turnId)
        assertTrue(interest.accepts("session-a", 91uL, owner.sessionId))
        assertFalse(interest.accepts("session-b", 91uL, owner.sessionId))
        assertFalse(interest.accepts("session-a", 92uL, owner.sessionId))
        val confirmed = owner.copy(sessionId = "confirmed", kind = SessionActivationKind.Resumed)
        assertEquals(confirmed, bootstrapWorkspaceSession(confirmed, "session-a", SessionMode.Code))
    }

    private fun snapshot(
        state: TurnRecoveryStateDto,
        turnId: ULong,
        lastSequence: ULong,
        revision: ULong = 1uL,
    ) = TurnRecoverySnapshotDto(
        sessionId = "session-a",
        turnId = turnId,
        revision = revision,
        state = state,
        firstSequence = 1u,
        lastSequence = lastSequence,
        safeToResume = true,
        projectionUncertain= false,
        reason = null,
    )
}

private data class RecoveredPrompt(
    val text: String,
    val images: List<ImageRefDto>,
    val turnId: Long,
    val visualization: VisualizationRef?,
    val mode: PromptModeDto?,
)

private class RecordingRecoveredSource : ConversationSource, SlashCommandConversationSource {
    val prompts = mutableListOf<RecoveredPrompt>()
    val slashCommands = mutableListOf<Triple<ClientTextDto, List<ImageRefDto>, Long>>()

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
    ): Flow<ReplyEvent> {
        prompts += RecoveredPrompt(text, images, turnId, visualizationContext, promptMode)
        return flowOf(ReplyEvent.End)
    }

    override fun submitSlashCommand(raw: ClientTextDto, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> {
        slashCommands += Triple(raw, images, turnId)
        return flowOf(ReplyEvent.CommandOutput("done", false, turnId.toULong()))
    }
}

internal class FakeSharedPreferences : SharedPreferences {
    private val values = linkedMapOf<String, Any?>()
    var failNextCommit = false
    var commitCalls = 0

    override fun getAll(): MutableMap<String, *> = values.toMutableMap()

    override fun getString(key: String?, defValue: String?): String? =
        values[key] as? String ?: defValue

    override fun getStringSet(key: String?, defValues: MutableSet<String>?): MutableSet<String>? =
        @Suppress("UNCHECKED_CAST")
        (values[key] as? MutableSet<String>) ?: defValues

    override fun getInt(key: String?, defValue: Int): Int = values[key] as? Int ?: defValue

    override fun getLong(key: String?, defValue: Long): Long = values[key] as? Long ?: defValue

    override fun getFloat(key: String?, defValue: Float): Float = values[key] as? Float ?: defValue

    override fun getBoolean(key: String?, defValue: Boolean): Boolean =
        values[key] as? Boolean ?: defValue

    override fun contains(key: String?): Boolean = values.containsKey(key)

    override fun edit(): SharedPreferences.Editor = Editor(values) {
        commitCalls++
        failNextCommit.also { failNextCommit = false }
    }

    override fun registerOnSharedPreferenceChangeListener(
        listener: SharedPreferences.OnSharedPreferenceChangeListener?,
    ) = Unit

    override fun unregisterOnSharedPreferenceChangeListener(
        listener: SharedPreferences.OnSharedPreferenceChangeListener?,
    ) = Unit

    private class Editor(
        private val values: MutableMap<String, Any?>,
        private val shouldFail: () -> Boolean,
    ) : SharedPreferences.Editor {
        private val pending = linkedMapOf<String, Any?>()
        private var clearRequested = false

        override fun putString(key: String?, value: String?): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = value
        }

        override fun putStringSet(
            key: String?,
            values: MutableSet<String>?,
        ): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = values
        }

        override fun putInt(key: String?, value: Int): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = value
        }

        override fun putLong(key: String?, value: Long): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = value
        }

        override fun putFloat(key: String?, value: Float): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = value
        }

        override fun putBoolean(key: String?, value: Boolean): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = value
        }

        override fun remove(key: String?): SharedPreferences.Editor = apply {
            pending[key.orEmpty()] = Removed
        }

        override fun clear(): SharedPreferences.Editor = apply {
            clearRequested = true
        }

        override fun commit(): Boolean {
            apply()
            return !shouldFail()
        }

        override fun apply() {
            if (clearRequested) values.clear()
            pending.forEach { (key, value) ->
                if (value === Removed) values.remove(key) else values[key] = value
            }
            pending.clear()
            clearRequested = false
        }
        private companion object {
            val Removed = Any()
        }
    }
}
