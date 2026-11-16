package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject
import java.net.URI
import java.util.UUID

// A Bot offered outside its publisher's Teams (a hosted Bot service), as a phone meets it: the person looks a service up
// by its link, agrees to its terms, and talks to its Bot in sessions of their own. A service is not a Team: nothing of it
// can be watched, so everything here is asked through the API (`/v1/services` for the service, the Team message route
// with `serviceId` for its sessions and requests). A phone hosts no Bots and offers no service.
//
// The rules are the ones of `packages/teams/hosted-service.ts` and iOS `HostedService.swift`.

internal object HostedService {
    private val ID = Regex("^s_[A-Za-z0-9_-]{6,62}$")
    /** The id space of a service. No Team has such an id. */
    fun isServiceId(value: String?) = value != null && ID.matches(value)
    /** The service a link or a pasted id names: `…/services/<id>` or the id itself. */
    fun serviceIdFrom(input: String): String? {
        val value = input.trim()
        if (isServiceId(value)) return value
        val uri = try { URI(value) } catch (_: Exception) { return null }
        if (uri.scheme.isNullOrEmpty() || uri.host.isNullOrEmpty()) return null
        val parts = (uri.rawPath ?: return null).removeSuffix("/").split("/")
        return if (parts.size == 3 && parts[0].isEmpty() && parts[1] == "services" && isServiceId(parts[2])) parts[2] else null
    }
    @StringRes fun availabilityRes(availability: String): Int = when (availability) {
        "available" -> R.string.services_state_available
        "disabled" -> R.string.services_state_disabled
        "not_ready" -> R.string.services_state_not_ready
        "host_offline" -> R.string.services_state_host_offline
        "service_quota_exhausted" -> R.string.services_state_service_quota
        "user_quota_exhausted" -> R.string.services_state_user_quota
        else -> R.string.services_state_withdrawn
    }
    /** A refusal in words. Codes a service has no word of its own for are said as Teams says them. */
    @StringRes fun errorRes(code: String): Int = when (code) {
        "NOT_FOUND" -> R.string.services_error_not_found
        "SERVICE_ENTRY_REQUIRED" -> R.string.services_error_entry_required
        "SERVICE_ACCESS_REQUIRED" -> R.string.services_error_access_required
        "STALE_MEMBERSHIP" -> R.string.services_error_stale
        "SERVICE_WITHDRAWN" -> R.string.services_error_withdrawn
        "SERVICE_DISABLED" -> R.string.services_error_disabled
        "SERVICE_USER_BUSY" -> R.string.services_error_user_busy
        "SERVICE_BUSY" -> R.string.services_error_busy
        "REQUEST_QUOTA_EXCEEDED" -> R.string.services_error_quota
        "HOST_UNAVAILABLE" -> R.string.services_error_host_unavailable
        "SERVICE_SESSION_LIMIT" -> R.string.services_error_session_limit
        "REPORT_LIMIT" -> R.string.services_error_report_limit
        "SERVICE_UNAVAILABLE" -> R.string.services_error_unavailable
        "INVALID_SERVICE_LINK" -> R.string.services_lookup_invalid
        "OWN_SERVICE" -> R.string.services_own
        else -> teamsErrorRes(code)
    }
    /** Codes after which the person is no longer in the service: its sessions are closed and the list is read again. */
    fun isAccessLoss(code: String) = code in setOf("NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN")
    /** The line under the messages for a request that is not answered; null for an answered one. */
    @StringRes fun taskRes(task: TeamTask): Int? = when (task.state) {
        "queued" -> R.string.services_task_queued
        "running" -> R.string.services_task_running
        "waiting" -> if (task.wait?.kind == "tool_approval") R.string.services_task_approval else R.string.services_task_question
        "failed" -> R.string.services_task_failed
        "expired" -> R.string.services_task_expired
        "interrupted" -> R.string.services_task_interrupted
        "revoked" -> R.string.services_task_revoked
        "cancelled" -> R.string.services_task_cancelled
        "authorization_required" -> R.string.services_task_authorization_required
        "needs_reconciliation" -> R.string.services_task_needs_reconciliation
        else -> null
    }
    val REPORT_REASONS = listOf("harmful", "deceptive", "privacy", "spam", "other")
    @StringRes fun reasonRes(reason: String): Int = when (reason) {
        "harmful" -> R.string.services_report_harmful
        "deceptive" -> R.string.services_report_deceptive
        "privacy" -> R.string.services_report_privacy
        "spam" -> R.string.services_report_spam
        else -> R.string.services_report_other
    }
    /** The title of a session made by its first request. */
    fun sessionTitle(text: String) = text.take(60)
}

/** What a person who may use a service is told about it. */
internal data class HostedServiceView(val serviceId: String, val name: String, val description: String, val publisherName: String, val sharedBotId: String,
    val availability: String, val entered: Boolean, val generation: Long?, val retention: String, val userConcurrent: Int, val userDaily: Int?, val userDailyLeft: Int?) {
    val available get() = availability == "available"
    val withdrawn get() = availability == "withdrawn"
    companion object {
        fun parse(v: JSONObject): HostedServiceView {
            val limits = v.getJSONObject("limits")
            fun optInt(o: JSONObject, key: String) = if (o.has(key) && !o.isNull(key)) o.getInt(key) else null
            return HostedServiceView(v.getString("serviceId"), v.getString("name"), v.getString("description"), v.getString("publisherName"), v.getString("sharedBotId"),
                v.getString("availability"), v.getBoolean("entered"), if (v.has("generation") && !v.isNull("generation")) v.getLong("generation") else null, v.getString("retention"),
                limits.getInt("userConcurrent"), optInt(limits, "userDaily"), optInt(limits, "userDailyLeft"))
        }
        /** What looking a service up gives: null when it is the viewer's own (the publisher is shown what they set). */
        fun lookup(v: JSONObject): HostedServiceView? = if (v.has("publisher")) null else parse(v)
    }
}

/** A session of a service as the API lists it. */
internal data class HostedServiceSession(val id: String, val title: String) {
    companion object { fun list(v: JSONObject) = v.getJSONArray("conversations").let { a -> (0 until a.length()).map { HostedServiceSession(a.getJSONObject(it).getString("id"), a.getJSONObject(it).getString("title")) } } }
}

/** An open session as far as it has been read: its messages by id, and how much of the change log they include. */
internal data class HostedServiceLog(val after: Long = 0, private val byId: Map<String, TeamMessage> = emptyMap()) {
    val messages: List<TeamMessage> get() = byId.values.sortedBy { it.seq }
    /**
     * Takes one `sync` page read from [after]: the next log and whether more is to be read. A page that does not continue
     * the log, names a message it does not carry, or carries a message of someone else is refused whole.
     */
    fun apply(page: JSONObject, uid: String, sharedBotId: String): Pair<HostedServiceLog, Boolean> {
        val changes = page.getJSONArray("changes")
        val carried = page.getJSONArray("messages").let { a -> (0 until a.length()).map { TeamMessage.parse(a.getJSONObject(it)) } }.associateBy { it.id }
        val next = byId.toMutableMap()
        for (index in 0 until changes.length()) {
            val change = changes.getJSONObject(index)
            if (change.getLong("seq") != after + index + 1) throw TeamMessageFailure("CHANGE_GAP")
            val message = carried[change.getString("messageId")] ?: throw TeamMessageFailure("CHANGE_GAP")
            // A session holds what its one person and the service's one Bot wrote.
            val own = if (message.authorKind == "user") message.authorUid == uid else message.sharedBotId == sharedBotId
            if (!own) throw TeamMessageFailure("INVALID_DATA")
            next[message.id] = message
        }
        return HostedServiceLog(after + changes.length(), next) to (page.getBoolean("hasMore") && changes.length() > 0)
    }
}

/** The commands a caller sends. The service refuses every other one; these are not sent when it would. */
internal object HostedServiceCommands {
    private val TASK_ACTIONS = setOf("createConversation", "submit", "answerWait", "decideWait", "cancelTask", "getTask", "listTasks")
    private val SERVICE_ACTIONS = setOf("listServices", "getService", "enterService", "leaveService", "reportService")
    private fun fail(): Nothing = throw TeamMessageFailure("INVALID_INPUT")
    private fun uuid() = UUID.randomUUID().toString()

    /** A session or request command: the shape of its Team counterpart, checked the same way, with the scope named `serviceId`. */
    fun task(serviceId: String, generation: Long, action: String, payload: JSONObject, commandId: String = uuid()): JSONObject {
        if (!HostedService.isServiceId(serviceId) || action !in TASK_ACTIONS) fail()
        val body = JSONObject().put("schemaVersion", 1).put("commandId", commandId).put("teamId", serviceId).put("generation", generation).put("action", action).put("payload", payload)
        TeamExecutionContract.validate(body)
        body.remove("teamId"); return body.put("serviceId", serviceId)
    }
    fun sessions(serviceId: String, generation: Long): JSONObject {
        if (!HostedService.isServiceId(serviceId) || generation < 1) fail()
        return JSONObject().put("action", "list").put("serviceId", serviceId).put("generation", generation)
    }
    fun sync(serviceId: String, generation: Long, sessionId: String, after: Long): JSONObject {
        if (!HostedService.isServiceId(serviceId) || generation < 1 || after < 0) fail()
        return JSONObject().put("action", "sync").put("serviceId", serviceId).put("generation", generation).put("conversationId", sessionId).put("after", after)
    }
    /** What a typed message is: the answer to the Bot's question when a request of this session waits for one from the viewer, else a new request. */
    fun send(serviceId: String, generation: Long, sharedBotId: String, sessionId: String, text: String, tasks: List<TeamTask>, uid: String,
             messageId: String = uuid(), changeId: String = uuid(), commandId: String = uuid()): JSONObject {
        val payload = JSONObject().put("conversationId", sessionId).put("messageId", messageId).put("changeId", changeId).put("text", text)
        val waitId = answerable(tasks, sessionId, uid, generation)
        return if (waitId != null) task(serviceId, generation, "answerWait", payload.put("waitId", waitId), commandId)
        else task(serviceId, generation, "submit", payload.put("targets", JSONArray().put(sharedBotId)), commandId)
    }
    fun answerable(tasks: List<TeamTask>, sessionId: String?, uid: String, generation: Long): String? =
        tasks.asSequence().filter { it.conversationId == sessionId }.mapNotNull { TeamTaskActions.answerableWait(it, TeamMemberRef(uid, generation), emptySet()) }.firstOrNull()
    fun newSession(serviceId: String, generation: Long, sharedBotId: String, sessionId: String, title: String, uid: String) = task(serviceId, generation, "createConversation",
        JSONObject().put("conversationId", sessionId).put("kind", "bot_dm").put("title", title).put("participantUids", JSONArray().put(uid)).put("sharedBotIds", JSONArray().put(sharedBotId)))
    /** The request that waits for the viewer to approve or decline, if any. */
    fun approval(tasks: List<TeamTask>, uid: String, generation: Long) = tasks.firstOrNull { it.state == "waiting" && it.wait?.kind == "tool_approval" && it.wait.recipient == TeamMemberRef(uid, generation) }
    /** A command for `/v1/services`. */
    fun service(action: String, serviceId: String? = null, more: JSONObject = JSONObject()): JSONObject {
        if (action !in SERVICE_ACTIONS || (action == "listServices") != (serviceId == null) || (serviceId != null && !HostedService.isServiceId(serviceId))) fail()
        val body = JSONObject(more.toString()).put("action", action)
        if (serviceId != null) body.put("serviceId", serviceId)
        return body
    }
    fun report(serviceId: String, reportId: String, reason: String, text: String, sessionId: String?): JSONObject {
        val trimmed = text.trim()
        if (reason !in HostedService.REPORT_REASONS || trimmed.length > 1000) fail()
        val more = JSONObject().put("reportId", reportId).put("reason", reason).put("text", trimmed)
        if (sessionId != null) more.put("sessionId", sessionId)
        return service("reportService", serviceId, more)
    }
}
