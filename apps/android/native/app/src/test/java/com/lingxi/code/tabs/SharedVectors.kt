package com.lingxi.code.tabs

import org.json.JSONArray
import org.json.JSONObject

/**
 * The cross-platform vectors of SPEC-mobile-team-bot-ui.md section 8, read from the class path:
 * `packages/bridge-client/fixtures` is a test-resources directory in `app/build.gradle.kts`, so the
 * JVM suites load the very file the iOS suites load by path. If a table and this file disagree the
 * file wins.
 */
internal object SharedVectors {
    private fun text(name: String): String {
        val stream = requireNotNull(SharedVectors::class.java.classLoader?.getResourceAsStream(name)) {
            "$name is not on the unit-test class path (packages/bridge-client/fixtures)"
        }
        return stream.bufferedReader(Charsets.UTF_8).use { it.readText() }
    }

    val root: JSONObject by lazy { JSONObject(text("mobile-team-bot-vectors.json")) }

    /** The section 8.6 tables that have no Desktop counterpart (`mobile`). */
    val mobile: JSONObject get() = root.getJSONObject("mobile")
}

internal fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }

/** A JSON string, or null when the value is missing or an explicit JSON null. */
internal fun JSONObject.stringOrNull(key: String): String? = if (isNull(key)) null else getString(key)
