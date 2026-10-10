package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.VisualizationBlockStatusDto
import com.lingxi.code.bindings.client.VisualizationContextDto
import com.lingxi.code.bindings.client.VisualizationRefDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** Inline visualizations: live slots, replay, the retained decoder and follow-ups. */
@OptIn(ExperimentalCoroutinesApi::class)
class VisualizationTest {
    private val dispatcher = UnconfinedTestDispatcher()
    private val chart = VisualizationRef("chart", 2u)

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    private class StubSource : ConversationSource {
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
    }

    private class FollowupSource : ConversationSource {
        val contexts = mutableListOf<VisualizationRef?>()
        private val never = MutableSharedFlow<ReplyEvent>()
        override fun submit(text: String): Flow<ReplyEvent> = never.asSharedFlow()
        override fun submit(
            text: String,
            images: List<ImageRefDto>,
            turnId: Long,
            visualizationContext: VisualizationRef?,
        ): Flow<ReplyEvent> {
            contexts += visualizationContext
            return never.asSharedFlow()
        }
    }

    @Test fun aPendingSlotSettlesInPlaceAndProseContinuesAfterIt() {
        val vm = ChatViewModel(StubSource())
        vm.reduce(ReplyEvent.Thinking)
        vm.reduce(ReplyEvent.Delta("Here is the chart:"))
        vm.reduce(ReplyEvent.Visualization(VisualizationBlockStatus.Pending, null))
        vm.reduce(ReplyEvent.Visualization(VisualizationBlockStatus.Ready, chart))
        vm.reduce(ReplyEvent.Delta("Notice the dip."))

        val live = buildChatRenderItems(vm.state.value)
        val slot = live.filterIsInstance<ChatRenderItem.Visualization>().single()
        assertEquals(VisualizationSlotStatus.Ready, slot.status)
        assertEquals(chart, slot.reference)
        assertEquals(
            listOf("Here is the chart:", "Notice the dip."),
            live.filterIsInstance<ChatRenderItem.Streaming>().map { it.message.text },
        )

        vm.reduce(ReplyEvent.End)
        val settled = buildChatRenderItems(vm.state.value)
        // The same keys before and after settling: the WebView is not remounted.
        assertEquals(live.map { it.key }, settled.filter { it !is ChatRenderItem.AgentRun }.map { it.key })
        assertEquals("Here is the chart:Notice the dip.", vm.state.value.messages.last().text)
    }

    @Test fun discardedOrAbandonedPlaceholdersLeaveNoRow() {
        val vm = ChatViewModel(StubSource())
        vm.reduce(ReplyEvent.Thinking)
        vm.reduce(ReplyEvent.Visualization(VisualizationBlockStatus.Pending, null))
        vm.reduce(ReplyEvent.Visualization(VisualizationBlockStatus.Discarded, null))
        assertTrue(vm.state.value.streamingMessage?.blocks.orEmpty().isEmpty())

        vm.reduce(ReplyEvent.Delta("text"))
        vm.reduce(ReplyEvent.Visualization(VisualizationBlockStatus.Pending, null))
        vm.reduce(ReplyEvent.End)
        assertTrue(
            vm.state.value.messages.flatMap { it.blocks }.none { it is MessageContent.Visualization },
        )
        assertTrue(buildChatRenderItems(vm.state.value).none { it is ChatRenderItem.Visualization })
    }

    @Test fun aReadySlotWithoutAReferenceRendersUnavailable() {
        val message = Message(role = Role.Ai, text = "")
            .applyingVisualizationBlock(VisualizationBlockStatus.Pending, null)
            .applyingVisualizationBlock(VisualizationBlockStatus.Ready, null)
        assertEquals(
            listOf(MessageContent.Visualization(VisualizationSlotStatus.Unavailable, null)),
            message.blocks,
        )
    }

    @Test fun replayRestoresWidgetsAndTheUserChip() {
        val messages = transcriptFromDtos(
            listOf(
                MessageDto(
                    role = "assistant",
                    blocks = listOf(
                        MessageBlockDto.Text("Chart:"),
                        MessageBlockDto.Visualization(VisualizationRefDto("chart", 1u)),
                        MessageBlockDto.Visualization(null),
                    ),
                ),
                MessageDto(
                    role = "user",
                    blocks = listOf(MessageBlockDto.Text("Why the dip?")),
                    visualizationContext = VisualizationContextDto("chart", 1u, "Sales"),
                ),
            ),
        )
        assertEquals(
            listOf(
                MessageContent.Text("Chart:"),
                MessageContent.Visualization(VisualizationSlotStatus.Ready, VisualizationRef("chart", 1u)),
                MessageContent.Visualization(VisualizationSlotStatus.Unavailable, null),
            ),
            messages[0].blocks,
        )
        assertEquals(VisualizationContextChip("chart", 1u, "Sales"), messages[1].visualizationContext)
    }

    @Test fun aMessageWithOnlyAWidgetStillRenders() {
        val messages = transcriptFromDtos(
            listOf(
                MessageDto(
                    role = "assistant",
                    blocks = listOf(MessageBlockDto.Visualization(VisualizationRefDto("chart", 1u))),
                ),
            ),
        )
        assertEquals(1, messages.size)
    }

    @Test fun liveAndRetainedEventsMapToTheSameReply() {
        assertEquals(
            ReplyEvent.Visualization(VisualizationBlockStatus.Ready, chart),
            clientEventToReply(
                ClientEvent.VisualizationBlock(VisualizationBlockStatusDto.READY, VisualizationRefDto("chart", 2u)),
            ),
        )
        assertEquals(
            ReplyEvent.Visualization(VisualizationBlockStatus.Ready, chart),
            retainedTurnEventToReply(
                """{"type":"visualization_block","status":"ready","reference":{"id":"chart","revision":2}}""",
            ),
        )
        assertEquals(
            ReplyEvent.Visualization(VisualizationBlockStatus.Ready, null),
            retainedTurnEventToReply(
                """{"type":"visualization_block","status":"ready","reference":{"id":"chart","revision":0}}""",
            ),
        )
        assertNull(retainedTurnEventToReply("""{"type":"visualization_block","status":"bogus"}"""))
    }

    @Test fun anAcceptedFollowupRidesTheNextPromptOnce() = runTest(dispatcher) {
        val source = FollowupSource()
        val vm = ChatViewModel(source)
        val followup = VisualizationFollowup("Why the dip?", VisualizationContextChip("chart", 2u, "Sales"))

        vm.offerVisualizationFollowup(followup)
        assertEquals(followup, vm.state.value.visualizationFollowup)
        vm.acceptVisualizationFollowup(followup)
        assertNull(vm.state.value.visualizationFollowup)
        assertEquals(followup.chip, vm.state.value.visualizationChip)

        vm.send("Why the dip?")
        runCurrent()
        assertEquals(listOf<VisualizationRef?>(chart), source.contexts)
        assertEquals(followup.chip, vm.state.value.messages.last().visualizationContext)
        assertNull(vm.state.value.visualizationChip)
    }

    @Test fun dismissingTheChipSendsAPlainPrompt() = runTest(dispatcher) {
        val source = FollowupSource()
        val vm = ChatViewModel(source)
        val followup = VisualizationFollowup("x", VisualizationContextChip("chart", 2u, "Sales"))
        vm.offerVisualizationFollowup(followup)
        vm.acceptVisualizationFollowup(followup)
        vm.clearVisualizationChip()

        vm.send("plain")
        runCurrent()
        assertEquals(listOf<VisualizationRef?>(null), source.contexts)
        assertNull(vm.state.value.messages.last().visualizationContext)
    }
}
