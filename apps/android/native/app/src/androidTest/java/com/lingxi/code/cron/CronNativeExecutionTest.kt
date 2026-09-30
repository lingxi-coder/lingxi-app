package com.lingxi.code.cron

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.bindings.android.*
import com.lingxi.code.clipboard.AndroidClipboardAdapter
import com.lingxi.code.location.AndroidLocationAdapter
import com.lingxi.code.notify.AndroidNotificationAdapter
import com.lingxi.code.share.AndroidShareAdapter
import com.lingxi.code.vision.AndroidCameraAdapter
import com.lingxi.code.voice.recorder.AndroidVoiceAdapter
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.File
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.SocketTimeoutException
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

/** Full JNI execution over a loopback-only synthetic provider; never uses real credentials. */
@RunWith(AndroidJUnit4::class)
class CronNativeExecutionTest {
    @Test
    fun fixedModelAndEffortRespectSessionStrategiesAndHumanDefaults(): Unit = runBlocking {
        withTimeout(180_000) {
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val root = File(context.cacheDir, "cron-execution-${UUID.randomUUID()}").apply { mkdirs() }
            val workspace = File(root, "scheduled/workspace").apply { mkdirs() }
            val server = LoopbackAnthropicServer()
            val events = Channel<ClientEvent>(Channel.UNLIMITED)
            try {
                val engine = buildAndroidEngine(
                    config = AndroidEngineLaunchConfigFfi(
                        apiBase = server.baseUrl, apiKey = "test-only-local", model = "anthropic/claude-sonnet-4-6", sessionMode = SessionModeDto.CODE,
                        visionDelegationEnabled = false, appFilesRoot = root.absolutePath, projectCwd = workspace.absolutePath,
                        providerConfig = AndroidProviderConfigFfi("{}", """{"mobileEnabledProfiles":["anthropic"]}"""),
                        mobileLinux = null, localAppsFullRuntime = false, localAppsRuntimeRoot = null,
                        physicalMemoryBytes = 0u, hostEnvironment = null),
                    listener = object : AndroidEventListener {
                        override suspend fun onEvent(event: ClientEvent) {
                            if (event is ClientEvent.ConversationControlsChanged || event is ClientEvent.Error || event is ClientEvent.TurnEnded) events.trySend(event)
                        }
                        override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) {}
                    },
                    stt = object : AndroidStt { override suspend fun transcribe(language: String?): String = error("No speech in scheduled fixture") },
                    tts = object : AndroidTts { override suspend fun synthesize(text: String, voice: String?): TtsAudioFfi = error("No speech in scheduled fixture") },
                    camera = AndroidCameraAdapter(), share = AndroidShareAdapter(), voice = AndroidVoiceAdapter(),
                    location = AndroidLocationAdapter(), notifications = AndroidNotificationAdapter(), clipboard = AndroidClipboardAdapter(),
                    permissions = object : AndroidPermissionSink { override suspend fun onRequest(request: PermissionRequest) { error("Scheduled fixture must not request permission") } },
                    computerUse = null, shell = null, git = null, gitCredentialProvider = null, secureStorage = null, deviceControl = null)

                val store = buildAndroidCronStore(root.path, null)
                try {
                    suspend fun controls(): ConversationControlsDto {
                        while (events.tryReceive().getOrNull() != null) { /* discard older snapshots */ }
                        engine.submit(ClientCommand.GetConversationControls)
                        return withTimeout(20_000) {
                            while (true) {
                                when (val event = events.receive()) {
                                    is ClientEvent.ConversationControlsChanged -> return@withTimeout event.controls
                                    is ClientEvent.Error -> error(event.message)
                                    else -> Unit
                                }
                            }
                            @Suppress("UNREACHABLE_CODE") error("missing controls")
                        }
                    }
                    suspend fun fire(id: String, occurrenceAt: ULong? = null): String {
                        val fired = withTimeout(45_000) {
                            if (occurrenceAt == null) engine.runCronTaskNow(id)
                            else engine.runCronTaskNowAt(id, occurrenceAt)
                        }
                        assertNotNull("Configured task must produce a result", fired)
                        assertEquals("Native execution failed: ${fired?.status}", CronFireStatusDto.Ok, fired!!.status)
                        return requireNotNull(fired.sessionId).removePrefix("sess:")
                    }
                    val human = UUID.randomUUID().toString()
                    engine.resumeEmptySession(human, "Human conversation")
                    engine.submit(ClientCommand.SetReasoningSelection(ReasoningSelectionDto.Level("low")))
                    val before = controls()
                    assertTrue(before.qualifiedModel.endsWith("claude-sonnet-4-6"))
                    assertEquals(ReasoningSelectionDto.Level("low"), before.reasoning.requested)
                    val fixed = CronAutomation.defaults("anthropic/claude-opus-4-6")
                        .withReasoning(ReasoningSelectionDto.Level("high"))
                    val prompt = "cron-native-fixture: Reply with a brief confirmation. Do not call tools."
                    val fresh = store.createConfigured("0 9 * * *", prompt, true, fixed.json)
                    val first = fire(fresh.id)
                    val second = fire(fresh.id)
                    assertNotEquals("new_session must allocate each run", first, second)
                    val dedicated = store.createConfigured("0 9 * * *", prompt, true, fixed.change("runMode", "task_session").json)
                    val owned = fire(dedicated.id)
                    assertEquals("task_session must reuse its durable conversation", owned, fire(dedicated.id))
                    val selected = store.createConfigured("0 9 * * *", prompt, true,
                        fixed.change("runMode", "selected_session").change("targetSessionId", human).json)
                    assertEquals("selected_session must continue the chosen conversation", human, fire(selected.id))
                    engine.submit(ClientCommand.ResumeSession(human, null))
                    val after = controls()
                    val oneShot = store.createConfigured("0 9 * * *", prompt, false, fixed.json)
                    fire(oneShot.id)
                    val completed = store.list().first { it.id == oneShot.id }
                    assertEquals("completed", CronAutomation.from(completed).status)
                    assertNull(completed.nextFireMs)
                    assertNull("Completed one-shot must not execute again", engine.runCronTaskNow(oneShot.id))
                    val requests = server.requests.filter { it.optJSONArray("messages")?.toString().orEmpty().contains("cron-native-fixture") }
                    assertTrue("Each real scheduled run must reach the loopback provider", requests.size >= 6)
                    requests.forEach { request ->
                        assertEquals("claude-opus-4-6", request.getString("model"))
                        assertEquals("high", request.getJSONObject("output_config").getString("effort"))
                    }
                    assertTrue("Loopback fixture failed: ${server.failures}", server.failures.isEmpty())
                    assertEquals("Scheduled model must not change human defaults", before.qualifiedModel, after.qualifiedModel)
                    assertEquals("Scheduled effort must not change human defaults", before.reasoning.requested, after.reasoning.requested)

                    suspend fun latestRun(taskId: String): JSONObject {
                        val task = store.list().first { it.id == taskId }
                        val runs = JSONObject(CronAutomation.from(task).json).getJSONArray("runs")
                        return runs.getJSONObject(runs.length() - 1)
                    }
                    // Keep a real human turn in flight so the scheduled target
                    // must return a durable busy result, then retry the same run.
                    val humanGate = server.blockNextRequest()
                    val humanTurn = async {
                        engine.submit(ClientCommand.SendPrompt("Human busy fixture", null, emptyList(), null))
                    }
                    humanGate.awaitRequest()
                    val manualOccurrence = System.currentTimeMillis().toULong()
                    val busy = withTimeout(20_000) { engine.runCronTaskNowAt(selected.id, manualOccurrence) }
                    assertNotNull("Busy execution must not disappear as a skipped result", busy)
                    val busyStatus = busy!!.status
                    assertTrue(busyStatus is CronFireStatusDto.Failed && busyStatus.message.startsWith("busy:"))
                    assertTrue(busy.retryable)
                    val pending = latestRun(selected.id)
                    assertEquals("queued", pending.getString("status"))
                    assertEquals(manualOccurrence.toLong(), pending.getLong("manualOccurrenceAt"))
                    humanGate.release()
                    humanTurn.await()
                    withTimeout(20_000) {
                        while (events.receive() !is ClientEvent.TurnEnded) { /* await the human turn */ }
                    }
                    assertEquals(human, fire(selected.id, manualOccurrence))
                    assertEquals("Busy retry must retain its persisted run ID", pending.getString("id"), latestRun(selected.id).getString("id"))
                    assertTrue(latestRun(selected.id).getLong("claimGeneration") > pending.getLong("claimGeneration"))
                    assertNull("Successful manual occurrence must not replay", engine.runCronTaskNowAt(selected.id, manualOccurrence))

                    // Exercise the actual UniFFI future cancellation path while
                    // the provider is blocked. The same process must recover.
                    val cancelledGate = server.blockNextRequest()
                    val cancelledOccurrence = System.currentTimeMillis().toULong()
                    val cancelledCall = async { engine.runCronTaskNowAt(fresh.id, cancelledOccurrence) }
                    cancelledGate.awaitRequest()
                    val cancelledId = latestRun(fresh.id).getString("id")
                    cancelledCall.cancelAndJoin()
                    withTimeout(20_000) {
                        while (latestRun(fresh.id).getString("status") != "cancelled") delay(20)
                    }
                    assertEquals(cancelledId, latestRun(fresh.id).getString("id"))
                    cancelledGate.release(sendResponse = false)
                    assertNull("Cancelled manual occurrence must not replay", engine.runCronTaskNowAt(fresh.id, cancelledOccurrence))
                    fire(fresh.id)
                    assertEquals("succeeded", latestRun(fresh.id).getString("status"))
                    assertNotEquals(cancelledId, latestRun(fresh.id).getString("id"))
                } finally { store.destroy(); engine.destroy() }
            } finally { events.close(); server.close(); root.deleteRecursively() }
        }
    }
}

private class RequestGate {
    private val started = Channel<Unit>(1)
    private val released = CountDownLatch(1)
    @Volatile private var respond = true
    suspend fun awaitRequest() = withTimeout(20_000) { started.receive() }
    fun waitForRelease(): Boolean {
        started.trySend(Unit)
        check(released.await(30, TimeUnit.SECONDS)) { "Fixture request was not released" }
        return respond
    }
    fun release(sendResponse: Boolean = true) {
        respond = sendResponse
        released.countDown()
    }
}

private class LoopbackAnthropicServer : Closeable {
    private val socket = ServerSocket(0, 8, InetAddress.getByName("127.0.0.1")).apply { soTimeout = 1_000 }
    val baseUrl = "http://127.0.0.1:${socket.localPort}"
    val requests = CopyOnWriteArrayList<JSONObject>()
    val failures = CopyOnWriteArrayList<String>()
    private val nextGate = AtomicReference<RequestGate?>()
    private val activeGate = AtomicReference<RequestGate?>()
    fun blockNextRequest(): RequestGate = RequestGate().also {
        check(nextGate.compareAndSet(null, it)) { "A request barrier is already pending" }
    }
    private val worker = thread(isDaemon = true, name = "cron-loopback-provider") {
        while (!socket.isClosed) {
            try {
                socket.accept().use { connection ->
                    connection.soTimeout = 15_000
                    val input = connection.getInputStream().buffered()
                    val firstLine = readLine(input)
                    val headers = mutableMapOf<String, String>()
                    while (true) {
                        val line = readLine(input)
                        if (line.isEmpty()) break
                        val split = line.indexOf(':')
                        if (split > 0) headers[line.substring(0, split).lowercase()] = line.substring(split + 1).trim()
                    }
                    val bytes = if (headers["transfer-encoding"]?.contains("chunked") == true) {
                        val chunks = ByteArrayOutputStream()
                        while (true) {
                            val count = readLine(input).substringBefore(';').toInt(16)
                            if (count == 0) { readLine(input); break }
                            require(chunks.size() + count <= 8 * 1024 * 1024)
                            chunks.write(readExact(input, count)); readLine(input)
                        }
                        chunks.toByteArray()
                    } else readExact(input, headers["content-length"]?.toInt() ?: 0)
                    val decoded = if (headers["content-encoding"] == "gzip") {
                        java.util.zip.GZIPInputStream(bytes.inputStream()).use { it.readBytes() }
                    } else bytes
                    val request = JSONObject(decoded.toString(Charsets.UTF_8))
                    require(firstLine.startsWith("POST ") && firstLine.contains("/v1/messages")) { firstLine }
                    requests += request
                    nextGate.getAndSet(null)?.let { gate ->
                        activeGate.set(gate)
                        val respond = try { gate.waitForRelease() } finally { activeGate.compareAndSet(gate, null) }
                        if (!respond) return@use
                    }
                    val frames = listOf(
                        "message_start" to JSONObject().put("type", "message_start").put("message",
                            JSONObject().put("id", "msg_fixture").put("type", "message").put("role", "assistant")
                                .put("model", request.optString("model")).put("content", org.json.JSONArray())
                                .put("usage", JSONObject().put("input_tokens", 1).put("output_tokens", 0))),
                        "content_block_start" to JSONObject("""{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"""),
                        "content_block_delta" to JSONObject("""{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Fixture completed."}}"""),
                        "content_block_stop" to JSONObject("""{"type":"content_block_stop","index":0}"""),
                        "message_delta" to JSONObject("""{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}"""),
                        "message_stop" to JSONObject("""{"type":"message_stop"}"""),
                    )
                    val streaming = request.optBoolean("stream", false)
                    val body = if (streaming) {
                        frames.joinToString("") { (event, payload) -> "event: $event\ndata: $payload\n\n" }.toByteArray()
                    } else {
                        JSONObject("""{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"text","text":"Fixture completed."}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":2}}""")
                            .put("model", request.optString("model")).toString().toByteArray()
                    }
                    val contentType = if (streaming) "text/event-stream" else "application/json"
                    connection.getOutputStream().apply {
                        write("HTTP/1.1 200 OK\r\nContent-Type: $contentType\r\nContent-Length: ${body.size}\r\nConnection: close\r\n\r\n".toByteArray())
                        write(body); flush()
                    }
                }
            } catch (_: SocketTimeoutException) {
                // A bounded accept poll lets close() terminate the daemon promptly.
            } catch (error: Exception) {
                if (!socket.isClosed) failures += error.toString()
            }
        }
    }
    override fun close() {
        nextGate.getAndSet(null)?.release(false)
        activeGate.getAndSet(null)?.release(false)
        socket.close()
        worker.join(2_000)
    }
    private fun readLine(input: InputStream): String {
        val bytes = ByteArrayOutputStream()
        while (true) {
            val value = input.read()
            if (value < 0) throw EOFException()
            if (value == 10) return bytes.toString("UTF-8").removeSuffix("\r")
            require(bytes.size() < 64 * 1024)
            bytes.write(value)
        }
    }
    private fun readExact(input: InputStream, count: Int): ByteArray {
        require(count in 0..8 * 1024 * 1024)
        val bytes = ByteArray(count)
        var offset = 0
        while (offset < count) {
            val read = input.read(bytes, offset, count - offset)
            if (read < 0) throw EOFException()
            offset += read
        }
        return bytes
    }
}
