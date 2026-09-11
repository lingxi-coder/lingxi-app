package com.lingxi.code.settings

import org.json.JSONArray
import org.json.JSONObject

/** Review entry deliberately redacts credential material from debug output. */
internal class ProviderImportEntry(
    val id: String,
    val definition: JSONObject,
    val credential: String?,
    val conflict: Boolean,
    val warnings: List<String>,
    val error: String?,
) {
    override fun toString() = "ProviderImportEntry(id=$id, credential=<redacted>)"
}
internal class ProviderImportResult(val entries: List<ProviderImportEntry>, val warnings: List<String>)

internal fun parseProviderImport(text: String, existing: JSONObject): ProviderImportResult {
    require(text.length <= 2_000_000) { "Provider import exceeds 2 MB" }
    val root = try { JSONObject(text) } catch (_: Exception) { error("Invalid JSON. Check quotes, commas and braces.") }
    require(!(root.has("provider") && root.has("providers"))) { "Use provider or providers, not both formats" }
    val openCode = root.has("provider")
    val wrapped = openCode || root.has("providers")
    val map = if (openCode) root.optJSONObject("provider") else if (wrapped) root.optJSONObject("providers") else root
    require(map != null && map.length() > 0) { "No provider definitions found" }
    val topWarnings = if (wrapped && root.keys().asSequence().any { it !in setOf("provider","providers", "\$schema") }) listOf("Only provider definitions are imported. Other top-level settings are ignored.") else emptyList()
    val entries = map.keys().asSequence().map { id ->
        val warnings = mutableListOf<String>()
        var credential: String? = null
        val definition = JSONObject()
        val issue = runCatching {
            val raw = map.getJSONObject(id)
            fun key(value: Any?) {
                if (value == null || value == JSONObject.NULL || value == "") return
                require(value is String) { "API key must be text" }
                val env = Regex("^\\{env:([A-Za-z_][A-Za-z0-9_]*)\\}$").matchEntire(value)
                if (env != null) definition.put("apiKeyEnv",env.groupValues[1])
                else {
                    require(!Regex("\\{(?:env|file):").containsMatchIn(value)) { "Unsupported credential reference. Use an API key or an environment variable name." }
                    credential = value
                }
            }
            if (openCode) {
                val type = mapOf("@ai-sdk/openai-compatible" to "openai", "@ai-sdk/openai" to "openai-responses", "@ai-sdk/anthropic" to "anthropic", "@ai-sdk/google" to "gemini")[raw.optString("npm")]
                require(type != null) { "Unsupported SDK. Choose a supported protocol in the source JSON." }
                definition.put("type",type)
                val options = if (raw.has("options")) raw.getJSONObject("options") else JSONObject()
                require(options.keys().asSequence().all { it in setOf("baseURL","apiKey") }) { "Unsupported request options. Remove them before importing." }
                if (options.has("baseURL")) definition.put("baseUrl",options.getString("baseURL"))
                key(options.opt("apiKey"))
                val models = raw.getJSONObject("models")
                val converted = JSONArray()
                models.keys().asSequence().forEach { alias ->
                    val model = models.getJSONObject(alias)
                    val modelId = if (model.has("id")) model.getString("id") else alias
                    val metadata = setOf("name","cost","limit","modalities","release_date","attachment","reasoning","temperature","tool_call","knowledge","open_weights","status")
                    require(model.keys().asSequence().all { it == "id" || it in metadata }) { "Unsupported model request fields. Remove them before importing." }
                    if (model.keys().asSequence().any { it != "id" }) warnings += "OpenCode model metadata is not converted; review model capabilities."
                    converted.put(JSONObject().put("id",modelId).also { if (modelId != alias) it.put("aliases",JSONArray(listOf(alias))) })
                }
                definition.put("models",converted)
                require(raw.keys().asSequence().all { it in setOf("npm","options","models","name","whitelist","blacklist") }) { "Unsupported OpenCode provider fields. Remove them before importing." }
                if (raw.has("whitelist") || raw.has("blacklist") || raw.has("name")) warnings += "OpenCode labels and model filters are not imported."
            } else {
                raw.keys().asSequence().filter { it != "apiKey" }.forEach { definition.put(it,raw.opt(it)) }
                val models = definition.optJSONArray("models")
                if (models != null) for (index in 0 until models.length()) {
                    if (models.opt(index) is String) models.put(index,JSONObject().put("id",models.getString(index)))
                }
                key(raw.opt("apiKey"))
            }
            validateProviderDefinitions(JSONObject().put(id,definition))
        }.exceptionOrNull()?.let { cause ->
            // JSON parser exceptions can contain input values, including secrets.
            if (cause is org.json.JSONException || cause is java.net.URISyntaxException) "Invalid provider shape. Check the definition in the source JSON." else cause.message ?: "Invalid provider definition"
        }
        ProviderImportEntry(id,definition,credential,existing.has(id),warnings.distinct(),issue)
    }.toList()
    return ProviderImportResult(entries,topWarnings)
}

internal fun mergeProviderImport(existing: JSONObject, entries: List<ProviderImportEntry>): JSONObject {
    require(entries.isNotEmpty()) { "Select at least one provider" }
    val result = JSONObject(existing.toString())
    entries.forEach { entry ->
        require(entry.error == null) { entry.error ?: "Invalid provider" }
        validateProviderDefinitions(JSONObject().put(entry.id,entry.definition))
        result.put(entry.id,entry.definition)
    }
    return result
}
