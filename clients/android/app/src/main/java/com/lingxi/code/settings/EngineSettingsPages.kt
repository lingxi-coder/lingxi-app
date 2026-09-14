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

// 一个可切换到的项目。只带 id 与展示名：把「当前是哪个」判在 activeProjectId 上，
// 而不是拿引擎回传的路径去和客户端的 workspace 路径做字符串比较 —— host/guest 两套
// 路径推导在这个仓库里已经分叉过一次，用 id 判等可以完全绕开它。
internal data class SettingsProjectChoice(val id: String, val name: String)

// 设置页内切换项目所需的一切，由宿主注入。switchTo 为 null 表示本宿主提供不了切换
// 能力，此时不画切换入口 —— 不画按钮好过画一个点了没反应的。
internal data class SettingsProjectSwitching(
    val projects: List<SettingsProjectChoice> = emptyList(),
    val activeProjectId: String? = null,
    val switchTo: ((String) -> Unit)? = null,
)

// 走 CompositionLocal 而不是构造参数：层选择器所在的 EngineSettingsPage 有两个调用
// 点，而真正能执行切换的 switchEngineScope 离这里隔着 SettingsHost 和 MainActivity。
internal val LocalSettingsProjectSwitching = staticCompositionLocalOf { SettingsProjectSwitching() }

// 引擎放项目级配置的目录名（Rust 侧的 branding::DOT_DIR）。
private const val SETTINGS_DOT_DIR = ".lingxi"

// 引擎解析 project / local 两层时实际用的那个项目目录，或 null。
//
// 唯一可信的来源是引擎自己在 files_json 里回传的路径：project_dir 由引擎进程启动时
// 的 cwd 定死，客户端这边任何「当前项目」状态都可能和答题的那个引擎不是同一个。
// 反推只做一件事：去掉末尾的 <DOT_DIR>/settings.json；对不上就返回 null —— 宁可不
// 显示项目，也不猜一个可能是错的。
internal fun projectDirectoryFromFiles(filesJson: String?): String? {
    val files = runCatching { JSONArray(filesJson ?: "[]") }.getOrNull() ?: return null
    for (index in 0 until files.length()) {
        val entry = files.optJSONObject(index) ?: continue
        if (entry.optString("layer") != "project") continue
        val path = entry.optString("path").ifBlank { return null }
        val segments = path.split("/")
        if (segments.size < 3) return null
        if (segments[segments.size - 1] != "settings.json") return null
        if (segments[segments.size - 2] != SETTINGS_DOT_DIR) return null
        val directory = segments.subList(0, segments.size - 2).joinToString("/")
        return directory.ifEmpty { "/" }
    }
    return null
}

// 项目目录的末段，用作人读的项目名。完整路径永远跟着一起显示：末段会重名。
internal fun projectDisplayName(directory: String): String =
    directory.split("/").lastOrNull { it.isNotEmpty() } ?: directory

// 落盘位置是否取决于当前项目。user 是本机全局的，给它标一个项目名就是在暗示一个
// 并不存在的作用域；managed 是只读策略，也不归任何项目管。
private fun isProjectScopedLayer(layer: String): Boolean = layer == "project" || layer == "local"

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
        Text(settingsLabel("${layer}_layer_desc"), color = LingXiTheme.palette.text4)
        if (isProjectScopedLayer(layer)) {
            val projectDirectory = projectDirectoryFromFiles(snapshot.filesJson)
            if (projectDirectory == null) {
                Text(settingsLabel("Checking with the engine which project this layer writes to…"), color = LingXiTheme.palette.text4)
            } else {
                Text(settingsLabel("Current project"), color = LingXiTheme.palette.text4)
                Text(projectDisplayName(projectDirectory))
                SelectionContainer { Text(projectDirectory, color = LingXiTheme.palette.text4) }
            }
            val switching = LocalSettingsProjectSwitching.current
            val switchTo = switching.switchTo
            if (switchTo != null) {
                var showProjects by remember(route) { mutableStateOf(false) }
                TextButton(onClick = { showProjects = !showProjects }) { Text(settingsLabel("Switch project")) }
                if (showProjects) {
                    // 这个后果必须先说：引擎是按会话起的，换设置的项目就等于换掉当前对话。
                    Text(settingsLabel("Switching the project also switches your current conversation."), color = LingXiTheme.palette.text4)
                    switching.projects.forEach { choice ->
                        TextButton(onClick = { showProjects = false; switchTo(choice.id) }) {
                            Text(if (choice.id == switching.activeProjectId) "${choice.name} · ${settingsLabel("Current project")}" else choice.name)
                        }
                    }
                }
            }
        }
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
