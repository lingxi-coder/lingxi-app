package com.lingxi.code.settings

import com.lingxi.code.bindings.android.AndroidSecureStorage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** Uses the same Keystore adapter and Rust SecureStorageData envelope as the engine. */
internal class PluginSecretRepository(private val storage: AndroidSecureStorage) {
    suspend fun configured(plugin: String, key: String): Boolean = withContext(Dispatchers.IO) {
        storage.list(SERVICE).contains(pluginSecretAccount(plugin,key))
    }
    suspend fun save(plugin: String, key: String, secret: String) = withContext(Dispatchers.IO) {
        require(secret.isNotEmpty()) { "A secret is required" }
        val account = pluginSecretAccount(plugin,key)
        val bytes = pluginSecretEnvelope(plugin,key,secret,System.currentTimeMillis())
        try {
            storage.store(SERVICE,account,bytes)
            check(storage.list(SERVICE).contains(account)) { "Secure storage did not confirm the write" }
        } finally { bytes.fill(0) }
    }
    suspend fun delete(plugin: String, key: String) = withContext(Dispatchers.IO) {
        val account = pluginSecretAccount(plugin,key)
        storage.delete(SERVICE,account)
        check(!storage.list(SERVICE).contains(account)) { "Secure storage did not confirm deletion" }
    }
    companion object { const val SERVICE = "lingxi" }
}
internal fun pluginSecretAccount(plugin: String, key: String): String {
    require(plugin.isNotBlank() && key.isNotBlank()) { "Plugin and field identifiers are required" }
    return "plugin-secret-$plugin/$key"
}
internal fun pluginSecretEnvelope(plugin: String, key: String, secret: String, nowMillis: Long): ByteArray {
    val secretBytes = secret.toByteArray(Charsets.UTF_8)
    try {
        val kind = "{\"PluginSecret\":{\"plugin\":${JSONObject.quote(plugin)},\"key\":${JSONObject.quote(key)}}}"
        val createdAt = JSONObject().put("secs_since_epoch",nowMillis / 1000).put("nanos_since_epoch",(nowMillis % 1000) * 1_000_000)
        val metadata = JSONObject().put("created_at",createdAt).put("last_accessed",JSONObject.NULL).put("kind",kind)
        return JSONObject().put("bytes",JSONArray(secretBytes.map { it.toInt() and 255 })).put("metadata",metadata).toString().toByteArray(Charsets.UTF_8)
    } finally { secretBytes.fill(0) }
}
