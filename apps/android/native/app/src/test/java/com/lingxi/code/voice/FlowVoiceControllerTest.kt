package com.lingxi.code.voice

import com.lingxi.code.settings.VersionedAudioConfiguration
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FlowVoiceControllerTest {
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

    private class FakeFlowAudioService(var snapshot: VersionedAudioConfiguration) : FlowVoiceAudioService {
        val operations = mutableListOf<DeviceAudioOperation>()
        val owners = mutableListOf<AudioOwnerKey>()
        val listenOperations = mutableListOf<DeviceAudioOperation.Listen>()
        val sessions = mutableListOf<FakeRealtimeListenSession>()
        var pendingAdmission: CompletableDeferred<RealtimeSpeechSession>? = null
        var onListenOpened: (FakeRealtimeListenSession) -> Unit = { session ->
            session.ready()
            session.finish("recognized")
        }

        override suspend fun openRealtimeAgent(owner: AudioOwnerKey, configuration: AudioConfigurationV4,
            callbacks: com.lingxi.code.voice.audio.RealtimeAgentCallbacks): RealtimeSpeechSession = openRealtimeListen(
                owner, DeviceAudioOperation.Listen(null, configuration), object : RealtimeSpeechCallbacks {
                    override fun onReady() = callbacks.onReady()
                    override fun onPartial(text: String) = callbacks.onTranscript(text, false, false)
                    override fun onFinal(text: String) = callbacks.onTranscript(text, false, true)
                    override fun onError(code: String, message: String, retriable: Boolean) = callbacks.onError(message)
                    override fun onClosed() = callbacks.onClosed()
                })

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
    ) : RealtimeSpeechSession {
        var stopRequested = false
        var cancelled = false
        var onStop: () -> Unit = {}

        override fun stop() {
            stopRequested = true
            onStop()
        }

        override fun cancel() {
            cancelled = true
            callbacks.onClosed()
        }

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
