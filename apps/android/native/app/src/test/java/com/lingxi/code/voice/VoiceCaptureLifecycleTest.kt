package com.lingxi.code.voice

import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceCaptureLifecycleTest {
    @Test
    fun heldCaptureWaitsForPlaybackCleanupAndDrainsBeforeReleasingAdmission() = runTest {
        val admission = Mutex()
        val speechCleanup = CompletableDeferred<Unit>()
        val captureCleanup = CompletableDeferred<Unit>()
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        suspend fun speak(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> try {
                awaitCancellation()
            } finally {
                withContext(NonCancellable) { speechCleanup.await() }
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val playerA = VoiceSpeechPlayer(::speak, admission, dispatcher)
        playerA.speakAsync("A", automatic = true)
        runCurrent()

        val scope = CoroutineScope(SupervisorJob() + dispatcher)
        var opened = 0
        var delivered: VoiceCaptureResult? = null
        val capture = VoiceCapture(
            requestPermission = {}, hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                ServiceRealtimeListenSession(scope, callbacks, admission, drain = { captureCleanup.await() }) { serviceCallbacks ->
                    opened++
                    FakeRealtimeSession(serviceCallbacks).also { it.finalOnStop = "B transcript"; it.ready() }
                }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        capture.start { delivered = it }
        playerA.yieldAutomaticPlayback()
        runCurrent()
        assertEquals(0, opened)
        speechCleanup.complete(Unit)
        runCurrent()
        assertEquals(1, opened)
        capture.stop()
        runCurrent()
        assertEquals(VoiceCaptureResult.Transcript("B transcript"), delivered)
        var nextOwnerAdmitted = false
        val nextOwner = launch { admission.withLock { nextOwnerAdmitted = true } }
        runCurrent()
        assertTrue(!nextOwnerAdmitted)
        captureCleanup.complete(Unit)
        runCurrent()
        assertTrue(nextOwnerAdmitted)
        nextOwner.join()
        capture.dispose()
        playerA.dispose()
        scope.coroutineContext[Job]?.cancel()
    }

    @Test
    fun releaseOrCancelWhileWaitingForAdmissionNeverOpensTheMicrophone() = runTest {
        val admission = Mutex(locked = true)
        val scope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler))
        var opened = 0
        val results = mutableListOf<VoiceCaptureResult>()
        fun capture() = VoiceCapture(
            requestPermission = {}, hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                ServiceRealtimeListenSession(scope, callbacks, admission) { serviceCallbacks ->
                    opened++
                    FakeRealtimeSession(serviceCallbacks)
                }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        val released = capture()
        released.start { results += it }
        released.stop()
        val cancelled = capture()
        cancelled.start { results += it }
        cancelled.cancel()
        admission.unlock()
        runCurrent()
        assertEquals(0, opened)
        assertEquals(listOf(VoiceCaptureResult.Empty), results)
        released.dispose()
        cancelled.dispose()
        scope.coroutineContext[Job]?.cancel()
    }

    private class FakeRealtimeSession(
        private val callbacks: RealtimeSpeechCallbacks,
    ) : RealtimeSpeechSession {
        var stopped = false
        var cancelled = false
        var closed = false
        var finalOnStop: String? = null

        override fun stop() {
            stopped = true
            finalOnStop?.let {
                callbacks.onFinal(it)
                callbacks.onClosed()
            }
        }

        override fun cancel() {
            cancelled = true
            callbacks.onClosed()
        }

        override fun close() {
            closed = true
            callbacks.onClosed()
        }

        fun ready() = callbacks.onReady()
        fun partial(text: String) = callbacks.onPartial(text)
        fun final(text: String) = callbacks.onFinal(text)
        fun error(code: String, message: String, retriable: Boolean = false) =
            callbacks.onError(code, message, retriable)
    }

    @Test
    fun start_without_permission_requests_permission_and_sets_state() {
        var requested = false
        val capture = VoiceCapture(
            requestPermission = { requested = true },
            hasPermission = { false },
            openRealtimeSession = { _, _ -> error("should not open session") },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )

        capture.start()

        assertTrue(requested)
        assertEquals(VoiceCapturePhase.PermissionRequired, VoiceCaptureStore.state.value.phase)
    }

    @Test
    fun completedPermissionRequestClearsItsWaitWithoutResettingActiveCapture() {
        val waiting = VoiceCapture(
            requestPermission = {},
            hasPermission = { false },
            openRealtimeSession = { _, _ -> error("should not open session") },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        waiting.start()
        VoiceCaptureStore.permissionRequestCompleted()
        assertEquals(VoiceCapturePhase.Idle, VoiceCaptureStore.state.value.phase)

        lateinit var session: FakeRealtimeSession
        val active = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks -> FakeRealtimeSession(callbacks).also { session = it } },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        active.start()
        session.ready()
        session.partial("keep")
        VoiceCaptureStore.permissionRequestCompleted()
        assertEquals(VoiceCapturePhase.Listening, VoiceCaptureStore.state.value.phase)
        assertEquals("keep", VoiceCaptureStore.state.value.partialTranscript)
        active.dispose()
    }

    @Test
    fun start_partial_and_release_emit_final_transcript() {
        lateinit var session: FakeRealtimeSession
        var finalTranscript: String? = null
        var partialTranscript: String? = null
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            onPartialTranscript = { partialTranscript = it },
            openRealtimeSession = { _, callbacks ->
                FakeRealtimeSession(callbacks).also { session = it }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )

        capture.start()
        session.ready()
        session.partial("hello wor")

        assertEquals(VoiceCapturePhase.Listening, VoiceCaptureStore.state.value.phase)
        assertEquals("hello wor", VoiceCaptureStore.state.value.partialTranscript)
        assertEquals("hello wor", partialTranscript)

        capture.stop { result ->
            finalTranscript = (result as? VoiceCaptureResult.Transcript)?.text
        }
        assertTrue(session.stopped)

        session.final("hello world")

        assertEquals("hello world", finalTranscript)
        assertEquals(VoiceCapturePhase.Completed, VoiceCaptureStore.state.value.phase)
        assertEquals("hello world", VoiceCaptureStore.state.value.finalTranscript)
    }

    @Test
    fun start_with_result_keeps_session_open_until_explicit_stop() {
        lateinit var session: FakeRealtimeSession
        var finalTranscript: String? = null
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                FakeRealtimeSession(callbacks).also { session = it }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )

        capture.start { result ->
            finalTranscript = (result as? VoiceCaptureResult.Transcript)?.text
        }

        assertTrue(!session.stopped)
        assertTrue(!session.cancelled)
        assertNull(finalTranscript)

        capture.stop()
        assertTrue(session.stopped)
        session.final("flow mode transcript")

        assertEquals("flow mode transcript", finalTranscript)
    }

    @Test
    fun serviceAdapterReleaseStopsTheSameLiveRecognizerAndKeepsItsFinalTranscript() = runTest {
        lateinit var session: FakeRealtimeSession
        var finalTranscript: String? = null
        var partialTranscript: String? = null
        val scope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler))
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            onPartialTranscript = { partialTranscript = it },
            openRealtimeSession = { _, callbacks ->
                ServiceRealtimeListenSession(scope, callbacks) { serviceCallbacks ->
                    FakeRealtimeSession(serviceCallbacks).also {
                        session = it
                        it.finalOnStop = "transcript from released session"
                        it.ready()
                        it.partial("partial from service")
                    }
                }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        capture.start { result -> finalTranscript = (result as? VoiceCaptureResult.Transcript)?.text }
        runCurrent()
        assertEquals("partial from service", partialTranscript)

        capture.stop()

        assertTrue("release must stop the service-owned recognizer", session.stopped)
        assertEquals("transcript from released session", finalTranscript)
        assertEquals(VoiceCapturePhase.Completed, VoiceCaptureStore.state.value.phase)
        scope.coroutineContext[Job]?.cancel()
    }

    @Test
    fun finalTranscriptIsNotOverwrittenByALaterTerminalCallback() {
        lateinit var session: FakeRealtimeSession
        val delivered = mutableListOf<VoiceCaptureResult>()
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks -> FakeRealtimeSession(callbacks).also { session = it } },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )

        capture.start { delivered += it }
        session.final("recognized transcript")
        session.error("no_speech", "Speech recognition ended without a result.")
        session.close()

        assertEquals(listOf(VoiceCaptureResult.Transcript("recognized transcript")), delivered)
        assertEquals(VoiceCapturePhase.Completed, VoiceCaptureStore.state.value.phase)
        assertEquals("recognized transcript", VoiceCaptureStore.state.value.finalTranscript)
    }

    @Test
    fun restartAfterCancelIgnoresCallbacksFromTheOldSession() {
        val sessions = mutableListOf<FakeRealtimeSession>()
        val delivered = mutableListOf<String>()
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                FakeRealtimeSession(callbacks).also(sessions::add)
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        capture.start { delivered += (it as VoiceCaptureResult.Transcript).text }
        sessions.single().ready()
        capture.cancel()
        capture.start { delivered += (it as VoiceCaptureResult.Transcript).text }
        sessions.last().ready()
        sessions.first().partial("stale partial")
        sessions.first().final("stale final")

        assertEquals(2, sessions.size)
        assertTrue(capture.isActive())
        assertEquals(VoiceCapturePhase.Listening, VoiceCaptureStore.state.value.phase)
        assertEquals("", VoiceCaptureStore.state.value.partialTranscript)
        assertTrue("the cancelled interaction cannot deliver into its replacement", delivered.isEmpty())

        sessions.last().final("new foreground utterance")
        assertEquals(listOf("new foreground utterance"), delivered)
    }

    @Test
    fun cancel_clears_pending_result_and_marks_cancelled() {
        lateinit var session: FakeRealtimeSession
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                FakeRealtimeSession(callbacks).also { session = it }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )
        var callbackCount = 0

        capture.start()
        capture.stop { callbackCount++ }
        capture.cancel()

        assertTrue(session.cancelled)
        assertEquals(0, callbackCount)
        assertEquals(VoiceCapturePhase.Cancelled, VoiceCaptureStore.state.value.phase)
    }

    @Test
    fun dispose_cancels_session_ignores_late_result_and_resets_store() {
        lateinit var session: FakeRealtimeSession
        var callbackCount = 0
        val capture = VoiceCapture(
            requestPermission = {},
            hasPermission = { true },
            openRealtimeSession = { _, callbacks ->
                FakeRealtimeSession(callbacks).also { session = it }
            },
            transcribeOnce = { VoiceCaptureResult.Empty },
        )

        capture.start { callbackCount++ }
        session.partial("partial")
        capture.dispose()

        assertTrue(session.cancelled)
        session.final("late transcript")
        assertEquals(0, callbackCount)
        assertEquals(VoiceCapturePhase.Idle, VoiceCaptureStore.state.value.phase)
        assertEquals("", VoiceCaptureStore.state.value.partialTranscript)
        assertNull(VoiceCaptureStore.state.value.errorMessage)
    }
}
