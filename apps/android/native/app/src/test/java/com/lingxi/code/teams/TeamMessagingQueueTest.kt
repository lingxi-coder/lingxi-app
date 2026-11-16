package com.lingxi.code.teams

import com.google.android.gms.tasks.TaskCompletionSource
import com.google.android.gms.tasks.Tasks
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseUser
import com.google.firebase.auth.GetTokenResult
import com.google.firebase.firestore.DocumentSnapshot
import com.google.firebase.firestore.EventListener
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.MetadataChanges
import com.google.firebase.firestore.QuerySnapshot
import java.io.BufferedInputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.SocketException
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.mockito.Mockito.*

class TeamMessagingQueueTest {
    @Test fun publicHumanSendUsesLiveConversationAndIsDurableBeforeOneHttpAck() = runBlocking {
        Harness().use { h ->
            h.start()
            assertTrue(h.storage.load(MEMBER, TEAM, GENERATION).conversations.isEmpty())
            assertEquals(CHAT, h.model.state.value.cache!!.conversations.single().id)
            h.send("Human-only message")
            h.awaitDelivery()
            val sent = h.sent.single()
            assertEquals("mutate", sent.getString("action"))
            assertEquals("create", sent.getString("operation"))
            assertEquals(CHAT, sent.getString("conversationId"))
            assertFalse(sent.has("targets"))
            assertTrue(h.durableBeforeAck.get())
            withContext(h.dispatcher) { h.model.refresh() }
            delay(100)
            assertEquals(1, h.sent.size)
        }
    }

    @Test fun publicBotSendPreservesValidatedLiveTargetsOutsideTheJournal() = runBlocking {
        Harness(bot = true).use { h ->
            h.start()
            assertTrue(h.storage.load(MEMBER, TEAM, GENERATION).conversations.isEmpty())
            h.send("Ask the shared assistant")
            h.awaitDelivery()
            val sent = h.sent.single()
            TeamExecutionContract.validate(sent)
            assertEquals("submit", sent.getString("action"))
            assertEquals(listOf(BOT), sent.getJSONObject("payload").getJSONArray("targets").strings())
            assertEquals(CHAT, sent.getJSONObject("payload").getString("conversationId"))
            assertEquals(GENERATION, sent.getLong("generation"))
            assertTrue(h.durableBeforeAck.get())
        }
    }

    @Test fun anAnswerToABotQuestionIsSentAsAnswerWaitAndAsksNoBot() = runBlocking {
        Harness(bot = true).use { h ->
            h.start()
            withContext(h.dispatcher) { h.model.queue("create", "Blue", replyTo = "question_0001", waitId = WAIT) }
            h.awaitDelivery()
            val sent = h.sent.single()
            TeamExecutionContract.validate(sent)
            assertEquals("answerWait", sent.getString("action"))
            assertEquals(GENERATION, sent.getLong("generation"))
            val payload = sent.getJSONObject("payload")
            assertEquals(setOf("conversationId", "waitId", "messageId", "changeId", "text"), payload.keys().asSequence().toSet())
            assertEquals(CHAT, payload.getString("conversationId")); assertEquals(WAIT, payload.getString("waitId")); assertEquals("Blue", payload.getString("text"))
            assertEquals(sent.getString("commandId"), payload.getString("changeId"))
            assertTrue(h.durableBeforeAck.get())
        }
    }

    @Test fun aClosedWaitKeepsTheAnswerAsAFailedDraftAndIsNotRetried() = runBlocking {
        Harness(bot = true).use { h ->
            h.start()
            h.fail["answerWait"] = "WAIT_CLOSED"
            withContext(h.dispatcher) { h.model.queue("create", "Blue", replyTo = "question_0001", waitId = WAIT) }
            eventually { h.storage.load(MEMBER, TEAM, GENERATION).outbox.singleOrNull()?.optString("error") == "WAIT_CLOSED" }
            val refused = h.refused.size
            withContext(h.dispatcher) { h.model.refresh() }
            delay(150)
            assertEquals(refused, h.refused.size)
            assertFalse(h.model.state.value.revoked)
        }
    }

    @Test fun cancellingATaskAsksOnceAndShowsTheTasksNewState() = runBlocking {
        Harness(bot = true).use { h ->
            h.start()
            withContext(h.dispatcher) { h.model.cancelTask(TASK) }
            eventually { h.model.state.value.tasks.singleOrNull()?.state == "cancelled" }
            assertEquals(listOf("cancelTask", "getTask"), h.asked.map { it.getString("action") })
            for (command in h.asked) { TeamExecutionContract.validate(command); assertEquals(TASK, command.getJSONObject("payload").getString("taskId")) }
            assertEquals("", h.model.state.value.error)
            // A task that ended before the request arrived is only refreshed.
            h.asked.clear(); h.fail["cancelTask"] = "TASK_FINISHED"
            withContext(h.dispatcher) { h.model.cancelTask(TASK) }
            eventually { h.asked.any { it.getString("action") == "getTask" } }
            assertEquals("", h.model.state.value.error)
        }
    }

    @Test fun accountChangeWhileTokenIsPendingCannotDeliverTheQueuedCommand() = runBlocking {
        Harness().use { h ->
            h.start()
            val pending = TaskCompletionSource<GetTokenResult>()
            doReturn(pending.task).`when`(h.user).getIdToken(false)
            h.send("Must stay in its original account")
            eventually { h.storage.load(MEMBER, TEAM, GENERATION).outbox.isNotEmpty() }
            withContext(h.dispatcher) {
                val other = mock(FirebaseUser::class.java)
                doReturn("other_member").`when`(other).uid
                doReturn(other).`when`(h.auth).currentUser
                h.authListener.onAuthStateChanged(h.auth)
            }
            pending.setResult(h.token)
            eventually { h.model.state.value.revoked }
            delay(100)
            assertTrue(h.sent.isEmpty())
            assertTrue(h.storage.load("other_member", TEAM, GENERATION).outbox.isEmpty())
            assertTrue(h.storage.load(MEMBER, TEAM, GENERATION + 1).outbox.isEmpty())
        }
    }

    /**
     * docs/FRIENDS-CLIENTS.md D25: a friend's chat is a messaging model pinned to the pair team, and its Bots page and Bot DMs run on the same
     * model. A Bot command the server refuses because the membership or the conversation is gone (listing the Bots, creating or sending in a Bot
     * DM, pausing) revokes the whole model, which is what ends the chat; any other failure leaves it alone.
     */
    @Test fun aBotCommandRefusedBecauseTheChatEndedRevokesTheWholeMessaging() = runBlocking {
        for (code in listOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "CONVERSATION_REQUIRED")) {
            for (command in listOf("pauseSharedBot", "createConversation", "submit", "listSharedBots")) {
                Harness(bot = true).use { h ->
                    h.start()
                    assertFalse("$command $code: not revoked before", h.model.state.value.revoked)
                    h.fail[command] = code
                    withContext(h.dispatcher) {
                        when (command) {
                            "createConversation" -> h.model.create("bot_dm", "Helper", emptyList(), "newconv_01", listOf(BOT)) { }
                            "submit" -> h.model.queue("create", "Ask the shared assistant")
                            else -> h.model.pause(ownBotCard())
                        }
                    }
                    eventually { h.model.state.value.revoked }
                    assertEquals("$command $code", "MEMBERSHIP_REQUIRED", h.model.state.value.error)
                }
            }
        }
    }

    @Test fun aBotCommandThatFailsForAnotherReasonLeavesTheChatAlone() = runBlocking {
        for (code in listOf("SERVICE_UNAVAILABLE", "BOT_USE_REQUIRED", "RATE_LIMITED")) {
            Harness(bot = true).use { h ->
                h.start()
                h.fail["pauseSharedBot"] = code
                withContext(h.dispatcher) { h.model.pause(ownBotCard()) }
                eventually { h.model.state.value.error == code }
                delay(200)
                assertFalse(code, h.model.state.value.revoked)
            }
        }
    }

    @Test fun aFileIsUploadedInPartsCheckedSentWithItsMessageAndReadBackWhole() = runBlocking {
        Harness().use { h ->
            h.start()
            val bytes = ByteArray(2500) { (it % 251).toByte() }
            val progress = CopyOnWriteArrayList<Int>()
            h.model.uploadRetryMs = 5
            // One part and the check are refused once by a busy service: each is the same step again, with the same command id.
            h.busy["putUploadPart"] = 1; h.busy["finalizeUpload"] = 1
            h.model.upload(CHAT, UPLOAD, "notes.txt", "text/plain", bytes) { progress += it }
            assertEquals(listOf(1000, 2000, 2500), progress.toList())
            assertEquals(listOf("createUpload", "putUploadPart", "putUploadPart", "putUploadPart", "finalizeUpload"), h.asked.map { it.getString("action") })
            assertEquals(listOf("${UPLOAD}_c", "${UPLOAD}_p0", "${UPLOAD}_p1", "${UPLOAD}_p2", "${UPLOAD}_f"), h.asked.map { it.getString("commandId") })
            assertEquals(2, h.refused.size)
            val created = h.asked.first().getJSONObject("payload")
            assertEquals(TeamAttachments.sha256Hex(bytes), created.getString("sha256")); assertEquals(2500, created.getInt("byteLength")); assertEquals(CHAT, created.getString("conversationId"))

            // The message: files alone, no Bot asked. It is a submit with no text and no targets, and its file names stay on the device.
            val file = TeamDraftFile(UPLOAD, "notes.txt", "text/plain", bytes.size, TeamDraftFile.State.Ready, bytes.size)
            withContext(h.dispatcher) { h.model.queue("create", "   ", files = listOf(file), conversationId = CHAT) }
            eventually { !h.model.state.value.submitting }
            h.awaitDelivery()
            val sent = h.sent.single()
            TeamExecutionContract.validate(sent)
            assertEquals("submit", sent.getString("action"))
            assertFalse(sent.has("attachmentNames"))
            val payload = sent.getJSONObject("payload")
            assertEquals("", payload.getString("text")); assertEquals(0, payload.getJSONArray("targets").length()); assertEquals(listOf(UPLOAD), payload.getJSONArray("attachments").strings())

            // Read back: every part is asked of the service, and the whole is checked against what the message says.
            val carried = TeamMessageAttachment(UPLOAD, "notes.txt", "text/plain", bytes.size, TeamAttachments.sha256Hex(bytes))
            assertArrayEquals(bytes, h.model.download(carried))
            assertEquals(3, h.asked.count { it.getString("action") == "readAttachmentPart" })
            assertEquals("ATTACHMENT_DAMAGED", runCatching { h.model.download(carried.copy(sha256 = "0".repeat(64))) }.exceptionOrNull().let { (it as TeamMessageFailure).code })
            h.model.revokeAttachment(UPLOAD)
            assertEquals(UPLOAD, h.asked.last { it.getString("action") == "revokeAttachment" }.getJSONObject("payload").getString("attachmentId"))
        }
    }

    @Test fun aRefusedFileStopsAtOnceAndFilesAreNeverQueuedForAnotherConversationOrUnchecked() = runBlocking {
        Harness().use { h ->
            h.start()
            h.model.uploadRetryMs = 5
            // The service's refusal is final: it is not asked again and nothing more of the file is sent.
            h.fail["createUpload"] = "UPLOAD_QUOTA_EXCEEDED"
            assertEquals("UPLOAD_QUOTA_EXCEEDED", (runCatching { h.model.upload(CHAT, UPLOAD, "a.txt", "text/plain", ByteArray(10)) { } }.exceptionOrNull() as TeamMessageFailure).code)
            assertEquals(1, h.refused.size); assertTrue(h.asked.isEmpty())
            // A file that arrives changed is refused by the check.
            h.fail.clear()
            h.partBytes = 4
            val upload = (runCatching { h.model.upload(CHAT, UPLOAD, "a.txt", "text/plain", ByteArray(10)) { h.parts["$UPLOAD/0"] = "AAAA" } }.exceptionOrNull() as TeamMessageFailure)
            assertEquals("UPLOAD_DIGEST_MISMATCH", upload.code)

            val ready = TeamDraftFile(UPLOAD, "a.txt", "text/plain", 10, TeamDraftFile.State.Ready, 10)
            suspend fun queued(files: List<TeamDraftFile>, conversation: String?, operation: String = "create", waitId: String? = null): Boolean {
                withContext(h.dispatcher) { h.model.queue(operation, "hello", messageId = if (operation == "edit") "message_0001" else null, waitId = waitId, files = files, conversationId = conversation) }
                eventually { !h.model.state.value.submitting }
                delay(50)
                return h.storage.load(MEMBER, TEAM, GENERATION).outbox.isNotEmpty() || h.sent.isNotEmpty()
            }
            assertFalse(queued(listOf(ready), "chat_other1"))
            assertFalse(queued(listOf(ready), null))
            assertFalse(queued(listOf(ready.copy(state = TeamDraftFile.State.Uploading)), CHAT))
            assertFalse(queued(List(6) { ready.copy(uploadId = "upload_000$it") }, CHAT))
            assertFalse(queued(listOf(ready), CHAT, operation = "edit"))
            assertFalse(queued(listOf(ready), CHAT, waitId = WAIT))
            assertTrue(h.sent.isEmpty())
            // Leaving the team ends an upload: nothing more is sent for it.
            withContext(h.dispatcher) { h.model.stop() }
            assertEquals("INVALID_SESSION", (runCatching { h.model.upload(CHAT, UPLOAD, "a.txt", "text/plain", ByteArray(10)) { } }.exceptionOrNull() as TeamMessageFailure).code)
        }
    }

    private class Harness(private val bot: Boolean = false) : AutoCloseable {
        val directory = Files.createTempDirectory("team-public-queue-").toFile()
        val storage = TeamCommandStorage(directory)
        val dispatcher = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
        private val scope = CoroutineScope(SupervisorJob() + dispatcher)
        val auth = mock(FirebaseAuth::class.java)
        val user = mock(FirebaseUser::class.java)
        val token = mock(GetTokenResult::class.java)
        private val db = mock(FirebaseFirestore::class.java, RETURNS_DEEP_STUBS)
        lateinit var authListener: FirebaseAuth.AuthStateListener
        private lateinit var memberListener: EventListener<DocumentSnapshot>
        private lateinit var directoryListener: EventListener<QuerySnapshot>
        private lateinit var messageListener: EventListener<QuerySnapshot>
        val sent = CopyOnWriteArrayList<JSONObject>()
        /** The task commands the model sent, in order, and every request the server refused. */
        val asked = CopyOnWriteArrayList<JSONObject>()
        val refused = CopyOnWriteArrayList<JSONObject>()
        val durableBeforeAck = AtomicBoolean(false)
        /** An action the server refuses with HTTP 403 and this error code, set by a test after the model has started. */
        val fail = java.util.concurrent.ConcurrentHashMap<String, String>()
        /** The upload service in small: what `createUpload` announced, the parts that arrived, and what was checked or given up. */
        val uploads = java.util.concurrent.ConcurrentHashMap<String, JSONObject>()
        val parts = java.util.concurrent.ConcurrentHashMap<String, String>()
        var partBytes = 1000
        /** How many more requests of an action the server answers with HTTP 503 before it serves them. */
        val busy = java.util.concurrent.ConcurrentHashMap<String, Int>()
        private val server = ServerSocket(0, 0, InetAddress.getByName("127.0.0.1"))
        private val httpThreads = Executors.newSingleThreadExecutor()
        val model: TeamMessagingModel

        init {
            doReturn(MEMBER).`when`(user).uid
            doReturn(user).`when`(auth).currentUser
            doReturn("synthetic-test-token").`when`(token).token
            doReturn(Tasks.forResult(token)).`when`(user).getIdToken(false)
            doAnswer { authListener = it.getArgument(0); null }.`when`(auth).addAuthStateListener(anyAuthListener())
            val root = db.collection("teams").document(TEAM)
            val registration = mock(ListenerRegistration::class.java)
            val memberDocument = root.collection("members").document(MEMBER)
            val conversations = root.collection("conversations").whereArrayContains("memberUids", MEMBER)
            val messages = root.collection("conversations").document(CHAT).collection("messages").orderBy("seq")
            doAnswer { memberListener = it.getArgument(1); registration }.`when`(memberDocument)
                .addSnapshotListener(anyMetadata(), anyListener())
            doAnswer { directoryListener = it.getArgument(1); registration }.`when`(conversations)
                .addSnapshotListener(anyMetadata(), anyListener())
            doAnswer { messageListener = it.getArgument(1); registration }.`when`(messages)
                .addSnapshotListener(anyMetadata(), anyListener())
            httpThreads.submit { while (!server.isClosed) try { server.accept().use { socket ->
                socket.soTimeout = 5000
                val stream = BufferedInputStream(socket.getInputStream())
                fun line(): String = buildString { while (true) { val c = stream.read(); if (c == -1 || c == 10) break; if (c != 13) append(c.toChar()) } }
                check(line().startsWith("POST /v1/teams/messages "))
                var length = 0
                while (true) {
                    val header = line(); if (header.isEmpty()) break
                    if (header.startsWith("Content-Length:", ignoreCase = true)) length = header.substringAfter(':').trim().toInt()
                }
                val input = JSONObject(String(stream.readNBytes(length), Charsets.UTF_8))
                val waits = busy[input.getString("action")] ?: 0
                if (waits > 0) busy[input.getString("action")] = waits - 1
                val refusal = if (waits > 0) "SERVICE_UNAVAILABLE" else fail[input.getString("action")]
                if (refusal != null) refused += input
                val result = if (refusal != null) JSONObject().put("error", JSONObject().put("code", refusal)) else when (input.getString("action")) {
                    "listSharedBots" -> JSONObject().put("bots", JSONArray().also { if (bot) it.put(botCard()) })
                    "listTasks" -> JSONObject().put("tasks", JSONArray()).put("next", JSONObject.NULL)
                    "cancelTask" -> { asked += input; JSONObject().put("taskId", TASK).put("state", "cancelled") }
                    "getTask" -> { asked += input; JSONObject("""{"taskId":"$TASK","conversationId":"$CHAT","sharedBotId":"$BOT","requester":{"uid":"$MEMBER","generation":$GENERATION},"state":"cancelled","createdAt":1,"deadline":2,"replyId":null,"artifactId":null}""") }
                    "mutate", "submit", "answerWait" -> {
                        val change = input.optJSONObject("payload")?.getString("changeId") ?: input.getString("changeId")
                        durableBeforeAck.set(storage.load(MEMBER, TEAM, GENERATION).outbox.any {
                            (it.optJSONObject("payload")?.getString("changeId") ?: it.getString("changeId")) == change
                        })
                        sent += input
                        JSONObject().put("ok", true)
                    }
                    "pauseSharedBot", "createConversation" -> JSONObject().put("ok", true)
                    "createUpload" -> { asked += input; val p = input.getJSONObject("payload"); uploads[p.getString("uploadId")] = p
                        JSONObject().put("uploadId", p.getString("uploadId")).put("state", "uploading").put("parts", (p.getInt("byteLength") + partBytes - 1) / partBytes).put("partBytes", partBytes) }
                    "putUploadPart" -> { asked += input; val p = input.getJSONObject("payload"); parts["${p.getString("uploadId")}/${p.getInt("index")}"] = p.getString("data"); JSONObject().put("index", p.getInt("index")) }
                    "finalizeUpload" -> { asked += input; val id = input.getJSONObject("payload").getString("uploadId"); val upload = uploads.getValue(id)
                        val count = (upload.getInt("byteLength") + partBytes - 1) / partBytes
                        val whole = (0 until count).map { java.util.Base64.getDecoder().decode(parts["$id/$it"] ?: "") }.fold(ByteArray(0)) { all, part -> all + part }
                        val good = whole.size == upload.getInt("byteLength") && TeamAttachments.sha256Hex(whole) == upload.getString("sha256")
                        JSONObject().put("uploadId", id).put("state", if (good) "verified" else "failed").put("parts", count).put("partBytes", partBytes).also { if (!good) it.put("code", "UPLOAD_DIGEST_MISMATCH") } }
                    "readAttachmentPart" -> { asked += input; val p = input.getJSONObject("payload"); val id = p.getString("attachmentId"); val upload = uploads.getValue(id)
                        JSONObject().put("attachmentId", id).put("index", p.getInt("index")).put("parts", (upload.getInt("byteLength") + partBytes - 1) / partBytes).put("data", parts.getValue("$id/${p.getInt("index")}")) }
                    "cancelUpload" -> { asked += input; JSONObject().put("uploadId", input.getJSONObject("payload").getString("uploadId")).put("state", "cancelled") }
                    "revokeAttachment" -> { asked += input; JSONObject().put("attachmentId", input.getJSONObject("payload").getString("attachmentId")).put("state", "revoked") }
                    else -> error("Unexpected public queue request")
                }
                val bytes = result.toString().toByteArray()
                socket.getOutputStream().use { output ->
                    output.write("HTTP/1.1 ${if (waits > 0) "503 Service Unavailable" else if (refusal != null) "403 Forbidden" else "200 OK"}\r\nContent-Type: application/json\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
                    output.write(bytes); output.flush()
                }
            } } catch (error: SocketException) { if (!server.isClosed) throw error } }
            model = TeamMessagingModel(scope, directory, "http://127.0.0.1:${server.localPort}", auth, db)
        }

        suspend fun start() {
            withContext(dispatcher) {
                model.start(MEMBER, TeamSummary(TEAM, "Queue regression", 2, "member", GENERATION))
                val member = mock(DocumentSnapshot::class.java, RETURNS_DEEP_STUBS)
                doReturn(GENERATION).`when`(member).getLong("generation")
                memberListener.onEvent(member, null)
                val row = mock(DocumentSnapshot::class.java, RETURNS_DEEP_STUBS)
                doReturn(CHAT).`when`(row).id
                doReturn(mapOf("kind" to if (bot) "bot_dm" else "dm", "title" to "Queue regression",
                    "participants" to if (bot) listOf("user:$MEMBER", "shared_bot:$BOT") else listOf("user:$MEMBER", "user:owner_test"),
                    "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null)).`when`(row).data
                val snapshot = mock(QuerySnapshot::class.java, RETURNS_DEEP_STUBS)
                doReturn(listOf(row)).`when`(snapshot).documents
                directoryListener.onEvent(snapshot, null)
                assertEquals("Live directory error: ${model.state.value.error}", listOf(CHAT), model.state.value.cache?.conversations?.map { it.id })
                model.select(CHAT)
                messageListener.onEvent(mock(QuerySnapshot::class.java, RETURNS_DEEP_STUBS), null)
            }
            eventually { model.state.value.botsLoaded }
        }

        suspend fun send(text: String) {
            withContext(dispatcher) { model.queue("create", text) }
            eventually { !model.state.value.submitting }
            withContext(dispatcher) { assertEquals("Public queue must use the live directory", "", model.state.value.error) }
        }
        suspend fun awaitDelivery() = eventually {
            sent.isNotEmpty() && storage.load(MEMBER, TEAM, GENERATION).outbox.isEmpty()
        }
        override fun close() {
            runBlocking(dispatcher) { model.stop() }
            scope.cancel(); dispatcher.close(); server.close(); httpThreads.shutdownNow()
            directory.deleteRecursively()
        }
    }

    companion object {
        private const val MEMBER = "member_test"
        private const val TEAM = "team_test01"
        private const val CHAT = "chat_test01"
        private const val BOT = "sharedbot_01"
        private const val GENERATION = 7L
        private const val TASK = "task_test01"
        private const val UPLOAD = "upload_test01"
        private const val WAIT = "wait_0123456789abcdef0123456789abcdef01234567"
        private suspend fun eventually(test: () -> Boolean) = withTimeout(5000) { while (!test()) delay(10) }
        private fun anyMetadata() = eq(MetadataChanges.INCLUDE) ?: MetadataChanges.INCLUDE
        private fun <T> anyListener(): EventListener<T> = any<EventListener<T>>() ?: EventListener { _, _ -> }
        private fun anyAuthListener(): FirebaseAuth.AuthStateListener = any<FirebaseAuth.AuthStateListener>() ?: FirebaseAuth.AuthStateListener { }
        /** The Bot the model's own member maintains in this generation, which is the only kind it may pause. */
        private fun ownBotCard() = TeamBotCard.parse(JSONObject(botCard().toString()).put("maintainer", JSONObject().put("uid", MEMBER).put("generation", GENERATION)))
        private fun botCard() = JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner_test","generation":1},"revision":1,"state":"published","permitted":true,"ready":false,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}""")
    }
}
