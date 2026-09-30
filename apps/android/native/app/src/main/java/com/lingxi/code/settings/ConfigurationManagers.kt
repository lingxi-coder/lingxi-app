package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

internal fun JSONArray.records(): List<JSONObject> = (0 until length()).mapNotNull { optJSONObject(it) }

@Composable
internal fun McpConfigurationPage(bridge: SettingsEngineBridge) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    var scope by remember(state.generation) { mutableStateOf("user") }
    var name by remember(state.generation, scope) { mutableStateOf("") }
    var config by remember(state.generation, scope) { mutableStateOf("{}") }
    var error by remember { mutableStateOf<String?>(null) }
    var deleteRequested by remember { mutableStateOf(false) }
    val snapshot = runCatching { JSONObject(state.catalogs["mcp"] ?: "{}") }.getOrDefault(JSONObject())
    val scopes = snapshot.optJSONArray("scopes")?.records().orEmpty()
    val current = scopes.firstOrNull { it.optString("scope") == scope }
    val raw = runCatching { JSONObject(current?.optString("raw_json") ?: "{}") }.getOrDefault(JSONObject())
    val servers = raw.optJSONObject("mcpServers") ?: JSONObject()
    val matchesStored = runCatching { jsonEquivalent(JSONObject(config),servers.optJSONObject(name)) }.getOrDefault(false)
    ReportSettingsDraft("mcp/$scope",(name.isNotBlank() || config != "{}") && !matchesStored)
    var revisionAtEdit by remember(state.generation, scope) { mutableStateOf<String?>(null) }
    val currentRevision = current?.optString("revision_sha256")?.takeIf(String::isNotBlank)
    LaunchedEffect(currentRevision) {
        val confirmedDraft = runCatching { jsonEquivalent(JSONObject(config), servers.optJSONObject(name)) }.getOrDefault(false)
        if (revisionAtEdit == null || confirmedDraft) revisionAtEdit = currentRevision
    }
    fun load() { coroutine.launch { runCatching { bridge.admin("mcp", "get_snapshot") }.onFailure { error = it.message } } }
    LaunchedEffect(state.connected) { if (state.connected) runCatching { bridge.admin("mcp", "get_snapshot") } }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(settingsLabel("MCP storage scopes are independent of settings layers. Writes use the loaded revision to prevent overwriting external edits."))
        if (!state.connected) { Text(settingsLabel("Connect an engine to manage MCP servers.")); return@Column }
        TextButton(onClick = ::load) { Text(settingsLabel("Refresh servers")) }
        SingleChoiceRow(listOf("user", "project", "local"), scope, { scope = it })
        // 「与设置层无关」上面那行已经说了，但没说清差在哪 —— 而「本地」两边的含义
        // 正好相反：设置层的 local 在项目内，MCP 的 local 在主目录。真实路径由引擎
        // 在下一行的 path 上回传，所以这两句只讲含义。
        Text(settingsLabel("mcp_scope_${scope}_desc"))
        Text(settingsLabel("mcp_scope_vs_layers_note"))
        current?.optString("path")?.let { Text(it) }
        if (current == null) Text(settingsLabel("Waiting for this scope's configuration…"))
        servers.keys().asSequence().toList().sorted().forEach { server ->
            TextButton(onClick = { name = server; config = prettyJsonValue(servers.opt(server)); revisionAtEdit = currentRevision }) { Text(server) }
        }
        TextButton(onClick = { name = ""; config = "{}"; revisionAtEdit = currentRevision }) { Text(settingsLabel("New server")) }
        if (revisionAtEdit != null && currentRevision != revisionAtEdit) Text(settingsLabel("This layer changed while you were editing. Review your draft before saving."))
        OutlinedTextField(name, { name = it }, label = { Text(settingsLabel("Server name")) }, singleLine = true, modifier = Modifier.fillMaxWidth())
        McpServerFields(config, { config = it }, current == null)
        Text(settingsLabel("Use command/args for stdio or url for a remote server. Runtime support and approval are checked by the engine."))
        Button(enabled = revisionAtEdit != null && name.isNotBlank() && state.pending == null, onClick = {
            coroutine.launch {
                runCatching {
                    val parsed = JSONObject(config)
                    require(parsed.has("command") || parsed.has("url")) { "Provide command or url" }
                    bridge.admin("mcp", "save_server", scope = scope, revision = revisionAtEdit,
                        payload = JSONObject().put("scope",scope).put("name",name.trim()).put("config",parsed).toString())
                }.onFailure { error = it.message }
            }
        }) { Text(settingsLabel("Save server")) }
        TextButton(enabled = servers.has(name) && state.pending == null, onClick = { deleteRequested = true }) { Text(settingsLabel("Remove server")) }
        val approval = snapshot.optJSONObject("approval")
        if (servers.has(name) && approval?.optString("revision_sha256")?.isNotBlank() == true) {
            listOf("approve", "reject", "clear").forEach { decision ->
                TextButton(onClick = { coroutine.launch {
                    runCatching { bridge.admin("mcp", "set_approval", scope = "local", revision = approval.optString("revision_sha256"),
                        payload = JSONObject().put("name",name).put("decision",decision).toString()) }.onFailure { error = it.message }
                } }) { Text(settingsLabel("$decision project approval")) }
            }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        state.notice?.let { Text(it) }
    }
    if (deleteRequested) AlertDialog(onDismissRequest = { deleteRequested = false }, title = { Text(settingsLabel("Remove $name?")) },
        text = { Text(settingsLabel("Remove this definition from $scope. Other scopes remain unchanged.")) },
        confirmButton = { TextButton(onClick = {
            deleteRequested = false
            coroutine.launch { runCatching { bridge.admin("mcp", "remove_server", scope = scope, revision = revisionAtEdit,
                payload = JSONObject().put("scope",scope).put("name",name).toString()) }.onFailure { error = it.message } }
        }) { Text(settingsLabel("Remove")) } }, dismissButton = { TextButton(onClick = { deleteRequested = false }) { Text(settingsLabel("Cancel")) } })
}

@Composable
internal fun SkillManager(bridge: SettingsEngineBridge) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    var selected by remember(state.generation) { mutableStateOf<String?>(null) }
    var name by remember(state.generation) { mutableStateOf("") }
    var markdown by remember(state.generation) { mutableStateOf("") }
    var scope by remember(state.generation) { mutableStateOf("user") }
    var error by remember { mutableStateOf<String?>(null) }
    val catalog = runCatching { JSONObject(state.catalogs["skills"] ?: "{}") }.getOrDefault(JSONObject())
    val document = runCatching { JSONObject(state.catalogs["document"] ?: "{}") }.getOrDefault(JSONObject())
    val loaded = selected != null && document.optString("id") == selected
    LaunchedEffect(state.connected) { if (state.connected) runCatching { bridge.admin("skills", "get_catalog") } }
    var loadedDocumentId by remember(state.generation) { mutableStateOf<String?>(null) }
    var documentRevision by remember(state.generation) { mutableStateOf<String?>(null) }
    var baselineMarkdown by remember(state.generation) { mutableStateOf("") }
    LaunchedEffect(state.catalogs["document"], selected) {
        if (loaded && (loadedDocumentId != selected || markdown == baselineMarkdown || markdown == document.optString("markdown"))) {
            name = document.optString("name"); markdown = document.optString("markdown")
            baselineMarkdown = markdown; loadedDocumentId = selected; documentRevision = document.optString("revision")
        }
    }
    ReportSettingsDraft("skill-document",if (selected == null) name.isNotBlank() || markdown.isNotBlank() else loaded && (markdown != baselineMarkdown || name != document.optString("name")))
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(settingsLabel("Skill documents"))
        TextButton(onClick = { coroutine.launch { runCatching { bridge.admin("skills", "get_catalog") } } }) { Text(settingsLabel("Refresh catalog")) }
        catalog.optJSONArray("entries")?.records().orEmpty().forEach { entry ->
            TextButton(onClick = { selected = entry.optString("id"); coroutine.launch { runCatching { bridge.admin("skills", "get_document", target = selected) }.onFailure { error = it.message } } }) {
                Text("${entry.optString("name")} · ${entry.optString("source")}")
            }
        }
        TextButton(onClick = { selected = null; name = ""; markdown = "" }) { Text(settingsLabel("Create skill")) }
        if (selected == null) SingleChoiceRow(listOf("user", "project"), scope, { scope = it })
        val writable = selected == null || (loaded && document.optBoolean("writable"))
        if (loaded && !writable) Text(document.optString("readonlyReason", "This skill is read-only"))
        OutlinedTextField(name, { name = it }, label = { Text(settingsLabel("Skill name")) }, readOnly = !writable, modifier = Modifier.fillMaxWidth())
        OutlinedTextField(markdown, { markdown = it }, label = { Text(settingsLabel("SKILL.md")) }, readOnly = !writable, modifier = Modifier.fillMaxWidth())
        Button(enabled = writable && name.isNotBlank() && state.pending == null, onClick = {
            coroutine.launch { runCatching {
                bridge.admin("skills", if (selected == null) "create_skill" else "save_document", target = selected,
                    scope = if (selected == null) scope else document.optString("source"),
                    revision = if (selected == null) "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" else documentRevision,
                    payload = JSONObject().put("name",name.trim()).put("markdown",markdown).toString())
            }.onFailure { error = it.message } }
        }) { Text(settingsLabel("Save skill")) }
        if (loaded && writable) {
            TextButton(enabled = state.pending == null, onClick = { coroutine.launch {
                runCatching { bridge.admin("skills", "trash_skill", target = selected, revision = documentRevision) }.onFailure { error = it.message }
            } }) { Text(settingsLabel("Move skill to trash")) }
            TextButton(enabled = state.pending == null, onClick = { coroutine.launch {
                runCatching { bridge.admin("skills", "move_skill", target = selected, scope = if (document.optString("source") == "user") "project" else "user",
                    revision = documentRevision, payload = JSONObject().put("name",name).toString()) }.onFailure { error = it.message }
            } }) { Text("Move to ${if (document.optString("source") == "user") "project" else "user"} scope") }
        }
        catalog.optJSONArray("trash")?.records().orEmpty().forEach { entry ->
            TextButton(enabled = state.pending == null, onClick = { coroutine.launch {
                runCatching { bridge.admin("skills", "restore_skill", target = entry.optString("trashId"), scope = scope,
                    revision = entry.optString("revision"), payload = JSONObject().put("name",entry.optString("name")).toString()) }.onFailure { error = it.message }
            } }) { Text("Restore ${entry.optString("name")} to $scope") }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}

@Composable
internal fun PluginManager(bridge: SettingsEngineBridge, layer: String, onReconnect: (() -> Unit)?) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current
    val secrets = remember(context.applicationContext) { PluginSecretRepository(com.lingxi.code.secure.AndroidSecureStorageAdapter(context.applicationContext)) }
    val catalog = runCatching { JSONObject(state.catalogs["plugins"] ?: "{}") }.getOrDefault(JSONObject())
    var proposal by remember(state.generation, layer) { mutableStateOf<String?>(null) }
    var previewId by remember(state.generation, layer) { mutableStateOf<ULong?>(null) }
    var marketplace by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    val revision = catalog.optJSONObject("revisions")?.optString(layer)?.takeIf(String::isNotBlank)
    val writable = layer != "managed" && state.pending == null && state.snapshot?.locked.orEmpty().none { it in setOf("enabledPlugins", "pluginConfigs", "extraKnownMarketplaces") }
    fun preview(payload: JSONObject) {
        proposal = payload.put("scope",layer).toString()
        coroutine.launch { runCatching {
            bridge.admin("plugins", "preview_operation", scope = layer, revision = revision, payload = proposal)
            previewId = bridge.state.value.pending ?: bridge.state.value.operation?.operationId
        }.onFailure { error = it.message; proposal = null } }
    }
    ReportSettingsDraft("plugin-operation/$layer",proposal != null || marketplace.isNotBlank())
    LaunchedEffect(state.connected) { if (state.connected) runCatching { bridge.admin("plugins", "get_catalog") } }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(settingsLabel("Installed plugins & marketplaces"))
        if (proposal != null || marketplace.isNotBlank()) TextButton(onClick={proposal=null;marketplace=""}) {Text(settingsLabel("Discard draft and reload"))}
        TextButton(onClick = { coroutine.launch { runCatching { bridge.admin("plugins", "get_catalog") } } }) { Text(settingsLabel("Refresh catalog")) }
        catalog.optJSONArray("installed")?.records().orEmpty().forEach { plugin ->
            val id = plugin.optString("id")
            SettingsSection(label = id) {
                Text(plugin.optString("description"))
                val schema = runCatching { JSONObject(plugin.optString("config_schema_json", "{}")) }.getOrDefault(JSONObject())
                val fields = schema.optJSONObject("fields") ?: JSONObject()
                androidx.compose.runtime.key(state.generation, id) { PluginSecretFields(id,fields,secrets,onReconnect) }
                (if (plugin.optString("source") == "builtin") listOf("enable","disable") else listOf("enable", "disable", "update", "uninstall")).forEach { action ->
                    TextButton(enabled = writable, onClick = { preview(JSONObject().put("action",action).put("plugin",id)) }) { Text(settingsLabel("Preview $action")) }
                }
            }
        }
        catalog.optJSONArray("available")?.records().orEmpty().forEach { plugin ->
            TextButton(enabled = writable, onClick = { preview(JSONObject().put("action",if (plugin.optBoolean("installed")) "update" else "install").put("plugin",plugin.optString("id"))) }) {
                Text("${plugin.optString("name")} · ${plugin.optString("marketplace")} · Preview install/update")
            }
        }
        catalog.optJSONArray("marketplaces")?.records().orEmpty().forEach { market ->
            Text(market.optString("name"))
            listOf("marketplace_update", "marketplace_remove").forEach { action ->
                TextButton(enabled = writable, onClick = { preview(JSONObject().put("action",action).put("name",market.optString("name"))) }) { Text(settingsLabel("Preview $action")) }
            }
        }
        OutlinedTextField(marketplace, { marketplace = it }, label = { Text(settingsLabel("Marketplace source")) }, modifier = Modifier.fillMaxWidth())
        TextButton(enabled = writable && marketplace.isNotBlank(), onClick = { preview(JSONObject().put("action","marketplace_add").put("source",marketplace.trim())) }) { Text(settingsLabel("Preview adding marketplace")) }
        val result = state.operation
        if (proposal != null && result != null && result.operationId == previewId && result.status == com.lingxi.code.bindings.client.ConfigurationOperationStatusDto.SUCCEEDED) {
            Text(result.message ?: "Review operation")
            result.detailsJson?.let { SelectionContainer { Text(prettyJson(it)) } }
            Button(enabled = writable && revision != null, onClick = {
                coroutine.launch { runCatching {
                    bridge.admin("plugins", "apply_operation", scope = layer, revision = revision,
                        payload = JSONObject(proposal!!).put("confirmed",true).toString())
                    proposal = null
                }.onFailure { error = it.message } }
            }) { Text(settingsLabel("Confirm and apply")) }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}

@Composable
internal fun HookManager(bridge: SettingsEngineBridge, layer: String) {
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    val document = runCatching { JSONObject(state.catalogs["hooks"] ?: "{}") }.getOrDefault(JSONObject())
    val loaded = document.optString("scope") == layer
    val raw = if (loaded) document.optString("own_json", "{}") else "{}"
    val draft = remember(state.generation, layer) { SettingsValueDraft(raw) }
    var draftRevision by remember(state.generation, layer) { mutableStateOf<String?>(null) }
    LaunchedEffect(raw, document.optString("revision_sha256")) {
        if (loaded) {
            draft.observe(raw)
            if (!draft.dirty) draftRevision = document.optString("revision_sha256")
        }
    }
    var error by remember(state.generation, layer) { mutableStateOf<String?>(null) }
    LaunchedEffect(state.generation, layer) { if (state.connected) runCatching { bridge.admin("hooks", "get_document", scope = layer) } }
    ReportSettingsDraft("hooks/$layer",draft.dirty)
    val locked = layer == "managed" || "hooks" in state.snapshot?.locked.orEmpty()
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(settingsLabel("Hook document · $layer"))
        if (!loaded) Text(settingsLabel("Waiting for this layer's hook document…"))
        if (locked) Text(settingsLabel("Managed policy: read-only"))
        OutlinedTextField(draft.value, { draft.edit(it) }, label = { Text(settingsLabel("Hook event matchers and handlers JSON")) }, readOnly = locked || !loaded, modifier = Modifier.fillMaxWidth())
        listOf("validate_document" to "Validate hooks", "save_document" to "Save hooks").forEach { (action,label) ->
            Button(enabled = loaded && !locked && state.pending == null, onClick = { coroutine.launch {
                runCatching { bridge.admin("hooks", action, scope = layer, revision = draftRevision,
                    payload = JSONObject().put("scope",layer).put("hooks",JSONObject(draft.value)).toString()) }.onFailure { error = it.message }
            } }) { Text(settingsLabel(label)) }
        }
        if (loaded) { Text(settingsLabel("Effective hooks")); SelectionContainer { Text(prettyJson(document.optString("effective_json","{}"))) } }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}
