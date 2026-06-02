package com.lingxi.code

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import com.lingxi.code.theme.AppearancePrefs
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.ThemeMode
import kotlinx.coroutines.launch

/**
 * The single Activity host. Edge-to-edge, Compose-only — everything else
 * (drawer, conversation, settings nav graph, voice overlay) is composed under
 * [RootScreen].
 *
 * Theme + accent are read from the DataStore-backed [AppearanceStore] and fed
 * into [LingXiTheme], so the appearance is persisted and live. Later phases fill
 * in [RootScreen] and let the Appearance page mutate the same store.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            val store = remember { AppearanceStore(applicationContext) }
            val scope = rememberCoroutineScope()
            val prefs by store.prefs.collectAsState(initial = AppearancePrefs())
            val darkTheme = when (prefs.themeMode) {
                ThemeMode.Dark -> true
                ThemeMode.Light -> false
                ThemeMode.System -> isSystemInDarkTheme()
            }
            LingXiTheme(darkTheme = darkTheme, accentId = prefs.accentId) {
                RootScreen(
                    isDark = darkTheme,
                    onToggleTheme = {
                        scope.launch {
                            store.setThemeMode(if (darkTheme) ThemeMode.Light else ThemeMode.Dark)
                        }
                    },
                )
            }
        }
    }
}
