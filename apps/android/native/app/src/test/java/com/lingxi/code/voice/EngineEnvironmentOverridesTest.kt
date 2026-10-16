package com.lingxi.code.voice

import org.junit.Assert.assertEquals
import org.junit.Test

class EngineEnvironmentOverridesTest {
    @Test
    fun readsOnlyTheThreeSupportedDevelopmentOverrides() {
        val overrides = resolveEngineEnvironmentOverrides(
            mapOf(
                "ANTHROPIC_API_KEY" to "fixture-key",
                "ANTHROPIC_BASE_URL" to "https://proxy.example.test",
                "LINGXI_MODEL" to "claude-opus",
                "UNRELATED_SECRET" to "must-not-be-read",
            ),
        )

        assertEquals("fixture-key", overrides.apiKey)
        assertEquals("https://proxy.example.test", overrides.apiBase)
        assertEquals("claude-opus", overrides.model)
    }

    @Test
    fun missingEnvironmentValuesRemainEmptyForCurrentProviderConfiguration() {
        val overrides = resolveEngineEnvironmentOverrides(emptyMap())

        assertEquals("", overrides.apiKey)
        assertEquals("", overrides.apiBase)
        assertEquals("", overrides.model)
    }

    @Test
    fun diagnosticRepresentationDoesNotExposeKeyOrEndpoint() {
        val overrides = resolveEngineEnvironmentOverrides(
            mapOf(
                "ANTHROPIC_API_KEY" to "fixture-key",
                "ANTHROPIC_BASE_URL" to "https://user:password@proxy.example.test",
            ),
        )

        assertEquals(
            "EngineEnvironmentOverrides(apiBaseConfigured=true, apiKeyConfigured=true, modelConfigured=false)",
            overrides.toString(),
        )
    }
}
