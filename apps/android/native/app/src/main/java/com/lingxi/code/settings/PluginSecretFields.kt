package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONObject

@Composable
internal fun PluginSecretFields(plugin: String, fields: JSONObject, repository: PluginSecretRepository, onReconnect: (() -> Unit)?) {
    val coroutine = rememberCoroutineScope()
    val registry = LocalSettingsDraftRegistry.current
    fields.keys().asSequence().toList().filter { fields.optJSONObject(it)?.optBoolean("sensitive") == true }.forEach { field ->
        var draft by remember(plugin,field) { mutableStateOf("") }
        var configured by remember(plugin,field) { mutableStateOf<Boolean?>(null) }
        var busy by remember(plugin,field) { mutableStateOf(false) }
        var notice by remember(plugin,field) { mutableStateOf<String?>(null) }
        ReportSettingsDraft("plugin-secret/$plugin/$field",draft.isNotEmpty() || busy)
        LaunchedEffect(plugin,field) { runCatching { repository.configured(plugin,field) }.onSuccess { configured=it } }
        Column(Modifier.padding(12.dp),verticalArrangement=Arrangement.spacedBy(8.dp)) {
            Text(fields.optJSONObject(field)?.optString("title")?.takeIf(String::isNotBlank) ?: field)
            Text(settingsLabel(when(configured) { true -> "Configured"; false -> "Not configured"; null -> "Unknown" }))
            OutlinedTextField(draft,{draft=it},label={Text(settingsLabel("Plugin secret"))},visualTransformation=PasswordVisualTransformation(),singleLine=true,enabled=!busy,modifier=Modifier.fillMaxWidth())
            Button(enabled=!busy && draft.isNotEmpty(),onClick={coroutine.launch {
                busy=true
                runCatching { repository.save(plugin,field,draft) }.onSuccess {
                    draft="";configured=true;registry?.requiresReconnect=true;notice="Saved securely. Reconnect the engine to apply the plugin secret."
                }.onFailure { notice="Secure storage operation failed. Please retry." }
                busy=false
            }}) {Text(settingsLabel("Save"))}
            TextButton(enabled=!busy && configured==true,onClick={coroutine.launch {
                busy=true
                runCatching { repository.delete(plugin,field) }.onSuccess {
                    draft="";configured=false;registry?.requiresReconnect=true;notice="Deleted securely. Reconnect the engine to clear the cached plugin secret."
                }.onFailure { notice="Secure storage operation failed. Please retry." }
                busy=false
            }}) {Text(settingsLabel("Remove"))}
            TextButton(enabled=!busy,onClick={coroutine.launch {runCatching {repository.configured(plugin,field)}.onSuccess {configured=it}}}) {Text(settingsLabel("Refresh status"))}
            notice?.let {Text(settingsLabel(it))}
            if (notice != null && onReconnect != null) TextButton(onClick=onReconnect) {Text(settingsLabel("Reconnect engine"))}
        }
    }
}
