package com.lingxi.code.inbox

import android.content.Context
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreException
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges
import com.lingxi.code.teams.TEAMS_API_ROOT
import com.lingxi.code.teams.TeamHttp
import com.lingxi.code.teams.TeamMessageFailure
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

internal data class InboxState(
    val uid: String? = null,
    val rows: List<InboxItem> = emptyList(),
    val loading: Boolean = false,
    val hasMore: Boolean = false,
    val cached: Boolean = false,
    val failed: Boolean = false,
    val writeError: String? = null,
    val rejected: Boolean = false,
    val dmReceipts: Boolean = false,
    val syncAssistant: Boolean = false,
    val online: Boolean = false,
    val readBatchReady: Boolean = false,
    val readBatchBusy: Boolean = false,
)

internal class InboxController private constructor(
    private val context: Context,
    private val auth: FirebaseAuth,
    private val db: FirebaseFirestore,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    internal val store = InboxStorage(context)
    private val mutable = MutableStateFlow(InboxState())
    val state = mutable.asStateFlow()
    private val projection = MutableStateFlow<Map<String, InboxItem>>(emptyMap())
    val projectedRows = projection.asStateFlow()
    private val badgeCount = MutableStateFlow(0)
    private val localAssistant = MutableStateFlow<List<InboxItem>>(emptyList())
    private val otherAssistant = MutableStateFlow<List<InboxItem>>(emptyList())
    private val pending = MutableStateFlow<List<InboxOperation>>(emptyList())
    private val deviceIdSeed = runCatching { com.lingxi.code.push.PushRegistration.identity(context).id }
        .getOrElse { java.util.UUID.randomUUID().toString() }
    val localSourceId: String get() = deviceIdSeed
    private var owner: String? = null
    private var loadJob: Job? = null
    private var drainJob: Job? = null
    private var drainTimer: Job? = null
    private val outboxLock = Mutex()
    private var refreshJob: Job? = null
    private var syncJob: Job? = null
    private var syncRequested = false
    private var presenceJob: Job? = null
    private var presenceOwner: Any? = null
    private val receiptUpdates = MutableStateFlow<Map<String, InboxItem>>(emptyMap())
    private var cloudQuery = InboxQuery()
    private var next: InboxCursor? = null
    private var cloudItems = emptyList<InboxItem>()
    private val verifiedAt = mutableMapOf<InboxRef, Long>()
    private var cloudLeaseJob: Job? = null
    private var cloudRenewalJob: Job? = null
    private var lastCloudRenewalAttempt = 0L
    private var authorityEpoch = 0L
    private var accountVerifiedAt = 0L
    private val registrations = mutableListOf<ListenerRegistration>()
    private var assistantConsent = AssistantConsent()
    private val consentWrites = Mutex()
    private val cacheWrites = Mutex()
    private var changingConsent = false
    private var consentReady = false
    private var conversationChoices = emptyMap<String, AssistantConversationConsent>()
    private var networkOnline = false
    private var readBatchJob: Job? = null
    private val remoteEnabled get() = assistantConsent.enabled
    private val pendingReadKeys = mutableSetOf<String>()
    private val _foreground = MutableStateFlow(false)
    private val _activeKey = MutableStateFlow<String?>(null)
    private val _banner = MutableStateFlow<InboxAlert?>(null)
    val banner = _banner.asStateFlow()
    private val assistantSources = mutableMapOf<String, AssistantCatalogEntry>()
    private var localOwner: String = ""
    private val assistantPublisher = InboxAssistantPublisher(store, ::request)

    val badge = combine(badgeCount, localAssistant) { count, local ->
        (count + local.count { it.localOnly && it.needsBadge }).coerceAtMost(100)
    }.stateIn(scope, SharingStarted.Eagerly, 0)
    val teamUnread = projectedRows.map { rows ->
        rows.values.filter { it.category == "team" && !it.archived }
            .groupingBy { it.ref.sourceId }.fold(0) { count, row -> (count + maxOf(row.unreadCount, if (row.manualUnread) 1 else 0)).coerceAtMost(100) }
    }.stateIn(scope, SharingStarted.Eagerly, emptyMap())
    val teamBadge = teamUnread.map { it.values.sum().coerceAtMost(100) }.stateIn(scope, SharingStarted.Eagerly, 0)

    init {
        changeAccount(auth.currentUser?.uid)
        auth.addAuthStateListener { changeAccount(it.currentUser?.uid) }
        scope.launch {
            com.lingxi.code.connectivity.ConnectivityObserver(context).isOnline.collect { connected ->
                networkOnline = connected
                mutable.update { it.copy(online = connected) }
                if (connected && owner != null && state.value.failed) refreshSoon()
                scheduleCloudRenewal()
            }
        }
        scope.launch {
            combine(localAssistant, otherAssistant, pending) { _, _, _ -> Unit }.collect { publishRows() }
        }
    }

    fun show(query: InboxQuery) {
        val serverQuery = query.copy(search = query.searchPrefix)
        if (cloudQuery != serverQuery) {
            cloudQuery = serverQuery
            cloudItems = emptyList()
            next = null
            load(reset = true, allowCache = true)
        } else if (state.value.failed) {
            retry()
        }
        publishRows()
    }

    fun nextPage() {
        if (!state.value.loading && next != null) load(reset = false, allowCache = true)
    }

    fun retry() {
        load(reset = true, allowCache = true)
        scheduleDrain(0)
        publishAssistant()
    }

    private fun changeAccount(uid: String?) {
        if (owner == uid && localOwner.isNotEmpty()) return
        owner = uid
        localOwner = uid ?: "local:$deviceIdSeed"
        registrations.forEach(ListenerRegistration::remove)
        registrations.clear()
        loadJob?.cancel()
        refreshJob?.cancel()
        drainJob?.cancel()
        drainTimer?.cancel()
        syncJob?.cancel()
        syncRequested = false
        presenceJob?.cancel()
        pendingReadKeys.clear()
        cloudLeaseJob?.cancel()
        cloudRenewalJob?.cancel()
        readBatchJob?.cancel()
        readBatchJob = null
        lastCloudRenewalAttempt = 0
        verifiedAt.clear()
        authorityEpoch++
        accountVerifiedAt = 0
        badgeCount.value = 0
        pending.value = emptyList()
        cloudItems = emptyList()
        projection.value = emptyMap()
        receiptUpdates.value = emptyMap()
        localAssistant.value = emptyList()
        otherAssistant.value = emptyList()
        assistantConsent = AssistantConsent()
        consentReady = false
        conversationChoices = emptyMap()
        mutable.value = InboxState(uid = uid, loading = uid != null, online = networkOnline)
        _banner.value = null
        _activeKey.value = null
        activeRef = null
        presenceOwner = null
        next = null
        assistantSources.entries.removeIf { it.value.route.hostId != null }
        refreshAssistantLocal()
        if (uid == null) return
        refreshPresence()
        scope.launch {
            val (queue, consent, rejected) = withContext(Dispatchers.IO) {
                Triple(store.pending(uid), store.consent(uid), store.hasRejected(uid))
            }
            val choices = withContext(Dispatchers.IO) { store.conversationConsents(uid) }
            if (!valid(uid)) return@launch
            pending.value = queue
            if (consent.revision >= assistantConsent.revision) assistantConsent = consent
            conversationChoices = choices + conversationChoices
            consentReady = true
            mutable.update { it.copy(syncAssistant = remoteEnabled, rejected = rejected) }
            publishRows()
            launch { restoreReadBatch(uid) }
            publishAssistant()
            scheduleDrain(0)
            launch {
                val prefs = runCatching { request(uid, JSONObject().put("action", "accountPreferences")) }.getOrNull()
                if (prefs != null && valid(uid)) mutable.update { it.copy(dmReceipts = prefs.optBoolean("dmReceipts")) }
            }
        }
        val root = db.collection("users").document(uid)
        registrations += root.collection("inboxSignals").limit(16).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!valid(uid)) return@addSnapshotListener
            if (error != null) {
                if (error.code in firestoreAuthorityErrors) invalidateCloud(uid)
                return@addSnapshotListener
            }
            if (snapshot?.metadata?.isFromCache == false) refreshSoon()
        }
        registrations += root.collection("inboxBadges").limit(16).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!valid(uid)) return@addSnapshotListener
            if (error != null) {
                if (error.code in firestoreAuthorityErrors) invalidateCloud(uid)
                return@addSnapshotListener
            }
            if (snapshot?.metadata?.isFromCache == false) {
                badgeCount.value = snapshot.documents.sumOf { (it.getLong("count") ?: 0).coerceIn(0, 100) }.coerceAtMost(100).toInt()
                accountVerifiedAt = System.currentTimeMillis()
                scheduleLeaseExpiry()
            }
        }
        load(reset = true, allowCache = true)
    }

    private fun valid(uid: String): Boolean = owner == uid && auth.currentUser?.uid == uid

    private fun load(reset: Boolean, allowCache: Boolean) {
        val uid = owner ?: return
        val requestedQuery = cloudQuery
        val epoch = authorityEpoch
        val cursor = if (reset) null else next ?: return
        val cacheKey = JSONArray(listOf(requestedQuery.filter.wire, requestedQuery.category, requestedQuery.search, cursor?.json()?.toString())).toString()
        loadJob?.cancel()
        mutable.update { it.copy(loading = true) }
        loadJob = scope.launch {
            try {
                if (allowCache && cloudItems.isEmpty()) {
                    val cached = withContext(Dispatchers.IO) { store.cached(uid, cacheKey, System.currentTimeMillis()) }
                    if (cached != null && valid(uid) && epoch == authorityEpoch) {
                        consumePage(cached.value, reset, offline = true, authorizedAt = cached.verifiedAt)
                    }
                }
                val command = JSONObject().put("action", "list")
                    .put("filter", if (requestedQuery.filter == InboxFilter.Archived) InboxFilter.All.wire else requestedQuery.filter.wire)
                when (requestedQuery.filter) {
                    InboxFilter.All -> command.put("archived", false)
                    InboxFilter.Archived -> command.put("archived", true)
                    InboxFilter.Unread, InboxFilter.Attention -> Unit
                }
                requestedQuery.category?.let { command.put("category", it) }
                requestedQuery.search.takeIf(String::isNotEmpty)?.let { command.put("searchPrefix", it) }
                cursor?.let { command.put("after", it.json()) }
                val page = request(uid, command)
                if (!valid(uid) || requestedQuery != cloudQuery || epoch != authorityEpoch) return@launch
                val authorizedAt = System.currentTimeMillis()
                accountVerifiedAt = authorizedAt
                consumePage(page, reset, offline = false, authorizedAt = authorizedAt)
                cacheWrites.withLock {
                    if (valid(uid) && epoch == authorityEpoch) withContext(Dispatchers.IO) { store.cache(uid, cacheKey, page, authorizedAt) }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                if (valid(uid) && epoch == authorityEpoch) {
                    pruneExpired()
                    cloudItems = cloudItems.map { it.copy(offline = true) }
                    mutable.update { it.copy(failed = true, cached = cloudItems.isNotEmpty(), loading = false) }
                    publishRows()
                }
            }
        }
    }

    private fun consumePage(page: JSONObject, reset: Boolean, offline: Boolean, authorizedAt: Long) {
        if (!isCloudInboxLeaseFresh(authorizedAt, System.currentTimeMillis())) return
        val rows = page.getJSONArray("items").inboxItems().take(50).map { if (offline) it.copy(offline = true) else it }
        next = page.optJSONObject("next")?.let { InboxCursor(it.getLong("lastActivityAt"), it.getString("key"), it.nullableLong("pinOrder"), it.nullableString("titleSort")) }
        rows.forEach { verify(it.ref, authorizedAt) }
        cloudItems = if (reset) rows else (cloudItems + rows).distinctBy { it.key }.take(10_000)
        projection.update { previous ->
            val resetUnfiltered = reset && cloudQuery == InboxQuery()
            val pageRows = rows.associateBy { it.key }
            if (resetUnfiltered || previous.size > 10_000) pageRows else previous + pageRows
        }
        mutable.update { it.copy(hasMore = next != null && cloudItems.size < 10_000, cached = offline, failed = false, loading = offline) }
        publishRows()
        scheduleLeaseExpiry()
        scheduleCloudRenewal()
    }

    private fun verify(ref: InboxRef, at: Long = System.currentTimeMillis()) {
        verifiedAt[ref] = maxOf(verifiedAt[ref] ?: 0, at)
    }

    private fun cloudFresh(ref: InboxRef, now: Long = System.currentTimeMillis()): Boolean =
        isCloudInboxLeaseFresh(verifiedAt[ref] ?: 0, now)

    private fun pruneExpired() {
        val now = System.currentTimeMillis()
        verifiedAt.keys.removeAll { !cloudFresh(it, now) }
        cloudItems = cloudItems.filter { cloudFresh(it.ref, now) }
        projection.update { rows -> rows.filterValues { cloudFresh(it.ref, now) } }
        receiptUpdates.update { rows -> rows.filterValues { cloudFresh(it.ref, now) } }
        if (!isCloudInboxLeaseFresh(accountVerifiedAt, now)) {
            badgeCount.value = 0
            accountVerifiedAt = 0
        }
    }

    private fun scheduleLeaseExpiry() {
        cloudLeaseJob?.cancel()
        val earliest = sequenceOf(verifiedAt.values.minOrNull(), accountVerifiedAt.takeIf { it > 0 }).filterNotNull().minOrNull() ?: return
        cloudLeaseJob = scope.launch {
            val now = System.currentTimeMillis()
            val wait = if (now < earliest) 0 else (earliest + CLOUD_INBOX_AUTHORIZATION_MS - now).coerceAtLeast(0)
            delay(wait)
            pruneExpired()
            publishRows()
            scheduleLeaseExpiry()
            if (_foreground.value && accountVerifiedAt == 0L) refreshSoon()
        }
    }

    private fun scheduleCloudRenewal() {
        cloudRenewalJob?.cancel()
        if (!_foreground.value || !networkOnline || owner == null || accountVerifiedAt <= 0) return
        val due = maxOf(accountVerifiedAt + CLOUD_INBOX_AUTHORIZATION_MS - 30_000, lastCloudRenewalAttempt + 15_000)
        cloudRenewalJob = scope.launch {
            delay((due - System.currentTimeMillis()).coerceAtLeast(0))
            lastCloudRenewalAttempt = System.currentTimeMillis()
            if (loadJob?.isActive != true) load(reset = true, allowCache = false)
        }
    }

    private fun invalidateCloud(uid: String, ref: InboxRef? = null) {
        if (!valid(uid)) return
        authorityEpoch++
        if (ref == null) {
            verifiedAt.clear()
            cloudItems = emptyList()
            next = null
            projection.value = emptyMap()
            receiptUpdates.value = emptyMap()
            badgeCount.value = 0
            accountVerifiedAt = 0
            mutable.update { it.copy(loading = false, hasMore = false, cached = false, failed = true, dmReceipts = false) }
        } else {
            verifiedAt.remove(ref)
            cloudItems = cloudItems.filterNot { it.ref == ref }
            projection.update { rows -> rows.filterValues { it.ref != ref } }
            receiptUpdates.update { rows -> rows.filterValues { it.ref != ref } }
            mutable.update { it.copy(loading = false) }
        }
        if (_banner.value?.ref.let { it != null && (ref == null || it == ref) }) _banner.value = null
        publishRows()
        scheduleLeaseExpiry()
        scope.launch {
            try {
                cacheWrites.withLock { withContext(Dispatchers.IO) { store.purgePages(uid) } }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) {
                if (valid(uid)) mutable.update { it.copy(writeError = "storage") }
            }
        }
    }

    private fun publishRows() {
        val local = (otherAssistant.value + localAssistant.value.map { row ->
            row.copy(conversationSummarySync = conversationChoices[row.key]?.consent?.enabled ?: true)
        }).associateBy { it.key }
        val rows = local.toMutableMap()
        for (remote in cloudItems) {
            if (!cloudFresh(remote.ref)) continue
            val source = local[remote.key]
            if (source == null || remote.ref.kind != "assistant") {
                rows[remote.key] = if (remote.ref.kind == "assistant") remote.copy(offline = true) else remote
                continue
            }
            rows[remote.key] = if (source.localOnly && source.conversationSummarySync != null &&
                (!remoteEnabled || !source.conversationSummarySync)) {
                source
            } else remote.copy(
                conversationSummarySync = source.conversationSummarySync,
                title = source.title.ifBlank { remote.title },
                context = source.context.ifBlank { remote.context },
                baselineCursor = source.baselineCursor,
                offline = source.offline,
            )
        }
        val grouped = pending.value.groupBy { it.ref.key }
        val merged = rows.values.map { item -> grouped[item.key].orEmpty().fold(item) { row, op -> op.apply(row) } }
        mutable.update { it.copy(rows = merged.sortedWith(InboxItem.ordering)) }
    }

    fun mutate(operation: InboxOperation, item: InboxItem? = null, automatic: Boolean = false) {
        if (!operation.ref.isValid()) return
        val account = owner
        val deviceOwner = localOwner
        scope.launch {
            try {
                if (item?.localOnly == true) {
                    outboxLock.withLock {
                        if (deviceOwner != localOwner) return@withLock
                        val changed = withContext(Dispatchers.IO) { store.rememberLocal(deviceOwner, item, operation) }
                        if (deviceOwner == localOwner) {
                            localAssistant.update { entries -> entries.map { if (it.ref == changed.ref) it.withLocalState(changed) else it } }
                            otherAssistant.update { entries -> entries.map { if (it.ref == changed.ref) it.withLocalState(changed) else it } }
                            if (operation.action == "markRead") AssistantInboxAlerts.cancelRead(operation.ref, operation.body.getLong("through"), operation.body.nullableLong("observedPendingActionRevision"),
                                operation.body.nullableLong("observedFailureRevision"))
                        }
                    }
                    return@launch
                }
                if (account == null || !valid(account)) return@launch
                val queued = outboxLock.withLock {
                    if (!valid(account)) return@withLock false
                    val queued = withContext(Dispatchers.IO) {
                        if (store.enqueue(account, operation)) store.pending(account) else null
                    }
                    if (!valid(account) || queued == null) return@withLock false
                    pending.value = queued
                    true
                }
                if (!valid(account)) return@launch
                if (!queued) {
                    mutable.update { it.copy(writeError = "capacity") }
                    return@launch
                }
                if (operation.action == "markRead") {
                    AssistantInboxAlerts.cancelRead(operation.ref, operation.body.getLong("through"), operation.body.nullableLong("observedPendingActionRevision"),
                                operation.body.nullableLong("observedFailureRevision"))
                    if (automatic) pendingReadKeys += operation.ref.key
                }
                mutable.update { it.copy(writeError = null) }
                scheduleDrain(if (automatic && activeRef?.key == operation.ref.key) 1_000 else 0)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                mutable.update { it.copy(writeError = "storage") }
            }
        }
    }

    fun flushRead(key: String) {
        if (pendingReadKeys.remove(key)) scheduleDrain(0)
    }

    fun movePin(row: InboxItem, direction: Int) {
        val pinned = state.value.rows.filter { !it.archived && it.pinOrder != null }.sortedWith(InboxItem.ordering)
        val from = pinned.indexOfFirst { it.ref == row.ref }
        if (from < 0) return
        val to = from + direction
        val sibling = pinned.getOrNull(to) ?: return
        val siblingRank = sibling.pinOrder ?: return
        val rowRank = row.pinOrder ?: return
        val left = if (direction < 0) pinned.getOrNull(to - 1)?.pinOrder ?: 0 else siblingRank
        val right = if (direction < 0) siblingRank else pinned.getOrNull(to + 1)?.pinOrder ?: 9_007_199_254_740_991L
        if (right - left > 1) {
            val rank = if (to == pinned.lastIndex) minOf(right - 1, left + 1_000) else left + (right - left) / 2
            mutate(InboxOperation.preferences(row, JSONObject().put("pinOrder", rank)), row)
        } else {
            mutate(InboxOperation.preferences(sibling, JSONObject().put("pinOrder", rowRank)), sibling)
            mutate(InboxOperation.preferences(row, JSONObject().put("pinOrder", siblingRank)), row)
        }
    }

    fun markAllRead() {
        val uid = owner ?: return
        if (!networkOnline || !state.value.readBatchReady || state.value.readBatchBusy || !valid(uid)) return
        val requestedAt = System.currentTimeMillis()
        val jobId = java.util.UUID.randomUUID().toString()
        val local = freezeLocalAssistantReads(localAssistant.value)
        mutable.update { it.copy(readBatchBusy = true, writeError = null) }
        readBatchJob = scope.launch {
            try {
                withContext(Dispatchers.IO) { store.setValue(uid, "readBatch", jobId) }
                if (!valid(uid)) return@launch
                if (!networkOnline) {
                    finishReadBatch(uid, "network")
                    return@launch
                }
                val completed = try {
                    val reply = request(uid, JSONObject().put("action", "markAllReadSnapshot")
                        .put("operationId", jobId).put("requestedAt", requestedAt))
                    completeBatchIfTerminal(uid, jobId, reply)
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (failure: TeamMessageFailure) {
                    if (failure.status in setOf(400, 401, 403, 409)) {
                        finishReadBatch(uid, "network")
                        true
                    } else false
                }
                catch (_: Exception) { false }
                if (valid(uid) && local.isNotEmpty()) {
                    outboxLock.withLock {
                        val saved = withContext(Dispatchers.IO) { store.rememberLocalBatch(uid, local) }
                        if (valid(uid)) {
                            localAssistant.update { rows -> rows.map { row -> row.withLocalState(saved[row.key]) } }
                            local.forEach { (row, read) ->
                                AssistantInboxAlerts.cancelRead(row.ref, read.body.getLong("through"), row.pendingActionRevision, row.failureRevision)
                            }
                        }
                    }
                }
                if (!completed) pollReadBatch(uid, jobId)
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) {
                if (valid(uid)) mutable.update { it.copy(readBatchBusy = false, writeError = "storage") }
            }
        }
    }

    private suspend fun restoreReadBatch(uid: String) {
        try {
            val jobId = withContext(Dispatchers.IO) { store.value(uid, "readBatch") }
            if (!valid(uid)) return
            mutable.update { it.copy(readBatchReady = true, readBatchBusy = jobId != null) }
            if (jobId != null) readBatchJob = scope.launch { pollReadBatch(uid, jobId) }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) {
            if (valid(uid)) mutable.update { it.copy(writeError = "storage") }
        }
    }

    private suspend fun pollReadBatch(uid: String, jobId: String) {
        // Recovery checks status only. A missing operation needs another explicit user action.
        while (valid(uid)) {
            delay(1_500)
            if (!networkOnline || !_foreground.value) continue
            try {
                val reply = request(uid, JSONObject().put("action", "readBatchStatus").put("jobId", jobId))
                if (completeBatchIfTerminal(uid, jobId, reply)) return
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: TeamMessageFailure) {
                if (failure.status in setOf(401, 403, 404, 410)) {
                    finishReadBatch(uid, "batchMissing")
                    return
                }
            } catch (_: Exception) { }
        }
    }

    private suspend fun completeBatchIfTerminal(uid: String, jobId: String, reply: JSONObject): Boolean {
        if (reply.optString("jobId") != jobId) return false
        val state = reply.optString("state")
        if (state == "complete" || state == "expired") {
            finishReadBatch(uid, "batchMissing".takeIf { state == "expired" })
            return true
        }
        return false
    }

    private suspend fun finishReadBatch(uid: String, error: String?) {
        withContext(Dispatchers.IO) { store.setValue(uid, "readBatch", null) }
        if (valid(uid)) {
            mutable.update { it.copy(readBatchBusy = false, writeError = error) }
            refreshSoon()
        }
    }

    fun clearRejected() {
        val uid = owner ?: return
        scope.launch {
            withContext(Dispatchers.IO) { store.clearRejected(uid) }
            if (valid(uid)) mutable.update { it.copy(rejected = false, writeError = it.writeError.takeUnless { code -> code == "preferenceConflict" }) }
        }
    }

    private fun scheduleDrain(wait: Long) {
        val uid = owner ?: return
        if (drainTimer?.isActive == true && wait > 0L) return
        drainTimer?.cancel()
        drainTimer = scope.launch {
            delay(wait)
            startDrain(uid)
        }
    }

    private fun startDrain(uid: String) {
        if (drainJob?.isActive == true || !valid(uid)) return
        drainJob = scope.launch {
            while (valid(uid)) {
                val first = pending.value.firstOrNull() ?: break
                val batch = if (first.action == "markRead") {
                    pending.value.asSequence().takeWhile { it.action == "markRead" }.distinctBy { it.ref.key }.take(50).toList()
                } else listOf(first)
                try {
                    if (batch.size == 1) {
                        val response = request(uid, first.body).optJSONObject("item")?.let(InboxItem::parse)
                        acknowledge(uid, first, response)
                    } else {
                        val reads = JSONArray(batch.map { JSONObject(it.body.toString()).apply { remove("action") } })
                        val results = request(uid, JSONObject().put("action", "markAllRead").put("entries", reads)).getJSONArray("results")
                        val byKey = (0 until results.length()).associate { results.getJSONObject(it).let { item -> item.getString("key") to item } }
                        for (operation in batch) {
                            val answer = byKey.getValue(operation.ref.key)
                            if (answer.optBoolean("ok")) acknowledge(uid, operation, null)
                            else reject(uid, operation, answer.optString("code"))
                        }
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    val serverFailure = failure as? TeamMessageFailure
                    if (serverFailure != null && (serverFailure.code in terminalErrors || serverFailure.status in setOf(401, 403))) {
                        if (batch.size > 1 && serverFailure.status !in setOf(401, 403)) {
                            delay(5_000)
                            continue
                        }
                        batch.forEach { reject(uid, it, serverFailure.code) }
                    } else {
                        delay(5_000)
                        continue
                    }
                }
                outboxLock.withLock {
                    val remaining = withContext(Dispatchers.IO) { store.pending(uid) }
                    if (valid(uid)) pending.value = remaining
                }
                refreshSoon()
            }
        }
    }

    private suspend fun acknowledge(uid: String, operation: InboxOperation, response: InboxItem?) {
        if (!valid(uid)) return
        if (response != null) {
            verify(response.ref)
            receiptUpdates.update { it + (response.key to response) }
            scheduleLeaseExpiry()
        }
        projection.update { rows ->
            val row = rows[operation.ref.key] ?: return@update rows
            rows + (row.key to (response ?: operation.apply(row)))
        }
        cloudItems = cloudItems.map { row -> if (row.ref == operation.ref) response ?: operation.apply(row) else row }
        withContext(Dispatchers.IO) { store.acknowledge(uid, operation) }
    }

    private suspend fun reject(uid: String, operation: InboxOperation, reason: String) {
        if (reason in sourceAuthorityErrors) invalidateCloud(uid, operation.ref)
        withContext(Dispatchers.IO) { store.reject(uid, operation, reason) }
        if (valid(uid)) mutable.update {
            it.copy(rejected = true, writeError = if (reason == "PREFERENCE_REVISION_CONFLICT") "preferenceConflict" else it.writeError)
        }
    }

    private fun refreshSoon() {
        if (refreshJob?.isActive == true) return
        refreshJob = scope.launch { delay(250); load(reset = true, allowCache = false) }
    }

    fun setReceipts(enabled: Boolean) {
        val uid = owner ?: return
        scope.launch {
            try {
                val response = request(uid, JSONObject().put("action", "accountPreferences").put("dmReceipts", enabled))
                if (valid(uid)) mutable.update { it.copy(dmReceipts = response.optBoolean("dmReceipts"), writeError = null) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                if (valid(uid)) mutable.update { it.copy(writeError = "network") }
            }
        }
    }

    suspend fun peerReceipt(ref: InboxRef): Long? {
        val uid = owner ?: return null
        if (!state.value.dmReceipts) return null
        return request(uid, JSONObject().put("action", "peerReceipt").put("key", ref.key).put("generation", ref.generation))
            .let { if (valid(uid) && !it.isNull("readThrough")) it.optLong("readThrough") else null }
    }

    fun setAssistantSync(enabled: Boolean) = changeAssistantConsent { uid ->
        val saved = withContext(Dispatchers.IO) { store.changeConsent(uid, enabled) }
        if (!valid(uid)) return@changeAssistantConsent
        assistantConsent = saved
        mutable.update { it.copy(syncAssistant = saved.enabled, writeError = null) }
    }

    fun setConversationSummarySync(row: InboxItem, enabled: Boolean) {
        val entry = assistantSources[row.key]?.takeIf { it.route.hostId == null } ?: return
        changeAssistantConsent { uid ->
            val choice = withContext(Dispatchers.IO) {
                store.changeConversationConsent(uid, entry.item.ref, entry.item.sourceRevision, enabled)
            }
            if (valid(uid)) {
                conversationChoices = conversationChoices + (row.key to choice)
                if (!enabled && row.ref.generation > 0) invalidateCloud(uid, row.ref)
            }
        }
    }

    private fun changeAssistantConsent(change: suspend (String) -> Unit) {
        val uid = owner ?: return
        scope.launch {
            consentWrites.withLock {
                if (!valid(uid) || !consentReady) return@withLock
                changingConsent = true
                try {
                    syncJob?.let { it.cancel(); it.join() }
                    syncJob = null
                    if (!valid(uid)) return@withLock
                    change(uid)
                    publishRows()
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) {
                    if (valid(uid)) mutable.update { it.copy(writeError = "storage") }
                } finally {
                    changingConsent = false
                    publishAssistant()
                }
            }
        }
    }

    fun assistantCatalog(local: List<AssistantCatalogEntry>, remoteUid: String?, remote: List<AssistantCatalogEntry>) {
        val previousLocal = assistantSources.values.filter { it.route.hostId == null }.associateBy { it.route.sessionId }
        val entries = local.map { current ->
            val previous = previousLocal[current.route.sessionId]
            if (current.publishable || previous == null || !previous.publishable) current
            else previous.copy(route = current.route, item = previous.item.copy(title = current.item.title, context = current.item.context))
        } + remote.takeIf { remoteUid != null && remoteUid == owner }.orEmpty()
        val next = entries.sortedByDescending { it.route.hostId != null }.associateBy { it.item.key }
        for (entry in local) {
            val item = entry.item
            val previous = assistantSources[item.key]?.item
            val cloudEnabled = remoteEnabled && conversationChoices[item.key]?.consent?.enabled != false
            if (!entry.publishable || cloudEnabled || item.sourceRevision <= (previous?.sourceRevision ?: -1)) continue
            val actionChanged = item.pendingActionRevision != previous?.pendingActionRevision || item.state != previous?.state
            if (actionChanged) AssistantInboxAlerts.cancelPending(item.ref)
            val newFailure = item.state == "failed" && (item.failureRevision ?: 0) > (previous?.failureRevision ?: 0)
            val kind = when {
                item.pendingActionCount > 0 && actionChanged -> if (item.state == "waiting_for_approval") "approval" else "input"
                newFailure -> "failure"
                previous == null || item.sourceCursor <= previous.sourceCursor -> continue
                item.attentionKind == "failure" -> "failure"
                else -> "completion"
            }
            val action = kind in setOf("input", "approval")
            val eventTime = if (action) item.pendingActionAt ?: 0 else if (newFailure) item.failureAt ?: 0 else item.lastActivityAt
            if (eventTime <= 0) continue
            val personal = localAssistant.value.firstOrNull { it.key == item.key }
            if (personal?.archived == true || personal?.notificationMode == "none") continue
            val revision = if (action) item.pendingActionRevision ?: item.sourceRevision else if (newFailure) item.failureRevision ?: item.sourceRevision else item.sourceRevision
            AssistantInboxAlerts.local(context, localOwner, InboxAlert(item.ref, "${item.key}:$revision:$kind",
                kind, item.sourceCursor, item.title, "", eventTime + 3_600_000, if (action) item.pendingActionRevision else null,
                deliverAfter = eventTime + if (action) 6_000 else if (kind == "completion") 5_000 else 0,
                failureRevision = item.failureRevision.takeIf { newFailure }))
        }
        if (next == assistantSources) return
        assistantSources.clear()
        assistantSources.putAll(next)
        refreshAssistantLocal()
        publishAssistant()
    }

    private fun refreshAssistantLocal() {
        val uid = localOwner
        val sources = assistantSources.values.toList()
        scope.launch {
            val hydrated = withContext(Dispatchers.IO) {
                sources.map { entry ->
                    val previous = store.value(uid, "local:${entry.item.key}")?.let { InboxItem.parse(JSONObject(it)) }
                    entry.item.withLocalState(previous) to entry.route.hostId
                }.partition { it.second == null }
            }
            if (localOwner != uid || sources != assistantSources.values.toList()) return@launch
            localAssistant.value = hydrated.first.map { it.first }
            otherAssistant.value = hydrated.second.map { it.first }
        }
    }

    private fun publishAssistant() {
        val uid = owner ?: return
        syncRequested = true
        if (!consentReady || changingConsent || syncJob?.isActive == true) return
        syncJob = scope.launch {
            delay(250)
            while (syncRequested && valid(uid)) {
                syncRequested = false
                val entries = assistantSources.values.filter { it.publishable && it.route.hostId == null }
                try {
                    if (assistantPublisher.sync(uid, assistantConsent, entries) && valid(uid)) refreshSoon()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    if (!valid(uid)) break
                    delay(5_000)
                    syncRequested = true
                }
            }
        }
    }

    fun assistantRoute(key: String): InboxAssistantRoute? = assistantSources[key]?.route

    fun remoteAssistant(hostId: String, sessionId: String): InboxItem? {
        val id = sessionId.removePrefix("sess:")
        val key = assistantSources.values.firstOrNull { it.route.hostId == hostId && it.item.ref.conversationId == id }?.item?.key
        return state.value.rows.firstOrNull { item ->
            item.ref.kind == "assistant" && item.ref.conversationId == id &&
                (item.key == key || item.remoteHostId == hostId)
        }
    }

    fun snapshot(ref: InboxRef): InboxItem? =
        (state.value.rows.firstOrNull { it.ref == ref } ?: receiptUpdates.value[ref.key]?.takeIf { it.ref == ref }
            ?: projection.value[ref.key]?.takeIf { it.ref == ref })?.takeIf { it.localOnly || cloudFresh(ref) }

    suspend fun prime(ref: InboxRef) {
        val uid = owner ?: return
        try {
            val row = findCloud(ref.key, ref.generation)
            if (valid(uid) && row != null) {
                verify(row.ref)
                receiptUpdates.update { it + (row.key to row) }
                projection.update { it + (row.key to row) }
                scheduleLeaseExpiry()
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { }
    }

    suspend fun restoreRemoteDraft(uid: String, host: String, session: String): String = withContext(Dispatchers.IO) {
        store.value(uid, "remoteDraft:$host:$session").orEmpty()
    }

    fun saveRemoteDraft(uid: String, host: String, session: String, text: String) {
        if (!valid(uid)) return
        scope.launch {
            draftLock.withLock { withContext(Dispatchers.IO) {
                store.setValue(uid, "remoteDraft:$host:$session", text.takeIf(String::isNotEmpty))
            } }
        }
    }

    private val draftLock = Mutex()


    fun setForeground(foreground: Boolean) {
        _foreground.value = foreground
        if (!foreground) _banner.value = null else {
            pruneExpired()
            publishRows()
            scheduleLeaseExpiry()
            if (owner != null && accountVerifiedAt == 0L) refreshSoon()
        }
        scheduleCloudRenewal()
        refreshPresence()
    }
    fun enter(owner: Any, ref: InboxRef) {
        presenceOwner = owner
        _activeKey.value = ref.key
        activeRef = ref
        refreshPresence()
    }

    fun leave(owner: Any) {
        if (presenceOwner !== owner) return
        _activeKey.value?.let(::flushRead)
        presenceOwner = null
        _activeKey.value = null
        activeRef = null
        refreshPresence()
    }

    private var activeRef: InboxRef? = null
    private fun refreshPresence() {
        val uid = owner ?: return
        val ref = activeRef?.takeIf { it.generation > 0 }
        val foreground = _foreground.value
        presenceJob?.cancel()
        presenceJob = scope.launch {
            val deviceId = deviceIdSeed
            do {
                try {
                    request(uid, JSONObject().put("action", "presence").put("deviceId", deviceId)
                        .put("key", ref?.key ?: JSONObject.NULL).apply { ref?.let { put("generation", it.generation) } }
                        .put("foreground", foreground))
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) { }
                if (foreground) delay(25_000)
            } while (foreground && valid(uid))
        }
    }

    fun alertDisposition(ref: InboxRef): InboxAlertDisposition = when {
        !_foreground.value -> InboxAlertDisposition.System
        activeRef == ref -> InboxAlertDisposition.Inline
        else -> InboxAlertDisposition.Banner
    }

    fun showBanner(alert: InboxAlert) { _banner.value = alert }
    fun dismissBanner() { _banner.value = null }
    fun isRead(ref: InboxRef, through: Long): Boolean {
        if (through <= 0) return false
        val row = snapshot(ref)
        return (row?.readThrough ?: 0) >= through ||
            pending.value.any { it.ref == ref && it.action == "markRead" && it.body.optLong("through") >= through }
    }

    fun isNotificationOwner(uid: String): Boolean =
        localOwner == uid && (owner == null || valid(uid))

    suspend fun findCloud(key: String, generation: Long): InboxItem? {
        if (generation == 0L) return state.value.rows.firstOrNull { it.key == key && it.ref.generation == 0L && it.localOnly }

        val uid = owner ?: return null
        if (InboxRef.fromKey(key, generation) == null) return null
        return request(uid, JSONObject().put("action", "get").put("key", key).put("generation", generation))
            .optJSONObject("item")?.let(InboxItem::parse)?.takeIf { it.ref.generation == generation }
    }

    suspend fun openNotice(target: InboxNoticeTarget, assistantPreview: suspend (InboxItem, InboxNoticeTarget) -> List<String>?): InboxNoticePreview? {
        val now = System.currentTimeMillis()
        if (!target.isValid(now)) return null
        val uid = owner
        if (target.ref.generation == 0L) {
            val deviceOwner = localOwner
            if (!withContext(Dispatchers.IO) { store.wasAdmitted(deviceOwner, target.ledgerKey(), now) } || localOwner != deviceOwner) return null
            val row = findCloud(target.ref.key, 0) ?: return null
            val bodies = assistantPreview(row, target) ?: return null
            if (localOwner != deviceOwner || !target.isValid(System.currentTimeMillis())) return null
            return InboxNoticePreview(target, row, bodies)
        }
        if (uid == null || !valid(uid)) return null
        val doc = request(uid, JSONObject().put("action", "notice").put("id", target.id).put("deviceId", deviceIdSeed))
            .optJSONObject("item") ?: return null
        val verified = target.verifiedBy(doc, now) ?: return null
        val row = findCloud(target.ref.key, target.ref.generation) ?: return null
        if (row.sourceCursor < verified.through || !valid(uid)) return null
        val bodies = if (row.ref.kind == "assistant") assistantPreview(row, verified) ?: return null
        else threadPreview(uid, verified) ?: return null
        if (!valid(uid) || !verified.isValid(System.currentTimeMillis())) return null
        return InboxNoticePreview(verified, row, bodies)
    }

    private suspend fun threadPreview(uid: String, target: InboxNoticeTarget): List<String>? {
        // A live task without a message ID is validated by notice lookup.
        if (target.through == 0L || target.messageId == null && target.taskId != null) return emptyList()
        val ref = target.ref
        val field = if (ref.kind == "service") "serviceId" else "teamId"
        val user = auth.currentUser?.takeIf { it.uid == uid } ?: return null
        val token = user.getIdToken(false).await().token ?: return null
        if (!valid(uid)) return null
        val response = TeamHttp.request("$TEAMS_API_ROOT/v1/teams/messages", token,
            JSONObject().put("action", "history").put(field, ref.sourceId).put("generation", ref.generation)
                .put("conversationId", ref.conversationId).put("before", target.through + 1))
        val raw = response.getJSONArray("messages")
        val messages = (0 until minOf(raw.length(), 50)).map { com.lingxi.code.teams.TeamMessage.parse(raw.getJSONObject(it)) }
        val anchor = messages.singleOrNull { it.seq == target.through && (target.messageId == null || it.id == target.messageId) && !it.recalled }
            ?: return null
        return listOf(anchor.text)
    }

    private suspend fun request(uid: String, body: JSONObject): JSONObject {
        val epoch = authorityEpoch
        val action = body.optString("action")
        val user = auth.currentUser?.takeIf { it.uid == uid } ?: throw CancellationException()
        val token = try {
            user.getIdToken(false).await().token ?: throw TeamMessageFailure("SIGN_IN_REQUIRED", 401)
        } catch (failure: Exception) {
            if (valid(uid) && (failure is com.google.firebase.auth.FirebaseAuthInvalidUserException || failure is TeamMessageFailure && failure.status == 401)) {
                invalidateCloud(uid)
                throw TeamMessageFailure("SIGN_IN_REQUIRED", 401)
            }
            throw failure
        }
        if (!valid(uid)) throw CancellationException()
        val result = try {
            TeamHttp.request("$TEAMS_API_ROOT/v1/inbox", token, body)
        } catch (failure: TeamMessageFailure) {
            if (valid(uid)) {
                val ref = InboxRef.fromKey(body.optString("key"), body.optLong("generation", -1))
                if (failure.status == 401 || failure.status == 403 && action in setOf("list", "accountPreferences", "markAllRead")) {
                    invalidateCloud(uid)
                } else if (ref != null && (failure.status == 403 || failure.code in sourceAuthorityErrors)) {
                    invalidateCloud(uid, ref)
                }
            }
            throw failure
        }
        if (!valid(uid) || action in setOf("get", "list", "peerReceipt", "notice") && epoch != authorityEpoch) throw CancellationException()
        return result
    }

    companion object {
        private val firestoreAuthorityErrors = setOf(FirebaseFirestoreException.Code.UNAUTHENTICATED, FirebaseFirestoreException.Code.PERMISSION_DENIED)
        private val sourceAuthorityErrors = setOf("STALE_MEMBERSHIP", "SOURCE_ACCESS_REQUIRED", "CONVERSATION_REQUIRED", "MEMBERSHIP_REQUIRED",
            "INBOX_ITEM_MISSING", "SERVICE_ENTRY_REQUIRED", "SERVICE_WITHDRAWN", "INBOX_AUTHORITY_GONE", "SOURCE_NOT_FOUND", "STALE_GENERATION")
        private val terminalErrors = sourceAuthorityErrors + setOf("UNAUTHENTICATED", "PERMISSION_DENIED", "SIGN_IN_REQUIRED",
            "INVALID_INBOX_REF", "INVALID_INPUT", "NOT_FOUND", "INVALID_READ_CURSOR", "MANUAL_REVISION_REQUIRED", "MANUAL_REVISION_CONFLICT", "PREFERENCE_REVISION_CONFLICT")

        @Volatile private var instance: InboxController? = null
        fun get(context: Context): InboxController = instance ?: synchronized(this) {
            instance ?: InboxController(context.applicationContext, FirebaseAuth.getInstance(), FirebaseFirestore.getInstance())
                .also { instance = it }
        }
        fun cached(): InboxController? = instance
    }
}
