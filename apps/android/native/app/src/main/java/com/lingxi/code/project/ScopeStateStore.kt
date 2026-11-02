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
import java.nio.file.Files

/**
 * Small durable conversation state — the last-active session id and unsent
 * composer drafts, keyed by [com.lingxi.code.model.ConversationScope]
 * persistence keys (`global` / `project.<id>` / `app.<id>`), plus which scope
 * was active last. JSON in app-private files with the same atomic
 * write-validate-rename discipline as [ProjectRepository] (shared
 * [ProjectAtomicWriter]).
 *
 * This store is authoritative for mode-scoped active sessions, drafts per
 * conversation, and the active workspace.
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

    data class ScopeState(val lastActiveSessionId: String? = null)

    data class WorkspacePresentationState(
        val pinnedAtEpochMillis: Long? = null,
        val collapsed: Boolean = false,
    )

    data class ConversationHandoff(
        val sourceScope: String,
        val sourceSessionId: String,
        val sourceMode: SessionMode,
        val targetMode: SessionMode,
        val cutoff: String,
        val requestId: String,
    )

    data class SessionOwner(val scopeKey: String, val mode: SessionMode)

    suspend fun validate() = withContext(Dispatchers.IO) {
        mutex.withLock { load() }
        Unit
    }

    suspend fun readConversationHandoff(scopeKey: String, sessionId: String): ConversationHandoff? =
        withContext(Dispatchers.IO) { mutex.withLock { load().handoffs[conversationDraftKey(scopeKey, canonicalSessionId(sessionId))] } }

    /** Each space remembers its own last location across process restarts. */
    suspend fun readActiveScopeKey(mode: SessionMode): String? = withContext(Dispatchers.IO) {
        mutex.withLock {
            load().activeScopes[mode.wireValue]
        }
    }

    suspend fun persistActiveSelection(
        scopeKey: String,
        mode: SessionMode,
        allowCommit: () -> Boolean = { true },
    ): Boolean = withContext(Dispatchers.IO) {
        mutex.withLock {
            synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                if (!allowCommit()) false else {
                    val snapshot = load()
                    val next = snapshot.copy(activeMode = mode,
                        activeScopes = snapshot.activeScopes + (mode.wireValue to scopeKey))
                    write(next)
                    true
                }
            }
        }
    }

    /** A UUID has one logical workspace and creation mode, independent of cwd. */
    suspend fun claimSessionOwner(
        sessionId: String,
        scopeKey: String,
        mode: SessionMode,
        allowNew: () -> Boolean = { true },
    ): Boolean =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val id = canonicalSessionId(sessionId)
                require(id.isNotBlank() && id != "new")
                require(com.lingxi.code.model.conversationScopeFromKey(scopeKey) != null) { "Invalid session owner scope" }
                val snapshot = load()
                val owner = SessionOwner(scopeKey, mode)
                val previous = snapshot.sessionOwners[id]
                if (previous != null) return@withLock previous == owner
                synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                    if (!allowNew()) false else {
                        write(snapshot.copy(sessionOwners = snapshot.sessionOwners + (id to owner)))
                        true
                    }
                }
            }
        }

    suspend fun readSessionOwners(): Map<String, SessionOwner> = withContext(Dispatchers.IO) {
        mutex.withLock { load().sessionOwners }
    }

    suspend fun matchesSessionOwner(sessionId: String, scopeKey: String, mode: SessionMode): Boolean =
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val owner = load().sessionOwners[canonicalSessionId(sessionId)]
                owner == null || owner == SessionOwner(scopeKey, mode)
            }
        }

    /** Persist the draft and its origin together so neither can outlive the other. */
    suspend fun persistHandoffDraft(
        scopeKey: String, sessionId: String, draft: String, handoff: ConversationHandoff,
    ) {
        val id = canonicalSessionId(sessionId)
        require(id.isNotBlank() && id != "new" && handoff.sourceSessionId.isNotBlank())
        val key = conversationDraftKey(scopeKey, id)
        mutate { it.copy(conversationDrafts = it.conversationDrafts + (key to draft),
            handoffs = it.handoffs + (key to handoff.copy(sourceSessionId = canonicalSessionId(handoff.sourceSessionId)))) }
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

    suspend fun readConversationDraft(scopeKey: String, sessionId: String): String? = withContext(Dispatchers.IO) {
        mutex.withLock {
            val owner = canonicalSessionId(sessionId)
            load().conversationDrafts[conversationDraftKey(scopeKey, owner)]
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
        val activeMode: SessionMode = SessionMode.Code,
        val activeScopes: Map<String, String> = emptyMap(),
        val selectedMode: SessionMode? = null,
        val sessionOwners: Map<String, SessionOwner> = emptyMap(),
        val scopes: Map<String, ScopeState> = emptyMap(),
        val conversationDrafts: Map<String, String> = emptyMap(),
        val handoffs: Map<String, ConversationHandoff> = emptyMap(),
        val workspacePresentation: Map<String, WorkspacePresentationState> = emptyMap(),
        val pinnedSessions: Map<String, Long> = emptyMap(),
        val cronResultsSeenAtEpochMillis: Long = 0L,
    )

    private suspend fun mutate(transform: (Snapshot) -> Snapshot) {
        withContext(Dispatchers.IO) {
            mutex.withLock {
                val next = transform(load())
                write(next)
            }
        }
    }

    private val observationKey: String = file.absoluteFile.normalize().path

    /** Never recreate a previously observed file or overwrite a corrupt one. */
    private fun load(): Snapshot {
        val symbolicLink = Files.isSymbolicLink(file.toPath())
        if (symbolicLink || file.exists()) {
            observedStateFiles.add(observationKey)
            require(!symbolicLink && file.isFile) { "Conversation state is not a file: $file" }
            return decode(file.readText(Charsets.UTF_8))
        }
        check(observationKey !in observedStateFiles) { "Conversation state is missing: $file" }
        return Snapshot()
    }

    private fun write(snapshot: Snapshot) {
        writer.write(file, encode(snapshot)) { decode(it) }
        observedStateFiles.add(observationKey)
    }

    private fun encode(snapshot: Snapshot): String = JSONObject()
        .put("version", 2)
        .put("activeMode", snapshot.activeMode.wireValue)
        .put("activeScopes", JSONObject(snapshot.activeScopes))
        .put("selectedMode", snapshot.selectedMode?.wireValue)
        .put("sessionOwners", JSONObject().also { owners ->
            snapshot.sessionOwners.forEach { (id, owner) ->
                owners.put(id, JSONObject().put("scope", owner.scopeKey).put("mode", owner.mode.wireValue))
            }
        })
        .put(
            "scopes",
            JSONObject().also { scopes ->
                snapshot.scopes.forEach { (key, state) ->
                    scopes.put(
                        key,
                        JSONObject().put("lastActiveSessionId", state.lastActiveSessionId),
                    )
                }
            },
        )
        .put("conversationDrafts", JSONObject().also { drafts ->
            snapshot.conversationDrafts.forEach { (key, value) -> drafts.put(key, value) }
        })
        .put("handoffs", JSONObject().also { records ->
            snapshot.handoffs.forEach { (key, value) ->
                records.put(key, JSONObject().put("sourceScope", value.sourceScope)
                    .put("sourceSessionId", value.sourceSessionId).put("sourceMode", value.sourceMode.wireValue)
                    .put("targetMode", value.targetMode.wireValue).put("cutoff", value.cutoff)
                    .put("requestId", value.requestId))
            }
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
        .toString(2).escapeSurrogateCodeUnits()

    private fun decode(text: String): Snapshot {
        val json = JSONObject(text)
        require(json.getInt("version") == 2)
        val scopesJson = json.getJSONObject("scopes")
        val scopes = buildMap {
            scopesJson.keys().forEach { key ->
                val entry = scopesJson.getJSONObject(key)
                val lastSessionId = if (entry.isNull("lastActiveSessionId")) null
                    else entry.optString("lastActiveSessionId").takeIf { it.isNotBlank() }
                put(
                    key,
                    ScopeState(lastActiveSessionId = lastSessionId),
                )
            }
        }
        val draftsJson = json.getJSONObject("conversationDrafts")
        val conversationDrafts = buildMap {
            draftsJson.keys().forEach { key -> put(key, draftsJson.getString(key)) }
        }
        val activeScopesJson = json.getJSONObject("activeScopes")
        val activeScopes = buildMap {
            activeScopesJson.keys().forEach { key ->
                com.lingxi.code.model.sessionModeFromWireValue(key)
                put(key, activeScopesJson.getString(key))
            }
        }
        val ownersJson = json.getJSONObject("sessionOwners")
        val sessionOwners = buildMap {
            ownersJson.keys().forEach { id ->
                val owner = ownersJson.getJSONObject(id)
                require(id.isNotBlank() && id != "new" && canonicalSessionId(id) == id) { "Invalid session owner id" }
                val scope = owner.getString("scope")
                require(com.lingxi.code.model.conversationScopeFromKey(scope) != null) { "Invalid session owner scope" }
                put(id, SessionOwner(scope,
                    com.lingxi.code.model.sessionModeFromWireValue(owner.getString("mode"))))
            }
        }
        val handoffsJson = json.getJSONObject("handoffs")
        val handoffs = buildMap {
            handoffsJson.keys().forEach { key ->
                val entry = handoffsJson.getJSONObject(key)
                put(key, ConversationHandoff(entry.getString("sourceScope"), entry.getString("sourceSessionId"),
                    com.lingxi.code.model.sessionModeFromWireValue(entry.getString("sourceMode")),
                    com.lingxi.code.model.sessionModeFromWireValue(entry.getString("targetMode")),
                    entry.getString("cutoff"), entry.getString("requestId")))
            }
        }
        val workspacePresentationJson = json.getJSONObject("workspacePresentation")
        val workspacePresentation = buildMap {
            workspacePresentationJson.keys().forEach { key ->
                val entry = workspacePresentationJson.getJSONObject(key)
                put(
                    key,
                    WorkspacePresentationState(
                        pinnedAtEpochMillis = if (entry.isNull("pinnedAtEpochMillis")) {
                            null
                        } else {
                            entry.getLong("pinnedAtEpochMillis").also { require(it > 0L) }
                        },
                        collapsed = entry.getBoolean("collapsed"),
                    ),
                )
            }
        }
        val pinnedSessionsJson = json.getJSONObject("pinnedSessions")
        val pinnedSessions = buildMap {
            pinnedSessionsJson.keys().forEach { sessionId ->
                put(sessionId, pinnedSessionsJson.getLong(sessionId).also { require(it > 0L) })
            }
        }
        return Snapshot(
            activeMode = com.lingxi.code.model.sessionModeFromWireValue(json.getString("activeMode")),
            activeScopes = activeScopes,
            selectedMode = if (json.isNull("selectedMode")) null
                else com.lingxi.code.model.sessionModeFromWireValue(json.getString("selectedMode")),
            sessionOwners = sessionOwners,
            scopes = scopes,
            conversationDrafts = conversationDrafts,
            handoffs = handoffs,
            workspacePresentation = workspacePresentation,
            pinnedSessions = pinnedSessions,
            cronResultsSeenAtEpochMillis = json.getLong("cronResultsSeenAtEpochMillis"),
        )
    }

    companion object {
        internal const val SCOPE_STATE_FILE = "scope-state.json"
        private val stateMutex = Mutex()
        // All access is serialized by stateMutex, including across store instances.
        private val observedStateFiles = mutableSetOf<String>()
        private fun conversationDraftKey(scopeKey: String, sessionId: String) = "$scopeKey#session:$sessionId"
    }
}

/** JSON can escape isolated UTF-16 units; UTF-8 encoding alone replaces them. */
private fun String.escapeSurrogateCodeUnits(): String = buildString(length) {
    for (unit in this@escapeSurrogateCodeUnits) {
        if (unit.isSurrogate()) append("\\u").append(unit.code.toString(16).padStart(4, '0'))
        else append(unit)
    }
}
