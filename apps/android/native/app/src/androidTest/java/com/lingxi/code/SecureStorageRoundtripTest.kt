package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Instrumentation round-trip for [AndroidSecureStorageAdapter] — the production
 * secure store the app hands to `build_android_engine` (OAuth `/login` token
 * persistence rides this path; it is also what flips the engine's
 * `oauth_supported` true).
 *
 * MUST run on a device/emulator: the AES-256-GCM key is a non-exportable
 * AndroidKeyStore key, which needs a real Android runtime (a JVM-host run cannot
 * seal/unseal). Proves the trait contract: store overwrites, retrieve returns the
 * exact bytes, list enumerates accounts, delete removes (and deleting a missing
 * entry is not an error). FAIL ⇒ debug via `adb logcat`.
 */
@RunWith(AndroidJUnit4::class)
class SecureStorageRoundtripTest {
    private val dir = File(
        InstrumentationRegistry.getInstrumentation().targetContext.cacheDir,
        "securestore-roundtrip-test",
    )
    private val store = AndroidSecureStorageAdapter(dir)

    @After
    fun cleanup() {
        dir.deleteRecursively()
    }

    @Test
    fun storeRetrieveListDeleteRoundtrip() = runBlocking {
        val svc = "anthropic"
        val acc = "default"
        val secret = "oauth-token-αβγ-🔐".toByteArray(Charsets.UTF_8)

        // Absent → null (missing key is distinct from an error).
        assertNull("missing entry must be null", store.retrieve(svc, acc))

        // Store → retrieve the EXACT bytes back (Keystore seal/unseal round-trip).
        store.store(svc, acc, secret)
        assertArrayEquals("retrieved bytes must equal stored", secret, store.retrieve(svc, acc))

        // List → contains the account.
        assertTrue("list must contain the account", store.list(svc).contains(acc))

        // Overwrite → replaces the prior value.
        val rotated = "rotated-token".toByteArray(Charsets.UTF_8)
        store.store(svc, acc, rotated)
        assertArrayEquals("overwrite must replace", rotated, store.retrieve(svc, acc))

        // Delete → gone; the service lists empty.
        store.delete(svc, acc)
        assertNull("deleted entry must be null", store.retrieve(svc, acc))
        assertEquals("list must be empty after delete", emptyList<String>(), store.list(svc))

        // Deleting a non-existent entry is not an error (trait contract).
        store.delete(svc, "never-existed")
    }
}
