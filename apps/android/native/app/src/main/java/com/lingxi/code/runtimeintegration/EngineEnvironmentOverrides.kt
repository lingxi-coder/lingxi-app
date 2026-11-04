package com.lingxi.code.runtimeintegration

/** Android development-only scalar overrides for the mobile engine launch. */
internal class EngineEnvironmentOverrides(
    val apiBase: String,
    val model: String,
) {
    // Avoid leaking a credential-bearing URL if this value is ever
    // included in a diagnostic or assertion message.
    override fun toString(): String =
        "EngineEnvironmentOverrides(apiBaseConfigured=${apiBase.isNotBlank()}, " +
            "modelConfigured=${model.isNotBlank()})"
}

/** Explicit endpoint/model development overrides; authentication belongs to provider accounts. */
internal fun resolveEngineEnvironmentOverrides(
    env: Map<String, String>,
): EngineEnvironmentOverrides = EngineEnvironmentOverrides(
    apiBase = env["ANTHROPIC_BASE_URL"].orEmpty(),
    model = env["LINGXI_MODEL"].orEmpty(),
)
