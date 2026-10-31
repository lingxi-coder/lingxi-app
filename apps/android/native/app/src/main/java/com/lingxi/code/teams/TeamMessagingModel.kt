package com.lingxi.code.teams

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.Timestamp
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreException
import com.google.firebase.firestore.DocumentChange
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.CancellationException
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

internal data class TeamMessagingState(val cache: TeamMessageCache? = null, val selected: String = "", val offline: Boolean = true, val error: String = "", val revoked: Boolean = false, val submitting: Boolean = false, val bots: List<TeamBotCard> = emptyList(), val botRuns: Map<String, TeamBotRun> = emptyMap())
internal class TeamMessagingModel @JvmOverloads constructor(application: Application, private val apiRoot: String = "https://lingxios.net", private val auth: FirebaseAuth = FirebaseAuth.getInstance(), private val db: FirebaseFirestore = FirebaseFirestore.getInstance()) : AndroidViewModel(application) {
    private val storage = TeamCommandStorage(File(application.noBackupFilesDir, "team-messages"))
    private val mutable = MutableStateFlow(TeamMessagingState())
    val state = mutable.asStateFlow()
    private var uid = ""
    private var teamId = ""
    private var generation = 0L
    private var active = false
    private var lifecycle = 0L
    private var running = false
    private var again = false
    private var botHistoryLimit = 100
    private var job: Job? = null
    private var retry: Job? = null
    private var retryDelay = 1000L
    private var deadline: Job? = null
    private var listener: ListenerRegistration? = null
    private var messageListener: ListenerRegistration? = null
    private var memberListener: ListenerRegistration? = null
    private var signalListener: ListenerRegistration? = null
    private val readListeners = mutableMapOf<String, ListenerRegistration>()
    private var membershipReady = false
    private var directoryCached = true
    private var messagesCached = true
    private var botsDirty = true
    private val terminalRuns = linkedMapOf<String, TeamBotRun>()
    private val missingRuns = linkedMapOf<String, String>()
    private val authListener = FirebaseAuth.AuthStateListener { if (active && it.currentUser?.uid != uid) invalidate() }
    fun start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle++; this.uid = uid; teamId = team.id; generation = team.generation
        botsDirty = true; directoryCached = true; messagesCached = true
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
        viewModelScope.launch { try { reload(life); refresh() } catch (error: Exception) { if (active && lifecycle == life) mutable.update { it.copy(error = "STORAGE_FAILED") } } }
    }
    private fun listenDirectory(life: Long) {
        val root = db.collection("teams").document(teamId)
        listener = root.collection("conversations").whereArrayContains("memberUids", uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
            if (error != null) { listenerFailed(error); return@addSnapshotListener }
            if (snapshot == null) return@addSnapshotListener
            try {
                val previous = mutable.value.cache?.conversations?.associateBy { it.id } ?: emptyMap()
                val conversations = snapshot.documents.map { doc ->
                    TeamMessageConversation.parse(JSONObject(doc.data ?: emptyMap<String, Any>()).put("id", doc.id).put("lastReadSeq", previous[doc.id]?.lastReadSeq ?: 0))
                }
                val ids = conversations.map { it.id }.toSet()
                directoryCached = snapshot.metadata.isFromCache
                mutable.update { it.copy(cache = it.cache?.copy(conversations = conversations, threads = it.cache.threads.filterKeys(ids::contains)), offline = sdkOffline()) }
                if (messageListener == null && mutable.value.selected in ids) select(mutable.value.selected)
                for (id in readListeners.keys.toList()) if (id !in ids) readListeners.remove(id)?.remove()
                for (id in ids) if (id !in readListeners) {
                    readListeners[id] = root.collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener(MetadataChanges.INCLUDE) { mark, failure ->
                        if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
                        if (failure != null) { listenerFailed(failure); return@addSnapshotListener }
                        val seq = if (mark?.getLong("generation") == generation) mark.getLong("lastReadSeq") ?: 0 else 0
                        mutable.update { it.copy(cache = it.cache?.copy(conversations = it.cache.conversations.map { c -> if (c.id == id) c.copy(lastReadSeq = seq) else c })) }
                    }
                }
                if (!snapshot.metadata.isFromCache) {
                    missingRuns.clear()
                    val selected = mutable.value.selected
                    if (selected.isNotEmpty() && selected !in ids && mutable.value.cache?.creation?.optString("conversationId") != selected) select("")
                    refresh()
                }
            } catch (_: Exception) { mutable.update { it.copy(error = "INVALID_MESSAGE_SNAPSHOT") } }
        }
        signalListener = root.collection("signals").addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid) return@addSnapshotListener
            if (error != null) listenerFailed(error) else if (snapshot?.metadata?.isFromCache == false) { botsDirty = true; missingRuns.clear(); refresh() }
        }
    }
    private fun listenerFailed(error: FirebaseFirestoreException) {
        if (error.code in setOf(FirebaseFirestoreException.Code.PERMISSION_DENIED, FirebaseFirestoreException.Code.UNAUTHENTICATED)) invalidate()
        else mutable.update { it.copy(offline = true, error = "MESSAGE_LISTENER_FAILED") }
    }
    private fun sdkOffline(selected: String = mutable.value.selected): Boolean = directoryCached || (selected.isNotEmpty() && messagesCached)
    fun stop() {
        active = false; lifecycle++; job?.cancel(); retry?.cancel(); deadline?.cancel(); deadline = null; job = null; retry = null
        listener?.remove(); messageListener?.remove(); memberListener?.remove(); signalListener?.remove()
        listener = null; messageListener = null; memberListener = null; signalListener = null
        readListeners.values.forEach { it.remove() }; readListeners.clear(); membershipReady = false
        auth.removeAuthStateListener(authListener); running = false; again = false; terminalRuns.clear(); missingRuns.clear(); botHistoryLimit = 100
    }
    private fun invalidate() {
        val owner = uid; val team = teamId; val epoch = generation
        mutable.value = TeamMessagingState(error = "MEMBERSHIP_REQUIRED", revoked = true)
        viewModelScope.launch { try { withContext(Dispatchers.IO) { storage.update(owner, team, epoch) { TeamMessageCache(owner, team, epoch) } } } catch (_: Exception) { } }
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
        messagesCached = true
        botHistoryLimit = 100
        mutable.update { it.copy(selected = id, offline = sdkOffline(id), botRuns = emptyMap(), cache = it.cache?.copy(threads = it.cache.threads.filterKeys { key -> key == id })) }; messageListener?.remove(); messageListener = null
        if (id.isEmpty() || mutable.value.cache?.conversations?.none { it.id == id } != false) { refresh(); return }
        val life = lifecycle
        var firstSnapshot = true
        messageListener = db.collection("teams").document(teamId).collection("conversations").document(id).collection("messages").orderBy("seq").addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (!active || lifecycle != life || auth.currentUser?.uid != uid || mutable.value.selected != id) return@addSnapshotListener
            if (error != null) { listenerFailed(error); return@addSnapshotListener }
            if (snapshot == null) return@addSnapshotListener
            try {
                val reset = firstSnapshot; firstSnapshot = false
                if (reset || snapshot.documentChanges.isNotEmpty()) {
                    val messages: MutableMap<String, TeamMessage> = if (reset) mutableMapOf() else (mutable.value.cache?.threads?.get(id)?.messages ?: emptyList()).associateBy { it.id }.toMutableMap()
                    for (change in snapshot.documentChanges) {
                        if (change.type == DocumentChange.Type.REMOVED) messages.remove(change.document.id)
                        else {
                            val data = JSONObject(change.document.data).put("id", change.document.id)
                            for (key in listOf("createdAt", "editedAt")) (change.document.get(key) as? Timestamp)?.let { data.put(key, java.time.Instant.ofEpochMilli(it.toDate().time).toString()) }
                            val message = TeamMessage.parse(data); messages[message.id] = message
                        }
                    }
                    val thread = TeamMessageThread(messages = messages.values.sortedBy { it.seq })
                    mutable.update { it.copy(cache = it.cache?.copy(threads = it.cache.threads + (id to thread))) }
                }
                messagesCached = snapshot.metadata.isFromCache
                mutable.update { it.copy(offline = sdkOffline()) }
                if (!snapshot.metadata.isFromCache) { mutable.update { it.copy(error = "") }; refresh() }
            } catch (_: Exception) { mutable.update { it.copy(error = "INVALID_MESSAGE_SNAPSHOT") } }
        }
        refresh()
    }
    fun showBotHistory(limit: Int) {
        val next = maxOf(100, limit)
        if (next != botHistoryLimit) { botHistoryLimit = next; refresh() }
    }
    fun refresh() {
        if (!active || mutable.value.revoked) return
        if (running) { again = true; return }
        running = true; val life = lifecycle
        mutable.update { it.copy(botRuns = emptyMap()) }
        job = viewModelScope.launch {
            try {
                reload(life)
                val creationLease = System.currentTimeMillis() + 30000
                if (mutable.value.cache?.creation != null) commit(life) { old -> val creation = old.creation; if (creation != null && !creation.has("error") && creation.optLong("leaseUntil") <= System.currentTimeMillis()) old.copy(creation = JSONObject(creation.toString()).put("leaseUntil", creationLease).put("attempted", true)) else old }
                mutable.value.cache?.creation?.takeIf { it.optLong("leaseUntil") == creationLease }?.let { creation ->
                    try {
                        val body = JSONObject(creation.toString()).also { it.remove("error"); it.remove("leaseUntil"); it.remove("attempted") }; call(body, life)
                        commit(life) { it.copy(creation = null) }
                        if (mutable.value.selected.isEmpty()) select(creation.getString("conversationId"))
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
                    val id = pending.getString("changeId"); val lease = System.currentTimeMillis() + 30000
                    commit(life) { it.copy(outbox = it.outbox.map { value -> if (value.getString("changeId") == id && !value.has("error") && value.optLong("leaseUntil") <= System.currentTimeMillis()) JSONObject(value.toString()).put("leaseUntil", lease).put("attempted", true) else value }) }
                    val intent = mutable.value.cache?.outbox?.find { it.getString("changeId") == id && it.optLong("leaseUntil") == lease } ?: continue
                    val body = JSONObject(intent.toString()).also { it.remove("error"); it.remove("leaseUntil"); it.remove("attempted") }
                    try { call(body, life); commit(life) { it.copy(outbox = it.outbox.filter { pending -> pending.getString("changeId") != intent.getString("changeId") }) } }
                    catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        val permanent = code in setOf("MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "INVALID_INPUT")
                        commit(life) { it.copy(outbox = it.outbox.map { value -> if (value.getString("changeId") == id) JSONObject(value.toString()).also { it.remove("leaseUntil"); if (permanent) it.put("error", code) } else value }) }
                        if (!permanent) throw error
                    }
                }
                for (pending in mutable.value.cache?.settlements ?: emptyList()) {
                    if (pending.has("error")) continue
                    val id = pending.getString("settlementId"); val lease = System.currentTimeMillis() + 30000
                    commit(life) { it.copy(settlements = it.settlements.map { value -> if (value.getString("settlementId") == id && !value.has("error") && value.optLong("leaseUntil") <= System.currentTimeMillis()) JSONObject(value.toString()).put("leaseUntil", lease).put("attempted", true) else value }) }
                    val intent = mutable.value.cache?.settlements?.find { it.getString("settlementId") == id && it.optLong("leaseUntil") == lease } ?: continue
                    try {
                        call(intent.getJSONObject("body"), life)
                        commit(life) { it.copy(settlements = it.settlements.filter { value -> value.getString("settlementId") != id }) }
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        val permanent = code in setOf("BOT_OWNER_REQUIRED", "BOT_REVISION_CONFLICT", "BOT_RESULT_CONFLICT", "RECOVERY_NOT_REQUIRED", "TRIGGER_REQUIRED", "BOT_RUN_BLOCKED", "INVALID_INPUT")
                        commit(life) { it.copy(settlements = it.settlements.map { value -> if (value.getString("settlementId") == id) JSONObject(value.toString()).also { item -> item.remove("leaseUntil"); if (permanent) item.put("error", code) } else value }) }
                        if (!permanent) throw error
                    }
                }
                // The same durable answerId and payload are retried after ACK loss or restart.
                for (pending in mutable.value.cache?.answers ?: emptyList()) {
                    if (pending.has("error")) continue
                    val answerId = pending.getString("answerId")
                    try {
                        // First sends need a live question. An attempted send may
                        // already have committed before unbind/rebind/recall; only
                        // the exact backend receipt, not botState, can confirm it.
                        val run = if (pending.optBoolean("attempted")) null else observeRun(pending.getString("conversationId"), pending.getString("triggerId"), life, pending.getString("questionId"))
                        val body = teamBotAnswerRetryBody(requireNotNull(mutable.value.cache), pending, run, System.currentTimeMillis())
                        commit(life) { it.copy(answers = it.answers.map { value -> if (value.getString("answerId") == answerId) JSONObject(value.toString()).put("attempted", true) else value }) }
                        call(body, life)
                        commit(life) { it.copy(answers = it.answers.filter { value -> value.getString("answerId") != answerId }) }
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        val permanent = code in setOf("QUESTION_RECIPIENT_REQUIRED", "QUESTION_REQUIRED", "ANSWER_CONFLICT", "INVALID_ANSWER", "BOT_USE_REQUIRED", "INVALID_INPUT")
                        if (!permanent) throw error
                        commit(life) { it.copy(answers = it.answers.map { value -> if (value.getString("answerId") == answerId) JSONObject(value.toString()).put("error", code) else value }) }
                    }
                }
                if (botsDirty) {
                    botsDirty = false
                    try { val bots = call(JSONObject().put("action", "bots"), life).getJSONArray("bots").objects().map(TeamBotCard::parse); mutable.update { it.copy(bots = bots) } }
                    catch (error: Exception) { botsDirty = true; throw error }
                }
                val id = mutable.value.selected
                if (id.isNotEmpty()) observeStates(id, life)
                mutable.update { it.copy(error = "") }; retryDelay = 1000L
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                if (active && lifecycle == life) {
                    val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                    again = false
                    mutable.update { it.copy(offline = true, error = code, bots = emptyList(), botRuns = emptyMap()) }
                    if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate()
                }
            } finally { if (lifecycle == life) { running = false; scheduleRetry(); scheduleDeadline(); if (again) { again = false; refresh() } } }
        }
    }
    private suspend fun observeStates(id: String, life: Long) {
        val cache = mutable.value.cache ?: return
        val conversation = cache.conversations.find { it.id == id } ?: return
        val fingerprints = mutableMapOf<String, String>()
        cache.threads[id]?.messages?.takeLast(botHistoryLimit)?.forEach { message ->
            visibleTeamBotTriggers(listOf(message), conversation, 1).forEach { trigger -> fingerprints[trigger] = "${message.id}/${message.changeSeq}/${conversation.participants.joinToString(",")}" }
        }
        val runs = mutableMapOf<String, TeamBotRun>()
        val missing = fingerprints.keys.filter { trigger ->
            val key = "$id/$trigger"
            val known = terminalRuns[key]
            if (known != null) { runs[trigger] = known; false } else missingRuns[key] != fingerprints[trigger]
        }
        for (batches in missing.chunked(30).chunked(4)) {
            val results = coroutineScope { batches.map { batch -> async { call(JSONObject().put("action", "botStates").put("conversationId", id).put("triggerIds", JSONArray(batch)), life).getJSONObject("states") } }.awaitAll() }
            for (states in results) for (trigger in states.keys()) {
                val key = "$id/$trigger"
                if (states.isNull(trigger)) missingRuns[key] = requireNotNull(fingerprints[trigger])
                else {
                    val run = TeamBotRun.parse(states.getJSONObject(trigger)); runs[trigger] = run
                    if (run.state in setOf("completed", "failed", "missed", "cancelled", "recovered", "revoked")) terminalRuns[key] = run
                }
            }
        }
        while (terminalRuns.size > 512) terminalRuns.remove(terminalRuns.keys.first())
        while (missingRuns.size > 512) missingRuns.remove(missingRuns.keys.first())
        if (mutable.value.selected == id) mutable.update { it.copy(botRuns = runs) }
    }
    fun create(kind: String, title: String, parties: List<String>, id: String, botKeys: List<String> = emptyList(), onSuccess: () -> Unit) = launchOperation {
        requireTeamBotSelections(botKeys, mutable.value.bots)
        if ((kind == "bot_dm" && botKeys.size != 1) || (kind == "dm" && botKeys.isNotEmpty())) throw TeamMessageFailure("BOT_SELECTION_STALE")
        val intent = JSONObject().put("action", "create").put("teamId", teamId).put("generation", generation).put("conversationId", id).put("kind", kind).put("title", title).put("participantUids", JSONArray(if (kind == "bot_dm") listOf(uid) else (listOf(uid) + parties).distinct())).put("botKeys", JSONArray(botKeys))
        commit(lifecycle) { if (it.creation != null) throw TeamMessageFailure("CREATION_PENDING"); it.copy(creation = intent) }
        select(id); onSuccess()
    }
    fun queue(operation: String, text: String, messageId: String? = null, replyTo: String? = null, targets: List<String> = emptyList(), onSuccess: () -> Unit = {}) {
        if (mutable.value.submitting) return
        mutable.update { it.copy(submitting = true) }; val life = lifecycle
        launchOperation {
            try {
                if (!active || mutable.value.revoked || mutable.value.selected.isEmpty() || (operation != "recall" && (text.isBlank() || text.toByteArray(Charsets.UTF_8).size > 8192))) throw TeamMessageFailure("INVALID_INPUT")
                if (operation == "create") {
                    val conversation = mutable.value.cache?.conversations?.find { it.id == mutable.value.selected } ?: throw TeamMessageFailure("CONVERSATION_REQUIRED")
                    if (conversation.kind == "bot_dm" && conversation.botKeys.size != 1) throw TeamMessageFailure("BOT_SELECTION_STALE")
                    requireTeamBotSelections(if (conversation.kind == "bot_dm") conversation.botKeys else targets, mutable.value.bots, conversation.botKeys)
                }
                val intent = JSONObject().put("action", "mutate").put("teamId", teamId).put("generation", generation).put("conversationId", mutable.value.selected).put("messageId", messageId ?: UUID.randomUUID().toString()).put("changeId", UUID.randomUUID().toString()).put("operation", operation).put("text", text)
                replyTo?.let { intent.put("replyTo", it) }
                if (operation == "create") intent.put("targets", JSONArray(targets.distinct().sorted()))
                commit(life) { it.copy(outbox = it.outbox + intent) }; onSuccess(); refresh()
            } finally { if (life == lifecycle) mutable.update { it.copy(submitting = false) } }
        }
    }
    private suspend fun observeRun(conversationId: String, triggerId: String, life: Long, questionId: String? = null) = TeamBotRun.parse(call(teamBotStateQuery(conversationId, triggerId, questionId), life))
    fun answer(message: TeamMessage, answers: Map<String, String>) = launchOperation {
        val life = lifecycle
        val conversation = mutable.value.selected
        val cache = mutable.value.cache ?: throw TeamMessageFailure("STORAGE_FAILED")
        if (cache.threads[conversation]?.messages?.none { it.id == message.id && it.question == message.question } != false) throw TeamMessageFailure("QUESTION_RECIPIENT_REQUIRED")
        if (mutable.value.offline) throw TeamMessageFailure("SERVICE_UNAVAILABLE")
        val run = mutable.value.botRuns[message.triggerId] ?: throw TeamMessageFailure("QUESTION_RECIPIENT_REQUIRED")
        val intent = teamBotAnswerIntent(cache, conversation, message, run, answers, UUID.randomUUID().toString(), System.currentTimeMillis())
        commit(life) { current ->
            if (current.answers.any { it.getString("conversationId") == conversation && it.getString("questionId") == intent.getString("questionId") }) throw TeamMessageFailure("ANSWER_PENDING")
            current.copy(answers = current.answers + intent)
        }
        refresh()
    }
    fun unbind(bot: TeamBotCard) = launchOperation {
        val current = mutable.value.bots.find { it.key == bot.key }
        if (current?.canUnbind(uid) != true || current.revision != bot.revision) throw TeamMessageFailure("BOT_SELECTION_STALE")
        call(JSONObject().put("action", "unbindBot").put("botKey", bot.key).put("expectedRevision", bot.revision), lifecycle)
        mutable.update { it.copy(bots = it.bots.filter { value -> value.key != bot.key }, botRuns = emptyMap()) }; botsDirty = true; refresh()
    }
    fun recover(triggerId: String, text: String) = launchOperation {
        val life = lifecycle; val conversation = mutable.value.selected
        val run = observeRun(conversation, triggerId, life)
        val cache = mutable.value.cache ?: throw TeamMessageFailure("STORAGE_FAILED")
        val intent = teamBotSettlementIntent(cache, conversation, triggerId, run, text, UUID.randomUUID().toString(), System.currentTimeMillis())
        commit(life) { current ->
            if (current.settlements.any { it.getJSONObject("body").getString("conversationId") == conversation && it.getJSONObject("body").getString("triggerId") == triggerId }) throw TeamMessageFailure("MESSAGE_IN_FLIGHT")
            current.copy(settlements = current.settlements + intent)
        }
        refresh()
    }
    fun discardSettlement(id: String) = launchOperation { commit(lifecycle) { cache ->
        if (cache.settlements.any { it.getString("settlementId") == id && !canDiscardTeamBotIntent(it, System.currentTimeMillis()) }) throw TeamMessageFailure("MESSAGE_IN_FLIGHT")
        cache.copy(settlements = cache.settlements.filter { it.getString("settlementId") != id })
    } }
    fun discardAnswer(id: String) = launchOperation { commit(lifecycle) { it.copy(answers = it.answers.filter { value -> value.getString("answerId") != id || !value.has("error") }) } }
    fun discard(id: String) = launchOperation { commit(lifecycle) { if (it.outbox.any { value -> value.getString("changeId") == id && ((value.optBoolean("attempted") && !value.has("error")) || value.optLong("leaseUntil") > System.currentTimeMillis()) }) throw TeamMessageFailure("MESSAGE_IN_FLIGHT"); it.copy(outbox = it.outbox.filter { value -> value.getString("changeId") != id }) } }
    fun discardCreation() = launchOperation { commit(lifecycle) { if ((it.creation?.optBoolean("attempted") == true && !it.creation.has("error")) || (it.creation?.optLong("leaseUntil") ?: 0) > System.currentTimeMillis()) throw TeamMessageFailure("MESSAGE_IN_FLIGHT"); it.copy(creation = null) } }
    fun markRead() = launchOperation {
        val id = mutable.value.selected; val seq = mutable.value.cache?.threads?.get(id)?.messages?.maxOfOrNull { it.seq } ?: 0
        val value = call(JSONObject().put("action", "read").put("conversationId", id).put("lastReadSeq", seq), lifecycle).getLong("lastReadSeq")
        mutable.update { it.copy(cache = it.cache?.copy(conversations = it.cache.conversations.map { c -> if (c.id == id) c.copy(lastReadSeq = maxOf(c.lastReadSeq, value)) else c })) }
    }
    private fun launchOperation(block: suspend () -> Unit) { val life = lifecycle; viewModelScope.launch { try { block() } catch (error: CancellationException) { throw error } catch (error: Exception) { if (active && life == lifecycle) { val code = (error as? TeamMessageFailure)?.code ?: "STORAGE_FAILED"; mutable.update { it.copy(error = code) }; if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate() } } } }
    private fun scheduleDeadline() {
        deadline?.cancel(); deadline = null
        val now = System.currentTimeMillis()
        val expirations = mutable.value.bots.filter { it.online }.map { it.onlineUntil } + mutable.value.botRuns.values.filter { it.state in setOf("claimed", "running") }.map { it.leaseUntil }
        val next = expirations.filter { it > now }.minOrNull() ?: return
        if (active) deadline = viewModelScope.launch { delay(next - now); botsDirty = true; refresh() }
    }
    private fun scheduleRetry() {
        retry?.cancel(); retry = null
        if (!active) return
        val cache = mutable.value.cache ?: return
        val pending = (cache.outbox + cache.answers + cache.settlements + listOfNotNull(cache.creation)).filter { !it.has("error") }
        val earliest = pending.minOfOrNull { it.optLong("leaseUntil") } ?: return
        val wait = maxOf(retryDelay, earliest - System.currentTimeMillis()); retryDelay = minOf(30000L, retryDelay * 2)
        retry = viewModelScope.launch { delay(wait); refresh() }
    }
    private suspend fun call(input: JSONObject, life: Long): JSONObject {
        val owner = uid
        val user = auth.currentUser
        if (!active || life != lifecycle || user?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        val token = requireNotNull(user.getIdToken(false).await().token)
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        val body = JSONObject(input.toString()).put("teamId", teamId).put("generation", generation)
        val result = TeamHttp.request("$apiRoot/v1/teams/messages", token, body)
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        return result
    }
    override fun onCleared() { stop() }
}
