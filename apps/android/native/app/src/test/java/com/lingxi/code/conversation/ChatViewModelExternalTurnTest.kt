package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.CostDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.TurnOutcomeDto
import com.lingxi.code.model.Role
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
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

/**
 * Drives the real [ChatViewModel] with a followed (external) source: turns the
 * phone did not start must render like its own, and a local source must be
 * untouched by the external path.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ChatViewModelExternalTurnTest {
    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    private class FakeExternalSource : ConversationSource, ExternalTurnConversationSource {
        val feed = MutableSharedFlow<ExternalFeedItem>(extraBufferCapacity = 64)
        val sent = mutableListOf<Pair<String, List<ImageRefDto>>>()
        val dismissed = mutableListOf<String>()
        val retried = mutableListOf<String>()
        var cancelled = 0
        override val externalFeed: Flow<ExternalFeedItem> = feed.asSharedFlow()
        override fun submit(text: String): Flow<ReplyEvent> = error("an external source never submits a turn")
        override suspend fun sendExternalPrompt(text: String, images: List<ImageRefDto>) { sent += text to images }
        override suspend fun retryExternalPrompt(promptId: String) { retried += promptId }
        override suspend fun dismissExternalPrompt(promptId: String) { dismissed += promptId }
        override suspend fun cancel() { cancelled++ }
    }

    private fun base(vararg texts: String) = ExternalFeedItem.Base(
        ActivatedSession(
            sessionId = "s1",
            transcript = texts.mapIndexed { index, text ->
                com.lingxi.code.model.Message(role = if (index % 2 == 0) Role.User else Role.Ai, text = text)
            },
            kind = SessionActivationKind.Resumed,
        ),
        title = null,
    )

    private val cost = CostDto(0.0, 0u, 0u, 0u, 0u, "$0.00")

    @Test fun baseReplacesEverythingHeld() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("one", "two"))
        source.feed.emit(ExternalFeedItem.Readiness(true, null))
        assertEquals(listOf("one", "two"), vm.state.value.messages.map { it.text })
        assertEquals("s1", vm.state.value.session.id)
        assertTrue(vm.state.value.sessionReady)
        source.feed.emit(base("fresh"))
        assertEquals(listOf("fresh"), vm.state.value.messages.map { it.text })
    }

    @Test fun olderMessagesGoInFrontOfTheHeldOnes() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("held"))
        source.feed.emit(ExternalFeedItem.OlderMessages(listOf(com.lingxi.code.model.Message(role = Role.User, text = "older"))))
        assertEquals(listOf("older", "held"), vm.state.value.messages.map { it.text })
    }

    @Test fun aTurnTheDesktopStartedStreamsAndEnds() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.UserPrompt(5L, "from the desk", 0, "来自桌面"))
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.TurnStarted(5uL)))
        assertTrue(vm.state.value.streaming)
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.TextDelta("answer".toClientTextDto())))
        assertEquals("answer", vm.state.value.streamingMessage?.text)
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost)))
        assertFalse(vm.state.value.streaming)
        val user = vm.state.value.messages.first { it.text == "from the desk" }
        assertEquals(Role.User, user.role)
        assertEquals("来自桌面", user.originLabel)
        assertEquals("answer", vm.state.value.messages.last().text)
    }

    @Test fun aSnapshotTakenMidTurnStartsAndTheMetaEndsIt() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Turn(running = true))
        assertTrue(vm.state.value.streaming)
        source.feed.emit(ExternalFeedItem.Turn(running = false))
        assertFalse(vm.state.value.streaming)
    }

    @Test fun outgoingRowsAppearThenGoWhenTheDesktopEchoes() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Outgoing(listOf(ExternalOutgoingPrompt("a", "mine", emptyList(), failed = false))))
        assertEquals("mine", vm.state.value.messages.last().text)
        // The desktop's own record replaces the provisional row.
        source.feed.emit(ExternalFeedItem.Outgoing(emptyList()))
        source.feed.emit(ExternalFeedItem.UserPrompt(6L, "mine", 0, "来自此设备"))
        assertEquals(listOf("old", "mine"), vm.state.value.messages.map { it.text })
        assertEquals("来自此设备", vm.state.value.messages.last().originLabel)
    }

    @Test fun aFailedPromptRestoresTheDraftAndRetriesWithTheOriginalId() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        val restored = mutableListOf<ExternalSendFailure>()
        val images = listOf(ImageRefDto("image/png", "AQID"))
        val collector = launch(start = CoroutineStart.UNDISPATCHED) { vm.externalSendFailures.collect { restored += it } }
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Outgoing(listOf(ExternalOutgoingPrompt("a", "lost", images, failed = false))))
        source.feed.emit(ExternalFeedItem.Outgoing(listOf(ExternalOutgoingPrompt("a", "lost", images, failed = true))))
        runCurrent()
        assertEquals(listOf("old"), vm.state.value.messages.map { it.text })
        assertEquals(listOf(ExternalSendFailure("lost", images)), restored)
        assertTrue(source.dismissed.isEmpty())
        assertTrue(vm.state.value.error != null)
        source.feed.emit(ExternalFeedItem.Readiness(true, null))
        vm.send("lost", images = images)
        runCurrent()
        assertEquals(listOf("a"), source.retried)
        assertTrue(source.sent.isEmpty())
        collector.cancel()
    }

    @Test fun pendingQuestionsAreTheCompleteSetOfEachUpdate() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(ExternalFeedItem.PendingQuestions(emptyList()))
        assertTrue(vm.state.value.pendingQuestions.isEmpty())
    }

    @Test fun readinessGatesTheComposerAndCarriesTheStatusLine() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Readiness(false, "offline"))
        assertFalse(vm.state.value.sessionReady)
        assertEquals("offline", vm.state.value.statusLine)
        vm.send("not while offline")
        runCurrent()
        assertTrue(source.sent.isEmpty())
        source.feed.emit(ExternalFeedItem.Readiness(true, null))
        vm.send("now")
        runCurrent()
        assertEquals(listOf("now"), source.sent.map { it.first })
    }

    @Test fun sendGoesToTheSourceAndNoLocalRowIsMade() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Readiness(true, null))
        vm.send("hello")
        runCurrent()
        assertEquals(listOf("hello"), source.sent.map { it.first })
        assertEquals(listOf("old"), vm.state.value.messages.map { it.text })
        assertFalse(vm.state.value.streaming)
    }

    @Test fun cancelStopsTheRemoteTurn() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(ExternalFeedItem.Turn(running = true))
        vm.cancel()
        runCurrent()
        assertEquals(1, source.cancelled)
    }

    @Test fun sideChatIsNotAvailableOnAFollowedSession() = runTest(dispatcher) {
        val vm = ChatViewModel(FakeExternalSource())
        assertFalse(vm.supportsSideChats)
    }

    @Test fun promptImagesAttachToTheirRow() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(ExternalFeedItem.UserPrompt(9L, "see", 1, null))
        assertNull(vm.state.value.messages.last().originLabel)
        val image = ImageRefDto("image/png", "AA==")
        source.feed.emit(ExternalFeedItem.PromptImages(9L, listOf(image)))
        assertEquals(listOf(image), vm.state.value.messages.last().images)
    }
    @Test fun remoteConfigurationOutputRendersWithoutOwningOrEndingATurn() = runTest(dispatcher) {
        val source = FakeExternalSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.feed.emit(base("old"))
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.SlashCommandResult(17uL, "model selected".toClientTextDto(), false)))
        assertEquals("model selected", vm.state.value.messages.last().text)
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.TurnStarted(19uL)))
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.TextDelta("working".toClientTextDto())))
        source.feed.emit(ExternalFeedItem.Event(ClientEvent.SlashCommandResult(20uL, "effort changed".toClientTextDto(), false)))
        assertTrue(vm.state.value.streaming)
        assertEquals("working", vm.state.value.streamingMessage?.text)
        assertEquals("effort changed", vm.state.value.messages.last().text)
    }

}
