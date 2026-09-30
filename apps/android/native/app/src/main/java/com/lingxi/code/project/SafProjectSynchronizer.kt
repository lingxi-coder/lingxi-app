package com.lingxi.code.project

import android.content.ContentResolver
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.provider.DocumentsContract
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.nio.channels.Channels
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.BasicFileAttributes
import java.security.MessageDigest
import java.util.UUID

private const val SYNC_TEMP_MARKER = ".__lingxi_temp__"
private const val SYNC_BACKUP_MARKER = ".__lingxi_backup__"
private const val MAX_SYNC_ENTRIES = 50_000
private const val MAX_SYNC_FILE_BYTES = 2L * 1024 * 1024 * 1024
private const val MAX_SYNC_TOTAL_BYTES = 8L * 1024 * 1024 * 1024
private const val MIN_FREE_SPACE_BYTES = 64L * 1024 * 1024

data class ProjectSyncResult(
    val project: ProjectSnapshot,
    val conflicts: List<ProjectSyncConflict>,
    val copiedFiles: Int,
    val skippedFiles: Int,
)

internal enum class ProjectSyncDirection {
    ExternalToInternal,
    InternalToExternal,
}

internal enum class ProjectSyncAction {
    Conflict,
    CopyExternal,
    CopyInternal,
    UpdateBaseline,
    Skip,
}

/**
 * Resolve one path without allowing a whole-sync conflict choice to overwrite
 * unrelated one-sided edits. A KeepExternal/KeepInternal choice only applies
 * when this exact path is an actual two-sided conflict.
 */
internal fun decideProjectSyncAction(
    direction: ProjectSyncDirection,
    resolution: ConflictResolution?,
    baselineSha256: String?,
    externalSha256: String?,
    internalSha256: String?,
): ProjectSyncAction {
    val externalChanged = externalSha256 != baselineSha256
    val internalChanged = internalSha256 != baselineSha256
    val conflict =
        externalSha256 != null &&
            internalSha256 != null &&
            externalChanged &&
            internalChanged &&
            externalSha256 != internalSha256

    if (conflict) {
        return when {
            resolution == null -> ProjectSyncAction.Conflict
            direction == ProjectSyncDirection.ExternalToInternal &&
                resolution == ConflictResolution.KeepExternal -> ProjectSyncAction.CopyExternal
            direction == ProjectSyncDirection.InternalToExternal &&
                resolution == ConflictResolution.KeepInternal -> ProjectSyncAction.CopyInternal
            else -> ProjectSyncAction.Skip
        }
    }

    if (externalSha256 != null && externalSha256 == internalSha256) {
        return ProjectSyncAction.UpdateBaseline
    }

    return when (direction) {
        ProjectSyncDirection.ExternalToInternal -> when {
            externalSha256 == null -> ProjectSyncAction.Skip
            externalChanged && !internalChanged -> ProjectSyncAction.CopyExternal
            internalSha256 == null && baselineSha256 == null -> ProjectSyncAction.CopyExternal
            else -> ProjectSyncAction.Skip
        }

        ProjectSyncDirection.InternalToExternal -> when {
            internalSha256 == null -> ProjectSyncAction.Skip
            internalChanged && !externalChanged -> ProjectSyncAction.CopyInternal
            externalSha256 == null && baselineSha256 == null -> ProjectSyncAction.CopyInternal
            else -> ProjectSyncAction.Skip
        }
    }
}

internal fun persistedPermissionAllows(
    direction: ProjectSyncDirection,
    read: Boolean,
    write: Boolean,
): Boolean = read && (direction == ProjectSyncDirection.ExternalToInternal || write)

internal fun isLingxiSyncArtifact(name: String): Boolean =
    syncArtifactOriginalName(name, SYNC_TEMP_MARKER) != null ||
        syncArtifactOriginalName(name, SYNC_BACKUP_MARKER) != null

private fun syncArtifactOriginalName(name: String, marker: String): String? {
    val markerIndex = name.lastIndexOf(marker)
    if (markerIndex <= 0) return null
    val originalName = name.substring(0, markerIndex)
    val transactionId = name.substring(markerIndex + marker.length)
    if (!isSafeSegment(originalName)) return null
    return originalName.takeIf {
        runCatching { UUID.fromString(transactionId) }.isSuccess
    }
}

internal interface SafDocumentBackend {
    fun createDocument(parentUri: Uri, mimeType: String, displayName: String): Uri
    fun openInput(uri: Uri): InputStream
    fun openOutput(uri: Uri): OutputStream
    fun renameDocument(uri: Uri, displayName: String): Uri
    fun deleteDocument(uri: Uri)
}

/**
 * Write and verify a sibling document before swapping it into place. Providers
 * that cannot rename documents fail closed; the existing user document remains
 * available and a later sync can clean up transaction artifacts.
 */
internal suspend fun commitExternalDocument(
    backend: SafDocumentBackend,
    parentUri: Uri,
    targetUri: Uri?,
    displayName: String,
    mimeType: String,
    expectedSha256: String,
    source: () -> InputStream,
) {
    val tempName = "$displayName$SYNC_TEMP_MARKER${UUID.randomUUID()}"
    val backupName = "$displayName$SYNC_BACKUP_MARKER${UUID.randomUUID()}"
    var tempUri: Uri? = backend.createDocument(parentUri, mimeType, tempName)
    var backupUri: Uri? = null
    var committed = false
    try {
        source().use { input ->
            backend.openOutput(checkNotNull(tempUri)).use { output ->
                copyCancellable(input, output)
            }
        }
        val verified = backend.openInput(checkNotNull(tempUri)).use { sha256(it) }
        check(verified.sha256 == expectedSha256) {
            "external provider changed data while writing $displayName"
        }

        if (targetUri != null) {
            backupUri = backend.renameDocument(targetUri, backupName)
        }
        try {
            backend.renameDocument(checkNotNull(tempUri), displayName)
            tempUri = null
            committed = true
        } catch (commitError: Throwable) {
            backupUri?.let { backup ->
                runCatching { backend.renameDocument(backup, displayName) }
                    .onSuccess { backupUri = null }
            }
            throw commitError
        }
        backupUri?.let(backend::deleteDocument)
        backupUri = null
    } finally {
        tempUri?.let { runCatching { backend.deleteDocument(it) } }
        if (!committed) {
            backupUri?.let { backup ->
                runCatching { backend.renameDocument(backup, displayName) }
            }
        }
    }
}

internal class SafProjectSynchronizer(
    context: Context,
    private val repository: ProjectRepository,
    private val now: () -> Long = System::currentTimeMillis,
) {
    private val resolver: ContentResolver = context.applicationContext.contentResolver
    private val documentBackend: SafDocumentBackend = ContentResolverSafDocumentBackend(resolver)

    suspend fun importInitial(projectId: String): ProjectSyncResult =
        synchronize(projectId, ProjectSyncDirection.ExternalToInternal, resolution = null)

    suspend fun reimport(projectId: String): ProjectSyncResult =
        synchronize(projectId, ProjectSyncDirection.ExternalToInternal, resolution = null)

    suspend fun export(projectId: String): ProjectSyncResult =
        synchronize(projectId, ProjectSyncDirection.InternalToExternal, resolution = null)

    suspend fun resolve(
        projectId: String,
        resolution: ConflictResolution,
    ): ProjectSyncResult = synchronize(
        projectId = projectId,
        direction = when (resolution) {
            ConflictResolution.KeepInternal -> ProjectSyncDirection.InternalToExternal
            ConflictResolution.KeepExternal -> ProjectSyncDirection.ExternalToInternal
        },
        resolution = resolution,
    )

    private suspend fun synchronize(
        projectId: String,
        direction: ProjectSyncDirection,
        resolution: ConflictResolution?,
    ): ProjectSyncResult = withContext(Dispatchers.IO) {
        val snapshot = repository.project(projectId)
        require(snapshot.record.storageKind == ProjectStorageKind.SafMirror) {
            "internal projects do not have an external directory"
        }
        val treeUri = snapshot.record.sourceTreeUri?.let(Uri::parse)
            ?: throw IllegalStateException("project is missing its SAF tree URI")
        if (!hasPersistedPermission(treeUri, direction)) {
            val lost = repository.updateProject(
                snapshot.record.copy(syncState = ProjectSyncState.AuthorizationLost),
            )
            return@withContext ProjectSyncResult(lost, emptyList(), 0, 0)
        }

        repository.updateProject(snapshot.record.copy(syncState = ProjectSyncState.Syncing))
        try {
            if (direction == ProjectSyncDirection.InternalToExternal) {
                recoverExternalTransactions(treeUri)
            }
            val external = scanExternal(treeUri)
            val internal = scanInternal(snapshot.workspace.hostDirectory)
            val baseline = repository.readBaseline(projectId)
            val conflicts = mutableListOf<ProjectSyncConflict>()
            val nextBaseline = baseline.files.toMutableMap()
            var copied = 0
            var skipped = 0
            val allPaths = (external.keys + internal.keys).toSortedSet()

            for (path in allPaths) {
                currentCoroutineContext().ensureActive()
                val externalFile = external[path]
                val internalFile = internal[path]
                val baseHash = baseline.files[path]?.sha256
                val action = decideProjectSyncAction(
                    direction = direction,
                    resolution = resolution,
                    baselineSha256 = baseHash,
                    externalSha256 = externalFile?.sha256,
                    internalSha256 = internalFile?.sha256,
                )
                when (action) {
                    ProjectSyncAction.Conflict -> {
                        conflicts += ProjectSyncConflict(
                            projectId = projectId,
                            relativePath = path,
                            internalSha256 = checkNotNull(internalFile).sha256,
                            externalSha256 = checkNotNull(externalFile).sha256,
                            baselineSha256 = baseHash,
                        )
                        skipped++
                    }

                    ProjectSyncAction.CopyExternal -> {
                        val source = checkNotNull(externalFile)
                        copyExternalToInternal(
                            source = source,
                            workspace = snapshot.workspace.hostDirectory,
                            relativePath = path,
                        )
                        nextBaseline[path] = ProjectSyncFile(path, source.sha256, source.sizeBytes)
                        copied++
                    }

                    ProjectSyncAction.CopyInternal -> {
                        val source = checkNotNull(internalFile)
                        writeExternal(
                            treeUri = treeUri,
                            relativePath = path,
                            source = source,
                            workspaceRoot = snapshot.workspace.hostDirectory,
                        )
                        nextBaseline[path] = ProjectSyncFile(path, source.sha256, source.sizeBytes)
                        copied++
                    }

                    ProjectSyncAction.UpdateBaseline -> {
                        val hash = checkNotNull(externalFile?.sha256 ?: internalFile?.sha256)
                        val size = externalFile?.sizeBytes ?: checkNotNull(internalFile).sizeBytes
                        nextBaseline[path] = ProjectSyncFile(path, hash, size)
                    }

                    ProjectSyncAction.Skip -> skipped++
                }
            }

            repository.writeBaseline(projectId, ProjectSyncBaseline(nextBaseline))
            val current = repository.project(projectId)
            val state = when {
                conflicts.isNotEmpty() -> ProjectSyncState.Conflict
                skipped > 0 -> ProjectSyncState.ChangesPending
                else -> ProjectSyncState.Synced
            }
            val updated = repository.updateProject(
                current.record.copy(
                    lastSyncAtEpochMillis = now(),
                    syncState = state,
                ),
            )
            ProjectSyncResult(updated, conflicts, copied, skipped)
        } catch (t: Throwable) {
            val syncState = if (t is CancellationException) {
                ProjectSyncState.ChangesPending
            } else {
                ProjectSyncState.Error
            }
            withContext(NonCancellable) {
                val current = repository.project(projectId)
                repository.updateProject(current.record.copy(syncState = syncState))
            }
            throw t
        }
    }

    private fun hasPersistedPermission(
        treeUri: Uri,
        direction: ProjectSyncDirection,
    ): Boolean {
        val permission = resolver.persistedUriPermissions.firstOrNull { it.uri == treeUri }
            ?: return false
        return persistedPermissionAllows(
            direction = direction,
            read = permission.isReadPermission,
            write = permission.isWritePermission,
        )
    }

    private suspend fun scanExternal(treeUri: Uri): Map<String, ExternalFile> {
        val rootId = DocumentsContract.getTreeDocumentId(treeUri)
        val rootUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, rootId)
        val files = linkedMapOf<String, ExternalFile>()
        scanExternalDirectory(
            treeUri = treeUri,
            directoryUri = rootUri,
            prefix = "",
            files = files,
            visitedDocumentIds = mutableSetOf(),
            depth = 0,
            budget = SyncBudget(),
        )
        return files
    }

    private suspend fun scanExternalDirectory(
        treeUri: Uri,
        directoryUri: Uri,
        prefix: String,
        files: MutableMap<String, ExternalFile>,
        visitedDocumentIds: MutableSet<String>,
        depth: Int,
        budget: SyncBudget,
    ) {
        currentCoroutineContext().ensureActive()
        require(depth <= 64) { "external directory nesting exceeds the supported limit" }
        val parentId = DocumentsContract.getDocumentId(directoryUri)
        if (!visitedDocumentIds.add(parentId)) return
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, parentId)
        for (child in queryChildren(childrenUri)) {
            currentCoroutineContext().ensureActive()
            budget.includeEntry()
            val safeName = child.displayName.takeIf(::isSafeSegment) ?: continue
            if (isLingxiSyncArtifact(safeName)) continue
            val path = if (prefix.isEmpty()) safeName else "$prefix/$safeName"
            if (!isSafeRelativePath(path)) continue
            val uri = DocumentsContract.buildDocumentUriUsingTree(treeUri, child.documentId)
            if (child.mimeType == DocumentsContract.Document.MIME_TYPE_DIR) {
                scanExternalDirectory(
                    treeUri = treeUri,
                    directoryUri = uri,
                    prefix = path,
                    files = files,
                    visitedDocumentIds = visitedDocumentIds,
                    depth = depth + 1,
                    budget = budget,
                )
            } else {
                val hashed = resolver.openInputStream(uri)?.use { sha256(it) }
                    ?: throw IllegalStateException("cannot read external file: $path")
                budget.includeFile(hashed.sizeBytes)
                files[path] = ExternalFile(uri, hashed.sha256, hashed.sizeBytes)
            }
        }
    }

    private fun queryChildren(childrenUri: Uri): List<DocumentRow> {
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        )
        val cursor = resolver.query(childrenUri, projection, null, null, null)
            ?: throw IllegalStateException("cannot query external directory")
        return cursor.use { rows ->
            buildList {
                while (rows.moveToNext()) {
                    add(
                        DocumentRow(
                            documentId = rows.string(0),
                            displayName = rows.string(1),
                            mimeType = rows.string(2),
                        ),
                    )
                }
            }
        }
    }

    private suspend fun scanInternal(root: File): Map<String, InternalFile> {
        val canonicalRoot = requireCanonicalWorkspaceRoot(root)
        val rootPath = canonicalRoot.toPath()
        val files = linkedMapOf<String, InternalFile>()
        val directories = ArrayDeque<File>().apply { add(canonicalRoot) }
        val budget = SyncBudget()
        var depth = 0
        while (directories.isNotEmpty()) {
            currentCoroutineContext().ensureActive()
            val directory = directories.removeLast()
            depth = rootPath.relativize(directory.toPath()).nameCount
            require(depth <= 64) { "workspace nesting exceeds the supported limit" }
            val entries = Files.newDirectoryStream(directory.toPath()).use { stream ->
                stream.toList()
            }
            for (entry in entries) {
                currentCoroutineContext().ensureActive()
                budget.includeEntry()
                val attributes = Files.readAttributes(
                    entry,
                    BasicFileAttributes::class.java,
                    LinkOption.NOFOLLOW_LINKS,
                )
                if (attributes.isSymbolicLink) continue
                val file = entry.toFile()
                if (!isSafeWorkspacePath(canonicalRoot, file)) continue
                if (attributes.isDirectory) {
                    directories.add(file)
                } else if (attributes.isRegularFile) {
                    val relativePath = rootPath.relativize(entry).toString().replace(File.separatorChar, '/')
                    if (!isSafeRelativePath(relativePath)) continue
                    val hashed = openInternalInput(file).use { sha256(it) }
                    budget.includeFile(hashed.sizeBytes)
                    files[relativePath] = InternalFile(file, hashed.sha256, hashed.sizeBytes)
                }
            }
        }
        return files
    }

    private suspend fun copyExternalToInternal(
        source: ExternalFile,
        workspace: File,
        relativePath: String,
    ) {
        require(isSafeRelativePath(relativePath))
        val root = requireCanonicalWorkspaceRoot(workspace)
        val target = File(root, relativePath)
        require(isSafeWorkspacePath(root, target)) { "copy escaped project workspace" }
        val requiredSpace = source.sizeBytes + MIN_FREE_SPACE_BYTES
        require(root.usableSpace >= requiredSpace) {
            "not enough free space to import $relativePath"
        }
        Files.createDirectories(checkNotNull(target.parentFile).toPath())
        require(requireCanonicalWorkspaceRoot(workspace) == root) {
            "project workspace changed during sync"
        }
        require(isSafeWorkspacePath(root, target)) { "copy target changed during sync" }
        val temp = File(target.parentFile, ".${target.name}.${UUID.randomUUID()}.tmp")
        try {
            resolver.openInputStream(source.uri)?.use { input ->
                FileChannel.open(
                    temp.toPath(),
                    StandardOpenOption.CREATE_NEW,
                    StandardOpenOption.WRITE,
                    LinkOption.NOFOLLOW_LINKS,
                ).use { channel ->
                    Channels.newOutputStream(channel).use { output ->
                        val copied = copyAndHashCancellable(input, output)
                        check(
                            copied.sizeBytes == source.sizeBytes &&
                                copied.sha256 == source.sha256,
                        ) {
                            "external file changed while importing $relativePath"
                        }
                        channel.force(true)
                    }
                }
            } ?: throw IllegalStateException("cannot open external file: $relativePath")
            require(isSafeWorkspacePath(root, target)) { "copy target changed during sync" }
            FilesMove.replace(temp, target)
        } finally {
            Files.deleteIfExists(temp.toPath())
        }
    }

    private suspend fun writeExternal(
        treeUri: Uri,
        relativePath: String,
        source: InternalFile,
        workspaceRoot: File,
    ) {
        require(isSafeRelativePath(relativePath))
        val canonicalWorkspaceRoot = requireCanonicalWorkspaceRoot(workspaceRoot)
        require(isSafeWorkspacePath(canonicalWorkspaceRoot, source.file)) {
            "source escaped project workspace"
        }
        val segments = relativePath.split('/')
        var parentUri = DocumentsContract.buildDocumentUriUsingTree(
            treeUri,
            DocumentsContract.getTreeDocumentId(treeUri),
        )
        for (name in segments.dropLast(1)) {
            currentCoroutineContext().ensureActive()
            parentUri = findChild(treeUri, parentUri, name, expectDirectory = true)
                ?: DocumentsContract.createDocument(
                    resolver,
                    parentUri,
                    DocumentsContract.Document.MIME_TYPE_DIR,
                    name,
                )
                ?: throw IllegalStateException("cannot create external directory: $name")
        }
        val name = segments.last()
        val target = findChild(treeUri, parentUri, name, expectDirectory = false)
        require(requireCanonicalWorkspaceRoot(workspaceRoot) == canonicalWorkspaceRoot) {
            "project workspace changed during sync"
        }
        require(isSafeWorkspacePath(canonicalWorkspaceRoot, source.file)) {
            "source changed during sync"
        }
        commitExternalDocument(
            backend = documentBackend,
            parentUri = parentUri,
            targetUri = target,
            displayName = name,
            mimeType = "application/octet-stream",
            expectedSha256 = source.sha256,
            source = { openInternalInput(source.file) },
        )
    }

    private suspend fun recoverExternalTransactions(treeUri: Uri) {
        val rootId = DocumentsContract.getTreeDocumentId(treeUri)
        val rootUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, rootId)
        recoverExternalDirectory(treeUri, rootUri, mutableSetOf(), depth = 0)
    }

    private suspend fun recoverExternalDirectory(
        treeUri: Uri,
        directoryUri: Uri,
        visitedDocumentIds: MutableSet<String>,
        depth: Int,
    ) {
        currentCoroutineContext().ensureActive()
        require(depth <= 64) { "external directory nesting exceeds the supported limit" }
        val parentId = DocumentsContract.getDocumentId(directoryUri)
        if (!visitedDocumentIds.add(parentId)) return
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, parentId)
        val children = queryChildren(childrenUri)
        for (directory in children.filter {
            it.mimeType == DocumentsContract.Document.MIME_TYPE_DIR &&
                isSafeSegment(it.displayName) &&
                !isLingxiSyncArtifact(it.displayName)
        }) {
            recoverExternalDirectory(
                treeUri,
                DocumentsContract.buildDocumentUriUsingTree(treeUri, directory.documentId),
                visitedDocumentIds,
                depth + 1,
            )
        }

        val filesByName = children
            .filter { it.mimeType != DocumentsContract.Document.MIME_TYPE_DIR }
            .associateBy { it.displayName }
        val backups = children.mapNotNull { child ->
            if (child.mimeType == DocumentsContract.Document.MIME_TYPE_DIR) {
                null
            } else {
                syncArtifactOriginalName(child.displayName, SYNC_BACKUP_MARKER)
                    ?.let { originalName -> originalName to child }
            }
        }.groupBy({ it.first }, { it.second })
        for ((originalName, originalBackups) in backups) {
            currentCoroutineContext().ensureActive()
            if (filesByName.containsKey(originalName)) {
                originalBackups.forEach { backup ->
                    documentBackend.deleteDocument(
                        DocumentsContract.buildDocumentUriUsingTree(treeUri, backup.documentId),
                    )
                }
            } else {
                val restore = originalBackups.first()
                documentBackend.renameDocument(
                    DocumentsContract.buildDocumentUriUsingTree(treeUri, restore.documentId),
                    originalName,
                )
                originalBackups.drop(1).forEach { backup ->
                    documentBackend.deleteDocument(
                        DocumentsContract.buildDocumentUriUsingTree(treeUri, backup.documentId),
                    )
                }
            }
        }
        for (temp in children.filter {
            it.mimeType != DocumentsContract.Document.MIME_TYPE_DIR &&
                syncArtifactOriginalName(it.displayName, SYNC_TEMP_MARKER) != null
        }) {
            currentCoroutineContext().ensureActive()
            documentBackend.deleteDocument(
                DocumentsContract.buildDocumentUriUsingTree(treeUri, temp.documentId),
            )
        }
    }

    private fun findChild(
        treeUri: Uri,
        parentUri: Uri,
        displayName: String,
        expectDirectory: Boolean,
    ): Uri? {
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(
            treeUri,
            DocumentsContract.getDocumentId(parentUri),
        )
        val row = queryChildren(childrenUri).firstOrNull { it.displayName == displayName } ?: return null
        val isDirectory = row.mimeType == DocumentsContract.Document.MIME_TYPE_DIR
        require(isDirectory == expectDirectory) {
            "external path type conflicts with $displayName"
        }
        return DocumentsContract.buildDocumentUriUsingTree(treeUri, row.documentId)
    }

    private fun openInternalInput(file: File): InputStream =
        Channels.newInputStream(
            Files.newByteChannel(
                file.toPath(),
                setOf(StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS),
            ),
        )

    private data class DocumentRow(
        val documentId: String,
        val displayName: String,
        val mimeType: String,
    )

    private data class ExternalFile(
        val uri: Uri,
        val sha256: String,
        val sizeBytes: Long,
    )

    private data class InternalFile(
        val file: File,
        val sha256: String,
        val sizeBytes: Long,
    )

    private class SyncBudget {
        private var entries = 0
        private var totalBytes = 0L

        fun includeEntry() {
            entries++
            require(entries <= MAX_SYNC_ENTRIES) {
                "project contains more than $MAX_SYNC_ENTRIES entries"
            }
        }

        fun includeFile(sizeBytes: Long) {
            require(sizeBytes <= MAX_SYNC_FILE_BYTES) {
                "project contains a file larger than the supported sync limit"
            }
            totalBytes = Math.addExact(totalBytes, sizeBytes)
            require(totalBytes <= MAX_SYNC_TOTAL_BYTES) {
                "project exceeds the supported sync size"
            }
        }
    }
}

private class ContentResolverSafDocumentBackend(
    private val resolver: ContentResolver,
) : SafDocumentBackend {
    override fun createDocument(parentUri: Uri, mimeType: String, displayName: String): Uri =
        DocumentsContract.createDocument(resolver, parentUri, mimeType, displayName)
            ?: throw IllegalStateException("document provider could not create $displayName")

    override fun openInput(uri: Uri): InputStream =
        resolver.openInputStream(uri)
            ?: throw IllegalStateException("document provider could not read $uri")

    override fun openOutput(uri: Uri): OutputStream =
        resolver.openOutputStream(uri, "w")
            ?: throw IllegalStateException("document provider could not write $uri")

    override fun renameDocument(uri: Uri, displayName: String): Uri =
        DocumentsContract.renameDocument(resolver, uri, displayName)
            ?: throw IllegalStateException("document provider could not rename $displayName")

    override fun deleteDocument(uri: Uri) {
        check(DocumentsContract.deleteDocument(resolver, uri)) {
            "document provider could not delete $uri"
        }
    }
}

private data class HashedInput(
    val sha256: String,
    val sizeBytes: Long,
)

private suspend fun sha256(input: InputStream): HashedInput {
    val digest = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    var sizeBytes = 0L
    while (true) {
        currentCoroutineContext().ensureActive()
        val read = input.read(buffer)
        if (read < 0) break
        if (read > 0) {
            sizeBytes = Math.addExact(sizeBytes, read.toLong())
            require(sizeBytes <= MAX_SYNC_FILE_BYTES) {
                "project contains a file larger than the supported sync limit"
            }
            digest.update(buffer, 0, read)
        }
    }
    return HashedInput(
        sha256 = digest.digest().joinToString("") { "%02x".format(it) },
        sizeBytes = sizeBytes,
    )
}

private suspend fun copyCancellable(input: InputStream, output: OutputStream): Long {
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    var copied = 0L
    while (true) {
        currentCoroutineContext().ensureActive()
        val read = input.read(buffer)
        if (read < 0) break
        if (read > 0) {
            copied = Math.addExact(copied, read.toLong())
            require(copied <= MAX_SYNC_FILE_BYTES) {
                "project contains a file larger than the supported sync limit"
            }
            output.write(buffer, 0, read)
        }
    }
    output.flush()
    return copied
}

private suspend fun copyAndHashCancellable(
    input: InputStream,
    output: OutputStream,
): HashedInput {
    val digest = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    var copied = 0L
    while (true) {
        currentCoroutineContext().ensureActive()
        val read = input.read(buffer)
        if (read < 0) break
        if (read > 0) {
            copied = Math.addExact(copied, read.toLong())
            require(copied <= MAX_SYNC_FILE_BYTES) {
                "project contains a file larger than the supported sync limit"
            }
            digest.update(buffer, 0, read)
            output.write(buffer, 0, read)
        }
    }
    output.flush()
    return HashedInput(
        sha256 = digest.digest().joinToString("") { "%02x".format(it) },
        sizeBytes = copied,
    )
}

private object FilesMove {
    fun replace(source: File, target: File) {
        try {
            Files.move(
                source.toPath(),
                target.toPath(),
                StandardCopyOption.ATOMIC_MOVE,
                StandardCopyOption.REPLACE_EXISTING,
            )
        } catch (_: java.nio.file.AtomicMoveNotSupportedException) {
            Files.move(
                source.toPath(),
                target.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }
}

private fun Cursor.string(column: Int): String =
    getString(column) ?: throw IllegalStateException("document provider returned a null column")

private fun isSafeSegment(value: String): Boolean =
    value.isNotBlank() && value != "." && value != ".." && '/' !in value && '\\' !in value

private fun requireCanonicalWorkspaceRoot(root: File): File {
    val absolutePath = root.absoluteFile.toPath().normalize()
    require(!Files.isSymbolicLink(absolutePath)) { "project workspace cannot be a symbolic link" }
    val canonicalRoot = root.canonicalFile
    require(canonicalRoot.toPath() == absolutePath) {
        "project workspace escaped its managed location"
    }
    return canonicalRoot
}

/**
 * Reject every symbolic-link hop before a SAF sync reads or writes a workspace
 * entry. Guest-created links must never turn "sync this Project" into access to
 * another Project, `.lingxi`, provider state, or any other host path.
 *
 * The candidate may not exist yet (an import target), so both the lexical path
 * and every existing ancestor are checked before the canonical containment
 * check.
 */
internal fun isSafeWorkspacePath(root: File, candidate: File): Boolean {
    val absoluteRootPath = root.absoluteFile.toPath().normalize()
    if (Files.isSymbolicLink(absoluteRootPath)) return false
    val canonicalRoot = runCatching { root.canonicalFile }.getOrNull() ?: return false
    val rootPath = canonicalRoot.toPath()
    if (rootPath != absoluteRootPath) return false
    val candidatePath = candidate.absoluteFile.toPath().normalize()
    if (candidatePath == rootPath || !candidatePath.startsWith(rootPath)) return false

    var current: File? = candidate.absoluteFile
    while (current != null && current.toPath().normalize() != rootPath) {
        if (Files.isSymbolicLink(current.toPath())) return false
        current = current.parentFile
    }
    if (current == null) return false

    val canonicalCandidate = runCatching { candidate.canonicalFile.toPath() }.getOrNull()
        ?: return false
    return canonicalCandidate.startsWith(rootPath) && canonicalCandidate != rootPath
}
