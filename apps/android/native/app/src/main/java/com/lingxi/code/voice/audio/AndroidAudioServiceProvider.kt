package com.lingxi.code.voice.audio

import android.content.Context
import com.lingxi.code.bindings.maxAudioPayloadBytes
import com.lingxi.code.voice.AndroidVoiceRuntime
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong

/** Process singleton: settings, UI, Flow, tool callbacks and Computer Use share this device lane. */
internal object AndroidAudioServiceProvider {
    @Volatile private var service: AndroidAudioServiceCore? = null
    private val generation = AtomicLong(0L)
    private val lifecycleScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    fun get(context: Context): AndroidAudioServiceCore = service ?: synchronized(this) {
        service ?: run {
            val appContext = context.applicationContext
            lateinit var core: AndroidAudioServiceCore
            val driver = AndroidAudioDeviceDriverImpl(appContext) {
                lifecycleScope.launch { runCatching { core.invalidate() } }
            }
            core = AndroidAudioServiceCore(
                runtime = AndroidRuntimeSpeechBridge(AndroidVoiceRuntime(appContext)),
                driver = driver,
                maxPayloadBytes = maxAudioPayloadBytes().toLong(),
            )
            core.also { service = it }
        }
    }

    suspend fun perform(
        context: Context,
        owner: AudioOwnerKey,
        operation: DeviceAudioOperation,
        timeoutBudgetMs: Long? = null,
    ): DeviceAudioResult {
        val core = get(context)
        val capability = core.capabilities()
        return core.execute(
            DeviceAudioRequest(
                identity = newIdentity(capability.serviceEpoch),
                owner = owner,
                timeoutBudgetMs = timeoutBudgetMs,
                maxPayloadBytes = capability.maxPayloadBytes,
                operation = operation,
            ),
        )
    }

    suspend fun playOffloadMedia(
        context: Context,
        owner: AudioOwnerKey,
        sessionLabel: String,
        target: String,
    ): DeviceAudioResult = perform(
        context = context,
        owner = owner,
        operation = DeviceAudioOperation.OffloadMediaPlay(sessionLabel, target),
    )

    suspend fun controlOffloadMedia(
        context: Context,
        owner: AudioOwnerKey,
        sessionLabel: String,
        command: OffloadMediaCommand,
    ): DeviceAudioResult = perform(
        context = context,
        owner = owner,
        operation = DeviceAudioOperation.OffloadMediaControl(sessionLabel, command),
    )

    suspend fun openRealtimeListen(
        context: Context,
        owner: AudioOwnerKey,
        language: String?,
        callbacks: RealtimeSpeechCallbacks,
        configuration: AudioConfigurationV3? = null,
        configurationRevision: Long? = null,
    ): RealtimeSpeechSession {
        val core = get(context)
        val capability = core.capabilities()
        return core.openRealtimeListen(
            request = DeviceAudioRequest(
                identity = newIdentity(capability.serviceEpoch),
                owner = owner,
                timeoutBudgetMs = null,
                maxPayloadBytes = capability.maxPayloadBytes,
                operation = DeviceAudioOperation.Listen(language, configuration, configurationRevision),
            ),
            callbacks = callbacks,
        )
    }

    suspend fun invalidate(context: Context) {
        service?.invalidate()
    }

    private fun newIdentity(serviceEpoch: Long): AudioOperationIdentity {
        val next = generation.incrementAndGet()
        require(next <= 9_007_199_254_740_991L) { "audio operation generation is exhausted" }
        return AudioOperationIdentity(UUID.randomUUID().toString(), next, serviceEpoch)
    }
}
