package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import java.util.concurrent.ConcurrentHashMap
import com.lingxi.code.settings.VersionedAudioConfiguration
import com.lingxi.code.bindings.AudioOperationResultDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidAudioServiceCoreTest {
    private val epoch = 17L
    private val owner = AudioOwnerKey.session("session-a")

    @Test
    fun busySpeakReturnsImmediatelyWhileAnotherPlaybackIsActive() = runTest {
        val trace = mutableListOf<String>()
        val runtime = FakeRuntime(trace)
        val driver = FakeDriver(trace)
        val service = AndroidAudioServiceCore(runtime, driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val first = launch { service.execute(request("speak-one", DeviceAudioOperation.Speak("hello", null, null, null))) }
        runCurrent()
        driver.playStarted.await()

        val second = service.execute(request("speak-two", DeviceAudioOperation.Speak("again", null, null, null)))
        assertEquals(DeviceAudioErrorKind.Busy, (second as DeviceAudioResult.Failed).error.kind)
        assertFalse(first.isCompleted)

        val invalidation = launch { service.invalidate() }
        runCurrent()
        invalidation.join()
        first.join()
        assertTrue(trace.contains("native-stop"))
        assertTrue(trace.contains("play-settled"))
        assertFalse("interrupted audio must not report natural playback completion", trace.contains("play-natural"))
    }

    @Test
    fun invalidationWaitsForCancelledSpeechRuntimeCleanupBeforeReturning() = runTest {
        val trace = mutableListOf<String>()
        val runtime = FakeRuntime(trace).apply { blockRender = true }
        val driver = FakeDriver(trace)
        val service = AndroidAudioServiceCore(runtime, driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val speak = launch { service.execute(request("speak-blocked", DeviceAudioOperation.Speak("hello", null, null, null))) }
        runCurrent()
        runtime.renderStarted.await()

        val invalidation = launch {
            service.invalidate()
            trace += "invalidated"
        }
        runCurrent()
        runtime.renderCleanupStarted.await()
        val newAdmission = launch {
            service.execute(request("after-invalidate", DeviceAudioOperation.Status(null)))
        }
        runCurrent()
        assertFalse("invalidation waits for runtime cleanup", invalidation.isCompleted)
        assertFalse("new operations cannot cross the invalidation boundary", newAdmission.isCompleted)
        runtime.renderCleanupGate.complete(Unit)
        invalidation.join()
        newAdmission.join()
        speak.join()

        assertTrue(runtime.renderCleaned.isCompleted)
        assertTrue(trace.indexOf("render-cleanup") < trace.indexOf("invalidated"))
    }

    @Test
    fun targetedCancellationWaitsForRuntimeCleanupBeforeSettling() = runTest {
        val trace = mutableListOf<String>()
        val runtime = FakeRuntime(trace).apply { blockRender = true }
        val service = AndroidAudioServiceCore(runtime, FakeDriver(trace), maxPayloadBytes = 64, initialEpoch = epoch)
        val operationIdentity = identity("cancel-and-cleanup")
        val request = DeviceAudioRequest(
            identity = operationIdentity,
            owner = owner,
            timeoutBudgetMs = null,
            maxPayloadBytes = 64,
            operation = DeviceAudioOperation.Synthesize("hello", null, null, null),
        )
        val synthesis = launch { service.execute(request) }
        runCurrent()
        runtime.renderStarted.await()

        val cancellation = launch { service.cancel(operationIdentity) }
        runCurrent()
        runtime.renderCleanupStarted.await()
        assertFalse("cancel must join native renderer cleanup", cancellation.isCompleted)
        assertEquals(1, service.diagnostics().activeLeaseCount)

        runtime.renderCleanupGate.complete(Unit)
        cancellation.join()
        synthesis.join()

        assertTrue(runtime.renderCleaned.isCompleted)
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertEquals(0, service.diagnostics().pendingOperationCount)
    }

    @Test
    fun recordingHandleSurvivesCompletedStartAndStopUsesStableOwnerWithNewOperationIdentity() = runTest {
        val runtime = FakeRuntime()
        val driver = FakeDriver()
        driver.recordingBytes = byteArrayOf(1, 2, 3)
        val service = AndroidAudioServiceCore(runtime, driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val started = service.execute(request("start", DeviceAudioOperation.StartRecording(16_000, "audio/m4a"))) as DeviceAudioResult.RecordingStarted

        assertTrue((service.execute(request("status", DeviceAudioOperation.Status(null))) as DeviceAudioResult.Status).recording)
        assertEquals("a completed start must not retain its operation state", 0, retainedLeaseEntries(service, "leaseOperations"))
        val result = service.execute(
            request("different-stop-id", DeviceAudioOperation.StopRecording(started.handle)),
        ) as DeviceAudioResult.Recording
        assertEquals("audio/m4a", result.mimeType)
        assertEquals(listOf<Byte>(1, 2, 3), result.bytes.toList())
        assertFalse((service.execute(request("status-after-stop", DeviceAudioOperation.Status(null))) as DeviceAudioResult.Status).recording)
    }

    @Test
    fun wrongOwnerCannotStopOrObserveRecording() = runTest {
        val runtime = FakeRuntime()
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(runtime, driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val started = service.execute(request("start", DeviceAudioOperation.StartRecording(16_000, "audio/m4a"))) as DeviceAudioResult.RecordingStarted
        val otherOwner = request("wrong-owner", DeviceAudioOperation.StopRecording(started.handle), owner = AudioOwnerKey.session("session-b"))

        val result = service.execute(otherOwner) as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.NotRecording, result.error.kind)
        assertEquals(0, driver.recordingStops)
    }

    @Test
    fun endedSessionOwnerCanStartANewOperationAfterEndOwnerBoundary() = runTest {
        val service = AndroidAudioServiceCore(FakeRuntime(), FakeDriver(), maxPayloadBytes = 64, initialEpoch = epoch)
        assertEquals(DeviceAudioResult.OwnerEnded, service.execute(request("end", DeviceAudioOperation.EndOwner)))

        val status = service.execute(request("reopened", DeviceAudioOperation.Status(null)))
        assertEquals(DeviceAudioResult.Status(recording = false, playing = false), status)
    }

    @Test
    fun endOwnerWaitsUntilActivePlaybackHasStopped() = runTest {
        val trace = mutableListOf<String>()
        val driver = FakeDriver(trace).apply { blockNativeStop = true }
        val service = AndroidAudioServiceCore(FakeRuntime(trace), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val speak = launch {
            service.execute(request("speak-before-stop", DeviceAudioOperation.Speak("hello", null, null, null)))
        }
        runCurrent()
        driver.playStarted.await()

        val stopAudio = async {
            service.execute(request("stop-audio", DeviceAudioOperation.EndOwner))
        }
        runCurrent()
        driver.nativeStopStarted.await()
        assertFalse("EndOwner must wait for the native stop to settle", stopAudio.isCompleted)

        driver.nativeStopGate.complete(Unit)
        assertEquals(DeviceAudioResult.OwnerEnded, stopAudio.await())
        speak.join()

        assertTrue(trace.contains("play-settled"))
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "leaseOperations"))
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
    }

    @Test
    fun failedNativeStopCanBeRetriedByEndOwner() = runTest {
        val driver = FakeDriver().apply { failNativeStops = 1 }
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        service.execute(request("start-for-retry", DeviceAudioOperation.StartRecording(16_000, "audio/m4a")))

        val failed = service.execute(request("end-fails-once", DeviceAudioOperation.EndOwner)) as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.NativeFailure, failed.error.kind)
        assertEquals(1, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))

        assertEquals(DeviceAudioResult.OwnerEnded, service.execute(request("end-retries", DeviceAudioOperation.EndOwner)))
        assertEquals(2, driver.nativeStopAttempts)
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "leaseOperations"))
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
    }

    @Test
    fun failedRecordingReleaseKeepsTheHandleAvailableForRetry() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val started = service.execute(request("record-before-stop-failure", DeviceAudioOperation.StartRecording(16_000, "audio/m4a")))
            as DeviceAudioResult.RecordingStarted
        driver.failNativeStops = 1

        val failed = service.execute(request("record-stop-fails", DeviceAudioOperation.StopRecording(started.handle)))
            as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.NativeFailure, failed.error.kind)
        assertEquals(1, service.diagnostics().activeLeaseCount)
        assertEquals(
            DeviceAudioResult.Status(recording = true, playing = false),
            service.execute(request("record-status-after-stop-failure", DeviceAudioOperation.Status(started.handle))),
        )

        assertTrue(service.execute(request("record-stop-retries", DeviceAudioOperation.StopRecording(started.handle))) is DeviceAudioResult.Recording)
        assertEquals(1, driver.recordingStops)
        assertEquals(0, service.diagnostics().activeLeaseCount)
        val duplicate = service.execute(request("record-stop-duplicate", DeviceAudioOperation.StopRecording(started.handle))) as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.NotRecording, duplicate.error.kind)
    }

    @Test
    fun failedMediaReleaseKeepsTheSessionAvailableForRetry() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        assertTrue(service.execute(request("media-before-stop-failure", DeviceAudioOperation.OffloadMediaPlay("music", "/music/a.mp3"))) is DeviceAudioResult.OffloadMedia)
        driver.failNativeStops = 1

        val failed = service.execute(request("media-stop-fails", DeviceAudioOperation.OffloadMediaControl("music", OffloadMediaCommand.STOP)))
            as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.NativeFailure, failed.error.kind)
        assertEquals(1, service.diagnostics().activeLeaseCount)

        assertTrue(service.execute(request("media-stop-retries", DeviceAudioOperation.OffloadMediaControl("music", OffloadMediaCommand.STOP))) is DeviceAudioResult.OffloadMedia)
        assertEquals(2, driver.nativeStopAttempts)
        assertEquals(0, service.diagnostics().activeLeaseCount)
    }

    @Test
    fun failedInvalidationRestoresAdmissionAndCanRetryNativeStop() = runTest {
        val driver = FakeDriver().apply { failNativeStops = 1 }
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        service.execute(request("start-before-failed-invalidate", DeviceAudioOperation.StartRecording(16_000, "audio/m4a")))

        val error = runCatching { service.invalidate() }.exceptionOrNull()
        assertTrue(error is AudioOperationException)
        assertEquals(1, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
        assertEquals(
            DeviceAudioResult.Status(recording = true, playing = false),
            service.execute(request("status-after-failed-invalidate", DeviceAudioOperation.Status(null))),
        )

        assertEquals(epoch + 1L, service.invalidate())
        assertEquals(2, driver.nativeStopAttempts)
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "leaseOperations"))
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
    }

    @Test
    fun latePermissionGrantCannotStartCaptureAfterOperationCancellation() = runTest {
        val runtime = FakeRuntime()
        val driver = FakeDriver().apply { waitForPermissionGrant = true }
        val service = AndroidAudioServiceCore(runtime, driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val identity = identity("late-start")
        val starting = launch { service.execute(request(identity.id, DeviceAudioOperation.StartRecording(16_000, "audio/m4a"))) }
        runCurrent()
        driver.permissionWaitStarted.await()

        val cancel = launch { service.cancel(identity) }
        runCurrent()
        cancel.join()
        assertTrue(cancel.isCompleted)
        driver.deliverLatePermissionGrant()
        starting.join()

        assertFalse(driver.captureStarted)
        assertTrue(driver.events.contains("late-start-rejected"))
    }

    @Test
    fun cancellationBeforeExecuteRegistrationRejectsOnlyThatIdentity() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val cancelledIdentity = AudioOperationIdentity("cancelled-before-register", generation = 71L, serviceEpoch = epoch)

        service.cancel(cancelledIdentity)
        val rejected = service.execute(
            DeviceAudioRequest(
                identity = cancelledIdentity,
                owner = owner,
                timeoutBudgetMs = null,
                maxPayloadBytes = 64,
                operation = DeviceAudioOperation.StartRecording(16_000, "audio/m4a"),
            ),
        ) as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.Cancelled, rejected.error.kind)
        assertFalse(driver.captureStarted)

        val freshIdentity = AudioOperationIdentity("fresh-after-cancel", generation = 0L, serviceEpoch = epoch)
        val started = service.execute(
            DeviceAudioRequest(
                identity = freshIdentity,
                owner = owner,
                timeoutBudgetMs = null,
                maxPayloadBytes = 64,
                operation = DeviceAudioOperation.StartRecording(16_000, "audio/m4a"),
            ),
        ) as DeviceAudioResult.RecordingStarted
        assertTrue(driver.captureStarted)
        service.execute(request("stop-fresh", DeviceAudioOperation.StopRecording(started.handle)))
    }

    @Test
    fun realtimeListenReleaseStopsItsRegisteredSessionAndSettlesCaptureLease() = runTest {
        val trace = mutableListOf<String>()
        val runtime = FakeRuntime(trace)
        val service = AndroidAudioServiceCore(runtime, FakeDriver(trace), maxPayloadBytes = 64, initialEpoch = epoch)
        var partial: String? = null
        var transcript: String? = null
        var terminalError: String? = null
        val callbacks = object : RealtimeSpeechCallbacks {
            override fun onPartial(text: String) { partial = text }
            override fun onFinal(text: String) { transcript = text }
            override fun onError(code: String, message: String, retriable: Boolean) { terminalError = code }
        }

        val session = service.openRealtimeListen(
            request("live-listen", DeviceAudioOperation.Listen(null)),
            callbacks,
        )
        assertEquals("partial", partial)
        assertEquals(DeviceAudioReadiness.BUSY, service.capabilities().readiness[DeviceAudioOperationKind.LISTEN])

        session.stop()
        withContext(Dispatchers.IO) {
            val deadline = System.nanoTime() + 2_000_000_000L
            while (service.diagnostics().activeLeaseCount != 0 && System.nanoTime() < deadline) {
                Thread.sleep(1)
            }
        }

        assertEquals("recognized after release", transcript)
        assertEquals("a delivered final transcript must not be followed by a false no-speech error", null, terminalError)
        assertTrue("recognizer teardown must precede driver release: $trace", trace.indexOf("recognizer-stop") < trace.indexOf("native-stop"))
        assertEquals(DeviceAudioReadiness.READY, service.capabilities().readiness[DeviceAudioOperationKind.LISTEN])
    }

    @Test
    fun recognizerBusyIsReportedAsAudioUnavailable() = runTest {
        val runtime = FakeRuntime(systemListenErrorCode = "audio_io_unavailable")
        val service = AndroidAudioServiceCore(runtime, FakeDriver(), maxPayloadBytes = 64, initialEpoch = epoch)

        val result = service.execute(request("recognizer-busy", DeviceAudioOperation.Listen(null)))

        assertEquals(
            DeviceAudioResult.Failed(DeviceAudioError(DeviceAudioErrorKind.Unavailable, "recognizer unavailable")),
            result,
        )
    }

    @Test
    fun diagnosticsRecordPinnedRevisionAndEffectiveAutomaticFallbackWithoutPayloads() = runTest {
        val runtime = FakeRuntime(snapshotRevision = 23L, failSystemRender = true)
        val service = AndroidAudioServiceCore(runtime, FakeDriver(), maxPayloadBytes = 64, initialEpoch = epoch)

        val result = service.execute(request("fallback-synthesis", DeviceAudioOperation.Synthesize("private text", null, null, null)))

        assertTrue(result is DeviceAudioResult.Synthesized)
        val diagnostics = service.diagnostics()
        assertEquals(0, diagnostics.activeLeaseCount)
        assertEquals(0, diagnostics.pendingOperationCount)
        val settled = diagnostics.recentOperations.last()
        assertEquals("synthesize", settled.operationKind)
        assertEquals("settled", settled.phase)
        assertEquals(23L, settled.configurationRevision)
        assertEquals("automatic", settled.requestedSource)
        assertEquals("offline", settled.effectiveSource)
        assertEquals("systemUnavailable", settled.fallbackReason)
    }

    @Test
    fun automaticRecognitionFallsBackOnlyForSystemUnavailableBeforeCapture() = runTest {
        val runtime = FakeRuntime(snapshotRevision = 29L, failSystemListen = true)
        val service = AndroidAudioServiceCore(runtime, FakeDriver(), maxPayloadBytes = 64, initialEpoch = epoch)

        val result = service.execute(request("recognition-fallback", DeviceAudioOperation.Listen(null)))

        assertEquals(DeviceAudioResult.Transcript("offline transcript", null, null), result)
        val settled = service.diagnostics().recentOperations.last()
        assertEquals(29L, settled.configurationRevision)
        assertEquals("automatic", settled.requestedSource)
        assertEquals("offline", settled.effectiveSource)
        assertEquals("systemUnavailable", settled.fallbackReason)
    }

    @Test
    fun synthesizedFfiResultPreservesRuntimeRendererSampleRate() = runTest {
        val service = AndroidAudioServiceCore(
            runtime = FakeRuntime(renderSampleRateHz = 16_000),
            driver = FakeDriver(),
            maxPayloadBytes = 64,
            initialEpoch = epoch,
        )

        val result = service.execute(request("system-tts-actual-rate", DeviceAudioOperation.Synthesize("hello", null, null, null)))
        val ffi = AndroidAudioResultDtoMapper.toDto(result, maxPayloadBytes = 64)

        assertTrue(ffi is AudioOperationResultDto.Synthesized)
        assertEquals(16_000u, (ffi as AudioOperationResultDto.Synthesized).sampleRateHz)
    }

    @Test
    fun offloadMediaUsesPlaybackLeaseAndMakesCaptureBusyUntilStop() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)

        val started = service.execute(
            request("media-start", DeviceAudioOperation.OffloadMediaPlay("music", "content://media/song")),
        ) as DeviceAudioResult.OffloadMedia
        assertTrue(started.state.playing)
        assertEquals("content://media/song", driver.mediaTargets.values.single())
        assertEquals(1, service.diagnostics().activeLeaseCount)

        val capture = service.execute(
            request("media-capture", DeviceAudioOperation.StartRecording(16_000, "audio/m4a")),
        ) as DeviceAudioResult.Failed
        assertEquals(DeviceAudioErrorKind.Busy, capture.error.kind)

        val paused = service.execute(
            request("media-pause", DeviceAudioOperation.OffloadMediaControl("music", OffloadMediaCommand.PAUSE)),
        ) as DeviceAudioResult.OffloadMedia
        assertFalse(paused.state.playing)
        val pausedStatus = service.execute(request("media-status-paused", DeviceAudioOperation.Status(null))) as DeviceAudioResult.Status
        assertFalse(pausedStatus.playing)
        assertEquals(DeviceAudioReadiness.BUSY, service.capabilities().readiness[DeviceAudioOperationKind.RECORD])

        service.execute(request("media-stop", DeviceAudioOperation.OffloadMediaControl("music", OffloadMediaCommand.STOP)))
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertTrue(driver.events.any { it.startsWith("media-stop:") })
    }

    @Test
    fun offloadMediaLabelsAreScopedToSessionOwnerAndEndOwnerReleasesNativePlayer() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        val otherOwner = AudioOwnerKey.session("session-b")
        service.execute(request("owner-a-play", DeviceAudioOperation.OffloadMediaPlay("shared-label", "/music/a.mp3")))
        val activeLease = service.diagnostics().activeLeaseCount

        val wrongOwnerStop = service.execute(
            request(
                "owner-b-stop",
                DeviceAudioOperation.OffloadMediaControl("shared-label", OffloadMediaCommand.STOP),
                owner = otherOwner,
            ),
        ) as DeviceAudioResult.OffloadMedia
        assertFalse(wrongOwnerStop.state.playing)
        assertEquals("a different session cannot stop or release this owner’s player", activeLease, service.diagnostics().activeLeaseCount)
        assertTrue(driver.events.none { it.startsWith("media-stop:") })

        assertEquals(DeviceAudioResult.OwnerEnded, service.execute(request("owner-b-end", DeviceAudioOperation.EndOwner, owner = otherOwner)))
        assertEquals(1, service.diagnostics().activeLeaseCount)
        service.execute(request("owner-a-end", DeviceAudioOperation.EndOwner))
        assertEquals(0, service.diagnostics().activeLeaseCount)
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
        assertTrue(driver.events.any { it.startsWith("media-stop:") })

        val reopened = service.execute(
            request("owner-a-reopen", DeviceAudioOperation.OffloadMediaPlay("shared-label", "/music/b.mp3")),
        )
        assertTrue("EndOwner allows the stable session owner to reopen", reopened is DeviceAudioResult.OffloadMedia)
    }

    @Test
    fun foregroundSpeechPreemptionDoesNotRetainStoppedPlaybackLedger() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        service.execute(request("media-before-preemption", DeviceAudioOperation.OffloadMediaPlay("music", "/music/a.mp3")))

        val speech = launch {
            service.execute(
                request(
                    "foreground-speech",
                    DeviceAudioOperation.Speak("hello", null, null, null, foregroundUserInitiated = true),
                ),
            )
        }
        runCurrent()
        driver.playStarted.await()

        assertEquals(1, service.diagnostics().activeLeaseCount)
        assertEquals("preempted lease completion must be forgotten", 0, retainedLeaseEntries(service, "nativeStopCompletion"))
        assertEquals("preempted media session must be forgotten", 0, retainedLeaseEntries(service, "offloadMedia"))
        service.invalidate()
        speech.join()
        assertEquals(0, retainedLeaseEntries(service, "leaseOperations"))
        assertEquals(0, retainedLeaseEntries(service, "nativeStopCompletion"))
    }

    @Test
    fun staleMediaCompletionCannotReleaseReplacementSessionLease() = runTest {
        val driver = FakeDriver()
        val service = AndroidAudioServiceCore(FakeRuntime(), driver, maxPayloadBytes = 64, initialEpoch = epoch)
        service.execute(request("old-media", DeviceAudioOperation.OffloadMediaPlay("same", "/music/old.mp3")))
        val oldLeaseId = driver.mediaCompletions.keys.single()
        service.execute(request("stop-old-media", DeviceAudioOperation.OffloadMediaControl("same", OffloadMediaCommand.STOP)))

        service.execute(request("new-media", DeviceAudioOperation.OffloadMediaPlay("same", "/music/new.mp3")))
        val newLeaseId = driver.mediaCompletions.keys.maxOrNull()!!
        assertTrue(newLeaseId != oldLeaseId)
        assertTrue(driver.completeMedia(oldLeaseId))
        awaitMediaCallbacks(service)

        assertEquals(1, service.diagnostics().activeLeaseCount)
        val status = service.execute(
            request("status-new-media", DeviceAudioOperation.OffloadMediaControl("same", OffloadMediaCommand.STATUS)),
        ) as DeviceAudioResult.OffloadMedia
        assertTrue(status.state.playing)

        assertTrue(driver.completeMedia(newLeaseId))
        awaitMediaCallbacks(service)
        assertEquals(0, service.diagnostics().activeLeaseCount)
    }

    private suspend fun awaitMediaCallbacks(service: AndroidAudioServiceCore) = withContext(Dispatchers.IO) {
        val deadline = System.nanoTime() + 2_000_000_000L
        while (service.diagnostics().pendingOperationCount != 0 && System.nanoTime() < deadline) {
            Thread.sleep(1)
        }
    }

    private fun retainedLeaseEntries(service: AndroidAudioServiceCore, fieldName: String): Int {
        val field = AndroidAudioServiceCore::class.java.getDeclaredField(fieldName)
        field.isAccessible = true
        return (field.get(service) as Map<*, *>).size
    }

    private fun request(
        id: String,
        operation: DeviceAudioOperation,
        owner: AudioOwnerKey = this.owner,
    ) = DeviceAudioRequest(identity(id), owner, timeoutBudgetMs = null, maxPayloadBytes = 64, operation = operation)

    private fun identity(id: String) = AudioOperationIdentity(id, generation = 1, serviceEpoch = epoch)

    private class FakeRuntime(
        private val events: MutableList<String> = mutableListOf(),
        private val snapshotRevision: Long = 0L,
        private val failSystemRender: Boolean = false,
        private val failSystemListen: Boolean = false,
        private val renderSampleRateHz: Int = 24_000,
        private val systemListenErrorCode: String? = null,
    ) : AudioServiceSpeechRuntime {
        val renderStarted = CompletableDeferred<Unit>()
        val renderCleaned = CompletableDeferred<Unit>()
        val renderCleanupStarted = CompletableDeferred<Unit>()
        val renderCleanupGate = CompletableDeferred<Unit>()
        var blockRender = false

        override fun configuration() = AudioConfigurationNormalizer.defaults
        override fun configurationSnapshot() = VersionedAudioConfiguration(configuration(), snapshotRevision)
        override fun microphonePermissionGranted() = true
        override fun resolveRecognition(
            configuration: AudioConfigurationV3,
            language: String?,
            systemStatusOverride: AudioReadiness?,
        ): AudioRouteResolution {
            val requested = RequestedAudioRoute(configuration.recognition.source, configuration.recognition.offlineModelId, null)
            if (configuration.recognition.source == AudioSource.AUTOMATIC && systemStatusOverride == AudioReadiness.UNAVAILABLE) {
                return AudioRouteResolution(
                    requested = requested,
                    effective = EffectiveAudioRoute(AudioSource.OFFLINE, "offline-stt", null),
                    status = AudioRouteStatus.READY,
                    reason = "ready",
                    fallbackReason = "systemUnavailable",
                )
            }
            return AudioRouteResolution(
                requested = requested,
                effective = EffectiveAudioRoute(AudioSource.SYSTEM, null, null),
                status = AudioRouteStatus.READY,
                reason = "ready",
            )
        }
        override fun openRealtimeSession(
            language: String?,
            configuration: AudioConfigurationV3,
            callbacks: RealtimeSpeechCallbacks,
        ): RealtimeSpeechSession {
            callbacks.onReady()
            callbacks.onPartial("partial")
            return object : RealtimeSpeechSession {
                private var closed = false
                override fun stop() {
                    if (closed) return
                    events += "recognizer-stop"
                    callbacks.onFinal("recognized after release")
                    callbacks.onClosed()
                    closed = true
                }
                override fun cancel() {
                    if (closed) return
                    events += "recognizer-cancel"
                    callbacks.onClosed()
                    closed = true
                }
                override fun close() = cancel()
            }
        }
        override fun resolveSpeech(
            configuration: AudioConfigurationV3,
            language: String?,
            voice: String?,
            rate: Float?,
            systemStatusOverride: AudioReadiness?,
        ): AudioRouteResolution {
            val requested = RequestedAudioRoute(configuration.speech.source, configuration.speech.offlineModelId, configuration.speech.voice)
            if (configuration.speech.source == AudioSource.AUTOMATIC && systemStatusOverride == AudioReadiness.UNAVAILABLE) {
                return AudioRouteResolution(
                    requested = requested,
                    effective = EffectiveAudioRoute(AudioSource.OFFLINE, "offline-tts", null),
                    status = AudioRouteStatus.READY,
                    reason = "ready",
                    fallbackReason = "systemUnavailable",
                )
            }
            return AudioRouteResolution(
                requested = requested,
                effective = EffectiveAudioRoute(AudioSource.SYSTEM, null, "default"),
                status = AudioRouteStatus.READY,
                reason = "ready",
            )
        }

        override suspend fun transcribe(language: String?, configuration: AudioConfigurationV3) =
            if (systemListenErrorCode != null) {
                SttResult.Err(systemListenErrorCode, "recognizer unavailable", true)
            } else if (failSystemListen && configuration.recognition.source == AudioSource.AUTOMATIC) {
                SttResult.Err("no_provider_configured", "system STT service disappeared before capture", false)
            } else {
                SttResult.Ok("offline transcript", language, null)
            }

        override suspend fun render(
            text: String,
            language: String?,
            voice: String?,
            rate: Float?,
            maxPcmBytes: Int,
            configuration: AudioConfigurationV3,
        ): Pair<ByteArray, Int> {
            renderStarted.complete(Unit)
            return try {
                if (blockRender) awaitCancellation()
                if (failSystemRender && configuration.speech.source == AudioSource.AUTOMATIC) {
                    throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "system renderer unavailable")
                }
                byteArrayOf(1, 0, 2, 0) to renderSampleRateHz
            } finally {
                withContext(NonCancellable) {
                    renderCleanupStarted.complete(Unit)
                    if (blockRender) renderCleanupGate.await()
                    events += "render-cleanup"
                    renderCleaned.complete(Unit)
                }
            }
        }

        private fun readyRoute(@Suppress("UNUSED_PARAMETER") kind: AudioProviderKind) = AudioRouteResolution(
            requested = RequestedAudioRoute(AudioSource.SYSTEM, null, null),
            effective = EffectiveAudioRoute(AudioSource.SYSTEM, null, "default"),
            status = AudioRouteStatus.READY,
            reason = "ready",
        )
    }

    private class FakeDriver(val events: MutableList<String> = mutableListOf()) : AndroidAudioDeviceDriver {
        val playStarted = CompletableDeferred<Unit>()
        val permissionWaitStarted = CompletableDeferred<Unit>()
        val nativeStopStarted = CompletableDeferred<Unit>()
        val nativeStopGate = CompletableDeferred<Unit>()
        var waitForPermissionGrant = false
        var blockNativeStop = false
        var failNativeStops = 0
        var nativeStopAttempts = 0
        var captureStarted = false
        var recordingStops = 0
        var recordingBytes = byteArrayOf(9, 8)
        private var playbackStop: CompletableDeferred<Unit>? = null
        private var mayStartAfterPermission: (() -> Boolean)? = null
        val mediaCompletions = ConcurrentHashMap<Long, () -> Unit>()
        private val mediaStates = ConcurrentHashMap<Long, DeviceMediaPlaybackState>()
        val mediaTargets = ConcurrentHashMap<Long, String>()

        override suspend fun startRecording(
            lease: AudioLease,
            sampleRateHz: Int,
            format: String,
            maxPayloadBytes: Int,
            mayStart: () -> Boolean,
        ): String {
            if (waitForPermissionGrant) {
                permissionWaitStarted.complete(Unit)
                mayStartAfterPermission = mayStart
                awaitCancellation()
            }
            if (!mayStart()) {
                events += "late-start-rejected"
                throw kotlinx.coroutines.CancellationException("stale microphone start")
            }
            captureStarted = true
            return "handle-${lease.leaseId}"
        }

        override suspend fun stopRecording(lease: AudioLease, handle: String, maxPayloadBytes: Int): DeviceAudioCapture {
            recordingStops += 1
            return DeviceAudioCapture(recordingBytes, "audio/m4a")
        }

        override suspend fun play(lease: AudioLease, pcm: ByteArray, sampleRateHz: Int): Long {
            playStarted.complete(Unit)
            val stopped = CompletableDeferred<Unit>()
            playbackStop = stopped
            try {
                stopped.await()
                throw kotlinx.coroutines.CancellationException("native playback stopped")
            } finally {
                events += "play-settled"
            }
        }

        override suspend fun playMedia(
            lease: AudioLease,
            target: String,
            mayStart: () -> Boolean,
            onCompleted: () -> Unit,
        ): DeviceMediaPlaybackState {
            if (!mayStart()) throw kotlinx.coroutines.CancellationException("stale media start")
            mediaTargets[lease.leaseId] = target
            mediaStates[lease.leaseId] = DeviceMediaPlaybackState(true, positionMs = 0, durationMs = 321)
            mediaCompletions[lease.leaseId] = onCompleted
            events += "media-start:${lease.leaseId}"
            return mediaStates.getValue(lease.leaseId)
        }

        override suspend fun controlMedia(lease: AudioLease, command: OffloadMediaCommand): DeviceMediaPlaybackState {
            val current = mediaStates[lease.leaseId]
                ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NotRecording, "media playback is unavailable"))
            val updated = when (command) {
                OffloadMediaCommand.PAUSE -> current.copy(playing = false)
                OffloadMediaCommand.RESUME -> current.copy(playing = true)
                OffloadMediaCommand.STATUS -> current
                OffloadMediaCommand.STOP -> current.copy(playing = false)
            }
            mediaStates[lease.leaseId] = updated
            return updated
        }

        override suspend fun stop(lease: AudioLease) {
            events += "native-stop"
            nativeStopAttempts += 1
            if (failNativeStops > 0) {
                failNativeStops -= 1
                throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, "native stop failed"))
            }
            playbackStop?.complete(Unit)
            if (blockNativeStop) {
                nativeStopStarted.complete(Unit)
                withContext(NonCancellable) { nativeStopGate.await() }
            }
            if (mediaStates.remove(lease.leaseId) != null) events += "media-stop:${lease.leaseId}"
        }

        fun deliverLatePermissionGrant() {
            if (mayStartAfterPermission?.invoke() == true) captureStarted = true
            else events += "late-start-rejected"
        }

        fun completeMedia(leaseId: Long): Boolean = mediaCompletions[leaseId]?.let { callback ->
            callback()
            true
        } ?: false
    }
}
