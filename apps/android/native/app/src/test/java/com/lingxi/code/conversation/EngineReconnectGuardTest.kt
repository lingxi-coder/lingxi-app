package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.AskUserQuestionRequestDto
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class EngineReconnectGuardTest {
    private val idle = ChatState(SessionRef("s", "Session"), emptyList(), model = EngineModelCatalog.pending)

    @Test fun permissionAndQuestionsBlockEvenWithoutStreaming() {
        assertTrue(idle.blocksEngineReconnect(hasPendingPermission = true))
        assertTrue(idle.copy(pendingQuestions = listOf(AskUserQuestionRequestDto(1uL, emptyList(), null)))
            .blocksEngineReconnect(hasPendingPermission = false))
        assertFalse(idle.blocksEngineReconnect(hasPendingPermission = false))
    }

    @Test fun compactionAndBackgroundWorkBlockReconnect() {
        val states = listOf(
            idle.copy(compaction = CompactionProgressUi(CompactionProgressStatus.Running)),
            idle.copy(activeBackgroundTaskIds = setOf("task")),
            idle.copy(backgroundTasks = mapOf("task" to BackgroundTaskUi(
                "task", "shell", "Executing", TaskStatusDto.RUNNING, false, null))),
            idle.copy(agentRun = AgentRunState(1, activeWorkers = 1)),
            idle.copy(cancellationInFlight = true),
            idle.copy(sessionTransitioning = true),
        )
        states.forEach { assertTrue(it.blocksEngineReconnect(hasPendingPermission = false)) }
        assertFalse(idle.copy(compaction = CompactionProgressUi(CompactionProgressStatus.Completed))
            .blocksEngineReconnect(hasPendingPermission = false))
    }

    @Test fun pendingTransitionExceptionNeverAllowsLiveWorkReplacement() {
        val pending = idle.copy(sessionTransitioning = true)
        assertFalse(pending.blocksWorkspaceReplacement(false, replacePendingTransition = true))
        assertTrue(pending.blocksWorkspaceReplacement(false, replacePendingTransition = false))
        assertTrue(pending.copy(activeBackgroundTaskIds = setOf("task"))
            .blocksWorkspaceReplacement(false, replacePendingTransition = true))
        assertTrue(pending.blocksWorkspaceReplacement(true, replacePendingTransition = true))
        assertTrue(pending.copy(cancellationInFlight = true)
            .blocksWorkspaceReplacement(false, replacePendingTransition = true))
    }

    @Test fun taskWorkflowAndAgentStatusesBlockWorkspaceReplacementWithoutStreaming() {
        val states = listOf(
            idle.copy(backgroundTasks = mapOf("task" to BackgroundTaskUi(
                "task", "agent", "Executing", TaskStatusDto.PENDING, false, null))),
            idle.copy(workflowRuns = mapOf("run" to WorkflowRunUi("s", "task", "run"))),
            idle.copy(sessionAgents = listOf(SessionAgentUi("agent", "Agent", "worker", null, "working", null))),
        )
        states.forEach { assertTrue(it.blocksWorkspaceReplacement(false, replacePendingTransition = true)) }
        assertFalse(idle.copy(sessionAgents = listOf(SessionAgentUi("agent", "Agent", "worker", null, "completed", null)))
            .blocksWorkspaceReplacement(false, replacePendingTransition = true))
        assertFalse(idle.copy(workflowRuns = mapOf("run" to WorkflowRunUi("s", "task", "run", TaskStatusDto.COMPLETED)))
            .blocksWorkspaceReplacement(false, replacePendingTransition = true))
    }
}
