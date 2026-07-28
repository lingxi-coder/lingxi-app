package com.lingxi.code.model

/**
 * The engine's REAL resumable-session catalog — the SEPARATE, out-of-band
 * session-state path, the exact sibling of [EngineModelState] for the model
 * catalog. `SessionList` (the reply to `ListSessions`) is NOT part of a text
 * turn, so it flows through `ConversationSource.sessionState` (a `StateFlow`)
 * rather than the per-turn reply stream — mirroring how `ModelList` rides
 * `ConversationSource.modelState`.
 *
 * [phase] distinguishes "still loading", "loaded but empty", and "could not
 * load". This keeps production UI from falling back to branded mock sessions
 * when the engine has not replied yet or is unavailable.
 */
enum class SessionCatalogPhase {
    Loading,
    Ready,
    Error,
}

data class EngineSessionState(
    val rows: List<SessionRow> = emptyList(),
    val phase: SessionCatalogPhase = SessionCatalogPhase.Loading,
    val errorMessage: String? = null,
) {
    val hasSessions: Boolean get() = rows.isNotEmpty()
    val isLoading: Boolean get() = phase == SessionCatalogPhase.Loading
    val isEmpty: Boolean get() = phase == SessionCatalogPhase.Ready && rows.isEmpty()
    val isError: Boolean get() = phase == SessionCatalogPhase.Error

    companion object {
        fun loading(): EngineSessionState = EngineSessionState(
            rows = emptyList(),
            phase = SessionCatalogPhase.Loading,
            errorMessage = null,
        )

        fun ready(rows: List<SessionRow>): EngineSessionState = EngineSessionState(
            rows = rows,
            phase = SessionCatalogPhase.Ready,
            errorMessage = null,
        )

        fun error(message: String): EngineSessionState = EngineSessionState(
            rows = emptyList(),
            phase = SessionCatalogPhase.Error,
            errorMessage = message,
        )
    }
}

/**
 * One drawer-facing resumable-session row — the UI projection of the engine's
 * wire `SessionRowDto` (`uuid` / `title` / `modified_rfc3339` / `message_count`
 * / `path`). [uuid] is the verbatim wire id (what `ResumeSession` sends);
 * [relativeTime] is the humanized [modifiedRfc3339] computed at map time by
 * [SessionCatalog.rowFromDto] so the row carries a display string and never the
 * raw timestamp.
 *
 * PURE (no engine / Android types) so the drawer's real-session rendering and
 * its search filter are unit-testable on the plain JVM.
 */
data class SessionRow(
    val uuid: String,
    val title: String,
    val messageCount: Int,
    /** A short, human relative time ("刚刚" / "5 分钟前" / "3 天前" / "5月3日"). */
    val relativeTime: String,
)

/**
 * Maps the engine's wire `SessionRowDto` onto the drawer-facing [SessionRow]
 * and humanizes an RFC 3339 timestamp into a short relative time. PURE — no
 * engine / Android dependency — so the mapping + the relative-time buckets are
 * exhaustively unit-testable on the plain JVM (where the generated UniFFI
 * bindings are unavailable). The drawer's `SessionList` folding (in
 * `EngineConversationSource`) calls [rowFrom] with the wire fields directly so
 * this file never names a binding type.
 */
object SessionCatalog {

    /**
     * Build a [SessionRow] from the wire fields of a `SessionRowDto`. Taken as
     * primitives (not the binding type) so this stays a pure-JVM function: the
     * `EngineConversationSource` listener unpacks the DTO and calls this.
     *
     * @param nowEpochSeconds the reference "now" (seconds since epoch) the
     *   relative time is computed against; defaults to the system clock.
     */
    fun rowFrom(
        uuid: String,
        title: String,
        messageCount: Int,
        modifiedRfc3339: String,
        nowEpochSeconds: Long = System.currentTimeMillis() / 1000L,
    ): SessionRow = SessionRow(
        uuid = uuid,
        title = title.ifBlank { "未命名会话" },
        messageCount = messageCount,
        relativeTime = relativeTime(modifiedRfc3339, nowEpochSeconds),
    )

    /**
     * Humanize an RFC 3339 timestamp into a short, Chinese relative-time label,
     * matching the prototype's drawer cadence (刚刚 / N 分钟前 / N 小时前 /
     * 昨天 / N 天前 / M月D日). A timestamp that can't be parsed (or one in the
     * future) falls back to "刚刚" so a row never renders a blank or a raw
     * ISO string.
     *
     * Parses only the calendar/clock fields out of the RFC 3339 string (the
     * shape `client_adapter::system_time_to_rfc3339` emits — UTC,
     * `YYYY-MM-DDТHH:MM:SS[.fff]Z`) with a tiny hand-rolled scan so this needs
     * no `java.time` (kept off the desugaring path) and stays pure JVM.
     */
    fun relativeTime(modifiedRfc3339: String, nowEpochSeconds: Long): String {
        val thenEpoch = parseRfc3339ToEpochSeconds(modifiedRfc3339) ?: return "刚刚"
        val delta = nowEpochSeconds - thenEpoch
        return when {
            delta < 0L -> "刚刚" // clock skew / future stamp
            delta < 60L -> "刚刚"
            delta < 3600L -> "${delta / 60L} 分钟前"
            delta < 86_400L -> "${delta / 3600L} 小时前"
            delta < 172_800L -> "昨天"
            delta < 604_800L -> "${delta / 86_400L} 天前"
            else -> monthDay(modifiedRfc3339) ?: "${delta / 86_400L} 天前"
        }
    }

    /**
     * Parse the RFC 3339 (UTC `Z`) calendar fields to seconds-since-epoch with a
     * proleptic-Gregorian day count — no `java.time`. Returns `null` for any
     * shape it can't read (so the caller falls back gracefully). Only the
     * `YYYY-MM-DDТHH:MM:SS` prefix is read; a fractional part / `Z` suffix is
     * ignored. A non-`Z` offset is treated as UTC (the lowering only ever emits
     * `Z`), which at the relative-time granularity here is harmless.
     */
    internal fun parseRfc3339ToEpochSeconds(s: String): Long? {
        // Expect at least "YYYY-MM-DDТHH:MM:SS" (19 chars).
        if (s.length < 19) return null
        val year = s.substring(0, 4).toIntOrNull() ?: return null
        if (s[4] != '-' || s[7] != '-') return null
        val month = s.substring(5, 7).toIntOrNull() ?: return null
        val day = s.substring(8, 10).toIntOrNull() ?: return null
        // The date/time separator is 'T' (or a space, tolerated).
        if (s[10] != 'T' && s[10] != 't' && s[10] != ' ') return null
        if (s[13] != ':' || s[16] != ':') return null
        val hour = s.substring(11, 13).toIntOrNull() ?: return null
        val minute = s.substring(14, 16).toIntOrNull() ?: return null
        val second = s.substring(17, 19).toIntOrNull() ?: return null
        if (month !in 1..12 || day !in 1..31) return null
        val days = daysFromCivil(year, month, day)
        return days * 86_400L + hour * 3600L + minute * 60L + second
    }

    /** "M月D日" for the timestamp, or `null` if it can't be parsed. */
    private fun monthDay(s: String): String? {
        if (s.length < 10) return null
        val month = s.substring(5, 7).toIntOrNull() ?: return null
        val day = s.substring(8, 10).toIntOrNull() ?: return null
        if (month !in 1..12 || day !in 1..31) return null
        return "${month}月${day}日"
    }

    /**
     * Days from 1970-01-01 to the given proleptic-Gregorian date (Howard
     * Hinnant's `days_from_civil` algorithm). Pure integer arithmetic, valid for
     * the full range of dates a session file could carry.
     */
    private fun daysFromCivil(year: Int, month: Int, day: Int): Long {
        val y = if (month <= 2) year - 1 else year
        val era = (if (y >= 0) y else y - 399) / 400
        val yoe = (y - era * 400).toLong() // [0, 399]
        val doy = ((153 * (if (month > 2) month - 3 else month + 9) + 2) / 5 + day - 1).toLong()
        val doe = yoe * 365 + yoe / 4 - yoe / 100 + doy // [0, 146096]
        return era.toLong() * 146_097L + doe - 719_468L
    }
}
