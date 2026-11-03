package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceSpeechPlayerTest {
    @Test
    fun yieldingAutomaticPlaybackDoesNotCancelExplicitSpeech() = runTest {
        val completion = CompletableDeferred<Unit>()
        var finished = false
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> { completion.await(); finished = true; DeviceAudioResult.PlaybackCompleted(1) }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("explicit", foregroundUserInitiated = true)
        runCurrent()
        player.yieldAutomaticPlayback()
        completion.complete(Unit)
        runCurrent()
        assertTrue(finished)
        player.dispose()
    }

    @Test
    fun explicitStopDoesNotReportCancellationAsPlaybackError() = runTest {
        val gate = CompletableDeferred<Unit>()
        val errors = mutableListOf<DeviceAudioError>()
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> try {
                gate.await()
                DeviceAudioResult.PlaybackCompleted(1)
            } catch (cancelled: CancellationException) {
                DeviceAudioResult.Failed(DeviceAudioError(DeviceAudioErrorKind.Cancelled, "stopped"))
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("playing", onError = errors::add)
        runCurrent()
        player.stop()
        runCurrent()
        assertTrue(errors.isEmpty())
        player.dispose()
    }

    @Test
    fun explicitStopDoesNotReportLateDriverException() = runTest {
        val gate = CompletableDeferred<Unit>()
        val errors = mutableListOf<DeviceAudioError>()
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> try {
                gate.await()
                DeviceAudioResult.PlaybackCompleted(1)
            } catch (cancelled: CancellationException) {
                throw IllegalStateException("driver stopped after cancellation")
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("playing", onError = errors::add)
        runCurrent()
        player.stop()
        runCurrent()
        assertTrue(errors.isEmpty())
        player.dispose()
    }

    @Test
    fun playbackOutlivesNavigationAndHandsTheDeviceToTheNextOwner() = runTest {
        val started = mutableListOf<String>()
        val firstPlayback = CompletableDeferred<Unit>()
        val secondPlayback = CompletableDeferred<Unit>()
        val admission = Mutex()
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> {
                started += operation.text
                if (operation.text == "first") firstPlayback.await() else secondPlayback.await()
                DeviceAudioResult.PlaybackCompleted(1)
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val first = VoiceSpeechPlayer(::perform, admission, dispatcher)
        val second = VoiceSpeechPlayer(::perform, admission, dispatcher)

        val navigatingUi = launch { first.speakAsync("first") }
        navigatingUi.join()
        second.speakAsync("second")
        runCurrent()
        assertEquals(listOf("first"), started)
        assertTrue(first.hasAudioWork())

        firstPlayback.complete(Unit)
        runCurrent()
        assertEquals(listOf("first", "second"), started)
        secondPlayback.complete(Unit)
        runCurrent()
        first.dispose()
        second.dispose()
    }

    @Test
    fun explicitStopCancelsPlayingAndQueuedSpeechForThatOwner() = runTest {
        val started = mutableListOf<String>()
        val gate = CompletableDeferred<Unit>()
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> {
                started += operation.text
                gate.await()
                DeviceAudioResult.PlaybackCompleted(1)
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("playing")
        player.speakAsync("queued")
        runCurrent()
        player.stop()
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("playing"), started)
        player.dispose()
    }
}
