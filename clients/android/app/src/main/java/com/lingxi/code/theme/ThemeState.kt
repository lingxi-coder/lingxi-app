package com.lingxi.code.theme

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.doublePreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.lingxi.code.R
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

/**
 * Appearance preferences persisted with DataStore — the Android analog of the
 * iOS `AppState` + `@AppStorage`. Holds the user-facing theme toggles (theme
 * mode, accent, density, ui text size) and is the source the [LingXiTheme]
 * composable observes.
 */

/** Single app-wide preferences DataStore (delegated extension on [Context]). */
val Context.appearanceDataStore: DataStore<Preferences> by preferencesDataStore(name = "appearance")

/** Theme appearance modes. The "system" option follows the OS setting. */
enum class ThemeMode(val raw: String) {
    Dark("dark"),
    Light("light"),
    System("system");

    companion object {
        fun from(raw: String?): ThemeMode = entries.firstOrNull { it.raw == raw } ?: Dark
    }
}

/** UI density (matches the prototype's Appearance density radio: 紧凑/舒适/宽松). */
enum class Density(val raw: String) {
    Compact("compact"),
    Comfortable("comfortable"),
    Spacious("spacious");

    companion object {
        fun from(raw: String?): Density = entries.firstOrNull { it.raw == raw } ?: Comfortable
    }
}

/** A snapshot of the persisted appearance preferences. */
data class AppearancePrefs(
    val themeMode: ThemeMode = ThemeMode.Dark,
    val accentId: String = Accents.DEFAULT_ID,
    val density: Density = Density.Comfortable,
    val fontSize: Float = 15f,
    // First-run profile + onboarding (the prototype's `lx_settings` blob +
    // `lx_setup_done` flag). The SetupWizard writes these once; the FlowMode orb
    // + drawer + settings read them back. This literal `"灵犀"` default stays:
    // it's a plain data class with no `Context`, and the DURABLE value flows
    // through `AppearanceStore.prefs` below, which resolves the real
    // localized `app_name` string. This field default is reachable only as
    // the transient `collectAsState(initial = AppearancePrefs())` seed
    // (MainActivity.kt / AppearancePage.kt) for the single frame before the
    // DataStore flow first emits.
    val assistantName: String = "灵犀", // wake-word name
    val userName: String = "",          // how the assistant addresses the user
    val voiceprint: Boolean = false,    // voiceprint enrolled in onboarding
    val defaultModelId: String = "lx-72b",
    val flowDefault: Boolean = true,    // open FlowMode by default for voice
    val inputDialog: Boolean = true,    // show the FlowMode pop-up text input
    val setupDone: Boolean = false,     // first-run setup complete
    val voiceLang: String = "",         // chosen offline voice-pack language ("zh"/"en"/"")
)

private object PrefKeys {
    val THEME = stringPreferencesKey("theme")
    val ACCENT = stringPreferencesKey("accent")
    val DENSITY = stringPreferencesKey("density")
    val FONT_SIZE = doublePreferencesKey("fontSize")
    val ASSISTANT_NAME = stringPreferencesKey("assistantName")
    val USER_NAME = stringPreferencesKey("userName")
    val VOICEPRINT = booleanPreferencesKey("voiceprint")
    val DEFAULT_MODEL = stringPreferencesKey("defaultModelId")
    val FLOW_DEFAULT = booleanPreferencesKey("flowDefault")
    val INPUT_DIALOG = booleanPreferencesKey("inputDialog")
    val SETUP_DONE = booleanPreferencesKey("setupDone")
    val VOICE_LANG = stringPreferencesKey("voiceLang")
}

/**
 * Reads/writes [AppearancePrefs] from the [appearanceDataStore]. A thin
 * repository so the theme layer owns its persistence and screens (Appearance)
 * mutate via suspend setters. Construct once (e.g. in the Application/ViewModel)
 * and hoist the [prefs] flow into [LingXiTheme].
 */
class AppearanceStore(private val context: Context) {

    /** Live preferences, defaulting to the brand dark + 靛紫 accent. */
    val prefs: Flow<AppearancePrefs> = context.appearanceDataStore.data.map { p ->
        AppearancePrefs(
            themeMode = ThemeMode.from(p[PrefKeys.THEME]),
            accentId = p[PrefKeys.ACCENT] ?: Accents.DEFAULT_ID,
            density = Density.from(p[PrefKeys.DENSITY]),
            fontSize = (p[PrefKeys.FONT_SIZE] ?: 15.0).toFloat(),
            assistantName = p[PrefKeys.ASSISTANT_NAME] ?: context.getString(R.string.app_name),
            userName = p[PrefKeys.USER_NAME] ?: "",
            voiceprint = p[PrefKeys.VOICEPRINT] ?: false,
            defaultModelId = p[PrefKeys.DEFAULT_MODEL] ?: "lx-72b",
            flowDefault = p[PrefKeys.FLOW_DEFAULT] ?: true,
            inputDialog = p[PrefKeys.INPUT_DIALOG] ?: true,
            setupDone = p[PrefKeys.SETUP_DONE] ?: false,
            voiceLang = p[PrefKeys.VOICE_LANG] ?: "",
        )
    }

    suspend fun setThemeMode(mode: ThemeMode) {
        context.appearanceDataStore.edit { it[PrefKeys.THEME] = mode.raw }
    }

    suspend fun setAccent(id: String) {
        context.appearanceDataStore.edit { it[PrefKeys.ACCENT] = id }
    }

    suspend fun setDensity(density: Density) {
        context.appearanceDataStore.edit { it[PrefKeys.DENSITY] = density.raw }
    }

    suspend fun setFontSize(size: Float) {
        context.appearanceDataStore.edit { it[PrefKeys.FONT_SIZE] = size.toDouble() }
    }

    suspend fun setVoiceLang(lang: String) {
        context.appearanceDataStore.edit { it[PrefKeys.VOICE_LANG] = lang }
    }

    // First-run profile / onboarding setters.
    suspend fun setAssistantName(name: String) {
        context.appearanceDataStore.edit { it[PrefKeys.ASSISTANT_NAME] = name }
    }
    suspend fun setUserName(name: String) {
        context.appearanceDataStore.edit { it[PrefKeys.USER_NAME] = name }
    }
    suspend fun setVoiceprint(enrolled: Boolean) {
        context.appearanceDataStore.edit { it[PrefKeys.VOICEPRINT] = enrolled }
    }
    suspend fun setDefaultModel(id: String) {
        context.appearanceDataStore.edit { it[PrefKeys.DEFAULT_MODEL] = id }
    }
    suspend fun setInputDialog(on: Boolean) {
        context.appearanceDataStore.edit { it[PrefKeys.INPUT_DIALOG] = on }
    }
    suspend fun setSetupDone(done: Boolean) {
        context.appearanceDataStore.edit { it[PrefKeys.SETUP_DONE] = done }
    }
}
