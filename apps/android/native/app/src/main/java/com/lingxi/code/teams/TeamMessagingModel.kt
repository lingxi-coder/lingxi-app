package com.lingxi.code.teams

import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.Timestamp
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreException
import com.google.firebase.firestore.DocumentSnapshot
import com.google.firebase.firestore.Query
import com.lingxi.code.inbox.InboxController
import com.lingxi.code.inbox.InboxOperation
import com.lingxi.code.inbox.InboxRef
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/**
 * [offline] = the SDK serves the directory or the open thread from its cache, or an HTTP step failed; [directoryCached] is the
 * directory snapshot alone (it gates automatic mark-read) and [directoryReady] says the first one has arrived. [readsReady] = every listed conversation has delivered its first
 * open thread has a private read mark. [botsLoaded] / [botsFailed] feed `composerLock`.
 */
internal data class TeamMessagingState(val cache: TeamMessageCache? = null, val selected: String = "", val offline: Boolean = true, val error: String = "", val revoked: Boolean = false, val submitting: Boolean = false, val bots: List<TeamBotCard> = emptyList(), val tasks: List<TeamTask> = emptyList(), val artifact: TeamArtifact? = null,
    val directoryCached: Boolean = true, val directoryReady: Boolean = false, val readsReady: Boolean = false, val botsLoaded: Boolean = false, val botsFailed: Boolean = false,
    /** Counts the service's Bot change signals (a Bot, its tasks or its shared list changed). An open list reads itself again on each. */
    val botSignals: Long = 0, val loadingHistory: Boolean = false,
    /** Why the Bot of a Bot DM stayed silent, by task: the rows of the Bot's auto-reply log, read for the person who maintains the Bot and for nobody else ([TeamMessagingModel.loadTriageReasons]). */
    val triageReasons: Map<String, TeamTriageItem> = emptyMap())
/**
 * The selected team's full messaging: directory, reads, signals, the open thread, the HTTP command journal and the Bot
 * cards and runs. Owned by [TeamsWorkspace], which starts and stops it; [scope] runs on the main thread.
 */
internal class TeamMessagingModel @JvmOverloads constructor(private val scope: CoroutineScope, storageDirectory: File, private val apiRoot: String = TEAMS_API_ROOT, private val auth: FirebaseAuth = FirebaseAuth.getInstance(), private val db: FirebaseFirestore = FirebaseFirestore.getInstance(), private val inbox: InboxController? = null) {
    private val storage = TeamCommandStorage(storageDirectory)
    private val mutable = MutableStateFlow(TeamMessagingState())
    val state = mutable.asStateFlow()
    /** The signed-in member this model reads for. */
    var uid = ""
        private set
    private var teamId = ""
    private var generation = 0L
    private var active = false
    private var lifecycle = 0L
    private var running = false
    private var again = false
    private var artifactNeedsValidation = false
    private var job: Job? = null
    private var retry: Job? = null
    private var retryDelay = 1000L
    private var deadline: Job? = null
    private var listener: ListenerRegistration? = null
    private var readListener: ListenerRegistration? = null
    private var historyJob: Job? = null
    private var syncJob: Job? = null
    private var lastChange = 0L
    private var selectionEpoch = 0L
    private var memberListener: ListenerRegistration? = null
    private var signalListener: ListenerRegistration? = null
    private var membershipReady = false
    private var directoryCached = true
    private var messagesCached = true
    private var botsDirty = true
    /** The tasks whose reason was already looked for, found or not: a task is looked for once. */
    private val triageLooked = mutableSetOf<String>()
    private val authListener = FirebaseAuth.AuthStateListener { if (active && it.currentUser?.uid != uid) invalidate() }
    fun start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle++; this.uid = uid; teamId = team.id; generation = team.generation
        botsDirty = true; directoryCached = true; messagesCached = true; triageLooked.clear()
        mutable.value = TeamMessagingState()
        auth.addAuthStateListener(authListener)
        val life = lifecycle
        mutable.update { it.copy(cache = TeamMessageCache(uid, team.id, generation)) }
        val root = db.collection("teams").document(team.id)
        memberListener = root.collection("members").document(uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
            if (error != null) { listenerFailed(error); return@addSnapshotListener }
            if (snapshot?.getLong("generation") == generation) {
                if (!membershipReady) { membershipReady = true; listenDirectory(life) }
            } else if (membershipReady || snapshot?.metadata?.isFromCache == false) invalidate()
        }
        scope.launch { try { reload(life); refresh() } catch (error: Exception) { if (active && lifecycle == life) mutable.update { it.copy(error = "STORAGE_FAILED") } } }
    }
    private fun listenDirectory(life: Long) {
        val root = db.collection("teams").document(teamId)
        listener = root.collection("conversations").whereArrayContains("memberUids", uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
            if (error != null) { listenerFailed(error); return@addSnapshotListener }
            if (snapshot == null) return@addSnapshotListener
            try {
                val previous = mutable.value.cache?.conversations?.associateBy { it.id } ?: emptyMap()
                val conversations = try {
                    snapshot.documents.map { doc -> TeamMessageConversation.fromDocument(doc.id, doc.data ?: throw TeamMessageFailure("INVALID_DATA"), previous[doc.id]?.lastReadSeq ?: 0, ::firestoreMillis) }
                } catch (_: TeamMessageFailure) {
                    // One bad conversation document stops this team's listeners; a half-parsed list is never shown.
                    invalidData(); return@addSnapshotListener
                }
                val ids = conversations.map { it.id }.toSet()
                directoryCached = snapshot.metadata.isFromCache
                mutable.update { it.copy(cache = it.cache?.copy(conversations = conversations, threads = it.cache.threads.filterKeys(ids::contains)), offline = sdkOffline(), directoryCached = directoryCached, directoryReady = true) }
                val selectedId = mutable.value.selected
                if (selectedId in ids && readListener == null) select(selectedId)
                syncChanges()
                if (!snapshot.metadata.isFromCache) {
                    val selected = mutable.value.selected
                    if (selected.isNotEmpty() && selected !in ids && mutable.value.cache?.creation?.optString("conversationId") != selected) select("")
                    refresh()
                }
            } catch (_: Exception) { mutable.update { it.copy(error = "INVALID_MESSAGE_SNAPSHOT") } }
        }
        signalListener = root.collection("signals").addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
            if (error != null) listenerFailed(error) else if (snapshot?.metadata?.isFromCache == false) {
                artifactNeedsValidation = true; botsDirty = true; mutable.update { it.copy(botSignals = it.botSignals + 1) }; refresh()
            }
        }
    }
    private fun listenerFailed(error: FirebaseFirestoreException) {
        if (error.code in setOf(FirebaseFirestoreException.Code.PERMISSION_DENIED, FirebaseFirestoreException.Code.UNAUTHENTICATED)) invalidate()
        else mutable.update { it.copy(offline = true, error = "MESSAGE_LISTENER_FAILED") }
    }
    private fun sdkOffline(selected: String = mutable.value.selected): Boolean = directoryCached || (selected.isNotEmpty() && messagesCached)
    /** A stored document broke the contract: show nothing half-parsed, say so, and stop this team's listeners (Desktop `INVALID_DATA`). */
    private fun invalidData() {
        mutable.update { it.copy(cache = it.cache?.copy(conversations = emptyList(), threads = emptyMap()), selected = "", error = "INVALID_DATA", offline = true, bots = emptyList(), tasks = emptyList(), artifact = null, readsReady = false, triageReasons = emptyMap()) }
        stop()
    }
    /** The banner's Reconnect: ask the server again (Bot cards and runs included); the SDK listeners reconnect on their own. */
    fun reconnect() {
        botsDirty = true
        val id = mutable.value.selected
        if (id.isNotEmpty() && mutable.value.cache?.threads?.get(id) == null && historyJob?.isActive != true) select(id)
        else syncChanges()
        refresh()
    }
    fun stop() {
        active = false; lifecycle++; job?.cancel(); retry?.cancel(); deadline?.cancel(); deadline = null; job = null; retry = null
        listener?.remove(); readListener?.remove(); memberListener?.remove(); signalListener?.remove()
        listener = null; readListener = null; memberListener = null; signalListener = null
        historyJob?.cancel(); syncJob?.cancel(); membershipReady = false; selectionEpoch++; triageLooked.clear()
        auth.removeAuthStateListener(authListener); running = false; again = false
    }
    /** Sign-out or account change: stop everything and forget the team. */
    fun reset() { stop(); mutable.value = TeamMessagingState() }
    private fun invalidate() {
        val owner = uid; val team = teamId; val epoch = generation
        mutable.value = TeamMessagingState(error = "MEMBERSHIP_REQUIRED", revoked = true)
        scope.launch { try { withContext(Dispatchers.IO) { storage.update(owner, team, epoch) { TeamMessageCache(owner, team, epoch) } } } catch (_: Exception) { } }
        stop()
    }
    private suspend fun commit(life: Long, reduce: (TeamMessageCache) -> TeamMessageCache) {
        if (!active || life != lifecycle || auth.currentUser?.uid != uid) throw CancellationException()
        val owner = uid; val team = teamId; val epoch = generation
        val cache = withContext(Dispatchers.IO) { storage.update(owner, team, epoch, reduce) }
        if (!active || life != lifecycle || auth.currentUser?.uid != uid) throw CancellationException()
        mutable.update { it.copy(cache = cache.copy(conversations = it.cache?.conversations ?: emptyList(), threads = it.cache?.threads ?: emptyMap())) }
    }
    private suspend fun reload(life: Long) {
        if (!active || life != lifecycle || auth.currentUser?.uid != uid) throw CancellationException()
        val owner = uid; val team = teamId; val epoch = generation
        val cache = withContext(Dispatchers.IO) { storage.load(owner, team, epoch) }
        if (!active || life != lifecycle || auth.currentUser?.uid != uid) throw CancellationException()
        mutable.update { it.copy(cache = cache.copy(conversations = it.cache?.conversations ?: emptyList(), threads = it.cache?.threads ?: emptyMap())) }
    }
    fun select(id: String) {
        val conversation = mutable.value.cache?.conversations?.firstOrNull { it.id == id }
        val epoch = ++selectionEpoch
        val life = lifecycle
        messagesCached = true
        lastChange = conversation?.lastChangeSeq ?: 0
        readListener?.remove()
        readListener = null
        historyJob?.cancel()
        syncJob?.cancel()
        mutable.update { it.copy(selected = id, offline = sdkOffline(id), tasks = emptyList(), artifact = null,
            readsReady = false, loadingHistory = false, cache = it.cache?.copy(threads = emptyMap())) }
        if (id.isEmpty() || conversation == null) { refresh(); return }
        val thread = db.collection("teams").document(teamId).collection("conversations").document(id)
        readListener = thread.collection("reads").document(uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!selectionCurrent(life, epoch, id)) return@addSnapshotListener
            if (error != null) { listenerFailed(error); return@addSnapshotListener }
            if (snapshot == null) return@addSnapshotListener
            val seq = if (snapshot.getLong("generation") == generation) snapshot.getLong("lastReadSeq") ?: 0 else 0
            mutable.update { it.copy(readsReady = true,
                cache = it.cache?.copy(conversations = it.cache.conversations.map { c -> if (c.id == id) c.copy(lastReadSeq = maxOf(c.lastReadSeq, seq)) else c })) }
        }
        scope.launch { inbox?.prime(InboxRef("team", teamId, id, generation)) }
        loadHistory(id, older = false)
        refresh()
    }

    private fun selectionCurrent(life: Long, epoch: Long, id: String) =
        active && lifecycle == life && epoch == selectionEpoch && mutable.value.selected == id && auth.currentUser?.uid == uid

    private fun loadHistory(id: String, older: Boolean) {
        historyJob?.cancel()
        val life = lifecycle
        val epoch = selectionEpoch
        val prior = mutable.value.cache?.threads?.get(id)
        val before = if (older) prior?.messages?.firstOrNull()?.seq ?: return else null
        mutable.update { it.copy(loadingHistory = true) }
        historyJob = scope.launch {
            val messages = db.collection("teams").document(teamId).collection("conversations").document(id).collection("messages")
            var query = messages.orderBy("seq", Query.Direction.DESCENDING).limit(50)
            if (before != null) query = query.startAfter(before)
            try {
                val snapshot = query.get().await()
                if (!selectionCurrent(life, epoch, id)) return@launch
                val page = snapshot.documents.map(::parseMessage).sortedBy { it.seq }
                val latestWindow = mutable.value.cache?.threads?.get(id)
                val merged = if (older) (page + latestWindow?.messages.orEmpty()).distinctBy { it.id }.sortedBy { it.seq } else page
                val window = TeamMessageThread(
                    messages = if (older) merged.take(200) else merged.takeLast(200),
                    hasEarlier = page.size == 50 && (page.firstOrNull()?.seq ?: 0) > 1,
                    hasLater = older && (merged.size > 200 || latestWindow?.hasLater == true),
                )
                messagesCached = snapshot.metadata.isFromCache
                mutable.update { it.copy(cache = it.cache?.copy(threads = mapOf(id to window)), offline = sdkOffline()) }
                if (!messagesCached) syncChanges()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                if (selectionCurrent(life, epoch, id)) {
                    if (failure is FirebaseFirestoreException) listenerFailed(failure)
                    else mutable.update { it.copy(error = "INVALID_MESSAGE_SNAPSHOT") }
                }
            } finally {
                if (selectionCurrent(life, epoch, id)) mutable.update { it.copy(loadingHistory = false) }
            }
        }
    }

    fun loadEarlier(id: String) {
        if (mutable.value.selected == id && historyJob?.isActive != true) loadHistory(id, older = true)
    }

    fun jumpLatest(id: String) {
        if (mutable.value.selected == id) loadHistory(id, older = false)
    }

    private fun parseMessage(doc: DocumentSnapshot): TeamMessage {
        val data = JSONObject(doc.data ?: throw TeamMessageFailure("INVALID_DATA")).put("id", doc.id)
        for (key in listOf("createdAt", "editedAt")) {
            (doc.get(key) as? Timestamp)?.let { data.put(key, java.time.Instant.ofEpochMilli(it.toDate().time).toString()) }
        }
        return TeamMessage.parse(data)
    }

    private fun syncChanges() {
        val id = mutable.value.selected
        val initial = mutable.value.cache?.threads?.get(id) ?: return
        if (!active || id.isEmpty() || syncJob?.isActive == true || directoryCached) return
        if (initial.messages.isEmpty() && historyJob?.isActive == true) return
        val life = lifecycle
        val epoch = selectionEpoch
        syncJob = scope.launch {
            try {
                while (selectionCurrent(life, epoch, id)) {
                    val conversation = mutable.value.cache?.conversations?.firstOrNull { it.id == id } ?: return@launch
                    if (conversation.lastChangeSeq <= lastChange) break
                    val request = JSONObject().put("action", "sync").put("conversationId", id).put("after", lastChange).put("limit", 50)
                    val response = call(request, life)
                    if (!selectionCurrent(life, epoch, id)) return@launch
                    val changes = response.getJSONArray("changes").objects()
                    if (changes.isEmpty() || changes.withIndex().any { (index, change) -> change.getLong("seq") != lastChange + index + 1 }) {
                        throw TeamMessageFailure("CHANGE_GAP")
                    }
                    val updated = response.getJSONArray("messages").objects().map(TeamMessage::parse)
                    val current = mutable.value.cache?.threads?.get(id) ?: return@launch
                    val byId = current.messages.associateBy { it.id }.toMutableMap()
                    val first = current.messages.firstOrNull()?.seq
                    for (message in updated) {
                        val previous = byId[message.id]
                        if (previous != null && previous.changeSeq < message.changeSeq ||
                            previous == null && !current.hasLater && (first == null || message.seq >= first)) {
                            byId[message.id] = message
                        }
                    }
                    val merged = byId.values.sortedBy { it.seq }
                    val window = current.copy(messages = merged.takeLast(200), hasEarlier = current.hasEarlier || merged.size > 200)
                    lastChange = changes.last().getLong("seq")
                    messagesCached = false
                    mutable.update { it.copy(cache = it.cache?.copy(threads = mapOf(id to window)), offline = sdkOffline()) }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                if (selectionCurrent(life, epoch, id)) {
                    val code = (failure as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                    if (code in setOf("STALE_MEMBERSHIP", "MEMBERSHIP_REQUIRED", "CONVERSATION_REQUIRED", "INVALID_SESSION")) invalidate()
                    else mutable.update { it.copy(offline = true, error = code) }
                }
            }
        }
    }
    fun refreshTasks() { refresh() }
    fun refresh() {
        if (!active || mutable.value.revoked) return
        if (running) { again = true; return }
        running = true; val life = lifecycle
        job = scope.launch {
            try {
                reload(life)
                val creationLease = System.currentTimeMillis() + 30000
                if (mutable.value.cache?.creation != null) commit(life) { old -> val creation = old.creation; if (creation != null && !creation.has("error") && creation.optLong("leaseUntil") <= System.currentTimeMillis()) old.copy(creation = JSONObject(creation.toString()).put("leaseUntil", creationLease).put("attempted", true)) else old }
                mutable.value.cache?.creation?.takeIf { it.optLong("leaseUntil") == creationLease }?.let { creation ->
                    try {
                        val body = JSONObject(creation.toString()).also { it.remove("error"); it.remove("leaseUntil"); it.remove("attempted") }; call(body, life)
                        commit(life) { it.copy(creation = null) }
                        if (mutable.value.selected.isEmpty()) select(creation.optJSONObject("payload")?.getString("conversationId") ?: creation.getString("conversationId"))
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED")) throw error
                        val permanent = code in setOf("CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "INVALID_INPUT")
                        commit(life) { it.copy(creation = it.creation?.let { value -> JSONObject(value.toString()).also { item -> item.remove("leaseUntil"); if (permanent) item.put("error", code) } }) }
                        if (!permanent) throw error
                    }
                }
                reload(life)
                for (pending in mutable.value.cache?.outbox ?: emptyList()) {
                    if (pending.has("error")) continue
                    val id = pending.optJSONObject("payload")?.getString("changeId") ?: pending.getString("changeId"); val lease = System.currentTimeMillis() + 30000
                    commit(life) { it.copy(outbox = it.outbox.map { value -> if ((value.optJSONObject("payload")?.getString("changeId") ?: value.getString("changeId")) == id && !value.has("error") && value.optLong("leaseUntil") <= System.currentTimeMillis()) JSONObject(value.toString()).put("leaseUntil", lease).put("attempted", true) else value }) }
                    val intent = mutable.value.cache?.outbox?.find { (it.optJSONObject("payload")?.getString("changeId") ?: it.getString("changeId")) == id && it.optLong("leaseUntil") == lease } ?: continue
                    val body = JSONObject(intent.toString()).also { it.remove("error"); it.remove("leaseUntil"); it.remove("attempted"); it.remove("attachmentNames") }
                    try { call(body, life); commit(life) { it.copy(outbox = it.outbox.filter { pending -> (pending.optJSONObject("payload")?.getString("changeId") ?: pending.getString("changeId")) != id }) } }
                    catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        val permanent = code in setOf("MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "MENTION_TARGET_REQUIRED", "REQUEST_QUOTA_EXCEEDED", "CONVERSATION_POLICY_REQUIRED", "INPUT_GRANT_REQUIRED", "COMMAND_ID_CONFLICT", "BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "INVALID_INPUT", "WAIT_REQUIRED", "WAIT_RECIPIENT_REQUIRED", "WAIT_CLOSED", "TASK_REVOKED", "ATTACHMENT_REQUIRED", "ATTACHMENT_LIMIT", "UPLOAD_EXPIRED")
                        commit(life) { it.copy(outbox = it.outbox.map { value -> if ((value.optJSONObject("payload")?.getString("changeId") ?: value.getString("changeId")) == id) JSONObject(value.toString()).also { it.remove("leaseUntil"); if (permanent) it.put("error", code) } else value }) }
                        if (!permanent) throw error
                    }
                }
                for (pending in mutable.value.cache?.commands ?: emptyList()) {
                    if (pending.has("error")) continue
                    val commandId = pending.getString("commandId"); val lease = System.currentTimeMillis() + 30000
                    commit(life) { it.copy(commands = it.commands.map { value -> if (value.getString("commandId") == commandId && value.optLong("leaseUntil") <= System.currentTimeMillis()) JSONObject(value.toString()).put("leaseUntil", lease).put("attempted", true) else value }) }
                    val intent = mutable.value.cache?.commands?.find { it.getString("commandId") == commandId && it.optLong("leaseUntil") == lease } ?: continue
                    try {
                        call(JSONObject(intent.toString()).also { it.remove("leaseUntil"); it.remove("attempted"); it.remove("error") }, life)
                        commit(life) { it.copy(commands = it.commands.filter { value -> value.getString("commandId") != commandId }) }
                        botsDirty = true
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        val permanent = code in setOf("INVALID_INPUT", "COMMAND_ID_CONFLICT", "BOT_REVISION_CONFLICT", "MAINTAINER_REQUIRED")
                        commit(life) { it.copy(commands = it.commands.map { value -> if (value.getString("commandId") == commandId) JSONObject(value.toString()).also { item -> item.remove("leaseUntil"); if (permanent) item.put("error", code) } else value }) }
                        if (!permanent) throw error
                    }
                }
                if (botsDirty) {
                    botsDirty = false
                    try { val bots = execute("listSharedBots", JSONObject(), life).getJSONArray("bots").objects().map(TeamBotCard::parse); mutable.update { it.copy(bots = TeamBotEntry.acceptSnapshot(it.bots, bots, System.currentTimeMillis()), botsLoaded = true, botsFailed = false) } }
                    catch (error: Exception) { botsDirty = true; throw error }
                }
                if (artifactNeedsValidation) {
                    artifactNeedsValidation = false
                    mutable.value.artifact?.let { current ->
                        try { execute("readArtifact", JSONObject().put("artifactId", current.artifactId), life) }
                        catch (error: CancellationException) { throw error }
                        catch (_: Exception) { mutable.update { it.copy(artifact = null) } }
                    }
                }
                val id = mutable.value.selected
                observeTasks(id.takeIf { selected -> mutable.value.cache?.conversations?.any { it.id == selected } == true }, life)
                mutable.update { it.copy(error = it.cache?.commands?.firstOrNull { intent -> intent.has("error") }?.getString("error") ?: "") }; retryDelay = 1000L
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                if (active && lifecycle == life) {
                    val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                    again = false
                    mutable.update { it.copy(offline = true, error = code, bots = it.bots.map(TeamBotEntry::unverified), tasks = emptyList(), artifact = null, botsFailed = true) }
                    if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate()
                }
            } finally { if (lifecycle == life) { running = false; scheduleRetry(); scheduleDeadline(); if (again) { again = false; refresh() } } }
        }
    }
    private suspend fun observeTasks(conversationId: String?, life: Long) {
        val payload = JSONObject(); conversationId?.let { payload.put("conversationId", it) }
        val selection = mutable.value.selected
        val tasks = execute("listTasks", payload, life).getJSONArray("tasks").objects().map(TeamTask::parse)
        mutable.update { if (it.selected == selection) it.copy(tasks = tasks) else it }
    }
    fun refreshTask(taskId: String) = launchOperation {
        val selection = mutable.value.selected
        val task = TeamTask.parse(execute("getTask", JSONObject().put("taskId", taskId), lifecycle))
        mutable.update { if (it.selected == selection) it.copy(tasks = if (it.tasks.any { old -> old.taskId == taskId }) it.tasks.map { old -> if (old.taskId == taskId) task else old } else it.tasks + task) else it }
    }
    /**
     * Sets which Bots of a Chat judge for themselves whether to answer it, and, when given, how many of them answer one message nobody asked them about (1 to 5)
     * and whether a message that mentions only people is judged like any other (`judge`) or is for them alone (`quiet`); what is left out is left as it is.
     * The listener brings the new setting.
     */
    fun setConversationAutoReply(conversationId: String, expectedRevision: Long, targets: List<String>, maxReplies: Int? = null, humanMentions: String? = null, onFailed: () -> Unit = {}) = launchOperation {
        val payload = JSONObject().put("conversationId", conversationId).put("expectedRevision", expectedRevision).put("targets", JSONArray(targets))
        if (maxReplies != null) payload.put("maxReplies", maxReplies)
        if (humanMentions != null) payload.put("humanMentions", humanMentions)
        try { execute("setConversationAutoReply", payload, lifecycle) }
        catch (error: Exception) { if (error !is CancellationException) onFailed(); throw error }
    }
    /**
     * Sets what one Bot of a Chat may use here, within what it published: [narrowing] is the whole narrowing, null lifts it. [expectedRevision] is the revision of
     * that Bot's entry in the Chat (0 when it has none), so a change someone else made meanwhile is not written over. Returns the revision of the Bot's entry that
     * the write made: the Chat's listener has brought the new setting once its entry has reached it. Fails with a [TeamMessageFailure].
     */
    suspend fun setConversationBotScope(conversationId: String, sharedBotId: String, expectedRevision: Long, narrowing: TeamBotScopeNarrowing?): Long = onModel {
        execute("setConversationBotScope", JSONObject().put("conversationId", conversationId).put("sharedBotId", sharedBotId).put("expectedRevision", expectedRevision)
            .put("scope", narrowing?.json() ?: JSONObject.NULL), lifecycle).getLong("revision")
    }
    /** Reads the Bot cards again: what a Bot shares may have changed under the person looking at it. */
    fun refreshBots() { botsDirty = true; refresh() }
    /** Stops a task: at once while it is queued or waiting, at the worker's next check while the Bot is working. A task that has already ended stays as it is. */
    fun cancelTask(taskId: String) = launchOperation {
        try { execute("cancelTask", JSONObject().put("taskId", taskId), lifecycle) }
        catch (error: TeamMessageFailure) { if (error.code != "TASK_FINISHED") throw error }
        refreshTask(taskId)
    }
    /** Approves or declines the list change a task waits on. [ids] are made once per decision by the caller: sending them again repeats the same command. */
    fun decideWait(waitId: String, taskId: String, approve: Boolean, ids: TeamDecisionIds) = launchOperation {
        val payload = JSONObject().put("conversationId", mutable.value.selected).put("waitId", waitId).put("messageId", ids.messageId).put("changeId", ids.changeId).put("decision", if (approve) "approve" else "deny")
        call(executionCommand("decideWait", payload, ids.commandId), lifecycle)
        refreshTask(taskId)
    }
    fun openArtifact(artifactId: String) = launchOperation {
        val selection = mutable.value.selected
        val artifact = TeamArtifact.parse(execute("readArtifact", JSONObject().put("artifactId", artifactId), lifecycle))
        mutable.update { if (it.selected == selection) it.copy(artifact = artifact) else it }
    }
    fun closeArtifact() { mutable.update { it.copy(artifact = null) } }
    /** One revision of a document ([revision] null: the newest). Reading checks the current Team membership every time. */
    suspend fun readArtifact(artifactId: String, revision: Long? = null): TeamArtifact = onModel {
        TeamArtifact.parse(execute("readArtifact", JSONObject().put("artifactId", artifactId).apply { if (revision != null) put("revision", revision) }, lifecycle))
    }
    suspend fun listArtifactRevisions(artifactId: String): TeamArtifactRevisions = onModel { TeamArtifactRevisions.parse(execute("listArtifactRevisions", JSONObject().put("artifactId", artifactId), lifecycle)) }
    /** Saves [draft] as the revision after [expectedRevision], or saves nothing. The same [operationId] sent again repeats the same request. */
    suspend fun reviseArtifact(artifactId: String, expectedRevision: Long, operationId: String, draft: TeamDocumentDraft) {
        onModel { execute("reviseArtifact", JSONObject().put("artifactId", artifactId).put("expectedRevision", expectedRevision).put("operationId", operationId).put("document", draft.json()), lifecycle) }
    }
    /** What a task did outside the Chat. Only its requester and the Bot's maintainer may ask. */
    suspend fun listOperations(taskId: String): List<TeamTaskOperation> = onModel { TeamTaskOperation.list(execute("listOperations", JSONObject().put("taskId", taskId), lifecycle)) }
    /** The maintainer states what happened to an action nobody could verify. The task is read again either way: a refusal often means it has moved. */
    suspend fun resolveOperation(taskId: String, operationId: String, happened: Boolean) {
        onModel {
            try { execute("resolveOperation", JSONObject().put("taskId", taskId).put("operationId", operationId).put("resolution", if (happened) "succeeded" else "failed"), lifecycle) }
            finally { refreshTask(taskId) }
        }
    }
    /** A Bot's shared list as it is now. It is read on demand: the list is not part of the message stream. */
    suspend fun readList(sharedBotId: String): TeamSharedList = onModel { TeamSharedList.parse(execute("readList", JSONObject().put("sharedBotId", sharedBotId), lifecycle)) }
    /** Saves [changes] on top of [expectedDataRevision], or saves nothing. The same [operationId] sent again repeats the same request, so a save whose answer was lost is not applied twice. */
    /** A page of the list's history, newest first; [beforeRevision] is the oldest revision of the page before. */
    suspend fun listHistory(sharedBotId: String, beforeRevision: Long? = null): TeamSharedListHistory = onModel {
        val payload = JSONObject().put("sharedBotId", sharedBotId); if (beforeRevision != null) payload.put("beforeRevision", beforeRevision)
        TeamSharedListHistory.parse(execute("listHistory", payload, lifecycle))
    }
    /**
     * A page of a Bot's auto-reply log, newest verdict first, for the person who maintains the Bot (anyone else is refused: `MAINTAINER_REQUIRED`). [conversationId] asks
     * for one Chat; [cursor] is the `nextCursor` of the page before; the service gives at most [TeamExecutionContract.TRIAGE_LIMIT] rows.
     */
    suspend fun listTriage(sharedBotId: String, conversationId: String? = null, cursor: String? = null, limit: Int = TeamTriageRules.PAGE): TeamTriagePage = onModel {
        val payload = JSONObject().put("sharedBotId", sharedBotId).put("limit", limit)
        if (conversationId != null) payload.put("conversationId", conversationId)
        if (cursor != null) payload.put("cursor", cursor)
        TeamTriagePage.parse(execute("listTriage", payload, lifecycle))
    }
    /**
     * Looks the verdicts of these tasks of a Bot DM up in the Bot's log, once each, for [TeamMessagingState.triageReasons]. The reason of a silence is a courtesy: a log
     * that cannot be read, or a task with no row in it, leaves the line that says the Bot did not reply as it was. Only ever asked for by the Bot's maintainer.
     */
    suspend fun loadTriageReasons(sharedBotId: String, conversationId: String, taskIds: List<String>) {
        onModel {
            val fresh = taskIds.filter { it !in triageLooked && it !in mutable.value.triageReasons }
            if (!active || fresh.isEmpty()) return@onModel
            triageLooked += fresh
            val life = lifecycle
            val found = try { TeamTriageSearch.find(fresh) { cursor, limit -> listTriage(sharedBotId, conversationId, cursor, limit) } }
                // A search that was cut short by the screen asking again is not a look: the next one looks for these.
                catch (cancelled: CancellationException) { triageLooked -= fresh.toSet(); throw cancelled }
            if (!active || life != lifecycle || found.isEmpty()) return@onModel
            mutable.update { it.copy(triageReasons = it.triageReasons + found) }
        }
    }
    /**
     * The replies of a Bot that wait for the person who maintains it, as it made them (anyone else is refused: `MAINTAINER_REQUIRED`): in one Chat when [conversationId] is given,
     * else in every Chat of the Team. The service keeps a held reply where only its maintainer can read it, so this is the one way a client sees one.
     */
    suspend fun listHeldReplies(sharedBotId: String, conversationId: String? = null): List<TeamHeldReply> = onModel {
        TeamHeldAnswer.parse(execute("listHeldReplies", TeamHeldRules.listPayload(sharedBotId, conversationId), lifecycle))
    }
    /**
     * Lets a held reply go, as the Bot made it or with [text] as the maintainer changed it to. [commandId] is made once per decision by the caller: sending it again repeats the same
     * command, so an approval whose answer was lost is answered again and not decided twice. A reply that is no longer held is refused with `REPLY_APPROVED`, `REPLY_REJECTED`,
     * `REPLY_EXPIRED` or `REPLY_SUPERSEDED`, for whichever came first.
     */
    suspend fun approveReply(taskId: String, text: String?, commandId: String): TeamHeldDecision = onModel {
        TeamHeldDecision.parse(call(executionCommand("approveReply", TeamHeldRules.approvePayload(taskId, text), commandId), lifecycle))
    }
    /** Drops a held reply: nothing is posted. Refused like an approval when the reply is no longer held. */
    suspend fun rejectReply(taskId: String, commandId: String): TeamHeldDecision = onModel {
        TeamHeldDecision.parse(call(executionCommand("rejectReply", TeamHeldRules.rejectPayload(taskId), commandId), lifecycle))
    }
    suspend fun writeList(sharedBotId: String, operationId: String, expectedDataRevision: Long, changes: List<TeamSharedListChange>) {
        onModel { execute("writeList", JSONObject().put("sharedBotId", sharedBotId).put("operationId", operationId).put("expectedDataRevision", expectedDataRevision).put("changes", changes.json()), lifecycle) }
    }
    /** Runs a caller's request where the model's own work runs, so it sees the same account, team and generation checks. */
    private suspend fun <T> onModel(block: suspend () -> T): T =
        kotlinx.coroutines.withContext(scope.coroutineContext[kotlin.coroutines.ContinuationInterceptor] ?: kotlin.coroutines.EmptyCoroutineContext) { block() }
    fun create(kind: String, title: String, parties: List<String>, id: String, sharedBotIds: List<String> = emptyList(), shouldOpen: () -> Boolean = { true }, onSuccess: () -> Unit) {
        val life = lifecycle
        launchOperation {
            if (!active || life != lifecycle || auth.currentUser?.uid != uid) throw CancellationException()
            requireTeamBotSelections(sharedBotIds, mutable.value.bots)
            if ((kind == "bot_dm" && sharedBotIds.size != 1) || (kind == "dm" && sharedBotIds.isNotEmpty())) throw TeamMessageFailure("BOT_SELECTION_STALE")
            val payload = JSONObject().put("conversationId", id).put("kind", kind).put("title", title).put("participantUids", JSONArray(if (kind == "bot_dm") listOf(uid) else (listOf(uid) + parties).distinct()))
            val intent = if (sharedBotIds.isNotEmpty()) executionCommand("createConversation", payload.put("sharedBotIds", JSONArray(sharedBotIds)), id)
                else payload.put("action", "create").put("teamId", teamId).put("generation", generation)
            commit(life) { if (it.creation != null) throw TeamMessageFailure("CREATION_PENDING"); it.copy(creation = intent) }
            if (shouldOpen()) { select(id); onSuccess() }
        }
    }
    /**
     * [waitId] makes a new message the answer to a Bot's question: it is sent as `answerWait`, asks no Bot and starts no task.
     * [files]: checked uploads of [conversationId] that a new message carries; it may then have no text and ask no Bot. A message with
     * files is only ever queued for the conversation they were uploaded for.
     */
    fun queue(operation: String, text: String, messageId: String? = null, replyTo: String? = null, targets: List<String> = emptyList(), waitId: String? = null,
              files: List<TeamDraftFile> = emptyList(), conversationId: String? = null,
              mentionEntities: List<TeamMentionEntity>? = null, onSuccess: () -> Unit = {}) {
        if (mutable.value.submitting) return
        mutable.update { it.copy(submitting = true) }; val life = lifecycle
        launchOperation {
            try {
                val carries = operation == "create" && waitId == null && files.isNotEmpty()
                if (files.isNotEmpty() && (!carries || files.size > TeamAttachments.MESSAGE_FILES || files.any { it.state != TeamDraftFile.State.Ready } || conversationId != mutable.value.selected)) throw TeamMessageFailure("INVALID_INPUT")
                if (!active || mutable.value.revoked || mutable.value.selected.isEmpty() || (operation != "recall" && ((text.isBlank() && !carries) || text.toByteArray(Charsets.UTF_8).size > 8192))) throw TeamMessageFailure("INVALID_INPUT")
                if (waitId != null) {
                    if (operation != "create") throw TeamMessageFailure("INVALID_INPUT")
                    val changeId = UUID.randomUUID().toString()
                    val payload = JSONObject().put("conversationId", mutable.value.selected).put("waitId", waitId).put("messageId", UUID.randomUUID().toString()).put("changeId", changeId).put("text", text)
                    commit(life) { it.copy(outbox = it.outbox + executionCommand("answerWait", payload, changeId)) }; onSuccess(); refresh()
                    return@launchOperation
                }
                val targetsForSubmit = if (operation == "create") {
                    val conversation = mutable.value.cache?.conversations?.find { it.id == mutable.value.selected } ?: throw TeamMessageFailure("CONVERSATION_REQUIRED")
                    if (conversation.kind == "bot_dm" && conversation.sharedBotIds.size != 1) throw TeamMessageFailure("BOT_SELECTION_STALE")
                    val selected = if (conversation.kind == "bot_dm") conversation.sharedBotIds else targets
                    requireTeamBotSelections(selected, mutable.value.bots, conversation.participants)
                    // A group or channel still takes a message for a Bot whose Desktop is off (rule 6); a Bot DM does not.
                    TeamBotEntry.requireNewRequest(selected, mutable.value.bots, waitsForHost = conversation.kind != "bot_dm")
                    selected
                } else emptyList()
                val said = if (carries && text.isBlank()) "" else text
                val intent = JSONObject().put("action", "mutate").put("teamId", teamId).put("generation", generation).put("conversationId", mutable.value.selected).put("messageId", messageId ?: UUID.randomUUID().toString()).put("changeId", UUID.randomUUID().toString()).put("operation", operation).put("text", said)
                replyTo?.let { intent.put("replyTo", it) }
                val entities = mentionEntities?.let { JSONArray(it.map { entity -> entity.json() }) }
                if (entities != null) intent.put("mentionEntities", entities)
                val command = if (targetsForSubmit.isNotEmpty() || carries) {
                    val payload = JSONObject().put("conversationId", intent.getString("conversationId")).put("messageId", intent.getString("messageId")).put("changeId", intent.getString("changeId")).put("text", said).put("targets", JSONArray(targetsForSubmit.distinct().sorted()))
                    replyTo?.let { payload.put("replyTo", it) }
                    if (entities != null) payload.put("mentionEntities", entities)
                    if (carries) payload.put("attachments", JSONArray(files.map { it.uploadId }))
                    executionCommand("submit", payload, intent.getString("changeId")).also { if (carries) it.put("attachmentNames", JSONArray(files.map { file -> file.name })) }
                } else intent
                commit(life) { it.copy(outbox = it.outbox + command) }; onSuccess(); refresh()
            } finally { if (life == lifecycle) mutable.update { it.copy(submitting = false) } }
        }
    }
    fun pause(bot: TeamBotCard) = launchOperation {
        if (!bot.canPause(uid, generation)) throw TeamMessageFailure("MAINTAINER_REQUIRED")
        val intent = executionCommand("pauseSharedBot", JSONObject().put("sharedBotId", bot.sharedBotId).put("expectedRevision", bot.revision), UUID.randomUUID().toString())
        commit(lifecycle) { it.copy(commands = it.commands + intent) }; refresh()
    }
    fun discardCommand(commandId: String) = launchOperation { commit(lifecycle) { cache ->
        val intent = cache.commands.firstOrNull { it.getString("commandId") == commandId } ?: throw TeamMessageFailure("INVALID_INPUT")
        if (!intent.has("error") || intent.optLong("leaseUntil") > System.currentTimeMillis()) throw TeamMessageFailure("MESSAGE_IN_FLIGHT")
        cache.copy(commands = cache.commands.filter { it.getString("commandId") != commandId })
    }; refresh() }
    fun discard(id: String) = launchOperation {
        // The files of a message that was refused are no message's: they are given up, so they stop counting against the sender.
        val files = mutable.value.cache?.outbox?.firstOrNull { value -> value.has("error") && value.optJSONObject("payload")?.optString("changeId") == id }?.optJSONObject("payload")?.optJSONArray("attachments")?.strings() ?: emptyList()
        commit(lifecycle) { if (it.outbox.any { value -> (value.optJSONObject("payload")?.getString("changeId") ?: value.getString("changeId")) == id && ((value.optBoolean("attempted") && !value.has("error")) || value.optLong("leaseUntil") > System.currentTimeMillis()) }) throw TeamMessageFailure("MESSAGE_IN_FLIGHT"); it.copy(outbox = it.outbox.filter { value -> (value.optJSONObject("payload")?.getString("changeId") ?: value.getString("changeId")) != id }) }
        for (uploadId in files) cancelUpload(uploadId)
    }
    fun discardCreation() = launchOperation { commit(lifecycle) { if ((it.creation?.optBoolean("attempted") == true && !it.creation.has("error")) || (it.creation?.optLong("leaseUntil") ?: 0) > System.currentTimeMillis()) throw TeamMessageFailure("MESSAGE_IN_FLIGHT"); it.copy(creation = null) } }
    fun markRead(conversationId: String, seq: Long, automatic: Boolean = false) {
        val ref = InboxRef("team", teamId, conversationId, generation)
        if (!active || !ref.isValid()) return
        if (inbox != null) {
            val row = inbox.snapshot(ref)
            val manualRevision = row?.manualUnreadRevision ?: 0L
            inbox.mutate(InboxOperation.read(ref, seq, manualRevision, row?.pendingActionRevision, row?.failureRevision.takeUnless { automatic }), automatic = automatic)
            return
        }
        launchOperation {
            val value = call(JSONObject().put("action", "read").put("conversationId", conversationId).put("lastReadSeq", seq), lifecycle).getLong("lastReadSeq")
            mutable.update { it.copy(cache = it.cache?.copy(conversations = it.cache.conversations.map { c -> if (c.id == conversationId) c.copy(lastReadSeq = maxOf(c.lastReadSeq, value)) else c })) }
        }
    }
    private fun launchOperation(block: suspend () -> Unit) { val life = lifecycle; scope.launch { try { block() } catch (error: CancellationException) { throw error } catch (error: Exception) { if (active && life == lifecycle) { val code = (error as? TeamMessageFailure)?.code ?: "STORAGE_FAILED"; mutable.update { it.copy(error = code) }; if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate() } } } }
    private fun scheduleDeadline() {
        deadline?.cancel(); deadline = null
        val now = System.currentTimeMillis()
        val expirations = listOfNotNull(TeamBotEntry.nextStatusDeadline(mutable.value.bots, now)) + mutable.value.tasks.filter { !it.terminal }.map { it.deadline }
        val next = expirations.filter { it > now }.minOrNull() ?: return
        val life = lifecycle
        if (active) deadline = scope.launch {
            delay(next - now)
            if (!active || life != lifecycle) return@launch
            mutable.update { it.copy(bots = TeamBotEntry.expireStatuses(it.bots, System.currentTimeMillis())) }
            botsDirty = true
            refresh()
        }
    }
    private fun scheduleRetry() {
        retry?.cancel(); retry = null
        if (!active) return
        val cache = mutable.value.cache ?: return
        val pending = (cache.outbox + cache.commands + listOfNotNull(cache.creation)).filter { !it.has("error") }
        val earliest = (pending.map { it.optLong("leaseUntil") } + if (mutable.value.tasks.any { it.busy }) listOf(System.currentTimeMillis() + 3000) else emptyList()).minOrNull() ?: return
        val wait = maxOf(retryDelay, earliest - System.currentTimeMillis()); retryDelay = minOf(30000L, retryDelay * 2)
        retry = scope.launch { delay(wait); refresh() }
    }
    private fun executionCommand(action: String, payload: JSONObject, commandId: String = UUID.randomUUID().toString()) =
        JSONObject().put("schemaVersion", 1).put("commandId", commandId).put("teamId", teamId).put("generation", generation).put("action", action).put("payload", payload)
    // --- files -------------------------------------------------------------------------------------

    /**
     * Uploads one file for one conversation and has it checked (Desktop `uploadAttachment`). Each step has its own stable command id, so a
     * step repeated after a lost answer is the same step. Throws the service's code when the file is refused (`UPLOAD_TYPE_MISMATCH`,
     * `UPLOAD_DIGEST_MISMATCH`, `UPLOAD_QUOTA_EXCEEDED`, …) and `INVALID_SESSION` when this team was closed or the account changed
     * meanwhile: nothing more is sent then.
     */
    suspend fun upload(conversationId: String, uploadId: String, name: String, mime: String, bytes: ByteArray, onProgress: (Int) -> Unit) = onModel {
        val life = lifecycle
        val digest = withContext(Dispatchers.Default) { TeamAttachments.sha256Hex(bytes) }
        val created = insist { call(executionCommand("createUpload", JSONObject().put("conversationId", conversationId).put("uploadId", uploadId).put("name", name).put("mime", mime).put("byteLength", bytes.size).put("sha256", digest), "${uploadId}_c"), life) }
        if (created.optString("state") == "verified") { onProgress(bytes.size); return@onModel }
        val parts = created.optInt("parts"); val partBytes = created.optInt("partBytes")
        if (created.optString("state") != "uploading" || partBytes <= 0 || parts <= 0) throw TeamMessageFailure(created.optString("code").ifEmpty { "UPLOAD_FAILED" })
        for (index in 0 until parts) {
            val from = index.toLong() * partBytes
            if (from >= bytes.size) throw TeamMessageFailure("UPLOAD_FAILED")
            val until = minOf(bytes.size.toLong(), from + partBytes).toInt()
            val data = withContext(Dispatchers.Default) { java.util.Base64.getEncoder().encodeToString(bytes.copyOfRange(from.toInt(), until)) }
            insist { call(executionCommand("putUploadPart", JSONObject().put("uploadId", uploadId).put("index", index).put("data", data), "${uploadId}_p$index"), life, 90) }
            onProgress(until)
        }
        val checked = insist { call(executionCommand("finalizeUpload", JSONObject().put("uploadId", uploadId), "${uploadId}_f"), life, 60) }
        if (checked.optString("state") != "verified") throw TeamMessageFailure(checked.optString("code").ifEmpty { "UPLOAD_FAILED" })
    }
    /** Gives up a file that no message carries. Best effort: one that is not given up here ends by itself after a day. */
    suspend fun cancelUpload(uploadId: String) {
        try { onModel { execute("cancelUpload", JSONObject().put("uploadId", uploadId), lifecycle) } }
        catch (error: CancellationException) { throw error } catch (_: Exception) { }
    }
    /**
     * A file of a message, whole and checked against what the message says it is. Asked of the service every time and kept nowhere:
     * only someone who reads that conversation now is answered.
     */
    suspend fun download(file: TeamMessageAttachment): ByteArray = onModel {
        val life = lifecycle
        suspend fun read(index: Int) = call(executionCommand("readAttachmentPart", JSONObject().put("attachmentId", file.attachmentId).put("index", index)), life, 90)
        val first = read(0)
        val parts = mutableListOf(first.getString("data"))
        for (index in 1 until first.getInt("parts")) parts += read(index).getString("data")
        withContext(Dispatchers.Default) { TeamAttachments.assemble(parts, file) }
    }
    /** The sender takes a file of their message back: its bytes are deleted and the message says so. */
    suspend fun revokeAttachment(attachmentId: String) {
        onModel { execute("revokeAttachment", JSONObject().put("attachmentId", attachmentId).put("changeId", UUID.randomUUID().toString()), lifecycle) }
    }
    /** A step of an upload: tried again, a little later each time, unless the answer will not change by asking again. */
    private suspend fun insist(step: suspend () -> JSONObject): JSONObject {
        var attempt = 0
        while (true) {
            try { return step() }
            catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                if (attempt >= 3 || TeamAttachments.isFinal((error as? TeamMessageFailure)?.code.orEmpty())) throw error
                delay(uploadRetryMs shl attempt); attempt++
            }
        }
    }
    /** The first pause before a step of an upload is tried again; a test shortens it. */
    internal var uploadRetryMs = 500L

    private suspend fun execute(action: String, payload: JSONObject, life: Long) = call(executionCommand(action, payload), life)
    suspend fun data(conversationId: String, command: JSONObject, operation: String? = null): JSONObject =
        call(JSONObject().put("command", command).apply { if (operation != null) put("operation", operation) },
            lifecycle, dataConversation = conversationId)

    suspend fun dataset(conversationId: String, body: JSONObject): JSONObject =
        call(body, lifecycle, dataConversation = conversationId, dataset = true)

    fun datasetShareDestination(conversationId: String): Pair<JSONObject, Long> =
        JSONObject().put("team", teamId).put("conversation", conversationId) to generation

    private suspend fun call(input: JSONObject, life: Long, seconds: Long = 20, dataConversation: String? = null, dataset: Boolean = false): JSONObject {
        val owner = uid
        val user = auth.currentUser
        if (!active || life != lifecycle || user?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        val token = requireNotNull(user.getIdToken(false).await().token)
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        if (dataConversation != null && mutable.value.selected != dataConversation) throw TeamMessageFailure("CONVERSATION_REQUIRED")
        val body = JSONObject(input.toString())
        if (!dataset) {
            if (dataConversation == null) body.put("teamId", teamId).put("generation", generation)
            else body.put("scope", JSONObject().put("kind", "team").put("teamId", teamId)
                .put("generation", generation).put("conversationId", dataConversation))
        }
        if (body.has("schemaVersion")) TeamExecutionContract.validate(body)
        val path = when {
            dataset -> "datasets"
            dataConversation != null -> "data"
            else -> "messages"
        }
        val result = TeamHttp.request("$apiRoot/v1/teams/$path", token, body, seconds)
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        if (dataConversation != null && mutable.value.selected != dataConversation) throw TeamMessageFailure("CONVERSATION_REQUIRED")
        return result
    }
}

/** A Firestore timestamp as epoch milliseconds; null for any other value (the strict `lastMessage.at` rule). */
internal fun firestoreMillis(value: Any): Long? = (value as? Timestamp)?.let { it.seconds * 1000 + it.nanoseconds / 1_000_000 }
