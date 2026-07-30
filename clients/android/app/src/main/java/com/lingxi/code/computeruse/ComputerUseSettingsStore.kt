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
        appSelections = decodeComputerUseAppSelections(
            preferences.getStringSet(KEY_APP_SELECTIONS, emptySet()).orEmpty(),
        ),
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
            .putStringSet(
                KEY_APP_SELECTIONS,
                encodeComputerUseAppSelections(configuration.appSelections),
            )
            .apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "computer_use_settings"
        const val KEY_LISTEN_ENABLED = "listen_enabled"
        const val KEY_SPEAK_ENABLED = "speak_enabled"
        const val KEY_MAX_LISTEN_SECONDS = "max_listen_seconds"
        const val KEY_APP_SELECTIONS = "app_selections"
        const val MIN_LISTEN_SECONDS = 5
        const val MAX_LISTEN_SECONDS = 60
    }
}

private const val APP_SELECTION_SEPARATOR = '|'

internal fun encodeComputerUseAppSelections(
    selections: Map<String, ComputerUseTier>,
): Set<String> = selections
    .asSequence()
    .filter { (packageName, _) -> packageName.isNotBlank() }
    .map { (packageName, tier) ->
        "$packageName$APP_SELECTION_SEPARATOR${tier.name}"
    }
    .toSortedSet()

internal fun decodeComputerUseAppSelections(
    encoded: Set<String>,
): Map<String, ComputerUseTier> = buildMap {
    encoded.sorted().forEach { entry ->
        val parts = entry.split(APP_SELECTION_SEPARATOR, limit = 2)
        val packageName = parts.getOrNull(0)?.takeIf(String::isNotBlank) ?: return@forEach
        val tierName = parts.getOrNull(1) ?: return@forEach
        val tier = ComputerUseTier.entries.firstOrNull { it.name == tierName }
            ?: return@forEach
        put(packageName, tier)
    }
}
