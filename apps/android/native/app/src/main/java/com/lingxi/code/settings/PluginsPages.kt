package com.lingxi.code.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ConfigurationOperationStatusDto
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.plugins.InstalledPlugin
import com.lingxi.code.plugins.PluginCatalog
import com.lingxi.code.plugins.PluginComponent
import com.lingxi.code.plugins.PluginDetails
import com.lingxi.code.plugins.PluginDetailsReader
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/** The Local App Builder has its own icon; every other plugin shows the plug. */
internal fun pluginIcon(id: String): LXIconName =
    if (id.substringBefore('@') == "local-app-builder") LXIconName.AppBuilder else LXIconName.Plug

/** "vendor · version", leaving out what is not known. */
internal fun pluginSubtitle(vendor: String?, version: String): String =
    listOfNotNull(vendor?.takeIf(String::isNotBlank), version.takeIf(String::isNotBlank)).joinToString(" · ")

/** The directory a plugin's marketplace is cloned to, for the marketplace's `owner`. */
private fun marketplaceDir(catalog: PluginCatalog, plugin: InstalledPlugin): File? =
    catalog.marketplace(plugin.marketplace)?.installLocation?.let(::File)

/** Asks for the plugin catalog and waits for it to arrive, changed or not. */
internal suspend fun SettingsEngineBridge.reloadPluginCatalog(): PluginCatalog {
    val before = state.value.catalogUpdates["plugins"]
    admin("plugins", "get_catalog")
    val next = withTimeoutOrNull(30_000) { state.first { it.catalogUpdates["plugins"] != before } } ?: state.value
    return PluginCatalog.parse(next.catalogs["plugins"])
}

/**
 * One operation of the plugin system in the user layer, the way the plugin
 * system defines it: applied with `confirmed` once the user agreed (the pages
 * ask first), awaited, then the catalog is read again. Fails with the engine's
 * message.
 */
internal suspend fun SettingsEngineBridge.applyPluginOperation(payload: JSONObject) {
    val revision = PluginCatalog.parse(state.value.catalogs["plugins"]).revisions["user"]
        ?: error("Refresh the plugin catalog before changing it")
    val body = payload.put("scope", "user").put("confirmed", true).toString()
    val id = admin("plugins", "apply_operation", scope = "user", revision = revision, payload = body)
        ?: error("The engine did not accept the operation")
    val result = try {
        awaitOperation(id)
    } finally {
        withContext(kotlinx.coroutines.NonCancellable) { runCatching { reloadPluginCatalog() } }
    }
    check(result.status != ConfigurationOperationStatusDto.FAILED) { result.message ?: "The plugin operation failed" }
}

/** What a page asks before a plugin operation that installs, removes or fetches. */
private data class PluginConfirmation(
    val title: String,
    val body: String,
    val confirm: String,
    val destructive: Boolean,
    val run: () -> Unit,
)

@Composable
private fun PluginConfirmDialog(confirmation: PluginConfirmation, onDismiss: () -> Unit) {
    val t = LingXiTheme.palette
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(confirmation.title) },
        text = { Text(confirmation.body) },
        confirmButton = {
            TextButton(onClick = { onDismiss(); confirmation.run() }) {
                Text(confirmation.confirm, color = if (confirmation.destructive) t.danger else t.accent)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}

/** A plugin's icon tile, as large as [size]. */
@Composable
private fun PluginIconTile(id: String, size: Int) {
    val t = LingXiTheme.palette
    val color = t.accent
    val shape = RoundedCornerShape((size * 0.26f).dp)
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(size.dp)
            .clip(shape)
            .background(color.mix(t.surface, 0.16f))
            .border(0.5.dp, color.tint(0.28f), shape),
    ) {
        LXIcon(name = pluginIcon(id), size = (size * 0.5f).dp, color = color, stroke = 1.8f)
    }
}

@Composable
private fun Busy() {
    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = LingXiTheme.palette.accent)
}

@Composable
private fun Pill(text: String, enabled: Boolean, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    Text(
        text = text,
        color = if (enabled) t.accent else t.text4,
        fontSize = 13.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(t.accent.copy(alpha = if (enabled) 0.12f else 0.05f))
            .then(if (enabled) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 14.dp, vertical = 6.dp),
    )
}

@Composable
private fun NoteRow(text: String) {
    Text(text, color = LingXiTheme.palette.text3, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp))
}

/**
 * Settings → Plugins: the installed plugins with their switch, the plugins the
 * added marketplaces offer, and the marketplaces themselves. Every change is an
 * operation of the plugin system in the user layer; the raw per-layer
 * configuration stays one tap away under Advanced.
 */
@Composable
internal fun PluginsPage(bridge: SettingsEngineBridge, onDetail: (String) -> Unit, onAdvanced: () -> Unit) {
    val t = LingXiTheme.palette
    val state by bridge.state.collectAsState()
    val catalogJson = state.catalogs["plugins"]
    val catalog = remember(catalogJson) { PluginCatalog.parse(catalogJson) }
    val coroutine = rememberCoroutineScope()
    var busy by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var confirmation by remember { mutableStateOf<PluginConfirmation?>(null) }
    var menuFor by remember { mutableStateOf<String?>(null) }
    var addingMarketplace by remember { mutableStateOf(false) }
    val vendors by produceState(emptyMap<String, String>(), catalog) {
        value = withContext(Dispatchers.IO) {
            catalog.installed.mapNotNull { plugin ->
                PluginDetailsReader.vendor(File(plugin.path), marketplaceDir(catalog, plugin))?.let { plugin.id to it }
            }.toMap()
        }
    }
    LaunchedEffect(state.connected) { if (state.connected) runCatching { bridge.admin("plugins", "get_catalog") } }

    fun run(key: String, payload: JSONObject) {
        coroutine.launch {
            busy = key
            error = null
            try {
                bridge.applyPluginOperation(payload)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                error = failure.message
            } finally {
                busy = null
            }
        }
    }

    val failedFmt = stringResource(R.string.plugins_operation_failed_fmt)
    val installTitle = stringResource(R.string.plugins_install_title_fmt)
    val installBody = stringResource(R.string.plugins_install_body)
    val installLabel = stringResource(R.string.plugins_action_install)
    val removeTitle = stringResource(R.string.plugins_marketplace_remove_title_fmt)
    val removeBody = stringResource(R.string.plugins_marketplace_remove_body)
    val removeLabel = stringResource(R.string.plugins_marketplace_remove)
    Column(Modifier.fillMaxWidth()) {
        Blurb(stringResource(R.string.plugins_blurb))
        if (!state.connected) {
            Text(settingsLabel("Connect an engine to view and edit its configuration. Device settings remain available."), color = t.text3)
            return@Column
        }
        if (catalogJson == null) {
            Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { Busy() }
            return@Column
        }
        error?.let { Text(String.format(failedFmt, it), color = t.danger, fontSize = 13.sp, modifier = Modifier.padding(bottom = 12.dp)) }

        SettingsSection(label = stringResource(R.string.plugins_section_installed)) {
            if (catalog.installed.isEmpty()) NoteRow(stringResource(R.string.plugins_installed_empty))
            catalog.installed.forEachIndexed { index, plugin ->
                SettingsRow(
                    label = plugin.displayName,
                    modifier = Modifier.testTag("settings.plugins.installed.${plugin.id}"),
                    icon = pluginIcon(plugin.id),
                    sub = pluginSubtitle(vendors[plugin.id], plugin.version),
                    chevron = false,
                    isLast = index == catalog.installed.lastIndex,
                    onTap = { onDetail(plugin.id) },
                    trailing = {
                        if (busy == plugin.id) Busy()
                        else LXToggle(
                            checked = catalog.isEnabled(plugin),
                            enabled = busy == null,
                            onCheckedChange = { on ->
                                run(plugin.id, JSONObject().put("action", if (on) "enable" else "disable").put("plugin", plugin.id))
                            },
                        )
                    },
                )
            }
        }

        val installable = catalog.installable
        SettingsSection(label = stringResource(R.string.plugins_section_available)) {
            if (installable.isEmpty()) {
                NoteRow(stringResource(if (catalog.marketplaces.isEmpty()) R.string.plugins_marketplaces_empty else R.string.plugins_available_empty))
            }
            installable.forEachIndexed { index, plugin ->
                SettingsRow(
                    label = plugin.name,
                    modifier = Modifier.testTag("settings.plugins.available.${plugin.id}"),
                    icon = pluginIcon(plugin.id),
                    sub = listOf(plugin.marketplace, plugin.description).filter(String::isNotBlank).joinToString(" · "),
                    chevron = false,
                    isLast = index == installable.lastIndex,
                    trailing = {
                        if (busy == plugin.id) Busy()
                        else Pill(installLabel, enabled = busy == null) {
                            confirmation = PluginConfirmation(
                                title = String.format(installTitle, plugin.name),
                                body = installBody,
                                confirm = installLabel,
                                destructive = false,
                            ) { run(plugin.id, JSONObject().put("action", "install").put("plugin", plugin.id)) }
                        }
                    },
                )
            }
        }

        SettingsSection(label = stringResource(R.string.plugins_section_marketplaces)) {
            catalog.marketplaces.forEach { marketplace ->
                val key = "marketplace:${marketplace.name}"
                SettingsRow(
                    label = marketplace.name,
                    modifier = Modifier.testTag("settings.plugins.marketplace.${marketplace.name}"),
                    icon = LXIconName.Apps,
                    sub = marketplace.sourceLocation.ifBlank { null },
                    chevron = false,
                    trailing = {
                        if (busy == key) Busy()
                        else Box {
                            Text(
                                "⋯",
                                color = if (busy == null) t.text2 else t.text4,
                                fontSize = 18.sp,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(8.dp))
                                    .then(if (busy == null) Modifier.clickable { menuFor = marketplace.name } else Modifier)
                                    .padding(horizontal = 8.dp, vertical = 2.dp),
                            )
                            DropdownMenu(expanded = menuFor == marketplace.name, onDismissRequest = { menuFor = null }) {
                                DropdownMenuItem(
                                    text = { Text(stringResource(R.string.plugins_marketplace_update)) },
                                    onClick = {
                                        menuFor = null
                                        run(key, JSONObject().put("action", "marketplace_update").put("name", marketplace.name))
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(removeLabel, color = t.danger) },
                                    onClick = {
                                        menuFor = null
                                        confirmation = PluginConfirmation(
                                            title = String.format(removeTitle, marketplace.name),
                                            body = removeBody,
                                            confirm = removeLabel,
                                            destructive = true,
                                        ) { run(key, JSONObject().put("action", "marketplace_remove").put("name", marketplace.name)) }
                                    },
                                )
                            }
                        }
                    },
                )
            }
            SettingsRow(
                label = stringResource(R.string.plugins_marketplace_add),
                modifier = Modifier.testTag("settings.plugins.marketplace.add"),
                icon = LXIconName.Plus,
                isLast = true,
                onTap = if (busy == null) ({ addingMarketplace = true }) else null,
                trailing = { if (busy == "marketplace:add") Busy() },
            )
        }

        SettingsSection {
            SettingsRow(
                label = stringResource(R.string.plugins_advanced),
                modifier = Modifier.testTag("settings.plugins.advanced"),
                icon = LXIconName.Cog,
                sub = stringResource(R.string.plugins_advanced_sub),
                isLast = true,
                onTap = onAdvanced,
            )
        }
    }

    confirmation?.let { PluginConfirmDialog(it) { confirmation = null } }
    if (addingMarketplace) {
        AddMarketplaceDialog(
            onAdd = { source -> run("marketplace:add", JSONObject().put("action", "marketplace_add").put("source", source)) },
            onDismiss = { addingMarketplace = false },
        )
    }
}

@Composable
private fun AddMarketplaceDialog(onAdd: (String) -> Unit, onDismiss: () -> Unit) {
    val t = LingXiTheme.palette
    var source by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.plugins_marketplace_add)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(stringResource(R.string.plugins_marketplace_add_body), color = t.text3, fontSize = 13.sp)
                OutlinedTextField(
                    value = source,
                    onValueChange = { source = it },
                    singleLine = true,
                    placeholder = { Text(stringResource(R.string.plugins_marketplace_add_hint)) },
                    modifier = Modifier.fillMaxWidth().testTag("settings.plugins.marketplace.source"),
                )
            }
        },
        confirmButton = {
            TextButton(enabled = source.isNotBlank(), onClick = { onDismiss(); onAdd(source.trim()) }) {
                Text(stringResource(R.string.plugins_marketplace_add))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}

/**
 * One installed plugin: who made it, what it contains (read from its own
 * files), its switch and secrets, and update and uninstall.
 */
@Composable
internal fun PluginDetailPage(
    bridge: SettingsEngineBridge,
    pluginId: String,
    onReconnect: (() -> Unit)?,
    onPop: () -> Unit,
) {
    val t = LingXiTheme.palette
    val state by bridge.state.collectAsState()
    val catalogJson = state.catalogs["plugins"]
    val catalog = remember(catalogJson) { PluginCatalog.parse(catalogJson) }
    val plugin = catalog.installed(pluginId)
    val coroutine = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var confirmation by remember { mutableStateOf<PluginConfirmation?>(null) }
    val details by produceState(PluginDetails(), plugin?.path, catalogJson) {
        value = withContext(Dispatchers.IO) {
            plugin?.let { PluginDetailsReader.read(File(it.path), marketplaceDir(catalog, it)) } ?: PluginDetails()
        }
    }
    LaunchedEffect(state.connected) {
        if (state.connected && catalogJson == null) runCatching { bridge.admin("plugins", "get_catalog") }
    }
    val context = LocalContext.current
    val secrets = remember(context.applicationContext) {
        PluginSecretRepository(com.lingxi.code.secure.AndroidSecureStorageAdapter(context.applicationContext))
    }

    fun perform(block: suspend () -> Unit) {
        coroutine.launch {
            busy = true
            error = null
            notice = null
            try {
                block()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                error = failure.message
            } finally {
                busy = false
            }
        }
    }

    if (plugin == null) {
        Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
            if (catalogJson == null && state.connected) Busy()
            else Text(stringResource(R.string.plugins_installed_empty), color = t.text3)
        }
        return
    }
    val upToDate = stringResource(R.string.plugins_up_to_date)
    val failedFmt = stringResource(R.string.plugins_operation_failed_fmt)
    Column(Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.padding(bottom = 14.dp),
        ) {
            PluginIconTile(plugin.id, 56)
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(plugin.displayName, color = t.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                Text(pluginSubtitle(details.vendor, plugin.version), color = t.text3, fontSize = 13.sp)
            }
        }
        if (plugin.description.isNotBlank()) {
            Text(plugin.description, color = t.text2, fontSize = 14.sp, lineHeight = 20.sp, modifier = Modifier.padding(bottom = 16.dp))
        }
        error?.let { Text(String.format(failedFmt, it), color = t.danger, fontSize = 13.sp, modifier = Modifier.padding(bottom = 12.dp)) }
        notice?.let { Text(it, color = t.text3, fontSize = 13.sp, modifier = Modifier.padding(bottom = 12.dp)) }

        SettingsSection {
            SettingsRow(
                label = stringResource(R.string.plugins_enabled),
                chevron = false,
                isLast = true,
                trailing = {
                    LXToggle(
                        checked = catalog.isEnabled(plugin),
                        enabled = !busy,
                        onCheckedChange = { on ->
                            perform { bridge.applyPluginOperation(JSONObject().put("action", if (on) "enable" else "disable").put("plugin", plugin.id)) }
                        },
                    )
                },
            )
        }

        val info = listOfNotNull(
            details.vendor?.let { stringResource(R.string.plugins_detail_vendor) to it },
            plugin.version.takeIf(String::isNotBlank)?.let { stringResource(R.string.plugins_detail_version) to it },
            plugin.marketplace?.let { stringResource(R.string.plugins_detail_marketplace) to it },
            details.license?.let { stringResource(R.string.plugins_detail_license) to it },
            details.homepage?.let { stringResource(R.string.plugins_detail_homepage) to it },
            details.repository?.let { stringResource(R.string.plugins_detail_repository) to it },
            plugin.dependencies.takeIf(List<String>::isNotEmpty)?.let { stringResource(R.string.plugins_detail_dependencies) to it.joinToString(", ") },
        )
        if (info.isNotEmpty()) {
            SettingsSection(label = stringResource(R.string.plugins_detail_about)) {
                info.forEachIndexed { index, (label, value) ->
                    SettingsRow(label = label, value = value, chevron = false, isLast = index == info.lastIndex)
                }
            }
        }
        ComponentSection(stringResource(R.string.plugins_detail_skills), details.skills)
        ComponentSection(stringResource(R.string.plugins_detail_commands), details.commands)
        ComponentSection(stringResource(R.string.plugins_detail_agents), details.agents)
        ComponentSection(stringResource(R.string.plugins_detail_hooks), details.hooks.map { PluginComponent(it, "") })
        ComponentSection(stringResource(R.string.plugins_detail_mcp), details.mcpServers.map { PluginComponent(it, "") })

        val fields = runCatching { JSONObject(plugin.configSchemaJson).optJSONObject("fields") }.getOrNull()
        if (fields != null && fields.keys().asSequence().any { fields.optJSONObject(it)?.optBoolean("sensitive") == true }) {
            SettingsSection(label = stringResource(R.string.plugins_detail_settings)) {
                androidx.compose.runtime.key(state.generation, plugin.id) { PluginSecretFields(plugin.id, fields, secrets, onReconnect) }
            }
        }

        val uninstallTitle = stringResource(R.string.plugins_uninstall_title_fmt, plugin.displayName)
        val uninstallBody = stringResource(R.string.plugins_uninstall_body)
        val uninstallLabel = stringResource(R.string.plugins_action_uninstall)
        SettingsSection {
            plugin.marketplace?.let { marketplace ->
                SettingsRow(
                    label = stringResource(R.string.plugins_action_update),
                    modifier = Modifier.testTag("settings.plugin.update"),
                    chevron = false,
                    onTap = if (busy) null else ({
                        perform {
                            // A marketplace only learns of new versions when it is fetched again.
                            bridge.applyPluginOperation(JSONObject().put("action", "marketplace_update").put("name", marketplace))
                            bridge.applyPluginOperation(JSONObject().put("action", "update").put("plugin", plugin.id))
                            val after = PluginCatalog.parse(bridge.state.value.catalogs["plugins"]).installed(plugin.id)
                            if (after?.version == plugin.version) notice = upToDate
                        }
                    }),
                    trailing = { if (busy) Busy() },
                )
            }
            SettingsRow(
                label = uninstallLabel,
                modifier = Modifier.testTag("settings.plugin.uninstall"),
                danger = true,
                chevron = false,
                isLast = true,
                onTap = if (busy) null else ({
                    confirmation = PluginConfirmation(uninstallTitle, uninstallBody, uninstallLabel, destructive = true) {
                        perform {
                            bridge.applyPluginOperation(JSONObject().put("action", "uninstall").put("plugin", plugin.id))
                            onPop()
                        }
                    }
                }),
            )
        }
    }
    confirmation?.let { PluginConfirmDialog(it) { confirmation = null } }
}

@Composable
private fun ComponentSection(label: String, components: List<PluginComponent>) {
    if (components.isEmpty()) return
    SettingsSection(label = label) {
        components.forEachIndexed { index, component ->
            SettingsRow(
                label = component.name,
                sub = component.description.ifBlank { null },
                chevron = false,
                isLast = index == components.lastIndex,
            )
        }
    }
}
