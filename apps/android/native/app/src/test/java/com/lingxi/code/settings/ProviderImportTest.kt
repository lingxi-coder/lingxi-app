package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ProviderImportTest {
    private val native = """{"providers":{"custom":{"type":"openai","baseUrl":"https://api.example.test/v1","models":["model"]}}}"""

    @Test fun configurationImportMarksConflictsAndKeepsModels() {
        val entry = parseProviderImport(native, JSONObject("{\"custom\":{}}")).entries.single()
        assertTrue(entry.conflict)
        assertNull(entry.error)
        assertEquals("model", entry.definition.getJSONArray("models").getJSONObject(0).getString("id"))
    }

    @Test fun credentialsAndEnvironmentReferencesMustUseAccountManagement() {
        for (value in listOf("fixture-secret", "{env:PROVIDER_API_KEY}")) {
            val document = JSONObject(native)
            document.getJSONObject("providers").getJSONObject("custom").put("apiKey", value)
            val entry = parseProviderImport(document.toString(), JSONObject()).entries.single()
            assertNotNull(entry.error)
            assertFalse(entry.error!!.contains(value))
            assertFalse(entry.definition.toString().contains(value))
            assertThrows(IllegalArgumentException::class.java) { mergeProviderImport(JSONObject(), listOf(entry)) }
        }
    }

    @Test fun openCodeMappingRetainsAliasesAndReportsMetadataDifferences() {
        val entry = parseProviderImport("""{"provider":{"custom":{"npm":"@ai-sdk/openai","options":{"baseURL":"https://api.example.test/v1"},"models":{"alias":{"id":"actual","name":"Example"}}}}}""", JSONObject()).entries.single()
        assertNull(entry.error)
        assertEquals("openai-responses", entry.definition.getString("type"))
        assertEquals("alias", entry.definition.getJSONArray("models").getJSONObject(0).getJSONArray("aliases").getString(0))
        assertTrue(entry.warnings.isNotEmpty())
    }

    @Test fun unsupportedSecretRequestOptionsNeverEchoValues() {
        val entry = parseProviderImport("""{"provider":{"custom":{"npm":"@ai-sdk/openai","options":{"headers":{"Authorization":"fixture-hidden"}},"models":{}}}}""", JSONObject()).entries.single()
        assertNotNull(entry.error)
        assertFalse(entry.error!!.contains("fixture-hidden"))
        assertFalse(entry.definition.toString().contains("fixture-hidden"))
    }

    @Test fun mergePreservesUnselectedExistingProfiles() {
        val current = JSONObject("{\"keep\":{\"customField\":7}}")
        val merged = mergeProviderImport(current, parseProviderImport(native, current).entries)
        assertEquals(7, merged.getJSONObject("keep").getInt("customField"))
        assertTrue(merged.has("custom"))
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
