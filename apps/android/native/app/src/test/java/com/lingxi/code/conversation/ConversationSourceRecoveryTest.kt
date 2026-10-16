package com.lingxi.code.conversation

import android.content.SharedPreferences
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.SessionMode
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
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

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
            submitted += ClientCommand.Cancel(turnId = 42u)
        }

        assertEquals(listOf(ClientCommand.Cancel(turnId = 42u)), submitted)
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

    private fun snapshot(
        state: TurnRecoveryStateDto,
        turnId: ULong,
        lastSequence: ULong,
    ) = TurnRecoverySnapshotDto(
        sessionId = "session-a",
        turnId = turnId,
        state = state,
        firstSequence = 1u,
        lastSequence = lastSequence,
        safeToResume = true,
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

private class FakeSharedPreferences : SharedPreferences {
    private val values = linkedMapOf<String, Any?>()

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

    override fun edit(): SharedPreferences.Editor = Editor(values)

    override fun registerOnSharedPreferenceChangeListener(
        listener: SharedPreferences.OnSharedPreferenceChangeListener?,
    ) = Unit

    override fun unregisterOnSharedPreferenceChangeListener(
        listener: SharedPreferences.OnSharedPreferenceChangeListener?,
    ) = Unit

    private class Editor(
        private val values: MutableMap<String, Any?>,
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
            return true
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
