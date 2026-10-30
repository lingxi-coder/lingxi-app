package com.lingxi.code.teams

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges
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
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

internal data class TeamMessagingState(val cache: TeamMessageCache? = null, val selected: String = "", val offline: Boolean = true, val error: String = "", val revoked: Boolean = false, val submitting: Boolean = false, val bots: List<TeamBotCard> = emptyList(), val botRuns: Map<String, TeamBotRun> = emptyMap())
internal class TeamMessagingModel @JvmOverloads constructor(application: Application, private val apiRoot: String = "https://lingxios.net") : AndroidViewModel(application) {
    private val auth = FirebaseAuth.getInstance()
    private val db = FirebaseFirestore.getInstance()
    private val storage = TeamMessageStorage(File(application.noBackupFilesDir, "team-messages"))
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
    private var poll: Job? = null
    private var listener: ListenerRegistration? = null
    private var readListener: ListenerRegistration? = null
    private val terminalRuns = mutableMapOf<String, TeamBotRun>()
    private val authListener = FirebaseAuth.AuthStateListener { if (active && it.currentUser?.uid != uid) invalidate() }
    fun start(uid: String, team: TeamSummary) {
        stop(); active = true; lifecycle++; this.uid = uid; teamId = team.id; generation = team.generation
        mutable.value = TeamMessagingState()
        auth.addAuthStateListener(authListener)
        val life = lifecycle
        listener = db.collection("teams").document(team.id).collection("conversations").whereArrayContains("memberUids", uid).addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            if (active && lifecycle == life && (snapshot?.metadata?.isFromCache == false || error != null)) refresh()
        }
        poll = viewModelScope.launch {
            try { commit(life) { it } } catch (error: Exception) { if (active && lifecycle == life) mutable.update { it.copy(error = "STORAGE_FAILED") }; return@launch }
            while (active && lifecycle == life) { refresh(); delay(15000) }
        }
    }
    fun stop() {
        active = false; lifecycle++; job?.cancel(); poll?.cancel(); job = null; poll = null
        listener?.remove(); readListener?.remove(); listener = null; readListener = null
        auth.removeAuthStateListener(authListener); running = false; again = false; terminalRuns.clear(); botHistoryLimit = 100
    }
    private fun invalidate() {
        val owner = uid; val team = teamId; val epoch = generation
        mutable.value = TeamMessagingState(error = "MEMBERSHIP_REQUIRED", revoked = true)
        viewModelScope.launch { try { withContext(Dispatchers.IO) { storage.update(owner, team, epoch) { TeamMessageCache(owner, team, epoch) } } } catch (_: Exception) { } }
        stop()
    }
    private suspend fun commit(life: Long, reduce: (TeamMessageCache) -> TeamMessageCache) {
        if (!active || life != lifecycle) throw CancellationException()
        val owner = uid; val team = teamId; val epoch = generation
        val cache = withContext(Dispatchers.IO) { storage.update(owner, team, epoch, reduce) }
        if (!active || life != lifecycle) throw CancellationException()
        mutable.update { it.copy(cache = cache) }
    }
    fun select(id: String) {
        botHistoryLimit = 100
        mutable.update { it.copy(selected = id, botRuns = emptyMap()) }; readListener?.remove()
        val life = lifecycle
        readListener = db.collection("teams").document(teamId).collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener { snapshot, _ -> if (active && lifecycle == life && snapshot?.metadata?.isFromCache == false) refresh() }
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
                commit(life) { it }
                val bots = call(JSONObject().put("action", "bots"), life).getJSONArray("bots").objects().map(TeamBotCard::parse)
                mutable.update { it.copy(bots = bots) }
                val creationLease = System.currentTimeMillis() + 30000
                commit(life) { old -> val creation = old.creation; if (creation != null && !creation.has("error") && creation.optLong("leaseUntil") <= System.currentTimeMillis()) old.copy(creation = JSONObject(creation.toString()).put("leaseUntil", creationLease).put("attempted", true)) else old }
                mutable.value.cache?.creation?.takeIf { it.optLong("leaseUntil") == creationLease }?.let { creation ->
                    try {
                        val body = JSONObject(creation.toString()).also { it.remove("error"); it.remove("leaseUntil"); it.remove("attempted") }; call(body, life)
                        commit(life) { it.copy(creation = null) }
                        if (mutable.value.selected.isEmpty()) mutable.update { it.copy(selected = creation.getString("conversationId")) }
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) {
                        val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                        if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED")) throw error
                        val permanent = code in setOf("CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "INVALID_INPUT")
                        commit(life) { it.copy(creation = it.creation?.let { value -> JSONObject(value.toString()).also { item -> item.remove("leaseUntil"); if (permanent) item.put("error", code) } }) }
                        if (!permanent) throw error
                    }
                }
                val conversations = mutableListOf<TeamMessageConversation>()
                var after: String? = null
                do {
                    val body = JSONObject().put("action", "list"); after?.let { body.put("after", it) }
                    val page = call(body, life)
                    conversations += page.getJSONArray("conversations").objects().map(TeamMessageConversation::parse); after = page.stringOrNull("next")
                } while (after != null)
                commit(life) { old -> old.copy(conversations = conversations.map { value -> value.merging(old.conversations.find { it.id == value.id }) }) }; commit(life) { it }
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
                val id = mutable.value.selected
                if (id.isNotEmpty() && conversations.any { it.id == id }) {
                    do {
                        val page = call(JSONObject().put("action", "sync").put("conversationId", id).put("after", mutable.value.cache?.threads?.get(id)?.cursor ?: 0), life)
                        commit(life) { it.apply(page) }
                    } while (page.getBoolean("hasMore"))
                } else if (id.isNotEmpty()) mutable.update { it.copy(selected = "") }
                if (id.isNotEmpty() && conversations.any { it.id == id }) {
                    val runs = mutableMapOf<String, TeamBotRun>()
                    val conversation = conversations.find { it.id == id }
                    val triggers = visibleTeamBotTriggers(mutable.value.cache?.threads?.get(id)?.messages ?: emptyList(), conversation, botHistoryLimit)
                    for (trigger in triggers) {
                        val key = "$id/$trigger"
                        val run = terminalRuns[key] ?: try { observeRun(id, trigger, life) } catch (error: TeamMessageFailure) {
                            // SDK mentions are display metadata and may have no authenticated trigger.
                            if (error.code == "TRIGGER_REQUIRED") continue else throw error
                        }
                        runs[trigger] = run
                        if (run.state in setOf("completed", "failed", "missed", "cancelled", "recovered", "revoked")) terminalRuns[key] = run
                    }
                    if (mutable.value.selected == id) mutable.update { it.copy(botRuns = runs) }
                }
                mutable.update { it.copy(offline = false, error = "") }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                if (active && lifecycle == life) {
                    val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                    mutable.update { it.copy(offline = true, error = code, bots = emptyList(), botRuns = emptyMap()) }
                    if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate()
                }
            } finally { if (lifecycle == life) { running = false; if (again) { again = false; refresh() } } }
        }
    }
    fun create(kind: String, title: String, parties: List<String>, id: String, botKeys: List<String> = emptyList(), onSuccess: () -> Unit) = launchOperation {
        requireTeamBotSelections(botKeys, mutable.value.bots)
        if ((kind == "bot_dm" && botKeys.size != 1) || (kind == "dm" && botKeys.isNotEmpty())) throw TeamMessageFailure("BOT_SELECTION_STALE")
        val intent = JSONObject().put("action", "create").put("teamId", teamId).put("generation", generation).put("conversationId", id).put("kind", kind).put("title", title).put("participantUids", JSONArray(if (kind == "bot_dm") listOf(uid) else (listOf(uid) + parties).distinct())).put("botKeys", JSONArray(botKeys))
        commit(lifecycle) { if (it.creation != null) throw TeamMessageFailure("CREATION_PENDING"); it.copy(creation = intent) }
        mutable.update { it.copy(selected = id) }; refresh(); onSuccess()
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
        mutable.update { it.copy(bots = it.bots.filter { value -> value.key != bot.key }, botRuns = emptyMap()) }; refresh()
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
    fun discardCreation() = launchOperation { commit(lifecycle) { if ((it.creation?.optBoolean("attempted") == true && it.creation?.has("error") == false) || (it.creation?.optLong("leaseUntil") ?: 0) > System.currentTimeMillis()) throw TeamMessageFailure("MESSAGE_IN_FLIGHT"); it.copy(creation = null) } }
    fun markRead() = launchOperation {
        val id = mutable.value.selected; val seq = mutable.value.cache?.threads?.get(id)?.messages?.maxOfOrNull { it.seq } ?: 0
        val value = call(JSONObject().put("action", "read").put("conversationId", id).put("lastReadSeq", seq), lifecycle).getLong("lastReadSeq")
        commit(lifecycle) { it.copy(conversations = it.conversations.map { c -> if (c.id == id) c.copy(lastReadSeq = maxOf(c.lastReadSeq, value)) else c }) }
    }
    private fun launchOperation(block: suspend () -> Unit) { val life = lifecycle; viewModelScope.launch { try { block() } catch (error: CancellationException) { throw error } catch (error: Exception) { if (active && life == lifecycle) { val code = (error as? TeamMessageFailure)?.code ?: "STORAGE_FAILED"; mutable.update { it.copy(error = code) }; if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate() } } } }
    private suspend fun call(input: JSONObject, life: Long): JSONObject {
        val owner = uid
        val user = auth.currentUser
        if (!active || life != lifecycle || user?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        val token = requireNotNull(user.getIdToken(false).await().token)
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        val body = JSONObject(input.toString()).put("teamId", teamId).put("generation", generation)
        val result = withContext(Dispatchers.IO) {
            val connection = URL("$apiRoot/v1/teams/messages").openConnection() as HttpURLConnection
            try {
                connection.connectTimeout = 20000; connection.readTimeout = 20000; connection.instanceFollowRedirects = false; connection.useCaches = false
                connection.requestMethod = "POST"; connection.doOutput = true; connection.setRequestProperty("Authorization", "Bearer $token"); connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
                val success = connection.responseCode in 200..299
                val stream = if (success) connection.inputStream else connection.errorStream
                val value = JSONObject(stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: "{}")
                if (!success) throw TeamMessageFailure(value.optJSONObject("error")?.optString("code") ?: "SERVICE_UNAVAILABLE")
                value
            } finally { connection.disconnect() }
        }
        if (!active || life != lifecycle || auth.currentUser?.uid != owner) throw TeamMessageFailure("INVALID_SESSION")
        return result
    }
    override fun onCleared() { stop() }
}
