package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.CostDto
import com.lingxi.code.bindings.client.SessionModeDto
import com.lingxi.code.bindings.client.TurnOutcomeDto
import com.lingxi.code.bindings.client.VisualizationBlockStatusDto
import com.lingxi.code.bindings.client.VisualizationRefDto
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** Exercises the real event fold and both ViewModel collectors, without native JNI. */
@OptIn(ExperimentalCoroutinesApi::class)
class SessionBootstrapTest {
    private val dispatcher = UnconfinedTestDispatcher()
    private val owner = "11111111-1111-4111-8111-111111111111"

    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    private class Source : ConversationSource {
        private val events = MutableSharedFlow<ClientEvent>()
        private val active = MutableStateFlow<ActivatedSession?>(null)
        override val clientEvents = events.asSharedFlow()
        override val activeSessionState = active.asStateFlow()
        override fun submit(text: String) = mapReplyStream(events, DefaultConversationStrings)

        suspend fun emit(event: ClientEvent) {
            // Same listener ordering as EngineConversationSource: retain the
            // owner first, then relay the event to live/recovery collectors.
            active.update { reduceActivatedSession(it, event, SessionMode.Code) }
            events.emit(event)
        }
    }

    @Test fun earlyOwnerListingIsRetainedUntilTheViewModelSubscribes() = runTest(dispatcher) {
        val source = Source()
        source.emit(ClientEvent.SessionAgentList(owner, emptyList()))
        val vm = ChatViewModel(source)
        runCurrent()
        assertEquals(owner, vm.state.value.session.id)
        assertTrue(vm.state.value.sessionReady)
        assertTrue(vm.state.value.messages.isEmpty())
    }

    @Test fun delayedOwnerListingPreservesTheLiveWidgetAndReplyCollector() = runTest(dispatcher) {
        val source = Source()
        val vm = ChatViewModel(source)
        vm.send("Draw a bicycle")
        runCurrent()
        source.emit(ClientEvent.TextDelta("Before ".toClientTextDto()))
        source.emit(ClientEvent.VisualizationBlock(VisualizationBlockStatusDto.READY, VisualizationRefDto("chart", 2u)))
        runCurrent()
        val before = vm.state.value
        assertEquals("new", before.session.id)
        assertTrue(before.streaming)

        source.emit(ClientEvent.SessionAgentList(owner, emptyList()))
        runCurrent()

        assertEquals(owner, vm.state.value.session.id)
        assertEquals(before.messages, vm.state.value.messages)
        assertEquals(before.streamingMessage, vm.state.value.streamingMessage)
        assertTrue(vm.state.value.streaming)
        source.emit(ClientEvent.TextDelta("after".toClientTextDto()))
        runCurrent()
        assertEquals("Before after", vm.state.value.streamingMessage?.text)
        assertEquals(VisualizationRef("chart", 2u),
            buildChatRenderItems(vm.state.value).filterIsInstance<ChatRenderItem.Visualization>().single().reference)
        source.emit(ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null,
            CostDto(0.0, 0u, 0u, 0u, 0u, "$0.00")))
        runCurrent()
        assertFalse(vm.state.value.streaming)
        assertEquals(owner, vm.state.value.session.id)
        assertEquals(VisualizationRef("chart", 2u),
            buildChatRenderItems(vm.state.value).filterIsInstance<ChatRenderItem.Visualization>().single().reference)
    }

    @Test fun bootstrapListingCannotConfirmAnExplicitNewSession() = runTest(dispatcher) {
        val source = Source()
        val vm = ChatViewModel(source)
        vm.startNewSession()
        runCurrent()
        source.emit(ClientEvent.SessionAgentList(owner, emptyList()))
        runCurrent()
        assertTrue(vm.state.value.sessionTransitioning)
        assertEquals("new", vm.state.value.session.id)

        source.emit(ClientEvent.SessionStarted("fresh", SessionModeDto.CODE))
        runCurrent()
        assertFalse(vm.state.value.sessionTransitioning)
        assertEquals("fresh", vm.state.value.session.id)
        source.emit(ClientEvent.SessionAgentList(owner, emptyList()))
        runCurrent()
        assertEquals("fresh", vm.state.value.session.id)
        assertEquals("fresh", source.activeSessionState.value?.sessionId)
    }

    @Test fun bootstrapListingCannotConfirmAResume() = runTest(dispatcher) {
        val source = Source()
        val vm = ChatViewModel(source)
        vm.openSession(SessionRef("saved", "Saved conversation"))
        runCurrent()
        source.emit(ClientEvent.SessionAgentList(owner, emptyList()))
        runCurrent()
        assertTrue(vm.state.value.sessionTransitioning)
        assertEquals("saved", vm.state.value.session.id)
        source.emit(ClientEvent.SessionResumed("saved", SessionModeDto.CODE, emptyList()))
        runCurrent()
        assertFalse(vm.state.value.sessionTransitioning)
        assertEquals("saved", vm.state.value.session.id)
    }

    @Test fun bootstrapUsesTheSourcesModeAndRejectsPlaceholderOwners() {
        val activation = reduceActivatedSession(null, ClientEvent.SessionAgentList(owner, emptyList()), SessionMode.Chat)
        assertEquals(SessionMode.Chat, activation?.mode)
        assertEquals(SessionActivationKind.Bootstrap, activation?.kind)
        for (id in listOf("", "new")) {
            assertNull(reduceActivatedSession(null, ClientEvent.SessionAgentList(id, emptyList()), SessionMode.Code))
        }
    }
}
