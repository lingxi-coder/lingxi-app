package com.lingxi.code.teams

/** Why the composer cannot be used (SPEC-mobile-team-bot-ui section 8.3; mirrors `Composer.tsx`). */
internal enum class ComposerLock { None, OwnerReading, BotHandedOver, BotUnverified, BotChecking, BotUnavailable }

/** The `@query` being typed right before the caret; [start] is the UTF-16 index of the `@`. */
internal data class MentionAt(val start: Int, val query: String)

/** Mirrors `packages/teams/compose.ts`. Indices are UTF-16 code-unit offsets (Kotlin string indices), `snippet` counts code points. */
internal object TeamsComposeLogic {
    const val MESSAGE_BYTES = 8192
    const val MAX_TARGETS = 8

    // JavaScript's \s: ASCII whitespace plus the Unicode space separators, BOM and line/paragraph separators.
    const val SPACE = "\\s\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000\\ufeff"
    private val mention = Regex("(^|[$SPACE(（])@([^$SPACE@]{0,40})\\z")
    private val spaces = Regex("[$SPACE]+")

    fun byteLength(text: String): Int = text.toByteArray(Charsets.UTF_8).size

    /** Non-blank and within the server's limit; the untrimmed text is measured. */
    fun canSendText(text: String): Boolean = text.isNotBlank() && byteLength(text) <= MESSAGE_BYTES

    fun mentionAt(text: String, caret: Int): MentionAt? {
        val before = text.substring(0, caret.coerceIn(0, text.length))
        val match = mention.find(before) ?: return null
        val query = match.groupValues[2]
        return MentionAt(before.length - query.length - 1, query)
    }

    /** The text without the `@query` that ran from [start] to [caret], and without the double space it leaves. */
    fun withoutMention(text: String, start: Int, caret: Int): String {
        val before = text.substring(0, start)
        val after = text.substring(caret)
        return if (before.endsWith(' ') && after.startsWith(' ')) before + after.substring(1) else before + after
    }

    /** Bots that can be asked now: only groups and channels, usable, taking part, not chosen yet, matching what was typed. */
    fun mentionCandidates(bots: List<TeamBotCard>, conversation: TeamMessageConversation, chosenKeys: Collection<String>, query: String): List<TeamBotCard> {
        if (conversation.kind != "group" && conversation.kind != "channel") return emptyList()
        val needle = query.trim().lowercase()
        return bots.filter { it.selectable && "shared_bot:${it.sharedBotId}" in conversation.participants && it.sharedBotId !in chosenKeys && (needle.isEmpty() || it.name.lowercase().contains(needle)) }
    }

    /**
     * What the box says once spoken words are added: after what is already there, with a space between, never in its place.
     * Nothing heard changes nothing. The words are only ever put in the box; sending is the reader's.
     */
    fun appendingTranscript(transcript: String, text: String): String {
        val heard = transcript.trim()
        if (heard.isEmpty()) return text
        if (text.isBlank()) return heard
        return text + (if (text.last().isWhitespace()) "" else " ") + heard
    }

    /** One line of a message for a quote or a reply bar: whitespace runs collapsed, cut at [length] code points plus an ellipsis. */
    fun snippet(text: String, length: Int = 90): String {
        val flat = text.replace(spaces, " ").trim()
        if (flat.codePointCount(0, flat.length) <= length) return flat
        return flat.substring(0, flat.offsetByCodePoints(0, length)) + "…"
    }

    /** A Bot DM from before its Bot changed maintainer ([readers]) is a record: the Bot answers in a new one. */
    fun composerLock(uid: String, kind: String, participants: List<String>, bots: List<TeamBotCard>, botsLoaded: Boolean, botsUnverified: Boolean, readers: List<String>? = null): ComposerLock {
        if (participants.isNotEmpty() && "user:$uid" !in participants) return ComposerLock.OwnerReading
        if (TeamsListLogic.botDmHandedOver(kind, participants, readers, bots)) return ComposerLock.BotHandedOver
        if (kind == "bot_dm" && participants.none { key -> key.startsWith("shared_bot:") && bots.any { it.sharedBotId == key.removePrefix("shared_bot:") && it.selectable } }) {
            return when {
                botsUnverified -> ComposerLock.BotUnverified
                !botsLoaded -> ComposerLock.BotChecking
                else -> ComposerLock.BotUnavailable
            }
        }
        return ComposerLock.None
    }
}
