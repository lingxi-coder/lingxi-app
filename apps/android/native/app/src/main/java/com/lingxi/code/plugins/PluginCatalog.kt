package com.lingxi.code.plugins

import org.json.JSONArray
import org.json.JSONObject

/**
 * The engine's plugin catalog (`PluginAdmin get_catalog`), parsed once for the
 * plugins pages and the bundled-plugin installer. Field names follow the
 * engine's rows; nothing here decides anything the engine does not.
 */
data class PluginCatalog(
    val installed: List<InstalledPlugin> = emptyList(),
    val available: List<AvailablePlugin> = emptyList(),
    val marketplaces: List<PluginMarketplace> = emptyList(),
    /** The effective `enabledPlugins` map across every settings layer. */
    val enabled: Map<String, Boolean> = emptyMap(),
    /** The revision a write to each settings layer (`user`, `project`, `local`) must name. */
    val revisions: Map<String, String> = emptyMap(),
) {
    /** The engine's rule: an explicit `enabledPlugins` entry wins, else the manifest's `defaultEnabled`. */
    fun isEnabled(plugin: InstalledPlugin): Boolean = enabled[plugin.id] ?: plugin.defaultEnabled

    fun installed(id: String): InstalledPlugin? = installed.firstOrNull { it.id == id }

    fun marketplace(name: String?): PluginMarketplace? = marketplaces.firstOrNull { it.name == name }

    /** Marketplace plugins that are not installed yet. */
    val installable: List<AvailablePlugin> get() = available.filter { !it.installed }

    companion object {
        fun parse(json: String?): PluginCatalog {
            val root = json?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return PluginCatalog()
            val enabled = runCatching { JSONObject(root.optString("enabled_json", "{}")) }.getOrDefault(JSONObject())
            val revisions = root.optJSONObject("revisions") ?: JSONObject()
            return PluginCatalog(
                installed = root.optJSONArray("installed").objects().map { row ->
                    InstalledPlugin(
                        id = row.optString("id"),
                        name = row.optString("name"),
                        displayName = row.optString("display_name").ifBlank { row.optString("name") },
                        version = row.optString("version"),
                        path = row.optString("path"),
                        defaultEnabled = row.optBoolean("default_enabled", true),
                        description = row.optString("description"),
                        dependencies = row.optJSONArray("dependencies").strings(),
                        configSchemaJson = row.textOrNull("config_schema_json") ?: "null",
                    )
                }.filter { it.id.isNotBlank() },
                available = root.optJSONArray("available").objects().map { row ->
                    AvailablePlugin(
                        id = row.optString("id"),
                        name = row.optString("name"),
                        marketplace = row.optString("marketplace"),
                        version = row.optString("version"),
                        description = row.optString("description"),
                        installed = row.optBoolean("installed"),
                        upgradeAvailable = row.optBoolean("upgrade_available"),
                    )
                }.filter { it.id.isNotBlank() },
                marketplaces = root.optJSONArray("marketplaces").objects().map { row ->
                    PluginMarketplace(
                        name = row.optString("name"),
                        sourceJson = row.textOrNull("source_json") ?: "{}",
                        installLocation = row.textOrNull("install_location"),
                        lastUpdated = row.textOrNull("last_updated"),
                    )
                }.filter { it.name.isNotBlank() },
                enabled = enabled.keys().asSequence()
                    .mapNotNull { key -> (enabled.opt(key) as? Boolean)?.let { key to it } }
                    .toMap(),
                revisions = revisions.keys().asSequence()
                    .mapNotNull { key -> revisions.textOrNull(key)?.let { key to it } }
                    .toMap(),
            )
        }
    }
}

/** One installed plugin, as the engine discovered it on disk. */
data class InstalledPlugin(
    /** `name@marketplace`, or the bare name for a plugin installed without one. */
    val id: String,
    val name: String,
    val displayName: String,
    val version: String,
    /** The install directory in the engine's plugin cache. */
    val path: String,
    val defaultEnabled: Boolean,
    val description: String,
    val dependencies: List<String>,
    val configSchemaJson: String,
) {
    /** The marketplace the plugin was installed from, from its `name@marketplace` id. */
    val marketplace: String? get() = id.substringAfter('@', "").ifEmpty { null }
}

/** One plugin a registered marketplace lists. */
data class AvailablePlugin(
    val id: String,
    val name: String,
    val marketplace: String,
    /** The marketplace entry's own `version`; empty when the entry names none. */
    val version: String,
    val description: String,
    val installed: Boolean,
    val upgradeAvailable: Boolean,
)

/** One registered marketplace. */
data class PluginMarketplace(
    val name: String,
    val sourceJson: String,
    val installLocation: String?,
    val lastUpdated: String?,
) {
    private val source: JSONObject get() = runCatching { JSONObject(sourceJson) }.getOrDefault(JSONObject())

    /** `directory`, `github`, `git` or `url`. */
    val sourceKind: String get() = source.optString("source")

    /** What the source points at: a directory path, an `owner/repo`, or a URL. */
    val sourceLocation: String
        get() = source.let { it.textOrNull("path") ?: it.textOrNull("repo") ?: it.textOrNull("url") }.orEmpty()
}

private fun JSONArray?.objects(): List<JSONObject> =
    if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }

private fun JSONArray?.strings(): List<String> =
    if (this == null) emptyList() else (0 until length()).mapNotNull { opt(it) as? String }

/** A string member, or null when it is absent, JSON null or blank. */
internal fun JSONObject.textOrNull(key: String): String? =
    (opt(key) as? String)?.takeIf(String::isNotBlank)
