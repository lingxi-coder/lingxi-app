package com.lingxi.code.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pure-JVM checks for [SessionCatalog] — the wire-dto → [SessionRow] mapping's
 * relative-time humanizer + its hand-rolled RFC 3339 parser (no `java.time`).
 * Anchored on a fixed reference "now" so every bucket is deterministic.
 */
class SessionCatalogTest {

    @Test
    fun `legacy display-prefixed session id is canonicalized`() {
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        assertEquals(uuid, canonicalSessionId("sess:$uuid"))
        assertEquals(uuid, canonicalSessionId(uuid.uppercase()))
        assertEquals("sess:not-a-uuid", canonicalSessionId("sess:not-a-uuid"))
    }

    // 2024-06-15T12:00:00Z as seconds since epoch (the reference "now").
    private val now = SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00Z")!!

    private fun rel(modified: String) = SessionCatalog.relativeTime(modified, now)

    // --- relative-time buckets --------------------------------------------

    @Test fun justNow_underAMinute() = assertEquals("刚刚", rel("2024-06-15T11:59:30Z"))

    @Test fun minutesAgo() = assertEquals("5 分钟前", rel("2024-06-15T11:55:00Z"))

    @Test fun hoursAgo() = assertEquals("3 小时前", rel("2024-06-15T09:00:00Z"))

    @Test fun yesterday() = assertEquals("昨天", rel("2024-06-14T10:00:00Z"))

    @Test fun daysAgo() = assertEquals("4 天前", rel("2024-06-11T12:00:00Z"))

    @Test fun overAWeek_fallsBackToMonthDay() =
        // 2024-05-03 is > 7 days before the reference → "M月D日".
        assertEquals("5月3日", rel("2024-05-03T08:00:00Z"))

    @Test fun futureStamp_clampsToJustNow() =
        assertEquals("刚刚", rel("2024-06-16T12:00:00Z"))

    @Test fun unparseable_fallsBackToJustNow() {
        assertEquals("刚刚", rel("not-a-timestamp"))
        assertEquals("刚刚", rel(""))
    }

    // --- the RFC 3339 parser ----------------------------------------------

    @Test fun parsesUtcZulu_toEpochSeconds() {
        // Epoch itself.
        assertEquals(0L, SessionCatalog.parseRfc3339ToEpochSeconds("1970-01-01T00:00:00Z"))
        // A known instant: 2024-06-15T12:00:00Z = 1718452800.
        assertEquals(1_718_452_800L, SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00Z"))
    }

    @Test fun parser_toleratesFractionalSecondsAndSpaceSeparator() {
        // A fractional part after the seconds is ignored (only the prefix is read).
        assertEquals(
            SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00Z"),
            SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00.512Z"),
        )
        // A space date/time separator is tolerated.
        assertEquals(
            SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15T12:00:00Z"),
            SessionCatalog.parseRfc3339ToEpochSeconds("2024-06-15 12:00:00Z"),
        )
    }

    @Test fun parser_rejectsMalformed_returnsNull() {
        assertNull(SessionCatalog.parseRfc3339ToEpochSeconds("2024/06/15T12:00:00Z"))
        assertNull(SessionCatalog.parseRfc3339ToEpochSeconds("short"))
        assertNull(SessionCatalog.parseRfc3339ToEpochSeconds("2024-13-15T12:00:00Z")) // month 13
    }

    // --- rowFrom: the full wire-field mapping ------------------------------

    @Test fun rowFrom_mapsAllFields_andHumanizesTime() {
        val row = SessionCatalog.rowFrom(
            uuid = "abc-123",
            title = "上海差旅规划",
            messageCount = 17,
            modifiedRfc3339 = "2024-06-15T11:00:00Z",
            nowEpochSeconds = now,
        )
        assertEquals("abc-123", row.uuid)
        assertEquals("上海差旅规划", row.title)
        assertEquals(17, row.messageCount)
        assertEquals("1 小时前", row.relativeTime)
    }

    @Test fun rowFrom_blankTitle_usesPlaceholder() {
        val row = SessionCatalog.rowFrom("u", "", 0, "2024-06-15T12:00:00Z", now)
        assertEquals("未命名会话", row.title)
    }
}
