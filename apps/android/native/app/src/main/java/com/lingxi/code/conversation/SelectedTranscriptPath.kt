package com.lingxi.code.conversation

/** A selected row must name the exact authenticated transcript, never a UUID-only substitute. */
internal fun selectedTranscriptPathMatches(selected: String?, actual: String?): Boolean =
    selected?.startsWith('/') == true && selected == actual

/** Check at native construction and pass the captured path, not a fresh UUID lookup. */
internal fun selectedTranscriptPathForCreation(selected: String?, indexed: String?, required: Boolean): String? {
    if (!required) return indexed
    check(selectedTranscriptPathMatches(selected, indexed)) { "Selected conversation source changed" }
    return selected
}
