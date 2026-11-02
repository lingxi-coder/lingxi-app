package com.lingxi.code.settings

import com.lingxi.code.bindings.client.*
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ProviderImportEngineTest {
    private val native = """{"providers":{"custom":{"type":"openai","baseUrl":"https://api.example.test/v1","models":["model"]}}}"""
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun configurationImportRequiresItsOwnSettingsAcknowledgement() = runTest {
        val bridge = SettingsEngineBridge()
        val commands = mutableListOf<ClientCommand>()
        var written: String? = null
        val source = object : ConversationSource {
            override val clientEvents = MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command: ClientCommand) {
                commands += command
                if (command is ClientCommand.UpdateSettings) written = command.patchJson
                if (command is ClientCommand.RefreshListings && written != null) bridge.accept(ClientEvent.SettingsSnapshot("{}", "{}", "[]", null, emptyList(), JSONObject().put("user", JSONObject(written!!)).toString(), emptyList()))
            }
        }
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        bridge.accept(ClientEvent.SettingsSnapshot("{}", "{}", "[]", null, emptyList(), "{\"user\":{}}", emptyList()))
        bridge.importProviders("user", JSONObject(), parseProviderImport(native, JSONObject()).entries)
        assertTrue(commands.any { it is ClientCommand.UpdateSettings })
        assertFalse(written!!.contains("apiKey"))
    }

}
