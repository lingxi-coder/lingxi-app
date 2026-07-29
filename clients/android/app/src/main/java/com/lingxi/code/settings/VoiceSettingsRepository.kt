package com.lingxi.code.settings

import android.content.Context
import com.lingxi.code.model.VoiceConfig

/**
 * Process-safe shared source of truth for chat voice and Computer Use audio.
 * Secrets are intentionally absent: system STT/TTS needs no API credential.
 */
class VoiceSettingsRepository(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES_NAME,
        Context.MODE_PRIVATE,
    )

    fun load(): VoiceConfig {
        val storedVoice = preferences.getString(KEY_VOICE, "default") ?: "default"
        return VoiceConfig(
            inputProvider = "system",
            inputLanguage = preferences.getString(KEY_INPUT_LANGUAGE, "auto") ?: "auto",
            // Android currently has one real STT/TTS implementation. Do not
            // resurrect legacy cloud-provider rows that never had a working
            // credential or transport behind them.
            preset = "system",
            voiceId = storedVoice.takeUnless { it == "zh-CN-XiaoxiaoNeural" } ?: "default",
            speed = preferences.getFloat(KEY_SPEED, 1.0f).coerceIn(0.5f, 2.0f),
            autoPlay = preferences.getBoolean(KEY_AUTO_PLAY, false),
        )
    }

    fun save(config: VoiceConfig) {
        preferences.edit()
            .putString(KEY_INPUT_PROVIDER, config.inputProvider)
            .putString(KEY_INPUT_LANGUAGE, config.inputLanguage)
            .putString(KEY_OUTPUT_PROVIDER, config.preset)
            .putString(KEY_VOICE, config.voiceId)
            .putFloat(KEY_SPEED, config.speed.coerceIn(0.5f, 2.0f))
            .putBoolean(KEY_AUTO_PLAY, config.autoPlay)
            .apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "voice_settings"
        const val KEY_INPUT_PROVIDER = "input_provider"
        const val KEY_INPUT_LANGUAGE = "input_language"
        const val KEY_OUTPUT_PROVIDER = "output_provider"
        const val KEY_VOICE = "voice"
        const val KEY_SPEED = "speed"
        const val KEY_AUTO_PLAY = "auto_play"
    }
}
