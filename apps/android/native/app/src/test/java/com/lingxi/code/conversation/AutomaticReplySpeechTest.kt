package com.lingxi.code.conversation

import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.VoiceSpeechPlayer
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AutomaticReplySpeechTest {
    @Test
    fun hiddenOwnerCompletionWaitsForSelectedOwnersWorkAndPlaysOnceForItsOwnOwner() = runTest {
        var selected = "A"
        var selectedNeedsInput = false
        val gate = AutomaticReplySpeechGate { selected == "B" && selectedNeedsInput }
        gate.configure(enabled = true, foreground = true)
        val admission = Mutex()
        val spoken = mutableListOf<Pair<AudioOwnerKey, String>>()
        val ended = mutableListOf<AudioOwnerKey>()
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> {
                spoken += owner to operation.text
                DeviceAudioResult.PlaybackCompleted(1)
            }
            DeviceAudioOperation.EndOwner -> { ended += owner; DeviceAudioResult.OwnerEnded }
            else -> error("unexpected operation")
        }
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val playerA = VoiceSpeechPlayer(::perform, admission, dispatcher)
        val playerB = VoiceSpeechPlayer(::perform, admission, dispatcher)
        val eventsA = MutableSharedFlow<ConversationTurnCompletion>()
        val eventsB = MutableSharedFlow<ConversationTurnCompletion>()
        val flowCompletions = mutableListOf<String>()
        fun observe(events: MutableSharedFlow<ConversationTurnCompletion>, player: VoiceSpeechPlayer, owner: String) =
            observeOwnerTurnCompletions(events, gate::acceptsCompletion, { false },
                { flowCompletions += owner },
                { text -> player.speakAsync(text, awaitStart = gate::awaitStart, mayStart = gate::mayStart, retryBusy = true) })
        val observerA = observe(eventsA, playerA, "A")
        val observerB = observe(eventsB, playerB, "B")

        // A has an admitted turn; while B is selected its capture owns admission.
        admission.lock()
        selected = "B"
        selectedNeedsInput = true
        gate.signal()
        eventsA.emit(ConversationTurnCompletion(1, ConversationTurnOrigin.Ordinary, ConversationTurnOutcome.Completed, "  A reply  "))
        eventsB.emit(ConversationTurnCompletion(2, ConversationTurnOrigin.Flow, ConversationTurnOutcome.Completed, "B flow"))
        runCurrent()
        assertEquals(emptyList<Pair<AudioOwnerKey, String>>(), spoken)
        assertTrue(playerA.hasAudioWork())
        assertEquals(listOf("B"), flowCompletions)

        selectedNeedsInput = false
        gate.signal()
        runCurrent() // A waits on the shared admission lock.
        selectedNeedsInput = true // B begins waiting for approval before the lock is available.
        admission.unlock()
        runCurrent()
        assertTrue(spoken.isEmpty())
        assertTrue(ended.isEmpty())

        selectedNeedsInput = false
        gate.signal()
        runCurrent()
        assertEquals(listOf(playerA.audioOwner to "A reply"), spoken)
        assertFalse(playerA.hasAudioWork())
        gate.signal()
        runCurrent()
        assertEquals(1, spoken.size)
        assertTrue(ended.isEmpty())

        observerA.cancel()
        observerB.cancel()
        playerA.dispose()
        playerB.dispose()
    }

    @Test
    fun busyAdmissionRetriesWithoutLosingOrDuplicatingReply() = runTest {
        val gate = AutomaticReplySpeechGate { false }
        gate.configure(enabled = true, foreground = true)
        var attempts = 0
        var completed = 0
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> {
                attempts++
                if (attempts == 1) DeviceAudioResult.Failed(DeviceAudioError(DeviceAudioErrorKind.Busy, "capture active"))
                else { completed++; DeviceAudioResult.PlaybackCompleted(1) }
            }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("reply", awaitStart = gate::awaitStart, mayStart = gate::mayStart, retryBusy = true)
        runCurrent()
        assertTrue(player.hasAudioWork())
        assertEquals(0, completed)
        advanceTimeBy(250)
        runCurrent()
        assertEquals(2, attempts)
        assertEquals(1, completed)
        assertFalse(player.hasAudioWork())
        player.dispose()
    }

    @Test
    fun disablingAutomaticSpeechDiscardsOnlyThePendingAutomaticReply() = runTest {
        var blocked = true
        val gate = AutomaticReplySpeechGate { blocked }
        gate.configure(enabled = true, foreground = true)
        var started = 0
        suspend fun perform(owner: AudioOwnerKey, operation: DeviceAudioOperation): DeviceAudioResult = when (operation) {
            is DeviceAudioOperation.Speak -> { started++; DeviceAudioResult.PlaybackCompleted(1) }
            DeviceAudioOperation.EndOwner -> DeviceAudioResult.OwnerEnded
            else -> error("unexpected operation")
        }
        val player = VoiceSpeechPlayer(::perform, Mutex(), UnconfinedTestDispatcher(testScheduler))
        player.speakAsync("pending", awaitStart = gate::awaitStart, mayStart = gate::mayStart, retryBusy = true)
        runCurrent()
        gate.configure(enabled = false, foreground = true)
        runCurrent()
        blocked = false
        gate.configure(enabled = true, foreground = true)
        runCurrent()
        assertEquals(0, started)
        assertFalse(player.hasAudioWork())
        player.dispose()
    }
}
