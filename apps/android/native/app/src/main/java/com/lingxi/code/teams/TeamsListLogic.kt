package com.lingxi.code.teams

import java.text.Normalizer

/**
 * The words the conversation list needs, for the UI language (SPEC-mobile-team-bot-ui section 8.1).
 * The UI builds the localized set from string resources; [ENGLISH] is the one set that is stored:
 * `defaultConversationTitle` is written into the conversation and `conversationLabel` recognises it.
 * The two separators are code constants, not resources: `、` and `：` for Chinese UI languages.
 */
internal data class TeamsListStrings(
    val channel: String, val directMessage: String, val group: String, val member: String, val you: String, val bot: String,
    val noMessages: String, val recalled: String, val nonText: String, val listSeparator: String, val authorSeparator: String,
) {
    companion object {
        val ENGLISH = TeamsListStrings("Channel", "Direct message", "Group", "Member", "You", "Bot", "No messages yet", "Message recalled", "[Non-text message]", ", ", ": ")
        const val ZH_LIST_SEPARATOR = "、"
        const val ZH_AUTHOR_SEPARATOR = "："
        fun isChinese(languageTag: String) = languageTag.lowercase().startsWith("zh")
    }
}

/** A person or a Bot as an avatar and a name; [id] is the uid of a person, the `botId` (else the key) of a Bot. */
internal data class TeamIdentity(val kind: String, val id: String, val name: String, val self: Boolean) {
    val isBot get() = kind == "shared_bot"
}

internal data class TeamConversationGroups(val channels: List<TeamMessageConversation>, val direct: List<TeamMessageConversation>, val botDirect: List<TeamMessageConversation>)

/** Mirrors `packages/teams/workspace.ts`: same ordering, grouping, labels and previews as the Desktop. */
internal object TeamsListLogic {
    /** Edits and recalls never advance `lastSeq`, so this is the number of messages written since the watermark. */
    fun unreadCount(conversation: TeamMessageConversation): Int = (conversation.lastSeq - conversation.lastReadSeq).coerceIn(0L, Int.MAX_VALUE.toLong()).toInt()

    /** Newest `lastMessage.at` first; rows without a time follow by `lastSeq`, then numeric-collated title, then id. */
    fun compareConversations(a: TeamMessageConversation, b: TeamMessageConversation): Int {
        val x = a.lastMessage?.atMs
        val y = b.lastMessage?.atMs
        if (x != null && y != null && x != y) return y.compareTo(x)
        if ((x == null) != (y == null)) return if (x == null) 1 else -1
        val bySeq = b.lastSeq.compareTo(a.lastSeq)
        if (bySeq != 0) return bySeq
        val byTitle = naturalCompare(a.title, b.title)
        if (byTitle != 0) return byTitle
        return a.id.compareTo(b.id)
    }

    /** Channels / people (dm + group) / Bot DMs; a Bot owner's view of other people's Bot DMs follows the viewer's own. */
    fun groupConversations(conversations: List<TeamMessageConversation>, uid: String): TeamConversationGroups {
        val channels = conversations.filter { it.kind == "channel" }.sortedWith(::compareConversations)
        val direct = conversations.filter { it.kind != "channel" && it.kind != "bot_dm" }.sortedWith(::compareConversations)
        fun mine(c: TeamMessageConversation) = c.participants.isEmpty() || "user:$uid" in c.participants
        val botDirect = conversations.filter { it.kind == "bot_dm" }.sortedWith(Comparator { a, b -> mine(b).compareTo(mine(a)).takeIf { it != 0 } ?: compareConversations(a, b) })
        return TeamConversationGroups(channels, direct, botDirect)
    }

    fun conversationLabel(conversation: TeamMessageConversation, members: List<TeamMember>, uid: String, bots: List<TeamBotCard>, strings: TeamsListStrings): String {
        val title = conversation.title.trim()
        val people = userIds(conversation)
        if (conversation.kind == "channel") return title.ifEmpty { strings.channel }
        if (conversation.kind == "bot_dm") {
            val bot = cardFor(conversation.sharedBotId, bots)?.name?.takeIf { it.isNotEmpty() } ?: title.ifEmpty { strings.bot }
            val others = people.filter { it != uid }
            return if (uid in people || others.isEmpty()) bot else bot + " · " + others.joinToString(strings.listSeparator) { memberName(it, members, uid, strings) }
        }
        val names = people.map { memberName(it, members, uid, strings) }
        if (title.isNotEmpty() && !isGeneratedTitle(title, names)) return title
        val others = people.filter { it != uid }.map { memberName(it, members, uid, strings) }
        return others.joinToString(strings.listSeparator).ifEmpty { title.ifEmpty { if (conversation.kind == "dm") strings.directMessage else strings.group } }
    }

    /** Other people (participant order), then Bots, then the viewer. A Bot is identified by its `botId` once its card is known. */
    fun conversationParticipants(conversation: TeamMessageConversation, members: List<TeamMember>, bots: List<TeamBotCard>, uid: String, strings: TeamsListStrings): List<TeamIdentity> {
        val people = userIds(conversation).map { TeamIdentity("user", it, memberName(it, members, uid, strings), it == uid) }
        val robots = conversation.participants.filter { it.startsWith("shared_bot:") }.map { key ->
            val card = cardFor(key.removePrefix("shared_bot:"), bots)
            TeamIdentity("shared_bot", card?.sharedBotId ?: key.removePrefix("shared_bot:"), card?.name?.takeIf { it.isNotEmpty() } ?: strings.bot, false)
        }
        return people.filter { !it.self } + robots + people.filter { it.self }
    }

    fun findBotDm(conversations: List<TeamMessageConversation>, uid: String, sharedBotId: String): TeamMessageConversation? =
        conversations.firstOrNull { it.kind == "bot_dm" && "shared_bot:$sharedBotId" in it.participants && "user:$uid" in it.participants }

    fun findDm(conversations: List<TeamMessageConversation>, uid: String, peerUid: String): TeamMessageConversation? =
        conversations.firstOrNull { it.kind == "dm" && it.participants.size == 2 && "user:$uid" in it.participants && "user:$peerUid" in it.participants }

    /** Stored title of a dm or group made without a chosen one: every participant, the creator first. Always English. */
    fun defaultConversationTitle(uids: List<String>, members: List<TeamMember>, uid: String, limit: Int = 120): String {
        val strings = TeamsListStrings.ENGLISH
        val names = (listOf(uid) + uids.filter { it != uid }).map { memberName(it, members, uid, strings) }
        var title = ""
        for (name in names) {
            val next = if (title.isEmpty()) name else "$title, $name"
            if (next.codePointCount(0, next.length) > limit) return title.ifEmpty { name.substring(0, name.offsetByCodePoints(0, minOf(limit, name.codePointCount(0, name.length)))) }
            title = next
        }
        return title
    }

    /** One line for the newest message: label for none / recalled / non-text, "You" prefix, author prefix in groups and channels. */
    fun conversationPreview(conversation: TeamMessageConversation, uid: String, strings: TeamsListStrings, author: (TeamLastMessage) -> String): String =
        pointerPreview(conversation.lastMessage, uid, strings, author.takeIf { conversation.kind == "group" || conversation.kind == "channel" })

    /**
     * The message part of a preview (a shared Bot's Markdown reads as plain text), which a Friends row prints from the pointer on its card (docs/FRIENDS-CLIENTS.md D3): [author] names
     * someone else's message only where the caller wants a prefix (groups and channels); the viewer's own always reads "You".
     */
    fun pointerPreview(last: TeamLastMessage?, uid: String, strings: TeamsListStrings, author: ((TeamLastMessage) -> String)? = null): String {
        if (last == null) return strings.noMessages
        val mine = last.authorKind == "user" && last.authorUid == uid
        val prefix = when {
            mine -> strings.you + strings.authorSeparator
            author != null -> author(last) + strings.authorSeparator
            else -> ""
        }
        if (last.recalled) return prefix + strings.recalled
        val text = if (TeamsBotText.rendersMarkdown(last.authorKind, last.recalled)) TeamsBotText.plainText(last.preview) else last.preview
        return prefix + text.ifEmpty { strings.nonText }
    }

    /** Case-insensitive contains on the label; a blank query matches everything. */
    fun conversationMatches(label: String, query: String): Boolean {
        val needle = query.trim().lowercase()
        return needle.isEmpty() || label.lowercase().contains(needle)
    }

    private fun userIds(conversation: TeamMessageConversation) = conversation.participants.filter { it.startsWith("user:") }.map { it.substring(5) }
    private fun cardFor(key: String?, bots: List<TeamBotCard>) = if (key == null) null else bots.firstOrNull { it.sharedBotId == key }
    private fun memberName(id: String, members: List<TeamMember>, uid: String, strings: TeamsListStrings): String =
        members.firstOrNull { it.uid == id }?.displayName?.takeIf { it.isNotEmpty() } ?: if (id == uid) strings.you else strings.member

    /** A title made of the participants' own names (any order) is a generated default, not a name a person chose. */
    private fun isGeneratedTitle(title: String, names: List<String>): Boolean {
        val parts = title.split(',', '、').map { it.trim() }.filter { it.isNotEmpty() }
        if (parts.isEmpty() || parts.size != names.size) return false
        val rest = names.toMutableList()
        for (part in parts) {
            val index = rest.indexOf(part)
            if (index < 0) return false
            rest.removeAt(index)
        }
        return true
    }

    /**
     * Numeric-collated, case-insensitive order (ICU `numeric` at primary strength; Desktop `Intl.Collator`): digit runs compare
     * by value, everything else by lower-cased text without accents. Written without ICU so it also runs in plain JVM tests.
     */
    fun naturalCompare(a: String, b: String): Int {
        val x = fold(a)
        val y = fold(b)
        var i = 0
        var j = 0
        while (i < x.length && j < y.length) {
            if (x[i].isDigit() && y[j].isDigit()) {
                var ei = i
                while (ei < x.length && x[ei].isDigit()) ei++
                var ej = j
                while (ej < y.length && y[ej].isDigit()) ej++
                val left = x.substring(i, ei).trimStart('0')
                val right = y.substring(j, ej).trimStart('0')
                if (left.length != right.length) return left.length.compareTo(right.length)
                val byDigits = left.compareTo(right)
                if (byDigits != 0) return byDigits
                i = ei
                j = ej
            } else {
                if (x[i] != y[j]) return x[i].compareTo(y[j])
                i++
                j++
            }
        }
        return (x.length - i).compareTo(y.length - j)
    }

    private fun fold(text: String): String =
        Normalizer.normalize(text, Normalizer.Form.NFD).filter { Character.getType(it) != Character.NON_SPACING_MARK.toInt() }.lowercase()
}
