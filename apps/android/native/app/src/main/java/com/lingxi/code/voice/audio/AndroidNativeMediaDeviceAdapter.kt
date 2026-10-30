package com.lingxi.code.voice.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Size
import androidx.core.content.ContextCompat
import com.lingxi.code.bindings.android.AndroidMediaDevicePorts
import com.lingxi.code.bindings.android.AndroidMediaDeviceCapabilities
import com.lingxi.code.bindings.android.AndroidMediaDeviceResponse
import com.lingxi.code.bindings.client.MediaDeviceAudioControlRequestDto
import com.lingxi.code.bindings.client.MediaDeviceAudioWriteRequestDto
import com.lingxi.code.bindings.client.MediaDeviceLeaseDto
import com.lingxi.code.bindings.client.MediaDeviceOpenRequestDto
import com.lingxi.code.bindings.client.MediaDeviceRequestDto
import com.lingxi.code.bindings.client.MediaDeviceRequestEnvelopeDto
import com.lingxi.code.bindings.client.MediaDeviceResponseDto
import com.lingxi.code.bindings.client.MediaDeviceResultDto
import com.lingxi.code.bindings.client.MediaDeviceUpdateRequestDto
import com.lingxi.code.bindings.client.MediaDeviceVideoWriteRequestDto
import com.lingxi.code.bindings.client.MediaErrorDto
import com.lingxi.code.bindings.client.MediaErrorKindDto
import com.lingxi.code.bindings.client.MediaFrameDto
import com.lingxi.code.bindings.client.MediaFramePayloadDto
import com.lingxi.code.bindings.client.MediaKindDto
import com.lingxi.code.bindings.client.MediaPcmFrameDto
import com.lingxi.code.bindings.client.MediaRenderReceiptDto
import com.lingxi.code.bindings.client.MediaSessionIntentDto
import com.lingxi.code.bindings.client.MediaSessionModeDto
import com.lingxi.code.bindings.client.MediaSessionStateDto
import com.lingxi.code.bindings.client.MediaSessionStatusDto
import com.lingxi.code.bindings.client.MediaSessionUpdateDto
import com.lingxi.code.bindings.client.MediaSourceRefDto
import com.lingxi.code.bindings.client.MediaVideoFrameDto
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withContext
import java.nio.charset.StandardCharsets
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.atomic.AtomicLong
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

internal fun androidNativeMediaSupportedSessionModes(): List<MediaSessionModeDto> = listOf(
    MediaSessionModeDto.AUDIO,
    MediaSessionModeDto.VIDEO,
    MediaSessionModeDto.AUDIO_VIDEO,
)

internal fun androidMediaNeedsPlayback(
    mode: MediaSessionModeDto,
    intent: MediaSessionIntentDto,
): Boolean =
    (mode == MediaSessionModeDto.AUDIO || mode == MediaSessionModeDto.AUDIO_VIDEO) &&
        intent == MediaSessionIntentDto.CONVERSATION

/** Android implementation of the versioned host-only media device callback. */
internal class AndroidNativeMediaDeviceAdapter(context: Context) : AndroidMediaDevicePorts {
    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val leases = ConcurrentHashMap<String, AndroidMediaLease>()
    private val openMutex = Mutex()
    private val pendingOpenCleanup = AndroidMediaOpenCleanupRegistry()
    private val openLeasesByIdentity = mutableMapOf<MediaOpenKey, OpenLease>()
    private val openIdentitiesByLease = mutableMapOf<String, MediaOpenKey>()
    private val confirmedClosedLeases = LinkedHashSet<String>()
    private var cameraLeaseID: String? = null

    override fun deviceCapabilities(): AndroidMediaDeviceCapabilities =
        AndroidMediaDeviceCapabilities(sessionModes = androidNativeMediaSupportedSessionModes())

    private data class MediaOpenKey(
        val owner: AudioOwnerKey,
        val operationId: String,
        val generation: ULong,
        val serviceEpoch: ULong,
    )

    private data class OpenSignature(
        val mode: MediaSessionModeDto,
        val intent: MediaSessionIntentDto,
        val audioSourceID: String?,
        val videoSourceID: String?,
        val timeoutBudgetMs: ULong?,
        val maxPayloadBytes: ULong,
    )

    private data class OpenLease(val leaseID: String, val signature: OpenSignature)

    override suspend fun request(request: MediaDeviceRequestEnvelopeDto): AndroidMediaDeviceResponse =
        AndroidMediaDeviceResponse(response = handleRequest(request))

    private suspend fun handleRequest(request: MediaDeviceRequestEnvelopeDto): MediaDeviceResponseDto {
        return try {
            val result = when (val operation = request.request) {
                is MediaDeviceRequestDto.Open -> open(operation.request)
                is MediaDeviceRequestDto.NextInput -> nextInput(operation.leaseId)
                is MediaDeviceRequestDto.WriteAudio -> writeAudio(operation.request)
                is MediaDeviceRequestDto.WriteVideo -> writeVideo(operation.request)
                is MediaDeviceRequestDto.FlushAudio -> flushAudio(operation.request)
                is MediaDeviceRequestDto.InterruptAudio -> interruptAudio(operation.request)
                is MediaDeviceRequestDto.Update -> update(operation.request)
                is MediaDeviceRequestDto.Stop -> stop(operation.leaseId)
            }
            MediaDeviceResponseDto(request.requestId, result)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: AndroidMediaOpenRejectedWithoutResource) {
            val failure = error.failure
            MediaDeviceResponseDto(
                request.requestId,
                MediaDeviceResultDto.OpenFailedWithoutResource(
                    MediaErrorDto(failure.kind, truncateUtf8(failure.message ?: "Android media open rejected", MAX_ERROR_BYTES)),
                ),
            )
        } catch (error: AndroidMediaDeviceFailure) {
            failure(request.requestId, error.kind, error.message ?: "Android media operation failed")
        } catch (error: AudioOperationException) {
            failure(request.requestId, error.kind.toMediaErrorKind(), error.message ?: "Android audio operation failed")
        } catch (error: AudioDriverException) {
            failure(request.requestId, error.error.kind.toMediaErrorKind(), error.error.message)
        } catch (error: SecurityException) {
            failure(request.requestId, MediaErrorKindDto.PERMISSION_DENIED, "Android media permission is required")
        } catch (error: Throwable) {
            failure(request.requestId, MediaErrorKindDto.NATIVE_FAILURE, error.message ?: "Android media operation failed")
        }
    }

    private suspend fun open(request: MediaDeviceOpenRequestDto): MediaDeviceResultDto.Opened = openMutex.withLock {
        // A failed partial open keeps the adapter-wide device lane reserved until
        // its same cleanup handle receives a close acknowledgement.
        pendingOpenCleanup.retryPending()
        val session = request.request
        val owner = request.owner.toAudioOwnerKeyOrNull()
            ?: throw AndroidMediaOpenRejectedWithoutResource(AndroidMediaDeviceFailure(MediaErrorKindDto.INVALID_REQUEST, "media owner is invalid"))
        val key = MediaOpenKey(
            owner = owner,
            operationId = request.identity.id,
            generation = request.identity.generation,
            serviceEpoch = request.identity.serviceEpoch,
        )
        val signature = OpenSignature(
            mode = session.mode,
            intent = session.intent,
            audioSourceID = session.audioSource?.id,
            videoSourceID = session.videoSource?.id,
            timeoutBudgetMs = request.timeoutBudgetMs,
            maxPayloadBytes = request.maxPayloadBytes,
        )
        openLeasesByIdentity[key]?.let { existing ->
            if (existing.signature != signature) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.INVALID_REQUEST, "media operation identity was reused with different settings")
            }
            val existingLease = leases[existing.leaseID]
            if (existingLease != null) {
                if (existingLease.stopState != AndroidMediaStopState.ACTIVE) {
                    throw AndroidMediaDeviceFailure(MediaErrorKindDto.BUSY, "the previous Android media lease is still stopping")
                }
                return@withLock MediaDeviceResultDto.Opened(MediaDeviceLeaseDto(existing.leaseID))
            }
            openLeasesByIdentity.remove(key)
        }
        if (request.maxPayloadBytes == 0uL || request.maxPayloadBytes > Long.MAX_VALUE.toULong()) {
            throw AndroidMediaOpenRejectedWithoutResource(AndroidMediaDeviceFailure(MediaErrorKindDto.INVALID_REQUEST, "media payload limit is outside the supported range"))
        }
        val maxPayloadBytes = request.maxPayloadBytes.toLong()
        val wantsAudio = session.mode == MediaSessionModeDto.AUDIO || session.mode == MediaSessionModeDto.AUDIO_VIDEO
        val wantsVideo = session.mode == MediaSessionModeDto.VIDEO || session.mode == MediaSessionModeDto.AUDIO_VIDEO
        val wantsPlayback = androidMediaNeedsPlayback(session.mode, session.intent)
        if (wantsAudio && session.audioSource?.id != MICROPHONE_SOURCE_ID) {
            throw AndroidMediaOpenRejectedWithoutResource(AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "Android media supports the app's default microphone source"))
        }
        if (wantsVideo && session.videoSource?.id != CAMERA_SOURCE_ID) {
            throw AndroidMediaOpenRejectedWithoutResource(AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "Android media supports the app's default camera source"))
        }

        var audio: AndroidMediaAudioPortSession? = null
        var video: AndroidCameraVideoInput? = null
        var audioOpenDispatched = false
        var cameraOpenDispatched = false
        val leaseID = UUID.randomUUID().toString()
        try {
            val permissions = buildSet {
                if (wantsAudio) add(Manifest.permission.RECORD_AUDIO)
                if (wantsVideo) add(Manifest.permission.CAMERA)
            }.filterNot { permission ->
                ContextCompat.checkSelfPermission(appContext, permission) == PackageManager.PERMISSION_GRANTED
            }.toSet()
            if (permissions.isNotEmpty()) {
                when (AndroidMediaPermissionCoordinator.shared.request(permissions)) {
                    AndroidMediaPermissionOutcome.Granted -> Unit
                    AndroidMediaPermissionOutcome.Denied -> throw AndroidMediaDeviceFailure(
                        MediaErrorKindDto.PERMISSION_DENIED,
                        "Android media permission was denied",
                    )
                    AndroidMediaPermissionOutcome.Unavailable -> throw AndroidMediaDeviceFailure(
                        MediaErrorKindDto.UNAVAILABLE,
                        "Android media permission requires the app to be in the foreground",
                    )
                    AndroidMediaPermissionOutcome.Busy -> throw AndroidMediaDeviceFailure(
                        MediaErrorKindDto.BUSY,
                        "another Android permission request is already in progress",
                    )
                }
            }
            if (wantsAudio) {
                if (ContextCompat.checkSelfPermission(appContext, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                    throw AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android microphone permission is required for media capture")
                }
                audioOpenDispatched = true
                audio = AndroidAudioServiceProvider.openMediaAudioPort(
                    appContext,
                    owner,
                    capture = true,
                    playback = wantsPlayback,
                    sampleRateHz = DEVICE_SAMPLE_RATE_HZ,
                )
            }
            if (wantsVideo) {
                if (cameraLeaseID != null) {
                    throw AndroidMediaDeviceFailure(MediaErrorKindDto.BUSY, "another media session owns the Android camera")
                }
                cameraLeaseID = leaseID
                cameraOpenDispatched = true
                video = AndroidCameraVideoInput.open(appContext, maxPayloadBytes)
            }
            val lease = AndroidMediaLease(
                leaseId = leaseID,
                audio = audio,
                video = video,
                audioSource = session.audioSource,
                videoSource = session.videoSource,
                maxPayloadBytes = maxPayloadBytes,
            )
            audio = null
            video = null
            leases[leaseID] = lease
            openLeasesByIdentity[key] = OpenLease(leaseID, signature)
            openIdentitiesByLease[leaseID] = key
            return@withLock MediaDeviceResultDto.Opened(MediaDeviceLeaseDto(leaseID))
        } catch (error: Throwable) {
            val cameraOpenFailure = error as? AndroidCameraOpenFailure
            val cameraOpenClosed = error as? AndroidCameraOpenClosed
            val audioOpenClosed = error as? AndroidMediaAudioOpenClosed
            val openError = cameraOpenFailure?.original ?: cameraOpenClosed?.original ?: audioOpenClosed?.original ?: error
            val unresolvedCameraClose = cameraOpenFailure?.cleanup
            var partialCleanupConfirmed = false
            if (audio != null || video != null || unresolvedCameraClose != null) {
                val partialAudio = audio
                val partialVideo = video
                val cleanup = AndroidMediaOpenCleanup {
                    var cleanupFailure: Throwable? = null
                    try { partialAudio?.stop() }
                    catch (closeError: Throwable) { cleanupFailure = closeError }
                    try { partialVideo?.close() }
                    catch (closeError: Throwable) { if (cleanupFailure == null) cleanupFailure = closeError }
                    try { unresolvedCameraClose?.close() }
                    catch (closeError: Throwable) { if (cleanupFailure == null) cleanupFailure = closeError }
                    if (cleanupFailure == null && cameraLeaseID == leaseID) cameraLeaseID = null
                    cleanupFailure?.let { throw it }
                }
                try {
                    pendingOpenCleanup.retainAndRetry(cleanup)
                    partialCleanupConfirmed = true
                } catch (cleanupError: Throwable) {
                    if (openError is CancellationException) {
                        openError.addSuppressed(cleanupError)
                        throw openError
                    }
                    throw AndroidMediaDeviceFailure(
                        MediaErrorKindDto.NATIVE_FAILURE,
                        "Android media open failed and partial resource cleanup is still unconfirmed",
                    )
                }
            } else if (cameraLeaseID == leaseID && (!cameraOpenDispatched || cameraOpenClosed != null)) {
                cameraLeaseID = null
            }
            if (openError is CancellationException) throw openError
            val audioClosed = !audioOpenDispatched || audioOpenClosed != null ||
                (audio != null && partialCleanupConfirmed)
            val cameraClosed = !cameraOpenDispatched || cameraOpenClosed != null ||
                ((video != null || unresolvedCameraClose != null) && partialCleanupConfirmed)
            if (audioClosed && cameraClosed) {
                throw AndroidMediaOpenRejectedWithoutResource(openError.asMediaDeviceFailure())
            }
            if (openError is SecurityException) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android media permission is required")
            }
            throw openError
        }
    }

    private suspend fun nextInput(leaseId: String): MediaDeviceResultDto.Input {
        val lease = requireLease(leaseId)
        while (true) {
            val received = lease.frames.receiveCatching()
            received.exceptionOrNull()?.let { cause ->
                if (cause is CancellationException) throw cause
                if (cause is AndroidMediaDeviceFailure) throw cause
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.NATIVE_FAILURE, cause.message ?: "Android media input failed")
            }
            when (val input = received.getOrNull()) {
                null -> return MediaDeviceResultDto.Input(null)
                is AndroidMediaInput.Failure -> throw input.failure
                is AndroidMediaInput.Frame -> {
                    val frame = input.frame
                    when (frame.payload) {
                        is MediaFramePayloadDto.Audio -> {
                            if (!lease.audioEnabled || input.audioGeneration != lease.audioGeneration) continue
                        }
                        is MediaFramePayloadDto.Video -> {
                            if (!lease.videoEnabled || input.videoGeneration != lease.videoGeneration) continue
                        }
                    }
                    return MediaDeviceResultDto.Input(frame)
                }
            }
        }
    }

    private suspend fun writeAudio(request: MediaDeviceAudioWriteRequestDto): MediaDeviceResultDto.AudioWritten {
        val lease = requireLease(request.leaseId)
        val audio = lease.audio ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media session has no audio output")
        if (!lease.audioEnabled) throw AndroidMediaDeviceFailure(MediaErrorKindDto.BUSY, "media audio output is disabled")
        val frame = request.frame
        if (frame.pcm16le.size.toLong() > lease.maxPayloadBytes) {
            throw AndroidMediaDeviceFailure(MediaErrorKindDto.MEDIA_TOO_LARGE, "audio output frame exceeds the host payload limit")
        }
        val receipt = audio.writeAudio(frame.pcm16le, frame.sampleRateHz.toInt(), frame.channels.toInt())
        return MediaDeviceResultDto.AudioWritten(
            MediaRenderReceiptDto(receipt.acceptedSampleFrames.toUInt(), receipt.playedSampleFrames.toUInt(), receipt.renderedAtUnixMs?.toULong()),
        )
    }

    private fun writeVideo(request: MediaDeviceVideoWriteRequestDto): MediaDeviceResultDto {
        requireLease(request.leaseId)
        throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "Android media session has no native remote-video renderer")
    }

    private suspend fun flushAudio(request: MediaDeviceAudioControlRequestDto): MediaDeviceResultDto.AudioFlushed {
        val audio = requireLease(request.leaseId).audio
            ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media session has no audio output")
        val receipt = audio.flushAudio()
        return MediaDeviceResultDto.AudioFlushed(
            MediaRenderReceiptDto(0u, receipt.playedSampleFrames.toUInt(), receipt.renderedAtUnixMs?.toULong()),
        )
    }

    private suspend fun interruptAudio(request: MediaDeviceAudioControlRequestDto): MediaDeviceResultDto.AudioInterrupted {
        val audio = requireLease(request.leaseId).audio
            ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media session has no audio output")
        val receipt = audio.interruptAudio()
        return MediaDeviceResultDto.AudioInterrupted(
            MediaRenderReceiptDto(0u, receipt.playedSampleFrames.toUInt(), receipt.renderedAtUnixMs?.toULong()),
        )
    }

    private suspend fun update(request: MediaDeviceUpdateRequestDto): MediaDeviceResultDto.Updated {
        val lease = requireLease(request.leaseId)
        return lease.controlMutex.withLock {
            if (leases[request.leaseId] !== lease || lease.stopState != AndroidMediaStopState.ACTIVE) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.BUSY, "native media lease stopped before the update could be applied")
            }
            when (val update = request.update) {
                is MediaSessionUpdateDto.SetEnabled -> when (update.kind) {
                    MediaKindDto.AUDIO -> {
                        val audio = lease.audio
                            ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media lease has no audio source")
                        if (!update.enabled) lease.setAudioEnabled(false)
                        audio.setInputEnabled(update.enabled)
                        if (!update.enabled) audio.interruptAudio()
                        if (update.enabled) lease.setAudioEnabled(true)
                    }
                    MediaKindDto.VIDEO -> {
                        val video = lease.video
                            ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media lease has no video source")
                        if (!update.enabled) lease.setVideoEnabled(false)
                        video.setEnabled(update.enabled)
                        if (update.enabled) lease.setVideoEnabled(true)
                    }
                    MediaKindDto.IMAGE -> throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "live media image updates are unsupported")
                }
                is MediaSessionUpdateDto.SelectSource -> {
                    val expected = when (update.kind) {
                        MediaKindDto.AUDIO -> lease.audioSource?.id
                        MediaKindDto.VIDEO -> lease.videoSource?.id
                        MediaKindDto.IMAGE -> null
                    }
                    if (expected != update.source.id) {
                        throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "Android media source cannot be changed during an active lease")
                    }
                }
                is MediaSessionUpdateDto.SetPlaybackGain -> lease.audio?.setPlaybackGain(update.gain)
                    ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNSUPPORTED, "this media session has no playback path")
            }
            MediaDeviceResultDto.Updated(
                MediaSessionStatusDto(
                    state = MediaSessionStateDto.READY,
                    audioEnabled = lease.audioEnabled,
                    videoEnabled = lease.videoEnabled,
                ),
            )
        }
    }

    private suspend fun stop(leaseId: String): MediaDeviceResultDto.Stopped {
        return withContext(NonCancellable) {
            openMutex.withLock {
                val lease = leases[leaseId] ?: run {
                    if (leaseId in confirmedClosedLeases) return@withLock MediaDeviceResultDto.Stopped
                    throw AndroidMediaDeviceFailure(MediaErrorKindDto.NOT_FOUND, "native media lease has no confirmed stop receipt")
                }
                lease.controlMutex.withLock {
                    lease.stop {
                        val openKey = openIdentitiesByLease.remove(leaseId)
                        if (openKey != null) openLeasesByIdentity.remove(openKey)
                        if (cameraLeaseID == leaseId) cameraLeaseID = null
                        leases.remove(leaseId, lease)
                    }
                }
                confirmedClosedLeases.add(leaseId)
                if (confirmedClosedLeases.size > 512) {
                    confirmedClosedLeases.remove(confirmedClosedLeases.first())
                }
                MediaDeviceResultDto.Stopped
            }
        }
    }

    private fun requireLease(leaseId: String): AndroidMediaLease {
        val lease = leases[leaseId]
            ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.NOT_FOUND, "native media lease is not active")
        if (lease.stopState != AndroidMediaStopState.ACTIVE) {
            throw AndroidMediaDeviceFailure(MediaErrorKindDto.BUSY, "native media lease is still stopping")
        }
        return lease
    }

    private fun failure(requestId: String, kind: MediaErrorKindDto, message: String) =
        MediaDeviceResponseDto(
            requestId,
            MediaDeviceResultDto.Error(MediaErrorDto(kind, truncateUtf8(message, MAX_ERROR_BYTES))),
        )

    private inner class AndroidMediaLease(
        val leaseId: String,
        val audio: AndroidMediaAudioPortSession?,
        val video: AndroidCameraVideoInput?,
        val audioSource: MediaSourceRefDto?,
        val videoSource: MediaSourceRefDto?,
        val maxPayloadBytes: Long,
    ) {
        val frames = Channel<AndroidMediaInput>(capacity = 8)
        val controlMutex = Mutex()
        private val stopGate = AndroidMediaStopGate()
        val stopState: AndroidMediaStopState get() = stopGate.state
        private val sequence = AtomicLong(0)
        @Volatile var audioEnabled = audio != null
            private set
        @Volatile var videoEnabled = video != null
            private set
        @Volatile var audioGeneration: Long = 0L
            private set
        @Volatile var videoGeneration: Long = 0L
            private set

        fun setAudioEnabled(enabled: Boolean) {
            if (audioEnabled == enabled) return
            audioEnabled = enabled
            audioGeneration += 1
        }

        fun setVideoEnabled(enabled: Boolean) {
            if (videoEnabled == enabled) return
            videoEnabled = enabled
            videoGeneration += 1
        }
        private val audioPump = audio?.let { port ->
            scope.launch {
                try {
                    while (true) {
                        val inputGeneration = audioGeneration
                        val pcm = port.nextInput() ?: break
                        if (!audioEnabled || inputGeneration != audioGeneration) continue
                        val source = audioSource ?: continue
                        if (pcm.pcm16le.size.toLong() > maxPayloadBytes) {
                            throw AndroidMediaDeviceFailure(MediaErrorKindDto.MEDIA_TOO_LARGE, "captured audio frame exceeds the host payload limit")
                        }
                        frames.send(
                            AndroidMediaInput.Frame(
                                frame = MediaFrameDto(
                                    source = source,
                                    sequence = sequence.incrementAndGet().toULong(),
                                    capturedAtUnixMs = pcm.capturedAtUnixMs.toULong(),
                                    payload = MediaFramePayloadDto.Audio(
                                        MediaPcmFrameDto(DEVICE_SAMPLE_RATE_HZ.toUInt(), 1u.toUShort(), pcm.pcm16le),
                                    ),
                                ),
                                audioGeneration = inputGeneration,
                            ),
                        )
                    }
                } catch (cancelled: CancellationException) {
                    frames.close(cancelled)
                    throw cancelled
                } catch (error: Throwable) {
                    val failure = error.asMediaDeviceFailure()
                    frames.trySend(AndroidMediaInput.Failure(failure))
                    frames.close(failure)
                }
            }
        }
        private val videoPump = video?.let { port ->
            scope.launch {
                try {
                    while (true) {
                        val inputGeneration = videoGeneration
                        val frame = port.nextFrame() ?: break
                        if (videoEnabled && inputGeneration == videoGeneration) {
                            val payloadBytes = frame.payload.let { payload ->
                                when (payload) {
                                    is MediaFramePayloadDto.Audio -> payload.v1.pcm16le.size.toLong()
                                    is MediaFramePayloadDto.Video -> when (val image = payload.v1) {
                                        is MediaVideoFrameDto.I420 -> image.y.size.toLong() + image.u.size + image.v.size
                                        is MediaVideoFrameDto.Rgba8 -> image.bytes.size.toLong()
                                    }
                                }
                            }
                            if (payloadBytes > maxPayloadBytes) {
                                throw AndroidMediaDeviceFailure(MediaErrorKindDto.MEDIA_TOO_LARGE, "captured video frame exceeds the host payload limit")
                            }
                            frames.send(AndroidMediaInput.Frame(
                                frame = frame.copy(sequence = sequence.incrementAndGet().toULong()),
                                videoGeneration = inputGeneration,
                            ))
                        }
                    }
                } catch (cancelled: CancellationException) {
                    frames.close(cancelled)
                    throw cancelled
                } catch (error: Throwable) {
                    val failure = error.asMediaDeviceFailure()
                    frames.trySend(AndroidMediaInput.Failure(failure))
                    frames.close(failure)
                }
            }
        }

        suspend fun stop(onStopped: () -> Unit = {}) = withContext(NonCancellable) {
            stopGate.stop(close = {
                audioPump?.cancelAndJoin()
                videoPump?.cancelAndJoin()
                frames.close()
                var failure: Throwable? = null
                try {
                    audio?.stop()
                } catch (error: Throwable) {
                    failure = error
                }
                try {
                    video?.close()
                } catch (error: Throwable) {
                    if (failure == null) failure = error
                }
                failure?.let { throw it }
            }, onStopped = onStopped)
        }
    }

    companion object {
        private const val MICROPHONE_SOURCE_ID = "media-runtime:microphone"
        private const val CAMERA_SOURCE_ID = "media-runtime:camera"
        private const val DEVICE_SAMPLE_RATE_HZ = 24_000
        private const val MAX_ERROR_BYTES = 1_024
    }
}

private fun truncateUtf8(value: String, maximumBytes: Int): String {
    val result = StringBuilder()
    var byteCount = 0
    var index = 0
    while (index < value.length) {
        val codePoint = value.codePointAt(index)
        val chunk = String(Character.toChars(codePoint))
        val chunkBytes = chunk.toByteArray(StandardCharsets.UTF_8).size
        if (byteCount + chunkBytes > maximumBytes) break
        result.append(chunk)
        byteCount += chunkBytes
        index += Character.charCount(codePoint)
    }
    return result.toString()
}

private class AndroidMediaDeviceFailure(val kind: MediaErrorKindDto, message: String) : RuntimeException(message)

/** This Open has no native resource or has an exact close receipt for each. */
private class AndroidMediaOpenRejectedWithoutResource(val failure: AndroidMediaDeviceFailure) : RuntimeException(failure)

private fun Throwable.asMediaDeviceFailure(): AndroidMediaDeviceFailure = when (this) {
    is AndroidMediaDeviceFailure -> this
    is AudioOperationException -> AndroidMediaDeviceFailure(kind.toMediaErrorKind(), message ?: "Android audio operation failed")
    is AudioDriverException -> AndroidMediaDeviceFailure(error.kind.toMediaErrorKind(), error.message)
    is SecurityException -> AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android media permission is required")
    else -> AndroidMediaDeviceFailure(MediaErrorKindDto.NATIVE_FAILURE, message ?: "Android media input failed")
}

private fun DeviceAudioErrorKind.toMediaErrorKind(): MediaErrorKindDto = when (this) {
    DeviceAudioErrorKind.PermissionDenied -> MediaErrorKindDto.PERMISSION_DENIED
    DeviceAudioErrorKind.Busy -> MediaErrorKindDto.BUSY
    DeviceAudioErrorKind.Cancelled -> MediaErrorKindDto.CANCELLED
    DeviceAudioErrorKind.Timeout -> MediaErrorKindDto.TIMEOUT
    DeviceAudioErrorKind.NoSpeech -> MediaErrorKindDto.NO_DATA
    DeviceAudioErrorKind.NotRecording -> MediaErrorKindDto.NOT_FOUND
    DeviceAudioErrorKind.Unavailable -> MediaErrorKindDto.UNAVAILABLE
    DeviceAudioErrorKind.Unsupported, DeviceAudioErrorKind.ModelMissing, DeviceAudioErrorKind.VoiceMissing -> MediaErrorKindDto.UNSUPPORTED
    DeviceAudioErrorKind.InvalidRequest -> MediaErrorKindDto.INVALID_REQUEST
    DeviceAudioErrorKind.MediaTooLarge -> MediaErrorKindDto.MEDIA_TOO_LARGE
    DeviceAudioErrorKind.SynthesisFailed, DeviceAudioErrorKind.NativeFailure -> MediaErrorKindDto.NATIVE_FAILURE
}

private sealed interface AndroidMediaInput {
    data class Frame(
        val frame: MediaFrameDto,
        val audioGeneration: Long? = null,
        val videoGeneration: Long? = null,
    ) : AndroidMediaInput
    data class Failure(val failure: AndroidMediaDeviceFailure) : AndroidMediaInput
}

private class AndroidCameraOpenFailure(
    val original: Throwable,
    val cleanup: AndroidMediaOpenCleanup,
    closeFailure: Throwable,
) : RuntimeException("Android camera open failed and asynchronous close remains unconfirmed", original) {
    init { addSuppressed(closeFailure) }
}

/** The exact camera open and its callback/close flight have settled. */
private class AndroidCameraOpenClosed(val original: Throwable) : RuntimeException(original)

/** Owns a camera close request and its callbacks across bounded caller waits. */
private class AndroidCameraCloseRequest(
    private val device: AtomicReference<CameraDevice?>,
    private val session: AtomicReference<CameraCaptureSession?>,
    private val reader: ImageReader,
    private val thread: HandlerThread,
    private val handler: AtomicReference<Handler?>,
    private val openCallbackFinished: CompletableDeferred<Unit>,
    private val sessionCallbackFinished: CompletableDeferred<Unit>,
    private val deviceClosed: CompletableDeferred<Unit>,
    private val sessionClosed: CompletableDeferred<Unit>,
) : AndroidMediaOpenCleanup {
    private val flight = AndroidMediaCloseFlight(
        closeTimeoutMs = CAMERA_CLOSE_WAIT_MS,
        canRetryAfterFailure = {
            // CameraDevice.close() is asynchronous. Only retry cleanup after the old request is
            // terminal according to Camera2 callbacks, so no second close flight can overlap it.
            openCallbackFinished.isCompleted && deviceClosed.isCompleted &&
                sessionCallbackFinished.isCompleted &&
                (session.get() == null || sessionClosed.isCompleted)
        },
        closeResource = ::closeAndAwaitCallbacks,
    )

    fun startClose() = flight.start()

    override suspend fun close() = flight.awaitClosed()

    private suspend fun closeAndAwaitCallbacks() {
        openCallbackFinished.await()
        val openedDevice = device.get()
        if (openedDevice != null && !deviceClosed.isCompleted) {
            val accepted = handler.get()?.post { runCatching { openedDevice.close() } } == true
            if (!accepted) runCatching { openedDevice.close() }
        }
        sessionCallbackFinished.await()
        deviceClosed.await()
        if (session.get() != null) sessionClosed.await()
        reader.close()
        if (thread.state != Thread.State.NEW) {
            thread.quitSafely()
            withContext(Dispatchers.IO) { thread.join() }
        }
    }

    private companion object {
        const val CAMERA_CLOSE_WAIT_MS = 5_000L
    }
}

private class AndroidCameraVideoInput private constructor(
    private val device: CameraDevice,
    private val session: CameraCaptureSession,
    private val reader: ImageReader,
    private val thread: HandlerThread,
    private val handler: Handler,
    private val source: MediaSourceRefDto,
    private val closeRequest: AndroidCameraCloseRequest,
) {
    private val frames = Channel<MediaFrameDto>(capacity = 2)
    private val sequence = AtomicLong(0L)
    private val stopRequested = AtomicLong(0L)
    private val stopGate = AndroidMediaStopGate()
    private val timestampOffsetMs = System.currentTimeMillis() - SystemClock.elapsedRealtime()

    suspend fun nextFrame(): MediaFrameDto? {
        val result = frames.receiveCatching()
        result.exceptionOrNull()?.let { throw it }
        return result.getOrNull()
    }

    suspend fun setEnabled(enabled: Boolean) = suspendCancellableCoroutine<Unit> { continuation ->
        if (stopRequested.get() != 0L) {
            continuation.resumeWithException(CancellationException("Android camera lease has ended"))
            return@suspendCancellableCoroutine
        }
        val accepted = handler.post {
            if (!continuation.isActive || stopRequested.get() != 0L) return@post
            try {
                if (enabled) {
                    while (frames.tryReceive().isSuccess) Unit
                    val request = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                        addTarget(reader.surface)
                    }.build()
                    session.setRepeatingRequest(request, null, handler)
                } else {
                    session.stopRepeating()
                }
                if (continuation.isActive) continuation.resume(Unit)
            } catch (error: Throwable) {
                if (continuation.isActive) continuation.resumeWithException(error)
            }
        }
        if (!accepted && continuation.isActive) {
            continuation.resumeWithException(IllegalStateException("Android camera control queue is unavailable"))
        }
    }

    suspend fun close() = withContext(NonCancellable) {
        stopRequested.set(1L)
        frames.close()
        stopGate.stop(close = { closeRequest.close() })
    }

    fun fail(error: AndroidMediaDeviceFailure) {
        if (stopRequested.getAndSet(1L) != 0L) return
        frames.close(error)
        closeRequest.startClose()
    }

    private fun onImage(image: Image) {
        try {
            if (stopRequested.get() != 0L) return
            val width = image.width
            val height = image.height
            val y = copyPlane(image.planes[0], width, height)
            val chromaWidth = (width + 1) / 2
            val chromaHeight = (height + 1) / 2
            val u = copyPlane(image.planes[1], chromaWidth, chromaHeight)
            val v = copyPlane(image.planes[2], chromaWidth, chromaHeight)
            val capturedAt = image.timestamp / 1_000_000L + timestampOffsetMs
            val frame = MediaFrameDto(
                source = source,
                sequence = sequence.incrementAndGet().toULong(),
                capturedAtUnixMs = capturedAt.coerceAtLeast(0L).toULong(),
                payload = MediaFramePayloadDto.Video(
                    MediaVideoFrameDto.I420(
                        width.toUInt(),
                        height.toUInt(),
                        width.toUInt(),
                        chromaWidth.toUInt(),
                        chromaWidth.toUInt(),
                        y,
                        u,
                        v,
                    ),
                ),
            )
            val sent = frames.trySend(frame)
            if (sent.isFailure) {
                frames.tryReceive()
                frames.trySend(frame)
            }
        } catch (error: Throwable) {
            val failure = error.asMediaDeviceFailure()
            fail(failure)
        } finally {
            image.close()
        }
    }

    private fun copyPlane(plane: Image.Plane, width: Int, height: Int): ByteArray {
        val buffer = plane.buffer.duplicate()
        val baseOffset = buffer.position()
        val limit = buffer.limit()
        val output = ByteArray(width * height)
        for (row in 0 until height) {
            for (column in 0 until width) {
                val index = baseOffset + row * plane.rowStride + column * plane.pixelStride
                if (index !in baseOffset until limit) {
                    throw IllegalArgumentException("Android camera plane buffer is shorter than its declared strides")
                }
                output[row * width + column] = buffer.get(index)
            }
        }
        return output
    }

    companion object {
        suspend fun open(context: Context, maxPayloadBytes: Long): AndroidCameraVideoInput {
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                throw AndroidMediaDeviceFailure(
                    MediaErrorKindDto.PERMISSION_DENIED,
                    "Android camera permission is required for media capture",
                )
            }
            val manager = try {
                context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
            } catch (error: SecurityException) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android camera permission is required for media capture")
            } catch (error: Throwable) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera service is unavailable")
            }
            val cameraID = try {
                manager.cameraIdList.firstOrNull { id ->
                    manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK
                } ?: manager.cameraIdList.firstOrNull()
            } catch (error: SecurityException) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android camera permission is required for media capture")
            } catch (error: Throwable) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera could not be enumerated")
            } ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera is unavailable")
            val outputSizes = try {
                manager.getCameraCharacteristics(cameraID)
                    .get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
                    ?.getOutputSizes(ImageFormat.YUV_420_888)
                    ?.filter { size ->
                        size.width <= 640 && size.height <= 480
                    }
                    ?.sortedByDescending { it.width.toLong() * it.height }
                    .orEmpty()
            } catch (error: SecurityException) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android camera permission is required for media capture")
            } catch (error: Throwable) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera formats are unavailable")
            }
            if (outputSizes.isEmpty()) {
                throw AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera has no bounded YUV output format")
            }
            val size = outputSizes.firstOrNull { yuv420PayloadBytes(it) <= maxPayloadBytes }
                ?: throw AndroidMediaDeviceFailure(MediaErrorKindDto.MEDIA_TOO_LARGE, "Android camera frame exceeds the host payload limit")
            val reader = ImageReader.newInstance(size.width, size.height, ImageFormat.YUV_420_888, 2)
            val source = MediaSourceRefDto("media-runtime:camera")
            val thread = HandlerThread("LingxiMediaCamera")
            val deviceRef = AtomicReference<CameraDevice?>()
            val sessionRef = AtomicReference<CameraCaptureSession?>()
            val handlerRef = AtomicReference<Handler?>()
            val openCallbackFinished = CompletableDeferred<Unit>()
            val sessionCallbackFinished = CompletableDeferred<Unit>()
            val deviceClosed = CompletableDeferred<Unit>()
            val sessionClosed = CompletableDeferred<Unit>()
            val openCallStarted = AtomicLong(0L)
            val sessionCallStarted = AtomicLong(0L)
            val portRef = AtomicReference<AndroidCameraVideoInput?>()
            val pendingDeviceFailure = AtomicReference<AndroidMediaDeviceFailure?>()
            val closeRequest = AndroidCameraCloseRequest(
                device = deviceRef,
                session = sessionRef,
                reader = reader,
                thread = thread,
                handler = handlerRef,
                openCallbackFinished = openCallbackFinished,
                sessionCallbackFinished = sessionCallbackFinished,
                deviceClosed = deviceClosed,
                sessionClosed = sessionClosed,
            )
            try {
                thread.start()
                val handler = Handler(thread.looper)
                handlerRef.set(handler)
                val openedDevice = suspendCancellableCoroutine<CameraDevice> { continuation ->
                    openCallStarted.set(1L)
                    continuation.invokeOnCancellation { closeRequest.startClose() }
                    try {
                        manager.openCamera(cameraID, object : CameraDevice.StateCallback() {
                            override fun onOpened(camera: CameraDevice) {
                                deviceRef.set(camera)
                                openCallbackFinished.complete(Unit)
                                if (continuation.isActive) continuation.resume(camera)
                                else closeRequest.startClose()
                            }
                            override fun onDisconnected(camera: CameraDevice) {
                                deviceRef.set(camera)
                                openCallbackFinished.complete(Unit)
                                sessionCallbackFinished.complete(Unit)
                                val failure = AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera disconnected")
                                pendingDeviceFailure.set(failure)
                                portRef.get()?.fail(failure)
                                if (continuation.isActive) continuation.resumeWithException(failure)
                            }
                            override fun onError(camera: CameraDevice, error: Int) {
                                deviceRef.set(camera)
                                openCallbackFinished.complete(Unit)
                                sessionCallbackFinished.complete(Unit)
                                val failure = AndroidMediaDeviceFailure(MediaErrorKindDto.NATIVE_FAILURE, "Android camera failed ($error)")
                                pendingDeviceFailure.set(failure)
                                portRef.get()?.fail(failure)
                                if (continuation.isActive) continuation.resumeWithException(failure)
                            }

                            override fun onClosed(camera: CameraDevice) {
                                deviceClosed.complete(Unit)
                                openCallbackFinished.complete(Unit)
                                if (sessionCallStarted.get() == 0L) {
                                    sessionCallbackFinished.complete(Unit)
                                    sessionClosed.complete(Unit)
                                }
                                if (continuation.isActive) {
                                    continuation.resumeWithException(
                                        AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera closed while opening"),
                                    )
                                }
                            }
                        }, handler)
                    } catch (error: SecurityException) {
                        continuation.resumeWithException(AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android camera permission is required for media capture"))
                    } catch (error: Throwable) {
                        continuation.resumeWithException(error)
                    }
                }
                deviceRef.set(openedDevice)
                val session = suspendCancellableCoroutine<CameraCaptureSession> { continuation ->
                    sessionCallStarted.set(1L)
                    continuation.invokeOnCancellation { closeRequest.startClose() }
                    try {
                        openedDevice.createCaptureSession(listOf(reader.surface), object : CameraCaptureSession.StateCallback() {
                            override fun onConfigured(session: CameraCaptureSession) {
                                sessionRef.set(session)
                                sessionCallbackFinished.complete(Unit)
                                try {
                                    val request = openedDevice.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                                        addTarget(reader.surface)
                                    }.build()
                                    session.setRepeatingRequest(request, null, handler)
                                    if (continuation.isActive) continuation.resume(session)
                                    else closeRequest.startClose()
                                } catch (error: Throwable) {
                                    closeRequest.startClose()
                                    if (continuation.isActive) continuation.resumeWithException(error)
                                }
                            }
                            override fun onConfigureFailed(session: CameraCaptureSession) {
                                sessionRef.set(session)
                                sessionCallbackFinished.complete(Unit)
                                if (continuation.isActive) continuation.resumeWithException(AndroidMediaDeviceFailure(MediaErrorKindDto.UNAVAILABLE, "Android camera stream could not be configured"))
                            }

                            override fun onClosed(session: CameraCaptureSession) {
                                sessionRef.compareAndSet(null, session)
                                sessionClosed.complete(Unit)
                                sessionCallbackFinished.complete(Unit)
                            }
                        }, handler)
                    } catch (error: SecurityException) {
                        continuation.resumeWithException(AndroidMediaDeviceFailure(MediaErrorKindDto.PERMISSION_DENIED, "Android camera permission is required for media capture"))
                    } catch (error: Throwable) {
                        continuation.resumeWithException(error)
                    }
                }
                sessionRef.set(session)
                val port = AndroidCameraVideoInput(openedDevice, session, reader, thread, handler, source, closeRequest)
                portRef.set(port)
                pendingDeviceFailure.get()?.let { port.fail(it) }
                reader.setOnImageAvailableListener({ sourceReader ->
                    try {
                        sourceReader.acquireLatestImage()?.let(port::onImage)
                    } catch (error: Throwable) {
                        port.fail(error.asMediaDeviceFailure())
                    }
                }, handler)
                return port
            } catch (error: Throwable) {
                withContext(NonCancellable) {
                    if (openCallStarted.get() == 0L) {
                        openCallbackFinished.complete(Unit)
                        deviceClosed.complete(Unit)
                    }
                    if (sessionCallStarted.get() == 0L) {
                        sessionCallbackFinished.complete(Unit)
                        sessionClosed.complete(Unit)
                    }
                    try {
                        closeRequest.close()
                    } catch (closeFailure: Throwable) {
                        throw AndroidCameraOpenFailure(error, closeRequest, closeFailure)
                    }
                }
                if (error is AndroidCameraOpenFailure) throw error
                if (error is CancellationException) throw error
                throw AndroidCameraOpenClosed(error)
            }
        }

        private fun yuv420PayloadBytes(size: Size): Long =
            size.width.toLong() * size.height + 2L * ((size.width + 1L) / 2L) * ((size.height + 1L) / 2L)
    }
}
