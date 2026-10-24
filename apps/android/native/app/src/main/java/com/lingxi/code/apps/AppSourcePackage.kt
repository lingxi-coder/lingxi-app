package com.lingxi.code.apps

import kotlinx.coroutines.ensureActive
import org.apache.commons.compress.archivers.zip.ZipFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import kotlin.coroutines.coroutineContext

/** Local source imports are unsigned. Validate the ZIP before canonical manifest import;
 * importing never grants execution or marks the package as market-reviewed.
 */
internal suspend fun unpackLocalAppSource(archive: File, output: File) {
    val inventory = JSONArray()
    val names = mutableSetOf<String>()
    var total = 0L
    var count = 0
    ZipFile.builder().setFile(archive).get().use { zip ->
        for (entry in zip.entries.asSequence()) {
            coroutineContext.ensureActive()
            require(++count <= 2000) { "Too many source package entries" }
            val name = entry.name
            require(!name.startsWith('/') && !name.contains('\\') && !name.contains(':') &&
                name.trimEnd('/').split('/').all { it.isNotEmpty() && it != "." && it != ".." }) { "Invalid package path" }
            val normalized = java.text.Normalizer.normalize(name.trimEnd('/'), java.text.Normalizer.Form.NFC).lowercase(java.util.Locale.ROOT)
            require(names.add(normalized)) { "Ambiguous or duplicate package path" }
            require(!entry.isUnixSymlink && !entry.generalPurposeBit.usesEncryption() && zip.canReadEntryData(entry)) { "Unsupported package entry" }
            if (entry.isDirectory) continue
            val digest = MessageDigest.getInstance("SHA-256")
            val crc = java.util.zip.CRC32()
            var size = 0L
            zip.getInputStream(entry).use { input ->
                val bytes = ByteArray(32768)
                while (true) {
                    coroutineContext.ensureActive()
                    val read = input.read(bytes); if (read < 0) break
                    size += read; total += read
                    require(size <= 25L * 1024 * 1024 && total <= 100L * 1024 * 1024) { "Expanded source package is too large" }
                    digest.update(bytes, 0, read); crc.update(bytes, 0, read)
                }
            }
            require(size == entry.size && crc.value == entry.crc) { "Damaged source package" }
            inventory.put(JSONObject().put("path", name).put("sizeBytes", size)
                .put("sha256", digest.digest().joinToString("") { "%02x".format(it) })
                .put("executable", entry.unixMode and 73 != 0))
        }
    }
    unpackAppSource(archive, output, inventory)
}

internal suspend fun unpackAppSource(archive: File, output: File, inventory: JSONArray) {
    val files = (0 until inventory.length()).associate { index -> inventory.getJSONObject(index).let { it.getString("path") to it } }
    require(files.size == inventory.length() && "lingxi-app.json" in files) { "Invalid source inventory" }
    val seen = mutableSetOf<String>()
    var total = 0L
    ZipFile.builder().setFile(archive).get().use { zip ->
        val entries = zip.entries
        while (entries.hasMoreElements()) {
            coroutineContext.ensureActive()
            val entry = entries.nextElement()
            val name = entry.name
            require(!name.startsWith('/') && !name.contains('\\') && !name.contains(':') && name.trimEnd('/').split('/').all { it.isNotEmpty() && it != "." && it != ".." }) { "Invalid package path" }
            require(!entry.isUnixSymlink && !entry.generalPurposeBit.usesEncryption() && zip.canReadEntryData(entry)) { "Unsupported package entry" }
            if (entry.isDirectory) continue
            require(seen.add(name)) { "Duplicate package file" }
            val expected = requireNotNull(files[name]) { "Undeclared package file" }
            require(expected.getLong("sizeBytes") in 0..100L * 1024 * 1024) { "Invalid file size" }
            val file = File(output, name)
            check(file.parentFile!!.isDirectory || file.parentFile!!.mkdirs())
            val digest = MessageDigest.getInstance("SHA-256")
            var size = 0L
            zip.getInputStream(entry).use { input -> file.outputStream().use { out ->
                val buffer = ByteArray(32768)
                while (true) {
                    coroutineContext.ensureActive()
                    val count = input.read(buffer); if (count < 0) break
                    total += count; size += count
                    require(total <= 100L * 1024 * 1024 && size <= expected.getLong("sizeBytes")) { "Package exceeds declared size" }
                    digest.update(buffer, 0, count); out.write(buffer, 0, count)
                }
            } }
            require(file.setExecutable(expected.optBoolean("executable", false), true)) { "Cannot apply package file permissions" }
            require(size == expected.getLong("sizeBytes") && digest.digest().joinToString("") { "%02x".format(it) } == expected.getString("sha256")) { "Package file hash mismatch" }
        }
    }
    require(seen == files.keys) { "Incomplete package" }
}
