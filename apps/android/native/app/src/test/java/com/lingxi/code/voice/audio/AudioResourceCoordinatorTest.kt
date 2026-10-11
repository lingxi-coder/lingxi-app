package com.lingxi.code.voice.audio

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

class AudioResourceCoordinatorTest {
    private val epoch = 7L

    private fun operation(id: String, generation: Long = 1) =
        AudioOperationIdentity(id, generation, epoch)

    @Test
    fun recordedRecognitionAndLiveCaptureCannotUseTheSameOfflineRecognizerConcurrently() {
        val coordinator = AudioResourceCoordinator(epoch)
        val owner = AudioOwnerKey.ui("audio")
        val decode = coordinator.acquire(operation("decode"), owner, AudioResource.OfflineRender, modelId = "recognizer") as AudioLeaseDecision.Granted
        assertEquals(AudioLeaseDecision.Busy, coordinator.acquire(operation("live"), owner, AudioResource.Capture, modelId = "recognizer"))
        coordinator.release(decode.lease)
        coordinator.acquire(operation("capture"), owner, AudioResource.Capture, modelId = "recognizer")
        assertEquals(AudioLeaseDecision.Busy, coordinator.acquire(operation("decode-again"), owner, AudioResource.OfflineRender, modelId = "recognizer"))
    }

    @Test
    fun foregroundUserCapturePreemptsPlaybackOnlyAfterNativeStop_andLateReleaseCannotClearCapture() = runTest {
        val coordinator = AudioResourceCoordinator(epoch)
        val playback = coordinator.acquire(
            identity = operation("playback"),
            owner = AudioOwnerKey.ui("screen-a"),
            resource = AudioResource.Playback,
        ) as AudioLeaseDecision.Granted
        val stopped = mutableListOf<AudioLease>()
        val admission = AudioLeaseAdmission(coordinator) { stopped += it }

        val capture = admission.acquire(
            identity = operation("capture"),
            owner = AudioOwnerKey.ui("screen-a"),
            resource = AudioResource.Capture,
            foregroundUserInitiated = true,
        ) as AudioLeaseDecision.Granted

        assertEquals(listOf(playback.lease), stopped)
        assertFalse(coordinator.isActive(playback.lease))
        assertTrue(coordinator.isActive(capture.lease))
        assertFalse(coordinator.release(playback.lease))
        assertTrue("the stale playback completion cannot release capture", coordinator.isActive(capture.lease))
    }

    @Test
    fun replacementAdmissionWaitsForNativeStopToSettle() = runTest {
        val coordinator = AudioResourceCoordinator(epoch)
        val playback = coordinator.acquire(
            operation("playback"), AudioOwnerKey.system("autoplay"), AudioResource.Playback,
        ) as AudioLeaseDecision.Granted
        val stopStarted = CompletableDeferred<Unit>()
        val finishStop = CompletableDeferred<Unit>()
        val events = mutableListOf<String>()
        val admission = AudioLeaseAdmission(coordinator) { lease ->
            if (lease == playback.lease) {
                events += "native-stop-started"
                stopStarted.complete(Unit)
                finishStop.await()
                events += "native-stop-settled"
            }
        }

        val captureRequest = launch {
            val lease = admission.acquire(
                operation("capture"), AudioOwnerKey.ui("screen-a"), AudioResource.Capture,
                foregroundUserInitiated = true,
            ) as AudioLeaseDecision.Granted
            assertTrue(coordinator.isActive(lease.lease))
            events += "capture-admitted"
        }
        runCurrent()
        stopStarted.await()

        assertTrue("the old lease remains held until native stop completes", coordinator.isActive(playback.lease))
        assertEquals(1, coordinator.allLeases().size)
        assertFalse(captureRequest.isCompleted)

        finishStop.complete(Unit)
        captureRequest.join()

        assertEquals(listOf("native-stop-started", "native-stop-settled", "capture-admitted"), events)
    }

    @Test
    fun cancellationDuringPreemptionStopsOldDeviceAndDoesNotInstallReplacement() = runTest {
        val coordinator = AudioResourceCoordinator(epoch)
        val playback = coordinator.acquire(
            operation("playback"), AudioOwnerKey.system("autoplay"), AudioResource.Playback,
        ) as AudioLeaseDecision.Granted
        val stopStarted = CompletableDeferred<Unit>()
        val finishStop = CompletableDeferred<Unit>()
        val admission = AudioLeaseAdmission(coordinator) { lease ->
            if (lease == playback.lease) {
                stopStarted.complete(Unit)
                finishStop.await()
            }
        }

        val captureRequest = launch {
            admission.acquire(
                operation("capture"), AudioOwnerKey.ui("screen-a"), AudioResource.Capture,
                foregroundUserInitiated = true,
            )
        }
        runCurrent()
        stopStarted.await()
        captureRequest.cancel()
        finishStop.complete(Unit)
        captureRequest.join()

        assertFalse("old I/O has fully stopped before its lease is released", coordinator.isActive(playback.lease))
        assertTrue("a cancelled replacement has no live capture reservation", coordinator.allLeases().isEmpty())
    }

    @Test
    fun unsolicitedCaptureAndPlaybackReturnBusyWithoutPreemptingCurrentOwner() {
        val coordinator = AudioResourceCoordinator(epoch)
        val playback = coordinator.acquire(
            identity = operation("autoplay"),
            owner = AudioOwnerKey.system("reply-autoplay"),
            resource = AudioResource.Playback,
        ) as AudioLeaseDecision.Granted

        val capture = coordinator.acquire(
            identity = operation("tool-listen"),
            owner = AudioOwnerKey.session("session-1"),
            resource = AudioResource.Capture,
        )

        assertTrue(capture is AudioLeaseDecision.Busy)
        assertTrue(coordinator.isActive(playback.lease))
    }

    @Test
    fun systemRenderConflictsWithCaptureAndPlayback_butOfflineRenderIsIndependent() {
        val coordinator = AudioResourceCoordinator(epoch)
        val capture = coordinator.acquire(
            identity = operation("capture"),
            owner = AudioOwnerKey.ui("screen-a"),
            resource = AudioResource.Capture,
        ) as AudioLeaseDecision.Granted

        assertTrue(
            coordinator.acquire(
                operation("system-render"),
                AudioOwnerKey.session("session-1"),
                AudioResource.SystemRender,
            ) is AudioLeaseDecision.Busy,
        )
        val offline = coordinator.acquire(
            identity = operation("offline-render"),
            owner = AudioOwnerKey.session("session-1"),
            resource = AudioResource.OfflineRender,
            modelId = "tts-en-a",
        ) as AudioLeaseDecision.Granted

        assertTrue(coordinator.isActive(capture.lease))
        assertTrue(coordinator.isActive(offline.lease))
        assertTrue(
            coordinator.acquire(
                operation("same-model-render"),
                AudioOwnerKey.ui("preview"),
                AudioResource.OfflineRender,
                modelId = "tts-en-a",
            ) is AudioLeaseDecision.Busy,
        )
        assertTrue(
            coordinator.acquire(
                operation("different-model-render"),
                AudioOwnerKey.ui("preview"),
                AudioResource.OfflineRender,
                modelId = "tts-en-b",
            ) is AudioLeaseDecision.Granted,
        )
    }

    @Test
    fun onlySameOwnerFlowInteractionMayHoldCaptureAndPlaybackTogether() {
        val coordinator = AudioResourceCoordinator(epoch)
        val owner = AudioOwnerKey.ui("flow-instance")
        val capture = coordinator.acquire(
            identity = operation("flow-listen"),
            owner = owner,
            resource = AudioResource.Capture,
            flowDuplex = true,
        ) as AudioLeaseDecision.Granted

        val flowPlayback = coordinator.acquire(
            identity = operation("flow-speak"),
            owner = owner,
            resource = AudioResource.Playback,
            flowDuplex = true,
        )
        val unrelatedPlayback = coordinator.acquire(
            identity = operation("autoplay"),
            owner = AudioOwnerKey.system("reply-autoplay"),
            resource = AudioResource.Playback,
            flowDuplex = true,
        )

        assertTrue(flowPlayback is AudioLeaseDecision.Granted)
        assertTrue(unrelatedPlayback is AudioLeaseDecision.Busy)
        assertTrue(coordinator.isActive(capture.lease))
    }

    @Test
    fun recordingHandleIsBoundToStableOwnerAndServiceEpoch() {
        val recordings = AudioRecordingRegistry()
        val stableSessionOwner = AudioOwnerKey.session("session-1")
        recordings.register("recording-1", stableSessionOwner, epoch)

        assertTrue(recordings.lookup("recording-1", AudioOwnerKey.session("session-1"), epoch) is RecordingLookup.Found)
        assertTrue(recordings.lookup("recording-1", AudioOwnerKey.session("session-2"), epoch) is RecordingLookup.WrongOwner)
        for (otherOwner in listOf(AudioOwnerKey.ui("session-1"), AudioOwnerKey.system("session-1"))) {
            assertTrue(recordings.lookup("recording-1", otherOwner, epoch) is RecordingLookup.WrongOwner)
            assertTrue("owner kind isolates recordings even when IDs match", recordings.endOwner(otherOwner, epoch).isEmpty())
        }
        assertTrue(recordings.lookup("recording-1", stableSessionOwner, epoch + 1) is RecordingLookup.StaleEpoch)

        val ended = recordings.endOwner(AudioOwnerKey.session("session-2"), epoch)
        assertTrue("an unrelated owner cannot end this recording", ended.isEmpty())
        assertTrue(recordings.lookup("recording-1", stableSessionOwner, epoch) is RecordingLookup.Found)
        assertEquals(listOf("recording-1"), recordings.endOwner(stableSessionOwner, epoch))
        assertTrue(recordings.lookup("recording-1", stableSessionOwner, epoch) is RecordingLookup.NotFound)
    }

    @Test
    fun staleOperationIdentityCannotReleaseALeaseAfterServiceEpochChanges() {
        val coordinator = AudioResourceCoordinator(epoch)
        val old = coordinator.acquire(
            operation("old"), AudioOwnerKey.ui("screen"), AudioResource.Playback,
        ) as AudioLeaseDecision.Granted

        coordinator.invalidate(newEpoch = epoch + 1)
        val current = coordinator.acquire(
            AudioOperationIdentity("new", 1, epoch + 1),
            AudioOwnerKey.ui("screen"),
            AudioResource.Capture,
        ) as AudioLeaseDecision.Granted

        assertFalse(coordinator.release(old.lease))
        assertTrue(coordinator.isActive(current.lease))
    }
}
