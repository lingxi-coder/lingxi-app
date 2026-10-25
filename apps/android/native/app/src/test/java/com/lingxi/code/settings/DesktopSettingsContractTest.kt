package com.lingxi.code.settings

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableSharedFlow
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
        assertEquals(com.lingxi.code.bindings.client.WritableScopeDto.PROJECT,rules.destination)
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
        bridge.accept(ClientEvent.ConfigurationOperation(com.lingxi.code.bindings.client.ConfigurationDomainDto.MCP,id,
            com.lingxi.code.bindings.client.ConfigurationOperationStatusDto.STARTED,com.lingxi.code.bindings.client.ConfigurationEffectDto.NOT_APPLICABLE,null,null))
        assertEquals(id,bridge.state.value.pending)
        bridge.accept(ClientEvent.ConfigurationOperation(com.lingxi.code.bindings.client.ConfigurationDomainDto.MCP,id,
            com.lingxi.code.bindings.client.ConfigurationOperationStatusDto.SUCCEEDED,com.lingxi.code.bindings.client.ConfigurationEffectDto.APPLIED,"Saved",null))
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

    private class CredentialSource(
        private val onCommand: suspend CredentialSource.(ClientCommand) -> Unit = {},
    ) : ConversationSource {
        override val clientEvents = MutableSharedFlow<ClientEvent>()
        val commands = mutableListOf<ClientCommand>()
        override suspend fun submitClientCommand(command: ClientCommand) {
            commands += command
            onCommand(command)
        }
    }

    private fun credentialStatus(
        id: ULong,
        configured: List<String>,
        error: String? = null,
        unavailable: List<String> = emptyList(),
    ) = ClientEvent.ProviderCredentialStatus(id, configured, unavailable, true, emptyMap(), error)

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialListingAppliesHealthyIdsAndPreservesUnavailablePresence() = runTest {
        var partial = false
        val source = CredentialSource { command ->
            if (command is ClientCommand.ListProviderCredentials) {
                clientEvents.emit(if (partial) {
                    credentialStatus(command.operationId, listOf("deepseek"), "OpenAI storage unavailable", listOf("openai"))
                } else {
                    credentialStatus(command.operationId, listOf("openai", "kimi"))
                })
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val ids = listOf("openai", "kimi", "deepseek")
        bridge.refreshCredentials(ids)
        assertEquals(mapOf("openai" to true, "kimi" to true, "deepseek" to false), bridge.state.value.credentialStates)

        partial = true
        bridge.refreshCredentials(ids)
        assertEquals(mapOf("openai" to true, "kimi" to false, "deepseek" to true), bridge.state.value.credentialStates)
        assertEquals("OpenAI storage unavailable", bridge.state.value.credentialStatusError)
        assertEquals(0L, bridge.state.value.credentialRevision)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun changingOneCredentialPreservesOtherProvidersConfirmedPresence() = runTest {
        val source = CredentialSource { command ->
            when (command) {
                is ClientCommand.ListProviderCredentials -> clientEvents.emit(credentialStatus(command.operationId, listOf("openai", "deepseek")))
                is ClientCommand.SetProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId)))
                is ClientCommand.DeleteProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                else -> Unit
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        bridge.refreshCredentials(listOf("openai", "deepseek"))
        bridge.setCredential("deepseek", "fixture-replacement")
        assertEquals(mapOf("openai" to true, "deepseek" to true), bridge.state.value.credentialStates)
        bridge.deleteCredential("deepseek")
        assertEquals(mapOf("openai" to true, "deepseek" to false), bridge.state.value.credentialStates)
        assertEquals(2L, bridge.state.value.credentialRevision)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun listingStartedBeforeDeletionCannotRestoreCredentialPresenceOrOldNotice() = runTest {
        val gate = CompletableDeferred<Unit>()
        var listings = 0
        var deepseekConfigured = true
        val source = CredentialSource { command ->
            when (command) {
                is ClientCommand.ListProviderCredentials -> {
                    val listing = ++listings
                    val configured = listOf("openai") + if (deepseekConfigured) listOf("deepseek") else emptyList()
                    if (listing == 2) gate.await()
                    clientEvents.emit(credentialStatus(command.operationId, configured, if (listing == 2) "Old listing failed" else null))
                }
                is ClientCommand.DeleteProviderCredential -> {
                    deepseekConfigured = false
                    clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                }
                else -> Unit
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val ids = listOf("openai", "deepseek")
        bridge.refreshCredentials(ids)
        val oldListing = async { bridge.refreshCredentials(ids) }; runCurrent()
        bridge.deleteCredential("deepseek")
        gate.complete(Unit)
        oldListing.await()

        assertEquals(mapOf("openai" to true, "deepseek" to false), bridge.state.value.credentialStates)
        assertEquals("Credential storage status refreshed", bridge.state.value.notice)
        assertNull(bridge.state.value.credentialStatusError)
        assertEquals(1L, bridge.state.value.credentialRevision)
        assertEquals(3, listings)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun initialListingRetriesAfterMutationToRestoreUnrelatedProviderPresence() = runTest {
        val gate = CompletableDeferred<Unit>()
        var listings = 0
        var deepseekConfigured = true
        val source = CredentialSource { command ->
            when (command) {
                is ClientCommand.ListProviderCredentials -> {
                    val listing = ++listings
                    val configured = listOf("openai") + if (deepseekConfigured) listOf("deepseek") else emptyList()
                    if (listing == 1) gate.await()
                    clientEvents.emit(credentialStatus(command.operationId, configured))
                }
                is ClientCommand.DeleteProviderCredential -> {
                    deepseekConfigured = false
                    clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                }
                else -> Unit
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val initialListing = async { bridge.refreshCredentials(listOf("openai", "deepseek")) }; runCurrent()
        bridge.deleteCredential("deepseek")
        assertEquals(mapOf("deepseek" to false), bridge.state.value.credentialStates)
        gate.complete(Unit)
        initialListing.await()

        assertEquals(mapOf("openai" to true, "deepseek" to false), bridge.state.value.credentialStates)
        assertEquals(2, listings)
        assertEquals(1L, bridge.state.value.credentialRevision)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun transportWideCredentialListingFailurePreservesConfirmedPresence() = runTest {
        var unavailable = false
        val source = CredentialSource { command ->
            if (command is ClientCommand.ListProviderCredentials) {
                clientEvents.emit(if (unavailable) {
                    credentialStatus(command.operationId, emptyList(), "Credential query unavailable")
                } else {
                    credentialStatus(command.operationId, listOf("openai"))
                })
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val ids = listOf("openai", "deepseek")
        bridge.refreshCredentials(ids)
        val confirmed = bridge.state.value.credentialStates
        unavailable = true
        bridge.refreshCredentials(ids)

        assertEquals(confirmed, bridge.state.value.credentialStates)
        assertEquals("Credential query unavailable", bridge.state.value.credentialStatusError)
        assertEquals(0L, bridge.state.value.credentialRevision)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun olderCredentialListingCannotOverwriteNewerListing() = runTest {
        val gate = CompletableDeferred<Unit>()
        var listings = 0
        val source = CredentialSource { command ->
            if (command is ClientCommand.ListProviderCredentials) {
                if (++listings == 1) {
                    gate.await()
                    clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                } else {
                    clientEvents.emit(credentialStatus(command.operationId, listOf("deepseek")))
                }
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val oldListing = async { bridge.refreshCredentials(listOf("deepseek")) }; runCurrent()
        bridge.refreshCredentials(listOf("deepseek"))
        gate.complete(Unit)
        oldListing.await()

        assertEquals(mapOf("deepseek" to true), bridge.state.value.credentialStates)
        assertEquals(0L, bridge.state.value.credentialRevision)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialsAwaitTheirOwnImmediateAcknowledgementAndRefreshModelsWithoutReconnect() = runTest {
        val source = CredentialSource { command ->
            when (command) {
                is ClientCommand.SetProviderCredential -> {
                    clientEvents.emit(credentialStatus(command.operationId + 1uL, emptyList()))
                    clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId)))
                }
                is ClientCommand.DeleteProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                else -> Unit
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        bridge.setCredential("deepseek", "fixture-secret")
        assertEquals(1L, bridge.state.value.credentialRevision)
        assertFalse(bridge.state.value.requiresReconnect)
        assertEquals(1, source.commands.count { it == ClientCommand.ListModels })

        source.clientEvents.emit(credentialStatus(1uL, listOf("deepseek")))
        assertEquals("Listing alone must not trigger another refresh", 1L, bridge.state.value.credentialRevision)
        bridge.deleteCredential("deepseek")
        assertEquals(2L, bridge.state.value.credentialRevision)
        assertFalse(bridge.state.value.requiresReconnect)
        assertEquals(2, source.commands.count { it == ClientCommand.ListModels })
        assertFalse(source.commands.any { it is ClientCommand.UpdateSettings })
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialStorageFailureIsNotReportedAsSaved() = runTest {
        val source = CredentialSource { command ->
            if (command is ClientCommand.SetProviderCredential) {
                clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId), "Secure storage unavailable"))
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val result = runCatching { bridge.setCredential("deepseek", "fixture-secret") }
        assertEquals("Secure storage unavailable", result.exceptionOrNull()?.message)
        assertEquals("Secure storage unavailable", bridge.state.value.notice)
        assertEquals(0L, bridge.state.value.credentialRevision)
        assertFalse(source.commands.contains(ClientCommand.ListModels))
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialMutationRequiresTheExpectedStoredPresence() = runTest {
        for (deleting in listOf(false, true)) {
            val source = CredentialSource { command ->
                when (command) {
                    is ClientCommand.SetProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, emptyList()))
                    is ClientCommand.DeleteProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId)))
                    else -> Unit
                }
            }
            val bridge = SettingsEngineBridge()
            backgroundScope.launch { bridge.bind(source) }; runCurrent()
            val result = runCatching {
                if (deleting) bridge.deleteCredential("deepseek") else bridge.setCredential("deepseek", "fixture-secret")
            }
            assertTrue(result.isFailure)
            assertEquals(0L, bridge.state.value.credentialRevision)
            assertFalse(source.commands.contains(ClientCommand.ListModels))
        }
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun catalogRefreshFailurePreservesConfirmedCredentialWrite() = runTest {
        val source = CredentialSource { command ->
            when (command) {
                is ClientCommand.SetProviderCredential -> clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId)))
                ClientCommand.ListModels -> error("Catalog unavailable")
                else -> Unit
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        bridge.setCredential("deepseek", "fixture-secret")
        assertEquals(1L, bridge.state.value.credentialRevision)
        assertEquals("Credential change confirmed by secure storage.", bridge.state.value.notice)
        assertFalse(bridge.state.value.requiresReconnect)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialAcknowledgementFromReplacedSourceCannotClaimCurrentEngineUpdated() = runTest {
        val gate = CompletableDeferred<Unit>()
        val old = CredentialSource { command ->
            if (command is ClientCommand.SetProviderCredential) {
                gate.await()
                clientEvents.emit(credentialStatus(command.operationId, listOf(command.providerId)))
            }
        }
        val next = CredentialSource()
        val bridge = SettingsEngineBridge()
        val oldBinding = backgroundScope.launch { bridge.bind(old) }; runCurrent()
        val write = async { runCatching { bridge.setCredential("deepseek", "fixture-secret") } }; runCurrent()
        oldBinding.cancelAndJoin()
        backgroundScope.launch { bridge.bind(next) }; runCurrent()
        val before = bridge.state.value
        val nextCommands = next.commands.toList()
        gate.complete(Unit)
        assertTrue(write.await().isFailure)
        assertEquals(before, bridge.state.value)
        assertEquals(nextCommands, next.commands)
        assertFalse(old.commands.contains(ClientCommand.ListModels))
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun canceledCredentialWriteReleasesAcknowledgementCollectorAndCannotReportSuccess() = runTest {
        val gate = CompletableDeferred<Unit>()
        var mutationId: ULong? = null
        val source = CredentialSource { command ->
            if (command is ClientCommand.SetProviderCredential) {
                mutationId = command.operationId
                gate.await()
            }
        }
        val bridge = SettingsEngineBridge()
        backgroundScope.launch { bridge.bind(source) }; runCurrent()
        val write = launch { bridge.setCredential("deepseek", "fixture-secret") }; runCurrent()
        assertTrue(bridge.state.value.credentialSaving)
        assertTrue(runCatching { bridge.deleteCredential("deepseek") }.isFailure)
        assertFalse(source.commands.any { it is ClientCommand.DeleteProviderCredential })
        assertEquals(2, source.clientEvents.subscriptionCount.value)
        write.cancelAndJoin()
        assertFalse(bridge.state.value.credentialSaving)
        assertEquals(1, source.clientEvents.subscriptionCount.value)
        source.clientEvents.emit(credentialStatus(mutationId!!, listOf("deepseek")))
        assertEquals(0L, bridge.state.value.credentialRevision)
        assertFalse(source.commands.contains(ClientCommand.ListModels))
    }

    // 项目层写到哪个目录，判据只能落在引擎回传的 files_json 上：project_dir 由引擎
    // 进程启动时的 cwd 定死，客户端的「当前项目」状态可能已经指向别处。
    @Test fun projectDirectoryComesFromTheEngineReportedProjectLayerPath() {
        val files = "[" +
            "{\"layer\":\"user\",\"path\":\"/home/me/.lingxi/settings.json\"}," +
            "{\"layer\":\"project\",\"path\":\"/home/me/work/engine-answer/.lingxi/settings.json\"}," +
            "{\"layer\":\"local\",\"path\":\"/home/me/work/engine-answer/.lingxi/settings.local.json\"}]"
        assertEquals("/home/me/work/engine-answer", projectDirectoryFromFiles(files, ".lingxi"))
        assertEquals("engine-answer", projectDisplayName("/home/me/work/engine-answer"))
    }

    @Test fun projectDirectoryUsesTheResolvedBrandConfigDirectory() {
        val files = "[{\"layer\":\"project\",\"path\":\"/home/atlas/projects/demo/.atlas/settings.json\"}]"
        assertEquals("/home/atlas/projects/demo", projectDirectoryFromFiles(files, ".atlas"))
        assertNull(projectDirectoryFromFiles(files, ".lingxi"))
        assertNull(projectDirectoryFromFiles(files, "../.atlas"))
        assertNull(projectDirectoryFromFiles(files, "nested/.atlas"))
    }

    // 后缀对不上时必须返回 null 而不是猜一个目录出来 —— 指着 B 写 A 比不显示更糟。
    @Test fun projectDirectoryRefusesToGuessWhenThePathDoesNotMatch() {
        assertNull(projectDirectoryFromFiles(null, ".lingxi"))
        assertNull(projectDirectoryFromFiles("[]", ".lingxi"))
        assertNull(projectDirectoryFromFiles("not json", ".lingxi"))
        assertNull(projectDirectoryFromFiles("[{\"layer\":\"user\",\"path\":\"/home/me/.lingxi/settings.json\"}]", ".lingxi"))
        for (path in listOf("/work/proj/settings.json", "/work/proj/.claude/settings.json", "/work/proj/.lingxi/settings.local.json", "settings.json")) {
            assertNull("must not guess a project from $path", projectDirectoryFromFiles("[{\"layer\":\"project\",\"path\":\"$path\"}]", ".lingxi"))
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


    /** A non-object `fallback` used to pass here and fail in the engine. */
    @Test
    fun nonObjectFallbackIsRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],
                  "connections":[{"id":"a"}],"fallback":"rate_limit"}}
            """.trimIndent()))
        }
    }

    /** `fallback` is real on a provider without `connections` too. */
    @Test
    fun fallbackIsValidatedWithoutConnections() {
        assertThrows(IllegalArgumentException::class.java) {
            validateProviderDefinitions(providers("""
                {"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],
                  "credentialIds":["k"],"fallback":{"on":["nope"]}}}
            """.trimIndent()))
        }
    }

}
