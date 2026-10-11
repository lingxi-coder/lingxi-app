package com.lingxi.code.voice.audio

import android.content.Context
import com.lingxi.code.bindings.runtime.maxAudioPayloadBytes
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
    @Volatile private var cloudBridge: ProviderAudioBridge? = null
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
                provider = providerBridge(appContext),
            )
            core.also { service = it }
        }
    }

    private fun providerBridge(context: Context): ProviderAudioBridge = cloudBridge ?: synchronized(this) {
        cloudBridge ?: RustProviderAudioBridge(context.applicationContext).also { cloudBridge = it }
    }

    suspend fun probeProvider(context: Context, configuration: AudioConfigurationV4, kind: String): ProviderAudioCapability {
        val request = DeviceAudioRequest(newIdentity(get(context).capabilities().serviceEpoch), AudioOwnerKey.ui("audio-settings"),
            5_000, maxAudioPayloadBytes().toLong(), DeviceAudioOperation.Status(null))
        val bridge = providerBridge(context)
        return try {
            bridge.capabilities(request, configuration, kind)
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            ProviderAudioCapability((error as? AudioOperationException)?.kind != DeviceAudioErrorKind.Unsupported,
                "unavailable", error.message ?: "Provider audio capabilities could not be loaded.", null, null, null)
        } finally {
            kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) { runCatching { bridge.cancel(request.identity.id) } }
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
        configuration: AudioConfigurationV4? = null,
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

    suspend fun openRealtimeAgent(
        context: Context, owner: AudioOwnerKey, configuration: AudioConfigurationV4, callbacks: RealtimeAgentCallbacks,
    ): RealtimeSpeechSession {
        val core = get(context)
        val capability = core.capabilities()
        return core.openRealtimeAgent(DeviceAudioRequest(newIdentity(capability.serviceEpoch), owner, null,
            capability.maxPayloadBytes, DeviceAudioOperation.Status(null)), configuration, callbacks)
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
