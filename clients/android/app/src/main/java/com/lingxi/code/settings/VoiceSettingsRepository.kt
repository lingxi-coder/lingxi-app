package com.lingxi.code.settings

import android.content.Context
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.theme.AppearanceStore
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking

/**
 * Process-safe shared source of truth for chat voice and Computer Use audio.
 * Secrets are intentionally absent: system STT/TTS needs no API credential.
 */
class VoiceSettingsRepository(context: Context) {
    private val appContext = context.applicationContext
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES_NAME,
        Context.MODE_PRIVATE,
    )

    fun load(): VoiceConfig {
        val schemaVersion = preferences.getInt(KEY_SCHEMA_VERSION, 0)
        val legacyVoiceLanguage = if (schemaVersion < VoiceConfig.CURRENT_SCHEMA_VERSION) {
            runBlocking { AppearanceStore(appContext).prefs.first().voiceLang }
        } else {
            null
        }
        val config = resolveVoiceConfig(
            schemaVersion = schemaVersion,
            recognitionMode = preferences.getString(KEY_RECOGNITION_MODE, null),
            language = preferences.getString(KEY_LANGUAGE, null),
            voiceSelection = preferences.getString(KEY_VOICE_SELECTION, null),
            rate = if (preferences.contains(KEY_RATE)) preferences.getFloat(KEY_RATE, 1.0f) else null,
            autoPlayReplies = if (preferences.contains(KEY_AUTO_PLAY_REPLIES)) {
                preferences.getBoolean(KEY_AUTO_PLAY_REPLIES, false)
            } else {
                null
            },
            legacyInputLanguage = preferences.getString(KEY_INPUT_LANGUAGE, null),
            legacyVoiceId = preferences.getString(KEY_VOICE, null),
            legacySpeed = if (preferences.contains(KEY_SPEED)) preferences.getFloat(KEY_SPEED, 1.0f) else null,
            legacyAutoPlay = if (preferences.contains(KEY_AUTO_PLAY)) preferences.getBoolean(KEY_AUTO_PLAY, false) else null,
            legacyVoiceLang = legacyVoiceLanguage,
        )
        save(config)
        return config
    }

    fun save(config: VoiceConfig) {
        preferences.edit()
            .putInt(KEY_SCHEMA_VERSION, VoiceConfig.CURRENT_SCHEMA_VERSION)
            .putString(KEY_RECOGNITION_MODE, normalizeRecognitionMode(config.recognitionMode))
            .putString(KEY_LANGUAGE, normalizeLanguage(config.language))
            .putString(KEY_VOICE_SELECTION, normalizeVoiceSelection(config.voiceSelection))
            .putFloat(KEY_RATE, config.rate.coerceIn(0.5f, 2.0f))
            .putBoolean(KEY_AUTO_PLAY_REPLIES, config.autoPlayReplies)
            .apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "voice_settings"
        const val KEY_SCHEMA_VERSION = "schema_version"
        const val KEY_RECOGNITION_MODE = "recognition_mode"
        const val KEY_LANGUAGE = "language"
        const val KEY_VOICE_SELECTION = "voice_selection"
        const val KEY_RATE = "rate"
        const val KEY_AUTO_PLAY_REPLIES = "auto_play_replies"
        const val KEY_INPUT_PROVIDER = "input_provider"
        const val KEY_INPUT_LANGUAGE = "input_language"
        const val KEY_OUTPUT_PROVIDER = "output_provider"
        const val KEY_VOICE = "voice"
        const val KEY_SPEED = "speed"
        const val KEY_AUTO_PLAY = "auto_play"
    }
}

internal fun resolveVoiceConfig(
    schemaVersion: Int,
    recognitionMode: String?,
    language: String?,
    voiceSelection: String?,
    rate: Float?,
    autoPlayReplies: Boolean?,
    legacyInputLanguage: String?,
    legacyVoiceId: String?,
    legacySpeed: Float?,
    legacyAutoPlay: Boolean?,
    legacyVoiceLang: String?,
): VoiceConfig {
    if (schemaVersion >= VoiceConfig.CURRENT_SCHEMA_VERSION) {
        return VoiceConfig(
            schemaVersion = VoiceConfig.CURRENT_SCHEMA_VERSION,
            recognitionMode = normalizeRecognitionMode(recognitionMode),
            language = normalizeLanguage(language),
            voiceSelection = normalizeVoiceSelection(voiceSelection),
            rate = (rate ?: 1.0f).coerceIn(0.5f, 2.0f),
            autoPlayReplies = autoPlayReplies ?: false,
        )
    }

    val migratedLanguage = when {
        !legacyInputLanguage.isNullOrBlank() && legacyInputLanguage != VoiceConfig.LANGUAGE_AUTO ->
            normalizeLanguage(legacyInputLanguage)
        legacyVoiceLang.equals("zh", ignoreCase = true) -> "zh-CN"
        legacyVoiceLang.equals("en", ignoreCase = true) -> "en-US"
        else -> VoiceConfig.LANGUAGE_AUTO
    }
    return VoiceConfig(
        schemaVersion = VoiceConfig.CURRENT_SCHEMA_VERSION,
        recognitionMode = VoiceConfig.MODE_AUTOMATIC,
        language = migratedLanguage,
        voiceSelection = migrateLegacyVoiceSelection(legacyVoiceId),
        rate = (legacySpeed ?: 1.0f).coerceIn(0.5f, 2.0f),
        autoPlayReplies = legacyAutoPlay ?: false,
    )
}

internal fun normalizeRecognitionMode(raw: String?): String =
    when (raw) {
        VoiceConfig.MODE_LOCAL_ONLY -> VoiceConfig.MODE_LOCAL_ONLY
        else -> VoiceConfig.MODE_AUTOMATIC
    }

internal fun normalizeLanguage(raw: String?): String {
    val trimmed = raw?.trim().orEmpty()
    if (trimmed.isEmpty() || trimmed.equals(VoiceConfig.LANGUAGE_AUTO, ignoreCase = true)) {
        return VoiceConfig.LANGUAGE_AUTO
    }
    return trimmed
}

internal fun normalizeVoiceSelection(raw: String?): String {
    val trimmed = raw?.trim().orEmpty()
    if (trimmed.isEmpty()) return VoiceConfig.DEFAULT_VOICE_SELECTION
    if (trimmed == VoiceConfig.DEFAULT_VOICE_ID) return VoiceConfig.DEFAULT_VOICE_SELECTION
    if (trimmed == "zh-CN-XiaoxiaoNeural") return VoiceConfig.DEFAULT_VOICE_SELECTION
    return when {
        trimmed.startsWith(VoiceConfig.SYSTEM_VOICE_PREFIX) -> trimmed
        trimmed.startsWith(VoiceConfig.SHERPA_VOICE_PREFIX) -> trimmed
        else -> VoiceConfig.systemVoiceSelection(trimmed)
    }
}

private fun migrateLegacyVoiceSelection(legacyVoiceId: String?): String =
    normalizeVoiceSelection(legacyVoiceId ?: VoiceConfig.DEFAULT_VOICE_ID)
