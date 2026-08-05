package com.lingxi.code.settings

import android.content.Context
import android.content.SharedPreferences
import com.lingxi.code.R
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ProviderCredentialSecretDto
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.ProviderPreset
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

data class ProviderCredentialSnapshot(
    val configuredProviderIds: Set<String>,
    val unavailableProviderIds: Set<String>,
    val storageEncrypted: Boolean,
    val error: String? = null,
)

data class ProviderStatusRefreshResult(
    val providers: List<GenericProvider>,
    val error: String? = null,
)

data class ProviderConnectionTestResult(
    val connected: Boolean,
    val reachable: Boolean,
    val authenticated: Boolean,
    val modelAvailable: Boolean,
    val httpStatus: Int?,
    val latencyMs: Long,
    val message: String,
    val usedStoredCredential: Boolean,
)

data class ProviderEngineLaunchConfig(
    val providerProfilesJson: String,
    val routingJson: String,
    val defaultModel: String,
)

interface ProviderCredentialClient : AutoCloseable {
    suspend fun list(providerIds: List<String>): ProviderCredentialSnapshot
    suspend fun set(providerId: String, secret: String): ProviderCredentialSnapshot
    suspend fun delete(providerId: String): ProviderCredentialSnapshot
    suspend fun test(
        providerId: String,
        providerPreset: String,
        apiBase: String,
        model: String,
        credentialOverride: String?,
    ): ProviderConnectionTestResult
    override fun close() = Unit
}

class EngineProviderCredentialClient(
    context: Context,
) : ProviderCredentialClient {
    private val appContext = context.applicationContext
    private val pending = ConcurrentHashMap<ULong, CompletableDeferred<ProviderCredentialSnapshot>>()
    private val closed = AtomicBoolean(false)
    private val handleDelegate = lazy(
        LazyThreadSafetyMode.SYNCHRONIZED,
    ) {
        buildVoiceEngine(
            context = appContext,
            apiBase = "",
            apiKey = "",
            model = "",
            onEvent = { event ->
                if (event is ClientEvent.ProviderCredentialStatus) {
                    pending.remove(event.operationId)?.complete(
                        ProviderCredentialSnapshot(
                            configuredProviderIds = event.configuredProviderIds.toSet(),
                            unavailableProviderIds = event.unavailableProviderIds.toSet(),
                            storageEncrypted = event.storageEncrypted,
                            error = event.error,
                        ),
                    )
                }
            },
            onPermission = {},
        )
    }
    private val handle: com.lingxi.code.bindings.MobileEngineHandle?
        get() = handleDelegate.value

    override suspend fun list(providerIds: List<String>): ProviderCredentialSnapshot =
        runOperation { handle, operationId ->
            handle.submit(
                ClientCommand.ListProviderCredentials(
                    operationId = operationId,
                    providerIds = providerIds,
                ),
            )
        }

    override suspend fun set(providerId: String, secret: String): ProviderCredentialSnapshot =
        runOperation { handle, operationId ->
            handle.submit(
                ClientCommand.SetProviderCredential(
                    operationId = operationId,
                    providerId = providerId,
                    credential = ProviderCredentialSecretDto(value = secret),
                ),
            )
        }

    override suspend fun delete(providerId: String): ProviderCredentialSnapshot =
        runOperation { handle, operationId ->
            handle.submit(
                ClientCommand.DeleteProviderCredential(
                    operationId = operationId,
                    providerId = providerId,
                ),
            )
        }

    override suspend fun test(
        providerId: String,
        providerPreset: String,
        apiBase: String,
        model: String,
        credentialOverride: String?,
    ): ProviderConnectionTestResult {
        if (closed.get()) {
            return unavailableConnectionResult("provider credential client is closed")
        }
        val engine = handle ?: return unavailableConnectionResult("engine unavailable")
        return try {
            val result = withTimeout(20_000) {
                engine.testProviderConnection(
                    providerId = providerId,
                    providerPreset = providerPreset,
                    apiBase = apiBase,
                    model = model,
                    credentialOverride = credentialOverride
                        ?.takeIf { it.isNotBlank() }
                        ?.let { ProviderCredentialSecretDto(value = it) },
                )
            }
            ProviderConnectionTestResult(
                connected = result.connected,
                reachable = result.reachable,
                authenticated = result.authenticated,
                modelAvailable = result.modelAvailable,
                httpStatus = result.httpStatus?.toInt(),
                latencyMs = result.latencyMs.toLong(),
                message = result.message,
                usedStoredCredential = result.usedStoredCredential,
            )
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            unavailableConnectionResult(appContext.getString(R.string.settings_provider_test_timeout))
        } catch (t: kotlinx.coroutines.CancellationException) {
            throw t
        } catch (_: Throwable) {
            unavailableConnectionResult(
                appContext.getString(R.string.settings_provider_test_unavailable),
            )
        }
    }

    private suspend fun runOperation(
        submit: suspend (com.lingxi.code.bindings.MobileEngineHandle, ULong) -> Unit,
    ): ProviderCredentialSnapshot {
        if (closed.get()) {
            return unavailableSnapshot("provider credential client is closed")
        }
        val operationId = nextOperationId()
        val result = CompletableDeferred<ProviderCredentialSnapshot>()
        val engine = handle ?: return unavailableSnapshot("engine unavailable")
        pending[operationId] = result
        return try {
            submit(engine, operationId)
            withTimeout(5_000) { result.await() }
        } catch (t: Throwable) {
            ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = t.message ?: "provider credential operation failed",
            )
        } finally {
            pending.remove(operationId)
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        val closedSnapshot = unavailableSnapshot("provider credential client was closed")
        pending.values.forEach { it.complete(closedSnapshot) }
        pending.clear()
        if (handleDelegate.isInitialized()) {
            runCatching { handleDelegate.value?.destroy() }
        }
    }

    private companion object {
        private val NEXT_OPERATION_ID = AtomicLong(1L)
        fun nextOperationId(): ULong = NEXT_OPERATION_ID.getAndIncrement().toULong()

        fun unavailableSnapshot(error: String) = ProviderCredentialSnapshot(
            configuredProviderIds = emptySet(),
            unavailableProviderIds = emptySet(),
            storageEncrypted = false,
            error = error,
        )

        fun unavailableConnectionResult(message: String) = ProviderConnectionTestResult(
            connected = false,
            reachable = false,
            authenticated = false,
            modelAvailable = false,
            httpStatus = null,
            latencyMs = 0,
            message = message,
            usedStoredCredential = false,
        )
    }
}

class ProviderSettingsRepository(
    context: Context,
    private val credentialClient: ProviderCredentialClient = EngineProviderCredentialClient(context),
) : AutoCloseable {
    private val appContext = context.applicationContext
    private val prefs: SharedPreferences =
        appContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    private val secureKeyStore = SecureKeyStore.create(appContext)

    fun loadProviderState(): Triple<List<GenericProvider>, List<GenericProvider>, List<GenericProvider>> {
        val llm = decodeProviders(PREF_LLM).ifEmpty { seedLegacyAnthropicIfPresent() }
        val search = decodeProviders(PREF_SEARCH)
        val fetch = decodeProviders(PREF_FETCH)
        return Triple(llm, search, fetch)
    }

    suspend fun refreshLlmStatuses(providers: List<GenericProvider>): ProviderStatusRefreshResult {
        val supportedIds = providers.mapNotNull { engineCredentialIdFor(it) }.distinct()
        if (supportedIds.isEmpty()) {
            return ProviderStatusRefreshResult(
                providers = providers.map { provider ->
                    provider.copy(
                        credentialConfigured = false,
                        status = statusFor(provider, false, encrypted = false, unavailable = false),
                    )
                },
            )
        }
        // One-time compatibility migration: early Android builds stored only
        // the Anthropic key in EncryptedSharedPreferences. Copy it into the
        // engine's shared SecureStorage so the live multi-provider client and
        // the settings status query observe the same credential.
        providers
            .firstOrNull {
                it.preset == "anthropic" &&
                    engineCredentialIdFor(it) == "anthropic" &&
                    secureKeyStore?.apiKey().orEmpty().isNotBlank()
            }
            ?.let { credentialClient.set("anthropic", secureKeyStore?.apiKey().orEmpty()) }
        val snapshot = credentialClient.list(supportedIds)
        return ProviderStatusRefreshResult(
            providers = applyCredentialSnapshot(providers, snapshot),
            error = snapshot.error,
        )
    }

    fun persistProviders(kind: ProviderKind, providers: List<GenericProvider>) {
        val key = prefKeyFor(kind)
        val encoded = JSONArray().apply {
            providers.forEach { put(providerToJson(it)) }
        }.toString()
        prefs.edit().putString(key, encoded).apply()
    }

    suspend fun setCredential(provider: GenericProvider, secret: String): ProviderCredentialSnapshot {
        val credentialId = engineCredentialIdFor(provider)
            ?: return ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = "provider ${provider.preset} is not supported by the built-in mobile catalog",
            )
        val snapshot = credentialClient.set(credentialId, secret)
        if (snapshot.error == null && provider.preset == "anthropic") {
            secureKeyStore?.setApiKey(secret)
            secureKeyStore?.setApiBase(provider.url)
        }
        return snapshot
    }

    suspend fun deleteCredential(provider: GenericProvider): ProviderCredentialSnapshot {
        val credentialId = engineCredentialIdFor(provider)
            ?: return ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = "provider ${provider.preset} is not supported by the built-in mobile catalog",
            )
        val snapshot = credentialClient.delete(credentialId)
        if (snapshot.error == null && provider.preset == "anthropic") {
            secureKeyStore?.setApiKey("")
            secureKeyStore?.setApiBase(provider.url)
        }
        return snapshot
    }

    suspend fun testConnection(
        provider: GenericProvider,
        credentialOverride: String?,
    ): ProviderConnectionTestResult {
        val credentialId = engineCredentialIdFor(provider)
            ?: return ProviderConnectionTestResult(
                connected = false,
                reachable = false,
                authenticated = false,
                modelAvailable = false,
                httpStatus = null,
                latencyMs = 0,
                message = appContext.getString(R.string.settings_provider_test_not_supported),
                usedStoredCredential = credentialOverride.isNullOrBlank(),
            )
        return credentialClient.test(
            providerId = credentialId,
            providerPreset = provider.preset,
            apiBase = provider.url,
            model = provider.model,
            credentialOverride = credentialOverride?.trim()?.takeIf { it.isNotEmpty() },
        )
    }

    /**
     * Non-secret provider settings consumed by the mobile Rust engine.
     *
     * Catalog-default endpoints use their stable built-in profile. Custom
     * endpoints, Qwen, and custom OpenAI-compatible entries get a profile named
     * after the persisted row, allowing more than one account to coexist.
     */
    fun engineLaunchConfig(): ProviderEngineLaunchConfig {
        return buildEngineLaunchConfig(loadProviderState().first)
    }

    override fun close() {
        credentialClient.close()
    }

    companion object {
        private const val PREFS_NAME = "provider_settings"
        private const val PREF_LLM = "providers_llm"
        private const val PREF_SEARCH = "providers_search"
        private const val PREF_FETCH = "providers_fetch"

        internal fun migrateLegacyDeepSeek(provider: GenericProvider): GenericProvider {
            if (provider.preset != "deepseek") return provider
            val migratedUrl = when (provider.url.trimEnd('/')) {
                "https://api.deepseek.com/v1" -> "https://api.deepseek.com"
                else -> provider.url
            }
            val migratedModel = when (provider.model) {
                "deepseek-chat", "deepseek-reasoner" -> "deepseek-v4-flash"
                else -> provider.model
            }
            return provider.copy(url = migratedUrl, model = migratedModel)
        }

        internal fun buildEngineLaunchConfig(
            savedProviders: List<GenericProvider>,
        ): ProviderEngineLaunchConfig {
            val providers = savedProviders.filter {
                it.enabled && it.credentialConfigured
            }
            val profiles = JSONObject()
            providers.forEach { provider ->
                if (!usesBuiltInProfile(provider)) {
                    userProfileJson(provider)?.let { profiles.put(profileNameFor(provider), it) }
                }
            }
            val selected = providers.firstOrNull { it.isDefault } ?: providers.firstOrNull()
            val defaultModel = selected
                ?.takeIf { it.model.isNotBlank() }
                ?.let { "${profileNameFor(it)}/${it.model.trim()}" }
                .orEmpty()
            return ProviderEngineLaunchConfig(
                providerProfilesJson = profiles.toString(),
                routingJson = buildMobileRoutingJson(mobileEnabledProfileNames(savedProviders)),
                defaultModel = defaultModel,
            )
        }

        internal const val MOBILE_ENABLED_PROFILES_KEY = "mobileEnabledProfiles"

        internal fun mobileEnabledProfileNames(
            savedProviders: List<GenericProvider>,
        ): List<String> =
            savedProviders
                .asSequence()
                .filter { it.enabled && it.credentialConfigured }
                .map(::profileNameFor)
                .distinct()
                .sorted()
                .toList()

        /**
         * Profile names are either catalog constants or normalized by
         * [profileNameFor] to `[A-Za-z0-9_.-]`, so direct quoting is safe and
         * keeps this tiny launch envelope testable on the plain JVM.
         */
        internal fun buildMobileRoutingJson(enabledProfiles: List<String>): String {
            if (enabledProfiles.isEmpty()) {
                return """{"$MOBILE_ENABLED_PROFILES_KEY":[]}"""
            }
            return enabledProfiles.joinToString(
                prefix = "{\"$MOBILE_ENABLED_PROFILES_KEY\":[\"",
                separator = "\",\"",
                postfix = "\"]}",
            )
        }

        internal fun engineCredentialIdFor(provider: GenericProvider): String? =
            providerType(provider)?.let { profileNameFor(provider) }

        internal fun profileNameFor(provider: GenericProvider): String =
            if (usesBuiltInProfile(provider)) {
                when (provider.preset) {
                    "google" -> "gemini"
                    else -> provider.preset
                }
            } else {
                provider.id
                    .lowercase()
                    .map { char ->
                        if (char.isLetterOrDigit() || char == '_' || char == '-' || char == '.') {
                            char
                        } else {
                            '_'
                        }
                    }
                    .joinToString("")
                    .trim('_')
                    .take(64)
                    .ifBlank { "mobile_provider" }
            }

        internal fun statusFor(
            provider: GenericProvider,
            credentialConfigured: Boolean,
            encrypted: Boolean,
            unavailable: Boolean,
        ): ConnStatus = when {
            unavailable -> ConnStatus.Error
            engineCredentialIdFor(provider) == null -> ConnStatus.Error
            credentialConfigured && encrypted -> ConnStatus.Configured
            credentialConfigured -> ConnStatus.Error
            else -> ConnStatus.Idle
        }

        internal fun applyCredentialSnapshot(
            providers: List<GenericProvider>,
            snapshot: ProviderCredentialSnapshot,
        ): List<GenericProvider> = providers.map { provider ->
            val credentialId = engineCredentialIdFor(provider)
            val configured = credentialId != null &&
                snapshot.configuredProviderIds.contains(credentialId)
            val unavailable = credentialId != null && (
                snapshot.unavailableProviderIds.contains(credentialId) ||
                    (snapshot.error != null && !configured)
                )
            provider.copy(
                credentialConfigured = configured,
                status = statusFor(
                    provider = provider,
                    credentialConfigured = configured,
                    encrypted = snapshot.storageEncrypted,
                    unavailable = unavailable,
                ),
            )
        }

        internal fun newProvider(kind: ProviderKind, preset: ProviderPreset): GenericProvider {
            val id = kind.idPrefix + "_" + UUID.randomUUID().toString().take(5).lowercase()
            return GenericProvider(
                id = id,
                preset = preset.id,
                name = preset.name,
                url = preset.defaultUrl,
                key = "",
                model = preset.models.firstOrNull().orEmpty(),
                status = ConnStatus.Idle,
                enabled = true,
            )
        }

        private fun usesBuiltInProfile(provider: GenericProvider): Boolean {
            val preset = ProviderKind.Llm.presets.firstOrNull { it.id == provider.preset }
                ?: return false
            if (provider.preset !in setOf("anthropic", "openai", "google", "deepseek", "kimi", "kimi-code", "openrouter")) {
                return false
            }
            return provider.url.isBlank() ||
                provider.url.trimEnd('/') == preset.defaultUrl.trimEnd('/')
        }

        private fun providerType(provider: GenericProvider): String? = when (provider.preset) {
            "anthropic" -> "anthropic"
            "google" -> "gemini"
            "openai", "deepseek", "kimi", "kimi-code", "openrouter", "qwen", "custom" -> "openai"
            else -> null
        }

        private fun userProfileJson(provider: GenericProvider): JSONObject? {
            val type = providerType(provider) ?: return null
            val baseUrl = provider.url.trim()
            if (baseUrl.isEmpty()) return null
            val presetModels = ProviderKind.Llm.presets
                .firstOrNull { it.id == provider.preset }
                ?.models
                .orEmpty()
            val models = (listOf(provider.model.trim()) + presetModels)
                .filter { it.isNotBlank() }
                .distinct()
            if (models.isEmpty()) return null
            return JSONObject().apply {
                put("type", type)
                put("baseUrl", baseUrl)
                put("apiKeyEnv", "LINGXI_MOBILE_${profileNameFor(provider).uppercase()}_API_KEY")
                put(
                    "models",
                    JSONArray().apply {
                        models.forEach { model -> put(JSONObject().put("id", model)) }
                    },
                )
            }
        }
    }

    private fun prefKeyFor(kind: ProviderKind): String = when (kind) {
        ProviderKind.Llm -> PREF_LLM
        ProviderKind.Search -> PREF_SEARCH
        ProviderKind.Fetch -> PREF_FETCH
    }

    private fun seedLegacyAnthropicIfPresent(): List<GenericProvider> {
        val storedKey = secureKeyStore?.apiKey().orEmpty()
        if (storedKey.isBlank()) return emptyList()
        return listOf(
            GenericProvider(
                id = "l_anthropic",
                preset = "anthropic",
                name = "Anthropic",
                url = secureKeyStore?.apiBase().orEmpty().ifBlank { "https://api.anthropic.com" },
                key = "",
                model = "claude-sonnet-4-5",
                status = ConnStatus.Configured,
                isDefault = true,
                enabled = true,
                credentialConfigured = true,
            ),
        )
    }

    private fun decodeProviders(key: String): List<GenericProvider> {
        val raw = prefs.getString(key, null).orEmpty()
        if (raw.isBlank()) return emptyList()
        return runCatching {
            val array = JSONArray(raw)
            buildList {
                for (i in 0 until array.length()) {
                    val obj = array.getJSONObject(i)
                    add(
                        migrateLegacyDeepSeek(
                            GenericProvider(
                                id = obj.getString("id"),
                                preset = obj.getString("preset"),
                                name = obj.getString("name"),
                                url = obj.getString("url"),
                                key = "",
                                model = obj.optString("model"),
                                cx = obj.optString("cx"),
                                status = ConnStatus.Idle,
                                isDefault = obj.optBoolean("isDefault"),
                                enabled = obj.optBoolean("enabled", true),
                                credentialConfigured = obj.optBoolean("credentialConfigured", false),
                            ),
                        ),
                    )
                }
            }
        }.getOrDefault(emptyList())
    }

    private fun providerToJson(provider: GenericProvider): JSONObject =
        JSONObject().apply {
            put("id", provider.id)
            put("preset", provider.preset)
            put("name", provider.name)
            put("url", provider.url)
            put("model", provider.model)
            put("cx", provider.cx)
            put("isDefault", provider.isDefault)
            put("enabled", provider.enabled)
            put("credentialConfigured", provider.credentialConfigured)
        }
}
