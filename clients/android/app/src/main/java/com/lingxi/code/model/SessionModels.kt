package com.lingxi.code.model

import android.content.Context
import com.lingxi.code.R

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

/**
 * Resolves a string resource id to its localized text for [SessionCatalog].
 * [fallback] is always the exact zh-Hans base-locale copy for [id], passed in
 * at the call site right next to the resource id. [DefaultSessionCatalogStrings]
 * (the parameter default everywhere this is threaded through) returns
 * [fallback] verbatim — formatted with [args] when present — which is why
 * [SessionCatalogTest] (a pure-JVM test with no Android `Context` at all) keeps
 * asserting the same literal Chinese relative-time copy without being touched.
 * The production implementation ([sessionCatalogStrings]) ignores [fallback]
 * and resolves the REAL localized text through [Context.getString].
 */
fun interface SessionCatalogStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultSessionCatalogStrings = SessionCatalogStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun sessionCatalogStrings(context: Context): SessionCatalogStrings =
    SessionCatalogStrings { id, _, args -> context.getString(id, *args) }

private val BARE_SESSION_UUID =
    Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")

/**
 * Convert the legacy mobile `SessionId::Display` spelling (`sess:<uuid>`) to
 * the bare UUID used by JSONL filenames and `ResumeSession`.
 *
 * Invalid/non-UUID values are preserved so callers still receive the engine's
 * honest malformed-id error rather than silently targeting a different value.
 */
fun canonicalSessionId(value: String): String {
    val trimmed = value.trim()
    val candidate = trimmed.removePrefix("sess:")
    return if (BARE_SESSION_UUID.matches(candidate)) candidate.lowercase() else trimmed
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
 * Keep durable session-index rows visible while the live engine catalog is
 * loading or has not yet observed a newly started session.
 *
 * Once the engine returns that row, the live version wins and the repository's
 * authoritative sync removes stale cached rows.
 */
fun EngineSessionState.withCachedRows(cachedRows: List<SessionRow>): EngineSessionState {
    if (cachedRows.isEmpty()) return this
    val liveIds = rows.mapTo(mutableSetOf()) { it.uuid }
    val merged = rows + cachedRows.filterNot { it.uuid in liveIds }
    return EngineSessionState.ready(merged)
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
        strings: SessionCatalogStrings = DefaultSessionCatalogStrings,
    ): SessionRow = SessionRow(
        uuid = canonicalSessionId(uuid),
        title = title.ifBlank { strings.resolve(R.string.session_untitled, "未命名会话") },
        messageCount = messageCount,
        relativeTime = relativeTime(modifiedRfc3339, nowEpochSeconds, strings),
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
    fun relativeTime(
        modifiedRfc3339: String,
        nowEpochSeconds: Long,
        strings: SessionCatalogStrings = DefaultSessionCatalogStrings,
    ): String {
        val justNow = { strings.resolve(R.string.project_relative_time_just_now, "刚刚") }
        val thenEpoch = parseRfc3339ToEpochSeconds(modifiedRfc3339) ?: return justNow()
        val delta = nowEpochSeconds - thenEpoch
        return when {
            delta < 0L -> justNow() // clock skew / future stamp
            delta < 60L -> justNow()
            delta < 3600L -> strings.resolve(R.string.session_relative_minutes_ago_fmt, "%1\$d 分钟前", delta / 60L)
            delta < 86_400L -> strings.resolve(R.string.session_relative_hours_ago_fmt, "%1\$d 小时前", delta / 3600L)
            delta < 172_800L -> strings.resolve(R.string.session_relative_yesterday, "昨天")
            delta < 604_800L -> strings.resolve(R.string.session_relative_days_ago_fmt, "%1\$d 天前", delta / 86_400L)
            else -> monthDay(modifiedRfc3339, strings)
                ?: strings.resolve(R.string.session_relative_days_ago_fmt, "%1\$d 天前", delta / 86_400L)
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
    private fun monthDay(s: String, strings: SessionCatalogStrings): String? {
        if (s.length < 10) return null
        val month = s.substring(5, 7).toIntOrNull() ?: return null
        val day = s.substring(8, 10).toIntOrNull() ?: return null
        if (month !in 1..12 || day !in 1..31) return null
        return strings.resolve(R.string.session_relative_month_day_fmt, "%1\$d月%2\$d日", month, day)
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
