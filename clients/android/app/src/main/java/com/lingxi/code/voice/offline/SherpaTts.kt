package com.lingxi.code.voice.offline

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import com.k2fsa.sherpa.onnx.OfflineTts
import com.k2fsa.sherpa.onnx.OfflineTtsConfig
import com.k2fsa.sherpa.onnx.OfflineTtsKittenModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsVitsModelConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * On-device text-to-speech via sherpa-onnx (offline pack). Wraps OfflineTts
 * (config mirrors ~/lingxi/android's RealSherpaOnnxEngine) and streams the
 * generated PCM-float samples straight to an [AudioTrack] as they arrive.
 */
class SherpaTts private constructor(
    private val tts: OfflineTts,
    private val sampleRateHz: Int,
) {
    @Volatile private var track: AudioTrack? = null
    @Volatile private var stopped = false

    /** Synthesize [text] and play it. [sid] selects the voice (0 = first). Blocks until done. */
    suspend fun speak(text: String, sid: Int = 0, speed: Float = 1.0f) = withContext(Dispatchers.IO) {
        if (text.isBlank()) return@withContext
        stopped = false
        val at = AudioTrack(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build(),
            AudioFormat.Builder()
                .setSampleRate(sampleRateHz)
                .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                .build(),
            maxOf(
                AudioTrack.getMinBufferSize(sampleRateHz, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_FLOAT),
                sampleRateHz * 4,
            ),
            AudioTrack.MODE_STREAM,
            AudioManager.AUDIO_SESSION_ID_GENERATE,
        )
        track = at
        try {
            at.play()
            // sherpa callback returns 1 to continue, 0 to stop.
            tts.generateWithCallback(text = text, sid = sid, speed = speed) { samples ->
                if (stopped) 0 else {
                    at.write(samples, 0, samples.size, AudioTrack.WRITE_BLOCKING)
                    1
                }
            }
        } catch (_: Throwable) {
            // best-effort playback
        } finally {
            try { at.stop() } catch (_: Throwable) {}
            at.release()
            track = null
        }
    }

    /** Abort any in-flight playback (barge-in). */
    fun stop() {
        stopped = true
        track?.let { try { it.pause(); it.flush() } catch (_: Throwable) {} }
    }

    fun close() { stop(); tts.release() }

    companion object {
        fun load(entry: OfflineModelEntry, modelDir: File): SherpaTts {
            // Optional companion assets the multi-lingual packs ship (guarded by
            // presence): espeak-ng-data (English phonemes), dict + lexicons (zh),
            // and number/date rule FSTs.
            fun dir(name: String) = File(modelDir, name).takeIf { it.isDirectory }?.absolutePath
            fun joinExisting(vararg names: String) =
                names.map { File(modelDir, it) }.filter { it.exists() }.joinToString(",") { it.absolutePath }

            val modelCfg = OfflineTtsModelConfig().apply {
                when (val p = entry.runtimeParams) {
                    is SherpaRuntimeParams.Tts.Vits -> vits = OfflineTtsVitsModelConfig().apply {
                        model = File(modelDir, "model.int8.onnx").absolutePath
                        tokens = File(modelDir, "tokens.txt").absolutePath
                        lexicon = File(modelDir, "lexicon.txt").absolutePath
                        dir("dict")?.let { dictDir = it }
                    }
                    is SherpaRuntimeParams.Tts.Kitten -> kitten = OfflineTtsKittenModelConfig().apply {
                        model = File(modelDir, "model.fp16.onnx").absolutePath
                        voices = File(modelDir, "voices.bin").absolutePath
                        tokens = File(modelDir, "tokens.txt").absolutePath
                        dir("espeak-ng-data")?.let { dataDir = it }
                    }
                    else -> error("SherpaTts.load: ${entry.id} is not a TTS model")
                }
                numThreads = when (val p = entry.runtimeParams) {
                    is SherpaRuntimeParams.Tts.Vits -> p.numThreads
                    is SherpaRuntimeParams.Tts.Kitten -> p.numThreads
                    else -> 2
                }
                provider = "cpu"
            }
            val cfg = OfflineTtsConfig().apply {
                model = modelCfg
                // Chinese number/date/phone text-normalization rules (if shipped).
                joinExisting(
                    "phone.fst", "date.fst", "number.fst",
                ).takeIf { it.isNotEmpty() }?.let { ruleFsts = it }
            }
            return SherpaTts(OfflineTts(assetManager = null, config = cfg), entry.sampleRateHz)
        }
    }
}
