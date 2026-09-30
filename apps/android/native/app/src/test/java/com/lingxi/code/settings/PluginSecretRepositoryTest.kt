package com.lingxi.code.settings

import com.lingxi.code.bindings.AndroidSecureStorage
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class PluginSecretRepositoryTest {
    @Test fun codecMatchesRustSecureStorageDataEnvelope() {
        val envelope = JSONObject(pluginSecretEnvelope("weather@acme","API_KEY","sk-x",1_700_000_000_125).toString(Charsets.UTF_8))
        val bytes = envelope.getJSONArray("bytes")
        assertEquals(listOf(115,107,45,120),(0 until bytes.length()).map(bytes::getInt))
        val metadata = envelope.getJSONObject("metadata")
        assertEquals(1_700_000_000,metadata.getJSONObject("created_at").getLong("secs_since_epoch"))
        assertEquals(125_000_000,metadata.getJSONObject("created_at").getLong("nanos_since_epoch"))
        assertTrue(metadata.isNull("last_accessed"))
        assertEquals("{\"PluginSecret\":{\"plugin\":\"weather@acme\",\"key\":\"API_KEY\"}}",metadata.getString("kind"))
        val kind = JSONObject(metadata.getString("kind")).getJSONObject("PluginSecret")
        assertEquals("weather@acme",kind.getString("plugin"))
        assertEquals("API_KEY",kind.getString("key"))
        assertEquals("plugin-secret-weather@acme/API_KEY",pluginSecretAccount("weather@acme","API_KEY"))
    }
    @Test fun saveAndDeleteExposePresenceOnlyAndUseExistingService() = runTest {
        val entries = mutableMapOf<Pair<String,String>,ByteArray>()
        val storage = object : AndroidSecureStorage {
            override suspend fun store(service: String, account: String, blob: ByteArray) { entries[service to account] = blob.copyOf() }
            override suspend fun retrieve(service: String, account: String): ByteArray? = error("Presence must never retrieve secret bytes")
            override suspend fun delete(service: String, account: String) { entries.remove(service to account) }
            override suspend fun list(service: String): List<String> = entries.keys.filter { it.first==service }.map { it.second }
        }
        val repo = PluginSecretRepository(storage)
        assertFalse(repo.configured("weather@acme","API_KEY"))
        repo.save("weather@acme","API_KEY","fixture-token")
        assertTrue(repo.configured("weather@acme","API_KEY"))
        assertEquals(setOf("lingxi" to "plugin-secret-weather@acme/API_KEY"),entries.keys)
        assertTrue(entries.values.single().any { it.toInt()!=0 })
        repo.delete("weather@acme","API_KEY")
        assertFalse(repo.configured("weather@acme","API_KEY"))
    }
}
