package com.lingxi.code

import android.content.Context
import android.content.ContextWrapper
import android.content.SharedPreferences
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.settings.ProviderConnectionTestResult
import com.lingxi.code.settings.ProviderCredentialClient
import com.lingxi.code.settings.ProviderCredentialSnapshot
import com.lingxi.code.settings.ProviderSettingsRepository
import java.lang.reflect.Proxy
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Test

class ProviderStatusReadOnlyTest {
    @Test
    fun repeatedRefreshOnlyReadsTheCurrentCredentialStore() = runBlocking {
        // Keep provider settings isolated from the installed app's preferences.
        val context = object : ContextWrapper(InstrumentationRegistry.getInstrumentation().targetContext) {
            private val preferences = mutableMapOf<String, SharedPreferences>()
            override fun getApplicationContext(): Context = this
            override fun getSharedPreferences(name: String, mode: Int): SharedPreferences =
                preferences.getOrPut(name) { memoryPreferences() }
        }
        val queries = mutableListOf<List<String>>()
        val client = object : ProviderCredentialClient {
            override suspend fun list(providerIds: List<String>): ProviderCredentialSnapshot {
                queries += providerIds
                return ProviderCredentialSnapshot(setOf("anthropic"), emptySet(), true)
            }
            override suspend fun set(providerId: String, secret: String): ProviderCredentialSnapshot =
                error("Status refresh must not write credentials")
            override suspend fun delete(providerId: String): ProviderCredentialSnapshot =
                error("Status refresh must not delete credentials")
            override suspend fun test(providerId: String, providerPreset: String, apiBase: String,
                model: String, credentialOverride: String?): ProviderConnectionTestResult =
                error("Status refresh must not call the provider")
        }
        val provider = GenericProvider(
            id = "l_anthropic", preset = "anthropic", name = "Anthropic",
            url = "https://api.anthropic.com", key = "", model = "claude-sonnet-4-6",
            status = ConnStatus.Idle, enabled = true,
        )
        ProviderSettingsRepository(context, client).use { repository ->
            repeat(3) {
                val refreshed = repository.refreshLlmStatuses(listOf(provider))
                assertEquals(ConnStatus.Configured, refreshed.providers.single().status)
            }
        }
        assertEquals(List(3) { listOf("anthropic") }, queries)
    }

    private fun memoryPreferences(): SharedPreferences {
        val values = mutableMapOf<String, Any?>()
        val editor = Proxy.newProxyInstance(javaClass.classLoader,
            arrayOf(SharedPreferences.Editor::class.java)) { proxy, method, args ->
            when (method.name) {
                "putString", "putStringSet", "putInt", "putLong", "putFloat", "putBoolean" -> {
                    values[args!![0] as String] = args[1]; proxy
                }
                "remove" -> { values.remove(args!![0] as String); proxy }
                "clear" -> { values.clear(); proxy }
                "commit" -> true
                "apply" -> null
                else -> error("Unexpected fixture editor operation: ${method.name}")
            }
        }
        return Proxy.newProxyInstance(javaClass.classLoader,
            arrayOf(SharedPreferences::class.java)) { _, method, args ->
            when (method.name) {
                "edit" -> editor
                "getAll" -> values.toMap()
                "contains" -> values.containsKey(args!![0])
                "getString", "getStringSet", "getInt", "getLong", "getFloat", "getBoolean" ->
                    values[args!![0]] ?: args[1]
                "registerOnSharedPreferenceChangeListener", "unregisterOnSharedPreferenceChangeListener" -> null
                else -> error("Unexpected fixture preference operation: ${method.name}")
            }
        } as SharedPreferences
    }
}
