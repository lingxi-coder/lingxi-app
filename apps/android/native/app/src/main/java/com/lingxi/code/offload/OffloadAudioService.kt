package com.lingxi.code.offload

import android.content.Context
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.OffloadMediaCommand
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.currentCoroutineContext

/** Narrow offload adapter over the same per-device AudioService used by UI and FFI. */
internal interface OffloadAudioService {
    suspend fun speak(owner: AudioOwnerKey, text: String, timeoutBudgetMs: Long): DeviceAudioResult
    suspend fun playMedia(owner: AudioOwnerKey, label: String, target: String): DeviceAudioResult
    suspend fun controlMedia(owner: AudioOwnerKey, label: String, command: OffloadMediaCommand): DeviceAudioResult
}

internal fun interface AudioServiceOperationPerformer {
    suspend fun perform(
        owner: AudioOwnerKey,
        operation: DeviceAudioOperation,
        timeoutBudgetMs: Long?,
    ): DeviceAudioResult
}

/** Port close cancels its own admitted work while caller cancellation still flows through. */
internal class OffloadPortLifetime {
    private val lifetimeJob = SupervisorJob()

    suspend fun <T> execute(block: suspend () -> T): T {
        val callerContext = currentCoroutineContext()
        val callerJob = callerContext[Job]
            ?: throw CancellationException("offload operation has no caller job")
        val operationJob = SupervisorJob(lifetimeJob)
        val callerCancellation = callerJob.invokeOnCompletion { cause ->
            if (cause != null) {
                operationJob.cancel(cause as? CancellationException ?: CancellationException("offload caller ended", cause))
            }
        }
        val operation = CoroutineScope(callerContext + operationJob).async(start = CoroutineStart.UNDISPATCHED) {
            block()
        }
        return try {
            operation.await()
        } finally {
            callerCancellation.dispose()
            operationJob.cancel()
        }
    }

    fun close() {
        lifetimeJob.cancel(CancellationException("offload command port is closing"))
    }
}

internal class AndroidOffloadAudioService internal constructor(
    private val performer: AudioServiceOperationPerformer,
) : OffloadAudioService {
    constructor(context: Context) : this(
        AudioServiceOperationPerformer { owner, operation, timeoutBudgetMs ->
            AndroidAudioServiceProvider.perform(
                context = context.applicationContext,
                owner = owner,
                operation = operation,
                timeoutBudgetMs = timeoutBudgetMs,
            )
        },
    )

    override suspend fun speak(owner: AudioOwnerKey, text: String, timeoutBudgetMs: Long): DeviceAudioResult =
        performer.perform(
            owner = owner,
            operation = DeviceAudioOperation.Speak(
                text = text,
                language = null,
                rate = null,
                voice = null,
            ),
            timeoutBudgetMs = timeoutBudgetMs,
        )

    override suspend fun playMedia(owner: AudioOwnerKey, label: String, target: String): DeviceAudioResult =
        performer.perform(owner, DeviceAudioOperation.OffloadMediaPlay(label, target), null)

    override suspend fun controlMedia(
        owner: AudioOwnerKey,
        label: String,
        command: OffloadMediaCommand,
    ): DeviceAudioResult = performer.perform(owner, DeviceAudioOperation.OffloadMediaControl(label, command), null)

}
