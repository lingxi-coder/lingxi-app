package com.lingxi.code.secure

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Precedence coverage for [resolveEngineCredentials] — the SHIP-BLOCKER #1
 * credential-resolution rule the engine builder relies on: the encrypted
 * SecureKeyStore wins, the process environment is only a dev-override fallback.
 *
 * Pure JVM tests with NO Android dependency: [resolveEngineCredentials] is a
 * free function over `(storedKey, storedBase, env)`, so the Keystore-backed
 * [SecureKeyStore] is never touched here.
 */
class EngineCredentialsTest {

    @Test
    fun storedKeyWinsOverEnv() {
        val creds = resolveEngineCredentials(
            storedKey = "sk-stored",
            storedBase = "",
            env = mapOf("ANTHROPIC_API_KEY" to "sk-env"),
        )
        assertEquals("sk-stored", creds.apiKey)
    }

    @Test
    fun envKeyUsedWhenStoreBlank() {
        val creds = resolveEngineCredentials(
            storedKey = "",
            storedBase = "",
            env = mapOf("ANTHROPIC_API_KEY" to "sk-env"),
        )
        assertEquals("sk-env", creds.apiKey)
    }

    @Test
    fun blankEverywhereResolvesToEmptyKey() {
        val creds = resolveEngineCredentials(
            storedKey = "",
            storedBase = "",
            env = emptyMap(),
        )
        assertEquals("", creds.apiKey)
        assertEquals("", creds.apiBase)
        assertEquals("", creds.model)
    }

    @Test
    fun whitespaceOnlyStoredKeyFallsBackToEnv() {
        // A stray blank stored value must NOT shadow the env override.
        val creds = resolveEngineCredentials(
            storedKey = "   ",
            storedBase = "",
            env = mapOf("ANTHROPIC_API_KEY" to "sk-env"),
        )
        assertEquals("sk-env", creds.apiKey)
    }

    @Test
    fun storedBaseWinsOverEnvBaseIndependentOfKey() {
        // Each field resolves independently: a stored base with an env key.
        val creds = resolveEngineCredentials(
            storedKey = "",
            storedBase = "https://proxy.internal",
            env = mapOf(
                "ANTHROPIC_API_KEY" to "sk-env",
                "ANTHROPIC_BASE_URL" to "https://api.anthropic.com",
            ),
        )
        assertEquals("sk-env", creds.apiKey)
        assertEquals("https://proxy.internal", creds.apiBase)
    }

    @Test
    fun envBaseUsedWhenStoreBaseBlank() {
        val creds = resolveEngineCredentials(
            storedKey = "sk-stored",
            storedBase = "",
            env = mapOf("ANTHROPIC_BASE_URL" to "https://api.anthropic.com"),
        )
        assertEquals("https://api.anthropic.com", creds.apiBase)
    }

    @Test
    fun modelComesFromEnvOnly() {
        // The model isn't a secret — it's never stored, only read from the env.
        val creds = resolveEngineCredentials(
            storedKey = "sk-stored",
            storedBase = "",
            env = mapOf("LINGXI_MODEL" to "claude-opus"),
        )
        assertEquals("claude-opus", creds.model)
    }
}
