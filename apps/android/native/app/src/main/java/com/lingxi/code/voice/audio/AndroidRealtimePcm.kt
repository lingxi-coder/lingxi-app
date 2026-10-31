package com.lingxi.code.voice.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CompletableDeferred
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

internal data class RealtimePcmWrite(
    val bytes: ByteArray,
    val generation: Long,
    val sourceSampleFrames: Int,
    val completion: CompletableDeferred<RealtimePcmWriteReceipt>,
)

internal data class RealtimePcmWriteReceipt(
    val playedSampleFrames: Int,
    val renderedAtUnixMs: Long?,
)

internal data class RealtimePcmCaptureFrame(
    val pcm16le: ByteArray,
    val capturedAtUnixMs: Long,
)

internal class AndroidRealtimePcmCapture {
    val paused = AtomicBoolean(false)
    val nativeAllocationAttempted = AtomicBoolean(false)
    private val stopped = AtomicBoolean(false)
    @Volatile private var recorder: AudioRecord? = null
    fun stop() { stopped.set(true); recorder?.let { runCatching { it.stop() } } }
    suspend fun run(
        format: RealtimePcmFormat,
        onChunk: suspend (ByteArray) -> Unit,
        onReady: () -> Unit = {},
    ) = runTimestamped(format, { onChunk(it.pcm16le) }, onReady)

    suspend fun runTimestamped(
        format: RealtimePcmFormat,
        onChunk: suspend (RealtimePcmCaptureFrame) -> Unit,
        onReady: () -> Unit = {},
        closeReceipt: AndroidMediaNativeCloseReceipt? = null,
    ) = withContext(Dispatchers.IO) {
        val min = try {
            format.requireSupported()
            AudioRecord.getMinBufferSize(format.sampleRateHz, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        } catch (error: Throwable) {
            closeReceipt?.confirmNoResource()
            throw error
        }
        if (min <= 0) {
            closeReceipt?.confirmNoResource()
            throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime microphone is unavailable.")
        }
        nativeAllocationAttempted.set(true)
        val native = AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, format.sampleRateHz, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT, maxOf(min, format.sampleRateHz / 5))
        recorder = native
        val frames = ByteArray(format.sampleRateHz / 25 * 2)
        try {
            if (native.state != AudioRecord.STATE_INITIALIZED) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime microphone could not initialize.")
            currentCoroutineContext().ensureActive()
            if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime capture stopped")
            native.startRecording()
            onReady()
            while (!stopped.get()) {
                currentCoroutineContext().ensureActive()
                val count = native.read(frames, 0, frames.size)
                if (count < 0) {
                    if (stopped.get()) break
                    throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime microphone read failed.")
                }
                if (count > 0 && !paused.get()) {
                    if (count % 2 != 0) {
                        throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime microphone returned an unaligned PCM16 frame.")
                    }
                    val pcm = frames.copyOf(count)
                    val capturedDurationMs = count.toLong() * 500L / format.sampleRateHz
                    val capturedAt = (System.currentTimeMillis() - capturedDurationMs).coerceAtLeast(0L)
                    onChunk(RealtimePcmCaptureFrame(pcm, capturedAt))
                }
            }
        } finally {
            recorder = null
            runCatching { native.stop() }
            try {
                native.release()
                closeReceipt?.confirmNoResource()
            } catch (error: Throwable) {
                closeReceipt?.failNativeClose(error)
                throw error
            }
        }
    }
}

internal class AndroidRealtimePcmPlayback(private val context: android.content.Context) {
    private val stopped = AtomicBoolean(false)
    val nativeAllocationAttempted = AtomicBoolean(false)
    private val activeWriteLock = Any()
    @Volatile private var track: AudioTrack? = null
    @Volatile private var sampleRateHz: Int = 0
    @Volatile private var activeWrite: RealtimePcmWrite? = null
    @Volatile private var activeStartHead: Long = 0L
    @Volatile private var activeAcceptedFrames: Int = 0
    private val minimumGeneration = java.util.concurrent.atomic.AtomicLong(0L)
    fun positionMs(): Long? = track?.let { native ->
        sampleRateHz.takeIf { it > 0 }?.let { rate -> runCatching {
            (native.playbackHeadPosition.toLong() and 0xffffffffL) * 1000 / rate
        }.getOrNull() }
    }
    fun stop() {
        synchronized(activeWriteLock) {
            stopped.set(true)
            track?.let { runCatching { it.pause() }; runCatching { it.flush() }; runCatching { it.stop() } }
        }
    }
    fun setVolume(gain: Float) {
        require(gain.isFinite() && gain in 0f..1f)
        track?.setVolume(gain)
    }
    /** Interrupt only output queued before the supplied generation; capture stays live. */
    fun interrupt(throughGeneration: Long): RealtimePcmWriteReceipt {
        synchronized(activeWriteLock) {
            minimumGeneration.accumulateAndGet(throughGeneration) { current, incoming -> maxOf(current, incoming) }
            val frame = activeWrite?.takeIf { it.generation < throughGeneration }
                ?: return RealtimePcmWriteReceipt(0, null)
            val native = track ?: return RealtimePcmWriteReceipt(0, null)
            runCatching { native.pause() }
            val played = playedSourceFrames(
                native,
                activeStartHead,
                activeAcceptedFrames,
                frame.sourceSampleFrames,
            )
            runCatching { native.flush() }
            runCatching { native.play() }
            // AudioTrack exposes rendered frame progress here, but this path
            // does not obtain a timestamped hardware clock sample.
            val receipt = RealtimePcmWriteReceipt(played, null)
            frame.completion.complete(receipt)
            if (activeWrite === frame) activeWrite = null
            return receipt
        }
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

    /** Write bounded frames and resolve each only after AudioTrack reports render progress. */
    suspend fun runAcknowledged(
        format: RealtimePcmFormat,
        chunks: ReceiveChannel<RealtimePcmWrite>,
        onReady: () -> Unit = {},
        closeReceipt: AndroidMediaNativeCloseReceipt? = null,
    ) = withContext(Dispatchers.IO) {
        val min = try {
            format.requireSupported()
            AudioTrack.getMinBufferSize(format.sampleRateHz, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        } catch (error: Throwable) {
            closeReceipt?.confirmNoResource()
            throw error
        }
        if (min <= 0) {
            closeReceipt?.confirmNoResource()
            throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Realtime speaker is unavailable.")
        }
        nativeAllocationAttempted.set(true)
        val native = AudioTrack.Builder().setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()).setAudioFormat(AudioFormat.Builder()
            .setSampleRate(format.sampleRateHz).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
            .setBufferSizeInBytes(maxOf(min, format.sampleRateHz / 5)).setTransferMode(AudioTrack.MODE_STREAM).build()
        track = native
        sampleRateHz = format.sampleRateHz
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
            onReady()
            for (frame in chunks) {
                currentCoroutineContext().ensureActive()
                if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback interrupted")
                if (frame.generation < minimumGeneration.get()) {
                    frame.completion.complete(RealtimePcmWriteReceipt(0, null))
                    continue
                }
                val startHead = native.playbackHeadPosition.toLong() and 0xffffffffL
                val becameActive = synchronized(activeWriteLock) {
                    if (frame.completion.isCompleted || frame.generation < minimumGeneration.get()) {
                        false
                    } else {
                        activeStartHead = startHead
                        activeAcceptedFrames = 0
                        activeWrite = frame
                        true
                    }
                }
                if (!becameActive) {
                    frame.completion.complete(RealtimePcmWriteReceipt(0, null))
                    continue
                }
                var offset = 0
                while (offset < frame.bytes.size) {
                    currentCoroutineContext().ensureActive()
                    if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback interrupted")
                    val count = synchronized(activeWriteLock) {
                        if (stopped.get() || frame.completion.isCompleted || frame.generation < minimumGeneration.get()) {
                            -1
                        } else {
                            val written = native.write(frame.bytes, offset, frame.bytes.size - offset, AudioTrack.WRITE_NON_BLOCKING)
                            if (written > 0 && written % 2 != 0) {
                                throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime speaker returned an unaligned PCM16 write.")
                            }
                            if (written > 0) activeAcceptedFrames += written / 2
                            written
                        }
                    }
                    if (count < 0) {
                        if (!frame.completion.isCompleted) {
                            throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "Realtime speaker write failed ($count).")
                        }
                        break
                    }
                    if (count == 0) delay(5) else offset += count
                }
                while (mediaDeviceFramesSince(startHead, native.playbackHeadPosition.toLong() and 0xffffffffL) < activeAcceptedFrames
                    && !frame.completion.isCompleted
                    && frame.generation >= minimumGeneration.get()) {
                    currentCoroutineContext().ensureActive()
                    if (stopped.get()) throw kotlinx.coroutines.CancellationException("Realtime playback interrupted")
                    delay(5)
                }
                completeActiveWriteFromHead(frame, native)
            }
        } catch (error: Throwable) {
            synchronized(activeWriteLock) {
                activeWrite?.let { frame ->
                    if (!frame.completion.isCompleted) frame.completion.completeExceptionally(error)
                    if (activeWrite === frame) activeWrite = null
                }
            }
            while (true) {
                val frame = chunks.tryReceive().getOrNull() ?: break
                frame.completion.completeExceptionally(error)
            }
            throw error
        } finally {
            var cleanupFailure: Throwable? = null
            try { focus.unregister() } catch (error: Throwable) { cleanupFailure = error }
            track = null
            runCatching { native.stop() }
            try { native.release() }
            catch (error: Throwable) {
                if (cleanupFailure == null) cleanupFailure = error else cleanupFailure.addSuppressed(error)
            }
            cleanupFailure?.let {
                closeReceipt?.failNativeClose(it)
                throw it
            }
            closeReceipt?.confirmNoResource()
        }
    }

    private fun completeActiveWriteFromHead(frame: RealtimePcmWrite, native: AudioTrack) {
        synchronized(activeWriteLock) {
            if (frame.completion.isCompleted) {
                if (activeWrite === frame) activeWrite = null
                return
            }
            val played = playedSourceFrames(
                native,
                activeStartHead,
                activeAcceptedFrames,
                frame.sourceSampleFrames,
            )
            frame.completion.complete(
                RealtimePcmWriteReceipt(
                    played,
                    null,
                ),
            )
            if (activeWrite === frame) activeWrite = null
        }
    }

    private fun playedSourceFrames(
        native: AudioTrack,
        startHead: Long,
        acceptedDeviceFrames: Int,
        sourceFrames: Int,
    ): Int {
        val currentHead = native.playbackHeadPosition.toLong() and 0xffffffffL
        return mediaPlayedSourceFrames(
            startHead,
            currentHead,
            acceptedDeviceFrames,
            sourceFrames,
        )
    }
}

internal fun mediaDeviceFramesSince(startHead: Long, currentHead: Long): Long =
    (currentHead - startHead) and 0xffffffffL

internal fun mediaPlayedSourceFrames(
    startDeviceHead: Long,
    currentDeviceHead: Long,
    acceptedDeviceFrames: Int,
    sourceSampleFrames: Int,
): Int {
    if (acceptedDeviceFrames <= 0 || sourceSampleFrames <= 0) return 0
    val playedDeviceFrames = mediaDeviceFramesSince(startDeviceHead, currentDeviceHead)
        .coerceAtMost(acceptedDeviceFrames.toLong())
    if (playedDeviceFrames == acceptedDeviceFrames.toLong()) return sourceSampleFrames
    val sourceFramesPlayed = (playedDeviceFrames * sourceSampleFrames + acceptedDeviceFrames - 1) / acceptedDeviceFrames
    return sourceFramesPlayed
        .coerceAtMost(sourceSampleFrames.toLong()).toInt()
}
