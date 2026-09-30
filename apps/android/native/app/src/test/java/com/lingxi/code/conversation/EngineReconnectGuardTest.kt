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
}
