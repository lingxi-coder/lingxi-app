package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.bindings.android.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File

/** Shared fixture exercises production presentation and correlation reducers, without JNI. */
class NativeConversationParityFixtureTest {
    private fun scenarios(): List<JSONObject> {
        val path = "packages/bridge-client/fixtures/native-conversation-parity.json"
        val file = generateSequence(File(requireNotNull(System.getProperty("user.dir")))) { it.parentFile }
            .map { File(it, path) }.first { it.isFile }
        val fixture = JSONObject(file.readText())
        assertEquals(1, fixture.getInt("version"))
        return fixture.getJSONArray("scenarios").let { rows -> (0 until rows.length()).map(rows::getJSONObject) }
    }

    private fun JSONObject.strings(key: String) = getJSONArray(key).let { array ->
        (0 until array.length()).map(array::getString)
    }

    @Test fun sharedToolSnapshotsPreserveGroupingStatusesAndActiveSelection() {
        for (scenario in scenarios()) {
            val events = scenario.getJSONArray("events")
            val blocks = (0 until events.length()).flatMap { index ->
                val event = events.getJSONObject(index)
                when (event.getString("kind")) {
                    "reasoning" -> messageDtoToMessage(MessageDto("assistant", listOf(
                        MessageBlockDto.Thinking(event.getString("text"), null)
                    ))).blocks
                    "tool" -> listOf(MessageContent.Tool(ToolCallUi(
                        event.getString("id"), event.getString("name"), status = when(event.getString("status")) {
                            "running" -> AgentToolStatus.Running
                            "completed" -> AgentToolStatus.Completed
                            "failed" -> AgentToolStatus.Failed
                            "cancelled" -> AgentToolStatus.Cancelled
                            else -> error("Unknown fixture tool status")
                        }
                    )))
                    else -> error("Unknown fixture event")
                }
            }
            val groups = transcriptBlocks(blocks).filterIsInstance<TranscriptBlock.Tools>()
            val expected = scenario.getJSONArray("expected_groups")
            assertEquals(scenario.getString("id"), expected.length(), groups.size)
            groups.forEachIndexed { index, group ->
                val want = expected.getJSONObject(index)
                assertEquals(want.strings("ids"), group.calls.map { it.id })
                assertEquals(want.strings("statuses"), group.calls.map { it.status.name.lowercase() })
                assertEquals(want.strings("active_ids"), group.calls.filter { it.status == AgentToolStatus.Running }.map { it.id })
                assertEquals(want.getString("summary"), group.summary.tool)
                assertEquals(if (group.running) want.strings("active_ids") else want.strings("ids"), group.visibleTools.map { it.id })
            }
            assertTrue(transcriptBlocks(blocks).none { it is TranscriptBlock.Prose })
        }
    }

    @Test fun sharedPermissionRetainsCorrelatorAndSuppressedRule() {
        for (scenario in scenarios().filter { it.has("permission") }) {
            val row = scenario.getJSONObject("permission")
            val prompt = permissionRequestToPrompt(PermissionRequest(
                row.getLong("request_id").toULong(),
                PermissionKindDto.ToolUseConfirm(row.getString("tool"), row.getString("input_json"), false),
                null, null, row.getBoolean("suppress_always_allow"), null,
            ))
            assertEquals(row.getLong("request_id").toULong(), prompt.requestId)
            assertEquals(row.getString("expected_detail"), prompt.detail)
            assertEquals(row.getBoolean("suppress_always_allow"), prompt.suppressAlwaysAllowRule)
        }
    }

    @Test fun sharedWorkflowEventsRejectLateProgressAndForeignSessions() {
        for (scenario in scenarios().filter { it.has("workflow_updates") }) {
            val vm = ChatViewModel()
            if (scenario.has("previous_session")) vm.applyActivatedSession(ActivatedSession(
                scenario.getString("previous_session"), emptyList(), SessionActivationKind.Started))
            vm.applyActivatedSession(ActivatedSession(scenario.getString("active_session"), emptyList(), SessionActivationKind.Resumed))
            val updates = scenario.getJSONArray("workflow_updates")
            for (index in 0 until updates.length()) {
                val row = updates.getJSONObject(index)
                vm.reduceWorkflowProgress(WorkflowProgressUpdate(row.getString("origin"), "fixture-task", row.getString("run"),
                    WorkflowProgressDto(kind = "workflow_agent", index = 0u, title = "Fixture agent",
                        message = row.getString("message"), label = null, phaseIndex = 0u, phaseTitle = "Test",
                        agentId = "fixture-agent", agentType = "test", model = null, fallbackModel = null,
                        state = row.getString("state"), error = null, toolUseId = null, queuedAtMs = 1u,
                        startedAtMs = 2u, lastProgressAtMs = row.getLong("time").toULong(), attempt = 1u,
                        lastAttemptReason = null, tokens = null, toolCalls = null, lastToolName = null,
                        lastToolSummary = null, promptPreview = null)))
            }
            val expected = scenario.getJSONObject("expected_workflow")
            val run = requireNotNull(vm.state.value.workflowRuns["fixture-task"])
            assertEquals(expected.getString("run"), run.runId)
            assertEquals(expected.getString("message"), run.agents.single().activity)
            assertEquals(expected.getString("state"), run.agents.single().status.wireValue)
        }
    }
}
