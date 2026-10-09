package com.lingxi.code.voice.audio

import android.content.Context
import com.lingxi.code.bindings.android.AndroidAudioFfiException
import com.lingxi.code.bindings.android.AndroidAudioService
import com.lingxi.code.bindings.client.AudioCapabilitySnapshotDto
import com.lingxi.code.bindings.client.AudioErrorKindDto
import com.lingxi.code.bindings.client.AudioOperationDto
import com.lingxi.code.bindings.client.AudioOperationKindDto
import com.lingxi.code.bindings.client.AudioOperationReadinessDto
import com.lingxi.code.bindings.client.AudioOperationRequestDto
import com.lingxi.code.bindings.client.AudioOperationResultDto
import com.lingxi.code.bindings.client.AudioOwnerDto
import com.lingxi.code.bindings.client.AudioReadinessStateDto
import kotlinx.coroutines.CancellationException
import java.util.UUID

/** Maps the crate-local UniFFI callback onto the process-wide Android audio service. */
internal class AndroidNativeAudioServiceAdapter(context: Context) : AndroidAudioService {
    private val appContext = context.applicationContext

    override fun capabilities(): AudioCapabilitySnapshotDto =
        AndroidAudioServiceProvider.get(appContext).capabilities().toDto()

    override suspend fun execute(request: AudioOperationRequestDto): AudioOperationResultDto {
        val identity = request.identity.toIdentityOrNull()
            ?: return failed(AudioErrorKindDto.INVALID_REQUEST, "audio operation identity is invalid")
        val owner = request.owner.toOwnerOrNull()
            ?: return failed(AudioErrorKindDto.INVALID_REQUEST, "audio owner is invalid")
        val maxPayloadBytes = request.maxPayloadBytes.toSafeLongOrNull()
            ?: return failed(AudioErrorKindDto.INVALID_REQUEST, "audio payload limit is invalid")
        val timeoutBudgetMs = request.timeoutBudgetMs?.toSafeLongOrNull()
            ?: if (request.timeoutBudgetMs == null) null else {
                return failed(AudioErrorKindDto.INVALID_REQUEST, "audio operation timeout is invalid")
            }
        val operation = request.operation.toDeviceOperation()
            ?: return failed(AudioErrorKindDto.INVALID_REQUEST, "audio operation arguments are invalid")
        return AndroidAudioServiceProvider.get(appContext).execute(
            DeviceAudioRequest(
                identity = identity,
                owner = owner,
                timeoutBudgetMs = timeoutBudgetMs,
                maxPayloadBytes = maxPayloadBytes,
                operation = operation,
            ),
        ).let { AndroidAudioResultDtoMapper.toDto(it, maxPayloadBytes) }
    }

    override suspend fun cancel(identity: com.lingxi.code.bindings.client.AudioOperationIdDto) {
        val converted = identity.toIdentityOrNull()
            ?: throw AndroidAudioFfiException.NativeFailure("audio operation identity is invalid")
        try {
            AndroidAudioServiceProvider.get(appContext).cancel(converted)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            throw AndroidAudioFfiException.NativeFailure(error.message ?: "audio operation could not be cancelled")
        }
    }

    internal fun diagnostics(): DeviceAudioServiceDiagnostics =
        AndroidAudioServiceProvider.get(appContext).diagnostics()

    private fun DeviceAudioCapabilities.toDto() = AudioCapabilitySnapshotDto(
        serviceEpoch = serviceEpoch.toULong(),
        supportRevision = supportRevision.toULong(),
        supportedOperations = supported.sortedBy { it.ordinal }.map { it.toDto() },
        readiness = readiness.entries
            .sortedBy { it.key.ordinal }
            .map { (operation, state) -> AudioOperationReadinessDto(operation.toDto(), state.toDto()) },
        maxPayloadBytes = maxPayloadBytes.toULong(),
    )

    private fun DeviceAudioOperationKind.toDto(): AudioOperationKindDto = when (this) {
        DeviceAudioOperationKind.RECORD -> AudioOperationKindDto.RECORD
        DeviceAudioOperationKind.LISTEN -> AudioOperationKindDto.LISTEN
        DeviceAudioOperationKind.SYNTHESIZE -> AudioOperationKindDto.SYNTHESIZE
        DeviceAudioOperationKind.SPEAK -> AudioOperationKindDto.SPEAK
    }

    private fun DeviceAudioReadiness.toDto(): AudioReadinessStateDto = when (this) {
        DeviceAudioReadiness.READY -> AudioReadinessStateDto.READY
        DeviceAudioReadiness.NEEDS_PERMISSION -> AudioReadinessStateDto.NEEDS_PERMISSION
        DeviceAudioReadiness.BUSY -> AudioReadinessStateDto.BUSY
        DeviceAudioReadiness.MISSING_MODEL -> AudioReadinessStateDto.MISSING_MODEL
        DeviceAudioReadiness.UNAVAILABLE -> AudioReadinessStateDto.UNAVAILABLE
    }

    private fun AudioOwnerDto.toOwnerOrNull(): AudioOwnerKey? = when (this) {
        is AudioOwnerDto.Session -> AudioOwnerKey.session(sessionId).takeIf { sessionId.isNotBlank() }
        is AudioOwnerDto.Ui -> instanceId.takeIf { it.isNotBlank() }?.let(AudioOwnerKey::ui)
        is AudioOwnerDto.System -> instanceId.takeIf { it.isNotBlank() }?.let(AudioOwnerKey::system)
    }

    private fun AudioOperationDto.toDeviceOperation(): DeviceAudioOperation? = when (this) {
        is AudioOperationDto.StartRecording -> sampleRateHz.toLong()
            .takeIf { it <= Int.MAX_VALUE }
            ?.let { DeviceAudioOperation.StartRecording(it.toInt(), format) }
        is AudioOperationDto.StopRecording -> DeviceAudioOperation.StopRecording(handle)
        is AudioOperationDto.Listen -> DeviceAudioOperation.Listen(language)
        is AudioOperationDto.Synthesize -> DeviceAudioOperation.Synthesize(text, language, rate, voice)
        is AudioOperationDto.Speak -> DeviceAudioOperation.Speak(text, language, rate, voice)
        is AudioOperationDto.Status -> DeviceAudioOperation.Status(handle)
        AudioOperationDto.EndOwner -> DeviceAudioOperation.EndOwner
    }

    private fun failed(kind: AudioErrorKindDto, message: String) = AndroidAudioResultDtoMapper.failed(kind, message)

    private fun com.lingxi.code.bindings.client.AudioOperationIdDto.toIdentityOrNull(): AudioOperationIdentity? {
        if (generation > MAX_SAFE_INTEGER_ULONG || serviceEpoch > MAX_SAFE_INTEGER_ULONG) return null
        if (id.isBlank() || runCatching { UUID.fromString(id).version() }.getOrNull() != 4) return null
        return AudioOperationIdentity(id, generation.toLong(), serviceEpoch.toLong())
    }

    private fun ULong.toSafeLongOrNull(): Long? = takeIf { it <= MAX_SAFE_INTEGER_ULONG }?.toLong()

    private companion object {
        const val MAX_SAFE_INTEGER_ULONG = 9_007_199_254_740_991uL
    }
}
