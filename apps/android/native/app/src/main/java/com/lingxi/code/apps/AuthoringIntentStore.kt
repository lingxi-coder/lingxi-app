package com.lingxi.code.apps

import android.content.Context
import com.lingxi.code.project.DefaultProjectAtomicWriter
import com.lingxi.code.project.ProjectAtomicWriter
import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.util.UUID

/** A selection is not a grant; native must revalidate its source ID and physical identity. */
internal data class AuthoringIntent(
    val selectionId: String,
    val sourceId: String,
    val sourceRequestId: String,
    val sourceDirectory: String,
    val projectId: String,
    val workspaceId: String,
    val appId: String?,
    val sessionId: String? = null,
    val transcriptPath: String? = null,
    val sessionMode: SessionMode? = null,
    val catalogued: Boolean = false,
    val bindingConfirmed: Boolean = false,
)

/** Persisted before a source-producing native call; selectionId is its idempotency key. */
internal data class AuthoringRequest(
    val selectionId: String,
    val operation: String,
    val name: String,
    val projectId: String,
    val workspaceId: String,
    val appId: String? = null,
    val revision: Long? = null,
    val sourceId: String? = null,
    val sourceDirectory: String? = null,
    val targetSessionId: String? = null,
    val targetTranscriptPath: String? = null,
    val completed: Boolean = false,
)

internal class InvalidAuthoringNameException : IllegalArgumentException()

data class PendingAuthoringRecovery(val projectId: String, val selectionId: String, val name: String)

/** Discovery is by each Project's exact pointer; these rows confer no source permission. */
internal suspend fun pendingAuthoringRecoveries(
    projects: List<PendingAuthoringRecovery>,
    store: AuthoringIntentStore,
): List<PendingAuthoringRecovery> = projects.filter { project ->
    try {
        val request = store.readRequest(project.selectionId)
        val selection = store.readSelection(project.selectionId)
        val requestMatches = request?.projectId == project.projectId
        val selectionMatches = selection?.projectId == project.projectId
        when {
            selection != null && !selectionMatches -> false
            request != null && !requestMatches -> false
            selection != null -> !selection.bindingConfirmed || (request != null && !request.completed)
            request != null -> !request.completed
            else -> false
        }
    } catch (cancelled: kotlinx.coroutines.CancellationException) {
        throw cancelled
    } catch (_: Exception) {
        false
    }
}

/** Matches Rust's Unicode-scalar length and trim rules without altering the idempotent request. */
internal fun validAuthoringName(name: String): Boolean {
    var offset = 0
    var count = 0
    var hasNonWhitespace = false
    while (offset < name.length) {
        val first = name[offset]
        val point = when {
            Character.isHighSurrogate(first) -> {
                if (offset + 1 >= name.length || !Character.isLowSurrogate(name[offset + 1])) return false
                Character.toCodePoint(first, name[offset + 1]).also { offset++ }
            }
            Character.isLowSurrogate(first) -> return false
            else -> first.code
        }
        offset++
        if (++count > 80) return false
        val whitespace = point in 0x09..0x0d || point == 0x20 || point == 0x85 || point == 0xa0 ||
            point == 0x1680 || point in 0x2000..0x200a || point == 0x2028 || point == 0x2029 ||
            point == 0x202f || point == 0x205f || point == 0x3000
        if (!whitespace) hasNonWhitespace = true
    }
    return hasNonWhitespace
}

/** Each selection has its own file so one damaged owner cannot block another. */
internal class AuthoringIntentStore(
    private val directory: File,
    private val writer: ProjectAtomicWriter = DefaultProjectAtomicWriter(),
) {
    constructor(context: Context) : this(File(context.filesDir, "local-app-authoring-intents"))

    suspend fun saveRequest(request: AuthoringRequest, mode: SessionMode?) = withContext(Dispatchers.IO) {
        requireCodeAuthoring(mode)
        validateRequest(request)
        mutex.withLock {
            ensureDirectory(create = true)
            val target = requestPath(request.selectionId)
            val existing = readRequestFile(target)
            check(existing == null || existing.copy(completed = false) == request.copy(completed = false)) {
                "Authoring request conflicts with an existing selection"
            }
            if (existing == null) {
                require(!request.completed)
                writeRequest(target, request)
            }
        }
    }

    suspend fun readRequest(selectionId: String): AuthoringRequest? = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (!ensureDirectory(create = false)) return@withLock null
            readRequestFile(requestPath(selectionId))?.also { require(it.selectionId == selectionId) }
        }
    }

    suspend fun savePrepared(intent: AuthoringIntent, mode: SessionMode?) = withContext(Dispatchers.IO) {
        requireCodeAuthoring(mode)
        validate(intent)
        require(intent.sessionId == null && intent.transcriptPath == null && intent.sessionMode == null &&
            !intent.catalogued && !intent.bindingConfirmed)
        mutex.withLock {
            ensureDirectory(create = true)
            val target = path(intent.selectionId)
            val old = load(target)
            check(old == null || old == intent) { "Authoring selection conflicts with existing intent" }
            if (old == null) write(target, intent)
        }
    }

    /** Atomically adds the native-assigned UUID and exact transcript to the same selection. */
    suspend fun attach(selectionId: String, sessionId: String, transcriptPath: String, sessionMode: SessionMode, trackLive: Boolean = true): AuthoringIntent =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                ensureDirectory(create = false)
                val target = path(selectionId)
                val old = checkNotNull(load(target)) { "Authoring selection is unavailable" }
                check(old.selectionId == selectionId) { "Authoring selection identity changed" }
                val next = old.copy(sessionId = sessionId, transcriptPath = transcriptPath, sessionMode = sessionMode)
                validate(next)
                check(old.sessionId == null || (old.sessionId == sessionId && old.transcriptPath == transcriptPath && old.sessionMode == sessionMode)) {
                    "Authoring selection belongs to another session"
                }
                if (old.sessionId == null) write(target, next)
                if (trackLive && !old.catalogued) liveSessions.add(sessionId)
                next
            }
        }

    suspend fun readSelection(selectionId: String): AuthoringIntent? = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (!ensureDirectory(create = false)) return@withLock null
            load(path(selectionId))?.also { require(it.selectionId == selectionId) }
        }
    }

    suspend fun read(sessionId: String): AuthoringIntent? = withContext(Dispatchers.IO) {
        require(canonicalUuid(sessionId))
        mutex.withLock {
            val matches = entries().filter { it.sessionId == sessionId }
            check(matches.size <= 1) { "Authoring session has ambiguous selections" }
            matches.singleOrNull()
        }
    }

    suspend fun staged(): List<AuthoringIntent> = withContext(Dispatchers.IO) {
        mutex.withLock { entries().filter { it.sessionId != null && !it.catalogued && it.sessionId !in liveSessions } }
    }

    suspend fun markCatalogued(sessionId: String) = withContext(Dispatchers.IO) {
        require(canonicalUuid(sessionId))
        mutex.withLock {
            val intent = checkNotNull(entries().filter { it.sessionId == sessionId }.singleOrNull())
            if (!intent.catalogued) write(path(intent.selectionId), intent.copy(catalogued = true))
            liveSessions.remove(sessionId)
        }
    }

    /** This flag exposes retries; the native receipt remains the authority for every open. */
    suspend fun markBindingConfirmed(selectionId: String, sessionId: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            val target = path(selectionId)
            val intent = checkNotNull(load(target))
            require(intent.selectionId == selectionId && intent.sessionId == sessionId && intent.sessionMode == SessionMode.Code)
            if (!intent.bindingConfirmed) write(target, intent.copy(bindingConfirmed = true))
        }
    }

    suspend fun markRequestCompleted(selectionId: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            val target = requestPath(selectionId)
            val request = checkNotNull(readRequestFile(target))
            require(request.selectionId == selectionId)
            if (!request.completed) writeRequest(target, request.copy(completed = true))
        }
    }

    suspend fun releaseLive(sessionId: String) = withContext(Dispatchers.IO) { mutex.withLock { liveSessions.remove(sessionId) } }

    private fun entries(): List<AuthoringIntent> {
        if (!ensureDirectory(create = false)) return emptyList()
        return directory.listFiles().orEmpty().mapNotNull { file ->
            val id = file.name.removePrefix("source-").removeSuffix(".json")
            if (file.name != "source-$id.json" || !canonicalUuid(id)) return@mapNotNull null
            runCatching { load(file)?.takeIf { it.selectionId == id } }.getOrNull()
        }
    }

    private fun ensureDirectory(create: Boolean): Boolean {
        if (!directory.exists() && !Files.isSymbolicLink(directory.toPath())) {
            if (!create) return false
            check(directory.mkdirs() || directory.isDirectory)
        }
        require(directory.isDirectory && !Files.isSymbolicLink(directory.toPath())) { "Authoring intent directory is unavailable" }
        return true
    }

    private fun path(selectionId: String): File {
        require(canonicalUuid(selectionId))
        return File(directory, "source-$selectionId.json")
    }

    private fun requestPath(selectionId: String): File {
        require(canonicalUuid(selectionId))
        return File(directory, "request-$selectionId.json")
    }

    private fun readRequestFile(file: File): AuthoringRequest? {
        if (!file.exists() && !Files.isSymbolicLink(file.toPath())) return null
        require(file.isFile && !Files.isSymbolicLink(file.toPath()))
        return decodeRequest(file.readText(Charsets.UTF_8))
    }

    private fun writeRequest(file: File, request: AuthoringRequest) {
        val text = JSONObject().put("version", 1).put("selectionId", request.selectionId)
            .put("operation", request.operation).put("name", request.name).put("projectId", request.projectId)
            .put("workspaceId", request.workspaceId).put("appId", request.appId ?: JSONObject.NULL)
            .put("revision", request.revision ?: JSONObject.NULL).put("sourceId", request.sourceId ?: JSONObject.NULL)
            .put("sourceDirectory", request.sourceDirectory ?: JSONObject.NULL)
            .put("targetSessionId", request.targetSessionId ?: JSONObject.NULL)
            .put("targetTranscriptPath", request.targetTranscriptPath ?: JSONObject.NULL)
            .put("completed", request.completed).toString()
        writer.write(file, text) { decodeRequest(it) }
    }

    private fun decodeRequest(text: String): AuthoringRequest {
        val row = JSONObject(text)
        require(row.get("version") == 1 && listOf("appId", "revision", "sourceId", "sourceDirectory", "targetSessionId", "targetTranscriptPath", "completed").all(row::has))
        return AuthoringRequest(row.getString("selectionId"), row.getString("operation"), row.getString("name"),
            row.getString("projectId"), row.getString("workspaceId"), if (row.isNull("appId")) null else row.getString("appId"),
            if (row.isNull("revision")) null else row.getLong("revision"), if (row.isNull("sourceId")) null else row.getString("sourceId"),
            if (row.isNull("sourceDirectory")) null else row.getString("sourceDirectory"),
            if (row.isNull("targetSessionId")) null else row.getString("targetSessionId"),
            if (row.isNull("targetTranscriptPath")) null else row.getString("targetTranscriptPath"), row.getBoolean("completed")).also(::validateRequest)
    }

    private fun validateRequest(request: AuthoringRequest) {
        require(canonicalUuid(request.selectionId) && canonicalUuid(request.projectId) && canonicalUuid(request.workspaceId))
        require(request.operation in setOf("create", "restore", "remix", "bind"))
        // Restore/bind do not send this display name to native; it must still fit the Project record.
        if (!validAuthoringName(request.name)) throw InvalidAuthoringNameException()
        require(request.appId == null || request.appId.isNotBlank())
        require((request.operation == "create") == (request.appId == null))
        require(request.revision == null || request.revision > 0)
        if (request.operation == "restore" || request.operation == "remix") require(request.revision != null)
        else require(request.revision == null)
        if (request.operation == "bind") require(request.sourceId != null && request.sourceDirectory != null)
        else require(request.sourceId == null && request.sourceDirectory == null)
        request.sourceId?.let { require(canonicalUuid(it)) }
        request.sourceDirectory?.let { require(it.startsWith('/') && '\u0000' !in it) }
        require((request.targetSessionId == null) == (request.targetTranscriptPath == null))
        if (request.operation != "bind") require(request.targetSessionId == null)
        request.targetSessionId?.let { require(canonicalUuid(it)) }
        request.targetTranscriptPath?.let { require(it.startsWith('/') && '\u0000' !in it) }
    }

    private fun write(target: File, intent: AuthoringIntent) {
        val encoded = JSONObject().put("version", 1).put("selectionId", intent.selectionId)
            .put("sourceId", intent.sourceId).put("sourceRequestId", intent.sourceRequestId).put("sourceDirectory", intent.sourceDirectory)
            .put("projectId", intent.projectId).put("workspaceId", intent.workspaceId)
            .put("appId", intent.appId ?: JSONObject.NULL).put("sessionId", intent.sessionId ?: JSONObject.NULL)
            .put("transcriptPath", intent.transcriptPath ?: JSONObject.NULL).put("sessionMode", intent.sessionMode?.wireValue ?: JSONObject.NULL)
            .put("catalogued", intent.catalogued).put("bindingConfirmed", intent.bindingConfirmed).toString()
        writer.write(target, encoded) { decode(it) }
    }

    private fun load(file: File): AuthoringIntent? {
        if (!file.exists() && !Files.isSymbolicLink(file.toPath())) return null
        require(file.isFile && !Files.isSymbolicLink(file.toPath())) { "Authoring intent is unavailable" }
        return decode(file.readText(Charsets.UTF_8))
    }

    private fun decode(text: String): AuthoringIntent {
        val row = JSONObject(text)
        require(row.get("version") == 1 && row.has("appId") && row.has("sessionId") && row.has("transcriptPath") &&
            row.has("sessionMode") && row.has("bindingConfirmed"))
        return AuthoringIntent(row.getString("selectionId"), row.getString("sourceId"), row.getString("sourceRequestId"), row.getString("sourceDirectory"),
            row.getString("projectId"), row.getString("workspaceId"), if (row.isNull("appId")) null else row.getString("appId"),
            if (row.isNull("sessionId")) null else row.getString("sessionId"),
            if (row.isNull("transcriptPath")) null else row.getString("transcriptPath"),
            if (row.isNull("sessionMode")) null else SessionMode.entries.single { it.wireValue == row.getString("sessionMode") },
            row.getBoolean("catalogued"), row.getBoolean("bindingConfirmed")).also(::validate)
    }

    private fun validate(intent: AuthoringIntent) {
        require(canonicalUuid(intent.selectionId) && canonicalUuid(intent.sourceId) && canonicalUuid(intent.sourceRequestId) &&
            canonicalUuid(intent.projectId) && canonicalUuid(intent.workspaceId))
        require(intent.sourceDirectory.startsWith('/') && intent.sourceDirectory.split('/').drop(1)
            .all { it.isNotBlank() && it != "." && it != ".." && '\\' !in it && '\u0000' !in it })
        require(intent.appId == null || intent.appId.isNotBlank())
        require((intent.sessionId == null) == (intent.transcriptPath == null))
        require((intent.sessionId == null) == (intent.sessionMode == null))
        require(intent.sessionMode == null || intent.sessionMode == SessionMode.Code)
        intent.sessionId?.let { require(canonicalUuid(it)) }
        intent.transcriptPath?.let { require(it.startsWith('/') && '\u0000' !in it) }
        require(!intent.catalogued || intent.sessionId != null)
        require(!intent.bindingConfirmed || intent.sessionId != null)
    }

    private companion object {
        val mutex = Mutex()
        val liveSessions = mutableSetOf<String>()
        fun canonicalUuid(value: String) = runCatching { UUID.fromString(value).toString() == value }.getOrDefault(false)
    }
}
