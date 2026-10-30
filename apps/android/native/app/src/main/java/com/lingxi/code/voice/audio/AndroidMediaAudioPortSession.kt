package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.ConcurrentLinkedQueue

internal data class AndroidMediaAudioWriteReceipt(
    val acceptedSampleFrames: Int,
    val playedSampleFrames: Int,
    val renderedAtUnixMs: Long?,
)

internal class AndroidMediaBarrierToken

internal class AndroidMediaReceiptClaim {
    private val owner = AtomicReference<AndroidMediaBarrierToken?>(null)

    fun claim(token: AndroidMediaBarrierToken): Boolean = owner.compareAndSet(null, token)
    fun isClaimedBy(token: AndroidMediaBarrierToken): Boolean = owner.get() === token
    fun release(token: AndroidMediaBarrierToken): Boolean = owner.compareAndSet(token, null)
}

/** One RTC audio stream using the same native lease coordinator as UI and tools. */
internal class AndroidMediaAudioPortSession(
    private val core: AndroidAudioServiceCore,
    private val owner: AudioOwnerKey,
    val format: RealtimePcmFormat,
    private val input: Channel<RealtimePcmCaptureFrame>,
    private val output: Channel<RealtimePcmWrite>,
    private val leases: List<AudioLease>,
    private val closeReceipts: List<AndroidMediaNativeCloseReceipt>,
    private val captureJob: Job?,
    private val playbackJob: Job?,
) {
    init {
        require(closeReceipts.size == leases.size && closeReceipts.map { it.lease }.toSet() == leases.toSet()) {
            "each media audio lease requires its own native close receipt"
        }
    }

    private val acceptingWrites = AtomicBoolean(true)
    private val stopGate = AndroidMediaStopGate()
    private val outputGeneration = AtomicLong(1L)
    private val pendingWrites = ConcurrentLinkedQueue<PendingMediaWrite>()
    private val barrierLock = Any()
    @Volatile private var inputDropBeforeUnixMs: Long = 0L

    private class PendingMediaWrite(val frame: RealtimePcmWrite) {
        val receiptClaim = AndroidMediaReceiptClaim()
    }

    fun stableOwner(): AudioOwnerKey = owner

    val stopState: AndroidMediaStopState get() = stopGate.state

    fun setInputEnabled(enabled: Boolean) {
        if (!enabled) inputDropBeforeUnixMs = System.currentTimeMillis()
        leases.firstOrNull { it.resource == AudioResource.Capture }?.let { core.setMediaCaptureEnabled(it, enabled) }
        if (!enabled) {
            while (input.tryReceive().isSuccess) Unit
        }
    }

    fun setPlaybackGain(gain: Float) {
        val playbackLease = leases.firstOrNull { it.resource == AudioResource.Playback }
            ?: throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "media session has no playback path")
        core.setMediaPlaybackGain(playbackLease, gain)
    }

    suspend fun nextInput(): RealtimePcmCaptureFrame? {
        if (stopGate.state == AndroidMediaStopState.STOPPED) return null
        while (true) {
            val result = input.receiveCatching()
            result.exceptionOrNull()?.let { throw it }
            val frame = result.getOrNull() ?: return null
            if (frame.capturedAtUnixMs > inputDropBeforeUnixMs) return frame
        }
    }

    suspend fun writeAudio(pcm16le: ByteArray, sampleRateHz: Int, channels: Int): AndroidMediaAudioWriteReceipt {
        if (stopGate.state != AndroidMediaStopState.ACTIVE || !acceptingWrites.get()) throw CancellationException("media audio session has ended")
        if (sampleRateHz !in 8_000..192_000 || channels !in 1..2 || pcm16le.isEmpty() || pcm16le.size % (channels * 2) != 0) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "media audio output must be bounded PCM16LE with a supported sample layout")
        }
        val sourceSampleFrames = pcm16le.size / (channels * 2)
        val normalized = normalizeToDeviceFormat(pcm16le, sampleRateHz, channels, format.sampleRateHz)
        val pending = synchronized(barrierLock) {
            if (stopGate.state != AndroidMediaStopState.ACTIVE || !acceptingWrites.get()) throw CancellationException("media audio session has ended")
            val frame = RealtimePcmWrite(
                bytes = normalized,
                generation = outputGeneration.get(),
                sourceSampleFrames = sourceSampleFrames,
                completion = CompletableDeferred(),
            )
            PendingMediaWrite(frame).also(pendingWrites::add)
        }
        try {
            output.send(pending.frame)
        } catch (error: Throwable) {
            pendingWrites.remove(pending)
            throw error
        }
        return AndroidMediaAudioWriteReceipt(sourceSampleFrames, 0, null)
    }

    suspend fun flushAudio(): AndroidMediaAudioWriteReceipt {
        val claim = AndroidMediaBarrierToken()
        val snapshot = synchronized(barrierLock) { pendingWrites.toList() }
        return settlePending(snapshot, claim)
    }

    suspend fun interruptAudio(): AndroidMediaAudioWriteReceipt = withContext(NonCancellable) {
        if (stopGate.state == AndroidMediaStopState.STOPPED) return@withContext AndroidMediaAudioWriteReceipt(0, 0, null)
        val claim = AndroidMediaBarrierToken()
        val (throughGeneration, snapshot) = synchronized(barrierLock) {
            val through = outputGeneration.incrementAndGet()
            val oldWrites = pendingWrites.toList().filter { it.frame.generation < through }
            oldWrites.forEach { it.receiptClaim.claim(claim) }
            through to oldWrites
        }
        try {
            leases.firstOrNull { it.resource == AudioResource.Playback }?.let { lease ->
                core.interruptMediaPortPlayback(lease, throughGeneration)
            }
        } catch (error: Throwable) {
            snapshot.forEach { it.receiptClaim.release(claim) }
            throw error
        }
        settlePending(snapshot, claim)
    }

    private suspend fun settlePending(
        snapshot: List<PendingMediaWrite>,
        claim: AndroidMediaBarrierToken,
    ): AndroidMediaAudioWriteReceipt {
        var played = 0
        var renderedAt: Long? = null
        try {
            for (pending in snapshot) {
                val receipt = pending.frame.completion.await()
                val ownsReceipt = pending.receiptClaim.claim(claim) || pending.receiptClaim.isClaimedBy(claim)
                if (ownsReceipt) {
                    played = (played.toLong() + receipt.playedSampleFrames).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
                    if (receipt.renderedAtUnixMs != null) renderedAt = receipt.renderedAtUnixMs
                    pendingWrites.remove(pending)
                }
            }
        } catch (error: Throwable) {
            snapshot.forEach { pending ->
                if (pending.receiptClaim.isClaimedBy(claim)) {
                    if (pending.frame.completion.isCompleted) pendingWrites.remove(pending)
                    else pending.receiptClaim.release(claim)
                }
            }
            throw error
        }
        return AndroidMediaAudioWriteReceipt(0, played, renderedAt)
    }

    suspend fun stop() = withContext(NonCancellable) {
        stopGate.stop(close = {
            synchronized(barrierLock) { acceptingWrites.set(false) }
            input.cancel(CancellationException("media audio session stopped"))
            output.close(CancellationException("media audio session stopped"))
            while (true) {
                val pending = output.tryReceive().getOrNull() ?: break
                pending.completion.complete(RealtimePcmWriteReceipt(0, null))
            }
            pendingWrites.forEach { it.frame.completion.complete(RealtimePcmWriteReceipt(0, null)) }
            pendingWrites.clear()

            // Request native stop before joining AudioRecord's blocking read. The
            // logical lease remains active until both stream jobs have exited.
            captureJob?.cancel()
            playbackJob?.cancel()
            var failure: Throwable? = null
            for (lease in leases.asReversed()) {
                try {
                    core.stopMediaPortLeaseNative(lease)
                }
                catch (error: Throwable) { if (failure == null) failure = error }
            }
            if (failure != null) throw failure!!

            try {
                withTimeout(STOP_JOIN_TIMEOUT_MS) {
                    captureJob?.join()
                    playbackJob?.join()
                    for (receipt in closeReceipts) {
                        receipt.confirmIfNeverDispatched()
                        receipt.awaitClosed()
                    }
                }
            } catch (_: TimeoutCancellationException) {
                throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "media audio stream did not stop after native close")
            }

            for (lease in leases.asReversed()) {
                try { core.releaseStoppedMediaPortLease(lease) }
                catch (error: Throwable) { if (failure == null) failure = error }
            }
            if (failure != null) throw failure!!
        })
    }

    private companion object {
        const val STOP_JOIN_TIMEOUT_MS = 5_000L
    }
}

internal fun normalizeToDeviceFormat(
    bytes: ByteArray,
    sourceRate: Int,
    channels: Int,
    targetRate: Int,
): ByteArray {
    val sourceFrames = bytes.size / (channels * 2)
    val convertedFrames = ((sourceFrames.toLong() * targetRate + sourceRate / 2L) / sourceRate)
        .coerceAtLeast(1L)
    if (convertedFrames > targetRate * 2L) {
        throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "media audio frame exceeds the two-second device buffer limit")
    }
    val targetFrames = convertedFrames.toInt()
    val result = ByteArray(targetFrames * 2)
    for (targetIndex in 0 until targetFrames) {
        val position = targetIndex.toDouble() * sourceRate / targetRate
        val left = position.toInt().coerceIn(0, sourceFrames - 1)
        val right = (left + 1).coerceAtMost(sourceFrames - 1)
        val fraction = position - left
        var sum = 0.0
        for (channel in 0 until channels) {
            val aIndex = (left * channels + channel) * 2
            val bIndex = (right * channels + channel) * 2
            val a = ((bytes[aIndex + 1].toInt() shl 8) or (bytes[aIndex].toInt() and 0xff)).toShort().toInt()
            val b = ((bytes[bIndex + 1].toInt() shl 8) or (bytes[bIndex].toInt() and 0xff)).toShort().toInt()
            sum += a + (b - a) * fraction
        }
        val sample = (sum / channels).toInt().coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
        result[targetIndex * 2] = sample.toByte()
        result[targetIndex * 2 + 1] = (sample shr 8).toByte()
    }
    return result
}
