package com.lingxi.code.voice

import com.lingxi.code.settings.VersionedAudioConfiguration
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import com.lingxi.code.voice.audio.NavigationDrainingRealtimeSession
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import com.lingxi.code.conversation.AutomaticReplySpeechGate
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FlowVoiceControllerTest {
    @Test fun rejectedVoiceSubmissionFinishesOnlyItsOwnListenGeneration() = runTest {
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 1L))
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        var firstGeneration = 0L
        val recognized = mutableListOf<String>()
        controller.listen({ text ->
            recognized += text
            firstGeneration = controller.submissionGeneration()
            controller.rejectSubmission(firstGeneration)
        }) {}
        runCurrent()
        assertEquals(listOf("recognized"), recognized)
        assertEquals(OrbPhase.Idle, controller.state.value.phase)

        controller.listen({ recognized += it }) {}
        runCurrent()
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        controller.rejectSubmission(firstGeneration)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        controller.rejectSubmission(controller.submissionGeneration())
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        controller.dispose()
    }
    @Test
    fun explicitFlowInputWaitsForHiddenAutomaticSpeechToDrainAndPreservesItsQueuedReply() = runTest {
        var flowVisible = false
        val gate = AutomaticReplySpeechGate { flowVisible }
        gate.configure(enabled = true, foreground = true)
        val admission = Mutex()
        val cleanup = CompletableDeferred<Unit>()
        val spoken = mutableListOf<String>()
        suspend fun speak(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> {
                spoken += operation.text
                if (operation.text == "first") {
                    try { awaitCancellation() } finally { withContext(NonCancellable) { cleanup.await() } }
                }
                DeviceAudioResult.PlaybackCompleted(1)
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val playerA = VoiceSpeechPlayer(::speak, admission, dispatcher)
        playerA.speakAsync("first", automatic = true, awaitStart = gate::awaitStart, mayStart = gate::mayStart)
        playerA.speakAsync("queued", automatic = true, awaitStart = gate::awaitStart, mayStart = gate::mayStart)
        runCurrent()
        assertEquals(listOf("first"), spoken)

        val serviceB = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 1L))
        serviceB.onListenOpened = { it.ready() }
        val flowB = FlowVoiceController(serviceB, { true }, {}, dispatcher, admission)
        flowVisible = true
        gate.signal()
        playerA.yieldAutomaticPlayback()
        flowB.ensureListening({}, {})
        runCurrent()
        assertTrue(serviceB.sessions.isEmpty()) // cleanup still owns device admission
        assertTrue(playerA.hasAudioWork())

        cleanup.complete(Unit)
        runCurrent()
        assertEquals(1, serviceB.sessions.size)
        assertEquals(listOf("first"), spoken)
        flowB.pause()
        flowVisible = false
        gate.signal()
        runCurrent()
        assertEquals(listOf("first", "queued"), spoken)
        flowB.dispose()
        playerA.dispose()
    }

    @Test
    fun permissionGrantResumesThePendingListen() = runTest {
        var permissionGranted = false
        var requestCount = 0
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 40L))
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { permissionGranted },
            requestMicrophonePermission = { requestCount += 1 },
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        assertEquals(1, requestCount)
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        assertTrue(service.listenOperations.isEmpty())

        permissionGranted = true
        controller.onMicrophonePermissionResult(granted = true)
        runCurrent()

        assertEquals(listOf("recognized"), sent)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        assertEquals(1, service.listenOperations.size)
        controller.dispose()
    }

    @Test
    fun permissionGrantAfterPauseDoesNotStartAStaleListen() = runTest {
        var permissionGranted = false
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 40L))
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { permissionGranted },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        controller.pause()
        permissionGranted = true
        controller.onMicrophonePermissionResult(granted = true)
        runCurrent()

        assertTrue(sent.isEmpty())
        assertTrue(service.listenOperations.isEmpty())
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        controller.dispose()
    }

    @Test
    fun restoringOverlayDoesNotCancelTheOwnersAdmittedTurnAfterPermission() = runTest {
        var permissionGranted = false
        var cancellations = 0
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), 40L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { permissionGranted }, {}, UnconfinedTestDispatcher(testScheduler))
        controller.ensureListening({}, { cancellations++ })
        permissionGranted = true
        controller.onMicrophonePermissionResult(true)
        runCurrent()
        assertEquals(0, cancellations)
        assertEquals(1, service.sessions.size)
        controller.dispose()
    }

    @Test
    fun stoppingRealtimeListenReturnsItsFinalTranscriptAndPreservesPinnedConfiguration() = runTest {
        val snapshot = VersionedAudioConfiguration(AudioConfigurationV4(language = "en-US"), revision = 41L)
        val service = FakeFlowAudioService(snapshot)
        service.onListenOpened = { session ->
            session.ready()
            session.onStop = { session.finish("kept transcript") }
        }
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()
        controller.stopListening()

        assertEquals(listOf("kept transcript"), sent)
        assertEquals("kept transcript", controller.state.value.userCaption)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        assertTrue(!controller.state.value.isFinalizing)
        assertTrue(service.sessions.single().stopRequested)
        val listen = service.listenOperations.single()
        assertEquals(snapshot.configuration, listen.configuration)
        assertEquals(snapshot.revision, listen.configurationRevision)
        controller.dispose()
    }

    @Test
    fun stopRequestedBeforeSessionAdmissionStopsItAsSoonAsItOpens() = runTest {
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 44L))
        val admission = CompletableDeferred<RealtimeSpeechSession>()
        service.pendingAdmission = admission
        service.onListenOpened = { session ->
            session.ready()
            session.onStop = { session.finish("early-stop transcript") }
        }
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()
        controller.stopListening()
        assertTrue(controller.state.value.isFinalizing)

        admission.complete(service.sessions.single())
        runCurrent()

        assertTrue(service.sessions.single().stopRequested)
        assertEquals(listOf("early-stop transcript"), sent)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        controller.dispose()
    }

    @Test
    fun realtimeListenErrorIsSurfacedWithoutSendingATranscript() = runTest {
        val snapshot = VersionedAudioConfiguration(AudioConfigurationV4(), revision = 42L)
        val service = FakeFlowAudioService(snapshot)
        service.onListenOpened = { session ->
            session.ready()
            session.fail("audio_io_unavailable", "Microphone is busy.")
        }
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()

        assertTrue(sent.isEmpty())
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        assertEquals(DeviceAudioErrorKind.Unavailable, controller.state.value.error?.kind)
        assertEquals("Microphone is busy.", controller.state.value.error?.message)
        assertEquals(snapshot.revision, controller.state.value.configurationRevision)
        controller.dispose()
    }

    @Test
    fun lateCallbacksFromReplacedListenAreIgnored() = runTest {
        val snapshot = VersionedAudioConfiguration(AudioConfigurationV4(), revision = 43L)
        val service = FakeFlowAudioService(snapshot)
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()
        val staleSession = service.sessions.single()
        controller.listen(sent::add) {}
        runCurrent()
        val currentSession = service.sessions.last()

        staleSession.finish("stale transcript")
        assertTrue(sent.isEmpty())
        assertEquals(OrbPhase.Listening, controller.state.value.phase)

        currentSession.finish("current transcript")
        assertEquals(listOf("current transcript"), sent)
        assertEquals("current transcript", controller.state.value.userCaption)
        controller.dispose()
    }

    @Test
    fun pauseCancelsRealtimeListenAndIgnoresItsLateTranscript() = runTest {
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), revision = 45L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()
        val session = service.sessions.single()
        controller.pause()

        assertTrue(session.cancelled)
        session.finish("late after pause")
        assertTrue(sent.isEmpty())
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
    }

    @Test
    fun interactionPinsListenConfigurationThroughReplyAndRefreshesNextListen() = runTest {
        val first = VersionedAudioConfiguration(
            configuration = AudioConfigurationV4(language = "en-US", rate = 0.8, autoPlayReplies = false),
            revision = 31L,
        )
        val second = VersionedAudioConfiguration(
            configuration = AudioConfigurationV4(language = "fr-FR", rate = 1.4, autoPlayReplies = true),
            revision = 32L,
        )
        val service = FakeFlowAudioService(first)
        val controller = FlowVoiceController(
            audioService = service,
            hasMicrophonePermission = { true },
            requestMicrophonePermission = {},
            dispatcher = UnconfinedTestDispatcher(testScheduler),
        )
        val sent = mutableListOf<String>()

        controller.listen(sent::add) {}
        runCurrent()
        assertEquals(listOf("recognized"), sent)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        assertEquals(31L, controller.state.value.configurationRevision)

        service.snapshot = second
        controller.updateReply("reply", streaming = true)
        controller.updateReply("reply", streaming = false)
        runCurrent()

        val firstListen = service.listenOperations.single()
        val reply = service.operations.filterIsInstance<DeviceAudioOperation.Speak>().single()
        assertEquals(first.configuration, firstListen.configuration)
        assertEquals(first.configuration, reply.configuration)
        assertEquals(OrbPhase.Idle, controller.state.value.phase)

        controller.listen(sent::add) {}
        runCurrent()
        val listens = service.listenOperations
        assertEquals(2, listens.size)
        assertEquals(second.configuration, listens.last().configuration)
        assertEquals(32L, controller.state.value.configurationRevision)
        assertTrue(service.owners.all { it.kind == AudioOwnerKey.Kind.Ui && it.id.startsWith("flow-mode-") })
        controller.dispose()
    }

    @Test
    fun changingProfileWithTheSameModelCancelsNativeRealtimeAndSuppressesLateTranscript() = runTest {
        val config = AudioConfigurationV4(conversation = com.lingxi.code.voice.audio.AudioConversationPreference(mode = "realtime"))
        val service = FakeFlowAudioService(VersionedAudioConfiguration(config, 50L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        val sent = mutableListOf<String>()
        controller.updateSessionBinding("shared-model", "profile-a")
        controller.listen(sent::add) {}
        runCurrent()
        val session = service.sessions.single()
        controller.updateSessionBinding("shared-model", "profile-a")
        assertTrue(!session.cancelled)
        controller.updateSessionBinding("shared-model", "profile-b")
        session.finish("late old profile")
        runCurrent()
        assertTrue(session.cancelled)
        assertTrue(sent.isEmpty())
        assertEquals("", controller.state.value.userCaption)
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        controller.dispose()
    }

    @Test
    fun changingModelCancelsActiveAgentFlowListen() = runTest {
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), 51L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        controller.updateSessionBinding("model-a", "profile")
        controller.listen({}) {}
        runCurrent()
        controller.updateSessionBinding("model-b", "profile")
        runCurrent()
        assertTrue(service.sessions.single().cancelled)
        assertEquals(OrbPhase.Idle, controller.state.value.phase)
        controller.dispose()
    }

    @Test
    fun changingProfileDrainsAlreadyAdmittedRealtimeWork() = runTest {
        val config = AudioConfigurationV4(conversation = com.lingxi.code.voice.audio.AudioConversationPreference(mode = "realtime"))
        val service = FakeFlowAudioService(VersionedAudioConfiguration(config, 51L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        controller.updateSessionBinding("model", "profile-a")
        controller.listen({}) {}
        runCurrent()
        val session = service.sessions.single()
        checkNotNull(service.agentCallbacks).onAgentTurnStarted()
        controller.updateSessionBinding("model", "profile-b")
        assertTrue(session.navigationRequested)
        assertFalse(session.cancelled)
        session.finish("completed")
        controller.dispose()
    }

    @Test
    fun navigationPreservesFinalizingOrdinaryVoiceAndItsReply() = runTest {
        val service = FakeFlowAudioService(VersionedAudioConfiguration(AudioConfigurationV4(), 52L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        val sent = mutableListOf<String>()
        controller.listen(sent::add) {}
        runCurrent()
        val session = service.sessions.single()
        controller.stopListening()
        controller.navigationAway()
        assertFalse(session.cancelled)
        session.finish("finish on original owner")
        runCurrent()
        assertEquals(listOf("finish on original owner"), sent)
        controller.updateReply("original reply", streaming = false)
        runCurrent()
        assertEquals("original reply", service.operations.filterIsInstance<DeviceAudioOperation.Speak>().single().text)
        controller.dispose()
    }

    @Test
    fun realtimeNavigationHoldsDeviceAdmissionUntilOldOwnerCloses() = runTest {
        val config = AudioConfigurationV4(conversation = com.lingxi.code.voice.audio.AudioConversationPreference(mode = "realtime"))
        val firstService = FakeFlowAudioService(VersionedAudioConfiguration(config, 53L))
        val secondService = FakeFlowAudioService(VersionedAudioConfiguration(config, 53L))
        firstService.onListenOpened = { it.ready() }
        secondService.onListenOpened = { it.ready() }
        val admission = Mutex()
        val first = FlowVoiceController(firstService, { true }, {}, UnconfinedTestDispatcher(testScheduler), admission)
        val second = FlowVoiceController(secondService, { true }, {}, UnconfinedTestDispatcher(testScheduler), admission)
        first.listen({}) {}
        runCurrent()
        val firstSession = firstService.sessions.single()
        first.navigationAway()
        second.listen({}) {}
        runCurrent()
        assertTrue(firstSession.navigationRequested)
        assertFalse(firstSession.cancelled)
        assertTrue(secondService.sessions.isEmpty())

        firstSession.finish("old owner turn")
        runCurrent()
        assertEquals(1, secondService.sessions.size)
        first.dispose()
        second.dispose()
    }

    @Test
    fun navigationBeforeRealtimeAdmissionDrainsTheReturnedSession() = runTest {
        val config = AudioConfigurationV4(conversation = com.lingxi.code.voice.audio.AudioConversationPreference(mode = "realtime"))
        val service = FakeFlowAudioService(VersionedAudioConfiguration(config, 54L))
        val admission = CompletableDeferred<RealtimeSpeechSession>()
        service.pendingAdmission = admission
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        controller.listen({}) {}
        runCurrent()
        controller.navigationAway()
        val session = service.sessions.single()
        admission.complete(session)
        runCurrent()
        assertTrue(session.navigationRequested)
        assertFalse(session.cancelled)
        session.finish("completed")
        controller.dispose()
    }

    @Test
    fun realtimeDoesNotListenAgainUntilTheAgentTerminal() = runTest {
        val config = AudioConfigurationV4(conversation = com.lingxi.code.voice.audio.AudioConversationPreference(mode = "realtime"))
        val service = FakeFlowAudioService(VersionedAudioConfiguration(config, 55L))
        service.onListenOpened = { it.ready() }
        val controller = FlowVoiceController(service, { true }, {}, UnconfinedTestDispatcher(testScheduler))
        controller.listen({}) {}
        runCurrent()
        val callbacks = checkNotNull(service.agentCallbacks)
        callbacks.onAgentTurnStarted()
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        callbacks.onPlayback(true)
        callbacks.onPlayback(false)
        assertEquals(OrbPhase.Thinking, controller.state.value.phase)
        callbacks.onAgentTurnCompleted()
        assertEquals(OrbPhase.Listening, controller.state.value.phase)
        callbacks.onTranscript("late user transcript", assistant = false, final = true)
        assertEquals("late user transcript", controller.state.value.userCaption)
        assertEquals(OrbPhase.Listening, controller.state.value.phase)
        callbacks.onPlayback(false)
        assertEquals(OrbPhase.Listening, controller.state.value.phase)
        controller.dispose()
    }

    private class FakeFlowAudioService(var snapshot: VersionedAudioConfiguration) : FlowVoiceAudioService {
        val operations = mutableListOf<DeviceAudioOperation>()
        val owners = mutableListOf<AudioOwnerKey>()
        val listenOperations = mutableListOf<DeviceAudioOperation.Listen>()
        val sessions = mutableListOf<FakeRealtimeListenSession>()
        var pendingAdmission: CompletableDeferred<RealtimeSpeechSession>? = null
        var agentCallbacks: com.lingxi.code.voice.audio.RealtimeAgentCallbacks? = null
        var onListenOpened: (FakeRealtimeListenSession) -> Unit = { session ->
            session.ready()
            session.finish("recognized")
        }

        override suspend fun openRealtimeAgent(owner: AudioOwnerKey, configuration: AudioConfigurationV4,
            callbacks: com.lingxi.code.voice.audio.RealtimeAgentCallbacks): RealtimeSpeechSession {
            agentCallbacks = callbacks
            return openRealtimeListen(
                owner, DeviceAudioOperation.Listen(null, configuration), object : RealtimeSpeechCallbacks {
                    override fun onReady() = callbacks.onReady()
                    override fun onPartial(text: String) = callbacks.onTranscript(text, false, false)
                    override fun onFinal(text: String) = callbacks.onTranscript(text, false, true)
                    override fun onError(code: String, message: String, retriable: Boolean) = callbacks.onError(message)
                    override fun onClosed() = callbacks.onClosed()
                })
        }

        override fun configurationSnapshot() = snapshot

        override suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult {
            owners += owner
            operations += operation
            return when (operation) {
                is DeviceAudioOperation.Speak -> DeviceAudioResult.PlaybackCompleted(100L)
                DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
                else -> error("unexpected flow operation: $operation")
            }
        }

        override suspend fun openRealtimeListen(
            owner: AudioOwnerKey,
            operation: DeviceAudioOperation.Listen,
            callbacks: RealtimeSpeechCallbacks,
        ): RealtimeSpeechSession {
            owners += owner
            operations += operation
            listenOperations += operation
            val session = FakeRealtimeListenSession(callbacks).also { created ->
                sessions += created
                onListenOpened(created)
            }
            return pendingAdmission?.await() ?: session
        }
    }

    private class FakeRealtimeListenSession(
        private val callbacks: RealtimeSpeechCallbacks,
    ) : NavigationDrainingRealtimeSession {
        var stopRequested = false
        var cancelled = false
        var navigationRequested = false
        var onStop: () -> Unit = {}

        override fun stop() {
            stopRequested = true
            onStop()
        }

        override fun cancel() {
            cancelled = true
            callbacks.onClosed()
        }

        override fun finishForNavigation() { navigationRequested = true }

        override fun close() = cancel()

        fun ready() = callbacks.onReady()
        fun finish(text: String) {
            callbacks.onFinal(text)
            callbacks.onClosed()
        }
        fun fail(code: String, message: String) {
            callbacks.onError(code, message, retriable = true)
            callbacks.onClosed()
        }
    }
}
