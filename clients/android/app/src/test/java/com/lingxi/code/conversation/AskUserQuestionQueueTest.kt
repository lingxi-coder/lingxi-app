package com.lingxi.code.conversation

import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.SheetValue
import com.lingxi.code.bindings.AskOptionDto
import com.lingxi.code.bindings.AskQuestionDto
import com.lingxi.code.bindings.AskUserQuestionRequestDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.TaskRowDto
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pending-question queue's exact lifecycle — [ChatViewModel.reduceClientEvent]
 * plus the answer/cancel commands — and the pure answer-assembly helpers the
 * card uses. All JVM-only: the binding DTOs are plain Kotlin data classes.
 */
@OptIn(ExperimentalCoroutinesApi::class, ExperimentalMaterial3Api::class)
class AskUserQuestionQueueTest {

    @Test
    fun `pending question sheet blocks implicit hide transitions`() {
        assertFalse(pendingQuestionSheetAllowsTransition(SheetValue.Hidden))
        assertTrue(pendingQuestionSheetAllowsTransition(SheetValue.PartiallyExpanded))
        assertTrue(pendingQuestionSheetAllowsTransition(SheetValue.Expanded))
    }

    private class RecordingSource : ConversationSource {
        val commands = mutableListOf<ClientCommand>()

        override suspend fun submitClientCommand(command: ClientCommand) {
            commands += command
        }

        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
    }

    @Test
    fun `a question enqueues once resolves away and survives everything but session end`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val viewModel = ChatViewModel(source = RecordingSource())
            runCurrent()

            viewModel.reduceClientEvent(ClientEvent.AskUserQuestion(request(1u)))
            viewModel.reduceClientEvent(ClientEvent.AskUserQuestion(request(2u)))
            assertEquals(listOf(1uL, 2uL), viewModel.state.value.pendingQuestions.map { it.requestId })

            // Dedupe: an engine re-emit of a still-parked request is a no-op.
            viewModel.reduceClientEvent(ClientEvent.AskUserQuestion(request(1u)))
            assertEquals(2, viewModel.state.value.pendingQuestions.size)

            // A question outlives its turn's stream: TurnEnded arrives on the
            // per-turn path and must NOT clear the queue (only SessionEnded /
            // Resolved may). The out-of-band reducer ignores it entirely.
            viewModel.reduceClientEvent(
                ClientEvent.TaskStatusChanged(
                    taskId = "abc123def",
                    status = TaskStatusDto.RUNNING,
                    originSessionId = null,
                    error = null,
                ),
            )
            assertEquals(2, viewModel.state.value.pendingQuestions.size)

            // Resolved drops exactly the named id.
            viewModel.reduceClientEvent(ClientEvent.AskUserQuestionResolved(1u))
            assertEquals(listOf(2uL), viewModel.state.value.pendingQuestions.map { it.requestId })

            // SessionEnded clears whatever is left.
            viewModel.reduceClientEvent(ClientEvent.SessionEnded)
            assertTrue(viewModel.state.value.pendingQuestions.isEmpty())
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `answering submits the command and drops the card immediately`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = ChatViewModel(source = source)
            runCurrent()

            viewModel.reduceClientEvent(ClientEvent.AskUserQuestion(request(7u)))
            viewModel.answerQuestion(7u, mapOf("想要什么布局？" to "列表"))
            runCurrent()

            assertTrue(viewModel.state.value.pendingQuestions.isEmpty())
            val sent = source.commands.filterIsInstance<ClientCommand.AnswerAskUserQuestion>().single()
            assertEquals(7uL, sent.requestId)
            assertEquals(mapOf("想要什么布局？" to "列表"), sent.answers)

            // The engine's own Resolved for the same id is then a no-op.
            viewModel.reduceClientEvent(ClientEvent.AskUserQuestionResolved(7u))
            assertTrue(viewModel.state.value.pendingQuestions.isEmpty())
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `cancelling submits the cancel command and drops the card`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val source = RecordingSource()
            val viewModel = ChatViewModel(source = source)
            runCurrent()

            viewModel.reduceClientEvent(ClientEvent.AskUserQuestion(request(9u)))
            viewModel.cancelQuestion(9u)
            runCurrent()

            assertTrue(viewModel.state.value.pendingQuestions.isEmpty())
            val sent = source.commands.filterIsInstance<ClientCommand.CancelAskUserQuestion>().single()
            assertEquals(9uL, sent.requestId)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun `a task transition surfaces as the transient status line`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val viewModel = ChatViewModel(source = RecordingSource())
            runCurrent()

            viewModel.reduceClientEvent(
                ClientEvent.TaskStatusChanged(
                    taskId = "abc123def",
                    status = TaskStatusDto.COMPLETED,
                    originSessionId = null,
                    error = null,
                ),
            )
            assertEquals("后台任务 abc123def 已完成", viewModel.state.value.statusLine)

            viewModel.reduceClientEvent(
                ClientEvent.TaskStatusChanged(
                    taskId = "abc123def",
                    status = TaskStatusDto.FAILED,
                    originSessionId = null,
                    error = null,
                ),
            )
            assertEquals("后台任务 abc123def 已失败", viewModel.state.value.statusLine)
        } finally {
            Dispatchers.resetMain()
        }
    }

    /**
     * r3-failure-paths-07 — a failed task must reach the user as
     * "<what it was> failed: <why>", not as a bare 9-char id.
     */
    @Test
    fun `a failed task names the task and the reason`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val viewModel = ChatViewModel(source = RecordingSource())
            runCurrent()

            viewModel.reduceClientEvent(
                ClientEvent.TaskRow(
                    TaskRowDto(
                        "abc123def",
                        "local_workflow",
                        TaskStatusDto.RUNNING,
                        "创建应用「食谱盒」",
                        false,
                        null,
                        null,
                    ),
                ),
            )
            // Vacuity guard: the row must actually be in the map, otherwise the
            // assertion below would be satisfied by the bare-id fallback.
            assertEquals(
                "创建应用「食谱盒」",
                viewModel.state.value.backgroundTasks["abc123def"]?.description,
            )

            viewModel.reduceClientEvent(
                ClientEvent.TaskStatusChanged(
                    taskId = "abc123def",
                    status = TaskStatusDto.FAILED,
                    originSessionId = null,
                    error = "第 2 步 `build` 退出码 1",
                ),
            )
            assertEquals(
                "后台任务 创建应用「食谱盒」 已失败：第 2 步 `build` 退出码 1",
                viewModel.state.value.statusLine,
            )
            assertEquals(
                "第 2 步 `build` 退出码 1",
                viewModel.state.value.backgroundTasks["abc123def"]?.error,
            )
        } finally {
            Dispatchers.resetMain()
        }
    }

    // ---- pure answer-assembly helpers -------------------------------------

    @Test
    fun `single select replaces and re-tapping clears while multi select toggles`() {
        assertEquals(listOf("列表"), toggleAskSelection(emptyList(), "列表", multiSelect = false))
        assertEquals(listOf("卡片"), toggleAskSelection(listOf("列表"), "卡片", multiSelect = false))
        assertEquals(emptyList<String>(), toggleAskSelection(listOf("列表"), "列表", multiSelect = false))

        assertEquals(listOf("列表", "卡片"), toggleAskSelection(listOf("列表"), "卡片", multiSelect = true))
        assertEquals(listOf("列表"), toggleAskSelection(listOf("列表", "卡片"), "卡片", multiSelect = true))
    }

    @Test
    fun `answers map question text to comma-joined labels or the free text`() {
        val request = request(
            id = 3u,
            questions = listOf(
                question("想要什么布局？", options = listOf("列表", "卡片"), multiSelect = true),
                question("应用叫什么名字？", options = emptyList()),
                question("要深色模式吗？", options = listOf("要", "不要")),
            ),
        )
        val answers = assembleAskAnswers(
            request = request,
            selections = mapOf(0 to listOf("列表", "卡片")),
            freeTexts = mapOf(1 to " 喝水打卡 "),
        )
        assertEquals(
            mapOf(
                "想要什么布局？" to "列表, 卡片",
                "应用叫什么名字？" to "喝水打卡",
            ),
            answers,
        )
        // The unanswered third question is omitted, so the set is incomplete.
        assertFalse(askAnswersComplete(request, mapOf(0 to listOf("列表")), mapOf(1 to "喝水打卡")))
        assertTrue(
            askAnswersComplete(
                request,
                mapOf(0 to listOf("列表"), 2 to listOf("要")),
                mapOf(1 to "喝水打卡"),
            ),
        )
    }

    @Test
    fun `a selection plus free text joins both into one answer`() {
        val request = request(3u, listOf(question("想要什么布局？", options = listOf("列表"))))
        assertEquals(
            mapOf("想要什么布局？" to "列表, 也要日历视图"),
            assembleAskAnswers(request, mapOf(0 to listOf("列表")), mapOf(0 to "也要日历视图")),
        )
    }

    @Test
    fun `agent run and first question stay outside transcript for pinned and sheet presentation`() {
        val user = Message(role = Role.User, text = "做个记账应用")
        val live = Message(role = Role.Ai, text = "先问几个问题")
        val state = ChatState(
            session = com.lingxi.code.model.SessionRef("s", "标题"),
            messages = listOf(user),
            streamingMessage = live,
            streaming = true,
            model = com.lingxi.code.model.EngineModelCatalog.pending,
            agentRun = AgentRunState(turnId = 1L),
            pendingQuestions = listOf(request(5u), request(6u)),
        )
        val items = buildChatRenderItems(state)
        assertEquals(
            listOf(user.id, live.id),
            items.map { it.key },
        )
        assertEquals(1L, state.agentRun?.turnId)
        assertEquals(5uL, pendingQuestionForSheet(state)?.requestId)
        assertTrue(
            "only the FIRST pending request is selected for the sheet",
            state.pendingQuestions.drop(1).none { it.requestId == pendingQuestionForSheet(state)?.requestId },
        )
    }

    private fun request(
        id: ULong,
        questions: List<AskQuestionDto> = listOf(question("想要什么布局？", options = listOf("列表", "卡片"))),
    ) = AskUserQuestionRequestDto(
        requestId = id,
        questions = questions,
        timeoutSecs = null,
    )

    private fun question(
        text: String,
        options: List<String>,
        multiSelect: Boolean = false,
    ) = AskQuestionDto(
        question = text,
        header = "布局",
        options = options.map { AskOptionDto(label = it, description = "$it 的说明", preview = null) },
        multiSelect = multiSelect,
    )
}
