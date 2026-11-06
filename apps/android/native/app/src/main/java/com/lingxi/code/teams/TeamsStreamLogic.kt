package com.lingxi.code.teams

import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/** What the stream needs to know about a message (SPEC-mobile-team-bot-ui section 8.2). */
internal data class StreamMessage(val id: String, val seq: Long, val authorKind: String, val authorUid: String, val authorSharedBotId: String?, val createdAtMs: Long?, val recalled: Boolean) {
    companion object {
        fun of(message: TeamMessage) = StreamMessage(message.id, message.seq, message.authorKind, message.authorUid, message.sharedBotId.takeIf { message.authorKind == "shared_bot" }, message.createdAtMs, message.recalled)
    }
}

internal enum class DayKind { Today, Yesterday, DateThisYear, DateOtherYear }
internal enum class ListTimeKind { None, Clock, Yesterday, Weekday, DateThisYear, DateOtherYear }

internal sealed interface StreamRow {
    val key: String
    /** A day separator; [atMs] is the first timed message of that day. */
    data class Date(override val key: String, val atMs: Long, val kind: DayKind) : StreamRow
    /** The "New messages" divider before the first unread message written by someone else. */
    data object Unread : StreamRow { override val key = "unread" }
    /** [grouped] = continues the previous message's run: no repeated avatar or name. */
    data class Message(val message: StreamMessage, val grouped: Boolean) : StreamRow { override val key get() = "message:${message.id}" }
}

/** Mirrors `packages/teams/stream.ts`. `now` and the zone are parameters; nothing reads the clock or the locale. */
internal object TeamsStreamLogic {
    const val WINDOW_SIZE = 100
    const val WINDOW_MAX = 400
    const val UNREAD_CONTEXT = 3
    const val GROUP_WINDOW_MS = 300_000L

    /** The first message after the watermark that someone else wrote: where the divider goes. Own messages never anchor it. */
    fun firstUnreadMessage(messages: List<StreamMessage>, lastReadSeq: Long, uid: String): StreamMessage? =
        messages.firstOrNull { it.seq > lastReadSeq && !(it.authorKind == "user" && it.authorUid == uid) }

    fun buildStreamRows(messages: List<StreamMessage>, lastReadSeq: Long, uid: String, nowMs: Long, zone: ZoneId): List<StreamRow> {
        val anchor = firstUnreadMessage(messages, lastReadSeq, uid)
        val rows = ArrayList<StreamRow>(messages.size + 8)
        var previous: StreamMessage? = null
        var previousTime: Long? = null
        var lastDay: LocalDate? = null
        for (message in messages) {
            val time = message.createdAtMs
            val day = time?.let { dayOf(it, zone) }
            val startsDay = day != null && day != lastDay
            if (startsDay) rows += StreamRow.Date("date:$day", time, dayLabelKind(time, nowMs, zone))
            val unread = message === anchor
            if (unread) rows += StreamRow.Unread
            val grouped = previous != null && !startsDay && !unread && !previous.recalled && sameAuthor(previous, message) &&
                time != null && previousTime != null && Math.abs(time - previousTime) <= GROUP_WINDOW_MS
            rows += StreamRow.Message(message, grouped)
            previous = message
            previousTime = time
            if (day != null) lastDay = day
        }
        return rows
    }

    /**
     * Where the rendered window starts: the newest [size] messages, widened back to the first unread (with [UNREAD_CONTEXT] above it)
     * but never past [max]; [startId] (the oldest message the reader asked to see) extends it without that limit.
     */
    fun visibleStart(messages: List<StreamMessage>, startId: String?, lastReadSeq: Long, uid: String, size: Int = WINDOW_SIZE, max: Int = WINDOW_MAX): Int {
        var start = maxOf(0, messages.size - size)
        val unread = firstUnreadMessage(messages, lastReadSeq, uid)
        if (unread != null) start = minOf(start, maxOf(0, messages.indexOf(unread) - UNREAD_CONTEXT))
        start = maxOf(start, messages.size - max, 0)
        val asked = if (startId == null) -1 else messages.indexOfFirst { it.id == startId }
        return if (asked >= 0) minOf(start, asked) else start
    }

    fun dayLabelKind(atMs: Long, nowMs: Long, zone: ZoneId): DayKind {
        val day = dayOf(atMs, zone)
        val today = dayOf(nowMs, zone)
        return when {
            day == today -> DayKind.Today
            day == today.minusDays(1) -> DayKind.Yesterday
            day.year == today.year -> DayKind.DateThisYear
            else -> DayKind.DateOtherYear
        }
    }

    /** Days 2 to 6 back are weekdays (seven days back would repeat today's name); a missing time is [ListTimeKind.None]. */
    fun listTimeKind(atMs: Long?, nowMs: Long, zone: ZoneId): ListTimeKind {
        if (atMs == null) return ListTimeKind.None
        val day = dayOf(atMs, zone)
        val today = dayOf(nowMs, zone)
        return when {
            day == today -> ListTimeKind.Clock
            day == today.minusDays(1) -> ListTimeKind.Yesterday
            !day.isBefore(today.minusDays(6)) && day.isBefore(today) -> ListTimeKind.Weekday
            day.year == today.year -> ListTimeKind.DateThisYear
            else -> ListTimeKind.DateOtherYear
        }
    }

    /** Short time of day in the user's locale and the given zone; empty when there is no time. */
    fun formatClock(atMs: Long?, zone: ZoneId = ZoneId.systemDefault(), locale: Locale = Locale.getDefault()): String =
        if (atMs == null) "" else DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale).withZone(zone).format(Instant.ofEpochMilli(atMs))

    private fun dayOf(ms: Long, zone: ZoneId): LocalDate = Instant.ofEpochMilli(ms).atZone(zone).toLocalDate()
    private fun sameAuthor(a: StreamMessage, b: StreamMessage) = a.authorKind == b.authorKind && a.authorUid == b.authorUid && (a.authorSharedBotId ?: "") == (b.authorSharedBotId ?: "")
}
