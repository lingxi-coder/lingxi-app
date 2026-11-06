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
        val durableBeforeAck = AtomicBoolean(false)
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
                val result = when (input.getString("action")) {
                    "listSharedBots" -> JSONObject().put("bots", JSONArray().also { if (bot) it.put(botCard()) })
                    "listTasks" -> JSONObject().put("tasks", JSONArray()).put("next", JSONObject.NULL)
                    "mutate", "submit" -> {
                        val change = input.optJSONObject("payload")?.getString("changeId") ?: input.getString("changeId")
                        durableBeforeAck.set(storage.load(MEMBER, TEAM, GENERATION).outbox.any {
                            (it.optJSONObject("payload")?.getString("changeId") ?: it.getString("changeId")) == change
                        })
                        sent += input
                        JSONObject().put("ok", true)
                    }
                    else -> error("Unexpected public queue request")
                }
                val bytes = result.toString().toByteArray()
                socket.getOutputStream().use { output ->
                    output.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
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
        private suspend fun eventually(test: () -> Boolean) = withTimeout(5000) { while (!test()) delay(10) }
        private fun anyMetadata() = eq(MetadataChanges.INCLUDE) ?: MetadataChanges.INCLUDE
        private fun <T> anyListener(): EventListener<T> = any<EventListener<T>>() ?: EventListener { _, _ -> }
        private fun anyAuthListener(): FirebaseAuth.AuthStateListener = any<FirebaseAuth.AuthStateListener>() ?: FirebaseAuth.AuthStateListener { }
        private fun botCard() = JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner_test","generation":1},"revision":1,"state":"published","permitted":true,"ready":false,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}""")
    }
}
