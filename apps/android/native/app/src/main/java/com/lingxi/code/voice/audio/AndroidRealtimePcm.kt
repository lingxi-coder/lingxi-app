package com.lingxi.code.voice.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicBoolean

internal data class RealtimePcmFormat(val sampleRateHz: Int, val channels: Int, val encoding: String) {
    fun requireSupported() {
        if (channels != 1 || encoding !in listOf("pcm16", "pcm_s16le") || sampleRateHz !in 8_000..48_000) {
            throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Realtime audio requires supported mono PCM16 device audio.")
        }
    }
}

internal class AndroidRealtimePcmCapture {
    val paused = AtomicBoolean(false)
    private val stopped = AtomicBoolean(false)
    @Volatile private var recorder: AudioRecord? = null
    fun stop() { stopped.set(true); recorder?.let { runCatching { it.stop() } } }
    suspend fun run(format: RealtimePcmFormat, onChunk: suspend (ByteArray) -> Unit) = withContext(Dispatchers.IO) {
        format.requireSupported()
        val min = AudioRecord.getMinBufferSize(format.sampleRateHz, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime microphone is unavailable.")
        val native = AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, format.sampleRateHz, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT, maxOf(min, format.sampleRateHz / 5))
        recorder = native
        val frames = ByteArray(format.sampleRateHz / 25 * 2)
        try {
            if (native.state != AudioRecord.STATE_INITIALIZED) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime microphone could not initialize.")
            currentCoroutineContext().ensureActive()
            if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime capture stopped")
            native.startRecording()
            while (!stopped.get()) {
                currentCoroutineContext().ensureActive()
                val count = native.read(frames, 0, frames.size)
                if (count < 0) {
                    if (stopped.get()) break
                    throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime microphone read failed.")
                }
                if (count > 0 && !paused.get()) onChunk(frames.copyOf(count - count % 2))
            }
        } finally {
            recorder = null
            runCatching { native.stop() }; native.release()
        }
    }
}

internal class AndroidRealtimePcmPlayback(private val context: android.content.Context) {
    private val stopped = AtomicBoolean(false)
    @Volatile private var track: AudioTrack? = null
    @Volatile private var sampleRateHz: Int = 0
    fun positionMs(): Long? = track?.let { native ->
        sampleRateHz.takeIf { it > 0 }?.let { rate -> runCatching {
            (native.playbackHeadPosition.toLong() and 0xffffffffL) * 1000 / rate
        }.getOrNull() }
    }
    fun stop() {
        stopped.set(true)
        track?.let { runCatching { it.pause() }; runCatching { it.flush() }; runCatching { it.stop() } }
    }
    suspend fun run(format: RealtimePcmFormat, chunks: ReceiveChannel<ByteArray>) = withContext(Dispatchers.IO) {
        format.requireSupported()
        val min = AudioTrack.getMinBufferSize(format.sampleRateHz, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime speaker is unavailable.")
        val native = AudioTrack.Builder().setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()).setAudioFormat(AudioFormat.Builder()
            .setSampleRate(format.sampleRateHz).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
            .setBufferSizeInBytes(maxOf(min, format.sampleRateHz / 5)).setTransferMode(AudioTrack.MODE_STREAM).build()
        track = native
        sampleRateHz = format.sampleRateHz
        var framesWritten = 0L
        val focus = AudioFocusController(context, object : AudioFocusListener {
            override fun onResume() = Unit
            override fun onPause() = stop()
            override fun onStop() = stop()
        })
        try {
            if (native.state != AudioTrack.STATE_INITIALIZED) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime speaker could not initialize.")
            if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback stopped")
            if (!focus.register()) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Audio focus is unavailable for realtime playback.")
            native.play()
            for (chunk in chunks) {
                currentCoroutineContext().ensureActive()
                if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback interrupted")
                var offset = 0
                while (offset < chunk.size) {
                    val count = native.write(chunk, offset, chunk.size - offset, AudioTrack.WRITE_BLOCKING)
                    if (count <= 0) throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime speaker write failed.")
                    offset += count; framesWritten += count / 2
                }
            }
            while ((native.playbackHeadPosition.toLong() and 0xffffffffL) < framesWritten) {
                currentCoroutineContext().ensureActive()
                if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback interrupted")
                delay(10)
            }
        } finally {
            focus.unregister()
            track = null
            runCatching { native.stop() }; native.release()
        }
    }
}
