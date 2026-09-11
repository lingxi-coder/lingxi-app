package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.lingxi.code.BuildConfig
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

internal val layeredPageKeys = mapOf(
    SettingsRoutes.CUSTOM_PROVIDERS to listOf("providers", "routing"),
    SettingsRoutes.ENGINE_PERMISSIONS to listOf("permissions", "trustedDirectories"),
    SettingsRoutes.TOOLS_AGENT to listOf("enabledTools", "disableArtifact", "disableAgentView", "disableAllHooks", "skipWebFetchPreflight", "alwaysThinkingEnabled", "showThinkingSummaries", "visionDelegationEnabled", "outputStyle", "modelOverrides"),
    SettingsRoutes.HOOKS to listOf("hooks"),
    SettingsRoutes.PLUGINS to listOf("enabledPlugins", "pluginConfigs", "extraKnownMarketplaces"),
    SettingsRoutes.ENGINE_SKILLS to listOf("syncClaudeAiSkills"),
    SettingsRoutes.PROJECTS to listOf("trustedDirectories"),
)

@Composable
internal fun EngineSettingsPage(route: String, bridge: SettingsEngineBridge, onReconnect: (() -> Unit)? = null) {
    val state by bridge.state.collectAsState()
    var layer by remember(route) { mutableStateOf("user") }
    val scope = rememberCoroutineScope()
    val clipboard = androidx.compose.ui.platform.LocalClipboardManager.current
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (!state.connected) {
            Text(settingsLabel("Connect an engine to view and edit its configuration. Device settings remain available."), color = LingXiTheme.palette.text4)
            return@Column
        }
        TextButton(onClick = { scope.launch { runCatching { bridge.refresh() } } }) { Text(settingsLabel("Refresh from engine")) }
        state.notice?.let { Text(it, color = LingXiTheme.palette.text4) }
        val snapshot = state.snapshot
        if (snapshot == null) { Text(settingsLabel("Waiting for engine settings snapshot…")); return@Column }
        if (route == SettingsRoutes.DIAGNOSTICS) {
            TextButton(onClick = {
                val report = JSONObject().put("platform", "Android").put("version", BuildConfig.VERSION_NAME)
                    .put("files", JSONArray(snapshot.filesJson ?: "[]")).put("locked", JSONArray(snapshot.locked.orEmpty()))
                    .put("mergedKeys", JSONArray(snapshot.mergedKeys.orEmpty()))
                clipboard.setText(androidx.compose.ui.text.AnnotatedString(report.toString(2)))
            }) { Text("Copy diagnostics") }
            Text(settingsLabel("Configuration files"))
            SelectionContainer { Text(prettyJson(snapshot.filesJson ?: "[]")) }
            Text("Managed keys: ${snapshot.locked.orEmpty().joinToString().ifBlank { "None" }}")
            Text("Merged keys: ${snapshot.mergedKeys.orEmpty().joinToString().ifBlank { "None" }}")
            return@Column
        }
        SingleChoiceRow(listOf("user", "project", "local", "managed"), layer, { layer = it })
        Text(settingsLabel("Edits replace only the chosen key in this layer. Effective values can include other layers. Use null to inherit."), color = LingXiTheme.palette.text4)
        if (route == SettingsRoutes.CUSTOM_PROVIDERS) ProviderBulkImport(bridge,layer)
        val raw = runCatching { JSONObject(snapshot.layersJson ?: "{}").optJSONObject(layer) }.getOrNull()
        val effective = JSONObject(snapshot.effectiveJson)
        val provenance = JSONObject(snapshot.provenanceJson)
        layeredPageKeys[route].orEmpty().filter { route != SettingsRoutes.HOOKS }.forEach { key ->
            val readOnly = layer == "managed" || key in snapshot.locked.orEmpty() || raw == null
            val rawValue = raw?.opt(key)?.let { prettyJsonValue(it) } ?: "null"
            val draft = remember(state.generation, route, layer, key) { SettingsValueDraft(rawValue) }
            LaunchedEffect(rawValue) { draft.observe(rawValue) }
            ReportSettingsDraft("$route/$layer/$key",draft.dirty)
            var error by remember(layer, key) { mutableStateOf<String?>(null) }
            SettingsSection(label = key) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(if (key in snapshot.mergedKeys.orEmpty()) "Effective: merged across layers" else "Source: ${provenance.optString(key, "defaults")}")
                    SelectionContainer { Text(settingsLabel("Effective value: ${prettyJsonValue(effective.opt(key))}"), color = LingXiTheme.palette.text4) }
                    if (readOnly) Text(if (raw == null) "Layer data unavailable" else "Managed policy: read-only")
                    androidx.compose.runtime.key(state.generation, route, layer, key) {
                        TypedSettingField(key, draft.value, { draft.edit(it); error = null }, readOnly, state.models)
                    }
                    if (draft.externalChange) Text(settingsLabel("This layer changed while you were editing. Review your draft before saving."))
                    if (draft.dirty) TextButton(onClick = { draft.reset(rawValue) }) { Text(settingsLabel("Discard draft and reload")) }
                    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                    Button(enabled = !readOnly, onClick = {
                        scope.launch {
                            runCatching {
                                val patch = JSONObject("{\"$key\":${draft.value}}")
                                require(patch.length() == 1) { "Enter one JSON value" }
                                if (key == "providers") validateProviderDefinitions(patch.optJSONObject("providers"))
                                if (route == SettingsRoutes.PLUGINS) {
                                    validateSettingsPatch(snapshot, layer, patch)
                                    val catalog = JSONObject(state.catalogs["plugins"] ?: "{}")
                                    val revision = catalog.optJSONObject("revisions")?.optString(layer)?.takeIf(String::isNotBlank)
                                        ?: error("Refresh the plugin catalog before saving")
                                    val payload = JSONObject().put("scope", layer)
                                    listOf("enabledPlugins", "pluginConfigs", "extraKnownMarketplaces").forEach { pluginKey ->
                                        payload.put(pluginKey, if (patch.has(pluginKey)) patch.opt(pluginKey) else raw?.opt(pluginKey) ?: JSONObject())
                                    }
                                    bridge.admin("plugins", "save_config", scope = layer, revision = revision, payload = payload.toString())
                                } else if (key == "permissions") {
                                    bridge.updatePermissions(layer, patch.getJSONObject("permissions"))
                                } else bridge.update(layer, patch)
                            }.onFailure { error = it.message }
                        }
                    }) { Text(settingsLabel("Save to $layer")) }
                }
            }
        }
        if (route == SettingsRoutes.HOOKS) HookManager(bridge, layer)
        if (route == SettingsRoutes.ENGINE_SKILLS) SkillManager(bridge)
        if (route == SettingsRoutes.PLUGINS) PluginManager(bridge, layer, onReconnect)
    }
}

@Composable
internal fun SingleChoiceRow(options: List<String>, selected: String, onSelect: (String) -> Unit) {
    // Wrap naturally on narrow phones and at larger accessibility font sizes.
    Column {
        options.forEach { option ->
            Row {
                RadioButton(selected == option, onClick = { onSelect(option) })
                TextButton(onClick = { onSelect(option) }) { Text(settingsLabel(option)) }
            }
        }
    }
}
internal fun prettyJson(raw: String): String = runCatching {
    if (raw.trim().startsWith("[")) JSONArray(raw).toString(2) else JSONObject(raw).toString(2)
}.getOrDefault(raw)
internal fun prettyJsonValue(value: Any?): String = when (value) {
    null, JSONObject.NULL -> "null"
    is JSONObject -> value.toString(2)
    is JSONArray -> value.toString(2)
    is String -> JSONObject.quote(value)
    else -> value.toString()
}

@Composable
internal fun NativeAboutPage(onLicenses: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(settingsLabel("LingXi"))
        Text(settingsLabel("Version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})"))
        Text(settingsLabel("Android · ${BuildConfig.DISTRIBUTION_CHANNEL}"))
        TextButton(onClick = onLicenses) { Text(settingsLabel("Open-source licenses")) }
    }
}
