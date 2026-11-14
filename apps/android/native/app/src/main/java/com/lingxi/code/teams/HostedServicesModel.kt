package com.lingxi.code.teams

import com.google.firebase.auth.FirebaseAuth
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import org.json.JSONObject
import java.util.UUID

/** What the Bot services screen shows. */
internal data class HostedServicesState(
    val uid: String = "",
    val entered: List<HostedServiceView> = emptyList(),
    val listed: Boolean = false,
    /** A service looked up and not entered: its terms are shown, nothing is sent until the person agrees. */
    val candidate: HostedServiceView? = null,
    /** The service whose sessions are on screen. */
    val open: HostedServiceView? = null,
    val sessions: List<HostedServiceSession> = emptyList(),
    val session: String? = null,
    val messages: List<TeamMessage> = emptyList(),
    val tasks: List<TeamTask> = emptyList(),
    /** The last refusal, as a code ([HostedService.errorRes] gives its words). */
    val error: String? = null,
    val busy: Boolean = false,
) {
    /** A request of the open session is under way: the session is read quickly. */
    val live get() = tasks.any { !it.terminal }
    /** The Bot's question to the viewer that the next message answers. */
    val answering get() = open?.generation?.let { HostedServiceCommands.answerable(tasks, session, uid, it) } != null
    val approval get() = open?.generation?.let { HostedServiceCommands.approval(tasks, uid, it) }
}

/**
 * Bot services on a phone (Desktop and Web: `packages/teams/HostedServices.tsx`, iOS: `HostedServicesModel.swift`): the
 * services the person entered, a service looked up by its link, and the open session with its Bot. A phone hosts no Bots
 * and offers no service.
 *
 * Nothing of a service can be watched: the open session is read through the API on a timer, every [answeringMs] while the
 * Bot is answering and every [idleMs] otherwise, and not at all while the screen is away ([pause]). [transport] sends one
 * command (`path`, body) as the signed-in account; [account] says who that is. Another account closes everything.
 */
internal class HostedServicesModel(
    private val scope: CoroutineScope,
    private val account: () -> String?,
    private val transport: suspend (path: String, body: JSONObject) -> JSONObject,
    private val answeringMs: Long = 2000,
    private val idleMs: Long = 10000,
) {
    private val mutableState = MutableStateFlow(HostedServicesState())
    val state: StateFlow<HostedServicesState> = mutableState.asStateFlow()
    private var log = HostedServiceLog()
    private var poll: Job? = null
    private var lifecycle = 0L
    private val current get() = mutableState.value

    /** The screen is on: the list is read, and a link the person followed is looked up (never entered by itself). */
    fun start(link: String? = null) {
        val uid = account().orEmpty()
        if (uid != current.uid) reset(uid)
        scope.launch {
            attempt { refresh() }
            if (link != null) lookup(link)
        }
        if (current.open != null) watch()
    }
    /** The screen is away: nothing is read until it is back. */
    fun pause() { poll?.cancel(); poll = null }
    /** The account changed: what belonged to the old one is gone. */
    fun accountChanged() { val uid = account().orEmpty(); if (uid != current.uid) reset(uid) }
    private fun reset(uid: String) { pause(); lifecycle++; log = HostedServiceLog(); mutableState.value = HostedServicesState(uid = uid) }
    fun clearError() = mutableState.update { it.copy(error = null) }
    fun dismissCandidate() = mutableState.update { it.copy(candidate = null) }

    // The service

    suspend fun refresh() {
        val entered = call("/v1/services", HostedServiceCommands.service("listServices")).getJSONArray("entered")
        mutableState.update { s -> s.copy(entered = (0 until entered.length()).map { HostedServiceView.parse(entered.getJSONObject(it)) }, listed = true) }
    }
    /** Looks a service up by a link or an id. Looking enters nothing. */
    suspend fun lookup(input: String) {
        val serviceId = HostedService.serviceIdFrom(input) ?: run { mutableState.update { it.copy(error = "INVALID_SERVICE_LINK") }; return }
        attempt {
            val view = HostedServiceView.lookup(call("/v1/services", HostedServiceCommands.service("getService", serviceId)))
            when {
                view == null -> mutableState.update { it.copy(candidate = null, error = "OWN_SERVICE") }
                view.entered -> { mutableState.update { it.copy(candidate = null) }; show(view) }
                else -> mutableState.update { it.copy(candidate = view) }
            }
        }
    }
    /** The person agreed to the terms they were shown. */
    suspend fun enter() {
        val candidate = current.candidate ?: return
        attempt {
            val entry = call("/v1/services", HostedServiceCommands.service("enterService", candidate.serviceId, JSONObject().put("acknowledged", true)))
            val view = HostedServiceView.parse(entry.getJSONObject("service")).copy(entered = true, generation = entry.getLong("generation"))
            mutableState.update { it.copy(candidate = null) }
            show(view)
            runCatching { refresh() }
        }
    }
    fun show(view: HostedServiceView) {
        closeService()
        mutableState.update { it.copy(open = view) }
        scope.launch { attempt { loadSessions(selectLast = true) } }
        watch()
    }
    fun closeService() {
        pause(); log = HostedServiceLog()
        mutableState.update { it.copy(open = null, sessions = emptyList(), session = null, messages = emptyList(), tasks = emptyList()) }
    }
    suspend fun leave(): Boolean {
        val open = current.open ?: return false
        return attempt {
            call("/v1/services", HostedServiceCommands.service("leaveService", open.serviceId))
            closeService(); runCatching { refresh() }
        }
    }
    /** [reportId] is made once per report, so sending it again after a lost answer is the same report. */
    suspend fun report(reason: String, text: String, withSession: Boolean, reportId: String): Boolean {
        val open = current.open ?: return false
        return attempt { call("/v1/services", HostedServiceCommands.report(open.serviceId, reportId, reason, text, if (withSession) current.session else null)) }
    }

    // Sessions

    private suspend fun loadSessions(selectLast: Boolean) {
        val open = current.open ?: return
        val generation = open.generation ?: return
        val found = HostedServiceSession.list(call("/v1/teams/messages", HostedServiceCommands.sessions(open.serviceId, generation)))
        if (current.open?.serviceId != open.serviceId) return
        mutableState.update { it.copy(sessions = found) }
        if (selectLast && current.session == null) found.lastOrNull()?.let { select(it.id) }
    }
    fun select(id: String) {
        if (id == current.session) return
        log = HostedServiceLog()
        mutableState.update { it.copy(session = id, messages = emptyList(), tasks = emptyList()) }
        watch()
    }
    suspend fun newSession(title: String): Boolean = attempt { makeSession(title) }
    private suspend fun makeSession(title: String): String {
        val open = current.open ?: throw TeamMessageFailure("SERVICE_ENTRY_REQUIRED")
        val generation = open.generation ?: throw TeamMessageFailure("SERVICE_ENTRY_REQUIRED")
        val id = UUID.randomUUID().toString()
        call("/v1/teams/messages", HostedServiceCommands.newSession(open.serviceId, generation, open.sharedBotId, id, title, current.uid))
        // The session exists from here on, whatever becomes of the request: a refused request is sent again into it.
        loadSessions(selectLast = false); select(id)
        return id
    }
    /** Sends what was typed: the answer to the Bot's question, else a new request. Returns whether it was taken; the text stays in the box when it was not. */
    suspend fun send(text: String): Boolean {
        val body = text.trim()
        val open = current.open ?: return false
        val generation = open.generation ?: return false
        if (body.isEmpty()) return false
        return attempt {
            val id = current.session ?: makeSession(HostedService.sessionTitle(body))
            call("/v1/teams/messages", HostedServiceCommands.send(open.serviceId, generation, open.sharedBotId, id, body, current.tasks, current.uid))
            read()
        }
    }
    suspend fun decide(approve: Boolean) {
        val open = current.open ?: return
        val generation = open.generation ?: return
        val session = current.session ?: return
        val wait = current.approval?.wait ?: return
        attempt {
            call("/v1/teams/messages", HostedServiceCommands.task(open.serviceId, generation, "decideWait", JSONObject().put("conversationId", session).put("waitId", wait.waitId)
                .put("messageId", UUID.randomUUID().toString()).put("changeId", UUID.randomUUID().toString()).put("decision", if (approve) "approve" else "deny")))
            read()
        }
    }
    suspend fun cancel(taskId: String) {
        val open = current.open ?: return
        val generation = open.generation ?: return
        attempt {
            try { call("/v1/teams/messages", HostedServiceCommands.task(open.serviceId, generation, "cancelTask", JSONObject().put("taskId", taskId))) }
            catch (failure: TeamMessageFailure) { if (failure.code != "TASK_FINISHED") throw failure }
            read()
        }
    }

    /** Reads the open session on a timer while the screen is on. */
    private fun watch() {
        pause()
        if (current.open == null) return
        val life = lifecycle
        poll = scope.launch {
            while (life == lifecycle) {
                try { read() } catch (failure: Exception) { fail(failure) }
                delay(if (current.live) answeringMs else idleMs)
            }
        }
    }
    private suspend fun read() {
        val open = current.open ?: return
        val generation = open.generation ?: return
        val session = current.session
        try {
            if (session != null) {
                var next = log
                while (true) {
                    val (read, more) = next.apply(call("/v1/teams/messages", HostedServiceCommands.sync(open.serviceId, generation, session, next.after)), current.uid, open.sharedBotId)
                    next = read
                    if (!more) break
                }
                val listed = call("/v1/teams/messages", HostedServiceCommands.task(open.serviceId, generation, "listTasks", JSONObject().put("conversationId", session))).getJSONArray("tasks")
                val tasks = (0 until listed.length()).map { TeamTask.parse(listed.getJSONObject(it)) }.sortedBy { it.createdAt }
                if (current.open?.serviceId != open.serviceId || current.session != session) return
                log = next
                mutableState.update { it.copy(messages = next.messages, tasks = tasks) }
            }
            val now = HostedServiceView.lookup(call("/v1/services", HostedServiceCommands.service("getService", open.serviceId)))
            if (now != null && current.open?.serviceId == open.serviceId) mutableState.update { it.copy(open = now.copy(generation = now.generation ?: generation)) }
        } catch (failure: TeamMessageFailure) {
            // A broken page is read again from the start next time.
            if (failure.code == "CHANGE_GAP" || failure.code == "INVALID_DATA") log = HostedServiceLog()
            throw failure
        }
    }

    // The API

    private suspend fun attempt(step: suspend () -> Unit): Boolean {
        mutableState.update { it.copy(busy = true, error = null) }
        return try { step(); true } catch (failure: Exception) { if (failure is kotlinx.coroutines.CancellationException) throw failure; fail(failure); false }
        finally { mutableState.update { it.copy(busy = false) } }
    }
    private fun fail(failure: Exception) {
        if (failure is kotlinx.coroutines.CancellationException) throw failure
        val code = (failure as? TeamMessageFailure)?.code ?: if (failure is org.json.JSONException) "INVALID_DATA" else "SERVICE_UNAVAILABLE"
        mutableState.update { it.copy(error = code) }
        // Out of the service: its sessions are no longer shown.
        if (current.open != null && HostedService.isAccessLoss(code)) { closeService(); scope.launch { runCatching { refresh() } } }
    }
    private suspend fun call(path: String, body: JSONObject): JSONObject {
        val life = lifecycle
        val uid = current.uid
        if (uid.isEmpty() || account() != uid) throw TeamMessageFailure("SIGN_IN_REQUIRED")
        val result = transport(path, body)
        // An answer for an account that is no longer signed in here is dropped.
        if (life != lifecycle || account() != uid) throw TeamMessageFailure("SIGN_IN_REQUIRED")
        return result
    }

    companion object {
        /** The model over the signed-in Firebase account and the Teams HTTP client. */
        fun create(scope: CoroutineScope, auth: FirebaseAuth = FirebaseAuth.getInstance(), apiRoot: String = TEAMS_API_ROOT) = HostedServicesModel(scope, { auth.currentUser?.uid }, { path, body ->
            val user = auth.currentUser ?: throw TeamMessageFailure("SIGN_IN_REQUIRED")
            val token = requireNotNull(user.getIdToken(false).await().token)
            TeamHttp.request("$apiRoot$path", token, body)
        })
    }
}
