// Generated from clients/voice/audio-config-schema.json and audio-config-fixtures.json.
// Do not edit by hand; run node clients/voice/scripts/generate-audio-config.mjs.
package com.lingxi.code.voice.audio

const val AUDIO_CONFIGURATION_SCHEMA_VERSION: Int = 3
const val AUDIO_LANGUAGE_AUTO: String = "auto"
const val AUDIO_MIN_RATE: Double = 0.5
const val AUDIO_MAX_RATE: Double = 2.0
const val AUDIO_DEFAULT_RATE: Double = 1.0

@JvmInline
value class AudioSource(val value: String) {
    companion object {
        val AUTOMATIC = AudioSource("automatic")
        val SYSTEM = AudioSource("system")
        val OFFLINE = AudioSource("offline")
    }
}

enum class AudioProviderKind { RECOGNITION, SPEECH }
enum class AudioReadiness { AVAILABLE, PERMISSION_REQUIRED, DENIED, UNAVAILABLE }
enum class AudioRouteStatus { READY, PERMISSION_REQUIRED, UNAVAILABLE, INVALID_REQUEST }
enum class AudioFallbackFailure { PERMISSION, UNAVAILABLE, BUSY, CANCELLED, TIMEOUT, INVALID_REQUEST, NO_SPEECH, NATIVE_FAILURE }

data class AudioVoiceSelection(
    val source: AudioSource,
    val id: String,
    val modelId: String? = null,
)

data class AudioRecognitionPreference(
    val source: AudioSource = AudioSource.AUTOMATIC,
    val offlineModelId: String? = null,
)

data class AudioSpeechPreference(
    val source: AudioSource = AudioSource.AUTOMATIC,
    val offlineModelId: String? = null,
    val voice: AudioVoiceSelection? = null,
)

data class AudioConfigurationV3(
    val schemaVersion: Int = AUDIO_CONFIGURATION_SCHEMA_VERSION,
    val recognition: AudioRecognitionPreference = AudioRecognitionPreference(),
    val speech: AudioSpeechPreference = AudioSpeechPreference(),
    val language: String = AUDIO_LANGUAGE_AUTO,
    val rate: Double = AUDIO_DEFAULT_RATE,
    val autoPlayReplies: Boolean = false,
)

data class AudioVoiceCatalogEntry(
    val source: AudioSource,
    val id: String,
    val modelId: String? = null,
    val label: String? = null,
    val aliases: List<String> = emptyList(),
)

data class AudioOfflineModelAvailability(
    val id: String,
    val kind: AudioProviderKind,
    val languages: List<String>,
    val installed: Boolean,
    val voiceIds: List<String>? = null,
)

data class AudioRouteRequest(
    val kind: AudioProviderKind,
    val source: AudioSource,
    val offlineModelId: String?,
    val voice: AudioVoiceSelection?,
    val language: String,
    val systemStatus: AudioReadiness,
    val offlineModels: List<AudioOfflineModelAvailability>,
    val systemVoiceIds: List<String>? = null,
) {
    constructor(
        kind: AudioProviderKind,
        preference: AudioRecognitionPreference,
        language: String,
        systemStatus: AudioReadiness,
        offlineModels: List<AudioOfflineModelAvailability>,
        systemVoiceIds: List<String>? = null,
        voiceOverride: AudioVoiceSelection? = null,
    ) : this(kind, preference.source, preference.offlineModelId, voiceOverride, language, systemStatus, offlineModels, systemVoiceIds)

    constructor(
        kind: AudioProviderKind,
        preference: AudioSpeechPreference,
        language: String,
        systemStatus: AudioReadiness,
        offlineModels: List<AudioOfflineModelAvailability>,
        systemVoiceIds: List<String>? = null,
        voiceOverride: AudioVoiceSelection? = null,
    ) : this(kind, preference.source, preference.offlineModelId, voiceOverride ?: preference.voice, language, systemStatus, offlineModels, systemVoiceIds)
}

data class AudioRouteResolution(
    val requested: RequestedAudioRoute,
    val effective: EffectiveAudioRoute?,
    val status: AudioRouteStatus,
    val reason: String,
    val fallbackReason: String? = null,
)

data class RequestedAudioRoute(val source: AudioSource, val offlineModelId: String?, val voice: AudioVoiceSelection?)
data class EffectiveAudioRoute(val source: AudioSource, val modelId: String?, val voiceId: String?)

object AudioConfigurationNormalizer {
    val defaults = AudioConfigurationV3()

    fun normalize(value: Any?): AudioConfigurationV3 {
        val raw = value.asStringMap()
        val recognitionRaw = raw["recognition"].asStringMap()
        val speechRaw = raw["speech"].asStringMap()
        val recognition = AudioRecognitionPreference(source(recognitionRaw["source"]), modelId(recognitionRaw["offlineModelId"]))
        var speech = AudioSpeechPreference(source(speechRaw["source"]), modelId(speechRaw["offlineModelId"]), voice(speechRaw["voice"]))
        if (speech.source == AudioSource.AUTOMATIC && speech.voice != null) {
            speech = speech.copy(
                source = speech.voice.source,
                offlineModelId = speech.offlineModelId ?: speech.voice.takeIf { it.source == AudioSource.OFFLINE }?.modelId,
            )
        }
        return AudioConfigurationV3(
            recognition = recognition,
            speech = speech,
            language = language(raw["language"]),
            rate = rate(raw["rate"]),
            autoPlayReplies = raw["autoPlayReplies"] == true,
        )
    }

    fun migrateLegacy(value: Any?, voiceCatalog: List<AudioVoiceCatalogEntry> = emptyList()): AudioConfigurationV3 {
        val raw = value.asStringMap()
        if ((raw["schemaVersion"] as? Number)?.toInt() == AUDIO_CONFIGURATION_SCHEMA_VERSION) return normalize(raw)
        val recognitionRaw = raw["recognition"].asStringMap()
        val speechRaw = raw["speech"].asStringMap()
        val recognitionSource = legacySource(recognitionRaw["source"] ?: first(raw, "inputProvider", "recognitionMode"), AudioSource.AUTOMATIC)
        val speechSource = legacySource(speechRaw["source"] ?: first(raw, "outputProvider", "speechProvider"), AudioSource.AUTOMATIC)
        val rawVoice = speechRaw["voice"] ?: first(raw, "voiceSelection", "voiceId", "voice")
        val selectedVoice = if (rawVoice is String) legacyVoice(rawVoice, voiceCatalog) else voice(rawVoice)
        var languageValue: Any? = first(raw, "language", "inputLanguage", "legacyInputLanguage", "voiceLanguage", "voiceLang")
        if ((languageValue == null || languageValue == "auto") && raw["legacyVoiceLang"] is String) {
            languageValue = when ((raw["legacyVoiceLang"] as String).lowercase()) {
                "zh" -> "zh-CN"
                "en" -> "en-US"
                else -> languageValue
            }
        }
        var speech = AudioSpeechPreference(
            source = speechSource,
            offlineModelId = modelId(speechRaw["offlineModelId"] ?: first(raw, "speechModelId", "outputModelId")),
            voice = selectedVoice,
        )
        if (speech.source == AudioSource.AUTOMATIC && selectedVoice != null) {
            speech = speech.copy(
                source = selectedVoice.source,
                offlineModelId = speech.offlineModelId ?: selectedVoice.takeIf { it.source == AudioSource.OFFLINE }?.modelId,
            )
        }
        return normalize(
            mapOf(
                "schemaVersion" to AUDIO_CONFIGURATION_SCHEMA_VERSION,
                "recognition" to mapOf(
                    "source" to recognitionSource.value,
                    "offlineModelId" to (recognitionRaw["offlineModelId"] ?: first(raw, "recognitionModelId", "inputModelId")),
                ),
                "speech" to mapOf("source" to speech.source.value, "offlineModelId" to speech.offlineModelId, "voice" to voiceMap(speech.voice)),
                "language" to (languageValue ?: AUDIO_LANGUAGE_AUTO),
                "rate" to (first(raw, "rate", "speed", "voiceSpeed") ?: AUDIO_DEFAULT_RATE),
                "autoPlayReplies" to (first(raw, "autoPlayReplies", "autoPlay", "voiceAutoPlay") ?: false),
            ),
        )
    }

    private fun source(value: Any?): AudioSource {
        val text = (value as? String)?.trim().orEmpty()
        return AudioSource(text.ifEmpty { "automatic" })
    }
    private fun modelId(value: Any?): String? = (value as? String)?.takeIf { it.isNotEmpty() }
    private fun language(value: Any?): String {
        val text = (value as? String)?.trim().orEmpty()
        return if (text.isEmpty() || text.equals(AUDIO_LANGUAGE_AUTO, true)) AUDIO_LANGUAGE_AUTO else text
    }
    private fun rate(value: Any?): Double {
        val number = (value as? Number)?.toDouble()?.takeIf { it.isFinite() } ?: AUDIO_DEFAULT_RATE
        return number.coerceIn(AUDIO_MIN_RATE, AUDIO_MAX_RATE)
    }
    private fun voice(value: Any?): AudioVoiceSelection? {
        if (value is String) return legacyVoice(value, emptyList())
        val raw = value.asStringMap()
        val id = raw["id"] as? String ?: return null
        if (id.isEmpty()) return null
        val selectedSource = source(raw["source"])
        if (selectedSource == AudioSource.OFFLINE) {
            return AudioVoiceSelection(selectedSource, id, modelId(raw["modelId"]))
        }
        return AudioVoiceSelection(selectedSource, id)
    }
    private fun legacySource(value: Any?, fallback: AudioSource): AudioSource {
        val text = (value as? String)?.trim()?.takeIf { it.isNotEmpty() } ?: return fallback
        return when (text.lowercase()) {
            "automatic", "auto" -> AudioSource.AUTOMATIC
            "system" -> AudioSource.SYSTEM
            "offline", "localonly", "on-device", "ondevice" -> AudioSource.OFFLINE
            else -> AudioSource(text)
        }
    }
    private fun uniqueMatch(catalog: List<AudioVoiceCatalogEntry>, selector: String): AudioVoiceCatalogEntry? {
        val needle = selector.lowercase()
        val matches = catalog.filter { (listOfNotNull(it.id, it.label) + it.aliases).any { name -> name.lowercase() == needle } }
        return matches.singleOrNull()
    }
    private fun legacyVoice(value: String, catalog: List<AudioVoiceCatalogEntry>): AudioVoiceSelection? {
        val text = value.trim()
        if (text.isEmpty()) return null
        if (text.startsWith("system:")) {
            val id = text.removePrefix("system:")
            val match = uniqueMatch(catalog.filter { it.source == AudioSource.SYSTEM }, id)
            return AudioVoiceSelection(AudioSource.SYSTEM, match?.id ?: id)
        }
        if (text.startsWith("sherpa:")) {
            val payload = text.removePrefix("sherpa:")
            val split = payload.indexOf(':')
            val modelKey = if (split < 0) payload else payload.substring(0, split)
            val voiceKey = if (split < 0) payload else payload.substring(split + 1)
            val offline = catalog.filter { it.source == AudioSource.OFFLINE }
            val match = uniqueMatch(offline, "$modelKey:$voiceKey") ?: uniqueMatch(offline, voiceKey)
            if (match?.modelId != null && (match.modelId == modelKey || match.modelId.endsWith(modelKey))) {
                return AudioVoiceSelection(AudioSource.OFFLINE, match.id, match.modelId)
            }
            return AudioVoiceSelection(AudioSource.OFFLINE, voiceKey, modelKey)
        }
        if (text == "default") return AudioVoiceSelection(AudioSource.SYSTEM, "default")
        val match = uniqueMatch(catalog, text)
        if (match != null) return AudioVoiceSelection(match.source, match.id, match.modelId)
        return AudioVoiceSelection(AudioSource.SYSTEM, text)
    }
    private fun voiceMap(value: AudioVoiceSelection?): Map<String, Any?>? = value?.let {
        mapOf("source" to it.source.value, "id" to it.id, "modelId" to it.modelId)
    }
    private fun first(raw: Map<String, Any?>, vararg names: String): Any? = names.firstNotNullOfOrNull { raw[it] }
}

private fun Any?.asStringMap(): Map<String, Any?> = (this as? Map<*, *>)
    ?.entries
    ?.filter { it.key is String }
    ?.associate { it.key as String to it.value }
    ?: emptyMap()

fun resolveAudioLanguage(configured: String, deviceLocale: String?): String {
    val selected = configured.trim()
    if (selected.isNotEmpty() && !selected.equals(AUDIO_LANGUAGE_AUTO, true)) return selected
    return deviceLocale?.trim()?.takeIf { it.isNotEmpty() } ?: "en-US"
}

private fun audioLanguageMatches(supported: List<String>, requested: String): Boolean {
    val language = requested.lowercase()
    return supported.any { candidate ->
        val normalized = candidate.lowercase()
        language == normalized || language.startsWith("$normalized-")
    }
}

private fun AudioRouteRequest.requested() = RequestedAudioRoute(source, offlineModelId, voice)
private fun audioUnavailable(request: AudioRouteRequest, reason: String, status: AudioRouteStatus = AudioRouteStatus.UNAVAILABLE) =
    AudioRouteResolution(request.requested(), null, status, reason)

private fun audioSystemRoute(request: AudioRouteRequest, voice: AudioVoiceSelection?): AudioRouteResolution {
    if (request.systemStatus == AudioReadiness.AVAILABLE || request.systemStatus == AudioReadiness.PERMISSION_REQUIRED) {
        val voiceId = voice?.id?.takeUnless { it == "default" }
        if (voiceId != null && request.systemVoiceIds != null && voiceId !in request.systemVoiceIds) return audioUnavailable(request, "systemVoiceUnknown")
        val status = if (request.systemStatus == AudioReadiness.AVAILABLE) AudioRouteStatus.READY else AudioRouteStatus.PERMISSION_REQUIRED
        return AudioRouteResolution(request.requested(), EffectiveAudioRoute(AudioSource.SYSTEM, null, voiceId), status, if (status == AudioRouteStatus.READY) "ready" else "systemPermissionRequired")
    }
    return audioUnavailable(request, if (request.systemStatus == AudioReadiness.DENIED) "systemDenied" else "systemUnavailable")
}

private fun audioOfflineRoute(request: AudioRouteRequest, modelId: String?, voice: AudioVoiceSelection?): AudioRouteResolution {
    val model: AudioOfflineModelAvailability
    if (modelId != null) {
        model = request.offlineModels.firstOrNull { it.id == modelId } ?: return audioUnavailable(request, "offlineModelUnknown")
        if (model.kind != request.kind) return audioUnavailable(request, "offlineModelKindMismatch")
        if (!audioLanguageMatches(model.languages, request.language)) return audioUnavailable(request, "offlineModelUnsupportedLanguage")
        if (!model.installed) return audioUnavailable(request, "offlineModelNotInstalled")
    } else {
        val compatible = request.offlineModels.filter { it.kind == request.kind && audioLanguageMatches(it.languages, request.language) }
        model = compatible.firstOrNull { it.installed } ?: return audioUnavailable(request, if (compatible.isEmpty()) "noCompatibleOfflineModel" else "offlineModelNotInstalled")
    }
    if (voice?.source == AudioSource.OFFLINE) {
        if (voice.modelId != model.id) return audioUnavailable(request, "offlineModelConflict", AudioRouteStatus.INVALID_REQUEST)
        if (model.voiceIds != null && voice.id !in model.voiceIds) return audioUnavailable(request, "offlineVoiceUnknown")
    }
    return AudioRouteResolution(request.requested(), EffectiveAudioRoute(AudioSource.OFFLINE, model.id, voice?.takeIf { it.source == AudioSource.OFFLINE }?.id), AudioRouteStatus.READY, "ready")
}

fun resolveAudioRoute(request: AudioRouteRequest): AudioRouteResolution {
    if (request.language.isBlank() || request.language.equals(AUDIO_LANGUAGE_AUTO, true)) return audioUnavailable(request, "languageUnresolved", AudioRouteStatus.INVALID_REQUEST)
    val source = request.source
    val voice = request.voice.takeIf { request.kind == AudioProviderKind.SPEECH }
    if (source != AudioSource.AUTOMATIC && source != AudioSource.SYSTEM && source != AudioSource.OFFLINE) return audioUnavailable(request, "unsupportedSource")
    if (voice != null && voice.source != AudioSource.SYSTEM && voice.source != AudioSource.OFFLINE) return audioUnavailable(request, "unsupportedVoiceSource", AudioRouteStatus.INVALID_REQUEST)
    if (voice != null && source != AudioSource.AUTOMATIC && voice.source != source) return audioUnavailable(request, "voiceSourceMismatch", AudioRouteStatus.INVALID_REQUEST)
    if (voice?.source == AudioSource.OFFLINE && request.offlineModelId != null && request.offlineModelId != voice.modelId) return audioUnavailable(request, "offlineModelConflict", AudioRouteStatus.INVALID_REQUEST)
    if (source == AudioSource.SYSTEM || (source == AudioSource.AUTOMATIC && voice?.source == AudioSource.SYSTEM)) return audioSystemRoute(request, voice?.takeIf { it.source == AudioSource.SYSTEM })
    if (source == AudioSource.OFFLINE || (source == AudioSource.AUTOMATIC && voice?.source == AudioSource.OFFLINE)) {
        return audioOfflineRoute(request, voice?.takeIf { it.source == AudioSource.OFFLINE }?.modelId ?: request.offlineModelId, voice)
    }
    val system = audioSystemRoute(request, null)
    if (system.status == AudioRouteStatus.READY || system.status == AudioRouteStatus.PERMISSION_REQUIRED) return system
    return audioOfflineRoute(request, request.offlineModelId, null).copy(fallbackReason = system.reason)
}

fun isAudioFallbackAllowed(failure: AudioFallbackFailure, operationStarted: Boolean): Boolean =
    !operationStarted && (failure == AudioFallbackFailure.PERMISSION || failure == AudioFallbackFailure.UNAVAILABLE)
