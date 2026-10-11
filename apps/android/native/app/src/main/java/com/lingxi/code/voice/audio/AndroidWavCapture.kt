package com.lingxi.code.voice.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean

/** Bounded mono PCM capture. AEC/full duplex are deliberately not advertised by this lane. */
internal class AndroidWavCapture(private val context: Context) {
    private val stopped = AtomicBoolean(false)
    @Volatile private var recorder: AudioRecord? = null

    fun stop() {
        stopped.set(true)
        recorder?.let { runCatching { it.stop() } }
    }

    suspend fun capture(maxPayloadBytes: Int, untilSilence: Boolean, onReady: () -> Unit): DeviceAudioCapture =
        withContext(Dispatchers.IO) {
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                throw AudioOperationException(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is not granted.")
            }
            if (maxPayloadBytes < 46) throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Audio payload limit is too small for WAV capture.")
            val minBuffer = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            if (minBuffer <= 0) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "PCM microphone capture is unavailable.")
            val native = AudioRecord(MediaRecorder.AudioSource.MIC, SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(minBuffer, 4096))
            recorder = native
            val output = ByteArrayOutputStream()
            var speechSeen = false
            var silentFrames = 0
            val buffer = ByteArray(maxOf(minBuffer, 4096))
            try {
                if (native.state != AudioRecord.STATE_INITIALIZED) throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "PCM microphone could not initialize.")
                currentCoroutineContext().ensureActive()
                if (stopped.get()) throw kotlinx.coroutines.CancellationException("Capture cancelled before admission")
                native.startRecording()
                onReady()
                while (!stopped.get()) {
                    currentCoroutineContext().ensureActive()
                    val size = native.read(buffer, 0, buffer.size)
                    if (size < 0) {
                        if (stopped.get()) break
                        throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "PCM microphone read failed ($size).")
                    }
                    if (size == 0) continue
                    val aligned = size - size % 2
                    if (output.size().toLong() + aligned + WAV_HEADER_SIZE > maxPayloadBytes) {
                        throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Microphone recording exceeds the payload limit.")
                    }
                    output.write(buffer, 0, aligned)
                    var peak = 0
                    for (index in 0 until aligned step 2) {
                        val sample = ((buffer[index].toInt() and 255) or (buffer[index + 1].toInt() shl 8)).toShort().toInt()
                        peak = maxOf(peak, kotlin.math.abs(sample))
                    }
                    if (peak > 800) { speechSeen = true; silentFrames = 0 } else silentFrames += aligned / 2
                    if (untilSilence && speechSeen && silentFrames >= SAMPLE_RATE) break
                    if (output.size() >= SAMPLE_RATE * 2 * MAX_CAPTURE_SECONDS) break
                }
                currentCoroutineContext().ensureActive()
                if (output.size() == 0) throw AudioOperationException(DeviceAudioErrorKind.NoSpeech, "No microphone audio was captured.")
                DeviceAudioCapture(pcm16ToWav(output.toByteArray(), SAMPLE_RATE), "audio/wav")
            } finally {
                recorder = null
                runCatching { native.stop() }
                native.release()
            }
        }

    companion object {
        const val SAMPLE_RATE = 16_000
        private const val MAX_CAPTURE_SECONDS = 30
        private const val WAV_HEADER_SIZE = 44
    }
}
