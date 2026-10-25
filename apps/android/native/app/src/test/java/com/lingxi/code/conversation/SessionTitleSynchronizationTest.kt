package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.SessionRow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.Assert.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class SessionTitleSynchronizationTest {
    @Before fun setup() = Dispatchers.setMain(UnconfinedTestDispatcher())
    @After fun teardown() = Dispatchers.resetMain()
    @Test fun catalogArrivingBeforeBootstrapStillNamesTheHeader() {
        val rows = MutableStateFlow(EngineSessionState.ready(listOf(
            SessionRow(uuid = "bootstrap", title = "Existing session", messageCount = 1, relativeTime = ""))))
        val source = object : ConversationSource {
            override val sessionState = rows
            override fun submit(text: String) = emptyFlow<ReplyEvent>()
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("bootstrap", emptyList(), SessionActivationKind.Bootstrap))
        assertEquals("Existing session", vm.state.value.session.title)
    }
    @Test fun catalogRenameUpdatesActiveHeaderWithoutApplyingAnotherSessionTitle() {
        val rows = MutableStateFlow(EngineSessionState.loading())
        val source = object : ConversationSource {
            override val sessionState = rows
            override fun submit(text: String) = emptyFlow<ReplyEvent>()
        }
        val vm = ChatViewModel(source, SavedStateHandle(mapOf("chat.session.id" to "active", "chat.session.title" to "Old")))
        fun row(id: String, title: String) = SessionRow(uuid = id, title = title, messageCount = 1, relativeTime = "")
        rows.value = EngineSessionState.ready(listOf(row("active", "Custom session name"), row("other", "Other")))
        assertEquals("Custom session name", vm.state.value.session.title)
        rows.value = EngineSessionState.ready(listOf(row("other", "Changed elsewhere")))
        assertEquals("Custom session name", vm.state.value.session.title)
        rows.value = EngineSessionState.ready(listOf(row("active", "Updated name")))
        assertEquals("Updated name", vm.state.value.session.title)
    }
}
