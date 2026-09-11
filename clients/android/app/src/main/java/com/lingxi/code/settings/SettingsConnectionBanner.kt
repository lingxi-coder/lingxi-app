package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import org.json.JSONObject

internal class SettingsDraftRegistry {
    private val drafts = mutableStateMapOf<String,Boolean>()
    val hasUnsavedDrafts: Boolean get() = drafts.values.any { it }
    var reconnectBlocked by mutableStateOf(false)
    var requiresReconnect by mutableStateOf(false)
    fun set(id: String, dirty: Boolean) { drafts[id] = dirty }
    fun remove(id: String) { drafts.remove(id) }
}
internal val LocalSettingsDraftRegistry = staticCompositionLocalOf<SettingsDraftRegistry?> { null }

@Composable
internal fun ReportSettingsDraft(id: String, dirty: Boolean) {
    val registry=LocalSettingsDraftRegistry.current
    SideEffect { registry?.set(id,dirty) }
    DisposableEffect(registry,id) { onDispose { registry?.remove(id) } }
}

internal fun settingsDifferFromActive(activeJson: String?, effectiveJson: String?): Boolean {
    if (activeJson == null || effectiveJson == null) return false
    return runCatching { !jsonEquivalent(JSONObject(activeJson),JSONObject(effectiveJson)) }.getOrDefault(false)
}

@Composable
internal fun SettingsConnectionBanner(state: SettingsEngineBridge.State, onReconnect: () -> Unit) {
    val registry=LocalSettingsDraftRegistry.current
    val dirty=registry?.hasUnsavedDrafts==true
    val pending=state.requiresReconnect || registry?.requiresReconnect==true || settingsDifferFromActive(state.snapshot?.activeJson,state.snapshot?.effectiveJson)
    if (!state.connected || (!pending && registry?.reconnectBlocked!=true)) return
    Surface(color=MaterialTheme.colorScheme.surfaceVariant,modifier=Modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal=16.dp,vertical=10.dp),verticalArrangement=Arrangement.spacedBy(6.dp)) {
            Text(settingsLabel("Saved settings will apply after reconnecting the engine."))
            if (dirty) Text(settingsLabel("Save or discard your drafts before reconnecting."))
            Button(enabled=!dirty && !state.savingSettings && state.pending==null,onClick=onReconnect) {
                Text(settingsLabel("Apply saved settings"))
            }
        }
    }
}
