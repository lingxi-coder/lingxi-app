package com.lingxi.code.computeruse

import android.content.Context
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.AndroidAudioSessionContext
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlin.math.max

internal data class ComputerUseTranscript(
    val text: String,
    val language: String?,
    val confidence: Float?,
    val durationMs: Long,
)

internal data class ComputerUseSpeechResult(
    val completed: Boolean,
    val durationMs: Long,
)

internal fun audioPlaybackTimeoutMs(frameCount: Int, sampleRate: Int): Long {
    val safeSampleRate = sampleRate.coerceAtLeast(1)
    val expectedMs = (
        frameCount.coerceAtLeast(0).toLong() * 1_000L + safeSampleRate - 1
    ) / safeSampleRate
    return (expectedMs + PLAYBACK_TIMEOUT_GRACE_MS)
        .coerceIn(PLAYBACK_TIMEOUT_GRACE_MS, MAX_PLAYBACK_TIMEOUT_MS)
}

/** Computer Use uses the same owner-scoped service as UI, Flow and engine operations. */
internal class ComputerUseAudioController(context: Context) {
    private val appContext = context.applicationContext
    private val cleanupScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val activeOwners = mutableMapOf<AudioOwnerKey, Int>()

    private fun requireOwner(engineOwner: String): AudioOwnerKey {
        val owner = AudioOwnerKey.computerUse(engineOwner)
        if (AndroidAudioSessionContext.engineFor(owner) == null) {
            throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "The originating Computer Use audio session is unavailable.")
        }
        return owner
    }

    private suspend fun <T> withOwner(engineOwner: String, action: suspend (AudioOwnerKey) -> T): T {
        val owner = requireOwner(engineOwner)
        synchronized(activeOwners) { activeOwners[owner] = (activeOwners[owner] ?: 0) + 1 }
        try { return action(owner) } finally {
            synchronized(activeOwners) {
                val count = activeOwners[owner] ?: 0
                if (count <= 1) activeOwners.remove(owner) else activeOwners[owner] = count - 1
            }
        }
    }

    suspend fun listen(engineOwner: String, language: String?, timeoutMs: Long): ComputerUseTranscript = withOwner(engineOwner) { owner ->
        val startedAt = System.currentTimeMillis()
        when (
            val result = AndroidAudioServiceProvider.perform(
                context = appContext,
                owner = owner,
                operation = DeviceAudioOperation.Listen(language),
                timeoutBudgetMs = timeoutMs,
            )
        ) {
            is DeviceAudioResult.Transcript -> ComputerUseTranscript(
                text = result.text,
                language = result.language ?: language,
                confidence = result.confidence,
                durationMs = (System.currentTimeMillis() - startedAt).coerceAtLeast(0L),
            )
            is DeviceAudioResult.Failed -> throw AudioOperationException(result.error.kind, result.error.message)
            else -> throw IllegalStateException("Android audio service returned an unexpected listen result")
        }
    }

    suspend fun speak(engineOwner: String, text: String, voice: String?, speed: Float): ComputerUseSpeechResult = withOwner(engineOwner) { owner ->
        val startedAt = System.currentTimeMillis()
        when (
            val result = AndroidAudioServiceProvider.perform(
                context = appContext,
                owner = owner,
                operation = DeviceAudioOperation.Speak(
                    text = text,
                    language = null,
                    rate = speed,
                    voice = voice,
                    foregroundUserInitiated = false,
                ),
            )
        ) {
            is DeviceAudioResult.PlaybackCompleted -> ComputerUseSpeechResult(
                completed = true,
                durationMs = (System.currentTimeMillis() - startedAt).coerceAtLeast(0L),
            )
            is DeviceAudioResult.Failed -> throw AudioOperationException(result.error.kind, result.error.message)
            else -> throw IllegalStateException("Android audio service returned an unexpected speak result")
        }
    }

    suspend fun stopAndWait(engineOwner: String) = stopOwner(AudioOwnerKey.computerUse(engineOwner))

    private suspend fun stopOwner(owner: AudioOwnerKey) {
        when (val result = AndroidAudioServiceProvider.perform(appContext, owner, DeviceAudioOperation.EndOwner)) {
            DeviceAudioResult.OwnerEnded -> Unit
            is DeviceAudioResult.Failed -> throw AudioOperationException(result.error.kind, result.error.message)
            else -> throw AudioOperationException(
                DeviceAudioErrorKind.NativeFailure,
                "Android audio service returned an unexpected stop result",
            )
        }
    }

    fun stop() {
        val owners = synchronized(activeOwners) { activeOwners.keys.toList() }
        cleanupScope.launch { owners.forEach { owner -> runCatching { stopOwner(owner) } } }
    }
}

private const val PLAYBACK_TIMEOUT_GRACE_MS = 5_000L
private const val MAX_PLAYBACK_TIMEOUT_MS = 10 * 60 * 1_000L
