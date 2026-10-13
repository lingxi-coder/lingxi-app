package com.lingxi.code.drawer

import androidx.compose.runtime.Immutable
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow

/**
 * The 最近 list's mode filter chip. A pure list filter: choosing a chip never
 * changes the live conversation's mode — the mode follows the session the user
 * opens, and a new chat picks its own mode on the new-chat page.
 */
enum class RecentsFilter(val key: String) {
    All("all"),
    Chat("chat"),
    Code("code"),
    ;

    fun accepts(mode: SessionMode): Boolean = when (this) {
        All -> true
        Chat -> mode == SessionMode.Chat
        Code -> mode == SessionMode.Code
    }

    /** The mode this chip stands for, or null for [All]. */
    val mode: SessionMode?
        get() = when (this) {
            All -> null
            Chat -> SessionMode.Chat
            Code -> SessionMode.Code
        }

    companion object {
        fun fromKey(key: String?): RecentsFilter = entries.firstOrNull { it.key == key } ?: All
    }
}

/** The bucket a recent session is listed under. Declaration order is display order. */
enum class RecentBucket {
    Pinned,
    Today,
    Yesterday,
    LastWeek,
    Earlier,
}

/**
 * What the drawer can honestly say about a session's live state. The engine is
 * bound to a single session at a time, so only the visible session can be
 * running or waiting for the user.
 */
enum class RecentStatus {
    Idle,
    Running,
    AwaitingConfirmation,
}

/** One row of the unified recent-sessions list. */
@Immutable
data class RecentSession(
    val uuid: String,
    val title: String,
    val mode: SessionMode,
    val scope: ConversationScope,
    val workspaceName: String,
    val messageCount: Int,
    val relativeTime: String,
    val modifiedAtEpochSeconds: Long?,
    val pinnedAtEpochMillis: Long?,
    val status: RecentStatus = RecentStatus.Idle,
)

@Immutable
data class RecentGroup(
    val bucket: RecentBucket,
    val sessions: List<RecentSession>,
)

/**
 * Flatten the global and per-project session catalogs into the single, mode-
 * agnostic recent list: pinned first, then today / yesterday / within seven
 * days / earlier, newest first inside each bucket.
 *
 * PURE (no Android/engine types, the clock and zone are parameters) so the
 * bucket boundaries are unit-testable on the plain JVM.
 *
 * @param query case-insensitive substring over title, workspace name and the
 *   relative-time label; blank means no filtering.
 * @param statusFor live state per session uuid.
 * @param utcOffsetSeconds the local zone's offset at [nowEpochSeconds], so
 *   "today" and "yesterday" are local calendar days.
 */
internal fun buildRecentGroups(
    globalName: String,
    globalSessions: List<SessionRow>,
    projects: List<Project>,
    filter: RecentsFilter,
    query: String,
    pinnedSessions: Map<String, Long>,
    statusFor: (String) -> RecentStatus,
    nowEpochSeconds: Long,
    utcOffsetSeconds: Int,
): List<RecentGroup> {
    val normalizedQuery = query.trim().lowercase()
    val all = buildList {
        globalSessions.forEach { row ->
            add(row.toRecent(ConversationScope.Global, globalName, pinnedSessions, statusFor))
        }
        projects.forEach { project ->
            val scope = ConversationScope.Project(project.id)
            project.sessions.forEach { session ->
                add(
                    RecentSession(
                        uuid = session.id,
                        title = session.title,
                        mode = session.mode,
                        scope = scope,
                        workspaceName = project.name,
                        messageCount = session.msgs,
                        relativeTime = session.activity,
                        modifiedAtEpochSeconds = session.updatedAtEpochMillis.takeIf { it > 0L }?.div(1000L),
                        pinnedAtEpochMillis = pinnedSessions[session.id],
                        status = statusFor(session.id),
                    ),
                )
            }
        }
    }
    val visible = all
        .distinctBy(RecentSession::uuid)
        .filter { filter.accepts(it.mode) }
        .filter { normalizedQuery.isEmpty() || it.matches(normalizedQuery) }
        .sortedWith(
            compareByDescending<RecentSession> { it.pinnedAtEpochMillis != null }
                .thenByDescending { it.pinnedAtEpochMillis ?: Long.MIN_VALUE }
                .thenByDescending { it.modifiedAtEpochSeconds ?: Long.MIN_VALUE }
                .thenBy(String.CASE_INSENSITIVE_ORDER) { it.title }
                .thenBy { it.uuid },
        )
    val today = localDay(nowEpochSeconds, utcOffsetSeconds)
    return visible
        .groupBy { it.bucket(today, utcOffsetSeconds) }
        .toSortedMap()
        .map { (bucket, sessions) -> RecentGroup(bucket, sessions) }
}

private fun SessionRow.toRecent(
    scope: ConversationScope,
    workspaceName: String,
    pinnedSessions: Map<String, Long>,
    statusFor: (String) -> RecentStatus,
): RecentSession = RecentSession(
    uuid = uuid,
    title = title,
    mode = mode,
    scope = scope,
    workspaceName = workspaceName,
    messageCount = messageCount,
    relativeTime = relativeTime,
    modifiedAtEpochSeconds = modifiedAtEpochSeconds,
    pinnedAtEpochMillis = pinnedSessions[uuid],
    status = statusFor(uuid),
)

private fun RecentSession.matches(normalizedQuery: String): Boolean =
    title.lowercase().contains(normalizedQuery) ||
        workspaceName.lowercase().contains(normalizedQuery) ||
        relativeTime.lowercase().contains(normalizedQuery)

private fun RecentSession.bucket(today: Long, utcOffsetSeconds: Int): RecentBucket {
    if (pinnedAtEpochMillis != null) return RecentBucket.Pinned
    val modified = modifiedAtEpochSeconds ?: return RecentBucket.Earlier
    return when (today - localDay(modified, utcOffsetSeconds)) {
        // A stamp from the future (clock skew) reads as today.
        in Long.MIN_VALUE..0L -> RecentBucket.Today
        1L -> RecentBucket.Yesterday
        in 2L..6L -> RecentBucket.LastWeek
        else -> RecentBucket.Earlier
    }
}

private fun localDay(epochSeconds: Long, utcOffsetSeconds: Int): Long =
    Math.floorDiv(epochSeconds + utcOffsetSeconds, 86_400L)
