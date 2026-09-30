package com.lingxi.code.settings

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Exercises the same engine-backed secure credential client used by the provider
 * settings screen. A fresh client represents reopening the app: the credential
 * must still be discoverable without ever returning its plaintext value.
 */
@RunWith(AndroidJUnit4::class)
class ProviderCredentialRoundtripTest {
    @Test
    fun credentialSurvivesFreshEngineClient() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val providerId = "test_${UUID.randomUUID().toString().replace("-", "").take(16)}"
        val first = EngineProviderCredentialClient(context)
        var second: EngineProviderCredentialClient? = null

        try {
            val stored = first.set(providerId, "sk-instrumentation-placeholder")
            assertNull(stored.error)
            assertTrue(stored.storageEncrypted)
            assertTrue(stored.configuredProviderIds.contains(providerId))
            first.close()

            second = EngineProviderCredentialClient(context)
            val reopened = second.list(listOf(providerId))
            assertNull(reopened.error)
            assertTrue(reopened.storageEncrypted)
            assertTrue(reopened.configuredProviderIds.contains(providerId))
        } finally {
            runCatching { second?.delete(providerId) }
            second?.close()
            first.close()
        }
    }
}
