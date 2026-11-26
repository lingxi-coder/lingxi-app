package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.util.UUID

// The replies of a Bot that wait for its maintainer to let them go (Bot v3 B6; Desktop's and Web's rules are `packages/teams/held-replies.ts`, iOS `TeamHeldReplies.swift`). A Bot
// can be published so that the reply it makes is held for its maintainer, who reads it before anyone else and lets it go as it is, changes its text, or rejects it. The service
// drops a held reply after ten minutes and, when nobody asked the Bot, when a newer message comes. It keeps a held reply where only the maintainer can read it, so the three
// commands here are the one way a client ever sees one: every other member sees no more than that the task awaits approval. This file has no UI and no network: the decoding,
// the rules and the list a screen shows, which the unit tests run.

/** The document that goes with a held reply, as the Bot made it. */
internal data class TeamHeldDocument(val title: String, val body: String) {
    companion object {
        /** Null for a document this version cannot read: it has a title (not empty) and a body, both text. */
        fun parse(v: JSONObject?): TeamHeldDocument? {
            if (v == null) return null
            val title = (v.opt("title") as? String)?.takeIf { it.isNotEmpty() } ?: return null
            val body = v.opt("body") as? String ?: return null
            return TeamHeldDocument(title, body)
        }
    }
}

/**
 * A reply of one Bot that waits for its maintainer, exactly as the Bot made it. [unasked]: nobody asked the Bot, it was given the message because auto-reply is on; only such a
 * reply is dropped by a newer message. [heldAt] and [expiresAt] are the service's clock, in epoch milliseconds: when it was held, and when it is dropped if nobody decides.
 * [sharedBotId] is the Bot that made it: the service's answer says whose list it is and not whose each reply is, so the read that asked for a Bot's replies sets it.
 */
internal data class TeamHeldReply(
    val taskId: String, val conversationId: String, val messageId: String, val requester: TeamMemberRef, val unasked: Boolean, val heldAt: Long, val expiresAt: Long, val text: String,
    val document: TeamHeldDocument? = null, val conversationTitle: String? = null, val sharedBotId: String = "",
) {
    companion object {
        /** A reply is never half read: what this version needs of it is there and known, or it is not one (null). */
        fun parse(v: JSONObject?): TeamHeldReply? {
            if (v == null) return null
            fun text(key: String) = (v.opt(key) as? String)?.takeIf { it.isNotEmpty() }
            val taskId = text("taskId") ?: return null
            val conversationId = text("conversationId") ?: return null
            val messageId = text("messageId") ?: return null
            val member = v.optJSONObject("requester") ?: return null
            val uid = (member.opt("uid") as? String)?.takeIf { it.isNotEmpty() } ?: return null
            val generation = (member.opt("generation") as? Number)?.toDouble()?.takeIf { it.isFinite() && it == Math.rint(it) && it >= 0 && it <= 9007199254740991.0 }?.toLong() ?: return null
            val unasked = v.opt("unasked") as? Boolean ?: return null
            val heldAt = TeamTriageRules.moment(v.opt("heldAt")) ?: return null
            val expiresAt = TeamTriageRules.moment(v.opt("expiresAt")) ?: return null
            val reply = text("text") ?: return null
            // A document that cannot be read is no reply to approve: the maintainer would be letting go of something they did not see.
            val document = if (v.has("document")) TeamHeldDocument.parse(v.optJSONObject("document")) ?: return null else null
            return TeamHeldReply(taskId, conversationId, messageId, TeamMemberRef(uid, generation), unasked, heldAt, expiresAt, reply, document, text("conversationTitle"))
        }
    }
}

/** The answer to `listHeldReplies`. */
internal object TeamHeldAnswer {
    /** A reply that this version cannot read is left out, and the others are still shown; an answer that is not a list at all is no empty list, it is `INVALID_DATA`. */
    fun parse(v: JSONObject): List<TeamHeldReply> {
        val rows = v.optJSONArray("replies") ?: throw TeamMessageFailure("INVALID_DATA")
        return (0 until rows.length()).mapNotNull { TeamHeldReply.parse(rows.optJSONObject(it)) }
    }
}

/**
 * What a decision on a held reply came to. Approving posts it (`completed`, with the `replyId` of the message and the `artifactId` of the document when it has one); rejecting
 * drops it (`declined`, `approval_rejected`). A reply that may not be posted any more (the message it answers was edited, its requester left) is dropped too: `declined`,
 * `revoked`. A reason this version does not know is left out.
 */
internal data class TeamHeldDecision(val taskId: String, val state: String, val declineReason: String? = null, val replyId: String? = null, val artifactId: String? = null) {
    companion object {
        /** An answer that says nothing of the task is not a decision: `INVALID_DATA`. */
        fun parse(v: JSONObject): TeamHeldDecision {
            fun text(key: String) = (v.opt(key) as? String)?.takeIf { it.isNotEmpty() }
            val taskId = text("taskId") ?: throw TeamMessageFailure("INVALID_DATA")
            val state = text("state") ?: throw TeamMessageFailure("INVALID_DATA")
            return TeamHeldDecision(taskId, state, text("declineReason")?.takeIf { it in TeamHeldRules.declineReasons }, text("replyId"), text("artifactId"))
        }
    }
}

/** What sending the maintainer's own text comes to: the text to send ([Send.text] is null when it says what the Bot said), or why it cannot be sent. */
internal sealed interface TeamHeldEdit {
    data class Send(val text: String?) : TeamHeldEdit
    data object Empty : TeamHeldEdit
    data object TooLong : TeamHeldEdit
}

internal object TeamHeldRules {
    /** How often the time left is brought up to date, in milliseconds. */
    const val TICK_MS = 30_000L
    /** How long after a reply's end it is read again: the service has dropped it by then, whatever its clock says. */
    const val EXPIRY_SLACK_MS = 500L
    /** A reply whose time ran out while it was shown. */
    const val EXPIRED = "REPLY_EXPIRED"
    /** What a client says of an approval the service took that posted nothing, because the reply could no longer be posted (`declined`, `revoked`). The service never sends it. */
    const val NOT_POSTED = "REPLY_NOT_POSTED"
    /**
     * How the service says a reply is no longer there to decide: it was decided, ran out of time or was dropped for a newer message, it is gone, or it is not this person's to
     * decide. A decision refused with one of these is not tried again.
     */
    val ended = setOf("REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "REPLY_EXPIRED", "REPLY_SUPERSEDED", "TASK_REQUIRED", "MAINTAINER_REQUIRED")
    val declineReasons = setOf("approval_rejected", "revoked")

    // What may be sent

    /**
     * The text of a held reply as the maintainer left it in the editor. The service takes it trimmed, at least a character and at most [TeamExecutionContract.HELD_REPLY_BYTES] of
     * UTF-8; text that is the Bot's own once trimmed is not sent at all, so that the reply is posted as the Bot made it and is not recorded as changed.
     */
    fun edit(draft: String, edited: String): TeamHeldEdit {
        val text = edited.trim()
        if (text.isEmpty()) return TeamHeldEdit.Empty
        if (text.toByteArray(Charsets.UTF_8).size > TeamExecutionContract.HELD_REPLY_BYTES) return TeamHeldEdit.TooLong
        return TeamHeldEdit.Send(if (text == draft.trim()) null else text)
    }
    /** The payloads of the three commands, as [TeamExecutionContract.validate] takes them. */
    fun listPayload(sharedBotId: String, conversationId: String? = null): JSONObject =
        JSONObject().put("sharedBotId", sharedBotId).apply { if (conversationId != null) put("conversationId", conversationId) }
    fun approvePayload(taskId: String, text: String? = null): JSONObject = JSONObject().put("taskId", taskId).apply { if (text != null) put("text", text) }
    fun rejectPayload(taskId: String): JSONObject = JSONObject().put("taskId", taskId)

    // Whose replies, and where they are shown

    /**
     * The Bots of a Chat whose held replies this person decides on: the ones they maintain that are in it, answering when asked or on their own. A maintainer need not take part
     * in a Chat for the service to show them its replies, so this does not depend on that.
     */
    fun bots(conversation: TeamMessageConversation, cards: List<TeamBotCard>, viewer: TeamMemberRef): List<TeamBotCard> =
        cards.filter { "shared_bot:${it.sharedBotId}" in conversation.participants && TeamTriageRules.maintains(it, viewer) }

    /** Where the held replies of a Chat are shown: [byTask] sits with the task it answers, [orphans] are rows of their own at the end of the stream. */
    data class Placed(val byTask: Map<String, TeamHeldRow>, val orphans: List<TeamHeldRow>)
    /**
     * A reply of a task the Chat lists sits with that task. A reply nobody asked for has no task in the list (the service keeps such a task out of every member's), so it is a row
     * of its own, oldest first as the stream is; an asked reply whose task is not listed yet waits for it.
     */
    fun place(rows: List<TeamHeldRow>, listed: Set<String>): Placed {
        val byTask = linkedMapOf<String, TeamHeldRow>(); val orphans = mutableListOf<TeamHeldRow>()
        for (row in rows) {
            if (row.reply.taskId in listed) byTask[row.reply.taskId] = row else if (row.reply.unasked) orphans.add(0, row)
        }
        return Placed(byTask, orphans)
    }
    /** The replies that still wait: not decided, not ended, not out of time. What the Bot card counts. */
    fun waiting(list: TeamHeldList): Int = list.rows.count { it.ended == null && it.reply.expiresAt > list.now }
    /** A Chat and the replies of the list that were made in it; [title] is null for a Chat the service gave no title for. */
    data class Chat(val id: String, val title: String?, val rows: List<TeamHeldRow>)
    /** The list by Chat, in the order of the list: the Chat of the newest reply first, and each Chat's replies newest first. */
    fun byChat(rows: List<TeamHeldRow>): List<Chat> {
        val result = mutableListOf<Chat>()
        for (row in rows) {
            val index = result.indexOfFirst { it.id == row.reply.conversationId }
            if (index >= 0) result[index] = result[index].copy(rows = result[index].rows + row)
            else result += Chat(row.reply.conversationId, row.reply.conversationTitle, listOf(row))
        }
        return result
    }

    // Words (the shared `teams_held_*` and `teams_error_reply_*` strings)

    /**
     * What a decision that did not go through says: the codes every message command can fail with, and the three of its own (the Bot is not theirs to decide for, the reply is gone,
     * and [NOT_POSTED]). The same codes mean something else for other commands.
     */
    @StringRes fun errorRes(code: String): Int = when (code) {
        "REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD" -> R.string.teams_error_reply_decided
        "REPLY_EXPIRED" -> R.string.teams_error_reply_expired
        "REPLY_SUPERSEDED" -> R.string.teams_error_reply_superseded
        "TASK_REQUIRED" -> R.string.teams_error_reply_gone
        "MAINTAINER_REQUIRED" -> R.string.teams_error_held_maintainer
        NOT_POSTED -> R.string.teams_held_revoked
        else -> teamsErrorRes(code)
    }
    /** The problem with the text of an edit, in words; nothing for text that can be sent. */
    @StringRes fun problemRes(edit: TeamHeldEdit): Int? = when (edit) {
        is TeamHeldEdit.Send -> null
        TeamHeldEdit.Empty -> R.string.teams_held_empty_text
        TeamHeldEdit.TooLong -> R.string.teams_held_too_long
    }
    /** The names of every string the held replies say anything with: each has text in every language (the `strings.xml` of each `values` folder). */
    val stringNames = listOf(
        "teams_held_title", "teams_held_expires_fmt", "teams_held_document_fmt", "teams_held_reject", "teams_held_reject_confirm", "teams_held_reject_detail", "teams_held_edit_label",
        "teams_held_empty_text", "teams_held_too_long", "teams_held_unasked", "teams_held_waiting", "teams_held_intro", "teams_held_none", "teams_held_revoked",
        "teams_error_reply_decided", "teams_error_reply_expired", "teams_error_reply_superseded", "teams_error_reply_gone", "teams_error_held_maintainer",
        "teams_approval_approve", "teams_message_edit", "teams_message_send", "common_cancel", "common_close", "common_retry", "teams_list_refresh", "teams_loading",
        "teams_triage_chat_untitled", "friends_open_chat",
    )

    // Reading

    fun code(failure: Throwable): String = (failure as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE"
    internal val newestFirst: Comparator<TeamHeldReply> = compareByDescending<TeamHeldReply> { it.heldAt }.thenBy { it.taskId }
    /**
     * The replies of these Bots that wait for this person, newest first, each tagged with its Bot. [one] reads the replies of one Bot. A Bot the service says is not theirs after all
     * adds nothing; if that is every Bot the answer is `MAINTAINER_REQUIRED`, and any other failure is the failure of the read (of the first Bot that failed). Being cancelled is not
     * a failure and is passed on.
     */
    suspend fun read(bots: List<String>, one: suspend (String) -> List<TeamHeldReply>): List<TeamHeldReply> {
        val answers = coroutineScope {
            bots.map { id ->
                async {
                    try { Result.success(one(id).map { it.copy(sharedBotId = id) }) }
                    catch (cancelled: CancellationException) { throw cancelled }
                    catch (failure: Exception) { Result.failure(failure) }
                }
            }.awaitAll()
        }
        val read = mutableListOf<TeamHeldReply>(); var answered = false
        for (answer in answers) {
            val replies = answer.getOrNull()
            if (replies != null) { read += replies; answered = true }
            else answer.exceptionOrNull()?.let { if (code(it) != "MAINTAINER_REQUIRED") throw it }
        }
        if (!answered) throw TeamMessageFailure("MAINTAINER_REQUIRED")
        return read.sortedWith(newestFirst)
    }
}

/**
 * One held reply as a screen shows it, and what is going on with it. [busy]: a decision on it is on its way. [error]: the last decision failed in a way that can be tried again (a
 * code). [ended]: it is not there to be decided any more, and why: a code of the service, [TeamHeldRules.EXPIRED] for time that ran out while it was shown,
 * [TeamHeldRules.NOT_POSTED] for an approval that posted nothing. It stays on screen, saying so, until it is closed.
 */
internal data class TeamHeldRow(val reply: TeamHeldReply, val busy: Busy? = null, val error: String? = null, val ended: String? = null) {
    enum class Busy { Approve, Reject }
    /** Why the card shows no decision, if it does not: it ended, or its time is up even though the service has not been asked again. */
    fun over(now: Long): String? = ended ?: if (reply.expiresAt <= now) TeamHeldRules.EXPIRED else null
}

/**
 * The list as the screen shows it. `Loading` until the first answer; `Failed`: the last read failed ([error] says how) and the rows are what was read before; `Denied`: the service
 * says this person maintains none of the Bots, so there is nothing to show and nothing to offer. [now] is the time the rows are shown at, in epoch milliseconds: it moves on every
 * [TeamHeldRules.TICK_MS] and at each reply's end, so that the time left is never staler than that.
 */
internal data class TeamHeldList(val status: Status = Status.Loading, val rows: List<TeamHeldRow> = emptyList(), val error: String? = null, val now: Long = 0) {
    enum class Status { Loading, Ready, Failed, Denied }
    fun row(taskId: String): TeamHeldRow? = rows.firstOrNull { it.reply.taskId == taskId }
}

/** The decisions: each is one command, sent with the id it was given. */
internal class TeamHeldDecide(
    val approve: suspend (taskId: String, text: String?, commandId: String) -> TeamHeldDecision,
    val reject: suspend (taskId: String, commandId: String) -> TeamHeldDecision,
)

/**
 * The held replies a screen shows and the decisions on them. It reads the list when asked to ([load], one read at a time; a request that comes while one is going on is answered by one
 * more read after it) and keeps the time ([keepTime]): the time left moves on every [TeamHeldRules.TICK_MS], and the list is read again when the first reply runs out. Nothing is read
 * while [online] says there is no connection.
 *
 * A reply that leaves the list stays on screen when someone is told something about it: while a decision on it is on its way, after the service refused one because it was decided
 * or ran out of time, and after its time ran out while it was shown. It is told about once ([dismiss]), and then it is gone. One that leaves for no such reason (it was decided on
 * another screen, or a newer message dropped it) simply goes. A read or a decision that answers after the board was closed changes nothing.
 *
 * It runs on one thread (the main one): its callers are the screen's effects. [scope] is where its reads run, so that a screen's effect that asks again and is replaced does not take
 * the read that is going on with it.
 */
internal class TeamHeldBoard(
    private val scope: CoroutineScope,
    private val read: suspend () -> List<TeamHeldReply>,
    private val decide: TeamHeldDecide,
    private val changed: (TeamHeldList) -> Unit,
    private val online: () -> Boolean = { true },
    private val clock: () -> Long = System::currentTimeMillis,
    private val newId: () -> String = { UUID.randomUUID().toString() },
    private val sleep: suspend (Long) -> Unit = { delay(it) },
) {
    private data class Entry(val reply: TeamHeldReply, val busy: TeamHeldRow.Busy? = null, val error: String? = null, val ended: String? = null)
    private class Attempt(val decision: String, val commandId: String)
    private val entries = mutableMapOf<String, Entry>()
    /** The latest attempt at each reply, for the decision it was: a retry of the same decision is the same command, a different decision is a new one. */
    private val attempts = mutableMapOf<String, Attempt>()
    private var status = TeamHeldList.Status.Loading
    private var error: String? = null
    private var now = clock()
    private var pending: Job? = null
    private var again = false
    private var closed = false
    var list = build()
        private set

    /** Nothing that is still on its way changes the list after this. */
    fun close() { closed = true }

    /** Reads the list (again); returns when the read that answers this request is over. Nothing happens without a connection. */
    suspend fun load() {
        if (closed || !online()) return
        val running = pending
        if (running != null) { again = true; running.join(); return }
        // Lazy, so that the job is known as the pending one before its first step runs.
        val work = scope.launch(start = CoroutineStart.LAZY) {
            try { do { again = false; pass() } while (again && !closed) } finally { pending = null }
        }
        pending = work
        work.start()
        work.join()
    }

    /** Lets the reply go, with the maintainer's text when they changed it. Whether the service took the decision. */
    suspend fun approve(taskId: String, text: String? = null): Boolean =
        decision(taskId, TeamHeldRow.Busy.Approve, "approve:" + (text ?: "")) { id ->
            val decision = decide.approve(taskId, text, id)
            // Accepted, and posted nothing: the reply could not be posted any more. It says so.
            if (decision.state == "declined" && decision.declineReason == "revoked") TeamHeldRules.NOT_POSTED else null
        }
    /** Drops the reply. Whether the service took the decision. */
    suspend fun reject(taskId: String): Boolean = decision(taskId, TeamHeldRow.Busy.Reject, "reject") { id -> decide.reject(taskId, id); null }
    /** The person has read what became of a reply that was shown: it goes. */
    fun dismiss(taskId: String) {
        if (entries[taskId]?.ended == null) return
        entries.remove(taskId)
        publish()
    }

    /**
     * Keeps the time until the coroutine is cancelled or the board is closed: wakes every [TeamHeldRules.TICK_MS], sooner when the first reply that waits runs out, and then reads
     * the list again, which no longer has it.
     */
    suspend fun keepTime() {
        while (!closed) {
            val current = clock()
            val next = entries.values.filter { it.ended == null && it.reply.expiresAt > current }.minOfOrNull { it.reply.expiresAt }
            sleep(next?.let { minOf(TeamHeldRules.TICK_MS, maxOf(0L, it - current) + TeamHeldRules.EXPIRY_SLACK_MS) } ?: TeamHeldRules.TICK_MS)
            if (closed) return
            val before = now
            now = clock()
            publish()
            // A reply ran out since the time was last brought up to date.
            if (entries.values.any { it.ended == null && it.reply.expiresAt > before && it.reply.expiresAt <= now }) load()
        }
    }

    private suspend fun decision(taskId: String, kind: TeamHeldRow.Busy, key: String, send: suspend (String) -> String?): Boolean {
        val entry = entries[taskId] ?: return false
        if (entry.busy != null || entry.ended != null || closed) return false
        entries[taskId] = entry.copy(busy = kind, error = null); publish()
        val attempt = attempts[taskId]?.takeIf { it.decision == key } ?: Attempt(key, newId())
        attempts[taskId] = attempt
        try {
            val ended = send(attempt.commandId)
            attempts.remove(taskId)
            if (closed) return true
            if (ended != null) entries[taskId]?.let { entries[taskId] = it.copy(busy = null, ended = ended) } else entries.remove(taskId)
            publish()
            scope.launch { load() }
            return true
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            if (closed) return false
            val code = TeamHeldRules.code(failure)
            // The reply is not there to be decided: say so, and read the list, which no longer has it. Anything else may be tried again, as the same command.
            val gone = code in TeamHeldRules.ended
            entries[taskId]?.let { entries[taskId] = if (gone) it.copy(busy = null, ended = code) else it.copy(busy = null, error = code) }
            if (gone) attempts.remove(taskId)
            publish()
            if (gone) scope.launch { load() }
            return false
        }
    }

    private suspend fun pass() {
        try {
            val replies = read()
            if (closed) return
            merge(replies)
            status = TeamHeldList.Status.Ready; error = null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            if (closed) return
            val code = TeamHeldRules.code(failure)
            if (code == "MAINTAINER_REQUIRED") { entries.clear(); status = TeamHeldList.Status.Denied } else status = TeamHeldList.Status.Failed
            error = code
        }
        now = clock()
        publish()
    }

    private fun merge(replies: List<TeamHeldReply>) {
        val current = clock()
        val listed = replies.mapTo(mutableSetOf()) { it.taskId }
        for ((taskId, entry) in entries.toMap()) {
            if (taskId in listed || entry.busy != null || entry.ended != null) continue
            if (entry.reply.expiresAt <= current) entries[taskId] = entry.copy(ended = TeamHeldRules.EXPIRED) else entries.remove(taskId)
        }
        for (reply in replies) entries[reply.taskId] = entries[reply.taskId]?.copy(reply = reply) ?: Entry(reply)
    }

    private fun build(): TeamHeldList =
        TeamHeldList(status, entries.values.sortedWith(compareBy<Entry, TeamHeldReply>(TeamHeldRules.newestFirst) { it.reply }).map { TeamHeldRow(it.reply, it.busy, it.error, it.ended) }, error, now)
    private fun publish() { list = build(); changed(list) }
}
