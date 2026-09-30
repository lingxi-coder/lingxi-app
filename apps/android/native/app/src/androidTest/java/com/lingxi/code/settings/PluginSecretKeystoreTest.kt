package com.lingxi.code.settings

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.secure.AndroidSecureStorageAdapter
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class PluginSecretKeystoreTest {
    @Test fun storesCompatibleEnvelopeEncryptedAndDeletesIt() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = File(context.cacheDir,"plugin-secret-test-${UUID.randomUUID()}")
        val adapter = AndroidSecureStorageAdapter(directory)
        val repository = PluginSecretRepository(adapter)
        try {
            repository.save("fixture@tests","TOKEN","test-only-noncredential")
            assertTrue(repository.configured("fixture@tests","TOKEN"))
            val sealed = directory.walkTopDown().first { it.isFile }.readBytes()
            assertFalse(sealed.toString(Charsets.UTF_8).contains("test-only-noncredential"))
            val bytes = adapter.retrieve("lingxi",pluginSecretAccount("fixture@tests","TOKEN"))!!
            try { assertEquals(23,JSONObject(bytes.toString(Charsets.UTF_8)).getJSONArray("bytes").length()) }
            finally { bytes.fill(0) }
            repository.delete("fixture@tests","TOKEN")
            assertFalse(repository.configured("fixture@tests","TOKEN"))
        } finally { directory.deleteRecursively() }
    }
}
