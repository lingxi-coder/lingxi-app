package com.lingxi.code.settings

import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class DesktopSettingsContractTest {
    @Test fun pendingSettingsCompareValuesRatherThanJsonOrdering() {
        org.junit.Assert.assertFalse(settingsDifferFromActive("{\"a\":1,\"b\":2}", "{\"b\":2,\"a\":1}"))
        org.junit.Assert.assertTrue(settingsDifferFromActive("{\"outputStyle\":\"concise\"}", "{\"outputStyle\":\"detailed\"}"))
        org.junit.Assert.assertFalse(settingsDifferFromActive(null, "{}"))
        org.junit.Assert.assertFalse(settingsDifferFromActive("broken", "{}"))
    }

    private fun snapshot(layers: String = "{\"user\":{},\"project\":{},\"local\":{},\"managed\":{}}", locked: List<String> = emptyList(), files: String = "[]") =
        ClientEvent.SettingsSnapshot("{}", "{}", files, null, locked, layers, emptyList())

    @Test fun refreshPreservesDirtyDraftButAdoptsConfirmedValue() {
        val draft = SettingsValueDraft("{\"enabled\":false}")
        draft.edit("{\"enabled\":true}")
        draft.observe("{\"enabled\":false,\"other\":1}")
        assertEquals("{\"enabled\":true}",draft.value)
        assertTrue(draft.externalChange)
        draft.observe("{\"enabled\":true}")
        assertFalse(draft.dirty)
        assertFalse(draft.externalChange)
        draft.observe("null")
        assertEquals("null",draft.value)
    }
    @Test fun permissionsUseDedicatedDeltaCommands() {
        val commands = permissionSettingsCommands("project",JSONObject("{\"allow\":[\"Read\",\"Write\"],\"defaultMode\":\"default\"}"),
            JSONObject("{\"allow\":[\"Read\",\"Bash(ls)\"],\"defaultMode\":\"plan\",\"additionalDirectories\":[\"/tmp/work\"]}"))
        assertFalse(commands.any { it is ClientCommand.UpdateSettings })
        val rules = commands.filterIsInstance<ClientCommand.UpdatePermissionRules>().single()
        assertEquals(listOf("Bash(ls)"),rules.add)
        assertEquals(listOf("Write"),rules.remove)
        assertEquals(com.lingxi.code.bindings.SettingsDestinationDto.PROJECT,rules.destination)
        assertEquals("plan",commands.filterIsInstance<ClientCommand.SetDefaultPermissionMode>().single().mode)
        assertEquals(listOf("/tmp/work"),commands.filterIsInstance<ClientCommand.UpdateWorkspaceDirectories>().single().add)
    }
    @Test fun advancedProvidersPreserveMetadataButRefuseInlineSecrets() {
        val definition=JSONObject("{\"custom\":{\"type\":\"openai\",\"baseUrl\":\"https://api.example.test/v1\",\"models\":[{\"id\":\"model\",\"metadata\":{\"contextWindowTokens\":32000}}]}}")
        validateProviderDefinitions(definition)
        definition.getJSONObject("custom").put("headers",JSONObject().put("Authorization","fixture"))
        assertThrows(IllegalArgumentException::class.java) { validateProviderDefinitions(definition) }
    }
    @Test fun desktopGroupsAndSearchRemainReachable() {
        assertEquals(listOf("Personal", "Models & services", "Coding", "Advanced"), desktopSettingsEntries.map { it.group }.distinct())
        assertEquals(16, desktopSettingsEntries.size)
        assertFalse(desktopSettingsEntries.any { it.route == SettingsRoutes.FUSION })
        assertEquals(SettingsRoutes.CUSTOM_PROVIDERS, searchDesktopSettings(" routing ").single().route)
        assertTrue(searchDesktopSettings("nonexistent-setting").isEmpty())
        assertEquals(desktopSettingsEntries, searchDesktopSettings(""))
    }
    @Test fun managedLockedAndBrokenLayersCannotWrite() {
        val patch = JSONObject("{\"hooks\":{}}")
        assertThrows(IllegalArgumentException::class.java) { validateSettingsPatch(snapshot(),"managed",patch) }
        assertThrows(IllegalArgumentException::class.java) { validateSettingsPatch(snapshot(locked=listOf("hooks")),"user",patch) }
        assertThrows(IllegalArgumentException::class.java) { validateSettingsPatch(snapshot(files="[{\"layer\":\"project\",\"parsed\":false}]"),"project",patch) }
        assertThrows(IllegalArgumentException::class.java) { validateSettingsPatch(snapshot(layers="{}"),"user",patch) }
        validateSettingsPatch(snapshot(),"user",patch)
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun mcpScopeAndOperationAcknowledgementAreIndependentFromSettingsLayers() = runTest {
        val commands = mutableListOf<ClientCommand>()
        val source = object : ConversationSource {
            override val clientEvents = kotlinx.coroutines.flow.MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command: ClientCommand) { commands += command }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        bridge.admin("mcp","save_server",scope="project",revision="original",payload="{}")
        val command = commands.last() as ClientCommand.McpAdmin
        assertEquals("project",command.command.scope)
        assertEquals("original",command.command.revision)
        val id = bridge.state.value.pending!!
        bridge.accept(ClientEvent.ConfigurationOperation(com.lingxi.code.bindings.ConfigurationDomainDto.MCP,id,
            com.lingxi.code.bindings.ConfigurationOperationStatusDto.STARTED,com.lingxi.code.bindings.ConfigurationEffectDto.NOT_APPLICABLE,null,null))
        assertEquals(id,bridge.state.value.pending)
        bridge.accept(ClientEvent.ConfigurationOperation(com.lingxi.code.bindings.ConfigurationDomainDto.MCP,id,
            com.lingxi.code.bindings.ConfigurationOperationStatusDto.SUCCEEDED,com.lingxi.code.bindings.ConfigurationEffectDto.APPLIED,"Saved",null))
        assertNull(bridge.state.value.pending)
        assertTrue(bridge.state.value.notice!!.contains("SUCCEEDED"))
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun inFlightSaveCannotRefreshOrMutateReplacementSource() = runTest {
        val bridge = SettingsEngineBridge()
        val gate = CompletableDeferred<Unit>()
        val old = object : ConversationSource {
            override val clientEvents = kotlinx.coroutines.flow.MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command: ClientCommand) { if (command is ClientCommand.UpdateSettings) gate.await() }
        }
        val commands = mutableListOf<ClientCommand>()
        val next = object : ConversationSource {
            override val clientEvents = kotlinx.coroutines.flow.MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command: ClientCommand) { commands += command } }
        val oldBinding = backgroundScope.launch { bridge.bind(old) }; runCurrent()
        assertTrue(bridge.state.value.connected)
        bridge.accept(snapshot())
        val write = launch { runCatching { bridge.update("user",JSONObject("{\"enabledTools\":[\"Read\"]}")) } }; runCurrent()
        assertEquals("Waiting for engine confirmation…",bridge.state.value.notice)
        oldBinding.cancel(); runCurrent()
        backgroundScope.launch { bridge.bind(next) }; runCurrent()
        val before = bridge.state.value
        val commandCount = commands.size
        gate.complete(Unit); write.join()
        assertEquals(commandCount, commands.size)
        assertEquals(before,bridge.state.value)
    }
}
