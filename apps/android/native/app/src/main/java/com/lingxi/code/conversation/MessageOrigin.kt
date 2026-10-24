package com.lingxi.code.conversation

internal data class CrossSessionMessage(val text: String, val label: String)

private val crossSessionEnvelope = Regex(
    """\s*<cross-session-message(?=\s|>)([^>]*)>([\s\S]*)</cross-session-message>\s*""",
)

/** Display-only: history and model input retain the complete envelope. */
internal fun crossSessionMessage(text: String): CrossSessionMessage? {
    val envelope = crossSessionEnvelope.matchEntire(text) ?: return null
    fun attribute(name: String): String? = Regex("""(?:^|\s)$name="([^"]*)"""")
        .find(envelope.groupValues[1])?.groupValues?.get(1)
        ?.replace(Regex("&(?:quot|apos|lt|gt|amp);")) {
            when (it.value) {
                "&quot;" -> "\""
                "&apos;" -> "'"
                "&lt;" -> "<"
                "&gt;" -> ">"
                else -> "&"
            }
        }?.trim()?.takeIf { it.isNotEmpty() }
    val sender = attribute("from-name") ?: attribute("from") ?: attribute("from-session")
    return CrossSessionMessage(envelope.groupValues[2].trim(),
        if (sender == null) "来自其他会话" else "来自其他会话 · $sender")
}
