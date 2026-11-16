package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.SessionAgentMessageRowDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class HeadlessConversationSnapshotTest {
    private val sessionId = "11111111-2222-4333-8444-555555555555"

    private fun activation() = ActivatedSession(
        sessionId = sessionId,
        transcript = listOf(Message(Role.User, "old bootstrap", transcriptUuid = "old")),
        kind = SessionActivationKind.Resumed,
        mode = SessionMode.Code,
        transcriptComplete = false,
    )

    private fun row(index: ULong, uuid: String, role: String, vararg blocks: MessageBlockDto) =
        SessionAgentMessageRowDto(
            messageIndex = index,
            messageUuid = uuid,
            message = MessageDto(null, role, blocks.toList(), sentAtMs = 1_700_000_000_000L + index.toLong()),
            apiErrorJson = null,
        )

    private fun text(value: String) = MessageBlockDto.Text(value.toClientTextDto())

    private fun snapshot(vararg rows: SessionAgentMessageRowDto) = ClientEvent.SessionAgentTranscript(
        sessionId = sessionId,
        agentId = "main",
        messages = rows.toList(),
        nextMessageIndex = (rows.maxOfOrNull { it.messageIndex } ?: 0uL) + 1uL,
        revision = 9uL,
    )

    @Test
    fun `first UI reads the full main display history after subscribing`() = runTest {
        val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 16)
        val response = snapshot(
            row(4uL, "b-prefix", "assistant", text("B prefix")),
            row(1uL, "a-tool", "assistant", MessageBlockDto.ToolUse("read", "Read", "{}", currentToolHeaderFixture())),
            row(0uL, "a-user", "user", text("A")),
            row(3uL, "b-user", "user", text("B")),
            row(2uL, "a-result", "user", MessageBlockDto.ToolResult(
                "read", "Read", "{}", false, currentToolDisplayFixture("A final result"),
            )),
        )
        val submitted = mutableListOf<ClientCommand>()

        val result = refreshHeadlessConversationSnapshot(activation(), events, submit = { command ->
            assertEquals("the native response can arrive synchronously", 1, events.subscriptionCount.value)
            submitted += command
            events.emit(response.copy(sessionId = "another-session"))
            events.emit(response.copy(agentId = "child"))
            events.emit(response.copy(sessionId = "sess:${sessionId.uppercase()}"))
        })

        assertEquals(listOf(ClientCommand.LoadSessionAgentTranscript("main")), submitted)
        assertTrue(result.transcriptComplete)
        assertEquals(listOf("a-user", "a-tool", "b-user", "b-prefix"), result.transcript.map { it.transcriptUuid })
        assertEquals(listOf("A", "", "B", "B prefix"), result.transcript.map { it.text })
        val tool = result.transcript[1].blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals(AgentToolStatus.Completed, tool.status)
        assertEquals("A final result", tool.display?.headline)
        assertEquals(1_700_000_000_004L, result.transcript.last().sentAtMs)
        assertEquals(0, events.subscriptionCount.value)
    }

    @Test
    fun `new sessions do not query a headless snapshot`() = runTest {
        val fresh = activation().copy(kind = SessionActivationKind.Started)
        val events = MutableSharedFlow<ClientEvent>()
        val result = refreshHeadlessConversationSnapshot(fresh, events, submit = { error("unexpected query") })
        assertSame(fresh, result)
        assertEquals(0, events.subscriptionCount.value)
    }

    @Test
    fun `retained UI projection does not query again`() = runTest {
        val state = ChatState(
            session = com.lingxi.code.model.SessionRef(sessionId, "Session"),
            messages = emptyList(),
            model = com.lingxi.code.model.EngineModelCatalog.pending,
        )
        val retained = RetainedConversationTranscript(
            sessionId, 7L, state, 10L, 7L, ConversationTurnOrigin.Ordinary,
            null, emptyMap(), emptyMap(), emptySet(), emptyMap(), null, emptyMap(), 7L,
        )
        val active = activation().copy(retainedProjection = retained)
        assertSame(active, refreshHeadlessConversationSnapshot(active, MutableSharedFlow(), submit = { error("unexpected query") }))
    }

    @Test
    fun `submit failure is reported and the next read can recover`() = runTest {
        val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 4)
        val failure = IllegalStateException("native submit rejected")
        val result = runCatching {
            refreshHeadlessConversationSnapshot(activation(), events, submit = { throw failure })
        }
        assertSame(failure, result.exceptionOrNull())
        assertEquals(0, events.subscriptionCount.value)

        val recovered = refreshHeadlessConversationSnapshot(activation(), events, submit = {
            events.emit(snapshot(row(0uL, "recovered", "assistant", text("A final"))))
        })
        assertEquals("A final", recovered.transcript.single().text)
    }

    @Test
    fun `cancelled snapshot read releases the subscriber and permits a later read`() = runTest {
        val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 4)
        val submitted = CompletableDeferred<Unit>()
        val request = launch {
            refreshHeadlessConversationSnapshot(activation(), events, submit = { submitted.complete(Unit) })
        }
        runCurrent()
        submitted.await()
        assertEquals(1, events.subscriptionCount.value)
        request.cancelAndJoin()
        assertTrue(request.isCancelled)
        assertEquals(0, events.subscriptionCount.value)

        val recovered = refreshHeadlessConversationSnapshot(activation(), events, submit = {
            events.emit(snapshot(row(0uL, "recovered", "assistant", text("after cancel"))))
        })
        assertEquals("after cancel", recovered.transcript.single().text)
        assertEquals(0, events.subscriptionCount.value)
    }

    @Test
    fun `missing matching response times out without adopting the bootstrap`() = runTest {
        val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 4)
        val result = runCatching {
            refreshHeadlessConversationSnapshot(activation(), events, submit = {
                events.emit(snapshot(row(0uL, "foreign", "assistant", text("wrong session"))).copy(sessionId = "foreign"))
            }, timeoutMillis = 50L)
        }
        assertTrue(result.exceptionOrNull() is TimeoutCancellationException)
        assertEquals(0, events.subscriptionCount.value)
    }

    @Test
    fun `conflicting native row identities are rejected`() = runTest {
        val duplicate = row(0uL, "same-row", "assistant", text("A"))
        val responses = listOf(
            snapshot(duplicate, duplicate.copy(messageIndex = 1uL)),
            snapshot(duplicate, duplicate.copy(messageUuid = "another-row")),
            snapshot(duplicate).copy(nextMessageIndex = 0uL),
        )
        responses.forEach { response ->
            val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 4)
            val result = runCatching {
                refreshHeadlessConversationSnapshot(activation(), events, submit = { events.emit(response) })
            }
            assertTrue(result.exceptionOrNull() is IllegalStateException)
            assertEquals(0, events.subscriptionCount.value)
        }
    }
}
