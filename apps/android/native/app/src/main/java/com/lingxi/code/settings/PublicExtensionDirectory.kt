package com.lingxi.code.settings

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.MCPServer
import com.lingxi.code.plugins.PluginMarketplace
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale
import kotlinx.coroutines.launch

/** One shared public-directory record. Config is retained as JSON until the user opens the form. */
internal data class PublicExtensionEntry(
    val id: String,
    val kind: String,
    val name: String,
    val publisher: String,
    val category: String,
    val platforms: Set<String>,
    val description: Map<String, String>,
    val requirements: Map<String, String>,
    val sourceUrl: String,
    val documentationUrl: String,
    val version: String?,
    val marketplace: PublicExtensionMarketplace?,
    val mcpName: String?,
    val mcpConfigJson: String?,
)

internal data class PublicExtensionMarketplace(
    val source: String,
    val name: String,
    val pluginId: String?,
)

internal data class McpEditorTarget(
    val name: String?,
    val scope: String? = null,
    val initialConfigJson: String? = null,
    val directoryConfigJson: String? = null,
)

internal data class ConfiguredMcpEntry(val name: String, val scope: String, val configJson: String)

/** Read the one repository-owned directory asset; do not maintain an Android copy. */
internal fun readPublicExtensionDirectory(context: Context): List<PublicExtensionEntry> =
    context.assets.open("public-directory.json").bufferedReader().use { input ->
        parsePublicExtensionDirectory(input.readText(), platform = "android")
    }

internal fun parsePublicExtensionDirectory(rawJson: String, platform: String = "android"): List<PublicExtensionEntry> {
    val root = JSONObject(rawJson)
    require(root.optInt("schemaVersion", -1) == 1) { "Unsupported public extension directory version" }
    val items = root.optJSONArray("items") ?: JSONArray()
    return (0 until items.length()).mapNotNull { index ->
        val row = items.optJSONObject(index) ?: return@mapNotNull null
        runCatching {
            val kind = row.optString("kind")
            val category = row.optString("category")
            require(kind in setOf("mcp", "plugin"))
            require(category in setOf("browser", "development", "productivity"))
            val platforms = row.optJSONArray("platforms").stringValues().toSet()
            val mcp = row.optJSONObject("mcp")
            val mcpName = mcp?.optString("name")?.takeIf(String::isNotBlank)
            val mcpConfig = mcp?.optJSONObject("config")
            val marketplace = row.optJSONObject("marketplace")?.let { source ->
                source.optString("source").takeIf(String::isNotBlank)?.let { marketplaceSource ->
                    PublicExtensionMarketplace(
                        source = marketplaceSource,
                        name = source.optString("name").ifBlank { row.optString("name") },
                        pluginId = source.optString("plugin").takeIf(String::isNotBlank),
                    )
                }
            }
            PublicExtensionEntry(
                id = row.optString("id").also { require(it.isNotBlank()) },
                kind = kind,
                name = row.optString("name").also { require(it.isNotBlank()) },
                publisher = row.optString("publisher").also { require(it.isNotBlank()) },
                category = category,
                platforms = platforms,
                description = row.optJSONObject("description").stringMap(),
                requirements = row.optJSONObject("requirements").stringMap(),
                sourceUrl = row.optString("sourceUrl"),
                documentationUrl = row.optString("documentationUrl"),
                version = row.optString("version").takeIf(String::isNotBlank),
                marketplace = marketplace,
                mcpName = mcpName,
                mcpConfigJson = mcpConfig?.toString(),
            )
        }.getOrNull()?.takeIf { platform in it.platforms }
    }
}

internal fun configuredMcpEntries(snapshotJson: String?): List<ConfiguredMcpEntry> {
    val scopes = runCatching { JSONObject(snapshotJson ?: "{}").optJSONArray("scopes") }.getOrNull() ?: return emptyList()
    return (0 until scopes.length()).flatMap { index ->
        val row = scopes.optJSONObject(index) ?: return@flatMap emptyList()
        val scope = row.optString("scope").takeIf { it in setOf("user", "project", "local") } ?: return@flatMap emptyList()
        val servers = runCatching {
            JSONObject(row.optString("raw_json", "{}")).optJSONObject("mcpServers")
        }.getOrNull() ?: return@flatMap emptyList()
        servers.keys().asSequence().mapNotNull { name ->
            servers.optJSONObject(name)?.let { ConfiguredMcpEntry(name, scope, it.toString()) }
        }.toList()
    }.distinctBy { it.name to it.scope }.sortedWith(
        compareBy<ConfiguredMcpEntry>({ it.name }, { when (it.scope) { "user" -> 0; "project" -> 1; else -> 2 } }),
    )
}

/** Same MCP name is not enough: public status also requires the configured endpoint/command to match. */
internal fun isPublicMcpConfigured(entry: PublicExtensionEntry, configured: List<ConfiguredMcpEntry>): Boolean {
    val name = entry.mcpName ?: return false
    if (entry.mcpConfigJson == null) return false
    return configured.any { configuredServer ->
        configuredServer.name == name &&
            mcpConfigurationMatches(entry.mcpConfigJson, configuredServer.configJson)
    }
}

internal fun mcpConfigurationMatches(expectedConfigJson: String?, actualConfigJson: String?): Boolean {
    val expected = expectedConfigJson?.let { runCatching { JSONObject(it).mcpIdentity() }.getOrNull() } ?: return false
    val actual = actualConfigJson?.let { runCatching { JSONObject(it).mcpIdentity() }.getOrNull() } ?: return false
    return expected != null && expected == actual
}

internal fun publicMcpNameConflicts(
    name: String,
    scope: String,
    initialName: String?,
    initialScope: String?,
    initialConfigJson: String?,
    directoryConfigJson: String?,
    existingConfigJson: String?,
): Boolean {
    if (directoryConfigJson == null || existingConfigJson == null) return false
    val editingSameDirectoryServer = initialConfigJson == null && name == initialName && scope == initialScope
    return !editingSameDirectoryServer && !mcpConfigurationMatches(directoryConfigJson, existingConfigJson)
}

private data class McpConfigurationIdentity(
    val transport: String,
    val endpoint: String,
    val args: List<String> = emptyList(),
    val type: String? = null,
)

private fun JSONObject.mcpIdentity(): McpConfigurationIdentity? {
    val url = optString("url").trim()
    if (url.isNotEmpty()) {
        return McpConfigurationIdentity("url", url, type = optString("type").takeIf(String::isNotBlank))
    }
    val command = optString("command").trim()
    if (command.isEmpty()) return null
    val args = optJSONArray("args")?.let { array -> (0 until array.length()).map { index -> array.optString(index) } }.orEmpty()
    return McpConfigurationIdentity("command", command, args, optString("type").takeIf(String::isNotBlank))
}

/** Compare public GitHub shorthand with the actual discriminated marketplace source DTO. */
internal fun marketplaceSourceMatches(publicSource: String, registered: PluginMarketplace): Boolean {
    val expectedRepo = githubRepositoryId(publicSource, allowShorthand = true) ?: return false
    val source = runCatching { JSONObject(registered.sourceJson) }.getOrNull() ?: return false
    if (source.optString("source") != registered.sourceKind) return false
    val location = when (registered.sourceKind) {
        "github" -> {
            if (source.keys().asSequence().toSet() != setOf("source", "repo")) return false
            githubRepositoryId(source.optString("repo"), allowShorthand = true)
        }
        "git" -> {
            if (source.keys().asSequence().toSet() != setOf("source", "url")) return false
            githubRepositoryId(source.optString("url"), allowShorthand = false)
        }
        else -> return false
    }
    return location == expectedRepo
}

/** Accept only a root GitHub repository identity: refs, subpaths, and other source kinds differ. */
private fun githubRepositoryId(raw: String, allowShorthand: Boolean): String? {
    val source = raw.trim()
    if (source.startsWith("git@github.com:", ignoreCase = true)) {
        return source.substringAfter(':').removeSuffix(".git")
            .takeIf { Regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$").matches(it) }
            ?.lowercase(Locale.ROOT)
    }
    if (allowShorthand && Regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\\.git)?$").matches(source)) {
        return source.removeSuffix(".git").lowercase(Locale.ROOT)
    }
    val uri = runCatching { java.net.URI(source) }.getOrNull() ?: return null
    if (uri.scheme !in setOf("https", "ssh") || uri.host?.equals("github.com", ignoreCase = true) != true) return null
    val defaultPort = uri.port == -1 || (uri.scheme == "https" && uri.port == 443) || (uri.scheme == "ssh" && uri.port == 22)
    val standardSshUser = uri.scheme == "ssh" && uri.rawUserInfo == "git"
    if (!defaultPort || (uri.rawUserInfo != null && !standardSshUser) || uri.rawQuery != null || uri.rawFragment != null) return null
    val segments = uri.rawPath.orEmpty().trim('/').split('/')
    if (segments.size != 2) return null
    val repository = segments[1].removeSuffix(".git")
    if (!Regex("^[A-Za-z0-9_.-]+$").matches(segments[0]) || !Regex("^[A-Za-z0-9_.-]+$").matches(repository)) return null
    return "${segments[0]}/$repository".lowercase(Locale.ROOT)
}

internal fun filterPublicExtensions(
    entries: List<PublicExtensionEntry>,
    query: String,
    category: String,
    languageTag: String,
): List<PublicExtensionEntry> {
    val needle = query.trim()
    return entries.filter { entry ->
        (category == "all" || entry.category == category) &&
            (needle.isEmpty() || sequenceOf(
                entry.name,
                entry.publisher,
                entry.category,
                entry.localizedDescription(languageTag),
                entry.localizedRequirements(languageTag),
            ).any { it.contains(needle, ignoreCase = true) })
    }
}

internal fun openPublicExtensionUrl(context: Context, url: String): Boolean {
    val uri = runCatching { Uri.parse(url) }.getOrNull() ?: return false
    if (uri.scheme !in setOf("http", "https") || uri.host.isNullOrBlank()) return false
    return runCatching {
        context.startActivity(Intent(Intent.ACTION_VIEW, uri))
        true
    }.getOrDefault(false)
}

internal fun PublicExtensionEntry.localizedDescription(languageTag: String): String = description.localized(languageTag)
internal fun PublicExtensionEntry.localizedRequirements(languageTag: String): String = requirements.localized(languageTag)

private fun Map<String, String>.localized(languageTag: String): String {
    val locale = Locale.forLanguageTag(languageTag)
    val candidates = when {
        locale.language == "zh" && (locale.script == "Hant" || locale.country == "TW") -> listOf("zh-Hant", "zh-TW", "zh")
        locale.language == "zh" -> listOf("zh-Hans", "zh-CN", "zh")
        else -> listOf(locale.language, languageTag)
    }
    return candidates.firstNotNullOfOrNull { this[it]?.takeIf(String::isNotBlank) }
        ?: this["en"]?.takeIf(String::isNotBlank)
        ?: values.firstOrNull(String::isNotBlank).orEmpty()
}

private fun JSONArray?.stringValues(): List<String> = this?.let { array ->
    (0 until array.length()).mapNotNull { index -> array.optString(index).takeIf(String::isNotBlank) }
}.orEmpty()

private fun JSONObject?.stringMap(): Map<String, String> = this?.let { obj ->
    obj.keys().asSequence().mapNotNull { key -> obj.optString(key).takeIf(String::isNotBlank)?.let { key to it } }.toMap()
}.orEmpty()

@Composable
internal fun PublicExtensionDirectory(
    entries: List<PublicExtensionEntry>,
    configuredMcpServers: List<ConfiguredMcpEntry>,
    registeredMarketplaces: List<PluginMarketplace>,
    canConfigureMcp: Boolean,
    onConfigureMcp: (PublicExtensionEntry) -> Unit,
    canAddMarketplace: Boolean,
    onAddMarketplace: (PublicExtensionEntry) -> Unit,
    onOpenPersonal: (PublicExtensionEntry) -> Unit,
) {
    val context = LocalContext.current
    val languageTag = context.resources.configuration.locales[0].toLanguageTag()
    var query by remember { mutableStateOf("") }
    var category by remember { mutableStateOf("all") }
    var linkError by remember { mutableStateOf(false) }
    val categories = listOf(
        "all" to R.string.extension_directory_category_all,
        "browser" to R.string.extension_directory_category_browser,
        "development" to R.string.extension_directory_category_development,
        "productivity" to R.string.extension_directory_category_productivity,
    )
    val visible = remember(entries, query, category, languageTag) {
        filterPublicExtensions(entries, query, category, languageTag)
    }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text(stringResource(R.string.extension_directory_search)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().testTag("settings.extensions.public.search"),
        )
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            categories.forEach { (key, label) ->
                FilterChip(
                    selected = category == key,
                    onClick = { category = key },
                    label = { Text(stringResource(label)) },
                    modifier = Modifier.testTag("settings.extensions.public.category.$key"),
                )
            }
        }
        if (linkError) Text(stringResource(R.string.extension_directory_link_unavailable), color = MaterialTheme.colorScheme.error)
        if (visible.isEmpty()) {
            Text(stringResource(R.string.extension_directory_empty), color = MaterialTheme.colorScheme.onSurfaceVariant)
        } else {
            visible.forEach { entry ->
                val marketplaceAdded = entry.marketplace?.let { publicMarketplace ->
                    registeredMarketplaces.any { marketplace -> marketplaceSourceMatches(publicMarketplace.source, marketplace) }
                } == true
                val marketplaceNameConflict = !marketplaceAdded && entry.marketplace?.let { publicMarketplace ->
                    registeredMarketplaces.any { marketplace ->
                        marketplace.name == publicMarketplace.name && !marketplaceSourceMatches(publicMarketplace.source, marketplace)
                    }
                } == true
                PublicExtensionCard(
                    entry = entry,
                    languageTag = languageTag,
                    configured = isPublicMcpConfigured(entry, configuredMcpServers),
                    marketplaceAdded = marketplaceAdded,
                    marketplaceNameConflict = marketplaceNameConflict,
                    canConfigureMcp = canConfigureMcp,
                    onConfigureMcp = { onConfigureMcp(entry) },
                    canAddMarketplace = canAddMarketplace,
                    onAddMarketplace = { onAddMarketplace(entry) },
                    onOpenPersonal = { onOpenPersonal(entry) },
                    onOpenUrl = { url -> linkError = !openPublicExtensionUrl(context, url) },
                )
            }
        }
    }
}

@Composable
private fun PublicExtensionCard(
    entry: PublicExtensionEntry,
    languageTag: String,
    configured: Boolean,
    marketplaceAdded: Boolean,
    marketplaceNameConflict: Boolean,
    canConfigureMcp: Boolean,
    onConfigureMcp: () -> Unit,
    canAddMarketplace: Boolean,
    onAddMarketplace: () -> Unit,
    onOpenPersonal: () -> Unit,
    onOpenUrl: (String) -> Unit,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        modifier = Modifier.fillMaxWidth().testTag("settings.extensions.public.item.${entry.id}"),
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(entry.name, style = MaterialTheme.typography.titleMedium)
            Text(
                listOfNotNull(entry.publisher, entry.version).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(entry.localizedDescription(languageTag), style = MaterialTheme.typography.bodyMedium)
            entry.localizedRequirements(languageTag).takeIf(String::isNotBlank)?.let {
                Text(stringResource(R.string.extension_directory_requirements_fmt, it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row {
                Text(
                    text = stringResource(if (entry.kind == "mcp") R.string.extension_directory_kind_mcp else R.string.extension_directory_kind_plugin),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.width(8.dp))
                Text(stringResource(categoryLabel(entry.category)), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (configured) {
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(R.string.extension_directory_configured), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.tertiary)
                }
            }
            if (entry.mcpName != null) {
                Text(stringResource(R.string.extension_directory_configured_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Button(
                    enabled = canConfigureMcp,
                    onClick = onConfigureMcp,
                    modifier = Modifier.fillMaxWidth().testTag("settings.extensions.public.configure.${entry.id}"),
                ) { Text(stringResource(R.string.extension_directory_configure)) }
            }
            if (entry.marketplace != null) {
                if (marketplaceNameConflict) {
                    Text(
                        stringResource(R.string.extension_directory_marketplace_name_conflict, entry.marketplace.name),
                        color = MaterialTheme.colorScheme.error,
                    )
                    OutlinedButton(onClick = onOpenPersonal, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(R.string.extension_directory_manage_personal))
                    }
                } else if (marketplaceAdded) {
                    Text(stringResource(R.string.extension_directory_marketplace_added), color = MaterialTheme.colorScheme.tertiary)
                } else {
                    Button(
                        enabled = canAddMarketplace,
                        onClick = onAddMarketplace,
                        modifier = Modifier.fillMaxWidth().testTag("settings.extensions.public.marketplace.${entry.id}"),
                    ) { Text(stringResource(R.string.extension_directory_add_marketplace)) }
                }
            } else if (entry.mcpName == null) {
                Button(
                    enabled = entry.documentationUrl.isNotBlank(),
                    onClick = { onOpenUrl(entry.documentationUrl) },
                    modifier = Modifier.fillMaxWidth().testTag("settings.extensions.public.instructions.${entry.id}"),
                ) { Text(stringResource(R.string.extension_directory_install_instructions)) }
            }
            if (entry.documentationUrl.isNotBlank() && (entry.mcpName != null || entry.marketplace != null)) {
                OutlinedButton(onClick = { onOpenUrl(entry.documentationUrl) }, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.extension_directory_documentation))
                }
            }
            if (entry.sourceUrl.isNotBlank()) {
                OutlinedButton(onClick = { onOpenUrl(entry.sourceUrl) }, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.extension_directory_source))
                }
            }
        }
    }
}

private fun categoryLabel(category: String): Int = when (category) {
    "browser" -> R.string.extension_directory_category_browser
    "development" -> R.string.extension_directory_category_development
    else -> R.string.extension_directory_category_productivity
}

/** Public/Personal MCP directory reached directly from Settings → MCP servers. */
@Composable
internal fun McpDirectoryPage(bridge: SettingsEngineBridge, runtimeServers: List<MCPServer>) {
    val context = LocalContext.current
    val state by bridge.state.collectAsState()
    val coroutine = rememberCoroutineScope()
    val directory = remember(context) { runCatching { readPublicExtensionDirectory(context) } }
    val mcpCatalogJson = state.catalogs["mcp"]
    val configured = remember(mcpCatalogJson) { configuredMcpEntries(mcpCatalogJson) }
    var selectedTab by remember(state.generation) { mutableStateOf(0) }
    var editor by remember(state.generation) { mutableStateOf<McpEditorTarget?>(null) }

    LaunchedEffect(state.connected) {
        if (state.connected) runCatching { bridge.admin("mcp", "get_snapshot") }
    }

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (editor != null) {
            TextButton(onClick = { editor = null }) { Text(stringResource(R.string.extension_directory_back)) }
            McpConfigurationPage(
                bridge = bridge,
                initialName = editor?.name,
                initialScope = editor?.scope,
                initialConfigJson = editor?.initialConfigJson,
                directoryConfigJson = editor?.directoryConfigJson,
            )
        } else {
            TabRow(selectedTabIndex = selectedTab) {
                Tab(
                    selected = selectedTab == 0,
                    onClick = { selectedTab = 0 },
                    text = { Text(stringResource(R.string.extension_directory_public)) },
                    modifier = Modifier.testTag("settings.mcp.extensions.tab.public"),
                )
                Tab(
                    selected = selectedTab == 1,
                    onClick = { selectedTab = 1 },
                    text = { Text(settingsLabel("Personal")) },
                    modifier = Modifier.testTag("settings.mcp.extensions.tab.personal"),
                )
            }
            if (selectedTab == 0) {
                val entries = directory.getOrNull()
                if (entries == null) {
                    Text(stringResource(R.string.extension_directory_load_failed), color = MaterialTheme.colorScheme.onSurfaceVariant)
                } else {
                    PublicExtensionDirectory(
                        entries = entries.filter { it.kind == "mcp" },
                        configuredMcpServers = configured,
                        registeredMarketplaces = emptyList(),
                        canConfigureMcp = state.connected && mcpCatalogJson != null,
                        onConfigureMcp = { entry ->
                            entry.mcpName?.let { name ->
                                val existing = configured.firstOrNull {
                                    it.name == name && mcpConfigurationMatches(entry.mcpConfigJson, it.configJson)
                                }
                                editor = if (existing == null) {
                                    McpEditorTarget(
                                        name,
                                        initialConfigJson = entry.mcpConfigJson ?: "{}",
                                        directoryConfigJson = entry.mcpConfigJson ?: "{}",
                                    )
                                } else {
                                    McpEditorTarget(name, scope = existing.scope, directoryConfigJson = entry.mcpConfigJson ?: "{}")
                                }
                            }
                        },
                        canAddMarketplace = false,
                        onAddMarketplace = {},
                        onOpenPersonal = { selectedTab = 1 },
                    )
                }
            } else if (!state.connected) {
                Text(stringResource(R.string.settings_mcp_disconnected), color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                TextButton(onClick = { coroutine.launch { runCatching { bridge.admin("mcp", "get_snapshot") } } }) {
                    Text(stringResource(R.string.extension_directory_refresh_mcp))
                }
                SettingsSection(label = stringResource(R.string.settings_mcp_servers)) {
                    if (mcpCatalogJson == null) {
                        Text(stringResource(R.string.settings_mcp_wait_scope), modifier = Modifier.padding(14.dp))
                    } else if (configured.isEmpty()) {
                        Text(stringResource(R.string.extension_directory_no_configured_mcp), modifier = Modifier.padding(14.dp))
                    } else {
                        configured.forEachIndexed { index, server ->
                            val runtimeStatus = runtimeServers.firstOrNull { it.name == server.name }?.status
                            val runtimeLabel = when (runtimeStatus) {
                                null, ConnStatus.Configured -> null
                                else -> mcpRuntimeStatusLabel(runtimeStatus)
                            }
                            SettingsRow(
                                label = server.name,
                                modifier = Modifier.testTag("settings.mcp.personal.${server.scope}.${server.name}"),
                                icon = com.lingxi.code.components.LXIconName.Plug,
                                sub = "${settingsLabel(server.scope)} · ${stringResource(R.string.extension_directory_configured)}",
                                value = runtimeLabel,
                                isLast = index == configured.lastIndex,
                                onTap = { editor = McpEditorTarget(server.name, scope = server.scope) },
                            )
                        }
                    }
                    SettingsRow(
                        label = stringResource(R.string.extension_directory_add_mcp),
                        icon = com.lingxi.code.components.LXIconName.Plus,
                        isLast = true,
                        onTap = if (mcpCatalogJson != null) ({ editor = McpEditorTarget(name = null) }) else null,
                    )
                }
                Text(stringResource(R.string.extension_directory_configured_note), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun mcpRuntimeStatusLabel(status: ConnStatus): String = when (status) {
    ConnStatus.Configured -> stringResource(R.string.settings_provider_status_configured)
    ConnStatus.Connected -> stringResource(R.string.settings_provider_status_connected)
    ConnStatus.Idle -> stringResource(R.string.settings_provider_status_idle)
    ConnStatus.Testing -> stringResource(R.string.settings_provider_status_testing)
    ConnStatus.Error -> stringResource(R.string.settings_provider_status_failed)
}
