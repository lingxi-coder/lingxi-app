package com.lingxi.code.project

import android.content.Context
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

/**
 * Small durable conversation state — the last-active session id and unsent
 * composer drafts, keyed by [com.lingxi.code.model.ConversationScope]
 * persistence keys (`global` / `project.<id>` / `app.<id>`), plus which scope
 * was active last. JSON in app-private files with the same atomic
 * write-validate-rename discipline as [ProjectRepository] (shared
 * [ProjectAtomicWriter]).
 *
 * The legacy Project/global last-active fields stay with [ProjectRepository]
 * for Code compatibility. This store is authoritative for mode-scoped active
 * sessions across every workspace, drafts per conversation, and the active workspace.
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

    // Screens recreate this store, but all instances write the same app file.
    private val mutex = stateMutex

    data class ScopeState(
        val lastActiveSessionId: String? = null,
        val draft: String = "",
        /** Owner of a draft saved by the former mode-scoped layout. */
        val draftSessionId: String? = null,
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

    suspend fun readSelectedMode(): SessionMode? = withContext(Dispatchers.IO) {
        mutex.withLock { load().selectedMode }
    }

    suspend fun read(scopeKey: String): ScopeState? = withContext(Dispatchers.IO) {
        mutex.withLock { load().scopes[scopeKey] }
    }

    /** A mode switch keeps the same draft; another conversation has its own. */
    suspend fun readConversationDraft(scopeKey: String, sessionId: String, mode: SessionMode): String? = withContext(Dispatchers.IO) {
        mutex.withLock {
            val snapshot = load()
            val owner = canonicalSessionId(sessionId)
            snapshot.conversationDrafts[conversationDraftKey(scopeKey, owner)] ?: run {
                // Older scope slots did not record a separate draft owner. The
                // decoder captures their last-active owner once; changing the
                // active session must never reassign that draft to a new owner.
                listOf(mode, if (mode == SessionMode.Code) SessionMode.Chat else SessionMode.Code)
                    .firstNotNullOfOrNull { candidate ->
                        snapshot.scopes["$scopeKey#${candidate.wireKey}"]
                            ?.takeIf { it.draftSessionId?.let(::canonicalSessionId) == owner }
                            ?.draft
                    }
            }
        }
    }

    suspend fun persistConversationDraft(scopeKey: String, sessionId: String, draft: String) {
        require(sessionId.isNotBlank() && sessionId != "new")
        mutate { snapshot ->
            snapshot.copy(conversationDrafts = snapshot.conversationDrafts +
                (conversationDraftKey(scopeKey, canonicalSessionId(sessionId)) to draft))
        }
    }

    suspend fun readWorkspacePresentation(): Map<String, WorkspacePresentationState> = withContext(Dispatchers.IO) {
        mutex.withLock { load().workspacePresentation }
    }

    /** Pinned sessions, keyed by session id with the pin time in epoch millis. */
    suspend fun readPinnedSessions(): Map<String, Long> = withContext(Dispatchers.IO) {
        mutex.withLock { load().pinnedSessions }
    }

    /** When the user last opened the scheduled-task page; 0 when they never have. */
    suspend fun readCronResultsSeenAt(): Long = withContext(Dispatchers.IO) {
        mutex.withLock { load().cronResultsSeenAtEpochMillis }
    }

    suspend fun persistActiveScope(scopeKey: String) {
        mutate { it.copy(activeScopeKey = scopeKey) }
    }

    suspend fun persistActiveMode(mode: SessionMode) {
        mutate { it.copy(activeMode = mode) }
    }

    suspend fun persistSelectedMode(mode: SessionMode) {
        mutate { it.copy(selectedMode = mode) }
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
            snapshot.copy(scopes = snapshot.scopes + (scopeKey to current.copy(draft = draft, draftSessionId = current.lastActiveSessionId)))
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

    /** Pin a session, or unpin it when [pinnedAtEpochMillis] is null. */
    suspend fun persistSessionPinned(sessionId: String, pinnedAtEpochMillis: Long?) {
        mutate { snapshot ->
            snapshot.copy(
                pinnedSessions = if (pinnedAtEpochMillis == null) {
                    snapshot.pinnedSessions - sessionId
                } else {
                    snapshot.pinnedSessions + (sessionId to pinnedAtEpochMillis)
                },
            )
        }
    }

    suspend fun persistCronResultsSeenAt(epochMillis: Long) {
        mutate { it.copy(cronResultsSeenAtEpochMillis = epochMillis) }
    }

    private data class Snapshot(
        val activeScopeKey: String? = null,
        val activeMode: SessionMode = SessionMode.Code,
        val selectedMode: SessionMode? = null,
        val scopes: Map<String, ScopeState> = emptyMap(),
        val conversationDrafts: Map<String, String> = emptyMap(),
        val workspacePresentation: Map<String, WorkspacePresentationState> = emptyMap(),
        val pinnedSessions: Map<String, Long> = emptyMap(),
        val cronResultsSeenAtEpochMillis: Long = 0L,
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
        .put("selectedMode", snapshot.selectedMode?.wireKey)
        .put(
            "scopes",
            JSONObject().also { scopes ->
                snapshot.scopes.forEach { (key, state) ->
                    scopes.put(
                        key,
                        JSONObject()
                            .put("lastActiveSessionId", state.lastActiveSessionId)
                            .put("draft", state.draft)
                            .put("draftSessionId", state.draftSessionId ?: JSONObject.NULL),
                    )
                }
            },
        )
        .put("conversationDrafts", JSONObject().also { drafts ->
            snapshot.conversationDrafts.forEach { (key, value) -> drafts.put(key, value) }
        })
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
        .put(
            "pinnedSessions",
            JSONObject().also { pins ->
                snapshot.pinnedSessions.forEach { (sessionId, at) -> pins.put(sessionId, at) }
            },
        )
        .put("cronResultsSeenAtEpochMillis", snapshot.cronResultsSeenAtEpochMillis)
        .toString(2)

    private fun decode(text: String): Snapshot {
        val json = JSONObject(text)
        require(json.getInt("version") == 1)
        val scopesJson = json.optJSONObject("scopes") ?: JSONObject()
        val scopes = buildMap {
            scopesJson.keys().forEach { key ->
                val entry = scopesJson.getJSONObject(key)
                val lastSessionId = if (entry.isNull("lastActiveSessionId")) null
                    else entry.optString("lastActiveSessionId").takeIf { it.isNotBlank() }
                put(
                    key,
                    ScopeState(
                        lastActiveSessionId = lastSessionId,
                        draft = entry.optString("draft"),
                        draftSessionId = if (entry.has("draftSessionId")) {
                            if (entry.isNull("draftSessionId")) null else entry.optString("draftSessionId").takeIf { it.isNotBlank() }
                        } else lastSessionId,
                    ),
                )
            }
        }
        val draftsJson = json.optJSONObject("conversationDrafts") ?: JSONObject()
        val conversationDrafts = buildMap {
            draftsJson.keys().forEach { key -> put(key, draftsJson.getString(key)) }
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
        val pinnedSessionsJson = json.optJSONObject("pinnedSessions") ?: JSONObject()
        val pinnedSessions = buildMap {
            pinnedSessionsJson.keys().forEach { sessionId ->
                pinnedSessionsJson.optLong(sessionId).takeIf { it > 0L }?.let { put(sessionId, it) }
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
            selectedMode = when (json.optString("selectedMode")) {
                SessionMode.Chat.wireValue -> SessionMode.Chat
                SessionMode.Code.wireValue -> SessionMode.Code
                else -> null
            },
            scopes = scopes,
            conversationDrafts = conversationDrafts,
            workspacePresentation = workspacePresentation,
            pinnedSessions = pinnedSessions,
            cronResultsSeenAtEpochMillis = json.optLong("cronResultsSeenAtEpochMillis", 0L),
        )
    }

    companion object {
        internal const val SCOPE_STATE_FILE = "scope-state.json"
        private val stateMutex = Mutex()
        private fun conversationDraftKey(scopeKey: String, sessionId: String) = "$scopeKey#session:$sessionId"
    }
}
