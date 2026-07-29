package com.lingxi.code.project

import com.lingxi.code.model.canonicalSessionId
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.UUID

internal const val PROJECTS_INDEX_FILE = "index.json"
internal const val PROJECT_MANIFEST_FILE = "project.json"
internal const val PROJECT_SESSION_INDEX_FILE = "session-index.json"
internal const val PROJECT_SYNC_BASELINE_FILE = "sync-baseline.json"
internal const val PROJECT_WORKSPACE_DIR = "workspace"
internal const val GLOBAL_SESSION_INDEX_FILE = "global-session-index.json"

fun interface ProjectAtomicWriter {
    fun write(target: File, text: String, validate: (String) -> Unit)
}

internal class DefaultProjectAtomicWriter : ProjectAtomicWriter {
    override fun write(target: File, text: String, validate: (String) -> Unit) {
        target.parentFile?.mkdirs()
        val temp = File(target.parentFile, ".${target.name}.${UUID.randomUUID()}.tmp")
        try {
            FileOutputStream(temp).use { output ->
                output.write(text.toByteArray(Charsets.UTF_8))
                output.fd.sync()
            }
            val staged = temp.readText(Charsets.UTF_8)
            validate(staged)
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
) {
    init {
        projectsRoot.mkdirs()
    }

    fun load(): ProjectStoreState {
        projectsRoot.mkdirs()
        val index = readIndex()
        val projectDirectories = projectsRoot.listFiles()
            .orEmpty()
            .filter { it.isDirectory && isLowercaseUuid(it.name) }
        val loaded = projectDirectories.map { projectDir ->
            projectDir to runCatching { loadProject(projectDir.name) }
        }
        val discovered = loaded.mapNotNull { it.second.getOrNull() }
            .sortedByDescending { it.record.updatedAtEpochMillis }
        val corruptCount = loaded.count { it.second.isFailure }
        val discoveredIds = discovered.map { it.record.id }
        val active = index.activeProjectId?.takeIf(discoveredIds::contains)
        if (index.projectIds != discoveredIds || active != index.activeProjectId) {
            writeIndex(ProjectIndex(active, discoveredIds))
        }
        return ProjectStoreState(
            projects = discovered,
            activeProjectId = active,
            globalSessions = readSessions(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE)),
            loading = false,
            errorMessage = if (corruptCount > 0) {
                "$corruptCount 个项目数据损坏或路径异常，已隔离；其他项目仍可使用。"
            } else {
                null
            },
        )
    }

    fun createInternal(name: String): ProjectSnapshot =
        createProject(name, ProjectStorageKind.Internal, null, null)

    fun createSafMirror(
        name: String,
        sourceTreeUri: String,
        sourceDisplayName: String?,
    ): ProjectSnapshot {
        require(sourceTreeUri.startsWith("content://")) { "SAF project must use a content:// URI" }
        return createProject(name, ProjectStorageKind.SafMirror, sourceTreeUri, sourceDisplayName)
    }

    private fun createProject(
        name: String,
        storageKind: ProjectStorageKind,
        sourceTreeUri: String?,
        sourceDisplayName: String?,
    ): ProjectSnapshot {
        val id = newId()
        require(isLowercaseUuid(id)) { "project id must be a lowercase UUID" }
        val cleanName = name.trim()
        require(cleanName.isNotEmpty()) { "project name cannot be blank" }
        require(cleanName.length <= 120) { "project name is too long" }
        val dir = projectDirectory(id)
        require(!dir.exists()) { "project already exists: $id" }
        val workspace = File(dir, PROJECT_WORKSPACE_DIR)
        require(workspace.mkdirs()) { "cannot create project workspace" }
        val timestamp = now()
        val record = ProjectRecord(
            id = id,
            name = cleanName,
            storageKind = storageKind,
            createdAtEpochMillis = timestamp,
            updatedAtEpochMillis = timestamp,
            sourceTreeUri = sourceTreeUri,
            sourceDisplayName = sourceDisplayName,
        )
        try {
            writeProject(record)
            writeSessions(id, emptyList())
            writeBaseline(id, ProjectSyncBaseline())
            val index = readIndex()
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

    fun setActiveProject(projectId: String?): ProjectStoreState {
        if (projectId != null) {
            validateProjectId(projectId)
            require(File(projectDirectory(projectId), PROJECT_MANIFEST_FILE).isFile) {
                "project does not exist: $projectId"
            }
        }
        val current = load()
        writeIndex(ProjectIndex(projectId, current.projects.map { it.record.id }))
        return load()
    }

    fun updateProject(record: ProjectRecord): ProjectSnapshot {
        validateProjectId(record.id)
        require(projectDirectory(record.id).isDirectory) { "project does not exist: ${record.id}" }
        val normalized = record.copy(updatedAtEpochMillis = now())
        writeProject(normalized)
        return loadProject(record.id)
    }

    fun updateSessions(
        projectId: String?,
        sessions: List<ProjectSessionSummary>,
    ): ProjectStoreState {
        val normalized = sessions
            .filter { it.sessionId.isNotBlank() }
            .distinctBy { it.sessionId }
            .sortedByDescending { it.updatedAtEpochMillis }
        if (projectId == null) {
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), normalized)
        } else {
            validateProjectId(projectId)
            writeSessions(projectId, normalized)
            val snapshot = loadProject(projectId)
            val lastActive = snapshot.record.lastActiveSessionId?.takeIf { id ->
                normalized.any { it.sessionId == id }
            }
            writeProject(snapshot.record.copy(lastActiveSessionId = lastActive))
        }
        return load()
    }

    /**
     * Persist a freshly confirmed Engine session before it has a rollout file.
     *
     * The engine's SessionList is file-backed, so a SessionStarted event can
     * precede the new empty session appearing in that listing. Keeping this
     * small, idempotent row makes the Project or global conversation recoverable
     * immediately; the next authoritative SessionList replaces it after the
     * first turn is persisted.
     */
    fun recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
    ): ProjectStoreState {
        val canonicalId = canonicalSessionId(sessionId)
        require(canonicalId.isNotBlank()) { "session id cannot be blank" }
        val snapshot = projectId?.let {
            validateProjectId(it)
            loadProject(it)
        }
        val sessions = snapshot?.sessions ?: load().globalSessions
        val timestamp = now()
        val existing = sessions.firstOrNull { it.sessionId == canonicalId }
        val started = ProjectSessionSummary(
            sessionId = canonicalId,
            title = title.ifBlank { existing?.title ?: "新对话" },
            messageCount = existing?.messageCount ?: 0,
            relativeTime = "刚刚",
            updatedAtEpochMillis = timestamp,
        )
        val updated = listOf(started) + sessions.filterNot { it.sessionId == canonicalId }
        if (projectId == null) {
            writeSessionFile(File(projectsRoot, GLOBAL_SESSION_INDEX_FILE), updated)
        } else {
            writeSessions(projectId, updated)
            writeProject(
                requireNotNull(snapshot).record.copy(
                    updatedAtEpochMillis = timestamp,
                    lastActiveSessionId = canonicalId,
                ),
            )
        }
        return load()
    }

    fun markActiveSession(projectId: String, sessionId: String): ProjectStoreState {
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

    fun workspace(projectId: String): ProjectWorkspace = loadProject(projectId).workspace

    fun project(projectId: String): ProjectSnapshot = loadProject(projectId)

    private fun loadProject(projectId: String): ProjectSnapshot {
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
        require(workspace.isDirectory || workspace.mkdirs()) { "project workspace is unavailable" }
        require(
            !Files.isSymbolicLink(workspace.toPath()) &&
                workspace.canonicalFile.parentFile == canonicalDir,
        ) {
            "project workspace escaped its managed project directory"
        }
        return ProjectSnapshot(
            record = record,
            workspace = ProjectWorkspace(projectId, workspace.canonicalPath),
            sessions = readSessions(File(dir, PROJECT_SESSION_INDEX_FILE)),
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

    private fun readSessions(file: File): List<ProjectSessionSummary> =
        if (file.isFile) runCatching { decodeSessions(file.readText(Charsets.UTF_8)) }.getOrDefault(emptyList())
        else emptyList()

    private fun readIndex(): ProjectIndex {
        val file = File(projectsRoot, PROJECTS_INDEX_FILE)
        return if (file.isFile) {
            runCatching { decodeIndex(file.readText(Charsets.UTF_8)) }.getOrElse {
                ProjectIndex().also { recovered ->
                    runCatching { writeIndex(recovered) }
                }
            }
        } else {
            ProjectIndex()
        }
    }

    private fun writeIndex(index: ProjectIndex) {
        writeJson(File(projectsRoot, PROJECTS_INDEX_FILE), encodeIndex(index), ::decodeIndex)
    }

    private fun <T> writeJson(target: File, value: String, decoder: (String) -> T) {
        writer.write(target, value) { decoder(it) }
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
        val ids = json.getJSONArray("projectIds").stringList().filter(::isLowercaseUuid)
        return ProjectIndex(json.optNullableString("activeProjectId"), ids.distinct())
    }

    private fun encodeProject(value: ProjectRecord): String = JSONObject()
        .put("version", 1)
        .put("id", value.id)
        .put("name", value.name)
        .put("storageKind", value.storageKind.wireName)
        .put("createdAtEpochMillis", value.createdAtEpochMillis)
        .put("updatedAtEpochMillis", value.updatedAtEpochMillis)
        .put("sourceTreeUri", value.sourceTreeUri)
        .put("sourceDisplayName", value.sourceDisplayName)
        .put("lastSyncAtEpochMillis", value.lastSyncAtEpochMillis)
        .put("lastActiveSessionId", value.lastActiveSessionId)
        .put("syncState", value.syncState.name)
        .toString(2)

    private fun decodeProject(text: String): ProjectRecord {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val id = json.getString("id")
        validateProjectId(id)
        return ProjectRecord(
            id = id,
            name = json.getString("name").also { require(it.isNotBlank()) },
            storageKind = ProjectStorageKind.fromWireName(json.getString("storageKind")),
            createdAtEpochMillis = json.getLong("createdAtEpochMillis"),
            updatedAtEpochMillis = json.getLong("updatedAtEpochMillis"),
            sourceTreeUri = json.optNullableString("sourceTreeUri"),
            sourceDisplayName = json.optNullableString("sourceDisplayName"),
            lastSyncAtEpochMillis = json.optNullableLong("lastSyncAtEpochMillis"),
            lastActiveSessionId = json.optNullableString("lastActiveSessionId")
                ?.let(::canonicalSessionId),
            syncState = runCatching {
                ProjectSyncState.valueOf(json.getString("syncState"))
            }.getOrElse { ProjectSyncState.Error },
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
                            .put("updatedAtEpochMillis", session.updatedAtEpochMillis),
                    )
                }
            },
        )
        .toString(2)

    private fun decodeSessions(text: String): List<ProjectSessionSummary> {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        return json.getJSONArray("sessions").objectList().map { row ->
            ProjectSessionSummary(
                sessionId = canonicalSessionId(row.getString("sessionId")),
                title = row.getString("title"),
                messageCount = row.getInt("messageCount"),
                relativeTime = row.getString("relativeTime"),
                updatedAtEpochMillis = row.getLong("updatedAtEpochMillis"),
            )
        }
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
