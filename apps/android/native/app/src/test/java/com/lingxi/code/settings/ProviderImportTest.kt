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
    @Test fun contextPoliciesSurviveImportAndRejectInvalidBudgets() {
        fun document(context: String): String = """{"providers":{"custom":{"type":"openai","baseUrl":"https://example.test/v1","models":[{"id":"m","metadata":{"context":$context}}]}}}"""
        for (context in listOf(
            """{"application":{"mode":"auto"}}""",
            """{"application":{"mode":"long"}}""",
            """{"sharedWindow":true,"thinkingMaxInputTokens":8192,"recommendation":{"inputTokens":4000,"source":"https://example.test/model","verifiedAt":"2026-10-15"},"application":{"mode":"custom","input_tokens":200000}}""",
        )) {
            val entry = parseProviderImport(document(context), JSONObject()).entries.single()
            assertNull(entry.error)
            val merged = mergeProviderImport(JSONObject(), listOf(entry))
            val saved = merged.getJSONObject("custom").getJSONArray("models").getJSONObject(0).getJSONObject("metadata").getJSONObject("context")
            assertEquals(JSONObject(context).toString(), saved.toString())
        }
        for (context in listOf(
            """{"sharedWindow":"true"}""", """{"thinkingMaxInputTokens":0}""",
            """{"recommendation":{"inputTokens":3000}}""",
            """{"application":{"mode":"custom","input_tokens":0}}""",
            """{"application":{"mode":"custom","input_tokens":"8192"}}""",
            """{"application":{"mode":"auto","input_tokens":8192}}""",
            """{"application":{"mode":"unknown"}}""",
        )) {
            val entry = parseProviderImport(document(context), JSONObject()).entries.single()
            assertNotNull(entry.error)
            assertThrows(IllegalArgumentException::class.java) { mergeProviderImport(JSONObject(), listOf(entry)) }
        }
    }

    @Test fun outputDefaultsRequirePositiveU32AndAllowRuntimeClamping() {
        fun document(budget: String): String = """{"providers":{"custom":{"type":"openai","baseUrl":"https://example.test/v1","models":[{"id":"m","metadata":{"maxOutputTokens":4096,"applicationOutputTokens":$budget}}]}}}"""
        for (budget in listOf("null", "1", "8192", "4294967295")) {
            val entry = parseProviderImport(document(budget), JSONObject()).entries.single()
            assertNull(entry.error)
            assertTrue(mergeProviderImport(JSONObject(), listOf(entry)).has("custom"))
        }
        for (budget in listOf("0", "-1", "1.5", "4294967296", "true", "\"8192\"")) {
            val entry = parseProviderImport(document(budget), JSONObject()).entries.single()
            assertNotNull(entry.error)
            assertThrows(IllegalArgumentException::class.java) { mergeProviderImport(JSONObject(), listOf(entry)) }
        }
    }

}
