package com.lingxi.code.conversation

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.QueuedTurnStatusDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch

internal data class DurableConversationTurnRecord(
    val scope: String,
    val sessionId: String,
    val turnId: Long,
    val admissionPending: Boolean = false,
)

internal data class DurableQueuedTurn(val sessionId: String, val turnId: Long)
internal data class DurableTerminalReceipt(val sessionId: String, val turnId: Long, val cancelledBeforeStart: Boolean = false)

internal fun confirmedQueuedMembers(
    queried: ULong, status: QueuedTurnStatusDto.Delivered, provenUuids: Set<String>, messages: List<Message>,
): List<Long> {
    val members = status.memberTurnIds
    if (members.isEmpty() || members.first() != status.batchOwnerTurnId || queried !in members ||
        members.toSet().size != members.size || members.any { it > Long.MAX_VALUE.toULong() } ||
        status.transcriptUuid !in provenUuids
    ) return emptyList()
    val row = messages.singleOrNull { it.role == Role.User && it.transcriptUuid == status.transcriptUuid }
        ?: return emptyList()
    if (messages.any { row.id in it.loopFoldedItemIds }) return emptyList()
    return members.map { it.toLong() }
}

/** Minimum non-secret launch context needed for service redelivery recovery. */
data class ConversationRecoverySpec(
    val projectId: String?,
    val hostPath: String?,
    val sessionMode: SessionMode,
    val linuxRuntimeMode: LinuxRuntimeMode,
    val workspaceKey: String? = null,
    val guestPath: String? = null,
    val additionalDirectories: List<String> = emptyList(),
) {
    val scopeKey: String
        get() = "${workspaceKey ?: hostPath ?: "__global__"}#${sessionMode.wireValue}"

    fun projectWorkspace(): ProjectWorkspace? {
        val path = hostPath ?: return null
        val guest = guestPath ?: return null
        return ProjectWorkspace(projectId = projectId ?: "recovered", hostPath = path, guestPath = guest, additionalDirectories = additionalDirectories)
    }
}

internal class DurableConversationTurnClientStore(
    private val preferences: SharedPreferences,
    private val scope: String,
    latestExit: ApplicationExitInfo? = null,
) {
    private val prefix = "scope_${java.security.MessageDigest.getInstance("SHA-256")
        .digest(scope.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it.toInt() and 0xff) }}_"
    private fun key(name: String) = prefix + name
    private fun identityPrefix(kind: String, sessionId: String): String {
        val hash = java.security.MessageDigest.getInstance("SHA-256")
            .digest(canonicalSessionId(sessionId).toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it.toInt() and 0xff) }
        return key("${kind}_${hash}_")
    }
    private fun identityKey(kind: String, sessionId: String, turnId: Long) = identityPrefix(kind, sessionId) + turnId

    companion object {
        private val storageLock = Any()
        // SharedPreferences commits rewrite the whole file. A failed commit
        // poisons every scope using this same in-memory preference object.
        private val failedPreferences = java.util.WeakHashMap<SharedPreferences, Boolean>()
    }

    private val storageFailed: Boolean get() = synchronized(storageLock) { failedPreferences[preferences] == true }
    fun isHealthy(): Boolean = !storageFailed

    private fun checkedCommit(edit: SharedPreferences.Editor): Boolean {
        if (storageFailed) return false
        val saved = edit.commit()
        if (!saved) failedPreferences[preferences] = true
        return saved
    }

    init {
        synchronized(storageLock) {
            migrateLegacyIfNeeded()
            val handledAt = maxOf(preferences.getLong(key("handled_exit_timestamp"), 0L),
                preferences.getLong("handled_exit_timestamp", 0L))
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && latestExit != null && latestExit.timestamp > handledAt) {
                if (latestExit.reason == ApplicationExitInfo.REASON_USER_REQUESTED) {
                    // A force-stop is not proof that output reached the
                    // transcript. Preserve every owner, but do not auto-resume.
                    markUncertainAfterUserExit()
                }
                checkedCommit(preferences.edit().putLong(key("handled_exit_timestamp"), latestExit.timestamp))
            }
        }
    }

    internal fun markUncertainAfterUserExit(): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val edit = preferences.edit()
        if (load() != null) edit.putBoolean(key("admission_pending"), true)
        val prefix = key("parked_")
        preferences.all.forEach { (name, value) ->
            if (name.startsWith(prefix) && value is String) {
                val turn = name.substringAfterLast('_').toLongOrNull() ?: return@forEach
                edit.putBoolean(identityKey("parked_pending", value, turn), true)
            }
        }
        checkedCommit(edit)
    }

    constructor(
        context: Context,
        scope: String,
    ) : this(
        preferences = context.applicationContext.getSharedPreferences(
            "durable_conversation_turn",
            Context.MODE_PRIVATE,
        ),
        scope = scope,
        latestExit = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            context.getSystemService(ActivityManager::class.java)
                .getHistoricalProcessExitReasons(context.packageName, 0, 1)
                .firstOrNull()
        } else {
            null
        },
    )

    fun begin(sessionId: String, turnId: Long, admissionPending: Boolean = false): Boolean {
        if (sessionId.isBlank() || storageFailed) return false
        return synchronized(storageLock) {
            migrateLegacyIfNeeded()
            if (!preferences.contains(key("scope")) && legacyRecord() != null) return@synchronized false
            checkedCommit(preferences.edit()
                .putString(key("scope"), scope)
                .putString(key("session_id"), sessionId)
                .putLong(key("turn_id"), turnId)
                .putBoolean(key("admission_pending"), admissionPending))
        }
    }

    fun confirmAdmission(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load()
        if (current != null && current.turnId == turnId && canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId)) {
            checkedCommit(preferences.edit().putBoolean(key("admission_pending"), false))
        } else if (preferences.getString(identityKey("parked", sessionId, turnId), null)?.let(::canonicalSessionId) == canonicalSessionId(sessionId)) {
            checkedCommit(preferences.edit().putBoolean(identityKey("parked_pending", sessionId, turnId), false))
        } else preferences.getString(identityKey("terminal", sessionId, turnId), null)
            ?.let(::canonicalSessionId) == canonicalSessionId(sessionId)
    }

    fun enqueue(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (sessionId.isBlank() || turnId <= 0 || storageFailed) return@synchronized false
        val entry = identityKey("queued", sessionId, turnId)
        val old = preferences.getString(entry, null)
        if (old != null && canonicalSessionId(old) != canonicalSessionId(sessionId)) return@synchronized false
        checkedCommit(preferences.edit().putString(entry, sessionId))
    }

    fun queued(sessionId: String): List<DurableQueuedTurn> = synchronized(storageLock) {
        val prefix = identityPrefix("queued", sessionId)
        preferences.all.mapNotNull { (name, value) ->
            val turn = name.removePrefix(prefix).takeIf { name.startsWith(prefix) }?.toLongOrNull() ?: return@mapNotNull null
            val owner = value as? String ?: return@mapNotNull null
            if (canonicalSessionId(owner) == canonicalSessionId(sessionId)) DurableQueuedTurn(owner, turn) else null
        }.sortedBy { it.turnId }
    }

    fun removeQueued(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val entry = identityKey("queued", sessionId, turnId)
        if (preferences.getString(entry, null)?.let(::canonicalSessionId) != canonicalSessionId(sessionId)) return@synchronized false
        checkedCommit(preferences.edit().remove(entry))
    }

    /** Only call after a matching Delivered response and visible transcript proof. */
    fun removeDeliveredQueued(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed || sessionId.isBlank() || turnId <= 0) return@synchronized false
        val entry = identityKey("queued", sessionId, turnId)
        val owner = preferences.getString(entry, null) ?: return@synchronized true
        if (canonicalSessionId(owner) != canonicalSessionId(sessionId)) return@synchronized false
        checkedCommit(preferences.edit().remove(entry))
    }

    /** Promote a proven queued owner and its checkpoint in one durable edit. */
    fun promoteQueued(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val entry = identityKey("queued", sessionId, turnId)
        if (preferences.getString(entry, null)?.let(::canonicalSessionId) != canonicalSessionId(sessionId) || load() != null) {
            return@synchronized false
        }
        checkedCommit(preferences.edit().putString(key("scope"), scope).putString(key("session_id"), sessionId)
            .putLong(key("turn_id"), turnId).putBoolean(key("admission_pending"), false).remove(entry))
    }

    fun recordTerminalEvidence(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (sessionId.isBlank() || storageFailed) return@synchronized false
        val entry = identityKey("terminal", sessionId, turnId)
        if (preferences.getString(entry, null)?.let(::canonicalSessionId) == canonicalSessionId(sessionId)) return@synchronized true
        checkedCommit(preferences.edit().putString(entry, sessionId))
    }

    /** Keep the recovery evidence while freeing the live slot for a queued turn. */
    fun finishTerminal(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load()
        if (current == null || current.turnId != turnId || canonicalSessionId(current.sessionId) != canonicalSessionId(sessionId)) {
            val parked = identityKey("parked", sessionId, turnId)
            if (preferences.getString(parked, null)?.let(::canonicalSessionId) == canonicalSessionId(sessionId)) {
                return@synchronized checkedCommit(preferences.edit().putString(identityKey("terminal", sessionId, turnId), sessionId)
                    .remove(parked).remove(identityKey("parked_pending", sessionId, turnId)))
            }
            return@synchronized preferences.getString(identityKey("terminal", sessionId, turnId), null)
                ?.let(::canonicalSessionId) == canonicalSessionId(sessionId)
        }
        checkedCommit(preferences.edit().putString(identityKey("terminal", sessionId, turnId), sessionId)
            .remove(key("scope")).remove(key("session_id")).remove(key("turn_id"))
            .remove(key("admission_pending")))
    }

    fun finishQueued(sessionId: String, turnId: Long, cancelledBeforeStart: Boolean = false): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val entry = identityKey("queued", sessionId, turnId)
        if (preferences.getString(entry, null)?.let(::canonicalSessionId) != canonicalSessionId(sessionId)) {
            return@synchronized preferences.getString(identityKey("terminal", sessionId, turnId), null)
                ?.let(::canonicalSessionId) == canonicalSessionId(sessionId) &&
                preferences.getBoolean(identityKey("terminal_cancelled", sessionId, turnId), false) == cancelledBeforeStart
        }
        checkedCommit(preferences.edit().putString(identityKey("terminal", sessionId, turnId), sessionId)
            .putBoolean(identityKey("terminal_cancelled", sessionId, turnId), cancelledBeforeStart).remove(entry))
    }

    fun terminalReceipts(sessionId: String): List<DurableTerminalReceipt> = synchronized(storageLock) {
        val prefix = identityPrefix("terminal", sessionId)
        preferences.all.mapNotNull { (name, value) ->
            val turn = name.removePrefix(prefix).takeIf { name.startsWith(prefix) }?.toLongOrNull() ?: return@mapNotNull null
            val owner = value as? String ?: return@mapNotNull null
            if (canonicalSessionId(owner) == canonicalSessionId(sessionId)) DurableTerminalReceipt(owner, turn,
                preferences.getBoolean(identityKey("terminal_cancelled", owner, turn), false)) else null
        }.sortedBy { it.turnId }
    }

    fun acknowledgeTerminals(sessionId: String, turns: List<Long>): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val edit = preferences.edit()
        turns.forEach { turn ->
            val entry = identityKey("terminal", sessionId, turn)
            if (preferences.getString(entry, null)?.let(::canonicalSessionId) == canonicalSessionId(sessionId)) {
                edit.remove(entry).remove(identityKey("terminal_cancelled", sessionId, turn))
            }
        }
        checkedCommit(edit)
    }

    fun parkCurrent(): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load() ?: return@synchronized true
        checkedCommit(preferences.edit().putString(identityKey("parked", current.sessionId, current.turnId), current.sessionId)
            .putBoolean(identityKey("parked_pending", current.sessionId, current.turnId), current.admissionPending)
            .remove(key("scope")).remove(key("session_id")).remove(key("turn_id"))
            .remove(key("admission_pending")))
    }

    fun restoreParked(sessionId: String): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load()
        if (current != null && canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId)) return@synchronized true
        if (current != null && !parkCurrent()) return@synchronized false
        val prefix = identityPrefix("parked", sessionId)
        val matches = preferences.all.mapNotNull { (name, value) ->
            val turn = name.removePrefix(prefix).takeIf { name.startsWith(prefix) }?.toLongOrNull() ?: return@mapNotNull null
            val owner = value as? String ?: return@mapNotNull null
            if (canonicalSessionId(owner) == canonicalSessionId(sessionId)) DurableConversationTurnRecord(scope, owner, turn,
                preferences.getBoolean(identityKey("parked_pending", owner, turn), false)) else null
        }
        if (matches.size > 1) return@synchronized false
        val record = matches.singleOrNull() ?: return@synchronized true
        checkedCommit(preferences.edit().putString(key("scope"), scope).putString(key("session_id"), record.sessionId)
            .putLong(key("turn_id"), record.turnId).putBoolean(key("admission_pending"), record.admissionPending)
            .remove(identityKey("parked", record.sessionId, record.turnId))
            .remove(identityKey("parked_pending", record.sessionId, record.turnId)))
    }

    fun load(): DurableConversationTurnRecord? = synchronized(storageLock) {
        migrateLegacyIfNeeded()
        if (!preferences.contains(key("scope"))) return@synchronized legacyRecord()
        if (preferences.getString(key("scope"), null) != scope) return@synchronized null
        val sessionId = preferences.getString(key("session_id"), null)?.takeIf(String::isNotBlank)
            ?: return@synchronized null
        if (!preferences.contains(key("turn_id"))) return@synchronized null
        DurableConversationTurnRecord(scope, sessionId, preferences.getLong(key("turn_id"), 0L),
            preferences.getBoolean(key("admission_pending"), false))
    }

    fun clear(turnId: Long? = null): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load() ?: return@synchronized true
        if (turnId == null || current.turnId == turnId) clearRecord() else false
    }

    /**
     * Clear only the exact session/turn checkpoint that produced a terminal
     * event.  Turn ids are client supplied, so the session comparison is
     * required as well: a late terminal event from a previous session must not
     * erase the checkpoint belonging to the current session.
     */
    fun clear(sessionId: String, turnId: Long): Boolean = synchronized(storageLock) {
        if (storageFailed) return@synchronized false
        val current = load() ?: return@synchronized true
        if (current.turnId == turnId && canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId)) {
            clearRecord()
        } else false
    }

    private fun clearRecord(): Boolean {
        val edit = preferences.edit()
            .remove(key("scope"))
            .remove(key("session_id"))
            .remove(key("turn_id"))
            .remove(key("admission_pending"))
        if (preferences.getString("scope", null) == scope) {
            edit.remove("scope").remove("session_id").remove("turn_id").remove("last_sequence")
        }
        return checkedCommit(edit)
    }

    private fun legacyRecord(): DurableConversationTurnRecord? {
        if (preferences.getString("scope", null) != scope) return null
        val session = preferences.getString("session_id", null)?.takeIf(String::isNotBlank) ?: return null
        if (!preferences.contains("turn_id")) return null
        return DurableConversationTurnRecord(scope, session, preferences.getLong("turn_id", 0L))
    }

    private fun migrateLegacyIfNeeded() {
        if (preferences.contains(key("scope")) || preferences.contains(key("session_id")) ||
            preferences.contains(key("turn_id"))) return
        val legacy = legacyRecord() ?: return
        checkedCommit(preferences.edit()
            .putString(key("scope"), scope)
            .putString(key("session_id"), legacy.sessionId)
            .putLong(key("turn_id"), legacy.turnId)
            .remove("scope").remove("session_id").remove("turn_id").remove("last_sequence"))
    }
}

/** A ViewModel has one active main-turn projection; the source owns shared leases. */
internal class ActiveRecoveryRevisionGuard {
    private var identity: Pair<String, ULong>? = null
    private var revision: Pair<ULong, TurnRecoveryStateDto>? = null
    fun accept(snapshot: TurnRecoverySnapshotDto): Boolean {
        val key = canonicalSessionId(snapshot.sessionId) to snapshot.turnId
        if (identity != key) { identity = key; revision = null }
        val previous = revision
        if (previous != null && (snapshot.revision < previous.first ||
                (snapshot.revision == previous.first && snapshot.state != previous.second))) return false
        if (previous == null || snapshot.revision > previous.first) revision = snapshot.revision to snapshot.state
        return true
    }
}

/** One engine's current main-turn interest; workflow and permission events are independent. */
internal class DurableTurnIngressInterest {
    private var turnId: ULong? = null
    private var sessionId: String? = null
    private var settled = false
    private var cancelling: Pair<String, ULong>? = null
    private var cancellationTerminalSeen = false
    private var cancellationTurnEndSeen = false
    private var failedCancellationTerminal: Pair<String, ULong>? = null
    private val queued = mutableMapOf<ULong, String?>()

    fun begin(session: String?, turn: Long) {
        turnId = turn.toULong()
        sessionId = session?.let(::canonicalSessionId)
        settled = false
        cancelling = null
        cancellationTerminalSeen = false
        cancellationTurnEndSeen = false
        failedCancellationTerminal = null
    }

    fun accepts(session: String, turn: ULong, activeSession: String?): Boolean {
        val canonical = canonicalSessionId(session)
        if (turnId != turn || canonical.isBlank()) return false
        val active = activeSession?.let(::canonicalSessionId) ?: return false
        if (canonical != active) return false
        val bound = sessionId
        if (bound == null) sessionId = canonical
        return bound == null || bound == canonical
    }

    fun currentIdentity(): Pair<String, ULong>? = sessionId?.let { session -> turnId?.let { session to it } }

    fun retire(session: String, turn: ULong) {
        if (turnId == turn && sessionId == canonicalSessionId(session)) {
            turnId = null
            sessionId = null
            settled = true
        }
    }

    fun enqueue(session: String?, turn: ULong) {
        queued[turn] = session?.let(::canonicalSessionId)
    }

    fun allowQueuedWhenNoOwner() { if (turnId == null) settled = true }

    fun removeQueued(turn: ULong) { queued.remove(turn) }

    fun isQueued(session: String, turn: ULong, activeSession: String?): Boolean {
        if (!queued.containsKey(turn)) return false
        val canonical = canonicalSessionId(session)
        if (canonical.isBlank() || canonical != activeSession?.let(::canonicalSessionId)) return false
        return queued[turn]?.let { it == canonical } ?: true
    }

    fun beginCancellation(session: String, turn: ULong) {
        if (turnId == turn && sessionId == canonicalSessionId(session)) {
            cancelling = canonicalSessionId(session) to turn
            cancellationTerminalSeen = false
            cancellationTurnEndSeen = false
        }
    }

    fun isCancelling(session: String, turn: ULong): Boolean =
        cancelling == (canonicalSessionId(session) to turn)

    fun cancellationHasPendingTerminal(session: String, turn: ULong): Boolean =
        isCancelling(session, turn) || failedCancellationTerminal == (canonicalSessionId(session) to turn)

    fun abortCancellation(session: String, turn: ULong): Pair<Boolean, Boolean> {
        if (!isCancelling(session, turn)) return false to false
        cancelling = null
        val observed = cancellationTerminalSeen
        val ended = cancellationTurnEndSeen
        cancellationTerminalSeen = false
        cancellationTurnEndSeen = false
        if (observed) failedCancellationTerminal = canonicalSessionId(session) to turn
        return observed to ended
    }

    fun finishFailedCancellationAfterTurnEnd(session: String, turn: ULong): Boolean {
        if (failedCancellationTerminal != (canonicalSessionId(session) to turn)) return false
        failedCancellationTerminal = null
        settled = true
        return true
    }

    fun finishCancellation(session: String, turn: ULong): Boolean {
        if (!isCancelling(session, turn)) return false
        cancelling = null
        val observed = cancellationTerminalSeen
        cancellationTerminalSeen = false
        cancellationTurnEndSeen = false
        if (observed) settled = true
        return observed
    }

    fun markSettled(session: String, turn: ULong, recoverySnapshot: Boolean = false) {
        if (turnId != turn || sessionId != canonicalSessionId(session)) return
        if (cancellationHasPendingTerminal(session, turn)) {
            if (recoverySnapshot) cancellationTerminalSeen = true
            else cancellationTurnEndSeen = true
        } else settled = true
    }

    fun canAdopt(session: String, turn: ULong, activeSession: String?): Boolean {
        if (!settled || !queued.containsKey(turn)) return false
        val canonical = canonicalSessionId(session)
        if (canonical != activeSession?.let(::canonicalSessionId)) return false
        val expected = queued[turn]
        return expected == null || expected == canonical
    }

    fun adopt(session: String, turn: ULong) {
        queued.remove(turn)
        turnId = turn
        sessionId = canonicalSessionId(session)
        settled = false
        cancelling = null
        cancellationTerminalSeen = false
        cancellationTurnEndSeen = false
        failedCancellationTerminal = null
    }

    fun reset() {
        turnId = null
        sessionId = null
        settled = false
        cancelling = null
        cancellationTerminalSeen = false
        cancellationTurnEndSeen = false
        failedCancellationTerminal = null
        queued.clear()
    }
}

/**
 * One process-wide serialization point for native callback side effects.
 * Sources that overlap on one durable store share a watermark while any of
 * them still owns the identity. Retirement fences future callbacks first;
 * already-enqueued deliveries retain their lease until consumers drain them.
 */
internal class DurableRecoveryIngress {
    private val lock = Any()
    internal data class Key(val store: String, val session: String, val turn: ULong)
    class Lease internal constructor(internal val key: Key) { internal var active = true }
    private data class Entry(val leases: MutableSet<Lease>, var revision: Pair<ULong, TurnRecoveryStateDto>? = null)
    private val entries = mutableMapOf<Key, Entry>()
    private val pending = java.util.ArrayDeque<() -> Unit>()
    private var draining = false

    fun newSource(store: String = "test-store"): Source = Source(store)
    fun watermarkCountForTesting(): Int = synchronized(lock) { entries.size }

    inner class Source(private val store: String) {
        private var retired = false
        private val current = mutableMapOf<Pair<String, ULong>, Lease>()
        private val leases = mutableSetOf<Lease>()

        fun acquireInterest(session: String, turn: ULong): Lease? = synchronized(lock) {
            if (retired) return@synchronized null
            val identity = canonicalSessionId(session) to turn
            current[identity]?.takeIf { it.active }?.let { return@synchronized it }
            val key = Key(store, identity.first, turn)
            val lease = Lease(key)
            entries.getOrPut(key) { Entry(mutableSetOf()) }.leases.add(lease)
            leases.add(lease)
            current[identity] = lease
            lease
        }

        /** Stops new callbacks for this lease; queued consumers can still drain it. */
        fun detachInterest(session: String, turn: ULong): Lease? = synchronized(lock) {
            current.remove(canonicalSessionId(session) to turn)
        }

        fun fence(session: String, turn: ULong): () -> Boolean {
            val lease = synchronized(lock) { current[canonicalSessionId(session) to turn] }
            return { synchronized(lock) { !retired && lease != null && lease.active && lease in leases } }
        }

        fun releaseInterest(lease: Lease) = synchronized(lock) {
            if (!leases.remove(lease)) return@synchronized
            lease.active = false
            if (current[lease.key.session to lease.key.turn] === lease) current.remove(lease.key.session to lease.key.turn)
            entries[lease.key]?.let { entry ->
                entry.leases.remove(lease)
                if (entry.leases.isEmpty()) entries.remove(lease.key)
            }
        }

        fun dispatch(
            event: ClientEvent,
            admit: () -> Boolean = { true },
            onRevisionRejected: () -> Unit = {},
            apply: () -> Unit,
        ): Boolean = enqueue(event, admit, onRevisionRejected, apply)

        fun dispatchBoundary(apply: () -> Unit): Boolean = enqueue(null, { true }, {}, apply)

        private fun enqueue(event: ClientEvent?, admit: () -> Boolean, onRevisionRejected: () -> Unit, apply: () -> Unit): Boolean = synchronized(lock) {
            if (retired) return@synchronized false
            var accepted = false
            pending.addLast {
                if (!retired && admit()) {
                    val snapshot = when (event) {
                        is ClientEvent.TurnRecoveryState -> event.snapshot
                        is ClientEvent.TurnAttachStarted -> event.snapshot
                        else -> null
                    }
                    val acceptedRevision = if (snapshot == null) true else {
                        val identity = canonicalSessionId(snapshot.sessionId) to snapshot.turnId
                        val lease = current[identity]
                        val entry = lease?.let { entries[it.key] }
                        val previous = entry?.revision
                        if (lease == null || !lease.active || entry == null ||
                            (previous != null && snapshot.revision < previous.first) ||
                            (previous != null && snapshot.revision == previous.first && snapshot.state != previous.second)
                        ) false else {
                            if (previous == null || snapshot.revision > previous.first) entry.revision = snapshot.revision to snapshot.state
                            true
                        }
                    }
                    if (acceptedRevision) {
                        accepted = true
                        apply()
                    } else onRevisionRejected()
                }
            }
            // A Java monitor is reentrant. Queue nested callbacks instead of
            // allowing a newer snapshot to finish inside an older projection.
            if (draining) return@synchronized true
            draining = true
            var failure: Throwable? = null
            try {
                while (pending.isNotEmpty()) {
                    try {
                        pending.removeFirst().invoke()
                    } catch (error: Throwable) {
                        if (failure == null) failure = error
                    }
                }
            } finally {
                draining = false
            }
            failure?.let { throw it }
            accepted
        }

        fun retire() = synchronized(lock) {
            retired = true
            leases.toList().forEach(::releaseInterest)
            current.clear()
        }

        fun isActive(): Boolean = synchronized(lock) { !retired }
    }
}

internal class DurableTurnReplayGate {
    private var pendingColdResumeAttach: Pair<String, Long>? = null
    private var suppressingRetainedReplay: Pair<String, Long>? = null
    private var awaitingTerminalConfirmation: Pair<String, Long>? = null

    @Synchronized
    fun noteAttachRequested(
        sessionId: String,
        turnId: Long,
        activation: ActivatedSession?,
        afterSequence: Long,
    ) {
        val key = canonicalSessionId(sessionId) to turnId
        pendingColdResumeAttach =
            key.takeIf {
                afterSequence <= 0L &&
                    activation?.kind == SessionActivationKind.Resumed
            }
        if (pendingColdResumeAttach != key) {
            clearTurn(key)
        }
    }

    @Synchronized
    fun cancelAttach(sessionId: String, turnId: Long) {
        if (pendingColdResumeAttach == (canonicalSessionId(sessionId) to turnId)) pendingColdResumeAttach = null
    }

    @Synchronized
    fun finishAttach(sessionId: String, turnId: Long) {
        clearTurn(canonicalSessionId(sessionId) to turnId)
    }

    @Synchronized
    fun shouldForward(event: ClientEvent): Boolean = when (event) {
        is ClientEvent.TurnRecoveryState -> {
            val key = canonicalSessionId(event.snapshot.sessionId) to event.snapshot.turnId.toLong()
            val terminal = event.snapshot.state in terminalStates
            when {
                pendingColdResumeAttach == key -> {
                    pendingColdResumeAttach = null
                    if (terminal) {
                        suppressingRetainedReplay = key
                        awaitingTerminalConfirmation = key
                    } else {
                        clearTurn(key)
                    }
                }
                awaitingTerminalConfirmation == key && terminal -> clearTurn(key)
            }
            true
        }
        is ClientEvent.TurnEventReplay ->
            suppressingRetainedReplay != (canonicalSessionId(event.sessionId) to event.turnId.toLong())
        is ClientEvent.Error -> {
            if (!isCompactTurnRejection(event.message.displayText())) clearAll()
            true
        }
        is ClientEvent.TurnEnded -> {
            clearAll()
            true
        }
        else -> true
    }

    private fun clearTurn(key: Pair<String, Long>) {
        if (pendingColdResumeAttach == key) pendingColdResumeAttach = null
        if (suppressingRetainedReplay == key) suppressingRetainedReplay = null
        if (awaitingTerminalConfirmation == key) awaitingTerminalConfirmation = null
    }

    private fun clearAll() {
        pendingColdResumeAttach = null
        suppressingRetainedReplay = null
        awaitingTerminalConfirmation = null
    }
    private companion object {
        val terminalStates = setOf(
            TurnRecoveryStateDto.COMPLETED,
            TurnRecoveryStateDto.FAILED,
            TurnRecoveryStateDto.CANCELLED,
        )
    }
}

/**
 * Single-flight ownership for durable-turn attachment on one engine source.
 *
 * The headless service and a newly-created UI can observe the same
 * SessionResumed activation.  They must not both submit the same AttachTurn /
 * ResumeTurn pair, but a UI takeover is allowed to request a new replay from
 * its own cursor.  The key therefore includes the canonical session, turn,
 * and cursor, while ownership is tracked separately from the key.
 */
internal enum class DurableAttachOwner {
    Headless,
    Ui,
}

internal data class DurableAttachRequest(
    val sessionId: String,
    val turnId: Long,
    val afterSequence: Long,
)

/** Correlates wire batch boundaries; replay envelopes themselves have no request id. */
internal class DurableAttachIngress {
    data class EnvelopeRoute(val accepted: Boolean, val batchRequestId: String? = null, val excludedRequestId: String? = null)
    data class Attempt(
        val sessionId: String,
        val turnId: ULong,
        val requestId: String,
        val uiOwner: Boolean,
        val expectsResume: Boolean,
        val admissionPending: Boolean = false,
        val consumed: kotlinx.coroutines.CompletableDeferred<Boolean> = kotlinx.coroutines.CompletableDeferred(),
        var barrier: AttachConsumptionBarrier? = null,
        var started: Boolean = false,
        var finished: Boolean = false,
        var incomplete: Boolean = false,
        var terminalObserved: Boolean = false,
        val pendingSnapshots: MutableList<ClientEvent.TurnRecoveryState> = mutableListOf(),
    )

    private var current: Attempt? = null
    private val wireBatches = mutableMapOf<Pair<String, ULong>, String>()
    private val bootstrapRequired = mutableSetOf<Pair<String, ULong>>()
    private val established = mutableSetOf<Pair<String, ULong>>()

    fun begin(sessionId: String, turnId: ULong, requestId: String, uiOwner: Boolean, expectsResume: Boolean,
        existingOwner: Boolean = false, admissionPending: Boolean = false): Attempt {
        val session = canonicalSessionId(sessionId)
        val key = session to turnId
        if (existingOwner) established.add(key) else bootstrapRequired.add(key)
        return Attempt(session, turnId, requestId, uiOwner, expectsResume, admissionPending).also { current = it }
    }

    fun noteLiveOwnership(sessionId: String, turnId: ULong) {
        established.add(canonicalSessionId(sessionId) to turnId)
    }

    fun isAdmissionPending(requestId: String): Boolean = current?.let {
        it.requestId == requestId && it.admissionPending
    } == true

    fun hasEstablishedOwner(sessionId: String, turnId: ULong): Boolean =
        (canonicalSessionId(sessionId) to turnId) in established

    fun started(requestId: String, snapshot: TurnRecoverySnapshotDto, belongsToSource: Boolean): Boolean {
        val key = canonicalSessionId(snapshot.sessionId) to snapshot.turnId
        if (!belongsToSource) return false
        wireBatches[key] = requestId
        val attempt = current ?: return false
        if (key != (attempt.sessionId to attempt.turnId)) return false
        // Remember an obsolete batch too: its replay envelopes lack an id and
        // must not be mistaken for live output belonging to this attempt.
        if (requestId != attempt.requestId) return false
        attempt.started = true
        return true
    }

    fun finished(requestId: String, sessionId: String, turnId: ULong): Attempt? {
        val key = canonicalSessionId(sessionId) to turnId
        if (wireBatches[key] != requestId) return null
        wireBatches.remove(key)
        val attempt = current ?: return null
        if (key != (attempt.sessionId to attempt.turnId) || requestId != attempt.requestId || !attempt.started) return null
        attempt.finished = true
        return attempt
    }

    fun routeEnvelope(sessionId: String, turnId: ULong): EnvelopeRoute {
        val key = canonicalSessionId(sessionId) to turnId
        val batch = wireBatches[key]
        val attempt = current
        if (batch != null) return if (attempt != null && key == (attempt.sessionId to attempt.turnId) &&
            batch == attempt.requestId && attempt.started) EnvelopeRoute(true, batchRequestId = batch) else EnvelopeRoute(false)
        if (attempt == null || key != (attempt.sessionId to attempt.turnId)) {
            return EnvelopeRoute(key !in bootstrapRequired)
        }
        // Live events received before Started are visible to existing UI
        // projections; the new owner gets the journal replay or an explicit gap.
        if (!attempt.started && !attempt.finished) return EnvelopeRoute(true, excludedRequestId = attempt.requestId)
        return EnvelopeRoute(attempt.finished)
    }

    fun mayForwardEnvelope(sessionId: String, turnId: ULong): Boolean = routeEnvelope(sessionId, turnId).accepted

    fun bufferSnapshot(event: ClientEvent.TurnRecoveryState): Boolean {
        val key = canonicalSessionId(event.snapshot.sessionId) to event.snapshot.turnId
        val attempt = current
        if (attempt == null || key != (attempt.sessionId to attempt.turnId)) return key in bootstrapRequired
        if (attempt.finished) return false
        if (key in established) return false // another owner still receives live state
        if (attempt.pendingSnapshots.size >= 512) attempt.incomplete = true
        else attempt.pendingSnapshots.add(event)
        return true
    }

    fun noteUnretainedBeforeBootstrap(sessionId: String, turnId: ULong) {
        current?.takeIf { it.sessionId == canonicalSessionId(sessionId) && it.turnId == turnId && !it.finished }
            ?.incomplete = true
    }

    fun rejectStarted() {
        current?.let { it.started = false; it.incomplete = true; it.consumed.complete(false) }
    }

    fun confirmStarted(snapshot: TurnRecoverySnapshotDto) {
        val key = canonicalSessionId(snapshot.sessionId) to snapshot.turnId
        bootstrapRequired.remove(key)
        established.add(key)
        if (snapshot.state in setOf(TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED)) {
            current?.takeIf { (it.sessionId to it.turnId) == key }?.terminalObserved = true
        }
    }

    fun noteTerminal(snapshot: TurnRecoverySnapshotDto): Boolean {
        val attempt = current ?: return false
        if (attempt.sessionId != canonicalSessionId(snapshot.sessionId) || attempt.turnId != snapshot.turnId) return false
        attempt.terminalObserved = true
        return true
    }

    fun forgetIdentity(sessionId: String, turnId: ULong) {
        val key = canonicalSessionId(sessionId) to turnId
        wireBatches.remove(key)
        bootstrapRequired.remove(key)
        established.remove(key)
    }

    fun awaitingBootstrap(sessionId: String, turnId: ULong): Boolean = current?.let {
        it.sessionId == canonicalSessionId(sessionId) && it.turnId == turnId && !it.started
    } == true

    fun end(attempt: Attempt) {
        if (current === attempt) current = null
        // A known wire batch stays fenced until its matching Finished or a
        // later authoritative Started replaces it.
    }
}

/** A durable attach/resume failed after the checkpoint was already identified. */
internal class DurableAttachFailure(
    val turnId: Long,
    val phase: String,
    cause: Throwable,
) : IllegalStateException("Unable to $phase durable turn $turnId", cause) {
    var orderedDelivery: Boolean = false
}

/** Resume is needed for fresh UI/cold-headless sources, not retained takeovers. */
internal fun shouldResumeDurableTurn(
    forceAttach: Boolean,
    resumeOnUiAttach: Boolean,
    admissionPending: Boolean = false,
): Boolean = !admissionPending && (!forceAttach || resumeOnUiAttach)

internal class DurableAttachCoordinator {
    private var claimedByUi = false
    private var lastOwner: DurableAttachOwner? = null
    private var lastRequest: DurableAttachRequest? = null

    /** Claim the source for UI attachment; subsequent headless requests skip. */
    @Synchronized
    fun claimForUi() {
        claimedByUi = true
    }

    /** Release the UI claim when the Activity is gone and service monitoring resumes. */
    @Synchronized
    fun releaseToHeadless() {
        claimedByUi = false
    }

    /**
     * Reserve one command pair. A UI takeover is intentionally a new owner,
     * even when it starts at cursor zero, because the service's SharedFlow
     * collector may have consumed that earlier replay.
     */
    @Synchronized
    fun reserve(request: DurableAttachRequest, owner: DurableAttachOwner): Boolean {
        if (owner == DurableAttachOwner.Headless && claimedByUi) return false
        if (lastOwner == owner && lastRequest == request) return false
        lastOwner = owner
        lastRequest = request
        return true
    }

    @Synchronized
    fun rollback(request: DurableAttachRequest, owner: DurableAttachOwner) {
        if (lastOwner == owner && lastRequest == request) {
            lastOwner = null
            lastRequest = null
        }
    }

    internal fun ownerForTesting(): DurableAttachOwner? = synchronized(this) { lastOwner }
}

internal class RecoveringConversationSource(
    private val pendingSource: Deferred<ConversationSource?>,
    private val strings: ConversationStrings,
    override val recoverySpec: ConversationRecoverySpec,
) : ConversationSource, SlashCommandConversationSource, BackgroundRetainableConversationSource {
    override val orderedCancellationBoundaries: Boolean get() = retainedSource?.orderedCancellationBoundaries == true
    override val supportsSubmissionAdmission: Boolean get() = true
    override suspend fun submissionSession(): String? = pendingSource.await()?.submissionSession()
    private val delegateScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val models = MutableStateFlow(EngineModelState())
    private val sessions = MutableStateFlow(EngineSessionState.loading())
    private val activeSession = MutableStateFlow<ActivatedSession?>(null)
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private val mcp = MutableStateFlow(emptyList<MCPServer>())
    @Volatile private var retainedSource: ConversationSource? = null
    @Volatile private var closed = false

    init {
        delegateScope.launch {
            val delegate = pendingSource.await() ?: run {
                delegateScope.cancel()
                return@launch
            }
            if (closed) {
                delegate.close()
                delegateScope.cancel()
                return@launch
            }
            retainedSource = delegate
            launch { delegate.modelState.collect { models.value = it } }
            launch { delegate.sessionState.collect { sessions.value = it } }
            launch { delegate.activeSessionState.collect { activeSession.value = it } }
            launch { delegate.pendingPermission.collect { permissions.value = it } }
            launch { delegate.mcpServers.collect { mcp.value = it } }
        }
    }

    override val clientEvents: Flow<ClientEvent> = flow {
        pendingSource.await()?.clientEvents?.let { emitAll(it) }
    }

    override val conversationDeliveries: Flow<ConversationDelivery> = flow {
        pendingSource.await()?.conversationDeliveries?.let { emitAll(it) }
    }

    override val workflowProgress: Flow<WorkflowProgressUpdate> = flow {
        pendingSource.await()?.workflowProgress?.let { emitAll(it) }
    }

    override val pendingPermission: StateFlow<PermissionPromptState?> = permissions.asStateFlow()
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()
    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

    override suspend fun submitClientCommand(command: ClientCommand) {
        pendingSource.await()?.submitClientCommand(command)
    }

    override suspend fun refreshExecutionStatus() {
        pendingSource.await()?.refreshExecutionStatus()
    }

    override suspend fun attachDurableTurnForUi(
        afterSequence: Long,
        onAttach: (String, Long, Boolean, String) -> Unit,
    ) {
        pendingSource.await()?.attachDurableTurnForUi(afterSequence, onAttach)
    }

    override suspend fun reconcileDurableTurnForUi(sessionId: String, turnId: Long, afterSequence: Long, trustNotFound: Boolean,
        onAttach: (String, Long, Boolean, String) -> Unit) {
        val delegate = pendingSource.await() ?: error("engine unavailable")
        retainedSource = delegate
        delegate.reconcileDurableTurnForUi(sessionId, turnId, afterSequence, trustNotFound, onAttach)
    }

    override suspend fun terminalRecoveryReceipts(sessionId: String): List<Long> =
        pendingSource.await()?.terminalRecoveryReceipts(sessionId).orEmpty()

    override suspend fun cancelledQueuedReceipts(sessionId: String): List<Long> =
        pendingSource.await()?.cancelledQueuedReceipts(sessionId).orEmpty()

    override suspend fun pendingQueuedRecoveryTurns(sessionId: String): List<Long> =
        pendingSource.await()?.pendingQueuedRecoveryTurns(sessionId).orEmpty()

    override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) {
        val delegate = pendingSource.await() ?: error("engine unavailable")
        delegate.queryQueuedRecovery(sessionId, turnId, requestId)
    }

    override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean =
        pendingSource.await()?.resolveQueuedRecovery(sessionId, turns, resolution) == true

    override suspend fun acknowledgeTerminalRecovery(sessionId: String, turns: List<Long>): Boolean =
        pendingSource.await()?.acknowledgeTerminalRecovery(sessionId, turns) == true

    override suspend fun recoveryStorageAvailable(): Boolean = pendingSource.await()?.recoveryStorageAvailable() == true

    override fun initialMessages(): List<Message> = emptyList()

    override suspend fun refreshSessions() {
        pendingSource.await()?.refreshSessions()
    }

    override suspend fun resumeSession(uuid: String) {
        pendingSource.await()?.resumeSession(uuid)
    }

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        pendingSource.await()?.resumeEmptySession(uuid, title)
    }

    override suspend fun refreshMcpServers() {
        pendingSource.await()?.refreshMcpServers()
    }

    override suspend fun newSession() {
        pendingSource.await()?.newSession()
    }

    override suspend fun setPermissionMode(mode: String) {
        val source = pendingSource.await() ?: error("engine is not connected")
        source.setPermissionMode(mode)
    }

    override suspend fun setModel(id: String) {
        val source = pendingSource.await() ?: error("engine is not connected")
        source.setModel(id)
    }

    override suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {
        pendingSource.await()?.approvePermission(requestId, response)
    }

    override suspend fun denyPermission(requestId: ULong) {
        pendingSource.await()?.denyPermission(requestId)
    }

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submit(text, images, turnId, null, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
    ): Flow<ReplyEvent> = submit(text, images, turnId, visualizationContext, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
    ): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        retainedSource = delegate
        emitAll(delegate.submit(text, images, turnId, visualizationContext, promptMode))
    }

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long, visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?, sessionId: String, admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        retainedSource = delegate
        emitAll(delegate.submit(text, images, turnId, visualizationContext, promptMode, sessionId, admission))
    }

    override fun submitSlashCommand(
        raw: ClientTextDto,
        images: List<ImageRefDto>,
        turnId: Long,
    ): Flow<ReplyEvent> = flow {
        val active = pendingSource.await()
        val delegate = active as? SlashCommandConversationSource
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        retainedSource = active
        emitAll(delegate.submitSlashCommand(raw, images, turnId))
    }

    override fun submitSlashCommand(raw: ClientTextDto, images: List<ImageRefDto>, turnId: Long,
        sessionId: String, admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
        val active = pendingSource.await()
        val delegate = active as? SlashCommandConversationSource
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        retainedSource = active
        emitAll(delegate.submitSlashCommand(raw, images, turnId, sessionId, admission))
    }

    override suspend fun abandonUnsubmittedTurn(sessionId: String?, turnId: Long) {
        retainedSource?.abandonUnsubmittedTurn(sessionId, turnId)
    }

    override suspend fun cancel(turnId: Long?) {
        pendingSource.await()?.cancel(turnId)
    }

    override suspend fun cancelForSession(sessionId: String, turnId: Long?) {
        pendingSource.await()?.cancelForSession(sessionId, turnId)
    }

    override suspend fun discardDurableTurnForSession(sessionId: String, turnId: Long) {
        pendingSource.await()?.discardDurableTurnForSession(sessionId, turnId)
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        pendingSource.await()?.discardDurableTurn(turnId)
    }

    override fun close() {
        closed = true
        retainedSource?.let {
            it.close()
            delegateScope.cancel()
        }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource? =
        (retainedSource as? BackgroundRetainableConversationSource)?.engineSourceForBackgroundRetention()
}
