package com.lingxi.code.apps

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.apache.commons.compress.archivers.zip.ZipArchiveEntry
import org.apache.commons.compress.archivers.zip.ZipArchiveOutputStream
import org.json.JSONObject
import java.io.File
import java.util.UUID
import kotlin.coroutines.coroutineContext

data class AppSourceExport(val appId: String, val revision: Long, val digest: String, val files: List<String>, val archive: File) : AutoCloseable {
    override fun close() { archive.parentFile?.deleteRecursively() }
}

/** The canonical exporter chooses files; native code only preserves that exact inventory in ZIP. */
internal suspend fun UnifiedLocalApps.sourceArchive(id: String, revision: Long, cache: File): AppSourceExport = withContext(Dispatchers.IO) {
    val release = release(id, revision)
    val folder = File(cache, "local-app-source-export/${UUID.randomUUID()}")
    check(folder.mkdirs()) { "Unable to prepare source export" }
    try {
        val source = File(folder, "source")
        val result = request("export", id) { put("revision", revision); put("destination", source.absolutePath) } as JSONObject
        val inventory = result.getJSONArray("files")
        val files = (0 until inventory.length()).map { inventory.getJSONObject(it) }.sortedBy { it.getString("path") }
        val archive = File(folder, "source.zip")
        ZipArchiveOutputStream(archive).use { zip ->
            for (item in files) {
                coroutineContext.ensureActive()
                val path = item.getString("path")
                val file = File(source, path).canonicalFile
                check(file.toPath().startsWith(source.canonicalFile.toPath()) && file.isFile) { "Exported source escaped its inventory" }
                val entry = ZipArchiveEntry(path).apply { time = 0; unixMode = 0x8000 or if (item.getBoolean("executable")) 0b111000000 else 0b110000000 }
                zip.putArchiveEntry(entry)
                file.inputStream().use { input -> input.copyTo(zip) }
                zip.closeArchiveEntry()
            }
        }
        check(archive.length() in 1..25L * 1024 * 1024) { "Source archive exceeds 25 MB" }
        AppSourceExport(id, revision, release.digest, files.map { it.getString("path") }, archive)
    } catch (error: Throwable) { folder.deleteRecursively(); throw error }
}
