package com.lingxi.code.runtimeintegration

/** Android development-only scalar overrides for the mobile engine launch. */
internal class EngineEnvironmentOverrides(
    val apiBase: String,
    val apiKey: String,
    val model: String,
) {
    // Avoid leaking an API key or credential-bearing URL if this value is ever
    // included in a diagnostic or assertion message.
    override fun toString(): String =
        "EngineEnvironmentOverrides(apiBaseConfigured=${apiBase.isNotBlank()}, " +
            "apiKeyConfigured=${apiKey.isNotBlank()}, modelConfigured=${model.isNotBlank()})"
}

/**
 * Read only explicit process-environment overrides. Persisted provider
 * credentials and endpoints come from the current provider settings and the
 * Rust shared secure store, never from an Android legacy preferences file.
 */
internal fun resolveEngineEnvironmentOverrides(
    env: Map<String, String>,
): EngineEnvironmentOverrides = EngineEnvironmentOverrides(
    apiBase = env["ANTHROPIC_BASE_URL"].orEmpty(),
    apiKey = env["ANTHROPIC_API_KEY"].orEmpty(),
    model = env["LINGXI_MODEL"].orEmpty(),
)
