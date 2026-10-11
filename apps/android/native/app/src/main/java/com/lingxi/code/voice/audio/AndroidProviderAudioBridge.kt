package com.lingxi.code.voice.audio

import android.content.Context
import com.lingxi.code.bindings.android.buildAndroidSessionAudioProviderHost
import com.lingxi.code.bindings.android.buildAndroidAudioProviderHost
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.settings.toStorageMap
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject
import java.util.Base64

data class ProviderAudioCapability(
    val supported: Boolean, val readiness: String, val reason: String?,
    val profileId: String?, val providerId: String?, val modelId: String?,
    val modelIds: List<String?> = emptyList(),
    val voices: List<ProviderAudioVoice> = emptyList(),
)

data class ProviderAudioVoice(val selection: AudioVoiceSelection, val label: String)

data class ProviderAudioProfileOption(val profileId: String, val label: String, val capability: ProviderAudioCapability)

/** SDK transport seam: native Kotlin never implements provider protocols or reads API keys. */
internal interface ProviderAudioBridge {
    suspend fun openRealtime(request: DeviceAudioRequest, configuration: AudioConfigurationV4, onEvent: suspend (String) -> Unit): ProviderRealtimeSession =
        throw AudioOperationException(DeviceAudioErrorKind.Unsupported, "Realtime Agent conversation is unavailable")
    suspend fun capabilities(request: DeviceAudioRequest, configuration: AudioConfigurationV4, kind: String): ProviderAudioCapability
    suspend fun transcribe(request: DeviceAudioRequest, configuration: AudioConfigurationV4, capture: DeviceAudioCapture): SttResult
    suspend fun synthesize(request: DeviceAudioRequest, configuration: AudioConfigurationV4, text: String): Pair<ByteArray, Int>
    suspend fun cancel(operationId: String)
}

internal interface ProviderRealtimeSession {
    suspend fun sendAudio(pcm: ByteArray)
    suspend fun commitInput()
    suspend fun interrupt(itemId: String?, audioEndMs: UInt?)
    suspend fun playbackCompleted(itemId: String?)
    suspend fun close()
    suspend fun abort()
}

/** Only a live conversation source attaches here; credentials and headless preview engines cannot replace it. */
internal object AndroidAudioSessionContext {
    @Volatile private var engine: MobileEngineHandle? = null
    @Synchronized fun attach(handle: MobileEngineHandle) { engine = handle }
    @Synchronized fun clear() { engine = null }
    fun currentEngine(): MobileEngineHandle? = engine
    fun detach(handle: MobileEngineHandle) { synchronized(this) { if (engine === handle) engine = null } }
    suspend fun snapshot(owner: AudioOwnerKey, handle: MobileEngineHandle? = engine): JSONObject? {
        val current = handle ?: return null
        val json = JSONObject(current.audioSessionContext())
        if (json.isNull("profileId") || json.optString("profileId").isBlank()) return null
        if (owner.kind == AudioOwnerKey.Kind.Session && json.optString("sessionId") != owner.id) return null
        return json
    }
}

internal class RustProviderAudioBridge(context: Context) : ProviderAudioBridge {
    private val appContext = context.applicationContext
    private data class Host(val profiles: String, val region: String, val handle: com.lingxi.code.bindings.android.AndroidAudioProviderHost)
    @Volatile private var cached: Host? = null
    private data class Operation(val host: Host, val json: String, val engine: MobileEngineHandle?)
    private val operations = java.util.concurrent.ConcurrentHashMap<String, Operation>()

    private fun host(sessionRegion: String?): Host {
        val repository = ProviderSettingsRepository(appContext)
        val (profiles, configuredRegion) = try { repository.audioProviderProfilesJson() to repository.audioProviderRegion() } finally { repository.close() }
        val region = when (sessionRegion ?: configuredRegion) {
            "china_mainland", "china" -> "china"
            "international" -> "international"
            else -> throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Select a supported provider region in provider settings.")
        }
        return synchronized(this) {
            cached?.takeIf { it.profiles == profiles && it.region == region } ?: Host(profiles, region,
                buildAndroidAudioProviderHost(profiles, region, AndroidSecureStorageAdapter(appContext))).also { cached = it }
        }
    }

    override suspend fun capabilities(request: DeviceAudioRequest, configuration: AudioConfigurationV4, kind: String): ProviderAudioCapability {
        val engine = AndroidAudioSessionContext.currentEngine()
        val session = AndroidAudioSessionContext.snapshot(request.owner, engine)
        val cloud = when (kind) {
            "recognition" -> configuration.recognition.cloud
            "speech" -> configuration.speech.cloud
            else -> configuration.conversation.cloud
        }
        val selectedHost = if (session != null && engine != null && (cloud.binding == "follow_session" || kind == "realtime")) {
            // Clone the actual runtime SDK for every admission so profile edits cannot leave a stale host cache.
            Host("session:${session.getString("sessionId")}", session.optionalString("region").orEmpty(),
                buildAndroidSessionAudioProviderHost(engine))
        } else host(session?.optionalString("region"))
        val operation = Operation(selectedHost, providerAudioRequestJson(request, configuration, kind, session), engine)
        operations[request.identity.id] = operation
        val response = JSONObject(operation.host.handle.capabilities(operation.json))
        response.throwIfError()
        val rawVoice = when (val input = request.operation) {
            is DeviceAudioOperation.Speak -> input.voice
            is DeviceAudioOperation.Synthesize -> input.voice
            else -> null
        }
        val scopedVoice = if (rawVoice != null) com.lingxi.code.voice.parseExplicitVoiceSelection(rawVoice)
            else if (kind == "realtime") configuration.conversation.voice else if (kind == "speech" && configuration.speech.source == AudioSource.PROVIDER) configuration.speech.voice else null
        val capability = parseProviderAudioCapability(response)
        if (scopedVoice != null && scopedVoice.modelId != capability.modelId) {
            return capability.copy(readiness = "needs_configuration",
                reason = "The selected voice belongs to another audio model. Select the current model's voice in audio settings.")
        }
        return capability
    }

    override suspend fun transcribe(request: DeviceAudioRequest, configuration: AudioConfigurationV4, capture: DeviceAudioCapture): SttResult {
        val operation = operations[request.identity.id] ?: throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Provider transcription was not admitted.")
        val active = operation.host.handle
        return try {
            val reply = JSONObject(active.transcribe(operation.json, capture.bytes, capture.mimeType))
            reply.throwIfError()
            AndroidAudioUsageJournal.record(request.identity.id, "recognition", reply)
            SttResult.Ok(reply.getString("text"), reply.optionalString("language"), null)
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable) { active.cancel(request.identity.id) }
            throw cancelled
        }
    }

    override suspend fun synthesize(request: DeviceAudioRequest, configuration: AudioConfigurationV4, text: String): Pair<ByteArray, Int> {
        val operation = operations[request.identity.id] ?: throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Provider synthesis was not admitted.")
        val active = operation.host.handle
        return try {
            val reply = JSONObject(active.synthesize(operation.json, text))
            reply.throwIfError()
            AndroidAudioUsageJournal.record(request.identity.id, "speech", reply)
            val encoded = reply.getString("pcmBase64")
            if (encoded.length.toLong() > ((request.maxPayloadBytes + 2) / 3) * 4) {
                throw AudioOperationException(DeviceAudioErrorKind.MediaTooLarge, "Provider audio exceeds the payload limit.")
            }
            Base64.getDecoder().decode(encoded) to reply.getInt("sampleRateHz")
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable) { active.cancel(request.identity.id) }
            throw cancelled
        }
    }

    override suspend fun cancel(operationId: String) { operations.remove(operationId)?.host?.handle?.cancel(operationId) }

    override suspend fun openRealtime(request: DeviceAudioRequest, configuration: AudioConfigurationV4, onEvent: suspend (String) -> Unit): ProviderRealtimeSession {
        val operation = operations[request.identity.id] ?: throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "Realtime audio was not admitted.")
        val engine = operation.engine ?: throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "Open a current Agent session before starting realtime conversation.")
        val session = com.lingxi.code.bindings.android.startAndroidRealtimeAudio(engine, operation.host.handle,
            operation.json, object : com.lingxi.code.bindings.android.AndroidRealtimeAudioListener {
                override suspend fun onEvent(eventJson: String) { onEvent(eventJson) }
            })
        val commands = Mutex()
        return object : ProviderRealtimeSession {
            override suspend fun sendAudio(pcm: ByteArray) = commands.withLock { session.sendAudio(pcm) }
            override suspend fun commitInput() = commands.withLock { session.commitInput() }
            override suspend fun interrupt(itemId: String?, audioEndMs: UInt?) = commands.withLock { session.interrupt(itemId, audioEndMs) }
            override suspend fun playbackCompleted(itemId: String?) = commands.withLock { session.playbackCompleted(itemId) }
            override suspend fun close() = session.closeSession()
            override suspend fun abort() = session.abort()
        }
    }
}

internal fun parseProviderAudioCapability(response: JSONObject): ProviderAudioCapability {
    try {
        val supported = response.get("supported") as? Boolean
            ?: throw org.json.JSONException("supported must be a boolean")
        val readiness = response.getString("readiness")
        if (readiness !in listOf("ready", "needs_configuration", "unavailable", "unsupported")) {
            throw org.json.JSONException("readiness is invalid")
        }
        val profileId = response.capabilityString("profileId")
        val modelIds = mutableListOf<String?>()
        val voices = mutableListOf<ProviderAudioVoice>()
        val models = response.getJSONArray("models")
        for (index in 0 until models.length()) {
            val model = models.getJSONObject(index)
            if (!model.has("id")) throw org.json.JSONException("model id is missing")
            val modelId = model.capabilityString("id")
            modelIds.add(modelId)
            val catalogVoices = model.getJSONArray("voices")
            for (voiceIndex in 0 until catalogVoices.length()) {
                val voice = catalogVoices.getJSONObject(voiceIndex)
                val id = voice.getString("id").takeIf(String::isNotBlank)
                    ?: throw org.json.JSONException("voice id is empty")
                voices.add(ProviderAudioVoice(AudioVoiceSelection(AudioSource.PROVIDER, id, modelId, profileId),
                    voice.capabilityString("label") ?: id))
            }
        }
        return ProviderAudioCapability(supported, readiness, response.capabilityString("reason"), profileId,
            response.capabilityString("providerId"), response.capabilityString("modelId"), modelIds, voices)
    } catch (error: org.json.JSONException) {
        throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest,
            "Provider audio capabilities require the current model and voice object format.")
    }
}

private fun JSONObject.capabilityString(key: String): String? =
    if (isNull(key)) null else getString(key).takeIf(String::isNotBlank)

internal fun providerAudioRequestJson(request: DeviceAudioRequest, config: AudioConfigurationV4, kind: String, session: JSONObject?): String {
    val all = JSONObject(config.toStorageMap())
    val preference = all.getJSONObject(if (kind == "realtime") "conversation" else if (kind == "recognition") "recognition" else "speech")
    val language = when (val operation = request.operation) {
        is DeviceAudioOperation.Listen -> operation.language
        is DeviceAudioOperation.Transcribe -> operation.language
        is DeviceAudioOperation.Synthesize -> operation.language
        is DeviceAudioOperation.Speak -> operation.language
        else -> null
    } ?: config.language.takeUnless { it == "auto" }
    val voiceOverride = when (val operation = request.operation) {
        is DeviceAudioOperation.Synthesize -> operation.voice
        is DeviceAudioOperation.Speak -> operation.voice
        else -> null
    }
    val rate = when (val operation = request.operation) {
        is DeviceAudioOperation.Synthesize -> operation.rate?.toDouble()
        is DeviceAudioOperation.Speak -> operation.rate?.toDouble()
        else -> null
    } ?: config.rate
    val cloud = preference.getJSONObject("cloud")
    val explicitVoice = voiceOverride?.let { com.lingxi.code.voice.parseExplicitVoiceSelection(it) }
    val scopedVoice = explicitVoice ?: if (voiceOverride == null) {
        if (kind == "realtime") config.conversation.voice else if (kind == "speech" && config.speech.source == AudioSource.PROVIDER) config.speech.voice else null
    } else null
    if (scopedVoice != null) {
        val profile = if (cloud.optString("binding") == "follow_session") session?.optString("profileId") else cloud.optString("profileId")
        if (scopedVoice.source != AudioSource.PROVIDER || scopedVoice.profileId != profile ||
            (!cloud.isNull("modelId") && scopedVoice.modelId != cloud.optString("modelId"))) {
            throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "The requested voice does not match the selected audio provider profile and model.")
        }
    }
    return JSONObject().apply {
        put("operationId", request.identity.id)
        put("kind", kind)
        put("session", session?.let { actual -> JSONObject().apply {
            put("sessionId", actual.getString("sessionId")); put("profileId", actual.getString("profileId")); put("accountScope", actual.getString("accountScope"))
        } } ?: JSONObject.NULL)
        put("cloud", cloud)
        put("language", language ?: JSONObject.NULL)
        put("voice", if (voiceOverride?.lowercase() in listOf("default", "auto")) JSONObject.NULL
            else explicitVoice?.id ?: voiceOverride ?: scopedVoice?.id ?: JSONObject.NULL)
        if (kind == "speech") put("rate", rate)
        if (kind == "realtime") put("interaction", config.conversation.interaction)
        request.timeoutBudgetMs?.let { put("timeoutMs", it) }
        put("maxPayloadBytes", request.maxPayloadBytes)
    }.toString()
}

private fun JSONObject.optionalString(key: String): String? = if (isNull(key)) null else optString(key).takeIf(String::isNotBlank)
private fun JSONObject.throwIfError() {
    val error = optJSONObject("error") ?: return
    throw AudioOperationException(when (error.optString("kind")) {
        "unsupported" -> DeviceAudioErrorKind.Unsupported
        "cancelled" -> DeviceAudioErrorKind.Cancelled
        "timeout" -> DeviceAudioErrorKind.Timeout
        "invalid_request" -> DeviceAudioErrorKind.InvalidRequest
        "media_too_large" -> DeviceAudioErrorKind.MediaTooLarge
        else -> DeviceAudioErrorKind.Unavailable
    }, error.optString("message", "Provider audio failed. Open audio and provider settings to recover."))
}
