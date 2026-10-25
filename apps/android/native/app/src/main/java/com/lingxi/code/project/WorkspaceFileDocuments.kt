package com.lingxi.code.project

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import java.io.File
import java.util.UUID

internal fun fileMime(name: String): String = android.webkit.MimeTypeMap.getSingleton()
    .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"

internal suspend fun WorkspaceDirectoryRepository.importDocuments(context: Context, uris: List<Uri>, destination: String,
    tree: Boolean, conflict: FileConflict, progress: suspend (FileProgress) -> Unit): List<FileFailure> {
    val parent = writable(destination)
    val failures = mutableListOf<FileFailure>()
    uris.forEachIndexed { index, uri ->
        currentCoroutineContext().ensureActive()
        var name = uri.lastPathSegment.orEmpty()
        try {
            val document = (if (tree) DocumentFile.fromTreeUri(context, uri) else DocumentFile.fromSingleUri(context, uri)) ?: error("Unavailable document")
            name = document.name ?: error("Missing file name")
            validDocumentName(name)
            availableWorkspaceTarget(parent, name, conflict)?.let { target ->
                val staging = File(parent, ".lingxi-copy-${UUID.randomUUID()}")
                try { copyDocument(context, document, staging); currentCoroutineContext().ensureActive(); java.nio.file.Files.move(staging.toPath(), target.toPath()) }
                finally { staging.deleteRecursively() }
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
        catch (error: Exception) { failures += FileFailure(name, error.message.orEmpty()) }
        progress(FileProgress(index + 1, uris.size, name))
    }
    return failures
}
private fun validDocumentName(name: String) {
    require(name.isNotBlank() && name != "." && name != ".." && !name.contains('/') && !name.any(Char::isISOControl)) { "Invalid document name" }
}
private suspend fun copyDocument(context: Context, source: DocumentFile, target: File) {
    currentCoroutineContext().ensureActive()
    if (source.isDirectory) {
        check(target.mkdir())
        source.listFiles().forEach { child ->
            val name = requireNotNull(child.name); validDocumentName(name)
            copyDocument(context, child, File(target, name))
        }
    } else {
        check(source.isFile)
        context.contentResolver.openInputStream(source.uri).use { input ->
            checkNotNull(input)
            target.outputStream().use { output ->
                val buffer = ByteArray(256 * 1024)
                while (true) { currentCoroutineContext().ensureActive(); val count = input.read(buffer); if (count < 0) break; output.write(buffer, 0, count) }
            }
        }
    }
}
internal suspend fun WorkspaceDirectoryRepository.exportDocuments(context: Context, paths: List<String>, destination: Uri,
    conflict: FileConflict, progress: suspend (FileProgress) -> Unit): List<FileFailure> {
    val parent = requireNotNull(DocumentFile.fromTreeUri(context, destination))
    val failures = mutableListOf<FileFailure>()
    paths.forEachIndexed { index, path ->
        currentCoroutineContext().ensureActive()
        try { exportDocument(context, resolve(path, false), parent, conflict) }
        catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
        catch (error: Exception) { failures += FileFailure(path, error.message.orEmpty()) }
        progress(FileProgress(index + 1, paths.size, path.substringAfterLast('/')))
    }
    return failures
}
private suspend fun exportDocument(context: Context, source: File, parent: DocumentFile, conflict: FileConflict) {
    currentCoroutineContext().ensureActive()
    check(!java.nio.file.Files.isSymbolicLink(source.toPath())) { "Symbolic link" }
    var name = source.name
    if (parent.findFile(name) != null) {
        when (conflict) {
            FileConflict.Skip -> return
            FileConflict.Cancel -> throw kotlinx.coroutines.CancellationException()
            FileConflict.KeepBoth -> { var i = 2; while (parent.findFile(name) != null) { name = source.nameWithoutExtension + " (${i++})" + if (source.extension.isEmpty()) "" else ".${source.extension}" } }
        }
    }
    val stagingName = ".lingxi-export-${UUID.randomUUID()}"
    val target = requireNotNull(if (source.isDirectory) parent.createDirectory(stagingName) else parent.createFile(fileMime(name), stagingName))
    var complete = false
    try {
        if (source.isDirectory) (source.listFiles() ?: error("Cannot read directory")).forEach { exportDocument(context, it, target, FileConflict.Cancel) }
        else {
            check(java.nio.file.Files.isRegularFile(source.toPath()))
            source.inputStream().use { input -> context.contentResolver.openOutputStream(target.uri, "w").use { output ->
                checkNotNull(output); val buffer = ByteArray(256 * 1024)
                while (true) { currentCoroutineContext().ensureActive(); val count = input.read(buffer); if (count < 0) break; output.write(buffer, 0, count) }
            } }
        }
        currentCoroutineContext().ensureActive()
        check(parent.findFile(name) == null && target.renameTo(name)) { "Cannot finish export" }
        complete = true
    } finally { if (!complete) target.delete() }
}
