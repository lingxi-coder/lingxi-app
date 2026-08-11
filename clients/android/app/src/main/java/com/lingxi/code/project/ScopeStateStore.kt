package com.lingxi.code.project

import android.content.Context
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
 * Project/global last-active-session persistence stays with [ProjectRepository]
 * (`ProjectRecord.lastActiveSessionId` / the global session index) — this store
 * is what gives LOCAL-APP scopes the same durability, and it is the authority
 * for "which scope was active" across process death.
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

    suspend fun readActiveScopeKey(): String? = withContext(Dispatchers.IO) {
        mutex.withLock { load().activeScopeKey }
    }

    suspend fun read(scopeKey: String): ScopeState? = withContext(Dispatchers.IO) {
        mutex.withLock { load().scopes[scopeKey] }
    }

    suspend fun persistActiveScope(scopeKey: String) {
        mutate { it.copy(activeScopeKey = scopeKey) }
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

    private data class Snapshot(
        val activeScopeKey: String? = null,
        val scopes: Map<String, ScopeState> = emptyMap(),
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
        return Snapshot(
            activeScopeKey = if (json.isNull("activeScopeKey")) {
                null
            } else {
                json.optString("activeScopeKey").takeIf { it.isNotBlank() }
            },
            scopes = scopes,
        )
    }

    companion object {
        internal const val SCOPE_STATE_FILE = "scope-state.json"
    }
}
