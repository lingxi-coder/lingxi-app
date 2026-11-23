package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject

/** A selected member's inline display text. Offsets use UTF-16, as on the wire. */
internal data class TeamMentionEntity(val userId: String, val offset: Int, val length: Int) {
    fun json() = JSONObject().put("userId", userId).put("offset", offset).put("length", length)

    companion object {
        const val LIMIT = 200
        private val query = Regex("(^|[${TeamsComposeLogic.SPACE}(（])@([^\r\n@]{0,120})\\z")
        private val spaces = Regex("[${TeamsComposeLogic.SPACE}]+")

        fun at(text: String, caret: Int, entities: List<TeamMentionEntity>): MentionAt? {
            val before = text.substring(0, caret.coerceIn(0, text.length))
            val match = query.find(before) ?: return null
            val term = match.groupValues[2]
            val start = before.length - term.length - 1
            if (entities.any { start >= it.offset && start < it.offset + it.length }) return null
            return MentionAt(start, term)
        }

        fun searchKey(text: String): String = text.replace(spaces, " ").trim().lowercase()

        fun replacing(entities: List<TeamMentionEntity>, start: Int, end: Int, insertedLength: Int): List<TeamMentionEntity> {
            val delta = insertedLength - (end - start)
            return entities.mapNotNull { entity ->
                when {
                    entity.offset + entity.length <= start -> entity
                    entity.offset >= end -> entity.copy(offset = entity.offset + delta)
                    else -> null
                }
            }
        }

        /** Prefer verified editor coordinates to the conservative fallback for ambiguous text. */
        fun shifted(
            entities: List<TeamMentionEntity>, before: String, after: String,
            selectionStart: Int? = null, selectionEnd: Int? = null, caretAfter: Int? = null,
        ): List<TeamMentionEntity> {
            if (entities.isEmpty()) return entities
            val delta = after.length - before.length
            fun fits(start: Int, end: Int): Boolean {
                val inserted = end - start + delta
                return start >= 0 && end >= start && end <= before.length && inserted >= 0
                    && start + inserted <= after.length
                    && before.regionMatches(0, after, 0, start)
                    && before.regionMatches(end, after, start + inserted, before.length - end)
            }
            if (selectionStart != null && selectionEnd != null && (caretAfter == null || selectionEnd + delta == caretAfter)
                && fits(selectionStart, selectionEnd)) {
                return replacing(entities, selectionStart, selectionEnd, selectionEnd - selectionStart + delta)
            }
            if (caretAfter != null && delta != 0) {
                val start = caretAfter - maxOf(0, delta)
                val end = caretAfter - delta
                if (fits(start, end)) return replacing(entities, start, end, maxOf(0, delta))
            }
            if (before == after) return entities
            val common = minOf(before.length, after.length)
            var prefix = 0
            var suffix = 0
            while (prefix < common && before[prefix] == after[prefix]) prefix++
            while (suffix < common && before[before.length - suffix - 1] == after[after.length - suffix - 1]) suffix++
            val start = minOf(prefix, common - suffix)
            val end = before.length - minOf(suffix, common - prefix)
            return replacing(entities, start, end, end - start + delta)
        }

        private val boundary = Regex("^[${TeamsComposeLogic.SPACE}\\p{P}]")
        fun needsSeparator(tail: String): Boolean = !boundary.containsMatchIn(tail)

        fun replacementEnd(text: String, start: Int, caret: Int, name: String): Int {
            val typed = searchKey(text.substring(start + 1, caret))
            val target = searchKey(name)
            fun startsNamePart(value: String) = (" " + target).contains(" " + value)
            if (typed.isEmpty() || typed == target || !startsNamePart(typed)) return caret
            val limit = minOf(text.length, start + 1 + maxOf(120, name.length))
            var end = caret
            var matched = caret
            while (end < limit) {
                val char = text.codePointAt(end)
                if (char == '\r'.code || char == '\n'.code || char == '@'.code) break
                end += Character.charCount(char)
                if (spaces.matches(String(Character.toChars(char)))) continue
                val extended = searchKey(text.substring(start + 1, end))
                if (!startsNamePart(extended)) break
                if (end == text.length || !needsSeparator(text.substring(end))) matched = end
                if (extended == target) break
            }
            return matched
        }

        fun valid(entities: List<TeamMentionEntity>, text: String): List<TeamMentionEntity> {
            if (entities.size > LIMIT) return emptyList()
            var end = 0
            for (entity in entities) {
                if (entity.offset < end || entity.offset >= text.length || entity.length !in 2..(text.length - entity.offset)
                    || text[entity.offset] != '@') return emptyList()
                end = entity.offset + entity.length
                if (end < text.length && text[end].isLowSurrogate()) return emptyList()
            }
            return entities
        }

        fun parse(values: JSONArray?, text: String): List<TeamMentionEntity> {
            if (values == null || values.length() > LIMIT) return emptyList()
            val entities = runCatching {
                values.objects().map { TeamMentionEntity(it.getString("userId"), it.getInt("offset"), it.getInt("length")) }
            }.getOrDefault(emptyList())
            return valid(entities, text)
        }
    }
}
