package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

/**
 * Small durable per-scope conversation state — the last-active session id and
 * the unsent composer draft, keyed by [com.lingxi.code.model.ConversationScope]
 * persistence keys (`global` / `project.<id>` / `app.<id>`), plus which scope
 * was active last. JSON in app-private files with the same atomic
 * write-validate-rename discipline as [ProjectRepository] (shared
 * [ProjectAtomicWriter]).
 *
 * The legacy Project/global last-active fields stay with [ProjectRepository]
 * for Code compatibility. This store is authoritative for mode-scoped active
 * sessions and drafts across every workspace, and for the active workspace.
 *
 * All suspend entry points hop to [Dispatchers.IO] and serialize on one mutex,
 * so callers may invoke them from the main thread (e.g. per-keystroke draft
 * mirroring) without jank or torn writes.
 */
class ScopeStateStore internal constructor(
    private val file: File,
    private val writer: ProjectAtomicWriter = DefaultProjectAtomicWriter(),
) {
    constructor(context: Context) : this(File(context.filesDir, SCOPE_STATE_FILE))

    private val mutex = Mutex()

    data class ScopeState(
        val lastActiveSessionId: String? = null,
        val draft: String = "",
    )

    data class WorkspacePresentationState(
        val pinnedAtEpochMillis: Long? = null,
        val collapsed: Boolean = false,
    )

    suspend fun readActiveScopeKey(): String? = withContext(Dispatchers.IO) {
        mutex.withLock { load().activeScopeKey }
    }

    suspend fun readActiveMode(): SessionMode? = withContext(Dispatchers.IO) {
        mutex.withLock { load().activeMode }
    }

    suspend fun read(scopeKey: String): ScopeState? = withContext(Dispatchers.IO) {
        mutex.withLock { load().scopes[scopeKey] }
    }

    suspend fun readWorkspacePresentation(): Map<String, WorkspacePresentationState> = withContext(Dispatchers.IO) {
        mutex.withLock { load().workspacePresentation }
    }

    suspend fun persistActiveScope(scopeKey: String) {
        mutate { it.copy(activeScopeKey = scopeKey) }
    }

    suspend fun persistActiveMode(mode: SessionMode) {
        mutate { it.copy(activeMode = mode) }
    }

    suspend fun persistLastActiveSession(scopeKey: String, sessionId: String?) {
        mutate { snapshot ->
            val current = snapshot.scopes[scopeKey] ?: ScopeState()
            snapshot.copy(
                scopes = snapshot.scopes + (scopeKey to current.copy(lastActiveSessionId = sessionId)),
            )
        }
    }

    suspend fun persistDraft(scopeKey: String, draft: String) {
        mutate { snapshot ->
            val current = snapshot.scopes[scopeKey] ?: ScopeState()
            snapshot.copy(scopes = snapshot.scopes + (scopeKey to current.copy(draft = draft)))
        }
    }

    suspend fun persistWorkspacePinned(workspaceKey: String, pinnedAtEpochMillis: Long?) {
        mutate { snapshot ->
            val current = snapshot.workspacePresentation[workspaceKey] ?: WorkspacePresentationState()
            snapshot.copy(
                workspacePresentation = snapshot.workspacePresentation + (
                    workspaceKey to current.copy(pinnedAtEpochMillis = pinnedAtEpochMillis)
                ),
            )
        }
    }

    suspend fun persistWorkspaceCollapsed(workspaceKey: String, collapsed: Boolean) {
        mutate { snapshot ->
            val current = snapshot.workspacePresentation[workspaceKey] ?: WorkspacePresentationState()
            snapshot.copy(
                workspacePresentation = snapshot.workspacePresentation + (
                    workspaceKey to current.copy(collapsed = collapsed)
                ),
            )
        }
    }

    private data class Snapshot(
        val activeScopeKey: String? = null,
        val activeMode: SessionMode = SessionMode.Code,
        val scopes: Map<String, ScopeState> = emptyMap(),
        val workspacePresentation: Map<String, WorkspacePresentationState> = emptyMap(),
    )

    private suspend fun mutate(transform: (Snapshot) -> Snapshot) {
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val next = transform(load())
                writer.write(file, encode(next)) { decode(it) }
            }
        }
    }

    /** A missing or corrupt file reads as empty — scope state is best-effort. */
    private fun load(): Snapshot =
        if (file.isFile) {
            runCatching { decode(file.readText(Charsets.UTF_8)) }.getOrDefault(Snapshot())
        } else {
            Snapshot()
        }

    private fun encode(snapshot: Snapshot): String = JSONObject()
        .put("version", 1)
        .put("activeScopeKey", snapshot.activeScopeKey)
        .put("activeMode", snapshot.activeMode.wireKey)
        .put(
            "scopes",
            JSONObject().also { scopes ->
                snapshot.scopes.forEach { (key, state) ->
                    scopes.put(
                        key,
                        JSONObject()
                            .put("lastActiveSessionId", state.lastActiveSessionId)
                            .put("draft", state.draft),
                    )
                }
            },
        )
        .put(
            "workspacePresentation",
            JSONObject().also { prefs ->
                snapshot.workspacePresentation.forEach { (key, state) ->
                    prefs.put(
                        key,
                        JSONObject()
                            .put("pinnedAtEpochMillis", state.pinnedAtEpochMillis)
                            .put("collapsed", state.collapsed),
                    )
                }
            },
        )
        .toString(2)

    private fun decode(text: String): Snapshot {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val scopesJson = json.optJSONObject("scopes") ?: JSONObject()
        val scopes = buildMap {
            scopesJson.keys().forEach { key ->
                val entry = scopesJson.getJSONObject(key)
                put(
                    key,
                    ScopeState(
                        lastActiveSessionId = if (entry.isNull("lastActiveSessionId")) {
                            null
                        } else {
                            entry.optString("lastActiveSessionId").takeIf { it.isNotBlank() }
                        },
                        draft = entry.optString("draft"),
                    ),
                )
            }
        }
        val workspacePresentationJson = json.optJSONObject("workspacePresentation") ?: JSONObject()
        val workspacePresentation = buildMap {
            workspacePresentationJson.keys().forEach { key ->
                val entry = workspacePresentationJson.getJSONObject(key)
                put(
                    key,
                    WorkspacePresentationState(
                        pinnedAtEpochMillis = if (entry.isNull("pinnedAtEpochMillis")) {
                            null
                        } else {
                            entry.optLong("pinnedAtEpochMillis").takeIf { it > 0L }
                        },
                        collapsed = entry.optBoolean("collapsed", false),
                    ),
                )
            }
        }
        return Snapshot(
            activeScopeKey = if (json.isNull("activeScopeKey")) {
                null
            } else {
                json.optString("activeScopeKey").takeIf { it.isNotBlank() }
            },
            activeMode = when (json.optString("activeMode")) {
                SessionMode.Chat.wireValue -> SessionMode.Chat
                else -> SessionMode.Code
            },
            scopes = scopes,
            workspacePresentation = workspacePresentation,
        )
    }

    companion object {
        internal const val SCOPE_STATE_FILE = "scope-state.json"
    }
}
