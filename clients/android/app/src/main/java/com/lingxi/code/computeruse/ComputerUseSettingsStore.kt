package com.lingxi.code.computeruse

import android.content.Context

internal class ComputerUseSettingsStore(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES_NAME,
        Context.MODE_PRIVATE,
    )

    fun load(): ComputerUseConfiguration = ComputerUseConfiguration(
        listenEnabled = preferences.getBoolean(KEY_LISTEN_ENABLED, false),
        speakEnabled = preferences.getBoolean(KEY_SPEAK_ENABLED, true),
        maxListenSeconds = preferences.getInt(KEY_MAX_LISTEN_SECONDS, 15)
            .coerceIn(MIN_LISTEN_SECONDS, MAX_LISTEN_SECONDS),
    )

    fun save(configuration: ComputerUseConfiguration) {
        preferences.edit()
            .putBoolean(KEY_LISTEN_ENABLED, configuration.listenEnabled)
            .putBoolean(KEY_SPEAK_ENABLED, configuration.speakEnabled)
            .putInt(
                KEY_MAX_LISTEN_SECONDS,
                configuration.maxListenSeconds.coerceIn(
                    MIN_LISTEN_SECONDS,
                    MAX_LISTEN_SECONDS,
                ),
            )
            .apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "computer_use_settings"
        const val KEY_LISTEN_ENABLED = "listen_enabled"
        const val KEY_SPEAK_ENABLED = "speak_enabled"
        const val KEY_MAX_LISTEN_SECONDS = "max_listen_seconds"
        const val MIN_LISTEN_SECONDS = 5
        const val MAX_LISTEN_SECONDS = 60
    }
}
