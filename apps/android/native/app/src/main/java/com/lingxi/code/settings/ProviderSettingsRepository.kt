package com.lingxi.code.settings

import android.content.Context
import android.content.SharedPreferences
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.ProviderConnection
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.model.ProviderPreset
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

data class ProviderEngineLaunchConfig(
    val providerProfilesJson: String,
    val routingJson: String,
    val defaultModel: String,
    val visionDelegationEnabled: Boolean,
)

class ProviderSettingsRepository(context: Context) {
    private val appContext = context.applicationContext
    private val prefs: SharedPreferences =
        appContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

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
        require(value in setOf("china_mainland", "international")) { "Unsupported provider region" }
        prefs.edit().putString("audio_provider_region", value).apply()
    }

    fun setVisionDelegationEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_VISION_DELEGATION_ENABLED, enabled).apply()
    }

    fun persistProviders(kind: ProviderKind, providers: List<GenericProvider>) {
        val key = prefKeyFor(kind)
        val encoded = encodeProvidersJson(providers)
        prefs.edit().putString(key, encoded).apply()
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

    companion object {
        private const val PREFS_NAME = "provider_settings"
        private const val PREF_LLM = "providers_llm"
        private const val PREF_SEARCH = "providers_search"
        private const val PREF_FETCH = "providers_fetch"
        private const val PREF_VISION_DELEGATION_ENABLED = "vision_delegation_enabled"

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
                catalogBaseUrl = catalogEntries.firstOrNull { it.profileId == (if (preset.id == "google") "gemini" else preset.id) }?.baseUrl,
                catalogProtocol = catalogEntries.firstOrNull { it.profileId == (if (preset.id == "google") "gemini" else preset.id) }?.protocol,
            )
        }

        private fun usesBuiltInProfile(provider: GenericProvider): Boolean {
            // Connections make this entry describe something the built-in
            // catalog does not: several endpoints under one group. Treating it
            // as a built-in would drop it from the emitted providers map and
            // the connections would vanish with no error on any surface.
            if (provider.connections.isNotEmpty()) return false
            val defaultUrl = provider.catalogBaseUrl ?: return false
            return provider.url.isBlank() || provider.url.trimEnd('/') == defaultUrl.trimEnd('/')
        }

        private fun providerType(provider: GenericProvider): String? = when (provider.catalogProtocol) {
            "anthropic_messages" -> "anthropic"
            "gemini_generate_content" -> "gemini"
            "open_ai_responses" -> "openai-responses"
            "open_ai_chat" -> "openai"
            else -> providerPresetType(provider)
        }

        private fun providerPresetType(provider: GenericProvider): String? = when (provider.preset) {
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
            val models = listOf(provider.model.trim()).filter { it.isNotBlank() }
            if (models.isEmpty()) return null
            return JSONObject().apply {
                put("type", type)
                put("baseUrl", baseUrl)
                put(
                    "models",
                    JSONArray().apply {
                        models.forEach { model -> put(JSONObject().put("id", model)) }
                    },
                )
                // A single connection would rename the engine profile to
                // `group:id` while describing the same one endpoint, so only a
                // real branch is emitted. Each entry restates its own baseUrl;
                // the protocol is inherited from the profile above.
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
                        add(providerFromJson(array.getJSONObject(i)))
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
                catalogBaseUrl = obj.optString("catalogBaseUrl").takeIf { it.isNotEmpty() },
                catalogProtocol = obj.optString("catalogProtocol").takeIf { it.isNotEmpty() },
                name = obj.getString("name"),
                url = obj.getString("url"),
                key = "",
                model = obj.optString("model"),
                cx = obj.optString("cx"),
                status = ConnStatus.Idle,
                isDefault = obj.optBoolean("isDefault"),
                enabled = obj.optBoolean("enabled", true),
                credentialConfigured = false,
                showInModelPicker = obj.optBoolean("showInModelPicker", true),
                visibleModelIds = distinctModelIds(obj.optJSONArray("visibleModelIds")),
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
                provider.catalogBaseUrl?.let { put("catalogBaseUrl", it) }
                provider.catalogProtocol?.let { put("catalogProtocol", it) }
                put("name", provider.name)
                put("url", provider.url)
                put("model", provider.model)
                put("cx", provider.cx)
                put("isDefault", provider.isDefault)
                put("enabled", provider.enabled)
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
