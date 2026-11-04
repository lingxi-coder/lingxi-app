package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.R
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.BasicFileAttributes
import java.util.UUID

/**
 * Resolves a localized string for [ProjectRepository], which runs outside a
 * `@Composable` body and is constructed directly (with no `Context`) by
 * [ProjectRepositoryInstrumentedTest]. [fallback] is always the exact zh-Hans
 * base-locale copy for [id]; [DefaultProjectStrings] (the parameter default)
 * returns it verbatim (`String.format`-ed when [args] are present), so that
 * instrumented test keeps asserting the same literal Chinese copy without being
 * touched. The production resolver ([projectStrings]) ignores [fallback] and
 * resolves the real localized text through [Context.getString].
 */
fun interface ProjectStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultProjectStrings = ProjectStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun projectStrings(context: Context): ProjectStrings =
    ProjectStrings { id, _, args -> context.getString(id, *args) }

internal const val PROJECTS_INDEX_FILE = "index.json"
internal const val PROJECT_MANIFEST_FILE = "project.json"
internal const val PROJECT_SESSION_INDEX_FILE = "session-index.json"
internal const val PROJECT_SYNC_BASELINE_FILE = "sync-baseline.json"
internal const val PROJECT_WORKSPACE_DIR = "workspace"
internal const val GLOBAL_SESSION_INDEX_FILE = "global-session-index.json"

fun interface ProjectAtomicWriter {
    fun write(target: File, text: String, validate: (String) -> Unit)

    /** Stage before taking a caller's short publication lock, when supported. */
    fun writeGuarded(target: File, text: String, validate: (String) -> Unit, publish: (() -> Unit) -> Unit) {
        publish { write(target, text, validate) }
    }
}

internal class DefaultProjectAtomicWriter : ProjectAtomicWriter {
    private data class TargetSnapshot(val fileKey: Any, val bytes: ByteArray, val modified: java.nio.file.attribute.FileTime)

    private fun snapshot(target: File): TargetSnapshot? {
        val path = target.toPath()
        val before = try {
            Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        } catch (_: NoSuchFileException) {
            return null
        }
        check(before.isRegularFile && !before.isSymbolicLink) { "Storage target is not a regular file: ${target.name}" }
        val fileKey = checkNotNull(before.fileKey()) { "Storage target identity is unavailable: ${target.name}" }
        val bytes = Files.newInputStream(path, StandardOpenOption.READ, LinkOption.NOFOLLOW_LINKS).use { it.readBytes() }
        val after = Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        check(after.isRegularFile && !after.isSymbolicLink && fileKey == after.fileKey() &&
            before.size() == after.size() && after.size() == bytes.size.toLong() &&
            before.lastModifiedTime() == after.lastModifiedTime()) {
            "Storage target changed while reading: ${target.name}"
        }
        return TargetSnapshot(fileKey, bytes, after.lastModifiedTime())
    }

    override fun write(target: File, text: String, validate: (String) -> Unit) =
        writeGuarded(target, text, validate) { it() }

    override fun writeGuarded(target: File, text: String, validate: (String) -> Unit, publish: (() -> Unit) -> Unit) {
        target.parentFile?.mkdirs()
        val previous = snapshot(target)
        val temp = File(target.parentFile, ".${target.name}.${UUID.randomUUID()}.tmp")
        try {
            FileOutputStream(temp).use { output ->
                output.write(text.toByteArray(Charsets.UTF_8))
                output.fd.sync()
            }
            val staged = temp.readText(Charsets.UTF_8)
            validate(staged)
            val current = snapshot(target)
            check((previous == null && current == null) || (previous != null && current != null &&
                previous.fileKey == current.fileKey && previous.bytes.contentEquals(current.bytes))) {
                "Storage target changed while writing: ${target.name}"
            }
            publish {
                // The byte comparison above happens outside the publication lock.
                // Recheck identity immediately before rename. As with the other
                // guarded writes, this is not an inode-CAS against an unrelated
                // process between the final check and rename.
                val latest = try {
                    Files.readAttributes(target.toPath(), BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
                } catch (_: NoSuchFileException) {
                    null
                }
                check((current == null && latest == null) || (current != null && latest != null &&
                    latest.isRegularFile && !latest.isSymbolicLink && current.fileKey == latest.fileKey() &&
                    current.bytes.size.toLong() == latest.size() && current.modified == latest.lastModifiedTime())) {
                    "Storage target changed before publication: ${target.name}"
                }
                try {
                    Files.move(
                        temp.toPath(),
                        target.toPath(),
                        StandardCopyOption.ATOMIC_MOVE,
                        StandardCopyOption.REPLACE_EXISTING,
                    )
                } catch (_: AtomicMoveNotSupportedException) {
                    Files.move(
                        temp.toPath(),
                        target.toPath(),
                        StandardCopyOption.REPLACE_EXISTING,
                    )
                }
            }
        } finally {
            temp.delete()
        }
    }
}

internal class ProjectRepository(
    private val projectsRoot: File,
    private val now: () -> Long = System::currentTimeMillis,
    private val newId: () -> String = { UUID.randomUUID().toString().lowercase() },
    private val writer: ProjectAtomicWriter = DefaultProjectAtomicWriter(),
    private val strings: ProjectStrings = DefaultProjectStrings,
    private val userHome: File?,
    private val projectsGuestRoot: String,
    private val userHomeGuestPath: String,
    private val ownerStateFile: File? = null,
) {
    // Repository operations are serialized by ProjectStore's repository mutex.
    // A caller may supply a short commit guard without holding it while fsync runs.
    private var writePublication: ((File, () -> Unit) -> Unit)? = null

    internal fun <T> withWritePublication(guard: (File, () -> Unit) -> Unit, operation: () -> T): T {
        check(writePublication == null)
        writePublication = guard
        return try { operation() } finally { writePublication = null }
    }
    init {
        projectsRoot.mkdirs()
    }

    fun load(): ProjectStoreState {
        projectsRoot.mkdirs()
        val projectsIndexPath = File(projectsRoot, PROJECTS_INDEX_FILE)
        val projectsIndexExists = projectsIndexPath.exists() || Files.isSymbolicLink(projectsIndexPath.toPath())
        val index = try { readIndex() } catch (error: Exception) {
            throw ProjectCatalogUnavailableException(listOf("projects-index"), "Project index is unavailable", error)
        }
        val projectDirectories = projectsRoot.listFiles()
            .orEmpty()
            .filter { it.isDirectory && isLowercaseUuid(it.name) }
        val loaded = projectDirectories.map { projectDir ->
            projectDir to runCatching { loadProject(projectDir.name) }
        }
        val discovered = loaded.mapNotNull { it.second.getOrNull() }
            .sortedByDescending { it.record.updatedAtEpochMillis }
        val directoryIds = projectDirectories.mapTo(mutableSetOf()) { it.name }
        val missing = index.projectIds.filter { it !in directoryIds }
        val unindexed = directoryIds.filter { it !in index.projectIds }
        if (missing.isNotEmpty() || unindexed.isNotEmpty()) {
            val scopes = (missing + unindexed).map { "project.$it" }.sorted()
            throw ProjectCatalogUnavailableException(scopes, "Project session index is unavailable for ${scopes.joinToString()}")
        }
        val corruptCount = loaded.count { it.second.isFailure }
        val discoveredIds = discovered.map { it.record.id }
        val active = index.activeProjectId?.takeIf(discoveredIds::contains)
        val globalIndex = File(projectsRoot, GLOBAL_SESSION_INDEX_FILE)
        val globalIndexExists = globalIndex.exists() || Files.isSymbolicLink(globalIndex.toPath())
        val newStore = !projectsIndexExists && !globalIndexExists && projectDirectories.isEmpty()
        if (newStore && ownerStateFile?.let { it.exists() || Files.isSymbolicLink(it.toPath()) } == true) {
            throw ProjectCatalogUnavailableException(listOf("global", "projects-index"),
                "Session indexes are missing while conversation state exists")
        }
        if (!projectsIndexExists && !newStore) {
            throw ProjectCatalogUnavailableException(listOf("projects-index"), "Project index is missing")
        }
        val globalSessions = try {
            if (newStore) emptyList() else readSessions(globalIndex)
        } catch (error: Exception) {
            throw ProjectCatalogUnavailableException(listOf("global"), "Global session index is unavailable", error)
        }
        requireUniqueSessionOwners(globalSessions, readKnownProjectSessions())
        if (newStore) {
            writeSessionFile(globalIndex, emptyList())
            writeIndex(ProjectIndex())
        }
        return ProjectStoreState(
            projects = discovered,
            activeProjectId = active,
            globalSessions = globalSessions,
            loading = false,
            errorMessage = if (corruptCount > 0) {
                strings.resolve(
                    R.string.project_corrupt_projects_message_fmt,
                    "%1\$d 个项目数据损坏或路径异常，已隔离；其他项目仍可使用。",
                    corruptCount,
                )
            } else {
                null
            },
        )
    }

    fun createInternal(name: String, mode: SessionMode): ProjectSnapshot =
        createProject(name, ProjectStorageKind.Internal, null, null, mode)

    fun createAuthoringPlaceholder(name: String, mode: SessionMode, projectId: String, authoringIntentId: String): ProjectSnapshot {
        com.lingxi.code.apps.requireCodeAuthoring(mode)
        validateProjectId(authoringIntentId)
        return createProject(name, ProjectStorageKind.Internal, null, null, mode, projectId, null, authoringIntentId)
    }

    /** Register an existing owned source directory without copying or relocating it. */
    fun registerAuthoringWorkspace(name: String, guestDirectory: String, mode: SessionMode,
        projectId: String, authoringIntentId: String): ProjectSnapshot {
        com.lingxi.code.apps.requireCodeAuthoring(mode)
        validateProjectId(projectId)
        validateProjectId(authoringIntentId)
        workspaceForDirectory("authoring", guestDirectory)
        val projects = load().projects
        projects.firstOrNull { it.record.directory == guestDirectory && it.record.id != projectId }?.let {
            error("Authoring source belongs to another project")
        }
        projects.firstOrNull { it.record.id == projectId }?.let {
            val placeholder = "$projectsGuestRoot/$projectId/workspace"
            require(it.record.directory == guestDirectory || (it.record.directory == placeholder && it.record.authoringIntentId == authoringIntentId)) {
                "Authoring project changed"
            }
            return updateProject(it.record.copy(directory = guestDirectory, activeDirectory = guestDirectory,
                authoringIntentId = authoringIntentId, visibleModes = it.record.visibleModes + mode))
        }
        return createProject(name, ProjectStorageKind.Internal, null, null, mode, projectId, guestDirectory, authoringIntentId)
    }

    fun createSafMirror(
        name: String,
        sourceTreeUri: String,
        sourceDisplayName: String?,
        mode: SessionMode,
    ): ProjectSnapshot {
        require(sourceTreeUri.startsWith("content://")) { "SAF project must use a content:// URI" }
        return createProject(name, ProjectStorageKind.SafMirror, sourceTreeUri, sourceDisplayName, mode)
    }

    private fun createProject(
        name: String,
        storageKind: ProjectStorageKind,
        sourceTreeUri: String?,
        sourceDisplayName: String?,
        mode: SessionMode,
        requestedProjectId: String? = null,
        initialDirectory: String? = null,
        authoringIntentId: String? = null,
    ): ProjectSnapshot {
        val id = requestedProjectId ?: newId()
        require(isLowercaseUuid(id)) { "project id must be a lowercase UUID" }
        val cleanName = if (authoringIntentId != null) name else name.trim()
        require(cleanName.isNotEmpty()) { "project name cannot be blank" }
        require(if (authoringIntentId != null) cleanName.codePointCount(0, cleanName.length) <= 80 else cleanName.length <= 120) {
            "project name is too long"
        }
        val dir = projectDirectory(id)
        require(!dir.exists()) { "project already exists: $id" }
        load()
        val index = readIndex()
        val workspace = File(dir, PROJECT_WORKSPACE_DIR)
        require(workspace.mkdirs()) { "cannot create project workspace" }
        val timestamp = now()
        val guestWorkspace = "$projectsGuestRoot/$id/workspace"
        val record = ProjectRecord(
            id = id,
            name = cleanName,
            storageKind = storageKind,
            createdAtEpochMillis = timestamp,
            updatedAtEpochMillis = timestamp,
            sourceTreeUri = sourceTreeUri,
            sourceDisplayName = sourceDisplayName,
            directory = initialDirectory ?: guestWorkspace,
            activeDirectory = initialDirectory ?: guestWorkspace,
            visibleModes = setOf(mode),
            authoringIntentId = authoringIntentId,
        )
        try {
            writeProject(record)
            writeSessions(id, emptyList())
            writeBaseline(id, ProjectSyncBaseline())
            writeIndex(
                ProjectIndex(
                    activeProjectId = index.activeProjectId,
                    projectIds = (index.projectIds + id).distinct(),
                ),
            )
        } catch (t: Throwable) {
            dir.deleteRecursively()
            throw t
        }
        return loadProject(id)
    }

    fun addVisibleMode(projectId: String, mode: SessionMode): ProjectSnapshot {
        load()
        val project = loadProject(projectId)
        if (mode !in project.record.visibleModes) {
            writeProject(project.record.copy(visibleModes = project.record.visibleModes + mode))
        }
        return loadProject(projectId)
    }

    fun setActiveProject(projectId: String?): ProjectStoreState {
        if (projectId != null) {
            validateProjectId(projectId)
            require(File(projectDirectory(projectId), PROJECT_MANIFEST_FILE).isFile) {
                "project does not exist: $projectId"
            }
        }
        val current = load()
        require(projectId == null || current.projects.any { it.record.id == projectId }) {
            "Project is unavailable: $projectId"
        }
        // Isolated or temporarily missing projects still belong to the index.
        // Changing the active project must not erase those entries.
        val index = readIndex()
        writeIndex(ProjectIndex(projectId, index.projectIds))
        return load()
    }

    fun updateProject(record: ProjectRecord): ProjectSnapshot {
        load()
        validateProjectId(record.id)
        require(projectDirectory(record.id).isDirectory) { "project does not exist: ${record.id}" }
        val normalized = record.copy(updatedAtEpochMillis = now())
        writeProject(normalized)
        return loadProject(record.id)
    }

    fun updateSessions(
        projectId: String?,
        sessions: List<ProjectSessionSummary>,
        preserveSessionIds: Set<String> = emptySet(),
    ): ProjectStoreState {
        val state = load()
        val snapshot = projectId?.let { loadProject(it) }
        val existing = snapshot?.sessions ?: state.globalSessions
        val canonical = sessions.map { it.copy(sessionId = canonicalSessionId(it.sessionId),
            transcriptPath = it.transcriptPath?.takeIf(String::isNotBlank)) }
        require(canonical.all { it.sessionId.isNotBlank() }) { "Session catalog contains an empty id" }
        require(canonical.groupBy { it.sessionId }.values.all { rows -> rows.map { it.mode }.distinct().size == 1 }) {
            "Session catalog contains conflicting modes"
        }
        require(canonical.groupBy { it.sessionId }.values.all { rows -> rows.mapNotNull { it.transcriptPath }.distinct().size <= 1 }) {
            "Session catalog contains conflicting transcript identities"
        }
        val incoming = canonical.groupBy { it.sessionId }.values
            .map { rows -> rows.first().copy(transcriptPath = rows.firstNotNullOfOrNull { it.transcriptPath }) }
            .map { row ->
                val cached = existing.firstOrNull { it.sessionId == row.sessionId }
                require(cached == null || cached.mode == row.mode) { "A session's mode cannot change" }
                require(cached?.transcriptPath == null || row.transcriptPath == null || cached.transcriptPath == row.transcriptPath) {
                    "A session's transcript identity cannot change"
                }
                if (cached?.pendingCatalogConfirmation == true && row.messageCount < cached.messageCount) {
                    return@map cached
                }
                row.copy(
                    isArchived = cached?.isArchived ?: false,
                    workspaceDirectory = row.workspaceDirectory ?: cached?.workspaceDirectory,
                    originalProjectDirectory = row.originalProjectDirectory ?: cached?.originalProjectDirectory,
                    transcriptPath = row.transcriptPath ?: cached?.transcriptPath,
                    pendingCatalogConfirmation = false,
                )
            }
        val incomingIds = incoming.mapTo(mutableSetOf()) { it.sessionId }
        // A listing is scoped to the engine's current directory. Keep the
        // selected exact transcript identity even when another directory's
        // listing omits it; a missing file is reported on restore, never
        // replaced by another transcript with the same UUID.
        val normalized = (incoming + existing.filter {
            it.sessionId !in incomingIds && (it.sessionId in preserveSessionIds || it.pendingCatalogConfirmation || it.isArchived || it.transcriptPath != null ||
                (snapshot != null && it.workspaceDirectory != null && it.workspaceDirectory != snapshot.record.activeDirectory))
        }).sortedByDescending { it.updatedAtEpochMillis }
        requireNoOtherSessionOwner(state, projectId, normalized)
        if (projectId == null) {
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), normalized)
        } else {
            validateProjectId(projectId)
            writeSessions(projectId, normalized)
            val snapshot = loadProject(projectId)
            val lastActive = snapshot.record.lastActiveSessionId?.takeIf { id ->
                normalized.any { it.sessionId == id }
            }
            writeProject(snapshot.record.copy(
                lastActiveSessionId = lastActive,
                visibleModes = snapshot.record.visibleModes + normalized.map { it.mode },
            ))
        }
        return load()
    }

    /**
     * Persist a freshly confirmed Engine session and its exact transcript source.
     *
     * The engine's SessionList is file-backed, so a SessionStarted event can
     * precede the new anchored session appearing in that listing. Keeping this
     * small, idempotent row makes the Project or global conversation recoverable
     * immediately. Omitted provisional rows survive stale lists until the
     * engine includes the id, at which point catalog data replaces the row.
     */
    fun recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: SessionMode,
        initialMessageCount: Int = 0,
        transcriptPath: String? = null,
        markActive: Boolean = true,
    ): ProjectStoreState {
        val canonicalId = canonicalSessionId(sessionId)
        require(canonicalId.isNotBlank()) { "session id cannot be blank" }
        val state = load()
        val snapshot = projectId?.let {
            validateProjectId(it)
            loadProject(it)
        }
        val sessions = snapshot?.sessions ?: state.globalSessions
        val timestamp = now()
        val existing = sessions.firstOrNull { it.sessionId == canonicalId }
        require(existing == null || existing.mode == mode) { "A session's mode cannot change" }
        require(existing?.transcriptPath == null || transcriptPath.isNullOrBlank() || existing.transcriptPath == transcriptPath) {
            "A session's transcript identity cannot change"
        }
        val started = ProjectSessionSummary(
            sessionId = canonicalId,
            title = title.ifBlank {
                existing?.title ?: strings.resolve(R.string.chat_new_conversation, "新对话")
            },
            messageCount = maxOf(existing?.messageCount ?: 0, initialMessageCount.coerceAtLeast(0)),
            relativeTime = strings.resolve(R.string.project_relative_time_just_now, "刚刚"),
            updatedAtEpochMillis = timestamp,
            mode = existing?.mode ?: mode,
            isArchived = existing?.isArchived ?: false,
            pendingCatalogConfirmation = (existing?.pendingCatalogConfirmation ?: true) ||
                initialMessageCount > (existing?.messageCount ?: 0),
            workspaceDirectory = existing?.workspaceDirectory ?: snapshot?.record?.activeDirectory,
            originalProjectDirectory = existing?.originalProjectDirectory ?: snapshot?.record?.activeDirectory,
            transcriptPath = transcriptPath?.takeIf(String::isNotBlank) ?: existing?.transcriptPath,
        )
        val updated = listOf(started) + sessions.filterNot { it.sessionId == canonicalId }
        requireNoOtherSessionOwner(state, projectId, updated)
        if (projectId == null) {
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), updated)
        } else {
            writeSessions(projectId, updated)
            val record = requireNotNull(snapshot).record
            writeProject(
                record.copy(
                    updatedAtEpochMillis = timestamp,
                    lastActiveSessionId = if (markActive) canonicalId else record.lastActiveSessionId,
                    visibleModes = record.visibleModes + mode,
                ),
            )
        }
        return load()
    }

    fun setSessionArchived(projectId: String?, sessionId: String, archived: Boolean): ProjectStoreState {
        val canonicalId = canonicalSessionId(sessionId)
        val state = load()
        val snapshot = projectId?.let { loadProject(it) }
        val sessions = snapshot?.sessions ?: state.globalSessions
        require(sessions.any { it.sessionId == canonicalId }) { "session is not indexed" }
        val updated = sessions.map {
            if (it.sessionId == canonicalId) it.copy(
                isArchived = archived,
                pendingCatalogConfirmation = it.pendingCatalogConfirmation || (it.isArchived && !archived),
            ) else it
        }
        if (projectId == null) {
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), updated)
        } else {
            writeSessions(projectId, updated)
        }
        return load()
    }

    fun markActiveSession(projectId: String, sessionId: String): ProjectStoreState {
        load()
        validateProjectId(projectId)
        val snapshot = loadProject(projectId)
        val known = snapshot.sessions.any { it.sessionId == sessionId }
        require(known) { "session is not indexed by project" }
        writeProject(snapshot.record.copy(lastActiveSessionId = sessionId))
        return load()
    }

    fun readBaseline(projectId: String): ProjectSyncBaseline {
        validateProjectId(projectId)
        val file = File(projectDirectory(projectId), PROJECT_SYNC_BASELINE_FILE)
        if (!file.isFile) return ProjectSyncBaseline()
        return decodeBaseline(file.readText(Charsets.UTF_8))
    }

    fun writeBaseline(projectId: String, baseline: ProjectSyncBaseline) {
        validateProjectId(projectId)
        val target = File(projectDirectory(projectId), PROJECT_SYNC_BASELINE_FILE)
        writeJson(target, encodeBaseline(baseline), ::decodeBaseline)
    }

    fun recordWorkspace(projectId: String?, state: SessionWorkspaceState, expectedPreviousPath: String? = null,
        checkPreviousPath: Boolean = false, updateProjectDirectory: Boolean = true): ProjectStoreState {
        val current = load()
        if (projectId == null) {
            if (current.globalSessions.none { canonicalSessionId(it.sessionId) == canonicalSessionId(state.sessionId) }) return current
            val existing = current.globalSessions.single { canonicalSessionId(it.sessionId) == canonicalSessionId(state.sessionId) }
            if (checkPreviousPath && existing.transcriptPath != expectedPreviousPath) return current
            require(existing.transcriptPath == null || state.transcriptPath.isBlank() || existing.transcriptPath == state.transcriptPath ||
                existing.transcriptPath == expectedPreviousPath) {
                "A session's transcript identity cannot change"
            }
            val sessions = current.globalSessions.map { session ->
                if (canonicalSessionId(session.sessionId) == canonicalSessionId(state.sessionId)) session.copy(
                    workspaceDirectory = state.cwd,
                    originalProjectDirectory = state.projectRoot,
                    transcriptPath = state.transcriptPath.takeIf(String::isNotBlank) ?: session.transcriptPath,
                ) else session
            }
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), sessions)
            return load()
        }
        val snapshot = loadProject(projectId)
        val existing = snapshot.sessions.firstOrNull { canonicalSessionId(it.sessionId) == canonicalSessionId(state.sessionId) }
        if (checkPreviousPath && existing?.transcriptPath != expectedPreviousPath) return current
        require(existing?.transcriptPath == null || state.transcriptPath.isBlank() || existing.transcriptPath == state.transcriptPath ||
            existing.transcriptPath == expectedPreviousPath) {
            "A session's transcript identity cannot change"
        }
        workspaceForDirectory(projectId, state.cwd)
        state.additionalDirectories.forEach { workspaceForDirectory(projectId, it, requireExisting = false) }
        if (updateProjectDirectory) {
            writeProject(snapshot.record.copy(
                activeDirectory = state.cwd,
                additionalDirectories = (snapshot.record.additionalDirectories + state.additionalDirectories + state.cwd)
                    .filter { it != snapshot.record.directory }.distinct(),
            ))
        }
        writeSessions(projectId, snapshot.sessions.map { session ->
            if (canonicalSessionId(session.sessionId) == canonicalSessionId(state.sessionId)) session.copy(
                workspaceDirectory = state.cwd,
                originalProjectDirectory = state.projectRoot,
                transcriptPath = state.transcriptPath.takeIf(String::isNotBlank) ?: session.transcriptPath,
            ) else session
        })
        return load()
    }

    private fun workspaceForDirectory(projectId: String, guestPath: String, requireExisting: Boolean = true): ProjectWorkspace {
        val projectsGuest = projectsGuestRoot
        val homeGuest = userHomeGuestPath
        require((guestPath == homeGuest || guestPath.startsWith("$homeGuest/")) && guestPath.split('/').none { it == ".." || it == "." }) { "workspace must be a normalized Linux user directory" }
        val (root, relative) = if (guestPath == projectsGuest || guestPath.startsWith("$projectsGuest/")) {
            projectsRoot to guestPath.removePrefix(projectsGuest).removePrefix("/")
        } else {
            requireNotNull(userHome) { "Linux user home is not configured" } to guestPath.removePrefix(homeGuest).removePrefix("/")
        }
        val canonicalRoot = root.canonicalFile
        val directory = File(root, relative).canonicalFile
        require((!requireExisting || directory.isDirectory) && directory.toPath().startsWith(canonicalRoot.toPath())) { "workspace escaped Linux user storage or is unavailable" }
        return ProjectWorkspace(projectId, directory.absolutePath, guestPath)
    }

    fun workspace(projectId: String): ProjectWorkspace = loadProjectLayout(projectId).workspace

    fun knownProjectIds(): Set<String> = readIndex().projectIds.toSet()

    fun bothSessionIndexesMissing(): Boolean = listOf(
        File(projectsRoot, PROJECTS_INDEX_FILE), File(projectsRoot, GLOBAL_SESSION_INDEX_FILE),
    ).all { !it.exists() && !Files.isSymbolicLink(it.toPath()) }

    fun project(projectId: String): ProjectSnapshot = loadProject(projectId)

    /** Includes readable indexes of projects whose manifest was isolated. */
    fun indexedSessionOwners(global: List<ProjectSessionSummary>): Map<String, ScopeStateStore.SessionOwner> {
        val projects = readKnownProjectSessions()
        requireUniqueSessionOwners(global, projects)
        return buildMap {
            global.forEach { put(canonicalSessionId(it.sessionId), ScopeStateStore.SessionOwner("global", it.mode)) }
            projects.forEach { (projectId, sessions) -> sessions.forEach {
                put(canonicalSessionId(it.sessionId), ScopeStateStore.SessionOwner("project.$projectId", it.mode))
            } }
        }
    }

    /** Include readable session indexes even when a project's manifest is isolated. */
    private fun readKnownProjectSessions(): List<Pair<String, List<ProjectSessionSummary>>> {
        val directories = projectsRoot.listFiles()
            ?: throw ProjectCatalogUnavailableException(listOf("projects-index"), "Project indexes are unavailable")
        val known = mutableListOf<Pair<String, List<ProjectSessionSummary>>>()
        val unavailable = mutableListOf<String>()
        directories.filter { isLowercaseUuid(it.name) }.forEach { dir ->
            val sessions = if (!dir.isDirectory || Files.isSymbolicLink(dir.toPath())) null else runCatching {
                readSessions(File(dir, PROJECT_SESSION_INDEX_FILE))
            }.getOrNull()
            if (sessions == null) unavailable += "project.${dir.name}"
            else known += dir.name to sessions
        }
        if (unavailable.isNotEmpty()) {
            val scopes = unavailable.sorted()
            throw ProjectCatalogUnavailableException(scopes, "Session index is unavailable for ${scopes.joinToString()}")
        }
        return known
    }

    private fun requireUniqueSessionOwners(
        global: List<ProjectSessionSummary>,
        projects: List<Pair<String, List<ProjectSessionSummary>>>,
    ) {
        val owners = mutableMapOf<String, String>()
        fun add(session: ProjectSessionSummary, owner: String) {
            val id = canonicalSessionId(session.sessionId)
            val previous = owners.putIfAbsent(id, owner)
            if (previous != null && previous != owner) {
                val scopes = listOf(previous, owner).sorted()
                throw ProjectCatalogUnavailableException(scopes, "Session $id has conflicting workspace owners: ${scopes.joinToString()}")
            }
        }
        global.forEach { add(it, "global") }
        projects.forEach { (projectId, sessions) -> sessions.forEach { add(it, "project.$projectId") } }
    }

    private fun requireNoOtherSessionOwner(
        state: ProjectStoreState,
        projectId: String?,
        sessions: List<ProjectSessionSummary>,
    ) {
        val incoming = sessions.mapTo(mutableSetOf()) { canonicalSessionId(it.sessionId) }
        val others = (if (projectId == null) emptyList() else state.globalSessions) +
            readKnownProjectSessions().filter { it.first != projectId }.flatMap { it.second }
        require(others.none { canonicalSessionId(it.sessionId) in incoming }) {
            "Session has a conflicting workspace owner"
        }
    }

    private data class ProjectLayout(
        val record: ProjectRecord,
        val workspace: ProjectWorkspace,
        val sourceWorkspace: ProjectWorkspace,
    )

    private fun loadProject(projectId: String): ProjectSnapshot {
        val layout = loadProjectLayout(projectId)
        return ProjectSnapshot(
            record = layout.record,
            workspace = layout.workspace,
            sourceWorkspace = layout.sourceWorkspace,
            sessions = readSessions(File(projectDirectory(projectId), PROJECT_SESSION_INDEX_FILE)),
        )
    }

    private fun loadProjectLayout(projectId: String): ProjectLayout {
        validateProjectId(projectId)
        val dir = projectDirectory(projectId)
        val canonicalRoot = projectsRoot.canonicalFile
        val canonicalDir = dir.canonicalFile
        require(!Files.isSymbolicLink(dir.toPath()) && canonicalDir.parentFile == canonicalRoot) {
            "project directory escaped the managed projects root"
        }
        val record = decodeProject(File(dir, PROJECT_MANIFEST_FILE).readText(Charsets.UTF_8))
        require(record.id == projectId) { "project manifest id does not match directory" }
        val workspace = File(dir, PROJECT_WORKSPACE_DIR)
        require(
            !Files.isSymbolicLink(workspace.toPath()) &&
                workspace.canonicalFile.parentFile == canonicalDir,
        ) {
            "project workspace escaped its managed project directory"
        }
        return ProjectLayout(
            record = record,
            workspace = workspaceForDirectory(projectId, record.activeDirectory, requireExisting = false).copy(additionalDirectories = record.additionalDirectories),
            sourceWorkspace = ProjectWorkspace(projectId, workspace.canonicalPath),
        )
    }

    private fun writeProject(record: ProjectRecord) {
        validateProjectId(record.id)
        val target = File(projectDirectory(record.id), PROJECT_MANIFEST_FILE)
        writeJson(target, encodeProject(record), ::decodeProject)
    }

    private fun writeSessions(projectId: String, sessions: List<ProjectSessionSummary>) {
        validateProjectId(projectId)
        writeSessionFile(File(projectDirectory(projectId), PROJECT_SESSION_INDEX_FILE), sessions)
    }

    private fun writeSessionFile(target: File, sessions: List<ProjectSessionSummary>) {
        writeJson(target, encodeSessions(sessions), ::decodeSessions)
    }

    private fun readSessions(file: File): List<ProjectSessionSummary> {
        require(!Files.isSymbolicLink(file.toPath()) && file.isFile) { "Session index is missing or not a file: $file" }
        return decodeSessions(file.readText(Charsets.UTF_8))
    }

    private fun readIndex(): ProjectIndex {
        val file = File(projectsRoot, PROJECTS_INDEX_FILE)
        return if (!Files.isSymbolicLink(file.toPath()) && file.isFile) {
            decodeIndex(file.readText(Charsets.UTF_8))
        } else {
            require(!Files.isSymbolicLink(file.toPath()) && !file.exists() && projectsRoot.listFiles().orEmpty().none { it.isDirectory && isLowercaseUuid(it.name) }) {
                "Project index is missing or not a file: $file"
            }
            ProjectIndex()
        }
    }

    private fun writeIndex(index: ProjectIndex) {
        writeJson(File(projectsRoot, PROJECTS_INDEX_FILE), encodeIndex(index), ::decodeIndex)
    }

    private fun <T> writeJson(target: File, value: String, decoder: (String) -> T) {
        val guard = writePublication
        if (guard == null) writer.write(target, value) { decoder(it) }
        else writer.writeGuarded(target, value, { decoder(it) }) { commit -> guard(target, commit) }
    }

    private fun projectDirectory(projectId: String): File = File(projectsRoot, projectId)

    private fun validateProjectId(projectId: String) {
        require(isLowercaseUuid(projectId)) { "project id must be a lowercase UUID" }
    }

    private data class ProjectIndex(
        val activeProjectId: String? = null,
        val projectIds: List<String> = emptyList(),
    )

    private fun encodeIndex(value: ProjectIndex): String = JSONObject()
        .put("version", 1)
        .put("activeProjectId", value.activeProjectId)
        .put("projectIds", JSONArray(value.projectIds))
        .toString(2)

    private fun decodeIndex(text: String): ProjectIndex {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val ids = json.getJSONArray("projectIds").stringList()
        require(ids.all(::isLowercaseUuid) && ids.distinct().size == ids.size)
        val active = json.optNullableString("activeProjectId")
        require(active == null || active in ids)
        return ProjectIndex(active, ids)
    }

    private fun encodeProject(value: ProjectRecord): String = JSONObject()
        .put("version", 1)
        .put("id", value.id)
        .put("name", value.name)
        .put("directory", value.directory)
        .put("activeDirectory", value.activeDirectory)
        .put("additionalDirectories", JSONArray(value.additionalDirectories))
        .put("visibleModes", JSONArray(value.visibleModes.map { it.wireValue }.sorted()))
        .put("storageKind", value.storageKind.wireName)
        .put("createdAtEpochMillis", value.createdAtEpochMillis)
        .put("updatedAtEpochMillis", value.updatedAtEpochMillis)
        .put("sourceTreeUri", value.sourceTreeUri)
        .put("sourceDisplayName", value.sourceDisplayName)
        .put("lastSyncAtEpochMillis", value.lastSyncAtEpochMillis)
        .put("lastActiveSessionId", value.lastActiveSessionId)
        .put("syncState", value.syncState.name)
        .put("authoringIntentId", value.authoringIntentId)
        .toString(2)

    private fun decodeProject(text: String): ProjectRecord {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val id = json.getString("id")
        validateProjectId(id)
        val authoringIntentId = json.optNullableString("authoringIntentId")?.also(::validateProjectId)
        return ProjectRecord(
            id = id,
            name = json.getString("name").also { require(if (authoringIntentId != null) it.isNotEmpty() else it.isNotBlank()) },
            directory = json.getString("directory"),
            activeDirectory = json.getString("activeDirectory"),
            additionalDirectories = json.getJSONArray("additionalDirectories").stringList(),
            visibleModes = json.getJSONArray("visibleModes").stringList()
                .map { com.lingxi.code.model.sessionModeFromWireValue(it) }.toSet(),
            storageKind = ProjectStorageKind.fromWireName(json.getString("storageKind")),
            createdAtEpochMillis = json.getLong("createdAtEpochMillis"),
            updatedAtEpochMillis = json.getLong("updatedAtEpochMillis"),
            sourceTreeUri = json.optNullableString("sourceTreeUri"),
            sourceDisplayName = json.optNullableString("sourceDisplayName"),
            lastSyncAtEpochMillis = json.optNullableLong("lastSyncAtEpochMillis"),
            lastActiveSessionId = json.optNullableString("lastActiveSessionId")
                ?.let(::canonicalSessionId),
            syncState = ProjectSyncState.valueOf(json.getString("syncState")),
            authoringIntentId = authoringIntentId,
        )
    }

    private fun encodeSessions(value: List<ProjectSessionSummary>): String = JSONObject()
        .put("version", 1)
        .put(
            "sessions",
            JSONArray().also { array ->
                value.forEach { session ->
                    array.put(
                        JSONObject()
                            .put("sessionId", session.sessionId)
                            .put("title", session.title)
                            .put("messageCount", session.messageCount)
                            .put("relativeTime", session.relativeTime)
                            .put("updatedAtEpochMillis", session.updatedAtEpochMillis)
                            .put("mode", session.mode.wireValue)
                            .put("workspaceDirectory", session.workspaceDirectory)
                            .put("originalProjectDirectory", session.originalProjectDirectory)
                            .put("transcriptPath", session.transcriptPath)
                            .put("isArchived", session.isArchived)
                            .put("pendingCatalogConfirmation", session.pendingCatalogConfirmation),
                    )
                }
            },
        )
        .toString(2)

    private fun decodeSessions(text: String): List<ProjectSessionSummary> {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val sessions = json.getJSONArray("sessions").objectList().map { row ->
            ProjectSessionSummary(
                isArchived = row.getBoolean("isArchived"),
                pendingCatalogConfirmation = row.getBoolean("pendingCatalogConfirmation"),
                sessionId = canonicalSessionId(row.getString("sessionId")),
                title = row.getString("title"),
                workspaceDirectory = row.optNullableString("workspaceDirectory"),
                originalProjectDirectory = row.optNullableString("originalProjectDirectory"),
                transcriptPath = row.optNullableString("transcriptPath"),
                messageCount = row.getInt("messageCount"),
                relativeTime = row.getString("relativeTime"),
                updatedAtEpochMillis = row.getLong("updatedAtEpochMillis"),
                mode = com.lingxi.code.model.sessionModeFromWireValue(row.getString("mode")),
            )
        }
        require(sessions.all { it.sessionId.isNotBlank() } && sessions.map { it.sessionId }.distinct().size == sessions.size) {
            "Session index contains an empty or duplicate id"
        }
        return sessions
    }

    private fun encodeBaseline(value: ProjectSyncBaseline): String = JSONObject()
        .put("version", 1)
        .put(
            "files",
            JSONArray().also { array ->
                value.files.values.sortedBy { it.relativePath }.forEach { file ->
                    array.put(
                        JSONObject()
                            .put("relativePath", file.relativePath)
                            .put("sha256", file.sha256)
                            .put("sizeBytes", file.sizeBytes),
                    )
                }
            },
        )
        .toString(2)

    private fun decodeBaseline(text: String): ProjectSyncBaseline {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val files = json.getJSONArray("files").objectList().map { file ->
            val relativePath = file.getString("relativePath")
            require(isSafeRelativePath(relativePath)) { "unsafe baseline path" }
            ProjectSyncFile(
                relativePath = relativePath,
                sha256 = file.getString("sha256"),
                sizeBytes = file.getLong("sizeBytes"),
            )
        }
        return ProjectSyncBaseline(files.associateBy { it.relativePath })
    }
}

internal fun isLowercaseUuid(value: String): Boolean =
    value.length == 36 &&
        value == value.lowercase() &&
        runCatching { UUID.fromString(value).toString() == value }.getOrDefault(false)

internal fun isSafeRelativePath(value: String): Boolean {
    if (value.isBlank() || value.startsWith('/') || value.startsWith('\\')) return false
    val parts = value.replace('\\', '/').split('/')
    return parts.none { it.isBlank() || it == "." || it == ".." }
}

private fun JSONObject.optNullableString(name: String): String? =
    if (isNull(name)) null else optString(name).takeIf { it.isNotBlank() }

private fun JSONObject.optNullableLong(name: String): Long? =
    if (isNull(name) || !has(name)) null else getLong(name)

private fun JSONArray.stringList(): List<String> =
    (0 until length()).map { getString(it) }

private fun JSONArray.objectList(): List<JSONObject> =
    (0 until length()).map { getJSONObject(it) }
