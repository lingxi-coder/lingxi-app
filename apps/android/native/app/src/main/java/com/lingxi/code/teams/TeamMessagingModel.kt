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

internal data class TeamMessagingState(val cache: TeamMessageCache? = null, val selected: String = "", val offline: Boolean = true, val error: String = "", val revoked: Boolean = false, val submitting: Boolean = false)
internal class TeamMessagingModel(application: Application) : AndroidViewModel(application) {
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
    private var job: Job? = null
    private var poll: Job? = null
    private var listener: ListenerRegistration? = null
    private var readListener: ListenerRegistration? = null
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
        auth.removeAuthStateListener(authListener); running = false; again = false
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
        mutable.update { it.copy(selected = id) }; readListener?.remove()
        val life = lifecycle
        readListener = db.collection("teams").document(teamId).collection("conversations").document(id).collection("reads").document(uid).addSnapshotListener { snapshot, _ -> if (active && lifecycle == life && snapshot?.metadata?.isFromCache == false) refresh() }
        refresh()
    }
    fun refresh() {
        if (!active || mutable.value.revoked) return
        if (running) { again = true; return }
        running = true; val life = lifecycle
        job = viewModelScope.launch {
            try {
                commit(life) { it }
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
                        val permanent = code in setOf("CONVERSATION_REJECTED", "PARTICIPANT_REQUIRED", "INVALID_INPUT")
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
                        val permanent = code in setOf("MESSAGE_ID_CONFLICT", "MESSAGE_RECALLED", "MESSAGE_AUTHOR_REQUIRED", "REPLY_REQUIRED", "INVALID_INPUT")
                        commit(life) { it.copy(outbox = it.outbox.map { value -> if (value.getString("changeId") == id) JSONObject(value.toString()).also { it.remove("leaseUntil"); if (permanent) it.put("error", code) } else value }) }
                        if (!permanent) throw error
                    }
                }
                val id = mutable.value.selected
                if (id.isNotEmpty() && conversations.any { it.id == id }) {
                    do {
                        val page = call(JSONObject().put("action", "sync").put("conversationId", id).put("after", mutable.value.cache?.threads?.get(id)?.cursor ?: 0), life)
                        commit(life) { it.apply(page) }
                    } while (page.getBoolean("hasMore"))
                } else if (id.isNotEmpty()) mutable.update { it.copy(selected = "") }
                mutable.update { it.copy(offline = false, error = "") }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                if (active && lifecycle == life) {
                    val code = (error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
                    mutable.update { it.copy(offline = true, error = code) }
                    if (code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED")) invalidate()
                }
            } finally { if (lifecycle == life) { running = false; if (again) { again = false; refresh() } } }
        }
    }
    fun create(kind: String, title: String, parties: List<String>, id: String, onSuccess: () -> Unit) = launchOperation {
        val intent = JSONObject().put("action", "create").put("teamId", teamId).put("generation", generation).put("conversationId", id).put("kind", kind).put("title", title).put("participantUids", JSONArray(listOf(uid) + parties))
        commit(lifecycle) { if (it.creation != null) throw TeamMessageFailure("CREATION_PENDING"); it.copy(creation = intent) }
        mutable.update { it.copy(selected = id) }; refresh(); onSuccess()
    }
    fun queue(operation: String, text: String, messageId: String? = null, replyTo: String? = null, onSuccess: () -> Unit = {}) {
        if (mutable.value.submitting) return
        mutable.update { it.copy(submitting = true) }; val life = lifecycle
        launchOperation {
            try {
                if (!active || mutable.value.revoked || mutable.value.selected.isEmpty() || (operation != "recall" && (text.isBlank() || text.toByteArray(Charsets.UTF_8).size > 8192))) throw TeamMessageFailure("INVALID_INPUT")
                val intent = JSONObject().put("action", "mutate").put("teamId", teamId).put("generation", generation).put("conversationId", mutable.value.selected).put("messageId", messageId ?: UUID.randomUUID().toString()).put("changeId", UUID.randomUUID().toString()).put("operation", operation).put("text", text)
                replyTo?.let { intent.put("replyTo", it) }
                commit(life) { it.copy(outbox = it.outbox + intent) }; onSuccess(); refresh()
            } finally { if (life == lifecycle) mutable.update { it.copy(submitting = false) } }
        }
    }
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
            val connection = URL("https://lingxios.net/v1/teams/messages").openConnection() as HttpURLConnection
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
