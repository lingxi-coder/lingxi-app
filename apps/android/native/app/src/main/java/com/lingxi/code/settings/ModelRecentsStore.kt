package com.lingxi.code.settings

import android.content.Context

/**
 * The models the user has most recently PICKED, newest first.
 *
 * Client-local on purpose. The engine keeps its own recents list
 * (`tui-core::recent_models` -> `~/.lingxi/settings.json`'s `recentModels`), but
 * nothing carries it over `ClientEvent::ModelList`, so surfacing it here would
 * mean a protocol change and a four-client rollout. Desktop and phone recents
 * are therefore independent; that is an accepted divergence, not an oversight.
 *
 * Entries are the provider-QUALIFIED reference (`anthropic/claude-sonnet-5`) —
 * byte-identical to what `ModelList` carries and what `SetModel` submits — so a
 * stored entry can be matched against the live catalog by plain equality. The
 * iOS twin is `apps/ios/native/Sources/Conversation/ModelRecents.swift`; keep the
 * two in step.
 */
class ModelRecentsStore(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES_NAME,
        Context.MODE_PRIVATE,
    )

    /** The remembered references, most-recently-picked first. */
    fun references(): List<String> =
        preferences.getString(KEY_RECENTS, null)
            ?.split(SEPARATOR)
            ?.filter { it.isNotBlank() }
            .orEmpty()

    /**
     * Record an explicit user pick: move it to the front, keep one entry per
     * reference, and forget the oldest beyond [LIMIT].
     *
     * Only a deliberate selection belongs here. The active model also changes on
     * every `ModelList` / `ModelChanged` the engine emits (boot, session resume,
     * a provider reconnect), and recording those would fill the list with models
     * the user never chose.
     */
    fun record(reference: String) {
        val trimmed = reference.trim()
        if (trimmed.isEmpty()) return
        val next = (listOf(trimmed) + references().filter { it != trimmed }).take(LIMIT)
        preferences.edit().putString(KEY_RECENTS, next.joinToString(SEPARATOR)).apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "lingxi_model_recents"
        const val KEY_RECENTS = "recent_model_refs"

        /**
         * A newline, not a comma: a model reference may contain almost anything
         * except a line break (OpenRouter ids carry `/`, `~`, `:` and `.`), so a
         * comma-joined list would round-trip wrong for real catalog entries.
         */
        const val SEPARATOR = "\n"

        /**
         * How many picks to remember. The engine's list keeps 8; 5 is enough to
         * stay useful without letting the picker's "recent" group crowd out the
         * provider groups below it. Matches the iOS `ModelRecents.limit`.
         */
        const val LIMIT = 5
    }
}
