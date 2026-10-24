package com.lingxi.code.settings

import android.content.Context
import android.content.SharedPreferences
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ProviderCredentialSecretDto
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.ProviderConnection
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.ProviderPreset
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

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
    val visionDelegationEnabled: Boolean,
)

interface ProviderCredentialClient : AutoCloseable {
    fun bindSource(source: ConversationSource?) {}
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
    private val closed = AtomicBoolean(false)
    private val credentialMutations = Mutex()
    private class ActiveBinding(val source: ConversationSource)
    @Volatile private var activeBinding: ActiveBinding? = null

    override fun bindSource(source: ConversationSource?) {
        if (!closed.get() && activeBinding?.source !== source) {
            activeBinding = source?.let(::ActiveBinding)
        }
    }

    override suspend fun list(providerIds: List<String>): ProviderCredentialSnapshot =
        runOperation { operationId ->
            ClientCommand.ListProviderCredentials(
                operationId = operationId,
                providerIds = providerIds,
                previewProviderIds = emptyList(),
            )
        }

    override suspend fun set(providerId: String, secret: String): ProviderCredentialSnapshot =
        runOperation { operationId ->
            ClientCommand.SetProviderCredential(
                operationId = operationId,
                providerId = providerId,
                credential = ProviderCredentialSecretDto(value = secret),
            )
        }

    override suspend fun delete(providerId: String): ProviderCredentialSnapshot =
        runOperation { operationId ->
            ClientCommand.DeleteProviderCredential(
                operationId = operationId,
                providerId = providerId,
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
        val binding = activeBinding
        val source = binding?.source ?: return unavailableConnectionResult("engine unavailable")
        return try {
            val result = withTimeout(20_000) {
                val draft = credentialOverride?.takeIf { it.isNotBlank() }?.let { ProviderCredentialSecretDto(value = it) }
                source.testProviderConnection(providerId, providerPreset, apiBase, model, draft)
            }
            if (activeBinding !== binding || closed.get()) return unavailableConnectionResult("engine changed; retry the connection test")
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
        command: (ULong) -> ClientCommand,
    ): ProviderCredentialSnapshot {
        if (closed.get()) {
            return unavailableSnapshot("provider credential client is closed")
        }
        val operationId = NEXT_OPERATION_ID.getAndIncrement().toULong()
        val binding = activeBinding
        val source = binding?.source ?: return unavailableSnapshot("engine unavailable")
        return try {
            val request = command(operationId)
            val current = { activeBinding === binding && !closed.get() }
            val snapshot = if (request is ClientCommand.SetProviderCredential || request is ClientCommand.DeleteProviderCredential) {
                credentialMutations.withLock {
                    runLiveProviderCredentialOperation(source, operationId, request, current)
                }
            } else {
                runLiveProviderCredentialOperation(source, operationId, request, current)
            }
            if (activeBinding !== binding) unavailableSnapshot("engine changed; retry the credential operation") else snapshot
        } catch (t: TimeoutCancellationException) {
            unavailableSnapshot("provider credential operation timed out")
        } catch (t: CancellationException) {
            throw t
        } catch (t: Throwable) {
            ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = t.message ?: "provider credential operation failed",
            )
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        activeBinding = null
    }

    private companion object {
        private val NEXT_OPERATION_ID = AtomicLong(1L)
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

/** Subscribe before dispatch: native acknowledgements may arrive during submit. */
internal suspend fun runLiveProviderCredentialOperation(
    source: ConversationSource,
    operationId: ULong,
    command: ClientCommand,
    isCurrentBinding: () -> Boolean,
): ProviderCredentialSnapshot = withTimeout(5_000) {
    coroutineScope {
        val reply = async(start = CoroutineStart.UNDISPATCHED) {
            source.clientEvents.filterIsInstance<ClientEvent.ProviderCredentialStatus>()
                .first { it.operationId == operationId }
        }
        try {
            check(isCurrentBinding()) { "engine changed; retry the credential operation" }
            source.submitClientCommand(command)
            val event = reply.await()
            check(isCurrentBinding()) { "engine changed; retry the credential operation" }
            val snapshot = ProviderCredentialSnapshot(
                configuredProviderIds = event.configuredProviderIds.toSet(),
                unavailableProviderIds = event.unavailableProviderIds.toSet(),
                storageEncrypted = event.storageEncrypted,
                error = event.error,
            )
            val mutationProvider = when (command) {
                is ClientCommand.SetProviderCredential -> command.providerId
                is ClientCommand.DeleteProviderCredential -> command.providerId
                else -> null
            }
            if (mutationProvider != null && snapshot.error == null) {
                val configured = mutationProvider in snapshot.configuredProviderIds
                check(configured == (command is ClientCommand.SetProviderCredential) &&
                    mutationProvider !in snapshot.unavailableProviderIds
                ) { "credential write was not confirmed" }
                // Availability changes after hot writes. Refresh the picker
                // through the same engine without rebuilding its session.
                try {
                    source.submitClientCommand(ClientCommand.ListModels)
                } catch (canceled: CancellationException) {
                    throw canceled
                } catch (_: Exception) {
                    // The credential is already committed. A transport failure
                    // refreshing the picker must not report that write failed.
                }
                check(isCurrentBinding()) { "engine changed; retry the credential operation" }
            }
            snapshot
        } finally {
            reply.cancel()
        }
    }
}

class ProviderSettingsRepository(
    context: Context,
    private val credentialClient: ProviderCredentialClient = EngineProviderCredentialClient(context),
) : AutoCloseable {
    private val appContext = context.applicationContext
    private val prefs: SharedPreferences =
        appContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun bindSource(source: ConversationSource?) = credentialClient.bindSource(source)

    fun loadProviderState(): Triple<List<GenericProvider>, List<GenericProvider>, List<GenericProvider>> {
        val llm = decodeProviders(PREF_LLM)
        val search = decodeProviders(PREF_SEARCH)
        val fetch = decodeProviders(PREF_FETCH)
        return Triple(llm, search, fetch)
    }

    fun visionDelegationEnabled(): Boolean =
        prefs.getBoolean(PREF_VISION_DELEGATION_ENABLED, true)

    /** Audio support can be inspected before a credential exists; only Rust resolves secrets. */
    fun audioProviderProfilesJson(): String = JSONObject().apply {
        loadProviderState().first.forEach { provider ->
            if (!usesBuiltInProfile(provider)) userProfileJson(provider)?.let { put(profileNameFor(provider), it) }
        }
    }.toString()

    fun audioProviderRegion(): String = prefs.getString("audio_provider_region", "international") ?: "international"

    fun recordAudioProviderRegion(settingsJson: String) {
        val value = JSONObject(settingsJson).optString("providerRegion", "international")
        prefs.edit().putString("audio_provider_region", value).apply()
    }

    fun setVisionDelegationEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_VISION_DELEGATION_ENABLED, enabled).apply()
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
        val snapshot = credentialClient.list(supportedIds)
        return ProviderStatusRefreshResult(
            providers = applyCredentialSnapshot(providers, snapshot),
            error = snapshot.error,
        )
    }

    fun persistProviders(kind: ProviderKind, providers: List<GenericProvider>) {
        val key = prefKeyFor(kind)
        val encoded = encodeProvidersJson(providers)
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
        return credentialClient.set(credentialId, secret)
    }

    suspend fun deleteCredential(provider: GenericProvider): ProviderCredentialSnapshot {
        val credentialId = engineCredentialIdFor(provider)
            ?: return ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = "provider ${provider.preset} is not supported by the built-in mobile catalog",
            )
        return credentialClient.delete(credentialId)
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
        return buildEngineLaunchConfig(
            loadProviderState().first,
            visionDelegationEnabled = visionDelegationEnabled(),
        )
    }

    override fun close() {
        credentialClient.close()
    }

    companion object {
        private const val PREFS_NAME = "provider_settings"
        private const val PREF_LLM = "providers_llm"
        private const val PREF_SEARCH = "providers_search"
        private const val PREF_FETCH = "providers_fetch"
        private const val PREF_VISION_DELEGATION_ENABLED = "vision_delegation_enabled"

        internal fun migrateLegacyDeepSeek(provider: GenericProvider): GenericProvider {
            if (provider.preset != "deepseek") return provider
            val migratedUrl = when (provider.url.trimEnd('/')) {
                "https://api.deepseek.com/v1" -> "https://api.deepseek.com"
                else -> provider.url
            }
            val migratedModel = when (provider.model) {
                "deepseek-chat", "deepseek-reasoner" -> "deepseek-flash"
                else -> provider.model
            }
            return provider.copy(url = migratedUrl, model = migratedModel)
        }

        internal fun buildEngineLaunchConfig(
            savedProviders: List<GenericProvider>,
            visionDelegationEnabled: Boolean = true,
        ): ProviderEngineLaunchConfig {
            // Routing describes configured profiles. Credential availability is
            // resolved dynamically by the engine, independently of its lifetime.
            val providers = savedProviders.filter { it.enabled }
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
                visionDelegationEnabled = visionDelegationEnabled,
            )
        }

        internal const val MOBILE_ENABLED_PROFILES_KEY = "mobileEnabledProfiles"

        internal fun mobileEnabledProfileNames(
            savedProviders: List<GenericProvider>,
        ): List<String> =
            savedProviders
                .asSequence()
                .filter { it.enabled }
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
            val unavailable = credentialId != null && (
                snapshot.unavailableProviderIds.contains(credentialId) ||
                    (snapshot.error != null && snapshot.unavailableProviderIds.isEmpty())
                )
            val configured = if (unavailable) provider.credentialConfigured else
                credentialId != null && snapshot.configuredProviderIds.contains(credentialId)
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

        internal fun newProvider(
            kind: ProviderKind,
            preset: ProviderPreset,
            catalogEntries: List<LlmProviderCatalogEntry> = emptyList(),
        ): GenericProvider {
            val id = kind.idPrefix + "_" + UUID.randomUUID().toString().take(5).lowercase()
            val seededModel = if (kind == ProviderKind.Llm) {
                val profileId = when (preset.id) {
                    "google" -> "gemini"
                    else -> preset.id
                }
                catalogEntries
                    .firstOrNull { it.profileId == profileId }
                    ?.modelDetails
                    ?.firstOrNull()
                    ?.modelId
                    .orEmpty()
            } else {
                preset.models.firstOrNull().orEmpty()
            }
            return GenericProvider(
                id = id,
                preset = preset.id,
                name = preset.name,
                url = preset.defaultUrl,
                key = "",
                model = seededModel,
                status = ConnStatus.Idle,
                enabled = true,
            )
        }

        private fun usesBuiltInProfile(provider: GenericProvider): Boolean {
            // Connections make this entry describe something the built-in
            // catalog does not: several endpoints under one group. Treating it
            // as a built-in would drop it from the emitted providers map and
            // the connections would vanish with no error on any surface.
            if (provider.connections.isNotEmpty()) return false
            val preset = ProviderKind.Llm.presets.firstOrNull { it.id == provider.preset }
            val defaultUrl = preset?.defaultUrl ?: mapOf(
                "openai-chatgpt" to "https://chatgpt.com/backend-api/codex",
                "github-copilot" to "https://api.githubcopilot.com",
                "zai" to "https://api.z.ai/api/paas/v4",
                "glm-coding" to "https://open.bigmodel.cn/api/anthropic",
            )[provider.preset] ?: return false
            if (provider.preset !in setOf(
                    "anthropic", "openai", "openai-chatgpt", "google", "deepseek", "kimi",
                    "kimi-code", "github-copilot", "zai", "glm-coding", "openrouter",
                )) {
                return false
            }
            return provider.url.isBlank() || provider.url.trimEnd('/') == defaultUrl.trimEnd('/')
        }

        private fun providerType(provider: GenericProvider): String? = when (provider.preset) {
            "anthropic" -> "anthropic"
            "glm-coding" -> "anthropic"
            "google" -> "gemini"
            "openai", "openai-chatgpt", "deepseek", "kimi", "kimi-code", "github-copilot", "zai", "openrouter", "qwen", "custom" -> "openai"
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
                // A single connection would rename the engine profile to
                // `group:id` while describing the same one endpoint, so only a
                // real branch is emitted. Each entry restates its own baseUrl;
                // the protocol and credential are inherited from the keys above.
                if (provider.connections.size > 1) {
                    put(
                        "connections",
                        JSONArray().apply {
                            provider.connections.forEach { connection ->
                                put(
                                    JSONObject().apply {
                                        put("id", connection.id.trim())
                                        put("baseUrl", connection.url.trim())
                                        val restricted = connection.modelIds.orEmpty()
                                        if (restricted.isNotEmpty()) {
                                            put(
                                                "models",
                                                JSONArray().apply {
                                                    restricted.forEach { put(JSONObject().put("id", it)) }
                                                },
                                            )
                                        }
                                    },
                                )
                            }
                        },
                    )
                }
            }
        }

        internal fun decodeProvidersJson(raw: String): List<GenericProvider> {
            if (raw.isBlank()) return emptyList()
            return runCatching {
                val array = JSONArray(raw)
                buildList {
                    for (i in 0 until array.length()) {
                        add(migrateLegacyDeepSeek(providerFromJson(array.getJSONObject(i))))
                    }
                }
            }.getOrDefault(emptyList())
        }

        internal fun encodeProvidersJson(providers: List<GenericProvider>): String =
            JSONArray().apply {
                providers.forEach { put(providerToJson(it)) }
            }.toString()

        private fun providerFromJson(obj: JSONObject): GenericProvider =
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
                showInModelPicker = obj.optBoolean("showInModelPicker", true),
                visibleModelIds = distinctModelIds(obj.optJSONArray("visibleModelIds")),
                // Absent from every provider saved before connections existed.
                connections = readConnections(obj.optJSONArray("connections")),
            )

        /** `null` for an absent array, so "inherits everything" stays distinct from "serves nothing". */
        private fun distinctModelIds(ids: JSONArray?): List<String>? {
            if (ids == null) return null
            val result = mutableListOf<String>()
            for (index in 0 until ids.length()) {
                val modelId = ids.optString(index).trim()
                if (modelId.isNotEmpty() && modelId !in result) result.add(modelId)
            }
            return result
        }

        private fun readConnections(entries: JSONArray?): List<ProviderConnection> {
            if (entries == null) return emptyList()
            val result = mutableListOf<ProviderConnection>()
            for (index in 0 until entries.length()) {
                val entry = entries.optJSONObject(index) ?: continue
                result.add(
                    ProviderConnection(
                        id = entry.optString("id"),
                        url = entry.optString("url"),
                        modelIds = distinctModelIds(entry.optJSONArray("modelIds")),
                    ),
                )
            }
            return result
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
                put("showInModelPicker", provider.showInModelPicker)
                if (provider.visibleModelIds != null) {
                    put(
                        "visibleModelIds",
                        JSONArray().apply {
                            provider.visibleModelIds.forEach(::put)
                        },
                    )
                }
                if (provider.connections.isNotEmpty()) {
                    put(
                        "connections",
                        JSONArray().apply {
                            provider.connections.forEach { connection ->
                                put(
                                    JSONObject().apply {
                                        put("id", connection.id)
                                        put("url", connection.url)
                                        if (connection.modelIds != null) {
                                            put(
                                                "modelIds",
                                                JSONArray().apply { connection.modelIds.forEach(::put) },
                                            )
                                        }
                                    },
                                )
                            }
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

    private fun decodeProviders(key: String): List<GenericProvider> {
        val raw = prefs.getString(key, null).orEmpty()
        return decodeProvidersJson(raw)
    }
}
