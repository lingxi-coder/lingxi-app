package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import androidx.compose.ui.unit.dp
import com.lingxi.code.bindings.client.ModelDetailsDto
import org.json.JSONArray
import org.json.JSONObject

private val booleanSettings = setOf("promptSuggestionEnabled", "disableArtifact", "disableAgentView", "disableAllHooks", "skipWebFetchPreflight", "alwaysThinkingEnabled", "showThinkingSummaries", "visionDelegationEnabled", "syncClaudeAiSkills")

@Composable
internal fun TypedSettingField(key: String, value: String, onChange: (String) -> Unit, readOnly: Boolean, models: List<ModelDetailsDto>) {
    when {
        key in booleanSettings -> Row {
            Switch(checked = value == "true" || (key == "promptSuggestionEnabled" && value == "null"), onCheckedChange = { onChange(it.toString()) }, enabled = !readOnly)
            Text(settingsLabel(if (value == "null") "Inherited" else if (value == "true") "Enabled" else "Disabled"))
            TextButton(enabled = !readOnly, onClick = { onChange("null") }) { Text(settingsLabel("Inherit")) }
        }
        key == "enabledTools" || key == "trustedDirectories" -> StringListField(key, value, onChange, readOnly)
        key == "outputStyle" -> {
            val text = runCatching { JSONObject("{\"v\":$value}").optString("v", "") }.getOrDefault("")
            OutlinedTextField(if (value == "null") "" else text, { onChange(JSONObject.quote(it)) }, label = { Text(settingsLabel("Output style")) }, enabled = !readOnly)
        }
        key == "providerRegion" -> Column {
            Text(settingsLabel("Model region"))
            listOf("international" to "International", "china_mainland" to "China mainland").forEach { (region, label) ->
                Row {
                    RadioButton(selected = value == JSONObject.quote(region), onClick = { onChange(JSONObject.quote(region)) }, enabled = !readOnly)
                    Text(settingsLabel(label))
                }
            }
            TextButton(enabled = !readOnly, onClick = { onChange("null") }) { Text(settingsLabel("Inherit")) }
            Text(settingsLabel("Defaults to international. Apply saved settings when idle to change the running region."))
        }
        key == "providers" -> ProviderFields(value, onChange, readOnly)
        key == "routing" -> RoutingFields(value, onChange, readOnly)
        key == "permissions" -> PermissionFields(value, onChange, readOnly)
        else -> OutlinedTextField(value, onChange, label = { Text(settingsLabel("$key JSON")) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
    }
}

@Composable
private fun StringListField(label: String, value: String, onChange: (String) -> Unit, readOnly: Boolean) {
    val array = runCatching { JSONArray(value) }.getOrDefault(JSONArray())
    val lines = (0 until array.length()).joinToString("\n") { array.optString(it) }
    OutlinedTextField(lines, { text -> onChange(JSONArray(text.lines().map(String::trim).filter(String::isNotBlank).distinct()).toString()) },
        label = { Text(stringResource(R.string.settings_list_one_per_line_fmt, settingsLabel(label))) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
}

@Composable
private fun PermissionFields(value: String, onChange: (String) -> Unit, readOnly: Boolean) {
    val permissions = runCatching { JSONObject(value) }.getOrDefault(JSONObject())
    fun set(key: String, next: Any) { onChange(JSONObject(permissions.toString()).put(key, next).toString()) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf("allow", "deny", "ask", "additionalDirectories").forEach { key ->
            StringListField(key, permissions.optJSONArray(key)?.toString() ?: "[]", { set(key, JSONArray(it)) }, readOnly)
        }
        Text(settingsLabel("Default permission mode"))
        PermissionModeOptions.values.toList().forEach { mode ->
            Row {
                RadioButton(permissions.optString("defaultMode", "default") == mode, onClick = { set("defaultMode",mode) }, enabled = !readOnly)
                Text(com.lingxi.code.conversation.permissionModeLabel(mode) ?: mode)
            }
        }
    }
}
@Composable
private fun ProviderFields(value: String, onChange: (String) -> Unit, readOnly: Boolean) {
    val providers = runCatching { JSONObject(value) }.getOrDefault(JSONObject())
    var selected by remember { mutableStateOf("") }
    var newId by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    ReportSettingsDraft("provider-new-profile",newId.isNotBlank())
    val current = providers.optJSONObject(selected)
    fun set(field: String, next: Any) {
        val provider = JSONObject(current?.toString() ?: "{}")
        provider.put(field,next)
        onChange(JSONObject(providers.toString()).put(selected,provider).toString())
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        providers.keys().asSequence().toList().sorted().forEach { id -> TextButton(onClick = { selected = id }) { Text(id) } }
        OutlinedTextField(newId, { newId = it }, label = { Text(settingsLabel("New profile ID")) }, readOnly = readOnly)
        TextButton(enabled = !readOnly && newId.isNotBlank(), onClick = {
            if (!Regex("[a-z0-9][a-z0-9._-]{0,63}").matches(newId) || newId in setOf("builtin","claude","prototype","constructor") || providers.has(newId)) error = "Use a unique lowercase profile ID (1–64 letters, digits, dots, underscores or hyphens)."
            else { selected = newId; onChange(JSONObject(providers.toString()).put(newId,JSONObject().put("type","openai").put("models",JSONArray())).toString()); newId = "" }
        }) { Text(settingsLabel("Add profile")) }
        error?.let { Text(settingsLabel(it), color = MaterialTheme.colorScheme.error) }
        if (current != null) {
            Text(settingsLabel("Protocol"))
            listOf("openai", "openai-responses", "anthropic", "gemini", "azure-openai", "bedrock-claude", "vertex-claude", "vertex-gemini", "foundry-claude").forEach { type ->
                Row { RadioButton(current.optString("type") == type, { set("type",type) }, enabled = !readOnly); Text(type) }
            }
            listOf("baseUrl" to "Base URL").forEach { (key,label) ->
                OutlinedTextField(current.optString(key), { set(key,it) }, label = { Text(settingsLabel(label)) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
            }
            val models = current.optJSONArray("models") ?: JSONArray()
            Text(settingsLabel("Models"))
            for (index in 0 until models.length()) {
                val model = models.optJSONObject(index) ?: JSONObject().put("id",models.optString(index))
                OutlinedTextField(model.optString("id"), { id ->
                    val next = JSONArray(models.toString()); next.put(index,JSONObject(model.toString()).put("id",id)); set("models",next)
                }, label = { Text(settingsLabel("Model ID")) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
            }
            TextButton(enabled = !readOnly, onClick = { set("models",JSONArray(models.toString()).put(JSONObject().put("id",""))) }) { Text(settingsLabel("Add model")) }
            TextButton(enabled = !readOnly, onClick = { val next = JSONObject(providers.toString()); next.remove(selected); onChange(next.toString()); selected = "" }) { Text(settingsLabel("Remove profile from this layer")) }
            var advanced by remember(selected) { mutableStateOf(false) }
            val advancedDraft = remember(selected) { SettingsValueDraft(current.toString(2)) }
            LaunchedEffect(current.toString()) { advancedDraft.observe(current.toString(2)) }
            ReportSettingsDraft("provider-advanced/$selected",advancedDraft.dirty)
            TextButton(onClick = { advanced = !advanced }) { Text(settingsLabel("Advanced")) }
            if (advanced) {
                OutlinedTextField(advancedDraft.value,{advancedDraft.edit(it)},label={Text(settingsLabel("Provider JSON · aliases, pricing, capabilities"))},readOnly=readOnly,modifier=Modifier.fillMaxWidth())
                TextButton(enabled=!readOnly,onClick={
                    runCatching {
                        val definition=JSONObject(advancedDraft.value)
                        validateProviderDefinitions(JSONObject().put(selected,definition))
                        onChange(JSONObject(providers.toString()).put(selected,definition).toString())
                        advancedDraft.reset(definition.toString(2));error=null
                    }.onFailure { error=it.message }
                }) {Text(settingsLabel("Use advanced configuration"))}
            }
        }
    }
}

@Composable
private fun RoutingFields(value: String, onChange: (String) -> Unit, readOnly: Boolean) {
    val routing = runCatching { JSONObject(value) }.getOrDefault(JSONObject())
    fun set(key: String, next: Any) { onChange(JSONObject(routing.toString()).put(key,next).toString()) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val aliases = routing.optJSONObject("aliases") ?: JSONObject()
        var alias by remember { mutableStateOf("") }
        var target by remember { mutableStateOf("") }
        ReportSettingsDraft("routing-alias",alias.isNotBlank() || target.isNotBlank())
        aliases.keys().asSequence().toList().sorted().forEach { name ->
            Row { Text(settingsLabel("$name → ${aliases.optString(name)}"), modifier = Modifier.weight(1f)); TextButton(enabled = !readOnly, onClick = { val next = JSONObject(aliases.toString()); next.remove(name); set("aliases",next) }) { Text(settingsLabel("Remove")) } }
        }
        OutlinedTextField(alias, { alias = it }, label = { Text(settingsLabel("Alias")) }, enabled = !readOnly)
        OutlinedTextField(target, { target = it }, label = { Text(settingsLabel("Target profile/model")) }, enabled = !readOnly)
        TextButton(enabled = !readOnly && alias.isNotBlank() && target.contains('/'), onClick = { set("aliases",JSONObject(aliases.toString()).put(alias.trim(),target.trim())); alias = ""; target = "" }) { Text(settingsLabel("Add alias")) }
        val retry = routing.optJSONObject("retry") ?: JSONObject()
        listOf("maxAttempts", "backoffMs").forEach { key ->
            OutlinedTextField(if (retry.has(key)) retry.optInt(key).toString() else "", { text ->
                val next = JSONObject(retry.toString())
                if (text.isBlank()) next.remove(key) else text.toIntOrNull()?.takeIf { it >= 0 }?.let { next.put(key,it) }
                set("retry",next)
            }, label = { Text(stringResource(if (key == "maxAttempts") R.string.settings_retry_max_attempts else R.string.settings_retry_backoff_ms)) }, enabled = !readOnly)
        }
        var fallbackDraft by remember(routing.optJSONObject("fallback")?.toString()) { mutableStateOf(routing.optJSONObject("fallback")?.toString(2) ?: "{}") }
        val parsedFallback = runCatching { JSONObject(fallbackDraft) }.getOrNull()
        ReportSettingsDraft("routing-fallback",!jsonEquivalent(parsedFallback,routing.optJSONObject("fallback") ?: JSONObject()))
        OutlinedTextField(fallbackDraft, { fallbackDraft = it }, label = { Text(settingsLabel("Fallback routes JSON")) }, readOnly = readOnly, isError = parsedFallback == null, modifier = Modifier.fillMaxWidth())
        TextButton(enabled = !readOnly && parsedFallback != null, onClick = { set("fallback",parsedFallback!!) }) { Text(settingsLabel("Use fallback routes")) }
    }
}

/**
 * Merge one connection over its provider's defaults.
 *
 * Shallow, matching the engine's desugaring: a connection that restates `models`
 * means "this endpoint serves exactly these", not "add to the provider's list".
 */
@Composable
internal fun McpServerFields(value: String, onChange: (String) -> Unit, readOnly: Boolean) {
    val config = runCatching { JSONObject(value) }.getOrDefault(JSONObject())
    val stdio = config.has("command") || !config.has("url")
    fun update(key: String, next: Any) { onChange(JSONObject(config.toString()).put(key,next).toString()) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row {
            RadioButton(stdio, onClick = { val next=JSONObject(config.toString());next.remove("url");next.put("command","");onChange(next.toString()) }, enabled = !readOnly)
            Text("stdio")
            RadioButton(!stdio, onClick = { val next=JSONObject(config.toString());next.remove("command");next.remove("args");next.put("url","");onChange(next.toString()) }, enabled = !readOnly)
            Text("HTTP / SSE")
        }
        val key = if (stdio) "command" else "url"
        OutlinedTextField(config.optString(key), { update(key,it) }, label = { Text(key) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
        if (stdio) {
            val args = config.optJSONArray("args") ?: JSONArray()
            OutlinedTextField((0 until args.length()).joinToString("\n") { args.optString(it) },
                { text -> update("args",JSONArray(if (text.isEmpty()) emptyList<String>() else text.lines())) },
                label = { Text(settingsLabel("args · one argument per line")) }, readOnly = readOnly, modifier = Modifier.fillMaxWidth())
        }
        var advanced by remember { mutableStateOf(false) }
        TextButton(onClick = { advanced = !advanced }) { Text(settingsLabel("Advanced")) }
        if (advanced) OutlinedTextField(value,onChange,label = { Text(settingsLabel("JSON · env, headers, transport")) },readOnly = readOnly,modifier = Modifier.fillMaxWidth())
    }
}
