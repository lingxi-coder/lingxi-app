package com.lingxi.code.offload

import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.DeviceMediaPlaybackState
import com.lingxi.code.voice.audio.OffloadMediaCommand
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotEquals
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

class AndroidSystemOffloadAudioTest {
    private class FakeAudioService : OffloadAudioService {
        data class Call(val name: String, val owner: AudioOwnerKey, val label: String? = null, val value: String? = null)

        val calls = CopyOnWriteArrayList<Call>()
        var blockedPlayTarget: String? = null
        var playEntered: CompletableDeferred<Unit>? = null
        var playGate: CompletableDeferred<Unit>? = null
        val cancelledPlayTargets = CopyOnWriteArrayList<String>()

        override suspend fun speak(owner: AudioOwnerKey, text: String, timeoutBudgetMs: Long): DeviceAudioResult {
            calls += Call("speak", owner, value = text)
            assertEquals(30_000L, timeoutBudgetMs)
            return DeviceAudioResult.PlaybackCompleted(100L)
        }

        override suspend fun playMedia(owner: AudioOwnerKey, label: String, target: String): DeviceAudioResult {
            calls += Call("play", owner, label, target)
            if (target == blockedPlayTarget) {
                playEntered?.complete(Unit)
                try {
                    playGate?.await()
                } catch (cancelled: CancellationException) {
                    cancelledPlayTargets += target
                    throw cancelled
                }
            }
            return DeviceAudioResult.OffloadMedia(DeviceMediaPlaybackState(true, 0, 250))
        }

        override suspend fun controlMedia(
            owner: AudioOwnerKey,
            label: String,
            command: OffloadMediaCommand,
        ): DeviceAudioResult {
            calls += Call(command.name.lowercase(), owner, label)
            return DeviceAudioResult.OffloadMedia(DeviceMediaPlaybackState(command == OffloadMediaCommand.RESUME, 50, 250))
        }
    }

    @Test
    fun speechSpeakAndSayCommandsUseSessionOwnedAudioServiceAndWaitForPlayback() = runTest {
        val audio = FakeAudioService()
        val port = SpeechPort(audio)

        val speak = port.execute(NativeOffloadRequest("speech", listOf("speak", "hello", "from", "device"), sessionId = "session-a"))
        val say = port.execute(NativeOffloadRequest("speech", listOf("say"), stdin = "spoken input".toByteArray(), sessionId = "session-b"))

        assertTrue(speak.succeeded)
        assertTrue(say.succeeded)
        assertEquals(
            listOf(
                FakeAudioService.Call("speak", AudioOwnerKey.session("session-a"), value = "hello from device"),
                FakeAudioService.Call("speak", AudioOwnerKey.session("session-b"), value = "spoken input"),
            ),
            audio.calls,
        )
    }

    @Test
    fun androidOffloadSpeechAdapterUsesDefaultV3SpeakRouteAndWaitsForPlaybackResult() = runTest {
        val owner = AudioOwnerKey.session("session-speech")
        val calls = mutableListOf<Triple<AudioOwnerKey, DeviceAudioOperation, Long?>>()
        val adapter = AndroidOffloadAudioService(
            AudioServiceOperationPerformer { requestOwner, operation, timeout ->
                calls += Triple(requestOwner, operation, timeout)
                DeviceAudioResult.PlaybackCompleted(777L)
            },
        )

        val result = adapter.speak(owner, "uses the v3 preference route", timeoutBudgetMs = 30_000L)

        assertEquals(DeviceAudioResult.PlaybackCompleted(777L), result)
        val (requestOwner, operation, timeout) = calls.single()
        assertEquals(owner, requestOwner)
        assertEquals(30_000L, timeout)
        assertTrue(operation is DeviceAudioOperation.Speak)
        operation as DeviceAudioOperation.Speak
        assertEquals("uses the v3 preference route", operation.text)
        assertNull(operation.rate)
        assertNull(operation.voice)
        assertNull(operation.configuration)
    }

    @Test
    fun playerCommandsKeepTheirOutputShapeAndTreatTheOptionalSessionAsAnOwnerLocalLabel() = runTest {
        val audio = FakeAudioService()
        val port = PlayerPort(audio)

        val play = port.execute(NativeOffloadRequest("player", listOf("play", "content://songs/1", "mix"), sessionId = "owner-a"))
        val pause = port.execute(NativeOffloadRequest("player", listOf("pause", "mix"), sessionId = "owner-a"))
        val resume = port.execute(NativeOffloadRequest("player", listOf("resume", "mix"), sessionId = "owner-a"))
        val status = port.execute(NativeOffloadRequest("player", listOf("status", "mix"), sessionId = "owner-a"))
        val stop = port.execute(NativeOffloadRequest("player", listOf("stop", "mix"), sessionId = "owner-b"))

        assertEquals("session=mix\ndurationMs=250\n", play.stdoutText())
        assertTrue(pause.stderrText(), pause.succeeded)
        assertTrue(resume.stderrText(), resume.succeeded)
        assertEquals("playing=false\npositionMs=50\ndurationMs=250\n", status.stdoutText())
        assertTrue("unknown labels stay inside their stable request owner", stop.succeeded)
        assertEquals(
            listOf("play", "pause", "resume", "status", "stop"),
            audio.calls.map { it.name },
        )
        assertEquals("owner-a", audio.calls.first().owner.id)
        assertEquals("owner-b", audio.calls.last().owner.id)
        assertEquals(audio.calls.first().label, audio.calls.last().label)
        val callsBeforeClose = audio.calls.size
        port.close()
        withContext(Dispatchers.IO) {
            withTimeout(1_000L) {
                while (audio.calls.none { it.name == "stop" && it.owner == AudioOwnerKey.session("owner-a") }) {
                    delay(1)
                }
            }
        }
        val closeStops = audio.calls.drop(callsBeforeClose).filter { it.name == "stop" }
        assertEquals(listOf(AudioOwnerKey.session("owner-a")), closeStops.map { it.owner })
    }

    @Test
    fun closingAnOldPortCancelsDelayedPlayAndCannotStopReplacementMedia() = runTest {
        val audio = FakeAudioService().apply {
            blockedPlayTarget = "/music/old.mp3"
            playEntered = CompletableDeferred()
            playGate = CompletableDeferred()
        }
        val owner = AudioOwnerKey.session("same-session")
        val oldPort = PlayerPort(audio)
        val oldCall = launch {
            oldPort.execute(NativeOffloadRequest("player", listOf("play", "/music/old.mp3", "mix"), sessionId = owner.id))
        }
        audio.playEntered!!.await()
        val oldServiceLabel = audio.calls.first { it.name == "play" }.label

        oldPort.close()
        oldCall.join()
        assertTrue("close cancels pending audio service admission", oldCall.isCancelled)
        assertEquals(listOf("/music/old.mp3"), audio.cancelledPlayTargets.toList())

        val replacement = PlayerPort(audio)
        val replacementResult = replacement.execute(
            NativeOffloadRequest("player", listOf("play", "/music/new.mp3", "mix"), sessionId = owner.id),
        )
        assertTrue(replacementResult.succeeded)
        val replacementLabel = audio.calls.last { it.name == "play" }.label
        assertNotEquals("port instances use separate internal media labels", oldServiceLabel, replacementLabel)

        withContext(Dispatchers.IO) {
            withTimeout(1_000L) {
                while (audio.calls.none { it.name == "stop" && it.label == oldServiceLabel }) delay(1)
            }
        }
        assertTrue("old port cleanup cannot stop replacement media", audio.calls.none { it.name == "stop" && it.label == replacementLabel })
        assertTrue(replacement.execute(
            NativeOffloadRequest("player", listOf("status", "mix"), sessionId = owner.id),
        ).succeeded)
        replacement.close()
    }

    @Test
    fun speechFailureRemainsVisibleToTheOffloadCommand() = runTest {
        val audio = object : OffloadAudioService by FakeAudioService() {
            override suspend fun speak(owner: AudioOwnerKey, text: String, timeoutBudgetMs: Long) =
                DeviceAudioResult.Failed(DeviceAudioError(DeviceAudioErrorKind.Unavailable, "provider unavailable"))
        }

        val result = SpeechPort(audio).execute(
            NativeOffloadRequest("speech", listOf("speak", "hello"), sessionId = "session-c"),
        )

        assertEquals(NativeOffloadResult.EXIT_FAILURE, result.exitCode)
        assertEquals("speech: provider unavailable\n", result.stderrText())
    }
}
