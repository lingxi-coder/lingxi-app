package com.lingxi.code.teams

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** Every table of SPEC-mobile-team-bot-ui section 8.1, run on the vectors the Desktop's `workspace.ts` produced. */
class TeamsListLogicTest {
    private val root get() = TeamsVectors.root
    private val members = TeamsVectors.members()
    private val bots = TeamsVectors.bots()
    private val uid = TeamsVectors.UID
    private fun ids(list: List<TeamMessageConversation>) = list.map { it.id }

    @Test fun unreadCountIsTheDistanceToTheWatermarkAndNeverNegative() {
        val rows = root.getJSONArray("unreadCount").objects()
        assertEquals(3, rows.size)
        rows.forEach { row ->
            val c = TeamsVectors.conversation(JSONObject().put("id", "u").put("kind", "dm").put("title", "").put("lastSeq", row.getLong("lastSeq")).put("lastReadSeq", row.getLong("lastReadSeq")))
            assertEquals(row.getInt("expect"), TeamsListLogic.unreadCount(c))
        }
    }

    @Test fun compareConversationsSortsNewestFirstThenRowsWithoutATimeBySeqThenNumericTitle() {
        val byId = TeamsVectors.conversations().associateBy { it.id }
        val sort = root.getJSONObject("sort")
        val input = TeamsVectors.strings(sort.getJSONArray("input")).map { byId.getValue(it) }
        assertEquals(TeamsVectors.strings(sort.getJSONArray("expect")), ids(input.sortedWith(TeamsListLogic::compareConversations)))
        // "room 2" before "room 10": numeric collation, and the case of letters does not matter.
        assertTrue(TeamsListLogic.naturalCompare("room 2", "room 10") < 0)
        assertEquals(0, TeamsListLogic.naturalCompare("Room 02", "room 2"))
        assertTrue(TeamsListLogic.naturalCompare("alpha", "Beta") < 0)
        assertEquals(0, TeamsListLogic.naturalCompare("café", "cafe"))
    }

    @Test fun groupConversationsPutsOwnBotDmsBeforeOtherPeoplesAndDirectIsDmPlusGroup() {
        val byId = TeamsVectors.conversations().associateBy { it.id }
        val group = root.getJSONObject("group")
        val groups = TeamsListLogic.groupConversations(TeamsVectors.strings(group.getJSONArray("input")).map { byId.getValue(it) }, uid)
        val expect = group.getJSONObject("expect")
        assertEquals(TeamsVectors.strings(expect.getJSONArray("channels")), ids(groups.channels))
        assertEquals(TeamsVectors.strings(expect.getJSONArray("direct")), ids(groups.direct))
        assertEquals(TeamsVectors.strings(expect.getJSONArray("botDirect")), ids(groups.botDirect))
    }

    @Test fun conversationLabelCoversChannelsDirectGroupsAndBotDms() {
        val rows = root.getJSONArray("labels").objects()
        assertEquals(12, rows.size)
        rows.forEach { row ->
            val conversation = TeamsVectors.resolve(row.getJSONObject("conversation"))
            val strings = TeamsVectors.strings(row.getBoolean("zh"))
            assertEquals(row.getString("name"), row.getString("expect"), TeamsListLogic.conversationLabel(conversation, members, uid, bots, strings))
        }
    }

    @Test fun defaultConversationTitleIsAlwaysEnglishAndCutAtTheLimit() {
        root.getJSONArray("defaultConversationTitle").objects().forEach { row ->
            assertEquals(row.getString("expect"), TeamsListLogic.defaultConversationTitle(TeamsVectors.strings(row.getJSONArray("uids")), members, uid, row.getInt("limit")))
        }
        // Not a stored Chinese title: a missing directory entry reads "Member" even when the UI is Chinese.
        assertEquals("Me, Member", TeamsListLogic.defaultConversationTitle(listOf("u7"), members, uid))
    }

    @Test fun conversationParticipantsListOthersThenBotsThenTheViewer() {
        val section = root.getJSONObject("participants")
        val conversation = TeamsVectors.conversationById(section.getString("conversation"))
        val got = TeamsListLogic.conversationParticipants(conversation, members, bots, uid, TeamsVectors.strings(false))
        val want = section.getJSONArray("expect").objects()
        assertEquals(want.size, got.size)
        want.zip(got).forEach { (w, g) ->
            assertEquals(w.getString("kind"), g.kind); assertEquals(w.getString("id"), g.id); assertEquals(w.getString("name"), g.name); assertEquals(w.getBoolean("self"), g.self)
        }
        // The viewer comes last; a Bot whose card has not loaded is identified by its key.
        val own = TeamsListLogic.conversationParticipants(TeamsVectors.conversationById("bd1"), members, emptyList(), uid, TeamsVectors.strings(false))
        assertEquals(listOf("bot", "user"), own.map { it.kind }); assertEquals("bot:t1:h:helper", own.first().id); assertEquals("Bot", own.first().name); assertTrue(own.last().self)
    }

    @Test fun findDmAndFindBotDmOnlyMatchTheViewersOwnConversations() {
        val byId = TeamsVectors.conversations().associateBy { it.id }
        val finds = root.getJSONObject("finds")
        assertEquals(finds.getString("botDm"), TeamsListLogic.findBotDm(listOf(byId.getValue("bd2"), byId.getValue("bd1")), uid, "bot:t1:h:helper")?.id)
        assertEquals(finds.getString("dm"), TeamsListLogic.findDm(listOf(byId.getValue("dm1"), byId.getValue("grp")), uid, "u2")?.id)
        assertNull(TeamsListLogic.findDm(listOf(byId.getValue("dm1")), uid, "u3"))
        assertTrue(finds.isNull("dmMissing"))
    }

    @Test fun conversationPreviewCoversNoneOwnRecalledNonTextAndAuthorPrefix() {
        val section = root.getJSONObject("previews")
        val names = section.getJSONObject("authorNames")
        val rows = section.getJSONArray("cases").objects()
        assertEquals(11, rows.size)
        rows.forEach { row ->
            val pointer = row.get("lastMessage").let { value ->
                if (value !is JSONObject) null else {
                    val author = value.getJSONObject("author")
                    TeamLastMessage(value.getString("id"), value.getLong("seq"), TeamsVectors.parseInstant(value.stringOrNull("at")), author.getString("kind"), author.getString("uid"), author.stringOrNull("botKey"), value.getString("preview"), value.optBoolean("recalled"))
                }
            }
            val conversation = TeamMessageConversation("c", row.getString("kind"), "t", emptyList(), 1, 1, 0, lastMessage = pointer)
            val text = TeamsListLogic.conversationPreview(conversation, uid, TeamsVectors.strings(row.getBoolean("zh"))) { last -> if (last.authorKind == "bot") names.getString("bot") else names.optString(last.authorUid, last.authorUid) }
            assertEquals(row.getString("name"), row.getString("expect"), text)
        }
    }

    @Test fun conversationMatchesIsACaseInsensitiveContainsOnTheTrimmedQuery() {
        val rows = root.getJSONArray("search").objects()
        assertEquals(7, rows.size)
        rows.forEach { assertEquals("${it.getString("label")} / ${it.getString("query")}", it.getBoolean("expect"), TeamsListLogic.conversationMatches(it.getString("label"), it.getString("query"))) }
        assertFalse(TeamsListLogic.conversationMatches("Design", "x"))
    }

    /** The two word sets of the vectors are the en and zh-Hans resource values of the keys in the table (section 13.1). */
    @Test fun listStringsEqualTheEnglishAndChineseResourceValues() {
        fun resources(directory: String): Map<String, String> {
            val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
            // Plain text on purpose: the Android XML classes on the unit-test class path are stubs.
            return Regex("<string\\s+name=\"([^\"]+)\"[^>]*>(.*?)</string>", RegexOption.DOT_MATCHES_ALL).findAll(file.readText())
                .associate { it.groupValues[1] to it.groupValues[2].replace("\\'", "'").replace("\\\"", "\"").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">") }
        }
        for ((zh, directory) in listOf(false to "values-en", true to "values")) {
            val res = resources(directory)
            val want = TeamsVectors.strings(zh)
            assertEquals(want.channel, res["teams_conversation_channel"]); assertEquals(want.directMessage, res["teams_label_direct"])
            assertEquals(want.group, res["teams_conversation_group"]); assertEquals(want.member, res["teams_member"])
            assertEquals(want.you, res["teams_preview_you"]); assertEquals(want.noMessages, res["teams_preview_none"])
            assertEquals(want.recalled, res["teams_message_recalled"]); assertEquals(want.nonText, res["teams_preview_non_text"])
            assertEquals("Bot", want.bot)
            assertEquals(want.listSeparator, if (zh) TeamsListStrings.ZH_LIST_SEPARATOR else ", ")
            assertEquals(want.authorSeparator, if (zh) TeamsListStrings.ZH_AUTHOR_SEPARATOR else ": ")
        }
        assertEquals(TeamsVectors.strings(false), TeamsListStrings.ENGLISH)
        assertTrue(TeamsListStrings.isChinese("zh-Hans-CN")); assertFalse(TeamsListStrings.isChinese("en-US"))
    }
}
