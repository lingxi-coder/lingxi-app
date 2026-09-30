package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.client.AudioErrorDto
import com.lingxi.code.bindings.client.AudioErrorKindDto
import com.lingxi.code.bindings.client.AudioOperationResultDto
import com.lingxi.code.bindings.client.AudioStatusDto
import java.util.Base64

/** Pure result projection shared by the generated callback and JVM contract tests. */
internal object AndroidAudioResultDtoMapper {
    fun toDto(result: DeviceAudioResult, maxPayloadBytes: Long): AudioOperationResultDto {
        return when (result) {
        is DeviceAudioResult.RecordingStarted -> AudioOperationResultDto.RecordingStarted(result.handle)
        is DeviceAudioResult.Recording -> {
            val audioBase64 = result.bytes.toBoundedBase64(maxPayloadBytes)
                ?: return failed(AudioErrorKindDto.MEDIA_TOO_LARGE, "recording exceeds the published audio payload limit")
            AudioOperationResultDto.Recording(audioBase64, result.mimeType)
        }
        is DeviceAudioResult.Transcript -> AudioOperationResultDto.Transcript(
            result.text,
            result.language,
            result.confidence,
        )
        is DeviceAudioResult.Synthesized -> {
            if (result.sampleRateHz !in 1..MAX_AUDIO_SAMPLE_RATE_HZ || result.pcm.isEmpty() || result.pcm.size % 2 != 0) {
                return failed(AudioErrorKindDto.SYNTHESIS_FAILED, "synthesis returned invalid PCM16 audio")
            }
            val pcmBase64 = result.pcm.toBoundedBase64(maxPayloadBytes)
                ?: return failed(AudioErrorKindDto.MEDIA_TOO_LARGE, "synthesized audio exceeds the published payload limit")
            AudioOperationResultDto.Synthesized(pcmBase64, result.sampleRateHz.toUInt())
        }
        is DeviceAudioResult.PlaybackCompleted -> AudioOperationResultDto.PlaybackCompleted(
            result.durationMs.coerceAtLeast(0L).toULong(),
        )
        is DeviceAudioResult.OffloadMedia -> failed(
            AudioErrorKindDto.INVALID_REQUEST,
            "internal media playback result cannot cross the audio callback",
        )
        is DeviceAudioResult.Status -> AudioOperationResultDto.Status(
            AudioStatusDto(result.recording, result.playing),
        )
        DeviceAudioResult.OwnerEnded -> AudioOperationResultDto.OwnerEnded
        is DeviceAudioResult.Failed -> failed(result.error.kind.toDto(), result.error.message)
        }
    }

    fun failed(kind: AudioErrorKindDto, message: String) = AudioOperationResultDto.Failed(
        AudioErrorDto(kind, message.take(MAX_ERROR_MESSAGE_LENGTH)),
    )

    private fun ByteArray.toBoundedBase64(maxPayloadBytes: Long): String? {
        if (maxPayloadBytes < 0L || size.toLong() > maxPayloadBytes) return null
        val maxEncodedLength = ((maxPayloadBytes + 2L) / 3L) * 4L
        val encoded = Base64.getEncoder().encodeToString(this)
        return encoded.takeIf { it.length.toLong() <= maxEncodedLength }
    }

    private fun DeviceAudioErrorKind.toDto(): AudioErrorKindDto = when (this) {
        DeviceAudioErrorKind.PermissionDenied -> AudioErrorKindDto.PERMISSION_DENIED
        DeviceAudioErrorKind.Busy -> AudioErrorKindDto.BUSY
        DeviceAudioErrorKind.Cancelled -> AudioErrorKindDto.CANCELLED
        DeviceAudioErrorKind.Timeout -> AudioErrorKindDto.TIMEOUT
        DeviceAudioErrorKind.NoSpeech -> AudioErrorKindDto.NO_SPEECH
        DeviceAudioErrorKind.NotRecording -> AudioErrorKindDto.NOT_RECORDING
        DeviceAudioErrorKind.Unavailable -> AudioErrorKindDto.UNAVAILABLE
        DeviceAudioErrorKind.Unsupported -> AudioErrorKindDto.UNSUPPORTED
        DeviceAudioErrorKind.ModelMissing -> AudioErrorKindDto.MODEL_MISSING
        DeviceAudioErrorKind.VoiceMissing -> AudioErrorKindDto.VOICE_MISSING
        DeviceAudioErrorKind.InvalidRequest -> AudioErrorKindDto.INVALID_REQUEST
        DeviceAudioErrorKind.SynthesisFailed -> AudioErrorKindDto.SYNTHESIS_FAILED
        DeviceAudioErrorKind.NativeFailure -> AudioErrorKindDto.NATIVE_FAILURE
        DeviceAudioErrorKind.MediaTooLarge -> AudioErrorKindDto.MEDIA_TOO_LARGE
    }

    private const val MAX_AUDIO_SAMPLE_RATE_HZ = 768_000
    private const val MAX_ERROR_MESSAGE_LENGTH = 1_024
}
