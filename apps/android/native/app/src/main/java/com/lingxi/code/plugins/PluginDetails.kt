package com.lingxi.code.plugins

import java.io.File
import org.json.JSONArray
import org.json.JSONObject

/**
 * What an installed plugin's own files say about it, for the plugins pages.
 * The engine's catalog carries ids, versions and paths only, so the vendor and
 * the component lists come from the plugin directory itself, read-only.
 */
data class PluginDetails(
    /** `author.name` (or a string `author`) from plugin.json, else the marketplace's `owner.name`. */
    val vendor: String? = null,
    val license: String? = null,
    val homepage: String? = null,
    val repository: String? = null,
    val skills: List<PluginComponent> = emptyList(),
    val commands: List<PluginComponent> = emptyList(),
    val agents: List<PluginComponent> = emptyList(),
    /** Hook event names, such as `PreToolUse`. */
    val hooks: List<String> = emptyList(),
    val mcpServers: List<String> = emptyList(),
)

data class PluginComponent(val name: String, val description: String)

/**
 * Reads the standard plugin layout: `.lingxi-plugin/plugin.json`, then the
 * default component locations (`skills/`, `commands/`, `agents/`,
 * `hooks/hooks.json`, `.mcp.json`) plus any extra paths the manifest declares.
 * Paths that leave the plugin directory are ignored.
 */
object PluginDetailsReader {
    fun read(pluginDir: File, marketplaceDir: File? = null): PluginDetails {
        val manifest = readJson(File(pluginDir, "$MANIFEST_DIR/plugin.json")) ?: JSONObject()
        return PluginDetails(
            vendor = author(manifest) ?: marketplaceDir?.let(::marketplaceOwner),
            license = manifest.textOrNull("license"),
            homepage = manifest.textOrNull("homepage"),
            repository = manifest.textOrNull("repository") ?: manifest.optJSONObject("repository")?.textOrNull("url"),
            skills = componentPaths(pluginDir, manifest, "skills").flatMap(::skillsIn).distinctBy { it.name },
            commands = componentPaths(pluginDir, manifest, "commands").flatMap(::markdownIn).distinctBy { it.name },
            agents = componentPaths(pluginDir, manifest, "agents").flatMap(::markdownIn).distinctBy { it.name },
            hooks = configNames(pluginDir, manifest, "hooks", "hooks/hooks.json", "hooks") { it is JSONArray },
            mcpServers = configNames(pluginDir, manifest, "mcpServers", ".mcp.json", "mcpServers") { it is JSONObject },
        )
    }

    /** Only the vendor, for list rows. */
    fun vendor(pluginDir: File, marketplaceDir: File? = null): String? =
        author(readJson(File(pluginDir, "$MANIFEST_DIR/plugin.json")) ?: JSONObject())
            ?: marketplaceDir?.let(::marketplaceOwner)

    private const val MANIFEST_DIR = ".lingxi-plugin"

    /** Frontmatter only ever sits at the top; a long body is never read. */
    private const val FRONTMATTER_LIMIT = 16 * 1024

    private fun author(manifest: JSONObject): String? =
        manifest.textOrNull("author") ?: manifest.optJSONObject("author")?.textOrNull("name")

    private fun marketplaceOwner(marketplaceDir: File): String? =
        readJson(File(marketplaceDir, "$MANIFEST_DIR/marketplace.json"))?.optJSONObject("owner")?.textOrNull("name")

    /** The default location named [key] plus the manifest's extra paths for it, existing and inside [root]. */
    private fun componentPaths(root: File, manifest: JSONObject, key: String): List<File> {
        val declared = when (val value = manifest.opt(key)) {
            is String -> listOf(value)
            is JSONArray -> (0 until value.length()).mapNotNull { value.opt(it) as? String }
            else -> emptyList()
        }
        return (listOf(key) + declared).mapNotNull { inside(root, it) }.filter(File::exists).distinctBy(File::getPath)
    }

    /** A skills location is either one skill (it holds SKILL.md) or a directory of skills. */
    private fun skillsIn(location: File): List<PluginComponent> {
        val skillDirs = if (File(location, "SKILL.md").isFile) listOf(location)
        else location.listFiles { file -> File(file, "SKILL.md").isFile }.orEmpty().sortedBy(File::getName)
        return skillDirs.map { dir ->
            val fields = frontmatter(head(File(dir, "SKILL.md")))
            PluginComponent(fields["name"]?.ifBlank { null } ?: dir.name, fields["description"].orEmpty())
        }
    }

    /** Commands and agents are Markdown files, one per entry, in subdirectories too. */
    private fun markdownIn(location: File): List<PluginComponent> =
        location.walkTopDown().maxDepth(4).filter { it.isFile && it.extension == "md" }.sortedBy(File::getPath).map { file ->
            val fields = frontmatter(head(file))
            PluginComponent(fields["name"]?.ifBlank { null } ?: file.nameWithoutExtension, fields["description"].orEmpty())
        }.toList()

    /**
     * Names under a hooks or MCP configuration: the default file, a manifest
     * path, or an object inlined in the manifest. Either shape is accepted,
     * with or without the wrapping [wrapper] key.
     */
    private fun configNames(
        root: File,
        manifest: JSONObject,
        key: String,
        defaultPath: String,
        wrapper: String,
        isEntry: (Any?) -> Boolean,
    ): List<String> {
        val declared = when (val value = manifest.opt(key)) {
            is JSONObject -> listOf(value)
            is String -> listOfNotNull(inside(root, value)?.let(::readJson))
            is JSONArray -> (0 until value.length()).mapNotNull { index ->
                when (val item = value.opt(index)) {
                    is JSONObject -> item
                    is String -> inside(root, item)?.let(::readJson)
                    else -> null
                }
            }
            else -> emptyList()
        }
        val documents = listOfNotNull(inside(root, defaultPath)?.let(::readJson)) + declared
        return documents.flatMap { document ->
            val entries = document.optJSONObject(wrapper) ?: document
            entries.keys().asSequence().filter { isEntry(entries.opt(it)) }.toList()
        }.distinct().sorted()
    }

    private fun inside(root: File, relative: String): File? {
        val base = runCatching { root.canonicalFile }.getOrNull() ?: return null
        val file = runCatching { File(base, relative.removePrefix("./")).canonicalFile }.getOrNull() ?: return null
        return file.takeIf { it == base || it.path.startsWith(base.path + File.separator) }
    }

    private fun readJson(file: File): JSONObject? =
        if (!file.isFile) null else runCatching { JSONObject(file.readText()) }.getOrNull()

    private fun head(file: File): String = runCatching {
        file.bufferedReader().use { reader ->
            val buffer = CharArray(FRONTMATTER_LIMIT)
            var read = 0
            while (read < buffer.size) {
                val count = reader.read(buffer, read, buffer.size - read)
                if (count < 0) break
                read += count
            }
            String(buffer, 0, read)
        }
    }.getOrDefault("")
}

/**
 * The `key: value` pairs of a leading `---` frontmatter block. Handles quoted
 * values, `>`/`|` block scalars and indented continuation lines, which is all
 * that skill, command and agent files use; anything else is skipped.
 */
internal fun frontmatter(text: String): Map<String, String> {
    val lines = text.lines()
    if (lines.firstOrNull()?.trim() != "---") return emptyMap()
    val end = (1 until lines.size).firstOrNull { lines[it].trim() == "---" } ?: return emptyMap()
    val fields = linkedMapOf<String, String>()
    val keyLine = Regex("""^([A-Za-z0-9_-]+):(.*)$""")
    var index = 1
    while (index < end) {
        val match = keyLine.find(lines[index])
        index++
        if (match == null) continue
        val key = match.groupValues[1]
        val value = match.groupValues[2].trim()
        val continuation = mutableListOf<String>()
        while (index < end && (lines[index].isBlank() || lines[index].first().isWhitespace())) {
            continuation += lines[index].trim()
            index++
        }
        fields[key] = when {
            value.startsWith("|") -> continuation.joinToString("\n").trim()
            value.startsWith(">") -> continuation.filter(String::isNotEmpty).joinToString(" ")
            else -> unquote((listOf(value) + continuation.filter(String::isNotEmpty)).joinToString(" ").trim())
        }
    }
    return fields
}

private fun unquote(value: String): String = when {
    value.length >= 2 && value.first() == '"' && value.last() == '"' ->
        value.substring(1, value.length - 1).replace("\\\"", "\"").replace("\\\\", "\\")
    value.length >= 2 && value.first() == '\'' && value.last() == '\'' ->
        value.substring(1, value.length - 1).replace("''", "'")
    else -> value
}
