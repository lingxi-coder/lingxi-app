package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import org.json.JSONObject

// The auto-reply log of a Bot, for the person who maintains it (Desktop's Bots tab, `packages/teams/triage-log.ts`, iOS `TeamTriageLog.swift`): what the Bot's
// host decided about each message nobody asked the Bot about, and why, in codes. Newest first, a page at a time, for every Chat of the Team or for one. Only
// the decision and the reason are kept, never what a model said. This file has no UI and no network: the rules, the decoding and the paged list.

/** Where a judged message ended up: it was answered, the Bot stayed silent (it declined, or the task was canceled before it spoke), it failed, or it is still going on. */
internal enum class TeamTriageOutcome { Replied, Silent, Failed, Waiting }

/** What the Bot's host decided and why. A reason or a probability this version does not know is left out; the decision is kept. [at] is the service's clock, in epoch milliseconds. */
internal data class TeamTriageVerdict(val action: String, val source: String, val at: Long, val reason: String? = null, val probability: Double? = null) {
    companion object {
        /** Null when this version cannot read it: an action or a source it does not know, or a time no date can hold. */
        fun parse(v: JSONObject?): TeamTriageVerdict? {
            if (v == null) return null
            val action = (v.opt("action") as? String)?.takeIf { it in TeamTriageRules.actions } ?: return null
            val source = (v.opt("source") as? String)?.takeIf { it in TeamTriageRules.sources } ?: return null
            val at = TeamTriageRules.moment(v.opt("at")) ?: return null
            val reason = (v.opt("reason") as? String)?.takeIf { it in TeamTriageRules.reasons }
            // What the Decision service gave, from 0 to 1; anything else is not one.
            val probability = (v.opt("probability") as? Number)?.toDouble()?.takeIf { it in 0.0..1.0 }
            return TeamTriageVerdict(action, source, at, reason, probability)
        }
    }
}

/** One message the Bot's host judged, as its maintainer reads the log. [state] is where the task stands now: it settles after the verdict. */
internal data class TeamTriageItem(
    val taskId: String, val conversationId: String, val messageId: String, val createdAt: Long, val triage: TeamTriageVerdict, val state: String, val conversationTitle: String? = null,
) {
    companion object {
        /** A row is never half read: what this version needs of it is there and known, or the row is not one (null). */
        fun parse(v: JSONObject?): TeamTriageItem? {
            if (v == null) return null
            fun text(key: String) = (v.opt(key) as? String)?.takeIf { it.isNotEmpty() }
            val taskId = text("taskId") ?: return null
            val conversationId = text("conversationId") ?: return null
            val messageId = text("messageId") ?: return null
            val createdAt = TeamTriageRules.moment(v.opt("createdAt")) ?: return null
            val triage = TeamTriageVerdict.parse(v.optJSONObject("triage")) ?: return null
            val state = text("state")?.takeIf { it in TeamTriageRules.outcomes } ?: return null
            return TeamTriageItem(taskId, conversationId, messageId, createdAt, triage, state, text("conversationTitle"))
        }
    }
}

/** A page of the log, newest verdict first. [nextCursor] is passed back to read the page after this one; it is absent at the end of the log. */
internal data class TeamTriagePage(val items: List<TeamTriageItem>, val nextCursor: String? = null) {
    companion object {
        /** A row that this version cannot read is left out, and the page it came in is still shown. An answer that is not a page at all is `INVALID_DATA`. */
        fun parse(v: JSONObject): TeamTriagePage {
            val rows = v.optJSONArray("items") ?: throw TeamMessageFailure("INVALID_DATA")
            val cursor = v.opt("nextCursor")
            if (cursor != null && cursor != JSONObject.NULL && cursor !is String) throw TeamMessageFailure("INVALID_DATA")
            return TeamTriagePage((0 until rows.length()).mapNotNull { TeamTriageItem.parse(rows.optJSONObject(it)) }, (cursor as? String)?.takeIf { it.isNotEmpty() })
        }
    }
}

/** The Chat a log is limited to, and how the person knows it. */
internal data class TeamTriageChat(val id: String, val title: String)

internal object TeamTriageRules {
    /** How many rows a page of the log has: the service gives at most [TeamExecutionContract.TRIAGE_LIMIT]. */
    const val PAGE = 20
    /** How many pages of 50 rows are read to find the verdict of a task before it is given up. */
    const val FIND_PAGES = 4
    val actions = setOf("decline", "run", "run_may_decline")
    val sources = setOf("quiet", "cooldown", "rule", "decision", "fallback")
    val reasons = setOf("quiet_hours", "cooldown", "keyword", "addressed", "below_threshold", "between_thresholds", "above_threshold", "decision_unavailable", "message_unavailable")
    /** Every state a task can be in, and where it leaves the judged message. A state this table does not have is a row this version cannot read. */
    val outcomes: Map<String, TeamTriageOutcome> = mapOf(
        "completed" to TeamTriageOutcome.Replied, "declined" to TeamTriageOutcome.Silent, "cancelled" to TeamTriageOutcome.Silent,
        "queued" to TeamTriageOutcome.Waiting, "running" to TeamTriageOutcome.Waiting, "waiting" to TeamTriageOutcome.Waiting,
        "failed" to TeamTriageOutcome.Failed, "expired" to TeamTriageOutcome.Failed, "interrupted" to TeamTriageOutcome.Failed,
        "authorization_required" to TeamTriageOutcome.Failed, "revoked" to TeamTriageOutcome.Failed, "needs_reconciliation" to TeamTriageOutcome.Failed,
    )
    /** A time in epoch milliseconds that a date can hold; null for anything else, a number as text included. */
    fun moment(value: Any?): Long? = (value as? Number)?.toDouble()?.takeIf { it.isFinite() && Math.abs(it) <= 8.64e15 }?.toLong()
    /** A probability the Decision service gave, as the whole percent it is shown as (halves go up, as on the other clients). */
    fun percent(probability: Double): Int = Math.round(probability * 100).toInt()

    // Words (the shared `teams_triage_*` strings)

    @StringRes fun actionLabel(action: String): Int? = when (action) {
        "decline" -> R.string.teams_triage_action_decline
        "run" -> R.string.teams_triage_action_run
        "run_may_decline" -> R.string.teams_triage_action_maybe
        else -> null
    }
    /** Why, in words; nothing for a reason this version does not know. */
    @StringRes fun reasonLabel(reason: String?): Int? = when (reason) {
        "quiet_hours" -> R.string.teams_triage_reason_quiet_hours
        "cooldown" -> R.string.teams_triage_reason_cooldown
        "keyword" -> R.string.teams_triage_reason_keyword
        "addressed" -> R.string.teams_triage_reason_addressed
        "below_threshold" -> R.string.teams_triage_reason_below_threshold
        "between_thresholds" -> R.string.teams_triage_reason_between_thresholds
        "above_threshold" -> R.string.teams_triage_reason_above_threshold
        "decision_unavailable" -> R.string.teams_triage_reason_decision_unavailable
        "message_unavailable" -> R.string.teams_triage_reason_message_unavailable
        else -> null
    }
    @StringRes fun outcomeLabel(outcome: TeamTriageOutcome): Int = when (outcome) {
        TeamTriageOutcome.Replied -> R.string.teams_triage_state_replied
        TeamTriageOutcome.Silent -> R.string.teams_triage_state_silent
        TeamTriageOutcome.Failed -> R.string.teams_triage_state_failed
        TeamTriageOutcome.Waiting -> R.string.teams_triage_state_waiting
    }
    /** The names of every string the log says anything with: each has text in every language (the `strings.xml` of each `values` folder). */
    val stringNames = listOf(
        "teams_triage_title", "teams_triage_intro", "teams_triage_loading", "teams_triage_empty", "teams_triage_more", "teams_triage_chat_untitled",
        "teams_triage_action_decline", "teams_triage_action_run", "teams_triage_action_maybe",
        "teams_triage_reason_quiet_hours", "teams_triage_reason_cooldown", "teams_triage_reason_keyword", "teams_triage_reason_addressed", "teams_triage_reason_below_threshold",
        "teams_triage_reason_between_thresholds", "teams_triage_reason_above_threshold", "teams_triage_reason_decision_unavailable", "teams_triage_reason_message_unavailable",
        "teams_triage_state_replied", "teams_triage_state_silent", "teams_triage_state_failed", "teams_triage_state_waiting", "teams_triage_probability_fmt",
        "teams_list_refresh", "common_retry", "common_close",
    )

    // Whose log

    /** Whether this person maintains the Bot, by the membership they hold now. */
    fun maintains(card: TeamBotCard, viewer: TeamMemberRef): Boolean = card.maintainer == viewer
    /**
     * The Bots of a Chat whose log this person reads there: the ones they maintain that answer on their own in it, or are published to. A maintainer need not
     * take part in a Chat to be shown it, so this does not depend on that.
     */
    fun bots(conversation: TeamMessageConversation, cards: List<TeamBotCard>, viewer: TeamMemberRef): List<TeamBotCard> {
        val on = conversation.autoReply?.targets.orEmpty()
        return cards.filter { card -> "shared_bot:${card.sharedBotId}" in conversation.participants && maintains(card, viewer) && (card.autoReply != null || card.sharedBotId in on) }
    }
    /** The Bot of a Bot DM when this person maintains it: the only one whose silences they are told the reasons of. */
    fun reasonBot(conversation: TeamMessageConversation, cards: List<TeamBotCard>, viewer: TeamMemberRef): TeamBotCard? {
        if (conversation.kind != "bot_dm") return null
        val id = conversation.sharedBotId ?: return null
        return cards.firstOrNull { it.sharedBotId == id && maintains(it, viewer) }
    }
    /** The tasks of this Bot that stayed silent: the ones whose reasons are asked for. */
    fun silentTasks(tasks: List<TeamTask>, sharedBotId: String): List<String> = tasks.filter { it.state == "declined" && it.sharedBotId == sharedBotId }.map { it.taskId }
    /** The line under "Did not reply" for the maintainer: the reason's words, then how likely the Decision service thought a reply was wanted. Nothing without a row. */
    fun reasonParts(item: TeamTriageItem?): Pair<Int?, Int?> =
        if (item == null) null to null else reasonLabel(item.triage.reason) to item.triage.probability?.let { percent(it) }
}

/** One read of the log: the rows after [cursor] (from the newest when there is none), at most [limit]. */
internal typealias TeamTriageRead = suspend (cursor: String?, limit: Int) -> TeamTriagePage

/**
 * The list as the screen shows it. `Loading`: the first page is being read; `More`: the next one is; `Failed`: the last read failed ([error] says how) and the rows
 * are what was read before; `Denied`: the service says this person does not maintain the Bot, so there is no log to show and nothing to offer.
 */
internal data class TeamTriageList(
    val status: Status = Status.Loading,
    /** Newest first, each task once. */
    val items: List<TeamTriageItem> = emptyList(),
    /** Pass it back to read the page after the last one; null at the end of the log. */
    val nextCursor: String? = null,
    val error: String = "",
) {
    enum class Status { Loading, Ready, More, Failed, Denied }
    val busy get() = status == Status.Loading || status == Status.More
}

/**
 * The paged list behind the screen. A read that was overtaken by a newer one, or that answers after the list was closed, does not change what the newer one showed.
 * It runs on one thread (the main one): its callers are the screen's effects.
 */
internal class TeamTriageReader(private val limit: Int = TeamTriageRules.PAGE, private val read: TeamTriageRead, private val changed: (TeamTriageList) -> Unit) {
    var list = TeamTriageList()
        private set
    private var run = 0

    /** Reads the first page. Also what "refresh" and "try again" do: the list starts over. */
    suspend fun load() {
        run += 1; val mine = run
        set(TeamTriageList())
        page(mine, null)
    }
    /** Reads the page after the last one. Nothing happens while a read is going on, at the end of the log, or once the service said no. */
    suspend fun more() {
        val cursor = list.nextCursor ?: return
        if (list.busy || list.status == TeamTriageList.Status.Denied) return
        run += 1; val mine = run
        set(list.copy(status = TeamTriageList.Status.More, error = ""))
        page(mine, cursor)
    }
    /** Nothing that is still being read changes the list after this. */
    fun close() { run += 1 }

    private suspend fun page(mine: Int, cursor: String?) {
        try {
            val page = read(cursor, limit)
            if (mine != run) return
            // Each task once: a page that repeats a row of the one before (the log moved on between the two reads) adds nothing.
            val items = (if (cursor == null) emptyList() else list.items).toMutableList()
            val seen = items.mapTo(mutableSetOf()) { it.taskId }
            for (item in page.items) if (seen.add(item.taskId)) items += item
            set(TeamTriageList(TeamTriageList.Status.Ready, items, page.nextCursor))
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            if (mine != run) return
            val code = (failure as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
            if (code == "MAINTAINER_REQUIRED") set(TeamTriageList(TeamTriageList.Status.Denied, error = code))
            else set(list.copy(status = TeamTriageList.Status.Failed, error = code))
        }
    }
    private fun set(next: TeamTriageList) { list = next; changed(next) }
}

internal object TeamTriageSearch {
    /**
     * The rows of the log for these tasks, read from the newest verdict down until every task has its row, the log ends, or [pages] pages were read. A task that
     * has no row is not in the answer. A read that fails ends the search with what it has found: the reason of a silence is a courtesy, never an error. Being
     * cancelled is not a failure and is passed on.
     */
    suspend fun find(taskIds: Collection<String>, pages: Int = TeamTriageRules.FIND_PAGES, read: TeamTriageRead): Map<String, TeamTriageItem> {
        val found = linkedMapOf<String, TeamTriageItem>()
        val missing = taskIds.toMutableSet()
        var cursor: String? = null
        try {
            var index = 0
            while (index < pages && missing.isNotEmpty()) {
                val page = read(cursor, TeamExecutionContract.TRIAGE_LIMIT)
                for (item in page.items) if (missing.remove(item.taskId)) found[item.taskId] = item
                val next = page.nextCursor ?: break
                if (next == cursor) break
                cursor = next; index += 1
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
        }
        return found
    }
}
