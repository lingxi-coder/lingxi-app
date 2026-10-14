package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ListingKindDto
import com.lingxi.code.bindings.client.SlashCommandDto
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SlashCommandCatalogTest {
    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    private fun catalogCommand() = SlashCommandDto(
        name = "deploy", description = "Deploy", source = "markdown",
        aliases = listOf("ship"), argumentHint = "[target] [version]",
        argumentNames = listOf("target", "version"), menuDescription = null, hidden = false,
    )

    @Test fun subscribesBeforeRequestAndRetainsAllArgumentMetadata() = runTest(dispatcher) {
        val events = MutableSharedFlow<ClientEvent>()
        var subscribed = false
        var requested = false
        val source = object : ConversationSource {
            override val clientEvents = events.onSubscription { subscribed = true }
            override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
            override suspend fun submitClientCommand(command: ClientCommand) {
                if (command is ClientCommand.RefreshListings && ListingKindDto.SLASH_COMMANDS in command.which) {
                    assertTrue(subscribed)
                    requested = true
                    events.emit(ClientEvent.SlashCommandCatalog(listOf(catalogCommand())))
                }
            }
        }
        val model = ChatViewModel(source)
        assertTrue(requested)
        assertEquals(
            listOf(ConversationSlashCommand("deploy", listOf("ship"), "[target] [version]", listOf("target", "version"))),
            model.state.value.slashCommands,
        )
        model.reduceClientEvent(ClientEvent.SlashCommandCatalog(emptyList()))
        assertTrue(model.state.value.slashCommands.isEmpty())
    }
}
