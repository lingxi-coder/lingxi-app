package com.lingxi.code.computeruse

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import androidx.core.content.ContextCompat
import com.lingxi.code.voice.audio.AudioFocusController
import com.lingxi.code.voice.audio.AudioFocusListener
import com.lingxi.code.voice.audio.AudioInput
import com.lingxi.code.voice.audio.SttResult
import com.lingxi.code.voice.audio.SystemSpeechRecognizerStt
import com.lingxi.code.voice.audio.SystemTextToSpeechTts
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlin.coroutines.CoroutineContext
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

/**
 * Short-lived microphone/TTS bridge for the Direct Computer Use host.
 *
 * It reuses the same Android system providers selected by the navigation voice
 * page. Only one audio operation is allowed at a time, and stop/session teardown
 * cancels the active coroutine and releases AudioTrack immediately.
 */
internal class ComputerUseAudioController(context: Context) {
    private val appContext = context.applicationContext
    private val operationMutex = Mutex()
    private val stateLock = Any()

    @Volatile
    private var activeJob: Job? = null

    @Volatile
    private var activeTrack: AudioTrack? = null

    suspend fun listen(language: String?, timeoutMs: Long): ComputerUseTranscript =
        operationMutex.withLock {
            requireMicrophonePermission()
            val startedAt = System.currentTimeMillis()
            withActiveJob(currentCoroutineContext()) {
                when (val result = withTimeout(timeoutMs) {
                    SystemSpeechRecognizerStt(appContext).transcribe(
                        audio = AudioInput.Pcm16(ByteArray(0), 16_000),
                        language = language,
                        keyProvider = { null },
                    )
                }) {
                    is SttResult.Ok -> ComputerUseTranscript(
                        text = result.text,
                        language = result.language ?: language,
                        confidence = result.confidence,
                        durationMs = System.currentTimeMillis() - startedAt,
                    )
                    is SttResult.Err -> error("${result.code}: ${result.message}")
                }
            }
        }

    suspend fun speak(
        text: String,
        voice: String?,
        speed: Float,
    ): ComputerUseSpeechResult = operationMutex.withLock {
        val startedAt = System.currentTimeMillis()
        withActiveJob(currentCoroutineContext()) {
            val (pcm, sampleRate) = SystemTextToSpeechTts(appContext).renderToPcm(
                text = text,
                voice = voice,
                speed = speed,
            )
            check(pcm.isNotEmpty()) { "Android TTS did not produce audio" }
            playPcm(pcm, sampleRate)
            ComputerUseSpeechResult(
                completed = true,
                durationMs = System.currentTimeMillis() - startedAt,
            )
        }
    }

    fun stop() {
        synchronized(stateLock) {
            activeJob?.cancel()
            activeTrack?.let { track ->
                runCatching { track.pause() }
                runCatching { track.flush() }
                runCatching { track.stop() }
                runCatching { track.release() }
            }
            activeTrack = null
        }
    }

    private fun requireMicrophonePermission() {
        check(
            ContextCompat.checkSelfPermission(appContext, Manifest.permission.RECORD_AUDIO) ==
                PackageManager.PERMISSION_GRANTED,
        ) {
            "microphone permission is not granted; open LingXi and allow microphone access first"
        }
    }

    private suspend fun playPcm(pcm: ByteArray, sampleRate: Int) {
        val frameCount = pcm.size / 2
        val durationMs = ((frameCount * 1_000L) / sampleRate.coerceAtLeast(1)).coerceAtLeast(1)
        val minBuffer = AudioTrack.getMinBufferSize(
            sampleRate,
            AudioFormat.CHANNEL_OUT_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        ).coerceAtLeast(0)
        val track = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_ASSISTANT)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(sampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .build(),
            )
            .setBufferSizeInBytes(max(minBuffer, minOf(pcm.size, sampleRate * 2)))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
        check(track.state == AudioTrack.STATE_INITIALIZED) { "Android audio output is unavailable" }

        val focus = AudioFocusController(
            appContext,
            object : AudioFocusListener {
                override fun onResume() {
                    runCatching { track.play() }
                }

                override fun onPause() {
                    runCatching { track.pause() }
                }

                override fun onStop() {
                    this@ComputerUseAudioController.stop()
                }
            },
        )
        check(focus.register()) { "another app owns audio focus" }
        synchronized(stateLock) { activeTrack = track }
        try {
            track.play()
            withContext(kotlinx.coroutines.Dispatchers.IO) {
                var offset = 0
                while (offset < pcm.size) {
                    val written = track.write(
                        pcm,
                        offset,
                        pcm.size - offset,
                        AudioTrack.WRITE_BLOCKING,
                    )
                    check(written > 0) { "Android audio output failed ($written)" }
                    offset += written
                }
            }
            delay(durationMs + 50)
        } finally {
            focus.unregister()
            synchronized(stateLock) {
                if (activeTrack === track) activeTrack = null
            }
            runCatching { track.stop() }
            runCatching { track.release() }
        }
    }

    private suspend fun <T> withActiveJob(
        context: CoroutineContext,
        block: suspend () -> T,
    ): T {
        val job = context[Job]
        synchronized(stateLock) { activeJob = job }
        return try {
            block()
        } finally {
            synchronized(stateLock) {
                if (activeJob === job) activeJob = null
            }
        }
    }
}
