package com.lingxi.code.apps

import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.MessageDigest
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

class AppSourcePackageTest {
    @get:Rule val temp = TemporaryFolder()
    private fun zip(files: Map<String, String>): File = temp.newFile().also { file ->
        ZipOutputStream(file.outputStream()).use { output -> files.forEach { (name, text) -> output.putNextEntry(ZipEntry(name)); output.write(text.toByteArray()); output.closeEntry() } }
    }
    private fun inventory(files: Map<String, String>, executable: String? = null) = JSONArray(files.map { (name, text) ->
        JSONObject().put("path", name).put("sizeBytes", text.toByteArray().size)
            .put("sha256", MessageDigest.getInstance("SHA-256").digest(text.toByteArray()).joinToString("") { "%02x".format(it) })
            .put("executable", name == executable)
    })
    @Test fun `verified package preserves executable scripts`() = runBlocking {
        val files = mapOf("lingxi-app.json" to "{}", "bin/start.sh" to "#!/bin/sh\necho hello")
        val out = temp.newFolder()
        unpackAppSource(zip(files), out, inventory(files, "bin/start.sh"))
        assertEquals(files.getValue("bin/start.sh"), File(out, "bin/start.sh").readText())
        assertTrue(File(out, "bin/start.sh").canExecute())
    }
    @Test fun `undeclared files and traversal are rejected`() {
        val files = mapOf("lingxi-app.json" to "{}")
        assertThrows(IllegalArgumentException::class.java) { runBlocking { unpackAppSource(zip(files + ("secret" to "x")), temp.newFolder(), inventory(files)) } }
        assertThrows(IllegalArgumentException::class.java) { runBlocking { unpackAppSource(zip(files + ("../escape" to "x")), temp.newFolder(), inventory(files)) } }
    }
    @Test fun `wrong digests and incomplete inventories are rejected`() {
        val files = mapOf("lingxi-app.json" to "{}", "index.html" to "hello")
        val incorrect = inventory(files).apply { getJSONObject(1).put("sha256", "0".repeat(64)) }
        assertThrows(IllegalArgumentException::class.java) { runBlocking { unpackAppSource(zip(files), temp.newFolder(), incorrect) } }
        assertThrows(IllegalArgumentException::class.java) { runBlocking { unpackAppSource(zip(mapOf("lingxi-app.json" to "{}")), temp.newFolder(), inventory(files)) } }
    }
    @Test fun `local source import derives a bounded inventory without market claims`() = runBlocking {
        val files = mapOf("lingxi-app.json" to "{}", "dist/index.html" to "local only")
        val out = temp.newFolder()
        unpackLocalAppSource(zip(files), out)
        assertEquals("local only", File(out, "dist/index.html").readText())
        assertFalse(File(out, "dist/index.html").canExecute())
    }
    @Test fun `local source imports reject ambiguous names and missing manifests`() {
        assertThrows(IllegalArgumentException::class.java) { runBlocking {
            unpackLocalAppSource(zip(mapOf("lingxi-app.json" to "{}", "index.html" to "a", "INDEX.html" to "b")), temp.newFolder())
        } }
        assertThrows(IllegalArgumentException::class.java) { runBlocking {
            unpackLocalAppSource(zip(mapOf("index.html" to "a")), temp.newFolder())
        } }
    }
}
