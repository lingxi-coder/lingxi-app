package com.lingxi.code.settings

import org.json.JSONArray
import org.json.JSONObject

private fun mergedConnection(provider: JSONObject, connection: JSONObject): JSONObject {
    val merged = JSONObject(provider.toString())
    merged.remove("connections")
    merged.remove("fallback")
    connection.keys().asSequence().forEach { key ->
        if (key != "id") merged.put(key, connection.get(key))
    }
    return merged
}

internal fun validateProviderDefinitions(providers: JSONObject?) {
    if (providers == null) return
    providers.keys().asSequence().forEach { id ->
        require(Regex("[a-z0-9][a-z0-9._-]{0,63}").matches(id) && id !in setOf("builtin", "claude", "prototype", "constructor")) { "Invalid profile ID: $id" }
        val provider = providers.getJSONObject(id)
        requireProviderSecretFree(provider)
        if (provider.has("fallback")) {
            val fallback = provider.optJSONObject("fallback")
            require(fallback != null) { "$id: fallback must be an object" }
            if (fallback.has("on")) {
                val triggers = fallback.optJSONArray("on")
                require(triggers != null) { "$id: fallback.on must be an array" }
                for (index in 0 until triggers.length()) {
                    require(triggers.optString(index) in setOf("rate_limit", "overloaded", "server_error", "network", "auth")) {
                        "$id: unsupported fallback.on trigger"
                    }
                }
            }
        }
        // A provider reachable several ways is validated CONNECTION BY
        // CONNECTION: the provider entry only supplies defaults, so requiring
        // baseUrl/models of it would reject a perfectly valid multi-connection
        // config outright.
        val connections = provider.optJSONArray("connections")
        if (connections != null) {
            require(connections.length() > 0) { "$id: connections must not be empty; omit it for a single connection" }
            val seen = mutableSetOf<String>()
            for (index in 0 until connections.length()) {
                val connection = connections.optJSONObject(index)
                require(connection != null) { "$id: connections[$index] must be an object" }
                val connectionId = connection.optString("id").trim()
                require(connectionId.isNotBlank()) { "$id: connections[$index] needs an id" }
                // The id becomes part of a qualified model reference
                // (`provider:connection/model`), so a separator inside it
                // produces a reference that cannot be routed.
                require(connectionId.none { it == '/' || it == ':' || it == '#' }) {
                    "$id: connection id \"$connectionId\" must not contain '/', ':' or '#'"
                }
                require(seen.add(connectionId)) { "$id: duplicate connection id \"$connectionId\"" }
                validateProviderDefinitions(JSONObject().put(id, mergedConnection(provider, connection)))
            }
            return@forEach
        }
        val type = provider.optString("type")
        require(type in setOf("openai", "openai-responses", "anthropic", "gemini", "azure-openai", "bedrock-claude", "vertex-claude", "vertex-gemini", "foundry-claude")) { "Choose a supported provider protocol" }
        if (type == "bedrock-claude") require(provider.optString("region").isNotBlank()) { "Bedrock requires a region" }
        else require(provider.optString("baseUrl").isNotBlank()) { "A base URL is required" }
        val models = provider.optJSONArray("models") ?: JSONArray()
        require(models.length() > 0) { "$id requires at least one model" }
        for (index in 0 until models.length()) {
            val metadata = models.optJSONObject(index)?.optJSONObject("metadata") ?: continue
            if (metadata.has("applicationOutputTokens") && !metadata.isNull("applicationOutputTokens")) {
                val budget = metadata.opt("applicationOutputTokens")
                require(budget is Number && budget.toDouble().let { it.isFinite() && it > 0 && it <= 4294967295.0 && it % 1.0 == 0.0 }) { "Application output budget must be a positive u32" }
            }
            if (metadata.has("context") && !metadata.isNull("context")) {
                val context = metadata.optJSONObject("context")
                require(context != null) { "Context metadata must be an object" }
                validateContextMetadata(context)
            }
        }
        val ids = (0 until models.length()).map { index -> models.optJSONObject(index)?.optString("id") ?: models.optString(index) }
        require(ids.all(String::isNotBlank) && ids.distinct().size == ids.size) { "$id requires nonempty distinct model IDs" }
        if (provider.has("baseUrl") && provider.optString("baseUrl").isNotBlank()) {
            val uri = java.net.URI(provider.optString("baseUrl"))
            require(uri.scheme in listOf("http", "https") && !uri.host.isNullOrBlank() && uri.userInfo == null && uri.fragment == null) { "Use an HTTP(S) base URL without embedded credentials" }
        }
        provider.optJSONObject("pricing")?.let { pricing ->
            pricing.keys().asSequence().forEach { model ->
                require(model in ids) { "Pricing must reference a configured model" }
                val prices = pricing.getJSONObject(model)
                require(prices.has("inputPerMtok") && prices.has("outputPerMtok")) { "Pricing requires inputPerMtok and outputPerMtok" }
                prices.keys().asSequence().forEach { price ->
                    require(price in setOf("inputPerMtok","outputPerMtok","cacheWritePerMtok","cacheReadPerMtok","reasoningPerMtok") && prices.optDouble(price,Double.NaN).let { it.isFinite() && it >= 0 }) { "Pricing values must be finite, nonnegative model prices" }
                }
            }
        }
    }
}

/** Credentials belong to account records, never provider/project settings. */
private fun requireProviderSecretFree(value: Any?) {
    when (value) {
        is JSONObject -> value.keys().asSequence().forEach { key ->
            require(key !in setOf("__proto__","prototype","constructor") && (!Regex("(?:api.?key|secret|password|authorization|credential|headers)|^(?:access|refresh|auth|bearer)?[_-]?token$",RegexOption.IGNORE_CASE).containsMatchIn(key))) {
                "Manage authentication in provider accounts, not provider configuration"
            }
            requireProviderSecretFree(value.opt(key))
        }
        is JSONArray -> (0 until value.length()).forEach { requireProviderSecretFree(value.opt(it)) }
    }
}

internal fun validateContextMetadata(context: JSONObject) {
    fun keys(value: JSONObject) = value.keys().asSequence().toSet()
    fun positive(value: Any?): Boolean = value is Number && value.toDouble().let { it.isFinite() && it > 0 && it <= 9007199254740991.0 && it % 1.0 == 0.0 }
    require(keys(context).all { it in setOf("sharedWindow", "thinkingMaxInputTokens", "recommendation", "application") }) { "Unknown context metadata field" }
    if (context.has("sharedWindow") && !context.isNull("sharedWindow")) require(context.opt("sharedWindow") is Boolean) { "sharedWindow must be boolean" }
    if (context.has("thinkingMaxInputTokens") && !context.isNull("thinkingMaxInputTokens")) require(positive(context.opt("thinkingMaxInputTokens"))) { "Thinking input limit must be positive" }
    if (context.has("recommendation") && !context.isNull("recommendation")) {
        val rec = context.optJSONObject("recommendation")
        require(rec != null && keys(rec).all { it in setOf("inputTokens", "source", "verifiedAt") } && positive(rec.opt("inputTokens")) && rec.opt("source") is String && rec.getString("source").isNotBlank() && rec.opt("verifiedAt") is String && rec.getString("verifiedAt").isNotBlank()) { "Recommendation requires input tokens, source and verification date" }
    }
    if (context.has("application") && !context.isNull("application")) {
        val policy = context.optJSONObject("application")
        require(policy != null && policy.opt("mode") in setOf("auto", "long", "custom")) { "Unknown context policy" }
        val custom = policy.getString("mode") == "custom"
        require(keys(policy).all { it == "mode" || (custom && it == "input_tokens") } && (!custom || positive(policy.opt("input_tokens")))) { "Custom input budget must be positive" }
    }
}
