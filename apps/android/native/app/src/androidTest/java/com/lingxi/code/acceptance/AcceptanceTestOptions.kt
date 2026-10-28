package com.lingxi.code.acceptance

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assume.assumeTrue

internal fun requireAcceptanceOptIn(name: String, reason: String, target: String, vararg legacyArguments: String) {
    val arguments = InstrumentationRegistry.getArguments()
    val enabled = acceptanceEnabled(name, arguments.getString(name))
    val explicitlyRequested = acceptanceRequested(
        target, arguments.getString("class"), legacyArguments.map { arguments.getString(it) },
    )
    check(enabled || !explicitlyRequested) { "$target was selected explicitly; enable it with -e $name true" }
    assumeTrue("$reason; enable with -e $name true", enabled)
}

internal fun acceptanceArgument(name: String): String =
    requiredAcceptanceArg(name, InstrumentationRegistry.getArguments().getString(name))
