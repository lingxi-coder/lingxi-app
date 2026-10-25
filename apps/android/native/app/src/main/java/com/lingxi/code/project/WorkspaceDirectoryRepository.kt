package com.lingxi.code.project

import com.lingxi.code.R
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.Properties
import java.util.UUID

internal data class WorkspaceLocation(val id: String, val name: String, val path: String)
internal class WorkspaceDirectoryException(val messageResource: Int) : Exception()

/** Native backing files of the SDK's Linux home. No shell or rootfs access. Call on IO. */
internal class WorkspaceDirectoryRepository(
    private val home: File,
    val guestHome: String,
    val workspace: String,
    private val projects: String,
    private val config: File,
) {
    private val store get() = File(config, "workspace-locations.properties")
    private fun fail(resource: Int): Nothing = throw WorkspaceDirectoryException(resource)
    private fun within(file: File, root: File) = file == root || file.toPath().startsWith(root.toPath())
    fun resolve(path: String): File {
        val expanded = when { path == "~" -> guestHome; path.startsWith("~/") -> guestHome + path.drop(1); else -> path }
        if ((expanded != guestHome && !expanded.startsWith("$guestHome/")) ||
            expanded.contains('\u0000') || expanded.split('/').any { it == "." || it == ".." } || Files.isSymbolicLink(home.toPath()))
            fail(R.string.workspace_browser_outside_home)
        val file = File(home, expanded.removePrefix(guestHome).trimStart('/')).canonicalFile
        if (!within(file, home.canonicalFile)) fail(R.string.workspace_browser_outside_home)
        if (!file.isDirectory) fail(R.string.workspace_browser_not_directory)
        return file
    }
    fun guest(file: File): String = guestHome + file.canonicalPath.removePrefix(home.canonicalPath)
    fun canonical(path: String) = guest(resolve(path))
    fun display(path: String) = if (path == guestHome) "~" else if (path.startsWith("$guestHome/")) "~" + path.removePrefix(guestHome) else path
    fun entries(path: String, hidden: Boolean): List<File> = (resolve(path).listFiles()
        ?: fail(R.string.workspace_browser_not_directory)).filter { hidden || !it.name.startsWith('.') }
        .sortedWith(compareBy<File> { !it.isDirectory }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    @Synchronized fun locations(): List<WorkspaceLocation> {
        checkStore()
        if (!store.exists()) return emptyList()
        val props = Properties().apply { store.inputStream().use { load(it) } }
        return props.getProperty("ids", "").split(',').filter { it.isNotEmpty() }.map { id ->
            WorkspaceLocation(id, props.getProperty("$id.name"), props.getProperty("$id.path"))
        }
    }
    private fun checkStore() {
        if (!within(config.canonicalFile, home.canonicalFile) || Files.isSymbolicLink(config.toPath()) || Files.isSymbolicLink(store.toPath()))
            fail(R.string.workspace_browser_outside_home)
    }
    private fun write(rows: List<WorkspaceLocation>) {
        checkStore()
        check(config.isDirectory || config.mkdirs())
        val props = Properties().apply {
            setProperty("ids", rows.joinToString(",") { it.id })
            rows.forEach { setProperty("${it.id}.name", it.name); setProperty("${it.id}.path", it.path) }
        }
        val temporary = File.createTempFile("workspace-locations-", ".tmp", config)
        try {
            temporary.outputStream().use { props.store(it, null) }
            Files.move(temporary.toPath(), store.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally { temporary.delete() }
    }
    @Synchronized fun save(path: String, name: String, id: String? = null) {
        val canonical = canonical(path)
        if (name.isBlank()) fail(R.string.workspace_browser_invalid_name)
        val rows = locations().toMutableList()
        if (id == null && rows.any { it.path == canonical }) return
        if (rows.any { it.path == canonical && it.id != id }) fail(R.string.workspace_browser_already_exists)
        val row = WorkspaceLocation(id ?: UUID.randomUUID().toString(), name.trim(), canonical)
        if (id == null) rows.add(row) else {
            val index = rows.indexOfFirst { it.id == id }
            if (index < 0) fail(R.string.workspace_browser_not_directory)
            rows[index] = row
        }
        write(rows)
    }
    @Synchronized fun unlink(id: String) = write(locations().filterNot { it.id == id })
    private fun name(value: String): String = value.trim().also {
        if (it.isEmpty() || it == "." || it == ".." || it.contains('/') || it.any(Char::isISOControl)) fail(R.string.workspace_browser_invalid_name)
    }
    private fun writable(path: String): File = resolve(path).also {
        if (within(it, config.canonicalFile)) fail(R.string.workspace_browser_in_use)
    }
    fun create(parent: String, name: String): String {
        val target = File(writable(parent), name(name))
        if (Files.exists(target.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) fail(R.string.workspace_browser_already_exists)
        check(target.mkdir())
        return guest(target)
    }
    private fun mutable(path: String, protected: List<String>): File {
        val target = writable(path)
        val original = File(home, path.removePrefix(guestHome).trimStart('/'))
        if (Files.isSymbolicLink(original.toPath())) fail(R.string.workspace_browser_in_use)
        (listOf(guestHome, workspace, projects) + protected).forEach { item ->
            val protectedFile = runCatching { resolve(item) }.getOrNull()
            if (protectedFile != null && within(protectedFile, target)) fail(R.string.workspace_browser_in_use)
        }
        return target
    }
    @Synchronized fun rename(path: String, name: String, protected: List<String>) {
        val file = mutable(path, protected)
        val target = File(file.parentFile, name(name))
        if (Files.exists(target.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) fail(R.string.workspace_browser_already_exists)
        val old = guest(file)
        val next = guest(target)
        val rows = locations().map { if (it.path == old || it.path.startsWith("$old/"))
            it.copy(path = next + it.path.removePrefix(old), name = if (it.path == old && it.name == file.name) target.name else it.name) else it }
        Files.move(file.toPath(), target.toPath())
        try { write(rows) } catch (error: Exception) { Files.move(target.toPath(), file.toPath()); throw error }
    }
    @Synchronized fun delete(path: String, protected: List<String>) {
        val file = mutable(path, protected)
        val canonical = guest(file)
        val previous = locations()
        write(previous.filterNot { it.path == canonical || it.path.startsWith("$canonical/") })
        try {
            // walkFileTree does not follow symbolic links outside this folder.
            Files.walkFileTree(file.toPath(), object : java.nio.file.SimpleFileVisitor<java.nio.file.Path>() {
                override fun visitFile(path: java.nio.file.Path, attrs: java.nio.file.attribute.BasicFileAttributes): java.nio.file.FileVisitResult {
                    Files.delete(path); return java.nio.file.FileVisitResult.CONTINUE
                }
                override fun postVisitDirectory(path: java.nio.file.Path, error: java.io.IOException?): java.nio.file.FileVisitResult {
                    if (error != null) throw error
                    Files.delete(path); return java.nio.file.FileVisitResult.CONTINUE
                }
            })
        } catch (error: Exception) { write(previous); throw error }
    }
}
