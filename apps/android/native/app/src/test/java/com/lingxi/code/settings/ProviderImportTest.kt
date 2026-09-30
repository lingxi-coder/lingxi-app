package com.lingxi.code.settings

import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.bindings.android.*
import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ProviderImportTest {
    private val native = """{"providers":{"custom":{"type":"openai","baseUrl":"https://api.example.test/v1","models":["model"],"apiKey":"fixture-secret"}}}"""
    @Test fun nativeImportSeparatesCredentialsAndMarksConflicts() {
        val entry=parseProviderImport(native,JSONObject("{\"custom\":{}}")).entries.single()
        assertTrue(entry.conflict)
        assertNull(entry.error)
        assertEquals("fixture-secret",entry.credential)
        assertFalse(entry.definition.toString().contains("fixture-secret"))
        assertFalse(entry.toString().contains("fixture-secret"))
        assertEquals("model",entry.definition.getJSONArray("models").getJSONObject(0).getString("id"))
    }
    @Test fun environmentReferenceNeverBecomesSecret() {
        val entry=parseProviderImport(native.replace("fixture-secret","{env:PROVIDER_API_KEY}"),JSONObject()).entries.single()
        assertEquals("PROVIDER_API_KEY",entry.definition.getString("apiKeyEnv"))
        assertNull(entry.credential)
        assertNull(entry.error)
    }
    @Test fun openCodeMappingRetainsAliasesAndReportsMetadataDifferences() {
        val entry=parseProviderImport("""{"provider":{"custom":{"npm":"@ai-sdk/openai","options":{"baseURL":"https://api.example.test/v1","apiKey":"fixture"},"models":{"alias":{"id":"actual","name":"Example"}}}}}""",JSONObject()).entries.single()
        assertNull(entry.error)
        assertEquals("openai-responses",entry.definition.getString("type"))
        assertEquals("alias",entry.definition.getJSONArray("models").getJSONObject(0).getJSONArray("aliases").getString(0))
        assertTrue(entry.warnings.isNotEmpty())
    }
    @Test fun unsupportedSecretRequestOptionsNeverEchoValues() {
        val entry=parseProviderImport("""{"provider":{"custom":{"npm":"@ai-sdk/openai","options":{"headers":{"Authorization":"fixture-hidden"}},"models":{}}}}""",JSONObject()).entries.single()
        assertNotNull(entry.error)
        assertFalse(entry.error!!.contains("fixture-hidden"))
        assertFalse(entry.definition.toString().contains("fixture-hidden"))
        assertThrows(IllegalArgumentException::class.java) { mergeProviderImport(JSONObject(),listOf(entry)) }
    }
    @Test fun mergePreservesUnselectedExistingProfiles() {
        val current=JSONObject("{\"keep\":{\"customField\":7}}")
        val merged=mergeProviderImport(current,parseProviderImport(native,current).entries)
        assertEquals(7,merged.getJSONObject("keep").getInt("customField"))
        assertTrue(merged.has("custom"))
        assertFalse(merged.toString().contains("fixture-secret"))
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun credentialsAreAcknowledgedBeforeSettingsWrite() = runTest {
        val bridge=SettingsEngineBridge()
        val commands=mutableListOf<ClientCommand>()
        var written: String? = null
        val source=object: ConversationSource {
            override val clientEvents=MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command: ClientCommand) {
                commands+=command
                if(command is ClientCommand.UpdateSettings) written=command.patchJson
                if(command is ClientCommand.RefreshListings && written != null) bridge.accept(ClientEvent.SettingsSnapshot("{}","{}","[]",null,emptyList(),JSONObject().put("user",JSONObject(written!!)).toString(),emptyList()))
                if(command is ClientCommand.SetProviderCredential) bridge.accept(ClientEvent.ProviderCredentialStatus(command.operationId,listOf(command.providerId),emptyList(),true,emptyMap(),null))
            }
        }
        backgroundScope.launch {bridge.bind(source)};runCurrent()
        bridge.accept(ClientEvent.SettingsSnapshot("{}","{}","[]",null,emptyList(),"{\"user\":{}}",emptyList()))
        bridge.importProviders("user",JSONObject(),parseProviderImport(native,JSONObject()).entries)
        val credentialIndex=commands.indexOfFirst {it is ClientCommand.SetProviderCredential}
        val writeIndex=commands.indexOfFirst {it is ClientCommand.UpdateSettings}
        assertTrue(credentialIndex>=0 && writeIndex>credentialIndex)
        assertFalse((commands[writeIndex] as ClientCommand.UpdateSettings).patchJson.contains("fixture-secret"))
    }
}
