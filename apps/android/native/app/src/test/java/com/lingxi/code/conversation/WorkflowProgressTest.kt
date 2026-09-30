package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.bindings.WorkflowProgressDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkflowProgressTest {
    @Test
    fun terminalAgentRejectsLateProgress() {
        val started = update(
            sessionId = "session-a",
            runId = "run-1",
            state = "progress",
            moment = 10u,
            message = "running tests",
        )
        val done = started.copy(
            progress = started.progress.copy(
                state = "done",
                message = "complete",
                lastProgressAtMs = 20u,
            ),
        )
        val stale = started.copy(
            progress = started.progress.copy(
                state = "progress",
                message = "old heartbeat",
                lastProgressAtMs = 15u,
            ),
        )

        val terminal = reduceWorkflowProgress(
            reduceWorkflowProgress(null, started, nowMs = 10u),
            done,
            nowMs = 20u,
        )
        val result = reduceWorkflowProgress(terminal, stale, nowMs = 30u)

        assertEquals(WorkflowAgentStatus.Done, result?.agents?.single()?.status)
        assertEquals("complete", result?.agents?.single()?.activity)
    }

    @Test
    fun aNewRunIdResetsOldAgentRows() {
        val first = reduceWorkflowProgress(
            null,
            update("session-a", "run-1", "done", 10u, index = 0u),
            nowMs = 10u,
        )
        val second = reduceWorkflowProgress(
            first,
            update("session-a", "run-2", "progress", 20u, index = 2u),
            nowMs = 20u,
        )

        assertEquals("run-2", second?.runId)
        assertEquals(listOf(2uL), second?.agents?.map { it.index })
    }

    @Test
    fun staleCallbackFromAnOldRunCannotReplaceTheNewRun() {
        val current = reduceWorkflowProgress(
            null,
            update("session-a", "run-2", "progress", 20u),
            nowMs = 20u,
        )
        val stale = reduceWorkflowProgress(
            current,
            update("session-a", "run-1", "progress", 10u),
            nowMs = 30u,
        )

        assertEquals("run-2", stale?.runId)
        assertEquals(20uL, stale?.lastUpdatedAtMs)
    }

    @Test
    fun viewModelPartitionsWorkflowUpdatesByOriginSession() {
        val vm = ChatViewModel()
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )

        vm.reduceWorkflowProgress(update("session-a", "run-a", "progress", 10u))
        vm.reduceWorkflowProgress(update("session-b", "run-b", "progress", 20u))

        assertEquals("run-a", vm.state.value.workflowRuns["task-1"]?.runId)
        assertTrue(vm.state.value.workflowRuns.values.none { it.runId == "run-b" })

        vm.applyActivatedSession(
            ActivatedSession("session-b", emptyList(), SessionActivationKind.Resumed),
        )
        assertEquals("run-b", vm.state.value.workflowRuns["task-1"]?.runId)
    }

    @Test
    fun statusArrivingBeforeProgressIsAppliedWhenTheRunAppears() {
        val vm = ChatViewModel()
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        vm.reduceClientEvent(
            ClientEvent.TaskStatusChanged(
                taskId = "task-1",
                status = TaskStatusDto.PAUSED,
                originSessionId = "session-a",
                error = null,
            ),
        )

        vm.reduceWorkflowProgress(update("session-a", "run-a", "progress", 10u))

        assertEquals(TaskStatusDto.PAUSED, vm.state.value.workflowRuns["task-1"]?.status)
        assertTrue(vm.state.value.activeBackgroundTaskIds.isEmpty())
        assertTrue(!TaskStatusDto.PAUSED.isActivelyExecutingWorkflow())
    }

    @Test
    fun modelLineShowsTheActualFallbackChain() {
        val run = reduceWorkflowProgress(
            null,
            update("session-a", "run-1", "progress", 10u),
            nowMs = 10u,
        ) ?: error("workflow update should create a run")
        val agent = run.agents.single()

        assertEquals("deepseek/deepseek-v3.2 → deepseek/deepseek-chat", workflowModelLine(agent))
        assertNull(workflowModelLine(agent.copy(model = null)))
    }

    private fun update(
        sessionId: String,
        runId: String,
        state: String,
        moment: ULong,
        message: String = "working",
        index: ULong = 0u,
    ) = WorkflowProgressUpdate(
        originSessionId = sessionId,
        taskId = "task-1",
        runId = runId,
        progress = WorkflowProgressDto(
            kind = "workflow_agent",
            index = index,
            title = "design",
            message = message,
            label = null,
            phaseIndex = 0u,
            phaseTitle = "Design",
            agentId = "agent-1",
            agentType = "design",
            model = "deepseek/deepseek-v3.2",
            fallbackModel = "deepseek/deepseek-chat",
            state = state,
            error = null,
            toolUseId = null,
            queuedAtMs = 5u,
            startedAtMs = 6u,
            lastProgressAtMs = moment,
            attempt = 1u,
            lastAttemptReason = null,
            tokens = 120u,
            toolCalls = 2u,
            lastToolName = "Bash",
            lastToolSummary = "npm test",
            promptPreview = "Design the app",
        ),
    )
}
