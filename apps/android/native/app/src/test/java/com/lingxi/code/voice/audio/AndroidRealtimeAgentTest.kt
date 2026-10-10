package com.lingxi.code.voice.audio

import com.lingxi.code.settings.VersionedAudioConfiguration
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidRealtimeAgentTest {
    private val request = DeviceAudioRequest(AudioOperationIdentity("realtime-operation", 1, 1), AudioOwnerKey.ui("voice"),
        null, 64, DeviceAudioOperation.Status(null))
    private val config = AudioConfigurationV4(conversation = AudioConversationPreference(mode = "realtime"))

    @Test
    fun interruptionModeWithoutVerifiedAecFailsBeforeProviderAndDeviceDispatch() = runTest {
        val cloud = Cloud()
        val driver = Driver()
        val core = AndroidAudioServiceCore(Runtime(), driver, 64, provider = cloud)
        try {
            core.openRealtimeAgent(request, config.copy(conversation = config.conversation.copy(interaction = "interruptible")), Callbacks())
            throw AssertionError("unsupported AEC mode was accepted")
        } catch (error: AudioOperationException) { assertEquals(DeviceAudioErrorKind.Unsupported, error.kind) }
        assertEquals(0, cloud.opened)
        assertEquals(0, driver.captures)
    }

    @Test
    fun nativePlaybackAcknowledgesOnlyConsumedAudioAndCancellationSuppressesLateTranscripts() = runTest {
        val cloud = Cloud()
        val driver = Driver()
        val callbacks = Callbacks()
        val core = AndroidAudioServiceCore(Runtime(), driver, 64, provider = cloud)
        core.openRealtimeAgent(request, config, callbacks)
        driver.captureStarted.await()
        assertEquals(1, cloud.sentChunks)
        cloud.event("""{"type":"transcript","role":"user","text":"hello","final":true}""")
        cloud.event("""{"type":"audio_delta","audioBase64":"AQACAA==","itemId":"assistant-item","sampleRateHz":24000,"channels":1,"encoding":"pcm16"}""")
        driver.playbackStarted.await()
        cloud.event("""{"type":"audio_delta","audioBase64":"AQACAA==","itemId":"assistant-item-2","sampleRateHz":24000,"channels":1,"encoding":"pcm16"}""")
        cloud.event("""{"type":"turn_completed"}""")
        assertFalse(cloud.acknowledged.isCompleted)
        driver.playbackGate.complete(Unit)
        assertEquals("assistant-item", cloud.acknowledged.await())
        cloud.allItemsAcknowledged.await()
        assertEquals(listOf("assistant-item", "assistant-item-2"), cloud.acknowledgementIds)
        core.cancel(request.identity)
        val transcriptCount = callbacks.transcripts.size
        cloud.event("""{"type":"transcript","role":"assistant","text":"late","final":true}""")
        assertEquals(transcriptCount, callbacks.transcripts.size)
        assertTrue(cloud.aborted)
        assertEquals(0, core.diagnostics().activeLeaseCount)
    }

    @Test
    fun providerInterruptionDuringPlaybackDrainAbortsWithoutAcknowledgingUnheardAudio() = runTest {
        val cloud = Cloud()
        val driver = Driver()
        val callbacks = Callbacks()
        val core = AndroidAudioServiceCore(Runtime(), driver, 64, provider = cloud)
        core.openRealtimeAgent(request, config, callbacks)
        driver.captureStarted.await()
        cloud.event("""{"type":"audio_delta","audioBase64":"AQACAA==","itemId":"unheard-item","sampleRateHz":24000,"channels":1,"encoding":"pcm16"}""")
        driver.playbackStarted.await()
        cloud.event("""{"type":"turn_completed"}""")
        cloud.event("""{"type":"interrupted"}""")
        callbacks.closed.await()
        assertFalse(cloud.acknowledged.isCompleted)
        assertTrue(cloud.aborted)
        assertEquals(0, core.diagnostics().activeLeaseCount)
    }

    private class Callbacks : RealtimeAgentCallbacks {
        val transcripts = mutableListOf<String>()
        val closed = CompletableDeferred<Unit>()
        override fun onReady() = Unit
        override fun onTranscript(text: String, assistant: Boolean, final: Boolean) { transcripts += text }
        override fun onPlayback(playing: Boolean) = Unit
        override fun onError(message: String) = Unit
        override fun onClosed() { closed.complete(Unit) }
    }

    private class Cloud : ProviderAudioBridge, ProviderRealtimeSession {
        lateinit var event: suspend (String) -> Unit
        var opened = 0
        var sentChunks = 0
        var aborted = false
        val acknowledged = CompletableDeferred<String?>()
        val allItemsAcknowledged = CompletableDeferred<Unit>()
        val acknowledgementIds = mutableListOf<String?>()
        override suspend fun capabilities(request: DeviceAudioRequest, configuration: AudioConfigurationV4, kind: String) =
            ProviderAudioCapability(true, "ready", null, "session-profile", "openai", "realtime")
        override suspend fun openRealtime(request: DeviceAudioRequest, configuration: AudioConfigurationV4, onEvent: suspend (String) -> Unit): ProviderRealtimeSession {
            opened++; event = onEvent
            event("""{"type":"session_ready","inputFormat":{"sampleRateHz":24000,"channels":1,"encoding":"pcm16"},"outputFormat":{"sampleRateHz":24000,"channels":1,"encoding":"pcm16"},"capabilities":{"audioTruncation":true}}""")
            return this
        }
        override suspend fun transcribe(request: DeviceAudioRequest, configuration: AudioConfigurationV4, capture: DeviceAudioCapture): SttResult = error("unexpected file STT")
        override suspend fun synthesize(request: DeviceAudioRequest, configuration: AudioConfigurationV4, text: String): Pair<ByteArray, Int> = error("unexpected TTS")
        override suspend fun cancel(operationId: String) = Unit
        override suspend fun sendAudio(pcm: ByteArray) { sentChunks++ }
        override suspend fun commitInput() = Unit
        override suspend fun interrupt(itemId: String?, audioEndMs: UInt?) = Unit
        override suspend fun playbackCompleted(itemId: String?) {
            acknowledgementIds += itemId
            acknowledged.complete(itemId)
            if (acknowledgementIds.size == 2) allItemsAcknowledged.complete(Unit)
        }
        override suspend fun close() = Unit
        override suspend fun abort() { aborted = true }
    }

    private class Driver : AndroidAudioDeviceDriver {
        var captures = 0
        val captureStarted = CompletableDeferred<Unit>()
        val playbackStarted = CompletableDeferred<Unit>()
        val playbackGate = CompletableDeferred<Unit>()
        override suspend fun streamCapture(lease: AudioLease, format: RealtimePcmFormat, onChunk: suspend (ByteArray) -> Unit) {
            captures++; onChunk(byteArrayOf(1, 0)); captureStarted.complete(Unit); awaitCancellation()
        }
        override suspend fun streamPlayback(lease: AudioLease, format: RealtimePcmFormat, chunks: ReceiveChannel<ByteArray>) {
            playbackStarted.complete(Unit)
            for (chunk in chunks) assertEquals(4, chunk.size)
            playbackGate.await()
        }
        override suspend fun startRecording(lease: AudioLease, sampleRateHz: Int, format: String, maxPayloadBytes: Int,
            onTerminated: (DeviceAudioError) -> Unit, mayStart: () -> Boolean): String = error("unexpected recording")
        override suspend fun stopRecording(lease: AudioLease, handle: String, maxPayloadBytes: Int): DeviceAudioCapture = error("unexpected recording")
        override suspend fun play(lease: AudioLease, pcm: ByteArray, sampleRateHz: Int): Long = error("unexpected one-shot playback")
        override suspend fun playMedia(lease: AudioLease, target: String, mayStart: () -> Boolean, onTerminal: (DeviceAudioError?) -> Unit): DeviceMediaPlaybackState = error("unexpected media")
        override suspend fun controlMedia(lease: AudioLease, command: OffloadMediaCommand): DeviceMediaPlaybackState = error("unexpected media")
        override suspend fun stop(lease: AudioLease) = Unit
    }

    private class Runtime : AudioServiceSpeechRuntime {
        override fun configuration() = AudioConfigurationV4()
        override fun configurationSnapshot() = VersionedAudioConfiguration(configuration(), 1)
        override fun microphonePermissionGranted() = true
        override fun resolveRecognition(configuration: AudioConfigurationV4, language: String?, systemStatusOverride: AudioReadiness?): AudioRouteResolution = error("unexpected local recognition")
        override fun resolveSpeech(configuration: AudioConfigurationV4, language: String?, voice: String?, rate: Float?, systemStatusOverride: AudioReadiness?): AudioRouteResolution = error("unexpected local speech")
        override suspend fun transcribe(language: String?, configuration: AudioConfigurationV4): SttResult = error("unexpected local recognition")
        override fun openRealtimeSession(language: String?, configuration: AudioConfigurationV4, callbacks: RealtimeSpeechCallbacks): RealtimeSpeechSession = error("unexpected local recognition")
        override suspend fun render(text: String, language: String?, voice: String?, rate: Float?, maxPcmBytes: Int, configuration: AudioConfigurationV4): Pair<ByteArray, Int> = error("unexpected local speech")
    }
}
