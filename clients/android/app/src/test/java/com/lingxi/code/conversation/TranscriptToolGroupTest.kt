package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class TranscriptToolGroupTest {
    private fun tool(id: String, status: AgentToolStatus = AgentToolStatus.Completed) =
        MessageContent.Tool(ToolCallUi(id, "Read", status = status))

    @Test fun consecutiveToolsUseStableFirstIdAndLastSummary() {
        val group = transcriptBlocks(listOf(tool("a"), tool("b"))).single() as TranscriptBlock.Tools
        assertEquals("tool-group:a", group.id)
        assertEquals("b", group.summary.id)
        assertEquals(2, group.visibleTools.size)
    }

    @Test fun prosePreservesToolBoundaries() {
        val rows = transcriptBlocks(listOf(tool("a"), MessageContent.Text("Result"), tool("b")))
        assertEquals(3, rows.size)
        assertTrue(rows[1] is TranscriptBlock.Prose)
    }

    @Test fun runningGroupShowsOnlyActiveTools() {
        val group = transcriptBlocks(listOf(tool("done"), tool("live", AgentToolStatus.Running)))
            .single() as TranscriptBlock.Tools
        assertTrue(group.running)
        assertEquals(listOf("live"), group.visibleTools.map { it.id })
        assertEquals(2, group.calls.size)
    }

    @Test fun blankBoundaryDoesNotSplitGroup() {
        assertEquals(1, transcriptBlocks(listOf(tool("a"), MessageContent.Text("  "), tool("b"))).size)
    }

    @Test fun historicalReasoningDoesNotDivideWireToolGroups() {
        val message = messageDtoToMessage(com.lingxi.code.bindings.MessageDto(
            role = "assistant", blocks = listOf(
                com.lingxi.code.bindings.MessageBlockDto.ToolUse("a", "Read", "{}", null),
                com.lingxi.code.bindings.MessageBlockDto.Thinking("private reasoning", null),
                com.lingxi.code.bindings.MessageBlockDto.RedactedThinking("opaque"),
                com.lingxi.code.bindings.MessageBlockDto.ToolUse("b", "Read", "{}", null),
            )))
        val group = transcriptBlocks(message.blocks).single() as TranscriptBlock.Tools
        assertEquals(listOf("a", "b"), group.calls.map { it.id })
        assertEquals("", message.text)
    }

    private fun state(vararg messages: com.lingxi.code.model.Message) = ChatState(
        session = com.lingxi.code.model.SessionRef("s", "Session"),
        messages = messages.toList(), model = com.lingxi.code.model.EngineModelCatalog.pending,
    )
    private fun message(id: String, vararg blocks: MessageContent) = com.lingxi.code.model.Message(
        role = com.lingxi.code.model.Role.Ai, text = "", id = id, blocks = blocks.toList())

    @Test fun groupsSpanAssistantEnvelopesButStopAtProseAndUserMessages() {
        val rows = buildChatRenderItems(state(
            message("m1", MessageContent.Text("Before"), tool("a")),
            message("m2", tool("b"), MessageContent.Text("After"), tool("c")),
            com.lingxi.code.model.Message(com.lingxi.code.model.Role.User, "Next"),
            message("m3", tool("d")),
        ))
        val groups = rows.filterIsInstance<ChatRenderItem.Tools>()
        assertEquals(listOf(listOf("a", "b"), listOf("c"), listOf("d")), groups.map { it.calls.map { call -> call.id } })
        assertEquals("tool-group:a", groups.first().key)
        assertEquals(rows.size, rows.map { it.key }.toSet().size)
    }

    @Test fun pinnedTerminalTurnStillSeparatesAdjacentToolGroups() {
        val first = message("m1", tool("a"))
        val terminal = AgentRunState(1, active = false, outcome = AgentRunOutcome.Completed, activeWorkers = 1)
        val rows = buildChatRenderItems(state(first, message("m2", tool("b"))).copy(
            agentRun = terminal, agentRunsByMessageId = mapOf(first.id to terminal)))
        assertEquals(2, rows.filterIsInstance<ChatRenderItem.Tools>().size)
    }

    @Test fun syntheticRestoredFootersDoNotSplitProviderEnvelopes() {
        val messages = listOf(message("m1", tool("a")), message("m2", tool("b")))
        val rows = buildChatRenderItems(state(*messages.toTypedArray()).copy(
            agentRunsByMessageId = reconstructTerminalAgentRuns(messages)))
        assertEquals(listOf("tool_group", "agent_run"), rows.map { it.contentType })
        assertEquals(listOf("a", "b"), (rows.first() as ChatRenderItem.Tools).calls.map { it.id })
    }

    @Test fun incompleteHistoryIsNeutralWithoutChangingPersistedCalls() {
        val recorded = message("incomplete", tool("missing", AgentToolStatus.Running))
        val group = buildChatRenderItems(state(recorded)).single() as ChatRenderItem.Tools
        assertEquals(AgentToolStatus.Unknown, group.calls.single().status)
        assertFalse(TranscriptBlock.Tools(group.calls).running)
        assertEquals(AgentToolStatus.Running, (recorded.blocks.single() as MessageContent.Tool).call.status)
    }

    @Test fun authoritativeRecoveryKeepsOnlyItsActuallyRunningToolActive() {
        val recorded = message("incomplete", tool("old", AgentToolStatus.Running), tool("recovering", AgentToolStatus.Running))
        val recovering = state(recorded).copy(streaming = true, agentRun = AgentRunState(7,
            tools = listOf(AgentToolRunState("recovering", "Read", status = AgentToolStatus.Running))))
        val group = buildChatRenderItems(recovering).filterIsInstance<ChatRenderItem.Tools>().single()
        assertEquals(listOf(AgentToolStatus.Unknown, AgentToolStatus.Running), group.calls.map { it.status })
        assertEquals(listOf("recovering"), TranscriptBlock.Tools(group.calls).visibleTools.map { it.id })
        val resolved = recovering.copy(agentRun = recovering.agentRun!!.copy(tools = listOf(
            AgentToolRunState("recovering", "Read", status = AgentToolStatus.Completed))))
        assertEquals(AgentToolStatus.Completed,
            buildChatRenderItems(resolved).filterIsInstance<ChatRenderItem.Tools>().single().calls.last().status)
    }

    @Test fun inactiveTraceCannotRestartAnIncompleteHistoricalTool() {
        val recorded = message("incomplete", tool("missing", AgentToolStatus.Running))
        val stale = AgentRunState(1, active = false, outcome = AgentRunOutcome.Finished,
            tools = listOf(AgentToolRunState("missing", "Read", status = AgentToolStatus.Running)))
        val group = buildChatRenderItems(state(recorded).copy(agentRun = stale))
            .filterIsInstance<ChatRenderItem.Tools>().single()
        assertEquals(AgentToolStatus.Unknown, group.calls.single().status)
    }
}
