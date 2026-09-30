package com.lingxi.code.localapps

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.webkit.WebStorage
import java.net.URI
import java.util.Base64
import kotlinx.coroutines.delay

interface LocalAppWebStorageCleanup {
    /** Persist the app's exact origin before a delete command can succeed. */
    fun prepareDeletion(appId: String, currentUrl: String?): Boolean

    /** Cancel only a not-yet-confirmed journal after command submission fails. */
    fun cancelDeletion(appId: String)

    /** Reconcile against the authoritative, complete AppsChanged id set. */
    fun reconcile(liveAppIds: Set<String>)

    /** Retry cleanups that were confirmed before the previous process exited. */
    fun retryConfirmed()
}

internal object NoopLocalAppWebStorageCleanup : LocalAppWebStorageCleanup {
    override fun prepareDeletion(appId: String, currentUrl: String?) = true
    override fun cancelDeletion(appId: String) = Unit
    override fun reconcile(liveAppIds: Set<String>) = Unit
    override fun retryConfirmed() = Unit
}

internal data class PendingLocalAppWebStorageCleanup(
    val appId: String,
    val origin: String,
    val confirmedDeleted: Boolean,
)

/** Pure persisted-state model; Android I/O is kept in the wrapper below. */
internal data class LocalAppWebStorageCleanupLedger(
    val origins: Map<String, String> = emptyMap(),
    val pending: List<PendingLocalAppWebStorageCleanup> = emptyList(),
) {
    fun remembering(appId: String, origin: String): LocalAppWebStorageCleanupLedger =
        copy(origins = origins + (appId to origin))

    fun hasPendingOrigin(origin: String): Boolean = pending.any { it.origin == origin }

    /**
     * A submitted deletion must not wedge the still-live app if the engine
     * later refuses it. The same app may reopen its own unconfirmed origin;
     * confirmed cleanup and every cross-app origin reuse remain blocked.
     */
    fun blocksOriginClaim(appId: String, origin: String): Boolean = pending.any { entry ->
        entry.origin == origin && (entry.confirmedDeleted || entry.appId != appId)
    }

    fun preparing(appId: String, explicitOrigin: String?): LocalAppWebStorageCleanupLedger {
        val origin = explicitOrigin ?: origins[appId] ?: return this
        val entry = PendingLocalAppWebStorageCleanup(appId, origin, confirmedDeleted = false)
        return copy(
            origins = origins + (appId to origin),
            // A same-id recreation can race an older confirmed purge. Keep
            // confirmed work durable; replace only a still-live delete intent.
            pending = pending.filterNot { it.appId == appId && !it.confirmedDeleted } + entry,
        )
    }

    fun confirmingMissing(liveAppIds: Set<String>): LocalAppWebStorageCleanupLedger = copy(
        pending = pending.map { entry ->
            if (entry.appId !in liveAppIds) entry.copy(confirmedDeleted = true) else entry
        },
    )

    fun cancellingUnconfirmed(appId: String): LocalAppWebStorageCleanupLedger = copy(
        pending = pending.filterNot { it.appId == appId && !it.confirmedDeleted },
    )

    fun completed(entry: PendingLocalAppWebStorageCleanup): LocalAppWebStorageCleanupLedger = copy(
        origins = if (entry in pending && origins[entry.appId] == entry.origin) origins - entry.appId else origins,
        pending = if (entry in pending) pending - entry else pending,
    )

    fun encode(): String = buildString {
        appendLine("v1")
        origins.toSortedMap().forEach { (appId, origin) ->
            append("O\t").append(encoded(appId)).append('\t').append(encoded(origin)).appendLine()
        }
        pending.forEach { entry ->
            append("P\t")
                .append(encoded(entry.appId)).append('\t')
                .append(encoded(entry.origin)).append('\t')
                .append(if (entry.confirmedDeleted) '1' else '0')
                .appendLine()
        }
    }

    companion object {
        fun decode(raw: String?): LocalAppWebStorageCleanupLedger {
            if (raw.isNullOrBlank()) return LocalAppWebStorageCleanupLedger()
            return runCatching {
                if (raw.lineSequence().firstOrNull() != "v1") return LocalAppWebStorageCleanupLedger()
                val origins = linkedMapOf<String, String>()
                val pending = mutableListOf<PendingLocalAppWebStorageCleanup>()
                raw.lineSequence().drop(1).forEach { line ->
                    val fields = line.split('\t')
                    when {
                        fields.size == 3 && fields[0] == "O" -> {
                            val appId = decoded(fields[1]).takeIf { it.isNotBlank() } ?: return@forEach
                            val origin = trustedLoopbackOrigin(decoded(fields[2])) ?: return@forEach
                            origins[appId] = origin
                        }
                        fields.size == 4 && fields[0] == "P" -> {
                            val appId = decoded(fields[1]).takeIf { it.isNotBlank() } ?: return@forEach
                            val origin = trustedLoopbackOrigin(decoded(fields[2])) ?: return@forEach
                            pending += PendingLocalAppWebStorageCleanup(
                                appId = appId,
                                origin = origin,
                                confirmedDeleted = fields[3] == "1",
                            )
                        }
                    }
                }
                LocalAppWebStorageCleanupLedger(origins, pending)
            }.getOrDefault(LocalAppWebStorageCleanupLedger())
        }

        private fun encoded(value: String): String =
            Base64.getUrlEncoder().withoutPadding().encodeToString(value.toByteArray(Charsets.UTF_8))

        private fun decoded(value: String): String =
            String(Base64.getUrlDecoder().decode(value), Charsets.UTF_8)
    }
}

internal class AndroidLocalAppWebStorageCleanup private constructor(context: Context) : LocalAppWebStorageCleanup {
    private val preferences = context.applicationContext.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    private val mainHandler = Handler(Looper.getMainLooper())
    private val purgesInFlight = mutableSetOf<Pair<String, String>>()

    /**
     * Atomically claims an origin for a WebView, but only after every deletion
     * previously journaled for that origin has been verified complete.
     *
     * Loopback ports are reusable. Loading first and deleting later would let a
     * replacement app observe the predecessor's storage, while deleting after
     * the replacement starts could erase its newly written data. This suspend
     * gate keeps the WebView uncreated until WebStorage's origin registry proves
     * the old origin is gone.
     */
    suspend fun awaitOriginReadyAndRemember(appId: String, origin: String): Boolean {
        val canonicalOrigin = trustedLoopbackOrigin(origin) ?: return false
        if (appId.isBlank()) return false
        while (true) {
            val attempt = synchronized(this) {
                val ledger = read()
                val blockers = ledger.pending.filter {
                    it.origin == canonicalOrigin && (it.confirmedDeleted || it.appId != appId)
                }
                when {
                    blockers.isNotEmpty() -> OriginClaimAttempt.Blocked(
                        blockers.filter { it.confirmedDeleted },
                    )
                    persist(ledger.remembering(appId, canonicalOrigin)) -> OriginClaimAttempt.Ready
                    else -> OriginClaimAttempt.Failed
                }
            }
            when (attempt) {
                OriginClaimAttempt.Ready -> return true
                OriginClaimAttempt.Failed -> return false
                is OriginClaimAttempt.Blocked -> {
                    // Unconfirmed work waits for AppsChanged. Confirmed work is
                    // actively retried so a reopened screen need not wait for
                    // the process-level retry timer.
                    if (attempt.confirmed.isNotEmpty()) purge(attempt.confirmed)
                    delay(ORIGIN_GATE_POLL_MS)
                }
            }
        }
    }

    @Synchronized
    override fun prepareDeletion(appId: String, currentUrl: String?): Boolean {
        val current = read()
        val origin = currentUrl?.let(::trustedLoopbackOrigin) ?: current.origins[appId] ?: return false
        val persisted = persist(current.preparing(appId, origin))
        if (persisted) LocalAppWebViewRegistry.suspendForDeletion(appId)
        return persisted
    }

    @Synchronized
    override fun cancelDeletion(appId: String) {
        val current = read()
        if (current.pending.none { it.appId == appId && !it.confirmedDeleted }) return
        if (persist(current.cancellingUnconfirmed(appId))) {
            LocalAppWebViewRegistry.resumeAfterFailedDeletion(appId)
        }
    }

    @Synchronized
    override fun reconcile(liveAppIds: Set<String>) {
        val reconciled = read().confirmingMissing(liveAppIds)
        if (!persist(reconciled)) return
        val confirmed = reconciled.pending.filter { it.confirmedDeleted }
        // A same-app reopen is allowed while deletion is merely unconfirmed.
        // Once the authoritative snapshot proves absence, detach any reopened
        // view before WebStorage removes that exact origin.
        confirmed.forEach { LocalAppWebViewRegistry.detach(it.appId) }
        purge(confirmed)
    }

    @Synchronized
    override fun retryConfirmed() {
        purge(read().pending.filter { it.confirmedDeleted })
    }

    private fun purge(entries: List<PendingLocalAppWebStorageCleanup>) {
        entries.forEach { entry ->
            val key = entry.appId to entry.origin
            val shouldStart = synchronized(this) { purgesInFlight.add(key) }
            if (!shouldStart) return@forEach
            mainHandler.post {
                runCatching {
                    val storage = WebStorage.getInstance()
                    storage.deleteOrigin(entry.origin)
                    storage.getOrigins { origins ->
                        // deleteOrigin has no completion callback. Keep the
                        // durable entry until the quota registry proves that
                        // exact origin is gone; otherwise launch retry remains
                        // armed after a process death or provider failure.
                        if (entry.origin !in origins.orEmpty().keys) {
                            synchronized(this) {
                                purgesInFlight.remove(key)
                                persist(read().completed(entry))
                            }
                        } else {
                            synchronized(this) { purgesInFlight.remove(key) }
                            scheduleRetry(entry)
                        }
                    }
                }.onFailure {
                    synchronized(this) { purgesInFlight.remove(key) }
                    scheduleRetry(entry)
                }
            }
        }
    }

    private fun scheduleRetry(entry: PendingLocalAppWebStorageCleanup) {
        mainHandler.postDelayed(
            {
                val stillPending = synchronized(this) {
                    read().pending.any {
                        it.appId == entry.appId && it.origin == entry.origin && it.confirmedDeleted
                    }
                }
                if (stillPending) purge(listOf(entry))
            },
            CLEANUP_RETRY_MS,
        )
    }

    private fun read(): LocalAppWebStorageCleanupLedger =
        LocalAppWebStorageCleanupLedger.decode(preferences.getString(STATE_KEY, null))

    private fun persist(ledger: LocalAppWebStorageCleanupLedger): Boolean {
        // This must be durable before DeleteApp is submitted. The payload is a
        // few hundred bytes, so synchronous commit is intentional here.
        return preferences.edit().putString(STATE_KEY, ledger.encode()).commit()
    }

    companion object {
        private const val PREFERENCES = "local-app-web-storage-cleanup-v1"
        private const val STATE_KEY = "ledger"
        private const val CLEANUP_RETRY_MS = 1_000L
        private const val ORIGIN_GATE_POLL_MS = 100L

        @Volatile
        private var instance: AndroidLocalAppWebStorageCleanup? = null

        fun get(context: Context): AndroidLocalAppWebStorageCleanup =
            instance ?: synchronized(this) {
                instance ?: AndroidLocalAppWebStorageCleanup(context).also { instance = it }
            }
    }
}

private sealed interface OriginClaimAttempt {
    data object Ready : OriginClaimAttempt
    data object Failed : OriginClaimAttempt
    data class Blocked(val confirmed: List<PendingLocalAppWebStorageCleanup>) : OriginClaimAttempt
}

internal fun trustedLoopbackOrigin(rawUrl: String): String? = runCatching {
    val uri = URI(rawUrl)
    val host = uri.host?.lowercase()?.removePrefix("[")?.removeSuffix("]")
    if (uri.scheme != "http" || host !in setOf("127.0.0.1", "localhost", "::1")) return null
    val port = if (uri.port >= 0) uri.port else 80
    val renderedHost = if (host == "::1") "[::1]" else host
    "http://$renderedHost:$port"
}.getOrNull()
