package com.lingxi.code.theme

import android.content.Context
import android.content.res.Configuration
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import java.util.Locale
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.runBlocking

/** Single app-wide language DataStore (delegated extension on [Context]). */
val Context.languageDataStore: DataStore<Preferences> by preferencesDataStore(name = "language")

/** Process-wide scope backing the language [StateFlow]; lives for the app. */
private val languageStoreScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

/**
 * Supported in-app languages. The empty code means "follow the system locale"
 * (the persisted default). These are the iOS client's language options; the
 * labels stay hardcoded here — Task 12 extracts them to resources.
 */
object AppLanguage {
    val SUPPORTED = listOf(
        "" to "跟随系统",
        "zh-CN" to "简体中文",
        "zh-TW" to "繁體中文",
        "en-US" to "English",
        "ja-JP" to "日本語",
        "ko-KR" to "한국어",
    )

    /** Maps an in-app code to the BCP-47 tag [Locale.forLanguageTag] accepts ("" = system). */
    fun canonical(code: String): String = when (code) {
        "zh-CN" -> "zh-Hans"
        "zh-TW" -> "zh-Hant"
        "en-US" -> "en"
        "ja-JP" -> "ja"
        "ko-KR" -> "ko"
        else -> ""
    }

    /** Human-readable label for a stored code, defaulting to the follow-system option. */
    fun label(code: String): String =
        SUPPORTED.firstOrNull { it.first == code }?.second ?: SUPPORTED.first().second
}

/**
 * Persists the user's in-app language choice via DataStore Preferences — the
 * language analog of [AppearanceStore]. The [language] flow is the live
 * persisted selection; [LocaleWrapper.wrap] applies it to the Activity base
 * context so every resource reloads in the forced locale.
 */
class AppLanguageStore(context: Context) {
    private val dataStore = context.languageDataStore

    /** Live persisted language code, defaulting to "" (follow system). */
    val language: StateFlow<String> = dataStore.data
        .map { it[LANGUAGE_KEY] ?: "" }
        .stateIn(languageStoreScope, SharingStarted.Eagerly, currentLanguage(context))

    /** Persists [code] ("" = follow system). The host calls [android.app.Activity.recreate] after. */
    suspend fun setLanguage(code: String) {
        dataStore.edit { it[LANGUAGE_KEY] = code }
        cached = code
    }

    companion object {
        private val LANGUAGE_KEY = stringPreferencesKey("app_language")

        /** Last known persisted code; warmed by [LocaleWrapper.wrap] in attachBaseContext. */
        @Volatile
        private var cached: String? = null

        /**
         * Synchronous read of the persisted code. [LocaleWrapper.wrap] performs
         * the one blocking DataStore read at attachBaseContext time (before any
         * composition) and every later caller hits the warm [cached] value, so
         * the UI thread never blocks during frame composition.
         */
        fun currentLanguage(context: Context): String =
            cached ?: runBlocking { context.languageDataStore.data.first()[LANGUAGE_KEY] ?: "" }
                .also { cached = it }
    }
}

/**
 * Applies the persisted in-app language to a base Context: builds a per-locale
 * [Configuration] and returns [Context.createConfigurationContext] (API 26+;
 * no AppCompat dependency). Called from [android.app.Activity.attachBaseContext].
 */
object LocaleWrapper {
    fun wrap(base: Context): Context {
        val canonical = AppLanguage.canonical(AppLanguageStore.currentLanguage(base))
        if (canonical.isEmpty()) return base
        val config = Configuration(base.resources.configuration)
        config.setLocale(Locale.forLanguageTag(canonical))
        return base.createConfigurationContext(config)
    }
}
