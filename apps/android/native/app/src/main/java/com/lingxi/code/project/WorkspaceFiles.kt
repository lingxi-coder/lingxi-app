package com.lingxi.code.project

import com.lingxi.code.R
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import java.io.File
import java.nio.file.Files
import java.util.UUID

internal enum class FileConflict { KeepBoth, Skip, Cancel }
internal enum class FileSort { Name, Modified, Size, Type }
internal data class WorkspaceFileEntry(val path: String, val name: String, val directory: Boolean,
    val symbolicLink: Boolean, val bytes: Long, val modified: Long) {
    val extension get() = name.substringAfterLast('.', "").lowercase()
    val kind get() = when {
        directory -> "folder"
        extension in setOf("jpg", "jpeg", "png", "gif", "webp", "heic", "bmp") -> "image"
        extension == "pdf" -> "pdf"
        extension in setOf("mp4", "mov", "mkv", "webm", "m4v", "mp3", "wav", "m4a", "flac", "ogg", "aac") -> "media"
        extension in setOf("txt", "md", "swift", "kt", "rs", "py", "json", "yaml", "yml", "html", "htm", "xhtml", "css", "js", "ts", "tsx", "jsx", "xml", "svg", "sh", "toml", "csv", "log", "ini", "conf", "c", "h", "cpp", "java", "sql") || name in setOf("Dockerfile", "Makefile", ".gitignore") -> "text"
        else -> "other"
    }
}
internal data class FileProgress(val completed: Int, val total: Int, val name: String)
internal data class FileFailure(val path: String, val message: String, val messageResource: Int? = null)
internal data class FileSnapshot(val directory: File, val file: File) : AutoCloseable {
    override fun close() { directory.deleteRecursively() }
}

internal fun naturalFileCompare(a: String, b: String): Int {
    val parts = Regex("[0-9]+|[^0-9]+")
    val x = parts.findAll(a.lowercase()).map { it.value }.toList()
    val y = parts.findAll(b.lowercase()).map { it.value }.toList()
    for (i in 0 until minOf(x.size, y.size)) {
        val numeric = x[i].first().isDigit() && y[i].first().isDigit()
        val result = if (numeric) {
            val left = x[i].trimStart('0').ifEmpty { "0" }; val right = y[i].trimStart('0').ifEmpty { "0" }
            left.length.compareTo(right.length).takeIf { it != 0 } ?: left.compareTo(right)
        } else x[i].compareTo(y[i])
        if (result != 0) return result
    }
    return x.size.compareTo(y.size).takeIf { it != 0 } ?: a.compareTo(b)
}
internal fun sortedFiles(rows: List<WorkspaceFileEntry>, sort: FileSort): List<WorkspaceFileEntry> = rows.sortedWith { a, b ->
    if (a.directory != b.directory) return@sortedWith if (a.directory) -1 else 1
    val result = when (sort) {
        FileSort.Name -> 0
        FileSort.Modified -> b.modified.compareTo(a.modified)
        FileSort.Size -> b.bytes.compareTo(a.bytes)
        FileSort.Type -> a.extension.compareTo(b.extension)
    }
    if (result != 0) result else naturalFileCompare(a.name, b.name)
}
internal suspend fun WorkspaceDirectoryRepository.files(path: String, hidden: Boolean, query: String, recursive: Boolean): List<WorkspaceFileEntry> {
    val result = mutableListOf<WorkspaceFileEntry>()
    val pending = ArrayDeque<String>().apply { add(path) }
    while (pending.isNotEmpty()) {
        currentCoroutineContext().ensureActive()
        entries(pending.removeLast(), hidden).forEach { file ->
            currentCoroutineContext().ensureActive()
            val link = Files.isSymbolicLink(file.toPath())
            val attrs = Files.readAttributes(file.toPath(), java.nio.file.attribute.BasicFileAttributes::class.java, java.nio.file.LinkOption.NOFOLLOW_LINKS)
            val row = WorkspaceFileEntry(guest(file), file.name, file.isDirectory, link, if (attrs.isRegularFile) attrs.size() else 0L, attrs.lastModifiedTime().toMillis())
            if (query.isBlank() || file.name.contains(query, ignoreCase = true)) result += row
            if (recursive && query.isNotBlank() && row.directory && !link) pending.add(row.path)
        }
    }
    return result
}
internal suspend fun copyWorkspaceTree(source: File, target: File) {
    currentCoroutineContext().ensureActive()
    check(!Files.isSymbolicLink(source.toPath())) { "Symbolic links cannot be copied" }
    if (source.isDirectory) {
        check(target.mkdir())
        (source.listFiles() ?: error("Cannot read directory")).forEach { copyWorkspaceTree(it, File(target, it.name)) }
    } else {
        check(Files.isRegularFile(source.toPath())) { "Not a regular file" }
        source.inputStream().use { input -> target.outputStream().use { output ->
            val buffer = ByteArray(256 * 1024)
            while (true) {
                currentCoroutineContext().ensureActive()
                val count = input.read(buffer)
                if (count < 0) break
                output.write(buffer, 0, count)
            }
        } }
    }
}
internal fun availableWorkspaceTarget(parent: File, name: String, conflict: FileConflict): File? {
    var target = File(parent, name)
    if (!Files.exists(target.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) return target
    when (conflict) {
        FileConflict.Skip -> return null
        FileConflict.Cancel -> throw kotlinx.coroutines.CancellationException()
        FileConflict.KeepBoth -> {
            val extension = target.extension
            val stem = if (extension.isEmpty()) name else target.nameWithoutExtension
            var index = 2
            do { target = File(parent, "$stem (${index++})" + if (extension.isEmpty()) "" else ".$extension") }
            while (Files.exists(target.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS))
            return target
        }
    }
}
internal suspend fun stagedWorkspaceCopy(source: File, target: File) {
    check(!target.canonicalFile.toPath().startsWith(source.canonicalFile.toPath())) { "Destination is inside source" }
    val staging = File(target.parentFile, ".lingxi-copy-${UUID.randomUUID()}")
    try {
        copyWorkspaceTree(source, staging)
        currentCoroutineContext().ensureActive()
        check(!Files.exists(target.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) { "Destination already exists" }
        Files.move(staging.toPath(), target.toPath())
    } finally { staging.deleteRecursively() }
}
internal suspend fun WorkspaceDirectoryRepository.snapshot(path: String, cache: File): FileSnapshot {
    val source = resolve(path, false)
    check(source.isFile)
    val directory = File(cache, UUID.randomUUID().toString()).apply { check(mkdirs()) }
    val target = File(directory, source.name)
    try { copyWorkspaceTree(source, target); return FileSnapshot(directory, target) }
    catch (error: Exception) { directory.deleteRecursively(); throw error }
}
internal fun FileSnapshot.textPreview(): String {
    val data = file.inputStream().use { input ->
        val limit = 2 * 1024 * 1024 + 1
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(65536)
        while (output.size() < limit) { val count = input.read(buffer, 0, minOf(buffer.size, limit - output.size())); if (count < 0) break; output.write(buffer, 0, count) }
        output.toByteArray()
    }
    if (data.size > 2 * 1024 * 1024) throw WorkspaceDirectoryException(R.string.files_large_text)
    if (data.contains(0)) throw WorkspaceDirectoryException(R.string.files_binary)
    return try { Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(data)).toString() }
    catch (_: java.nio.charset.CharacterCodingException) { throw WorkspaceDirectoryException(R.string.files_binary) }
}
internal suspend fun WorkspaceDirectoryRepository.transferFiles(sources: List<String>, destination: String, move: Boolean,
    conflict: FileConflict, protected: List<String>, progress: suspend (FileProgress) -> Unit): List<FileFailure> {
    val parent = writable(destination)
    val selected = sources.filter { path -> sources.none { path != it && path.startsWith("$it/") } }
    val failures = mutableListOf<FileFailure>()
    selected.forEachIndexed { index, path ->
        currentCoroutineContext().ensureActive()
        try {
            val original = File(home, path.removePrefix(guestHome).trimStart('/'))
            check(!Files.isSymbolicLink(original.toPath())) { "Symbolic link" }
            val source = if (move) mutable(path, protected) else resolve(path, false)
            if (parent == source || parent.toPath().startsWith(source.toPath())) throw WorkspaceDirectoryException(R.string.files_self_move)
            availableWorkspaceTarget(parent, source.name, conflict)?.let { target ->
                if (move) {
                    val old = guest(source); val next = guest(target)
                    val saved = locations().map { row -> if (row.path == old || row.path.startsWith("$old/")) row.copy(path = next + row.path.removePrefix(old)) else row }
                    Files.move(source.toPath(), target.toPath())
                    try { write(saved) } catch (error: Exception) { Files.move(target.toPath(), source.toPath()); throw error }
                } else stagedWorkspaceCopy(source, target)
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
        catch (error: Exception) { failures += FileFailure(path, error.message.orEmpty(), (error as? WorkspaceDirectoryException)?.messageResource) }
        progress(FileProgress(index + 1, selected.size, path.substringAfterLast('/')))
    }
    return failures
}
internal suspend fun WorkspaceDirectoryRepository.deleteFiles(sources: List<String>, protected: List<String>, progress: suspend (FileProgress) -> Unit): List<FileFailure> {
    val selected = sources.filter { path -> sources.none { path != it && path.startsWith("$it/") } }
    val failures = mutableListOf<FileFailure>()
    selected.forEachIndexed { index, path ->
        currentCoroutineContext().ensureActive()
        try { delete(path, protected) }
        catch (error: Exception) { failures += FileFailure(path, error.message.orEmpty(), (error as? WorkspaceDirectoryException)?.messageResource) }
        progress(FileProgress(index + 1, selected.size, path.substringAfterLast('/')))
    }
    return failures
}
