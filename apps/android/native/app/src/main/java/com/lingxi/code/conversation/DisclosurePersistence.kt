package com.lingxi.code.conversation

import org.json.JSONArray
import org.json.JSONObject

/** Presentation preferences only; never persist transcript bodies into the saved-state Bundle. */
internal fun encodeDisclosures(sessions: Map<String, Set<String>>): String {
    val result = JSONArray()
    // Most recently touched sessions win within a bounded Binder-safe navigation slice.
    sessions.entries.toList().asReversed().take(32).forEach { (session, ids) ->
        if (session.isBlank() || session == "new" || session.length > 256 || ids.isEmpty()) return@forEach
        val kept = ids.filter { it.isNotBlank() && it.length <= 256 }.takeLast(128)
        if (kept.isEmpty()) return@forEach
        result.put(JSONObject().put("session", session).put("ids", JSONArray(kept)))
        if (result.toString().length > MAX_DISCLOSURE_CHARS) result.remove(result.length() - 1)
    }
    return result.toString()
}

internal fun decodeDisclosures(encoded: String?): MutableMap<String, Set<String>> {
    if (encoded == null || encoded.length > MAX_DISCLOSURE_CHARS) return linkedMapOf()
    return runCatching {
        val source = JSONArray(encoded)
        linkedMapOf<String, Set<String>>().apply {
            // Restore insertion order independently of JSONObject's key iteration order.
            (minOf(source.length(), 32) - 1 downTo 0).forEach { rowIndex ->
                val row = source.optJSONObject(rowIndex) ?: return@forEach
                val session = row.opt("session") as? String ?: return@forEach
                if (session.isBlank() || session == "new" || session.length > 256) return@forEach
                val ids = row.optJSONArray("ids") ?: return@forEach
                val restored = (0 until minOf(ids.length(), 128)).mapNotNull { index ->
                    (ids.opt(index) as? String)?.takeIf { it.isNotBlank() && it.length <= 256 }
                }.toSet()
                if (restored.isNotEmpty()) put(session, restored)
            }
        }
    }.getOrElse { linkedMapOf() }
}

internal const val MAX_DISCLOSURE_CHARS = 32 * 1024
