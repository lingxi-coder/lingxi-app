package com.lingxi.code.voice.offline

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineMoonshineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineRecognizer
import com.k2fsa.sherpa.onnx.OnlineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineTransducerModelConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.coroutines.coroutineContext
import kotlin.math.sqrt

/**
 * On-device speech-to-text via sherpa-onnx (offline pack). Wraps the
 * com.k2fsa.sherpa.onnx API (config shapes mirror ~/lingxi/android's
 * RealSherpaOnnxEngine). One instance per STT model id; built lazily and reused.
 *
 * [transcribeOnce] records one utterance from the mic (16 kHz PCM16) with a
 * simple RMS VAD (stop after trailing silence) and returns the recognized text.
 * Requires RECORD_AUDIO (the orb requests it before calling).
 */
class SherpaStt private constructor(
    private val online: OnlineRecognizer?,
    private val offline: OfflineRecognizer?,
) {
    /**
     * Record one utterance and transcribe it. The UI checks RECORD_AUDIO before
     * entering this method; the constructor is still guarded because permission
     * can be revoked between that check and opening the recorder.
     */
    @SuppressLint("MissingPermission")
    suspend fun transcribeOnce(): String? = withContext(Dispatchers.IO) {
        val sampleRate = 16_000
        val minBuf = AudioRecord.getMinBufferSize(sampleRate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record = try {
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_RECOGNITION, sampleRate,
                AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                maxOf(minBuf, sampleRate),
            )
        } catch (_: SecurityException) {
            return@withContext null
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) { record.release(); return@withContext null }
        val all = ArrayList<Float>(sampleRate * 6)
        try {
            record.startRecording()
            val frame = 1600 // 100 ms
            val buf = ShortArray(frame)
            var ambient = 0f; var calibrated = 0
            var voiced = 0; var silence = 0; var started = false
            val maxFrames = 120 // ~12 s hard cap
            var frames = 0
            while (frames < maxFrames) {
                coroutineContext.ensureActive()
                val n = record.read(buf, 0, frame)
                if (n <= 0) continue
                var sum = 0.0
                for (i in 0 until n) { val f = buf[i] / 32768f; all.add(f); sum += (f * f).toDouble() }
                val rms = sqrt(sum / n).toFloat()
                frames++
                if (calibrated < 10) { ambient += rms; calibrated++; continue } // 1s noise floor
                val thr = maxOf(0.02f, 1.8f * (ambient / 10f))
                if (rms > thr) { voiced++; silence = 0; if (voiced >= 3) started = true }
                else if (started) { silence++; if (silence >= 12) break } // ~1.2s trailing silence ends turn
            }
        } catch (_: Throwable) {
            return@withContext null
        } finally {
            try { record.stop() } catch (_: Throwable) {}
            record.release()
        }
        if (all.size < sampleRate / 2) return@withContext null // < 0.5s → ignore
        val pcm = FloatArray(all.size) { all[it] }
        decode(pcm, sampleRate).trim().ifEmpty { null }
    }

    private fun decode(pcm: FloatArray, sampleRate: Int): String {
        online?.let { rec ->
            val s = rec.createStream("")
            s.acceptWaveform(pcm, sampleRate)
            s.inputFinished()
            while (rec.isReady(s)) rec.decode(s)
            val text = rec.getResult(s).text
            s.release()
            return text
        }
        offline?.let { rec ->
            val s = rec.createStream()
            s.acceptWaveform(pcm, sampleRate)
            rec.decode(s)
            val text = rec.getResult(s).text
            s.release()
            return text
        }
        return ""
    }

    fun close() { online?.release(); offline?.release() }

    companion object {
        /** Build the recognizer for an STT [entry] whose files are unpacked in [modelDir]. */
        fun load(entry: OfflineModelEntry, modelDir: File): SherpaStt = when (val p = entry.runtimeParams) {
            is SherpaRuntimeParams.Asr.OnlineTransducer -> {
                val cfg = OnlineRecognizerConfig().apply {
                    featConfig = FeatureConfig().apply { sampleRate = entry.sampleRateHz; featureDim = 80 }
                    modelConfig = OnlineModelConfig().apply {
                        transducer = OnlineTransducerModelConfig().apply {
                            encoder = File(modelDir, "encoder-epoch-99-avg-1.int8.onnx").absolutePath
                            decoder = File(modelDir, "decoder-epoch-99-avg-1.onnx").absolutePath
                            joiner = File(modelDir, "joiner-epoch-99-avg-1.int8.onnx").absolutePath
                        }
                        tokens = File(modelDir, "tokens.txt").absolutePath
                        numThreads = p.numThreads
                        provider = "cpu"
                    }
                    enableEndpoint = false
                    decodingMethod = p.decoding
                }
                SherpaStt(online = OnlineRecognizer(assetManager = null, config = cfg), offline = null)
            }
            is SherpaRuntimeParams.Asr.OfflineMoonshine -> {
                val cfg = OfflineRecognizerConfig().apply {
                    featConfig = FeatureConfig().apply { sampleRate = entry.sampleRateHz; featureDim = 80 }
                    modelConfig = OfflineModelConfig().apply {
                        moonshine = OfflineMoonshineModelConfig().apply {
                            preprocessor = File(modelDir, "preprocess.onnx").absolutePath
                            encoder = File(modelDir, "encode.int8.onnx").absolutePath
                            uncachedDecoder = File(modelDir, "uncached_decode.int8.onnx").absolutePath
                            cachedDecoder = File(modelDir, "cached_decode.int8.onnx").absolutePath
                        }
                        tokens = File(modelDir, "tokens.txt").absolutePath
                        numThreads = p.numThreads
                        provider = "cpu"
                    }
                }
                SherpaStt(online = null, offline = OfflineRecognizer(assetManager = null, config = cfg))
            }
            else -> error("SherpaStt.load: ${entry.id} is not an STT model")
        }
    }
}
