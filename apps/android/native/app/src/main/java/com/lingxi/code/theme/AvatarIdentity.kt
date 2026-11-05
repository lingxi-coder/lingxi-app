package com.lingxi.code.theme

/**
 * Who looks like what, shared with the Desktop and iOS (SPEC-mobile-team-bot-ui sections 7 and 8.5):
 * people get initials on a colour taken from their id, Bots get the shipped art by their `botId`.
 */
object AvatarIdentity {
    /** White initials on each of these are at least 4.5:1; the same colours are used in light and dark mode. */
    val AVATAR_COLORS: List<Long> = listOf(
        0xFF4A5FC1, 0xFF2E7D6B, 0xFFB04A6E, 0xFF8A5A1F, 0xFF6B4FA3, 0xFF2A7AA3,
        0xFF9A4B2D, 0xFF4F7A28, 0xFFA1427D, 0xFF5A6B78, 0xFF3F6FB5, 0xFFA8571A,
    )

    // JavaScript's \s, as in the Desktop initials().
    private val spaces = Regex("[\\s\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000\\ufeff]+")

    /** FNV-1a, 32 bit, over the UTF-16 code units of [id]. */
    fun fnv1a(id: String): Long {
        var hash = 0x811c9dc5.toInt()
        for (unit in id) hash = (hash xor unit.code) * 0x01000193
        return hash.toLong() and 0xFFFFFFFFL
    }

    fun avatarColorIndex(id: String): Int = (fnv1a(id) % AVATAR_COLORS.size).toInt()
    fun avatarColorArgb(id: String): Long = AVATAR_COLORS[avatarColorIndex(id)]
    fun avatarColorHex(id: String): String = "#%06x".format(avatarColorArgb(id) and 0xFFFFFF)

    /** Two letters for a person ("Ann Lee" is AL), the first character alone for CJK names, "?" for no name. */
    fun initials(name: String): String {
        val words = name.trim().split(spaces).filter { it.isNotEmpty() }
        val first = words.firstOrNull() ?: return "?"
        val head = first.substring(0, first.offsetByCodePoints(0, 1))
        if (isCjk(head.codePointAt(0))) return head
        val second = words.getOrNull(1)?.let { it.substring(0, it.offsetByCodePoints(0, 1)) } ?: ""
        return (head + second).uppercase(java.util.Locale.ROOT)
    }

    /** The tile letter of a team: the first character of its name, upper-cased, or "?". */
    fun teamInitial(name: String): String {
        val trimmed = name.trim()
        return if (trimmed.isEmpty()) "?" else trimmed.substring(0, trimmed.offsetByCodePoints(0, 1)).uppercase(java.util.Locale.ROOT)
    }

    /** The id a Bot's avatar is drawn from when only its key `bot:<owner>:<host>:<botId>` is known: the text after the last colon. */
    fun teamBotAvatarId(identityId: String): String =
        if (identityId.startsWith("bot:")) identityId.substring(identityId.lastIndexOf(':') + 1) else identityId

    private fun isCjk(codePoint: Int) = codePoint in 0x3400..0x9FFF || codePoint in 0xF900..0xFAFF || codePoint in 0x3040..0x30FF || codePoint in 0xAC00..0xD7AF
}
