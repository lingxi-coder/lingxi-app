package com.lingxi.code.teams

import com.lingxi.code.tabs.objects
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.time.Instant

/** The 27 `lastMessage` pointer cases of SPEC-mobile-team-bot-ui section 8.4, executed on the real Desktop parser. */
class TeamsPointerParserTest {
    /** Stands in for a Firestore `Timestamp`, which the pure parser must not know about. */
    private data class FakeTimestamp(val ms: Long)

    private fun convert(value: Any?): Any? = when (value) {
        JSONObject.NULL, null -> null
        is JSONObject -> if (value.has("\$ts")) FakeTimestamp(Instant.parse(value.getString("\$ts")).toEpochMilli()) else value.keys().asSequence().associateWith { convert(value.get(it)) }
        is JSONArray -> (0 until value.length()).map { convert(value.get(it)) }
        else -> value
    }
    private val millis = { value: Any -> (value as? FakeTimestamp)?.ms }

    @Test fun everyVectorCaseIsAcceptedOrRejectedExactlyLikeTheDesktop() {
        val cases = TeamsVectors.root.getJSONArray("lastMessage").objects()
        assertEquals(27, cases.size)
        for (case in cases) {
            val name = case.getString("name")
            val input = case.get("input")
            val absent = input is JSONObject && input.has("\$absent")
            val expect = case.getJSONObject("expect")
            if (expect.has("error")) {
                try {
                    TeamsPointerParser.parseLastMessage(!absent, if (absent) null else convert(input), millis)
                    fail("$name should be INVALID_DATA")
                } catch (error: TeamMessageFailure) { assertEquals(name, expect.getString("error"), error.code) }
                continue
            }
            val parsed = TeamsPointerParser.parseLastMessage(true, convert(input), millis)
            val ok = expect.get("ok")
            if (ok === JSONObject.NULL) { assertNull(name, parsed); continue }
            val want = ok as JSONObject
            val got = requireNotNull(parsed) { name }
            assertEquals(name, want.getString("id"), got.id)
            assertEquals(name, want.getLong("seq"), got.seq)
            assertEquals(name, if (want.isNull("atMs")) null else want.getLong("atMs"), got.atMs)
            assertEquals(name, want.getString("authorKind"), got.authorKind)
            assertEquals(name, want.getString("authorUid"), got.authorUid)
            assertEquals(name, if (want.isNull("authorSharedBotId")) null else want.getString("authorSharedBotId"), got.authorSharedBotId)
            assertEquals(name, want.getString("preview"), got.preview)
            assertEquals(name, want.getBoolean("recalled"), got.recalled)
        }
    }

    @Test fun anAbsentFieldIsInvalidButAnExplicitNullIsNoMessage() {
        assertNull(TeamsPointerParser.parseLastMessage(true, null, millis))
        assertTrue(runCatching { TeamsPointerParser.parseLastMessage(false, null, millis) }.exceptionOrNull() is TeamMessageFailure)
    }

    @Test fun aFirestoreDoubleWithAnIntegralValueIsASequenceNumberButNothingElseNumericIs() {
        val base = mapOf("id" to "m", "at" to null, "preview" to "", "author" to mapOf("kind" to "user", "uid" to "u"))
        assertEquals(7L, TeamsPointerParser.parseLastMessage(true, base + ("seq" to 7.0), millis)!!.seq)
        assertEquals(7L, TeamsPointerParser.parseLastMessage(true, base + ("seq" to 7L), millis)!!.seq)
        for (bad in listOf<Any>(7.5, "7", Double.NaN, Double.POSITIVE_INFINITY, 9007199254740992L, true)) {
            assertTrue("$bad", runCatching { TeamsPointerParser.parseLastMessage(true, base + ("seq" to bad), millis) }.exceptionOrNull() is TeamMessageFailure)
        }
    }

    @Test fun aWholeConversationDocumentIsParsedStrictlyAndKeepsItsBotKeys() {
        val pointer = mapOf("id" to "m", "seq" to 4L, "at" to FakeTimestamp(5), "preview" to "hi", "author" to mapOf("kind" to "shared_bot", "uid" to "u", "sharedBotId" to "sharedbot_b"))
        val data = mapOf("kind" to "bot_dm", "title" to "Helper", "participants" to listOf("user:u1", "shared_bot:sharedbot_b"), "lastSeq" to 4L, "lastChangeSeq" to 4L, "lastMessage" to pointer)
        val parsed = TeamMessageConversation.fromDocument("c1", data, 2, millis)
        assertEquals(listOf("sharedbot_b"), parsed.sharedBotIds); assertEquals("sharedbot_b", parsed.sharedBotId)
        assertEquals(2L, parsed.lastReadSeq); assertEquals(5L, parsed.lastMessage!!.atMs); assertEquals("shared_bot", parsed.lastMessage.authorKind)
        for (broken in listOf(data - "lastMessage", data - "kind", data - "title", data - "participants", data - "lastSeq", data - "lastChangeSeq", data + ("participants" to listOf(1)), data + ("lastSeq" to "4"))) {
            val failure = runCatching { TeamMessageConversation.fromDocument("c1", broken, 0, millis) }.exceptionOrNull()
            assertEquals(broken.toString(), "INVALID_DATA", (failure as? TeamMessageFailure)?.code)
        }
        assertNull(TeamMessageConversation.fromDocument("c1", data + ("lastMessage" to null), 0, millis).lastMessage)
    }
}
