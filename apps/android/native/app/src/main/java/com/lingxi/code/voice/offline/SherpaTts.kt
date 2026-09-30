package com.lingxi.code.voice.offline

import com.lingxi.code.voice.audio.AudioDriverException
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.k2fsa.sherpa.onnx.OfflineTts
import com.k2fsa.sherpa.onnx.OfflineTtsConfig
import com.k2fsa.sherpa.onnx.OfflineTtsKittenModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsVitsModelConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.withContext
import java.io.File
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.coroutineContext

/**
 * On-device text-to-speech via sherpa-onnx (offline pack). Returns bounded
 * PCM16 to the app-scoped audio service; this class never owns playback.
 */
class SherpaTts private constructor(
    private val tts: OfflineTts,
    private val sampleRateHz: Int,
) {
    suspend fun renderToPcm(
        text: String,
        sid: Int = 0,
        speed: Float = 1.0f,
        maxPcmBytes: Int,
    ): Pair<ByteArray, Int> = withContext(Dispatchers.IO) {
        require(maxPcmBytes >= 0) { "audio payload limit must not be negative" }
        if (text.isBlank()) return@withContext ByteArray(0) to sampleRateHz
        val pcm = ByteArrayOutputStream()
        var collectedBytes = 0L
        val tooLarge = AtomicBoolean(false)
        val interrupted = AtomicBoolean(false)
        val operationJob = coroutineContext[Job]
        tts.generateWithCallback(text = text, sid = sid, speed = speed) { samples ->
            if (operationJob?.isActive == false) {
                interrupted.set(true)
                0
            } else if (tooLarge.get() || interrupted.get()) {
                0
            } else {
                val chunkBytes = samples.size.toLong() * 2L
                if (collectedBytes + chunkBytes > maxPcmBytes.toLong()) {
                    tooLarge.set(true)
                    0
                } else {
                    pcm.write(samples.toPcm16Bytes())
                    collectedBytes += chunkBytes
                    1
                }
            }
        }
        if (interrupted.get() || operationJob?.isActive == false) {
            throw kotlinx.coroutines.CancellationException("offline speech synthesis was cancelled")
        }
        if (tooLarge.get()) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "synthesized audio exceeds the payload limit"))
        }
        pcm.toByteArray() to sampleRateHz
    }

    private fun FloatArray.toPcm16Bytes(): ByteArray {
        val bytes = ByteArray(size * 2)
        val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        for (sample in this) {
            val clamped = sample.coerceIn(-1f, 1f)
            buffer.putShort((clamped * Short.MAX_VALUE).toInt().toShort())
        }
        return bytes
    }

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
