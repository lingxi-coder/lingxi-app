package com.lingxi.code.theme

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.doublePreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
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

/** UI density (matches the prototype's Appearance density radio). */
enum class Density(val raw: String) {
    Comfortable("comfortable"),
    Compact("compact");

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
)

private object PrefKeys {
    val THEME = stringPreferencesKey("theme")
    val ACCENT = stringPreferencesKey("accent")
    val DENSITY = stringPreferencesKey("density")
    val FONT_SIZE = doublePreferencesKey("fontSize")
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
}
