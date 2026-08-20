package com.lingxi.code.voice.audio

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import com.lingxi.code.voice.AndroidVoiceRuntime
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlin.math.max

internal class VoiceSpeechPlayer(
    context: Context,
    private val voiceRuntime: AndroidVoiceRuntime = AndroidVoiceRuntime(context),
) {
    private val appContext = context.applicationContext
    private val operationMutex = Mutex()
    private val stateLock = Any()

    @Volatile
    private var activeJob: Job? = null

    @Volatile
    private var activeTrack: AudioTrack? = null

    suspend fun speak(
        text: String,
        voice: String? = null,
        speed: Float? = null,
    ): Boolean = operationMutex.withLock {
        val playbackJob = Job(currentCoroutineContext()[Job])
        synchronized(stateLock) { activeJob = playbackJob }
        try {
            withContext(playbackJob) {
                val (pcm, sampleRate) = voiceRuntime.renderSpeech(
                    text = text,
                    voiceOverride = voice,
                    speedOverride = speed,
                )
                if (pcm.isEmpty()) return@withContext false
                playPcm(pcm, sampleRate)
                true
            }
        } finally {
            synchronized(stateLock) {
                if (activeJob === playbackJob) activeJob = null
            }
            playbackJob.cancel()
        }
    }

    fun stop() {
        val job: Job?
        val track: AudioTrack?
        synchronized(stateLock) {
            job = activeJob
            track = activeTrack
            activeJob = null
            activeTrack = null
        }
        job?.cancel()
        track?.let {
            runCatching { it.pause() }
            runCatching { it.flush() }
            runCatching { it.stop() }
            runCatching { it.release() }
        }
    }

    private suspend fun playPcm(pcm: ByteArray, sampleRate: Int) {
        val frameCount = pcm.size / 2
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
                    this@VoiceSpeechPlayer.stop()
                }
            },
        )
        try {
            check(track.state == AudioTrack.STATE_INITIALIZED) {
                "Android audio output is unavailable"
            }
            check(focus.register()) { "another app owns audio focus" }
            synchronized(stateLock) { activeTrack = track }
            track.play()
            withTimeout(playbackTimeoutMs(frameCount, sampleRate)) {
                var offset = 0
                while (offset < pcm.size) {
                    val written = track.write(
                        pcm,
                        offset,
                        pcm.size - offset,
                        AudioTrack.WRITE_NON_BLOCKING,
                    )
                    check(written >= 0) { "Android audio output failed ($written)" }
                    if (written == 0) {
                        delay(AUDIO_WRITE_RETRY_MS)
                    } else {
                        offset += written
                    }
                }
                while (track.playbackHeadPosition.toLong() < frameCount) {
                    val remainingFrames = frameCount - track.playbackHeadPosition.toLong()
                    val remainingMs = (remainingFrames * 1_000L) /
                        sampleRate.coerceAtLeast(1)
                    delay(remainingMs.coerceIn(10, 100))
                }
            }
        } finally {
            focus.unregister()
            synchronized(stateLock) {
                if (activeTrack === track) activeTrack = null
            }
            runCatching { track.stop() }
            runCatching { track.release() }
        }
    }
}

private fun playbackTimeoutMs(frameCount: Int, sampleRate: Int): Long {
    val safeSampleRate = sampleRate.coerceAtLeast(1)
    val expectedMs = (
        frameCount.coerceAtLeast(0).toLong() * 1_000L + safeSampleRate - 1
        ) / safeSampleRate
    return (expectedMs + PLAYBACK_TIMEOUT_GRACE_MS)
        .coerceIn(PLAYBACK_TIMEOUT_GRACE_MS, MAX_PLAYBACK_TIMEOUT_MS)
}

private const val AUDIO_WRITE_RETRY_MS = 10L
private const val PLAYBACK_TIMEOUT_GRACE_MS = 5_000L
private const val MAX_PLAYBACK_TIMEOUT_MS = 10 * 60 * 1_000L
