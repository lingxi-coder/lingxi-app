package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.bindings.client.ConfigurationOperationStatusDto
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import org.json.JSONObject

/** External data is an editable proposal. Only the final confirmation can write configuration. */
@Composable
internal fun DeepLinkImportPage(link: DeepLink, bridge: SettingsEngineBridge, onClose: () -> Unit) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    val kind = link.route.substringAfter('/')
    var draft by remember(link) { mutableStateOf(JSONObject(link.payload).toString(2)) }
    var scope by remember(link) { mutableStateOf(link.params["scope"] ?: "user") }
    var reviewed by remember(link, state.generation) { mutableStateOf<String?>(null) }
    var baseline by remember(link, state.generation) { mutableStateOf<String?>(null) }
    var revision by remember(link, state.generation) { mutableStateOf<String?>(null) }
    var previewId by remember(link, state.generation) { mutableStateOf<ULong?>(null) }
    var confirm by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var applied by remember { mutableStateOf(false) }
    var conflicts by remember { mutableStateOf<List<String>>(emptyList()) }
    ReportSettingsDraft("deep-link-import", !applied)
    LaunchedEffect(kind, state.connected) {
        if (state.connected) runCatching {
            when (kind) {
                "mcp" -> bridge.admin("mcp", "get_snapshot")
                "plugin" -> bridge.admin("plugins", "get_catalog")
                "skill" -> bridge.admin("skills", "get_catalog")
                else -> bridge.refresh()
            }
        }.onFailure {
            if (it is kotlinx.coroutines.CancellationException) throw it
            error = it.message
        }
    }
    fun currentBaseline(): String? = when (kind) {
        "mcp" -> state.catalogs["mcp"]
        "plugin" -> state.catalogs["plugins"]
        "skill" -> state.catalogs["skills"]
        else -> state.snapshot?.layersJson
    }
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().safeDrawingPadding().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            TextButton(onClick = onClose) { Text(if (applied) "Done" else "Cancel import") }
            Text("Import $kind", style = MaterialTheme.typography.headlineSmall)
            Text("Review the source and destination. Links cannot supply credentials. Imported commands, skills and plugins may access your workspace when enabled.")
            SingleChoiceRow(if (kind == "skill") listOf("user", "project") else listOf("user", "project", "local"), scope, { scope = it; reviewed = null })
            OutlinedTextField(draft, { draft = it; reviewed = null }, label = { Text("Import proposal") }, modifier = Modifier.fillMaxWidth(), minLines = 6, readOnly = applied)
            Button(enabled = !applied && state.connected && state.pending == null && !state.savingSettings, onClick = {
                coroutine.launch {
                    runCatching {
                        val parsed = prepareDeepLinkImportProposal(kind, draft, scope)
                        baseline = currentBaseline() ?: error("Waiting for the engine catalog. Retry preview when connected.")
                        revision = when (kind) {
                            "mcp" -> JSONObject(baseline!!).optJSONArray("scopes")?.records()?.firstOrNull { it.optString("scope") == scope }?.optString("revision_sha256")?.takeIf(String::isNotBlank)
                            "plugin" -> JSONObject(baseline!!).optJSONObject("revisions")?.optString(scope)?.takeIf(String::isNotBlank)
                            "skill" -> "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                            else -> null
                        }
                        if (kind in setOf("mcp", "plugin")) check(revision != null) { "Waiting for this scope configuration. Retry preview when loaded." }
                        conflicts = when (kind) {
                            "provider" -> {
                                val existing = JSONObject(state.snapshot?.layersJson ?: "{}").optJSONObject(scope)?.optJSONObject("providers") ?: JSONObject()
                                parseProviderImport(parsed.toString(), existing).entries.filter { it.conflict }.map { it.id }
                            }
                            "mcp" -> {
                                val raw = JSONObject(baseline!!).optJSONArray("scopes")?.records()?.firstOrNull { it.optString("scope") == scope }?.optString("raw_json")
                                val name = parsed.getString("name")
                                if (JSONObject(raw ?: "{}").optJSONObject("mcpServers")?.has(name) == true) listOf(name) else emptyList()
                            }
                            else -> emptyList()
                        }
                        if (kind == "plugin") {
                            previewId = bridge.admin("plugins", "preview_operation", scope = scope, revision = revision, payload = parsed.put("scope",scope).toString())
                        }
                        reviewed = parsed.toString()
                        error = null
                    }.onFailure { error = it.message ?: "Invalid import proposal."; reviewed = null }
                }
            }) { Text("Preview import") }
            if (reviewed != null && !applied) {
                Text("Destination: $scope · $kind")
                if (conflicts.isNotEmpty()) Text("Existing definitions will be replaced: " + conflicts.joinToString(", "), color = MaterialTheme.colorScheme.error)
                Text(JSONObject(reviewed!!).toString(2))
                val pluginPreview = state.operation?.takeIf { it.operationId == previewId && it.status == ConfigurationOperationStatusDto.SUCCEEDED }
                pluginPreview?.detailsJson?.let { Text(it) }
                Button(enabled = state.pending == null && (kind != "plugin" || pluginPreview != null), onClick = { confirm = true }) { Text("Review and confirm import") }
            }
            if (applied) Text("Import saved. Review its configuration before enabling or running it.")
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            state.notice?.let { Text(it) }
        }
    }
    if (confirm) AlertDialog(onDismissRequest = { confirm = false }, title = { Text("Import into $scope?") }, text = { Text("Save the reviewed $kind configuration? No command or prompt will be submitted by this link.") },
        confirmButton = { TextButton(onClick = {
            confirm = false
            coroutine.launch {
                runCatching {
                    check(baseline == currentBaseline()) { "Configuration changed. Preview the import again." }
                    val parsed = JSONObject(reviewed ?: error("Preview this import first."))
                    when (kind) {
                        "provider" -> {
                            val existing = JSONObject(state.snapshot?.layersJson ?: "{}").optJSONObject(scope)?.optJSONObject("providers") ?: JSONObject()
                            val entries = parseProviderImport(parsed.toString(), existing).entries
                            // Provider definitions are imported without setting or requesting credentials.
                            val providers = mergeProviderImport(existing, entries)
                            val generation = state.generation
                            bridge.update(scope, JSONObject().put("providers", providers))
                            val result = withTimeout(15_000) { bridge.state.first { it.generation != generation || !it.savingSettings } }
                            check(result.generation == generation) { "Workspace changed. Preview the import again." }
                            val stored = JSONObject(result.snapshot?.layersJson ?: "{}").optJSONObject(scope)?.optJSONObject("providers")
                            check(jsonEquivalent(stored, providers)) { "The engine did not confirm this import. Review and retry." }
                        }
                        else -> {
                            val domain = when (kind) { "skill" -> "skills"; "plugin" -> "plugins"; else -> "mcp" }
                            val action = when (kind) { "skill" -> "create_skill"; "plugin" -> "apply_operation"; else -> "save_server" }
                            if (kind == "plugin") parsed.put("confirmed", true)
                            if (kind == "mcp") parsed.put("scope", scope)
                            val operation = bridge.admin(domain, action, scope = scope, revision = revision, payload = parsed.toString()) ?: error("No import operation returned.")
                            val result = bridge.awaitOperation(operation)
                            check(result.status == ConfigurationOperationStatusDto.SUCCEEDED) { result.message ?: "Import failed." }
                        }
                    }
                    applied = true
                    error = null
                }.onFailure { error = it.message ?: "Import failed." }
            }
        }) { Text("Confirm import") } }, dismissButton = { TextButton(onClick = { confirm = false }) { Text("Keep reviewing") } })
}

internal fun prepareDeepLinkImportProposal(kind: String, draft: String, scope: String): JSONObject {
    require(scope in setOf("user", "project", "local")) { "Choose a writable scope." }
    if (kind == "skill") require(scope != "local") { "Skills support user or project scope." }
    val parsed = JSONObject(draft)
    // Reapply the credential and depth checks after edits as well as on incoming URLs.
    DeepLink.build("import/$kind", payload = parsed.keys().asSequence().associateWith { parsed.get(it) })
    when (kind) {
        "mcp" -> {
            require(parsed.keys().asSequence().all { it in setOf("name", "config") }) { "Use name and config for an MCP server." }
            require(parsed.getString("name").isNotBlank())
            val config = parsed.getJSONObject("config")
            require(config.has("command") || config.has("url")) { "Provide command or url." }
        }
        "skill" -> {
            require(parsed.keys().asSequence().all { it in setOf("name", "markdown") }) { "Use name and markdown for a skill." }
            require(parsed.getString("name").isNotBlank() && parsed.getString("markdown").isNotBlank())
        }
        "plugin" -> {
            require(parsed.keys().asSequence().all { it in setOf("plugin", "source") } && parsed.length() == 1) { "Use plugin or source for a marketplace." }
            if (parsed.has("plugin")) { require(parsed.getString("plugin").isNotBlank()); parsed.put("action", "install") }
            else { require(parsed.getString("source").isNotBlank()); parsed.put("action", "marketplace_add") }
        }
        "provider" -> {
            val entries = parseProviderImport(parsed.toString(), JSONObject()).entries
            require(entries.isNotEmpty() && entries.all { it.error == null && it.credential == null }) { "Review valid provider definitions without credentials." }
        }
    }
    return parsed
}
