package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
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

    private fun clientText(text: String) =
        ClientTextDto(codeUnits = text.map { it.code.toUShort() })

    private fun catalogCommand(
        argumentNames: List<ClientTextDto> = listOf(clientText("target"), clientText("version")),
    ) = SlashCommandDto(
        name = clientText("deploy"), description = clientText("Deploy"), source = "markdown",
        aliases = listOf(clientText("ship")), argumentHint = clientText("[target] [version]"),
        argumentNames = argumentNames, menuDescription = null, hidden = false,
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
            listOf(
                ConversationSlashCommand(
                    name = "deploy",
                    aliases = listOf("ship"),
                    argumentHint = "[target] [version]",
                    argumentNames = listOf("target", "version"),
                ),
            ),
            model.state.value.slashCommands,
        )
        model.reduceClientEvent(ClientEvent.SlashCommandCatalog(emptyList()))
        assertTrue(model.state.value.slashCommands.isEmpty())
    }

    @Test fun convertsCatalogTextOnlyAtTheDisplayBoundary() = runTest(dispatcher) {
        val malformedArgument = ClientTextDto(
            codeUnits = listOf('t'.code.toUShort(), 0xD800.toUShort()),
        )
        val command = catalogCommand(argumentNames = listOf(malformedArgument))
        assertEquals(0xD800.toUShort(), command.argumentNames.single().codeUnits.last())

        val source = object : ConversationSource, BackgroundRetainableConversationSource {
            override fun engineSourceForBackgroundRetention(): EngineConversationSource? = null
        }
        val model = ChatViewModel(source)
        model.reduceClientEvent(ClientEvent.SlashCommandCatalog(listOf(command)))

        val displayed = model.state.value.slashCommands.single()
        assertEquals("deploy", displayed.name)
        assertEquals(listOf("ship"), displayed.aliases)
        assertEquals("[target] [version]", displayed.argumentHint)
        assertEquals(listOf("t\uFFFD"), displayed.argumentNames)
    }
}
