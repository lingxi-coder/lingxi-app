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
        assertEquals(listOf("shared_bot", "user"), own.map { it.kind }); assertEquals("sharedbot_helper", own.first().id); assertEquals("Bot", own.first().name); assertTrue(own.last().self)
    }

    @Test fun findDmAndFindBotDmOnlyMatchTheViewersOwnConversations() {
        val byId = TeamsVectors.conversations().associateBy { it.id }
        val finds = root.getJSONObject("finds")
        assertEquals(finds.getString("botDm"), TeamsListLogic.findBotDm(listOf(byId.getValue("bd2"), byId.getValue("bd1")), uid, "sharedbot_helper")?.id)
        assertEquals(finds.getString("dm"), TeamsListLogic.findDm(listOf(byId.getValue("dm1"), byId.getValue("grp")), uid, "u2")?.id)
        assertNull(TeamsListLogic.findDm(listOf(byId.getValue("dm1")), uid, "u3"))
        assertTrue(finds.isNull("dmMissing"))
    }

    /** M3-3: a Bot DM keeps the readers it was made with, so one from before a handover is a record and the Bot answers in a new one. */
    @Test fun aBotDmFromBeforeItsBotChangedMaintainerIsNotTheCurrentOne() {
        val maintainer = bots.first { it.sharedBotId == "sharedbot_helper" }.maintainer.uid
        fun dm(id: String, readers: List<String>?) = TeamMessageConversation(id, "bot_dm", "", listOf("user:$uid", "shared_bot:sharedbot_helper"), 0, 0, 0, listOf("sharedbot_helper"), readers = readers)
        val before = dm("before", listOf(uid, "earlier_maintainer")); val after = dm("after", listOf(uid, maintainer)); val unknown = dm("unknown", null)
        assertEquals("after", TeamsListLogic.findBotDm(listOf(before, after), uid, "sharedbot_helper", bots)?.id)
        assertNull(TeamsListLogic.findBotDm(listOf(before), uid, "sharedbot_helper", bots))
        assertEquals("before", TeamsListLogic.findBotDm(listOf(before), uid, "sharedbot_helper")?.id)
        assertEquals("unknown", TeamsListLogic.findBotDm(listOf(unknown), uid, "sharedbot_helper", bots)?.id)
        fun lock(c: TeamMessageConversation) = TeamsComposeLogic.composerLock(uid, c.kind, c.participants, bots, botsLoaded = true, botsUnverified = false, readers = c.readers)
        assertEquals(ComposerLock.BotHandedOver, lock(before))
        assertFalse(lock(after) == ComposerLock.BotHandedOver); assertFalse(lock(unknown) == ComposerLock.BotHandedOver)
        // The new maintainer's own earlier chat had the earlier maintainer as a reader: it is not the current one either.
        assertTrue(TeamsListLogic.botDmHandedOver("bot_dm", listOf("user:$maintainer", "shared_bot:sharedbot_helper"), listOf(maintainer, "earlier_maintainer"), bots))
        assertFalse(TeamsListLogic.botDmHandedOver("group", before.participants, before.readers, bots))
        // The stored readers are read from the conversation document.
        val parsed = TeamMessageConversation.fromDocument("c1", mapOf("kind" to "bot_dm", "title" to "", "participants" to listOf("user:$uid", "shared_bot:sharedbot_helper"),
            "memberUids" to listOf(uid, "earlier_maintainer"), "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null), 0) { null }
        assertEquals(listOf(uid, "earlier_maintainer"), parsed.readers)
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
                    TeamLastMessage(value.getString("id"), value.getLong("seq"), TeamsVectors.parseInstant(value.stringOrNull("at")), author.getString("kind"), author.optString("uid", ""), author.stringOrNull("sharedBotId"), value.getString("preview"), value.optBoolean("recalled"))
                }
            }
            val conversation = TeamMessageConversation("c", row.getString("kind"), "t", emptyList(), 1, 1, 0, lastMessage = pointer)
            val text = TeamsListLogic.conversationPreview(conversation, uid, TeamsVectors.strings(row.getBoolean("zh"))) { last -> if (last.authorKind == "shared_bot") names.getString("bot") else names.optString(last.authorUid, last.authorUid) }
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
            val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml", "apps/android/native/app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
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

    /** A shared Bot's last message writes Markdown, so its preview is plain text; a person's stays exactly as stored. */
    @Test fun onlyABotsPreviewIsPlainText() {
        val strings = TeamsListStrings.ENGLISH
        fun last(kind: String, preview: String, recalled: Boolean = false) = TeamLastMessage("m", 1, 1, kind, if (kind == "user") "u2" else "", if (kind == "shared_bot") "b" else null, preview, recalled)
        val markdown = "# QA Follow-Up Check\n\n**Goal: close the open items.**\n\n- Scope: defects"
        assertEquals("QA Follow-Up Check Goal: close the open items. Scope: defects", TeamsListLogic.pointerPreview(last("shared_bot", markdown), uid, strings))
        assertEquals("Fixture Bot: QA Follow-Up Check", TeamsListLogic.pointerPreview(last("shared_bot", "# QA Follow-Up Check"), uid, strings) { "Fixture Bot" })
        assertEquals("2 * 3 * 4 # keep", TeamsListLogic.pointerPreview(last("user", "2 * 3 * 4 # keep"), uid, strings))
        assertEquals(markdown, TeamsListLogic.pointerPreview(last("user", markdown), uid, strings))
        assertEquals(strings.nonText, TeamsListLogic.pointerPreview(last("shared_bot", "---"), uid, strings))
        assertEquals(strings.nonText, TeamsListLogic.pointerPreview(last("shared_bot", ""), uid, strings))
        assertEquals(strings.recalled, TeamsListLogic.pointerPreview(last("shared_bot", "**x**", recalled = true), uid, strings))
        val group = TeamMessageConversation("g", "group", "", listOf("user:$uid", "user:u2"), 1, 1, 0, emptyList(), last("shared_bot", "**G"))
        assertEquals("Bot: G", TeamsListLogic.conversationPreview(group, uid, strings) { "Bot" })
    }
}
