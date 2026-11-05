package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.time.ZoneId
import java.time.ZoneOffset
import java.util.Locale

/** SPEC-mobile-team-bot-ui section 8.2: rows, windowing and the two time classifications, on the Desktop `stream.ts` vectors. */
class TeamsStreamLogicTest {
    private val utc: ZoneId = ZoneOffset.UTC
    private val now = TeamsVectors.NOW_MS
    private val uid = TeamsVectors.UID
    private val stream get() = TeamsVectors.root.getJSONObject("stream")

    private fun message(json: JSONObject) = StreamMessage(
        json.getString("id"), json.getLong("seq"), json.getString("authorKind"), json.getString("authorUid"), json.stringOrNull("authorBotKey"),
        if (json.isNull("createdAtMs")) null else json.getLong("createdAtMs"), json.getBoolean("recalled"),
    )
    private fun messages(array: JSONArray) = array.objects().map(::message)

    /** The vectors' row notation: `date:<day>:today|yesterday|other`, `unread`, `message:<id>:head|grouped`. */
    private fun render(rows: List<StreamRow>): List<String> = rows.map { row ->
        when (row) {
            is StreamRow.Date -> row.key + ":" + when (row.kind) { DayKind.Today -> "today"; DayKind.Yesterday -> "yesterday"; DayKind.DateThisYear, DayKind.DateOtherYear -> "other" }
            StreamRow.Unread -> "unread"
            is StreamRow.Message -> "message:${row.message.id}:${if (row.grouped) "grouped" else "head"}"
        }
    }
    private fun expected(array: JSONArray) = TeamsVectors.strings(array)

    @Test fun theGroupingWindowIsFiveMinutes() {
        assertEquals(stream.getLong("groupWindowMs"), TeamsStreamLogic.GROUP_WINDOW_MS)
    }

    @Test fun rowsForTheMainConversationMatchTheDesktopWithAndWithoutAnUnreadWatermark() {
        val list = messages(stream.getJSONArray("messages"))
        assertEquals(10, list.size)
        assertEquals(expected(stream.getJSONArray("expectRows")), render(TeamsStreamLogic.buildStreamRows(list, stream.getLong("lastReadSeq"), uid, now, utc)))
        assertEquals(expected(stream.getJSONArray("expectRowsAllRead")), render(TeamsStreamLogic.buildStreamRows(list, 10, uid, now, utc)))
        assertEquals(stream.getString("firstUnreadId"), TeamsStreamLogic.firstUnreadMessage(list, stream.getLong("lastReadSeq"), uid)?.id)
        assertNull(TeamsStreamLogic.firstUnreadMessage(list, 10, uid))
    }

    @Test fun yourOwnMessagesNeverAnchorTheDivider() {
        val own = stream.getJSONObject("ownOnly")
        val list = messages(own.getJSONArray("messages"))
        assertEquals(expected(own.getJSONArray("expectRows")), render(TeamsStreamLogic.buildStreamRows(list, own.getLong("lastReadSeq"), uid, now, utc)))
        assertNull(TeamsStreamLogic.firstUnreadMessage(list, own.getLong("lastReadSeq"), uid))
    }

    @Test fun everyFurtherStreamCaseMatchesTheDesktop() {
        val cases = stream.getJSONArray("more").objects()
        assertEquals(7, cases.size)
        cases.forEach { case ->
            assertEquals(case.getString("name"), expected(case.getJSONArray("expectRows")),
                render(TeamsStreamLogic.buildStreamRows(messages(case.getJSONArray("messages")), case.getLong("lastReadSeq"), case.getString("uid"), now, utc)))
        }
    }

    @Test fun aRecalledMessageStartsAFreshRunForTheNextOne() {
        val list = messages(stream.getJSONArray("messages"))
        val rows = TeamsStreamLogic.buildStreamRows(list, 10, uid, now, utc).filterIsInstance<StreamRow.Message>().associateBy { it.message.id }
        assertEquals(true, rows.getValue("m7").grouped)
        assertEquals(false, rows.getValue("m8").grouped)
    }

    @Test fun dayBoundariesFollowTheGivenZoneNotUtc() {
        // 2026-11-01T23:30Z is "yesterday" in UTC but already 07:30 on the current day in Shanghai (now = 18:00 there).
        val late = 1793575800000L
        val shanghai = ZoneId.of("Asia/Shanghai")
        assertEquals(DayKind.Yesterday, TeamsStreamLogic.dayLabelKind(late, now, utc))
        assertEquals(DayKind.Today, TeamsStreamLogic.dayLabelKind(late, now, shanghai))
        assertEquals(ListTimeKind.Yesterday, TeamsStreamLogic.listTimeKind(late, now, utc))
        assertEquals(ListTimeKind.Clock, TeamsStreamLogic.listTimeKind(late, now, shanghai))
        val row = TeamsStreamLogic.buildStreamRows(listOf(StreamMessage("a", 1, "user", "u2", null, late, false)), 1, uid, now, shanghai).first() as StreamRow.Date
        assertEquals("date:2026-11-02", row.key)
    }

    @Test fun visibleStartMatchesTheDesktopWindowRules() {
        val section = TeamsVectors.root.getJSONObject("visibleStart")
        val constants = section.getJSONObject("constants")
        assertEquals(constants.getInt("WINDOW_SIZE"), TeamsStreamLogic.WINDOW_SIZE)
        assertEquals(constants.getInt("WINDOW_MAX"), TeamsStreamLogic.WINDOW_MAX)
        assertEquals(constants.getInt("UNREAD_CONTEXT"), TeamsStreamLogic.UNREAD_CONTEXT)
        val cases = section.getJSONArray("cases").objects()
        assertEquals(6, cases.size)
        cases.forEach { case ->
            // "n1..n150 (150, alternating)": odd seq = u2, even = u1; "c1..c600 (600, all u2)": everything u2.
            val label = case.getString("messages")
            val count = Regex("\\((\\d+),").find(label)!!.groupValues[1].toInt()
            val list = (1..count).map { n ->
                val other = label.startsWith("c") || n % 2 == 1
                StreamMessage((if (label.startsWith("c")) "c" else "n") + n, n.toLong(), "user", if (other) "u2" else "u1", null, null, false)
            }
            val startId = case.stringOrNull("startId")
            assertEquals("$label / ${case.getLong("lastReadSeq")} / $startId", case.getInt("expect"), TeamsStreamLogic.visibleStart(list, startId, case.getLong("lastReadSeq"), uid))
        }
    }

    @Test fun visibleStartNeverPassesTheHardCapUnlessTheReaderAskedForOlderMessages() {
        val list = (1..600).map { StreamMessage("c$it", it.toLong(), "user", "u2", null, null, false) }
        assertEquals(200, TeamsStreamLogic.visibleStart(list, null, 0, uid))
        assertEquals(10, TeamsStreamLogic.visibleStart(list, "c11", 0, uid))
        assertEquals(500, TeamsStreamLogic.visibleStart(list, null, 600, uid))
        assertEquals(500, TeamsStreamLogic.visibleStart(list, "missing", 600, uid))
        assertEquals(0, TeamsStreamLogic.visibleStart(emptyList(), null, 0, uid))
    }

    @Test fun listTimeKindsMatchTheDesktop() {
        val cases = TeamsVectors.root.getJSONArray("listTimeKinds").objects()
        assertEquals(9, cases.size)
        cases.forEach { case ->
            val at = TeamsVectors.parseInstant(case.stringOrNull("at"))
            assertEquals("${case.opt("at")}", case.getString("expect"), TeamsStreamLogic.listTimeKind(at, now, utc).name.replaceFirstChar { it.lowercase() })
        }
    }

    @Test fun dayLabelKindsMatchTheDesktop() {
        val cases = TeamsVectors.root.getJSONArray("dayLabelKinds").objects()
        assertEquals(4, cases.size)
        cases.forEach { case ->
            val at = TeamsVectors.parseInstant(case.getString("at"))!!
            assertEquals(case.getString("at"), case.getString("expect"), TeamsStreamLogic.dayLabelKind(at, now, utc).name.replaceFirstChar { it.lowercase() })
        }
    }

    @Test fun theClockIsShortAndEmptyWithoutATime() {
        assertEquals("", TeamsStreamLogic.formatClock(null, utc, Locale.US))
        // JDK 20+ separates the day period with a narrow no-break space; older JDKs use a plain one.
        assertEquals("8:05 AM", TeamsStreamLogic.formatClock(1793606700000L, utc, Locale.US).replace(' ', ' '))
    }

    @Test fun aStreamMessageTakesTheBotKeyOnlyFromBotAuthors() {
        val bot = TeamMessage("m", 1, "u5", "bot", "hi", 1, "c", false, null, null, botKey = "bot:k", createdAt = "2026-11-02T08:00:00Z")
        val user = bot.copy(authorKind = "user", authorUid = "u2")
        assertEquals("bot:k", StreamMessage.of(bot).authorBotKey)
        assertNull(StreamMessage.of(user).authorBotKey)
        assertEquals(1793606400000L, StreamMessage.of(bot).createdAtMs)
    }
}
