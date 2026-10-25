package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

@Composable
internal fun EngineCredentialsPage(bridge: SettingsEngineBridge, onMobileProfiles: () -> Unit) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    var provider by remember(state.generation) { mutableStateOf("") }
    var secret by remember(state.generation) { mutableStateOf("") }
    var error by remember(state.generation) { mutableStateOf<String?>(null) }
    var deletionProvider by remember(state.generation) { mutableStateOf<String?>(null) }
    var submitting by remember(state.generation) { mutableStateOf(false) }
    val busy = submitting || state.credentialSaving
    ReportSettingsDraft("provider-credential",secret.isNotEmpty())
    val custom = runCatching { JSONObject(state.snapshot?.effectiveJson ?: "{}").optJSONObject("providers")?.keys()?.asSequence()?.toList() }.getOrNull().orEmpty()
    val ids = (custom + listOf("anthropic","openai","google","deepseek","kimi","qwen") + state.credentialStates.keys + state.credentials?.configuredProviderIds.orEmpty()).distinct()
    LaunchedEffect(state.generation, ids) { if (state.connected) runCatching { bridge.refreshCredentials(ids) } }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        TextButton(enabled = !busy, onClick = onMobileProfiles) { Text(settingsLabel("Manage provider credentials")) }
        if (!state.connected) { Text(settingsLabel("Connect an engine to view and edit its configuration. Device settings remain available.")); return@Column }
        state.credentialStorageEncrypted?.let { encrypted ->
            Text(stringResource(if (encrypted) R.string.settings_credential_storage_encrypted else R.string.settings_credential_storage_unavailable))
        }
        state.credentialStatusError?.let { Text(it,color=MaterialTheme.colorScheme.error) }
        ids.forEach { id ->
            val statusLabel = settingsLabel(when (state.credentialStates[id]) {
                true -> "Configured"
                false -> "Not configured"
                null -> "Unknown"
            })
            TextButton(enabled = !busy, onClick = { provider=id; secret="" }) { Text("$id · $statusLabel") }
        }
        OutlinedTextField(provider,{provider=it},enabled=!busy,label={Text(stringResource(R.string.settings_credential_profile_id))},singleLine=true,modifier=Modifier.fillMaxWidth())
        OutlinedTextField(secret,{secret=it},enabled=!busy,label={Text(stringResource(R.string.settings_provider_api_key))},singleLine=true,visualTransformation=PasswordVisualTransformation(),modifier=Modifier.fillMaxWidth())
        Button(enabled=!busy && provider.isNotBlank() && secret.isNotBlank(),onClick={
            if (!submitting && !bridge.state.value.credentialSaving) {
                val submittedProvider = provider
                val submittedSecret = secret
                val generation = state.generation
                submitting = true
                error = null
                coroutine.launch {
                    try {
                        bridge.setCredential(submittedProvider.trim(), submittedSecret)
                        if (bridge.state.value.generation == generation && provider == submittedProvider && secret == submittedSecret) secret = ""
                    } catch (canceled: CancellationException) {
                        throw canceled
                    } catch (failure: Exception) {
                        if (bridge.state.value.generation == generation) error = failure.message
                    } finally {
                        submitting = false
                    }
                }
            }
        }) { Text(settingsLabel("Save")) }
        TextButton(enabled=!busy && state.credentialStates[provider] == true,onClick={deletionProvider=provider}) { Text(settingsLabel("Remove")) }
        error?.let { Text(it,color=MaterialTheme.colorScheme.error) }
        state.notice?.let { Text(it) }
    }
    deletionProvider?.let { target ->
        AlertDialog(onDismissRequest={deletionProvider=null},title={Text(stringResource(R.string.settings_credential_remove_fmt, target))},
            confirmButton={TextButton(enabled=!busy,onClick={
                if (!submitting && !bridge.state.value.credentialSaving) {
                    val generation = state.generation
                    deletionProvider = null
                    submitting = true
                    error = null
                    coroutine.launch {
                        try {
                            bridge.deleteCredential(target)
                        } catch (canceled: CancellationException) {
                            throw canceled
                        } catch (failure: Exception) {
                            if (bridge.state.value.generation == generation) error = failure.message
                        } finally {
                            submitting = false
                        }
                    }
                }
            }) {Text(settingsLabel("Remove"))}},
            dismissButton={TextButton(onClick={deletionProvider=null}) {Text(settingsLabel("Cancel"))}})
    }
}
