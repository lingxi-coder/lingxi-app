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
internal fun EngineCredentialsPage(bridge: SettingsEngineBridge, onMobileProfiles: () -> Unit) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    var provider by remember(state.generation) { mutableStateOf("") }
    var secret by remember(state.generation) { mutableStateOf("") }
    var error by remember(state.generation) { mutableStateOf<String?>(null) }
    var deleting by remember(state.generation) { mutableStateOf(false) }
    ReportSettingsDraft("provider-credential",secret.isNotEmpty())
    val custom = runCatching { JSONObject(state.snapshot?.effectiveJson ?: "{}").optJSONObject("providers")?.keys()?.asSequence()?.toList() }.getOrNull().orEmpty()
    val ids = (custom + listOf("anthropic","openai","google","deepseek","kimi","qwen") + state.credentials?.configuredProviderIds.orEmpty()).distinct()
    LaunchedEffect(state.generation, ids) { if (state.connected) runCatching { bridge.refreshCredentials(ids) } }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        TextButton(onClick = onMobileProfiles) { Text(settingsLabel("Manage provider credentials")) }
        if (!state.connected) { Text(settingsLabel("Connect an engine to view and edit its configuration. Device settings remain available.")); return@Column }
        state.credentials?.let { status ->
            Text(if (status.storageEncrypted) "Encrypted credential storage" else "Secure credential storage unavailable")
            status.error?.let { Text(it,color=MaterialTheme.colorScheme.error) }
        }
        ids.forEach { id ->
            val statusLabel = settingsLabel(if (id in state.credentials?.configuredProviderIds.orEmpty()) "Configured" else "Not configured")
            TextButton(onClick = { provider=id; secret="" }) { Text("$id · $statusLabel") }
        }
        OutlinedTextField(provider,{provider=it},label={Text("Provider / profile ID")},singleLine=true,modifier=Modifier.fillMaxWidth())
        OutlinedTextField(secret,{secret=it},label={Text("API key")},singleLine=true,visualTransformation=PasswordVisualTransformation(),modifier=Modifier.fillMaxWidth())
        Button(enabled=provider.isNotBlank() && secret.isNotBlank(),onClick={ coroutine.launch {
            runCatching { bridge.setCredential(provider.trim(),secret);secret="" }.onFailure { error=it.message }
        } }) { Text(settingsLabel("Save")) }
        TextButton(enabled=provider in state.credentials?.configuredProviderIds.orEmpty(),onClick={deleting=true}) { Text(settingsLabel("Remove")) }
        error?.let { Text(it,color=MaterialTheme.colorScheme.error) }
        state.notice?.let { Text(it) }
    }
    if (deleting) AlertDialog(onDismissRequest={deleting=false},title={Text("Remove credential for $provider?")},
        confirmButton={TextButton(onClick={deleting=false;coroutine.launch {runCatching {bridge.deleteCredential(provider)}.onFailure {error=it.message}}}) {Text(settingsLabel("Remove"))}},
        dismissButton={TextButton(onClick={deleting=false}) {Text(settingsLabel("Cancel"))}})
}
