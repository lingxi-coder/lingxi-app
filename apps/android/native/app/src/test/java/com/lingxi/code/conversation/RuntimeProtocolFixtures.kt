package com.lingxi.code.conversation

import java.io.File

/** Resolve the fixed Cargo dependency; never fall back to a local protocol copy. */
internal object RuntimeProtocolFixtures {
    private val root: File by lazy {
        val resolver = generateSequence(File("").absoluteFile) { it.parentFile }
            .map { File(it, "scripts/lib/runtime_source.py") }
            .firstOrNull { it.isFile }
            ?: error("Cannot locate the host runtime_source.py resolver")
        val process = ProcessBuilder("python3", resolver.absolutePath, "--root")
            .redirectError(ProcessBuilder.Redirect.INHERIT)
            .start()
        val output = process.inputStream.bufferedReader().use { it.readText() }.trim()
        check(process.waitFor() == 0) { "Pinned Harness source resolution failed" }
        check(output.isNotEmpty()) { "Pinned Harness resolver returned no root" }
        File(output)
    }

    fun snapshot(relative: String): File =
        File(root, "crates/client/snapshots/$relative").also {
            check(it.isFile) { "Pinned Harness snapshot missing: $it" }
        }
}
