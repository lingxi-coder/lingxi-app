package com.lingxi.code.apps

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class LegacyAppEntry(val key: String, val name: String, val kind: String, val sourcePath: String? = null)

internal suspend fun UnifiedLocalApps.legacyEntries(): List<LegacyAppEntry> = withContext(Dispatchers.IO) {
    val chat = request("legacyEntries") as JSONArray
    val result = (0 until chat.length()).map { i -> chat.getJSONObject(i).let { LegacyAppEntry(it.getString("legacyId"), it.getString("name"), "聊天应用归档") } }.toMutableList()
    val root = legacyMarketRoot()
    val registry = File(root, "registry.json")
    if (registry.isFile) {
        require(registry.length() <= 2 * 1024 * 1024) { "Legacy market registry is too large" }
        val rows = JSONObject(registry.readText()).getJSONArray("apps")
        for (index in 0 until rows.length()) {
            val row = rows.getJSONObject(index)
            val directory = row.getString("directory")
            require(directory.matches(Regex("[a-zA-Z0-9-]{1,80}"))) { "Invalid legacy market directory" }
            val versions = File(root, "versions").canonicalFile
            val path = File(versions, directory).canonicalFile
            require(path.parentFile == versions && !Files.isSymbolicLink(path.toPath())) { "Invalid legacy market package" }
            if (path.isDirectory) result.add(LegacyAppEntry("market:${row.getString("installationId")}", row.getString("appId"), "市场应用归档", path.absolutePath))
        }
    }
    result
}

internal suspend fun UnifiedLocalApps.importLegacy(entry: LegacyAppEntry): AppRecord = withContext(Dispatchers.IO) {
    val guest = newLegacyImportGuestPath()
    val source = hostFile(guest)
    require(!source.exists()) { "Import directory already exists" }
    val authoring = JSONObject().put("sourceDirectory", guest)
    if (entry.sourcePath == null) {
        val value = request("importLegacy") { put("legacyId", entry.key); put("sourcePath", source.absolutePath); put("authoring", authoring) } as JSONObject
        refresh()
        value.getJSONObject("record").appRecord()
    } else {
        val old = File(entry.sourcePath).canonicalFile
        require(old.parentFile == File(legacyMarketRoot(), "versions").canonicalFile && old.isDirectory) { "Legacy package is unavailable" }
        check(source.mkdirs())
        var bytes = 0L
        val files = mutableListOf<String>()
        for (file in old.walkTopDown()) {
            require(!Files.isSymbolicLink(file.toPath())) { "Legacy package contains a symbolic link" }
            if (file == old || file.isDirectory) continue
            val path = file.relativeTo(old).invariantSeparatorsPath
            require(path != "lingxi-app.json" && files.size < 2000) { "Legacy package cannot be safely imported" }
            bytes += file.length(); require(bytes < 100L * 1024 * 1024) { "Legacy package is too large" }
            val destination = File(source, path); destination.parentFile!!.mkdirs(); file.copyTo(destination)
            files.add(path)
        }
        require("index.html" in files) { "Legacy package has no index.html" }
        val manifest = JSONObject().put("schema", 2).put("name", entry.name.take(60))
            .put("entry", JSONObject().put("type", "static").put("path", "index.html"))
            .put("targets", JSONArray(listOf(JSONObject().put("platform", "android").put("environment", "web"))))
            .put("package", JSONObject().put("files", JSONArray(files + "lingxi-app.json")).put("dependencyLocks", JSONArray()))
        File(source, "lingxi-app.json").writeText(manifest.toString(2))
        val value = request("import") { put("sourcePath", source.absolutePath); put("authoring", authoring) } as JSONObject
        refresh()
        value.getJSONObject("record").appRecord()
    }
}
