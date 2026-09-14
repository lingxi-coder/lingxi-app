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

    // 项目层写到哪个目录，判据只能落在引擎回传的 files_json 上：project_dir 由引擎
    // 进程启动时的 cwd 定死，客户端的「当前项目」状态可能已经指向别处。
    @Test fun projectDirectoryComesFromTheEngineReportedProjectLayerPath() {
        val files = "[" +
            "{\"layer\":\"user\",\"path\":\"/home/me/.lingxi/settings.json\"}," +
            "{\"layer\":\"project\",\"path\":\"/home/me/work/engine-answer/.lingxi/settings.json\"}," +
            "{\"layer\":\"local\",\"path\":\"/home/me/work/engine-answer/.lingxi/settings.local.json\"}]"
        assertEquals("/home/me/work/engine-answer", projectDirectoryFromFiles(files))
        assertEquals("engine-answer", projectDisplayName("/home/me/work/engine-answer"))
    }

    // 后缀对不上时必须返回 null 而不是猜一个目录出来 —— 指着 B 写 A 比不显示更糟。
    @Test fun projectDirectoryRefusesToGuessWhenThePathDoesNotMatch() {
        assertNull(projectDirectoryFromFiles(null))
        assertNull(projectDirectoryFromFiles("[]"))
        assertNull(projectDirectoryFromFiles("not json"))
        assertNull(projectDirectoryFromFiles("[{\"layer\":\"user\",\"path\":\"/home/me/.lingxi/settings.json\"}]"))
        for (path in listOf("/work/proj/settings.json", "/work/proj/.claude/settings.json", "/work/proj/.lingxi/settings.local.json", "settings.json")) {
            assertNull("must not guess a project from $path", projectDirectoryFromFiles("[{\"layer\":\"project\",\"path\":\"$path\"}]"))
        }
    }

    private fun providers(json: String) = JSONObject(json)

    /** The historical single-connection shape must keep validating unchanged. */
    @Test
    fun flatProviderStillValidates() {
        validateProviderDefinitions(providers("""
            {"p":{"type":"openai","baseUrl":"https://x.example.com/v1","models":[{"id":"m"}]}}
        """.trimIndent()))
    }

    /**
     * A provider reachable several ways carries baseUrl/models on its
     * CONNECTIONS. Requiring them of the provider row rejected the whole config.
     */
    @Test
    fun multiConnectionProviderValidates() {
        validateProviderDefinitions(providers("""
            {"deepseek":{"type":"openai","models":[{"id":"deepseek-flash"}],
              "connections":[
                {"id":"intl","baseUrl":"https://api.deepseek.test"},
                {"id":"cn","baseUrl":"https://cn.deepseek.test/v1"}],
              "fallback":{"on":["rate_limit","auth"]}}}
        """.trimIndent()))
    }

    /** Each connection is checked as the flat provider it desugars to. */
    @Test
    fun aBrokenConnectionIsRejectedEvenWhenSiblingsAreValid() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","models":[{"id":"m"}],
                  "connections":[{"id":"good","baseUrl":"https://good.test/v1"},{"id":"bad"}]}}
            """.trimIndent()))
        }
    }

    /** An id with a reference separator would produce an unroutable model ref. */
    @Test
    fun connectionIdsRejectReferenceSeparators() {
        for (bad in listOf("a/b", "a:b", "a#b")) {
            assertThrows(IllegalArgumentException::class.java) {
                validateProviderDefinitions(providers("""
                    {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],
                      "connections":[{"id":"$bad"}]}}
                """.trimIndent()))
            }
        }
    }

    /** Two rows with one id would desugar to two profiles sharing a name. */
    @Test
    fun duplicateConnectionIdsAreRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],
                  "connections":[{"id":"a"},{"id":"a"}]}}
            """.trimIndent()))
        }
    }

    /** An empty list must be rejected rather than read as "no connections". */
    @Test
    fun emptyConnectionsArrayIsRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"connections":[]}}
            """.trimIndent()))
        }
    }

    /** `credentialIds` names stored credentials: distinct and non-blank. */
    @Test
    fun credentialIdsMustBeDistinctAndNonBlank() {
        validateProviderDefinitions(providers("""
            {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"credentialIds":["k1","k2"]}}
        """.trimIndent()))
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"credentialIds":["k1","k1"]}}
            """.trimIndent()))
        }
    }

    /** A typo in a trigger silently disables failover, so it must be named. */
    @Test
    fun unknownFallbackTriggerIsRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],
                  "connections":[{"id":"a"}],"fallback":{"on":["rate_limits"]}}}
            """.trimIndent()))
        }
    }

}
