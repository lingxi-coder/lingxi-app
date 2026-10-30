package com.lingxi.code.teams

import androidx.lifecycle.ViewModel
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
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

data class TeamSummary(val id: String, val name: String, val memberCount: Int, val role: String, val generation: Long)
data class TeamMember(val uid: String, val displayName: String, val role: String, val generation: Long)
data class TeamInvite(val id: String, val role: String, val expiresAt: String, val uses: Int, val maxUses: Int, val revoked: Boolean)
data class TeamDetail(val team: TeamSummary, val members: List<TeamMember>, val invites: List<TeamInvite>)
data class TeamsState(val uid: String? = null, val teams: List<TeamSummary> = emptyList(), val detail: TeamDetail? = null,
    val selectedId: String = "", val busy: Boolean = false, val loading: Boolean = false, val error: String? = null, val invitationLink: String = "")


/** SDK listeners invalidate; only authenticated HTTP responses populate the UI. */
class TeamsViewModel internal constructor(private val apiRoot: String = "https://lingxios.net") : ViewModel() {
    private val auth = FirebaseAuth.getInstance()
    private val db = FirebaseFirestore.getInstance()
    private val mutableState = MutableStateFlow(TeamsState())
    val state = mutableState.asStateFlow()
    private val listeners = mutableMapOf<String, ListenerRegistration>()
    private var refreshJob: Job? = null
    private var commandJob: Job? = null
    private var epoch = 0L
    private var revision = 0L
    private var authority = 0L
    private var started = false
    private val authListener = FirebaseAuth.AuthStateListener { accountChanged() }

    fun start() {
        if (started) return
        started = true; auth.addAuthStateListener(authListener); accountChanged()
    }
    fun stop() {
        if (!started) return
        started = false; auth.removeAuthStateListener(authListener); reset(); mutableState.value = TeamsState()
    }
    private fun reset() {
        epoch++; revision++; refreshJob?.cancel(); commandJob?.cancel()
        listeners.values.forEach { it.remove() }; listeners.clear()
        mutableState.value = TeamsState()
    }
    private fun accountChanged() {
        if (!started) return
        val uid = auth.currentUser?.uid
        if (uid != mutableState.value.uid) { reset(); mutableState.value = TeamsState(uid = uid) }
        if (uid == null || listeners.containsKey("index")) return
        val expected = epoch
        listeners["index"] = db.collection("users").document(uid).collection("teams").addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
            changed(expected, snapshot?.metadata?.isFromCache != false || error != null)
        }
        refresh()
    }
    private fun changed(expected: Long, invalidated: Boolean) {
        if (expected != epoch || !started) return
        if (invalidated) { revision++; authority++; mutableState.update { it.copy(teams = emptyList(), detail = null, invitationLink = "") } }
        refresh()
    }
    fun select(teamId: String) {
        mutableState.update { it.copy(selectedId = teamId, detail = null, invitationLink = "") }
        if (!listeners.containsKey(teamId)) {
            val expected = epoch
            listeners[teamId] = db.collection("teams").document(teamId).collection("members").addSnapshotListener(MetadataChanges.INCLUDE) { snapshot, error ->
                if (error != null && expected == epoch) listeners.remove(teamId)?.remove()
                changed(expected, snapshot?.metadata?.isFromCache != false || error != null)
            }
        }
        refresh()
    }
    fun refresh() {
        refreshJob?.cancel()
        refreshJob = viewModelScope.launch { delay(80); refreshNow() }
    }
    private suspend fun refreshNow() {
        if (mutableState.value.uid == null) return
        revision++; val request = revision; val expected = epoch; val selected = mutableState.value.selectedId
        mutableState.update { it.copy(loading = true) }
        try {
            val list = call().getJSONArray("teams")
            if (request != revision || expected != epoch) return
            mutableState.update { it.copy(teams = (0 until list.length()).map { index -> summary(list.getJSONObject(index)) }, error = null) }
            if (selected.isNotEmpty()) {
                try {
                    val value = detail(call("/$selected"))
                    if (request == revision && expected == epoch) mutableState.update { it.copy(detail = value) }
                } catch (error: CancellationException) { throw error }
                catch (error: Exception) {
                    if (request == revision && expected == epoch) mutableState.update { it.copy(selectedId = "", detail = null, invitationLink = "", error = errorCode(error)) }
                }
            }
        } catch (error: CancellationException) { throw error }
        catch (error: Exception) {
            if (request == revision && expected == epoch) mutableState.update { it.copy(teams = emptyList(), detail = null, error = errorCode(error)) }
        } finally { if (request == revision && expected == epoch) mutableState.update { it.copy(loading = false) } }
    }
    fun execute(input: Map<String, Any>, onSuccess: () -> Unit = {}) {
        if (mutableState.value.busy || mutableState.value.uid == null) return
        val expected = epoch
        val authorized = authority
        mutableState.update { it.copy(busy = true, error = null) }
        commandJob = viewModelScope.launch {
            try {
                val result = call(body = JSONObject(input))
                if (expected != epoch) return@launch
                if (input["action"] in setOf("leave", "delete")) mutableState.update { it.copy(selectedId = "", detail = null) }
                if (authorized != authority) { refresh(); return@launch }
                result.optString("code").takeIf { it.isNotBlank() }?.let { code -> mutableState.update { it.copy(invitationLink = TeamInvitation.link(code)) } }
                if (input["action"] in setOf("create", "join")) result.optString("teamId").takeIf { it.isNotBlank() }?.let(::select)
                onSuccess(); refreshNow()
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { if (expected == epoch) mutableState.update { it.copy(error = errorCode(error)) } }
            finally { if (expected == epoch) mutableState.update { it.copy(busy = false) } }
        }
    }
    private suspend fun call(path: String = "", body: JSONObject? = null): JSONObject {
        val uid = requireNotNull(mutableState.value.uid)
        val user = requireNotNull(auth.currentUser)
        check(user.uid == uid)
        val token = requireNotNull(user.getIdToken(false).await().token)
        check(auth.currentUser?.uid == uid)
        val result = withContext(Dispatchers.IO) {
            val connection = URL("$apiRoot/v1/teams$path").openConnection() as HttpURLConnection
            try {
                connection.connectTimeout = 20000; connection.readTimeout = 20000; connection.instanceFollowRedirects = false; connection.useCaches = false
                connection.setRequestProperty("Authorization", "Bearer $token")
                if (body != null) {
                    connection.requestMethod = "POST"; connection.doOutput = true; connection.setRequestProperty("Content-Type", "application/json")
                    connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
                }
                val success = connection.responseCode in 200..299
                val stream = if (success) connection.inputStream else connection.errorStream
                val json = JSONObject(stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: "{}")
                if (!success) throw ApiFailure(json.optJSONObject("error")?.optString("code") ?: "SERVICE_UNAVAILABLE")
                json
            } finally { connection.disconnect() }
        }
        check(auth.currentUser?.uid == uid)
        return result
    }
    private class ApiFailure(val code: String) : Exception()
    private fun errorCode(error: Exception) = (error as? ApiFailure)?.code ?: "SERVICE_UNAVAILABLE"
    override fun onCleared() { stop() }
    companion object {
        internal fun summary(value: JSONObject) = TeamSummary(value.getString("id"), value.getString("name"), value.getInt("memberCount"), value.getString("role"), value.getLong("generation"))
        internal fun detail(value: JSONObject): TeamDetail {
            val members = value.getJSONArray("members")
            val invites = value.getJSONArray("invites")
            return TeamDetail(summary(value.getJSONObject("team")), (0 until members.length()).map { i -> val v = members.getJSONObject(i); TeamMember(v.getString("uid"), v.getString("displayName"), v.getString("role"), v.getLong("generation")) },
                (0 until invites.length()).map { i -> val v = invites.getJSONObject(i); TeamInvite(v.getString("id"), v.getString("role"), v.getString("expiresAt"), v.getInt("uses"), v.getInt("maxUses"), v.getBoolean("revoked")) })
        }
    }
}
