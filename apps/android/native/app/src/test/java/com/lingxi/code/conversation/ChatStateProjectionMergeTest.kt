package com.lingxi.code.conversation

import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ChatStateProjectionMergeTest {
    private fun row(uuid: String, text: String, role: Role = Role.Ai, id: String = uuid) =
        Message(role, text, id = id, transcriptUuid = uuid)

    private fun state(vararg messages: Message) = ChatState(
        session = SessionRef("session-a", "Session"),
        messages = messages.toList(),
        model = EngineModelCatalog.pending,
    )

    @Test
    fun `headless snapshot retains A final and a B completion delivered while reading`() {
        val userA = row("user-a", "A", Role.User, id = "ui-user-a")
        val bootstrapA = row("a", "A bootstrap", id = "ui-a")
        val userB = row("user-b", "B", Role.User, id = "ui-user-b")
        val baseline = listOf(userA, bootstrapA, userB)
        val fresh = listOf(
            userA.copy(id = "disk-user-a"),
            bootstrapA.copy(id = "disk-a", text = "A final suffix"),
            userB.copy(id = "disk-user-b"),
            row("b", "B persisted prefix", id = "disk-b"),
        )
        val live = baseline + row("b", "B live completion", id = "ui-b").copy(streamed = true)

        val merged = mergeHeadlessSnapshotMessages(fresh, live, baseline)
        assertEquals(listOf("ui-user-a", "ui-a", "ui-user-b", "ui-b"), merged.map { it.id })
        assertEquals(listOf("A", "A final suffix", "B", "B live completion"), merged.map { it.text })
        assertTrue(merged.last().streamed)

        val replayed = mergeTranscriptHistory(merged, listOf(row("b", "B live completion", id = "attach-b")))
        assertEquals(merged.map { it.id }, replayed.map { it.id })
        assertEquals(merged.map { it.text }, replayed.map { it.text })
        assertEquals(replayed, mergeTranscriptHistory(replayed, listOf(row("b", "B live completion", id = "attach-b-again"))))
    }

    @Test
    fun `unchanged aggregate bootstrap aliases cannot duplicate UI ids in fresh history`() {
        val bootstrap = row("a1", "stale", id = "ui-a").copy(transcriptAliases = setOf("a2"))
        val fresh = listOf(row("a1", "A before tool"), row("a2", "A final"), row("b", "B prefix"))
        val merged = mergeHeadlessSnapshotMessages(fresh, listOf(bootstrap), listOf(bootstrap))
        assertEquals(listOf("A before tool", "A final", "B prefix"), merged.map { it.text })
        assertEquals(merged.size, merged.map { it.id }.toSet().size)
        assertEquals("ui-a", merged.first().id)
    }

    @Test
    fun `replay aliases collapse only their own durable rows without moving user messages`() {
        val first = row("b1", "B before tool", id = "ui-b")
        val following = row("b2", "B after tool", id = "disk-b2")
        val user = row("user-c", "C", Role.User)
        val complete = row("b1", "B complete", id = "replay-b").copy(transcriptAliases = setOf("b2"))
        val merged = mergeTranscriptHistory(listOf(first, following, user), listOf(complete))
        assertEquals(listOf("ui-b", "user-c"), merged.map { it.id })
        assertEquals(setOf("b1", "b2"), merged.first().transcriptAliases)
        assertEquals("B complete", merged.first().text)
        assertEquals(merged, mergeTranscriptHistory(merged, listOf(complete)))
    }

    @Test
    fun `identical prose and different durable identities remain separate`() {
        val first = row("first", "same")
        val next = row("next", "same")
        val anonymous = Message(Role.Ai, "same", id = "anonymous")
        val merged = mergeTranscriptHistory(listOf(first), listOf(next, anonymous))
        assertEquals(listOf("first", "next", "anonymous"), merged.map { it.id })
        val differentRole = row("first", "same", Role.User, id = "user")
        assertEquals(4, mergeTranscriptHistory(merged, listOf(differentRole)).size)
    }

    @Test
    fun `snapshot and earlier replay cannot downgrade persisted terminal tool results`() {
        for (terminal in listOf(AgentToolStatus.Completed, AgentToolStatus.Failed, AgentToolStatus.Cancelled)) {
            val completedTool = ToolCallUi(
                "read", "Read", status = terminal,
                header = ToolHeaderUi(ToolVerbUi.Read, label = "Read", primary = "a.txt", title = "Read(a.txt)"),
                display = ToolResultDisplayUi(headline = "durable result"),
            )
            val completed = row("a", "A final", id = "ui-a").copy(blocks = listOf(MessageContent.Tool(completedTool)))
            for (unfinished in listOf(AgentToolStatus.Running, AgentToolStatus.Unknown)) {
                val earlyReplay = row("a", "A final", id = "attach-a").copy(
                    blocks = listOf(MessageContent.Tool(ToolCallUi("read", "Read", status = unfinished))),
                )
                val merged = mergeTranscriptHistory(listOf(completed), listOf(earlyReplay)).single()
                assertEquals("ui-a", merged.id)
                assertEquals(completedTool, merged.blocks.filterIsInstance<MessageContent.Tool>().single().call)
                val refreshed = mergeHeadlessSnapshotMessages(listOf(earlyReplay), listOf(completed), listOf(completed)).single()
                assertEquals(completedTool, refreshed.blocks.filterIsInstance<MessageContent.Tool>().single().call)
            }
        }
    }

    @Test
    fun `a later complete message keeps older tool rows missing from its text projection`() {
        val previous = row("b", "B prefix", id = "ui-b").copy(
            blocks = listOf(MessageContent.Tool(ToolCallUi("read", "Read", status = AgentToolStatus.Completed))),
        )
        val result = mergeTranscriptHistory(listOf(previous), listOf(row("b", "B complete", id = "replay-b"))).single()
        assertEquals("B complete", (result.blocks.first() as MessageContent.Text).text)
        assertEquals("read", result.blocks.filterIsInstance<MessageContent.Tool>().single().call.id)
        assertEquals("ui-b", result.id)
    }

    @Test
    fun `settling a replay moves the existing authoritative terminal card to its stable row`() {
        for (outcome in listOf(AgentRunOutcome.Completed, AgentRunOutcome.Failed, AgentRunOutcome.Cancelled)) {
            val prefix = row("b1", "prefix", id = "ui-b")
            val final = row("b2", "final", id = "disk-b2")
            val terminal = AgentRunState(19L).finish(outcome)
            val restored = AgentRunState(-1L).finish(AgentRunOutcome.Finished)
            val state = state(prefix, final).copy(agentRunsByMessageId = mapOf(prefix.id to restored, final.id to terminal))
            val settling = row("b1", "complete", id = "replay-b").copy(transcriptAliases = setOf("b2"))

            val settled = state.settleTurn(terminal, settling)
            assertEquals(listOf("ui-b"), settled.messages.map { it.id })
            assertEquals(mapOf("ui-b" to terminal), settled.agentRunsByMessageId)
            val replayed = state.copy(messages = settled.messages, agentRunsByMessageId = settled.agentRunsByMessageId)
                .settleTurn(terminal, settling)
            assertEquals(settled.messages, replayed.messages)
            assertEquals(settled.agentRunsByMessageId, replayed.agentRunsByMessageId)
        }
    }

    @Test
    fun `headless row collapse reanchors precise terminal results and retires orphan map keys`() {
        for (outcome in listOf(AgentRunOutcome.Completed, AgentRunOutcome.Failed, AgentRunOutcome.Cancelled)) {
            val prefix = row("b1", "B prefix", id = "ui-prefix")
            val final = row("b2", "B complete", id = "ui-final").copy(transcriptAliases = setOf("b1"))
            val restored = AgentRunState(-1L).finish(AgentRunOutcome.Finished)
            val terminal = AgentRunState(19L).finish(outcome)
            val previous = listOf(prefix, final)
            val fresh = listOf(row("a", "A final"), row("b1", "B prefix"), row("b2", "B persisted final"))
            val messages = mergeHeadlessSnapshotMessages(fresh, previous, baseline = listOf(prefix))
            val runs = mergeHeadlessSnapshotAgentRuns(
                previous, mapOf(prefix.id to restored, final.id to terminal, "retired" to terminal), messages,
            )

            assertEquals(listOf("a", "ui-prefix"), messages.map { it.id })
            assertEquals(setOf("a", "ui-prefix"), runs.keys)
            assertEquals(AgentRunOutcome.Finished, runs["a"]?.outcome)
            assertEquals(terminal, runs["ui-prefix"])
        }
    }
}
