package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
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
