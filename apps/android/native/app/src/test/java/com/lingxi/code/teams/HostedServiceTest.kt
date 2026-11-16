package com.lingxi.code.teams

import com.lingxi.code.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

/** Bot services (Bots offered outside Teams) on a phone: the rules applied before anything is asked, and the screen's model against a fake service. */
class HostedServiceTest {
    private val serviceId = "s_travel_0001"
    private val sharedBotId = "sharedbot_01"
    private val uid = "caller"

    @Test fun aLinkOrAnIdNamesAServiceAndNothingElseDoes() {
        assertEquals(serviceId, HostedService.serviceIdFrom(" https://lingxios.net/services/$serviceId "))
        assertEquals(serviceId, HostedService.serviceIdFrom("https://lingxios.net/services/$serviceId/"))
        assertEquals(serviceId, HostedService.serviceIdFrom(serviceId))
        for (value in listOf("", "team00001", "https://lingxios.net/teams/join?code=abc", "https://lingxios.net/services/$serviceId/extra",
                "https://lingxios.net/services/team00001", "/services/$serviceId", "services/$serviceId", "s_ab", "S_travel_0001", "not a link at all")) {
            assertNull(value, HostedService.serviceIdFrom(value))
        }
    }

    @Test fun everyAvailabilityRefusalAndEndedRequestHasItsOwnWords() {
        val availability = listOf("available", "disabled", "not_ready", "host_offline", "service_quota_exhausted", "user_quota_exhausted", "withdrawn").map(HostedService::availabilityRes)
        assertEquals(7, availability.toSet().size)
        val codes = listOf("NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN", "SERVICE_DISABLED", "SERVICE_USER_BUSY", "SERVICE_BUSY",
            "REQUEST_QUOTA_EXCEEDED", "HOST_UNAVAILABLE", "SERVICE_SESSION_LIMIT", "REPORT_LIMIT", "SERVICE_UNAVAILABLE", "INVALID_SERVICE_LINK", "OWN_SERVICE")
        assertEquals(codes.size, codes.map(HostedService::errorRes).toSet().size)
        assertFalse(codes.map(HostedService::errorRes).contains(R.string.teams_error))
        // A code a service has no word of its own for is said as Teams says it.
        assertEquals(teamsErrorRes("INVALID_INPUT"), HostedService.errorRes("INVALID_INPUT"))
        assertEquals(R.string.teams_error, HostedService.errorRes("SOMETHING_NEW"))
        assertTrue(listOf("NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN").all(HostedService::isAccessLoss))
        assertFalse(listOf("SERVICE_DISABLED", "SERVICE_BUSY", "SERVICE_USER_BUSY", "REQUEST_QUOTA_EXCEEDED", "HOST_UNAVAILABLE", "SERVICE_UNAVAILABLE").any(HostedService::isAccessLoss))
        val states = listOf("queued", "running", "failed", "expired", "interrupted", "revoked", "cancelled", "authorization_required", "needs_reconciliation")
        assertEquals(states.size, states.map { HostedService.taskRes(task(it)) }.toSet().size)
        assertEquals(R.string.services_task_question, HostedService.taskRes(task("waiting", "human_input")))
        assertEquals(R.string.services_task_approval, HostedService.taskRes(task("waiting", "tool_approval")))
        assertNull(HostedService.taskRes(task("completed")))
        assertEquals(5, HostedService.REPORT_REASONS.map(HostedService::reasonRes).toSet().size)
    }

    @Test fun aServiceAndThePublishersOwnAreToldApart() {
        val view = HostedServiceView.lookup(view(entered = false))!!
        assertEquals(listOf("Travel helper", "Pat", "Kept for 30 days.", "host_offline"), listOf(view.name, view.publisherName, view.retention, view.availability))
        assertEquals(listOf(1, 20, 18), listOf(view.userConcurrent, view.userDaily, view.userDailyLeft))
        assertFalse(view.entered); assertNull(view.generation); assertFalse(view.available); assertFalse(view.withdrawn)
        // A service whose Bot is not published yet has no daily number.
        assertNull(HostedServiceView.parse(view(entered = false).put("limits", JSONObject().put("userConcurrent", 1))).userDaily)
        assertNull(HostedServiceView.lookup(JSONObject().put("serviceId", serviceId).put("publisher", uid).put("state", "active")))
    }

    @Test fun aSessionIsReadByItsChangeLogAndABrokenPageIsRefusedWhole() {
        var (log, more) = HostedServiceLog().apply(page(listOf(1L to "m1", 2L to "m2"), listOf(message("m1", 1, 1, "Where to?"), message("m2", 2, 2, "Lisbon", bot = true)), more = true), uid, sharedBotId)
        assertTrue(more); assertEquals(2, log.after); assertEquals(listOf("Where to?", "Lisbon"), log.messages.map { it.text })
        // A later change to an earlier message replaces it; the order stays the messages' own.
        log.apply(page(listOf(3L to "m1"), listOf(message("m1", 1, 3, "Where to next?"))), uid, sharedBotId).let { (next, again) -> log = next; assertFalse(again) }
        assertEquals(3, log.after); assertEquals(listOf("Where to next?", "Lisbon"), log.messages.map { it.text })
        // Nothing new: the cursor stays and nothing more is asked for, whatever the page says.
        assertFalse(log.apply(page(emptyList(), emptyList(), more = true), uid, sharedBotId).second)
        val broken = listOf(
            page(listOf(5L to "m3"), listOf(message("m3", 3, 5, "skipped"))) to "CHANGE_GAP",
            page(listOf(4L to "m3"), emptyList()) to "CHANGE_GAP",
            page(listOf(4L to "m3", 5L to "m4"), listOf(message("m3", 3, 4, "ok"), message("m4", 4, 5, "someone else", author = "stranger"))) to "INVALID_DATA",
            page(listOf(4L to "m3"), listOf(message("m3", 3, 4, "another Bot", bot = true, author = "sharedbot_99"))) to "INVALID_DATA",
        )
        for ((value, code) in broken) {
            val failure = assertThrows(TeamMessageFailure::class.java) { log.apply(value, uid, sharedBotId) }
            assertEquals(code, failure.code)
        }
        assertEquals(3, log.after)
    }

    @Test fun commandsNameTheServiceNeverATeamAndAreCheckedLikeTheirTeamCounterparts() {
        val sent = HostedServiceCommands.send(serviceId, 2, sharedBotId, "chat_0001", "Where to?", emptyList(), uid, "message_0001", "change_0001", "command_0001")
        assertEquals(setOf("schemaVersion", "commandId", "serviceId", "generation", "action", "payload"), sent.keys().asSequence().toSet())
        assertEquals(serviceId, sent.getString("serviceId")); assertEquals("submit", sent.getString("action"))
        assertEquals(sharedBotId, sent.getJSONObject("payload").getJSONArray("targets").getString(0)); assertFalse(sent.getJSONObject("payload").has("waitId"))
        // The Bot's question to the viewer, in this session, is answered; anything else is a new request.
        val answered = HostedServiceCommands.send(serviceId, 2, sharedBotId, "chat_0001", "Two", listOf(task("waiting", "human_input")), uid)
        assertEquals("answerWait", answered.getString("action")); assertEquals("wait_0001", answered.getJSONObject("payload").getString("waitId"))
        assertFalse(answered.getJSONObject("payload").has("targets"))
        for (other in listOf(task("waiting", "tool_approval"), task("waiting", "human_input", session = "chat_0002"), task("waiting", "human_input", recipient = "someone"), task("running"))) {
            assertEquals("submit", HostedServiceCommands.send(serviceId, 2, sharedBotId, "chat_0001", "Two", listOf(other), uid).getString("action"))
        }
        assertEquals("wait_0001", HostedServiceCommands.approval(listOf(task("running"), task("waiting", "tool_approval")), uid, 2)?.wait?.waitId)
        assertNull(HostedServiceCommands.approval(listOf(task("waiting", "human_input"), task("waiting", "tool_approval", recipient = "someone")), uid, 2))
        val made = HostedServiceCommands.newSession(serviceId, 2, sharedBotId, "chat_0001", HostedService.sessionTitle("a".repeat(90)), uid).getJSONObject("payload")
        assertEquals("bot_dm", made.getString("kind")); assertEquals(60, made.getString("title").length)
        assertEquals(uid, made.getJSONArray("participantUids").getString(0)); assertEquals(sharedBotId, made.getJSONArray("sharedBotIds").getString(0))
        // What the service would refuse is not sent.
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.task(serviceId, 2, "publishSharedBot", JSONObject()) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.task(serviceId, 2, "readArtifact", JSONObject().put("artifactId", "artifact_0001")) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.task("team_0001", 2, "cancelTask", JSONObject().put("taskId", "task_0001")) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.task(serviceId, 0, "cancelTask", JSONObject().put("taskId", "task_0001")) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.send(serviceId, 2, sharedBotId, "chat_0001", "", emptyList(), uid) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.sessions("team_0001", 2) }
        assertEquals("""{"action":"sync","serviceId":"$serviceId","generation":2,"conversationId":"chat_0001","after":4}""".let(::JSONObject).toString(), HostedServiceCommands.sync(serviceId, 2, "chat_0001", 4).toString())
    }

    @Test fun serviceCommandsCarryTheAcknowledgementAndAReportOnlyWhatWasChosen() {
        assertEquals("""{"action":"listServices"}""", HostedServiceCommands.service("listServices").toString())
        val enter = HostedServiceCommands.service("enterService", serviceId, JSONObject().put("acknowledged", true))
        assertEquals(true, enter.getBoolean("acknowledged")); assertEquals(serviceId, enter.getString("serviceId"))
        val report = HostedServiceCommands.report(serviceId, "report_0001", "privacy", "  it asked for my address \n", null)
        assertEquals("it asked for my address", report.getString("text")); assertFalse(report.has("sessionId"))
        assertEquals("chat_0001", HostedServiceCommands.report(serviceId, "report_0001", "other", "", "chat_0001").getString("sessionId"))
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.report(serviceId, "report_0001", "boring", "", null) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.report(serviceId, "report_0001", "spam", "a".repeat(1001), null) }
        // A phone offers nothing: the publisher's commands are not ones it can make.
        for (action in listOf("createService", "updateService", "setServiceOnline", "withdrawService")) assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.service(action, serviceId) }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.service("getService") }
        assertThrows(TeamMessageFailure::class.java) { HostedServiceCommands.service("listServices", serviceId) }
    }

    @Test fun lookingEntersNothingAgreeingOpensItAndARefusedRequestIsSentAgainIntoTheSameSession() = runBlocking {
        Fake().use { f ->
            f.model.accountChanged()
            f.model.lookup("not a link")
            assertEquals("INVALID_SERVICE_LINK", f.model.state.value.error)
            assertTrue(f.sent.isEmpty())
            f.model.lookup("https://lingxios.net/services/$serviceId")
            assertEquals("Travel helper", f.model.state.value.candidate?.name)
            assertEquals(listOf("getService"), f.sent.map { it.second.getString("action") })
            f.model.enter()
            val entered = f.sent.single { it.second.getString("action") == "enterService" }.second
            assertTrue(entered.getBoolean("acknowledged"))
            assertEquals(serviceId, f.model.state.value.open?.serviceId); assertEquals(2L, f.model.state.value.open?.generation)
            assertNull(f.model.state.value.candidate)
            // The first request is refused: its session stays, and asking again goes into it.
            f.refuse = "SERVICE_BUSY"
            assertFalse(f.model.send("Where to?"))
            assertEquals("SERVICE_BUSY", f.model.state.value.error)
            val session = f.model.state.value.session!!
            f.refuse = null
            assertTrue(f.model.send("Where to?"))
            assertEquals(1, f.sessions.size); assertEquals(session, f.model.state.value.session)
            val submit = f.sent.last { it.second.optString("action") == "submit" }
            assertEquals("/v1/teams/messages" to serviceId, submit.first to submit.second.getString("serviceId")); assertFalse(submit.second.has("teamId"))
            assertEquals(listOf("Where to?"), f.model.state.value.messages.map { it.text })
            assertEquals(listOf("queued"), f.model.state.value.tasks.map { it.state }); assertTrue(f.model.state.value.live)
            // The Bot asks something back; the next message is the answer.
            f.botAsks("How many people?")
            f.refresh()
            assertTrue(f.model.state.value.answering)
            assertTrue(f.model.send("Two"))
            assertEquals("answerWait", f.sent.last { it.second.has("schemaVersion") && it.second.getString("action") != "listTasks" }.second.getString("action"))
            assertEquals(listOf("Where to?", "How many people?", "Two"), f.model.state.value.messages.map { it.text })
        }
    }

    @Test fun losingAccessClosesTheSessionsAndAnotherAccountSeesNothingOfTheFirst() = runBlocking {
        Fake(entered = true).use { f ->
            f.model.accountChanged()
            f.model.refresh()
            assertEquals(listOf(serviceId), f.model.state.value.entered.map { it.serviceId })
            f.model.show(f.model.state.value.entered.single())
            assertTrue(f.model.send("Hello"))
            assertNotNull(f.model.state.value.session)
            // The publisher withdrew it: the person is told, and nothing of it is shown any more.
            f.withdrawn = true
            assertFalse(f.model.send("Again"))
            assertEquals("SERVICE_WITHDRAWN", f.model.state.value.error)
            assertNull(f.model.state.value.open); assertTrue(f.model.state.value.messages.isEmpty())
            f.withdrawn = false
            f.model.refresh()
            f.model.show(f.model.state.value.entered.single())
            f.refresh()
            assertTrue(f.model.state.value.messages.isNotEmpty())
            f.account = "someone_else"
            f.model.accountChanged()
            assertEquals("someone_else", f.model.state.value.uid)
            assertNull(f.model.state.value.open); assertTrue(f.model.state.value.entered.isEmpty()); assertTrue(f.model.state.value.messages.isEmpty())
        }
    }

    @Test fun aReportGoesWithTheSessionOnlyWhenChosenAndLeavingClosesTheService() = runBlocking {
        Fake(entered = true).use { f ->
            f.model.accountChanged(); f.model.refresh()
            f.model.show(f.model.state.value.entered.single())
            assertTrue(f.model.send("Hello"))
            assertTrue(f.model.report("harmful", "bad answer", withSession = false, reportId = "report_0001"))
            assertTrue(f.model.report("harmful", "bad answer", withSession = true, reportId = "report_0002"))
            val reports = f.sent.filter { it.second.optString("action") == "reportService" }.map { it.second }
            assertEquals(listOf(false, true), reports.map { it.has("sessionId") })
            assertTrue(f.model.leave())
            assertNull(f.model.state.value.open)
            assertTrue(f.model.state.value.entered.isEmpty())
        }
    }

    // A service as the API keeps it, for one caller.
    private inner class Fake(entered: Boolean = false) : AutoCloseable {
        var account: String? = uid
        var refuse: String? = null
        var withdrawn = false
        private var inService = entered
        val sent = mutableListOf<Pair<String, JSONObject>>()
        val sessions = mutableListOf<Pair<String, String>>()
        private val messages = mutableMapOf<String, MutableList<JSONObject>>()
        private val tasks = mutableListOf<JSONObject>()
        private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val model = HostedServicesModel(scope, { account }, ::answer, answeringMs = 600_000, idleMs = 600_000)

        private fun say(sessionId: String, text: String, bot: Boolean, waitId: String? = null): String {
            val list = messages.getValue(sessionId)
            val id = "message_${sessionId.take(8)}_${list.size + 1}"
            list += message(id, list.size + 1L, list.size + 1L, text, bot).also { if (waitId != null) it.put("waitId", waitId) }
            return id
        }
        fun botAsks(text: String) {
            val task = tasks.first { it.getString("state") == "queued" }
            val id = say(task.getString("conversationId"), text, bot = true, waitId = "wait_0001")
            task.put("state", "waiting").put("wait", JSONObject().put("waitId", "wait_0001").put("kind", "human_input")
                .put("recipient", JSONObject().put("uid", uid).put("generation", 2)).put("expiresAt", 9_000_000_000_000L).put("messageId", id))
        }
        /** One more reading of the open session, as the timer would do. */
        suspend fun refresh() { model.cancel("task_none_0") }

        private suspend fun answer(path: String, body: JSONObject): JSONObject {
            sent += path to JSONObject(body.toString())
            val action = body.getString("action")
            if (path == "/v1/services") return when (action) {
                "listServices" -> JSONObject().put("published", JSONArray()).put("entered", if (inService && !withdrawn) JSONArray().put(view(entered = true)) else JSONArray())
                "getService" -> if (withdrawn) throw TeamMessageFailure("SERVICE_WITHDRAWN") else view(inService)
                "enterService" -> { inService = true; JSONObject().put("generation", 2).put("service", view(entered = true)) }
                "leaveService" -> { inService = false; JSONObject().put("left", true) }
                "reportService" -> JSONObject().put("reported", true)
                else -> throw TeamMessageFailure("INVALID_INPUT")
            }
            if (!inService) throw TeamMessageFailure("SERVICE_ENTRY_REQUIRED")
            if (withdrawn) throw TeamMessageFailure("SERVICE_WITHDRAWN")
            val p = body.optJSONObject("payload") ?: JSONObject()
            return when (action) {
                "list" -> JSONObject().put("conversations", JSONArray().also { a -> sessions.forEach { (id, title) -> a.put(JSONObject().put("id", id).put("title", title)) } })
                "sync" -> {
                    val list = messages[body.getString("conversationId")].orEmpty(); val after = body.getLong("after")
                    val fresh = list.filter { it.getLong("changeSeq") > after }
                    JSONObject().put("changes", JSONArray().also { a -> fresh.forEach { a.put(JSONObject().put("seq", it.getLong("changeSeq")).put("messageId", it.getString("id"))) } })
                        .put("messages", JSONArray().also { a -> fresh.forEach(a::put) }).put("hasMore", false)
                }
                "createConversation" -> { sessions += p.getString("conversationId") to p.getString("title"); messages[p.getString("conversationId")] = mutableListOf(); JSONObject() }
                "listTasks" -> JSONObject().put("tasks", JSONArray().also { a -> tasks.filter { it.getString("conversationId") == p.getString("conversationId") }.forEach(a::put) })
                "cancelTask" -> if (p.getString("taskId") == "task_none_0") throw TeamMessageFailure("TASK_FINISHED") else JSONObject()
                else -> {
                    refuse?.let { throw TeamMessageFailure(it) }
                    when (action) {
                        "submit" -> {
                            say(p.getString("conversationId"), p.getString("text"), bot = false)
                            tasks += JSONObject().put("taskId", "task_000${tasks.size + 1}").put("conversationId", p.getString("conversationId")).put("sharedBotId", sharedBotId)
                                .put("requester", JSONObject().put("uid", uid).put("generation", 2)).put("state", "queued").put("createdAt", tasks.size + 1).put("deadline", 9_000_000_000_000L)
                            JSONObject()
                        }
                        "answerWait" -> {
                            say(p.getString("conversationId"), p.getString("text"), bot = false)
                            tasks.first { it.optJSONObject("wait")?.getString("waitId") == p.getString("waitId") }.apply { put("state", "queued"); remove("wait") }
                            JSONObject()
                        }
                        else -> throw TeamMessageFailure("INVALID_INPUT")
                    }
                }
            }
        }
        override fun close() { scope.cancel() }
    }

    private fun view(entered: Boolean) = JSONObject().put("serviceId", serviceId).put("name", "Travel helper").put("description", "Answers travel questions").put("publisherName", "Pat")
        .put("sharedBotId", sharedBotId).put("availability", if (entered) "available" else "host_offline").put("entered", entered).put("cost", "publisher").put("visibility", "requester_and_publisher")
        .put("retention", "Kept for 30 days.").put("limits", JSONObject().put("userConcurrent", 1).put("userDaily", 20).put("userDailyLeft", 18))
        .also { if (entered) it.put("generation", 2) }

    private fun message(id: String, seq: Long, change: Long, text: String, bot: Boolean = false, author: String? = null) = JSONObject().put("id", id).put("seq", seq)
        .put("author", if (bot) JSONObject().put("kind", "shared_bot").put("sharedBotId", author ?: sharedBotId) else JSONObject().put("kind", "user").put("uid", author ?: uid))
        .put("kind", "text").put("text", text).put("replyTo", JSONObject.NULL).put("createdAt", "2026-11-13T09:00:00.000Z").put("editedAt", JSONObject.NULL)
        .put("changeSeq", change).put("changeId", "change_$change")

    private fun page(changes: List<Pair<Long, String>>, messages: List<JSONObject>, more: Boolean = false) = JSONObject()
        .put("conversation", JSONObject().put("id", "chat_0001"))
        .put("changes", JSONArray().also { a -> changes.forEach { (seq, id) -> a.put(JSONObject().put("seq", seq).put("messageId", id)) } })
        .put("messages", JSONArray().also { a -> messages.forEach(a::put) }).put("hasMore", more)

    private fun task(state: String, wait: String? = null, session: String = "chat_0001", recipient: String = uid) = TeamTask("task_0001", session, sharedBotId, TeamMemberRef(uid, 2), state, 1, 9_000_000_000_000L, null, null,
        wait = wait?.let { TeamTaskWait("wait_0001", it, TeamMemberRef(recipient, 2), 9_000_000_000_000L, "message_9") })
}
