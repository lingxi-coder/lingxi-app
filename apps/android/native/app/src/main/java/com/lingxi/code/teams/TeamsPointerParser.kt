package com.lingxi.code.teams

/**
 * Strict reader of the `lastMessage` pointer on a conversation document
 * (SPEC-mobile-team-bot-ui section 8.4; mirrors `firestoreLastMessage` in `packages/teams/firestore.ts`).
 *
 * The field is always present: absent is `INVALID_DATA`, an explicit null is "no message yet". Anything else
 * must be a whole pointer. There is no fallback for a missing or half-written pointer (project decision: no
 * compatibility code for older data), and one bad document stops the team's listeners.
 */
internal object TeamsPointerParser {
    /** JavaScript's `Number.MAX_SAFE_INTEGER`; larger values are not exact on the Desktop either. */
    const val MAX_SAFE_INTEGER = 9007199254740991L

    /**
     * [present] tells an absent field from an explicit null (`DocumentSnapshot.get` returns null for both),
     * [value] is the raw Firestore value, [millis] turns a Firestore timestamp into epoch milliseconds and returns
     * null for any other type. Returns null for "no message yet" and throws `TeamMessageFailure("INVALID_DATA")`.
     */
    fun parseLastMessage(present: Boolean, value: Any?, millis: (Any) -> Long?): TeamLastMessage? {
        if (!present) throw invalid()
        if (value == null) return null
        val pointer = value as? Map<*, *> ?: throw invalid()
        val id = (pointer["id"] as? String)?.takeIf { it.isNotEmpty() } ?: throw invalid()
        val seq = safeInteger(pointer["seq"]) ?: throw invalid()
        if (!pointer.containsKey("at")) throw invalid()
        val rawAt = pointer["at"]
        val atMs = if (rawAt == null) null else millis(rawAt) ?: throw invalid()
        val preview = pointer["preview"] as? String ?: throw invalid()
        val author = pointer["author"] as? Map<*, *> ?: throw invalid()
        val uid = if (author["kind"] == "user") (author["uid"] as? String)?.takeIf { it.isNotEmpty() } ?: throw invalid() else ""
        val kind = author["kind"] as? String
        val sharedBotId = when (kind) {
            "user" -> null
            "shared_bot" -> (author["sharedBotId"] as? String)?.takeIf { it.isNotEmpty() } ?: throw invalid()
            else -> throw invalid()
        }
        val recalled = when {
            !pointer.containsKey("recalled") -> false
            pointer["recalled"] == true -> true
            else -> throw invalid()
        }
        return TeamLastMessage(id, seq, atMs, kind, uid, sharedBotId, preview, recalled)
    }

    /** An integer in the JavaScript safe range. A Firestore double with an integral value counts; fractions and strings do not. */
    private fun safeInteger(value: Any?): Long? = when (value) {
        is Long, is Int, is Short, is Byte -> (value as Number).toLong().takeIf { it in -MAX_SAFE_INTEGER..MAX_SAFE_INTEGER }
        is Double, is Float -> {
            val number = (value as Number).toDouble()
            if (number.isFinite() && number == Math.rint(number) && Math.abs(number) <= MAX_SAFE_INTEGER.toDouble()) number.toLong() else null
        }
        else -> null
    }

    private fun invalid() = TeamMessageFailure("INVALID_DATA")
}
