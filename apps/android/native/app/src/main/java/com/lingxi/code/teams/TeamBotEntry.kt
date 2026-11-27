package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter

internal data class TeamBotCapabilities(val durableWait: Boolean, val writeOperations: Boolean) {
    companion object {
        fun parse(value: JSONObject?): TeamBotCapabilities? {
            val wait = value?.opt("durableWait") as? Boolean ?: return null
            val write = value.opt("writeOperations") as? Boolean ?: return null
            return TeamBotCapabilities(wait, write)
        }
    }
}

internal data class TeamBotEntryContext(val uid: String, val teamId: String, val generation: Long, val epoch: Long)

/** A UI lifetime in addition to the workspace identity: an old creation may not navigate a newly opened screen. */
internal class TeamBotEntryLease { var active = true }

/** The value of a "label: value" item of the member text: words from the catalog (Supported, Not supported, Unknown) or a figure that needs no translation (a count, a UTC time). */
internal sealed interface TeamBotValue {
    data class Words(@StringRes val id: Int) : TeamBotValue
    data class Figure(val text: String) : TeamBotValue
}

/** One "label: value" item of the member text; [label] is a string resource. */
internal data class TeamBotItem(@StringRes val label: Int, val value: TeamBotValue)

internal object TeamBotEntry {
    private val unavailableStatuses = setOf("paused", "offline", "maintenance", "quota_exhausted")
    val availabilities = setOf("ready", "paused", "offline", "maintenance", "quota_exhausted", "unverified")
    fun nonnegativeInteger(value: Any?): Long? = (value as? Number)?.toDouble()?.takeIf {
        it.isFinite() && it >= 0 && it <= 9_007_199_254_740_991.0 && it == Math.rint(it)
    }?.toLong()

    fun examples(value: JSONObject): List<String> = value.optJSONArray("examples")?.let { array ->
        (0 until array.length()).mapNotNull { array.opt(it) as? String }
            .filter { it.isNotBlank() && TeamsComposeLogic.canSendText(it) }.distinct().take(3)
    }.orEmpty()

    /** The Bot ids an array of a card lists, each once; nothing for what is no array of text. */
    fun idList(value: JSONObject, key: String): List<String> = value.optJSONArray(key)?.let { array ->
        (0 until array.length()).mapNotNull { (array.opt(it) as? String)?.takeIf(String::isNotEmpty) }.distinct()
    }.orEmpty()

    /** These cards were already filtered for visibility by the server; listing them never grants invocation permission. */
    fun memberDirectory(bots: List<TeamBotCard>): List<TeamBotCard> = bots.filter { it.state in setOf("published", "paused") }

    fun statusExpiry(bot: TeamBotCard): Long? = bot.statusExpiresAt ?: bot.onlineUntil.takeIf { bot.ready }

    /** Ready=false maintenance has an independent health deadline, even if Firestore emits no further signal. */
    fun nextStatusDeadline(bots: List<TeamBotCard>, now: Long): Long? =
        bots.mapNotNull(::statusExpiry).filter { it > now }.minOrNull()

    fun unverified(bot: TeamBotCard): TeamBotCard = bot.copy(
        ready = false, availability = "unverified", statusExpiresAt = null,
        admissionBlockedPendingRefresh = bot.admissionBlockedPendingRefresh || bot.availability in unavailableStatuses,
    )

    /** Only a fresh, unexpired available server projection lifts a remembered block. Unknown snapshots cannot lift it. */
    fun acceptSnapshot(previous: List<TeamBotCard>, incoming: List<TeamBotCard>, now: Long): List<TeamBotCard> {
        val old = previous.associateBy { it.sharedBotId }
        return expireStatuses(incoming.map { bot ->
            val prior = old[bot.sharedBotId]
            val freshAvailable = bot.availability == "ready" && (statusExpiry(bot)?.let { it > now } ?: true)
            bot.copy(admissionBlockedPendingRefresh = !freshAvailable && prior?.let {
                it.admissionBlockedPendingRefresh || it.availability in unavailableStatuses
            } == true)
        }, now)
    }

    fun expireStatuses(bots: List<TeamBotCard>, now: Long): List<TeamBotCard> = bots.map { bot ->
        if (statusExpiry(bot)?.let { it <= now } == true) unverified(bot) else bot
    }

    fun canOpen(origin: TeamBotEntryContext, current: TeamBotEntryContext?, active: Boolean): Boolean = active && origin == current

    /**
     * Explicit host availability blocks new Bot requests; device network state is a separate existing queue concern. [waitsForHost]: in a group or channel a Bot whose
     * Desktop is off is no refusal, the message is sent and its task waits for the Desktop (rule 6); a Bot DM says no at once.
     */
    fun blockedRequests(targets: Collection<String>, bots: List<TeamBotCard>, continuation: Boolean, waitsForHost: Boolean = false): List<TeamBotCard> =
        if (continuation) emptyList() else bots.filter {
            it.sharedBotId in targets && !(waitsForHost && it.availability == "offline") && (it.admissionBlockedPendingRefresh || it.availability in unavailableStatuses)
        }

    /** Called by queue before journaling a new Bot submit, as well as by the composer gate. */
    fun requireNewRequest(targets: Collection<String>, bots: List<TeamBotCard>, waitsForHost: Boolean = false) {
        val blocked = blockedRequests(targets, bots, continuation = false, waitsForHost = waitsForHost).firstOrNull() ?: return
        throw TeamMessageFailure(when {
            blocked.admissionBlockedPendingRefresh -> "BOT_STATUS_UNVERIFIED"
            blocked.availability == "quota_exhausted" -> "REQUEST_QUOTA_EXCEEDED"
            else -> "HOST_UNAVAILABLE"
        })
    }

    fun canSeed(draft: ComposerDraft, hasFiles: Boolean, example: String): Boolean =
        draft == ComposerDraft() && !hasFiles && TeamsComposeLogic.canSendText(example)

    /**
     * The member text is the catalog's: the `teams_bot_*` strings Desktop and Web read too (`packages/teams/BotMemberDetails.tsx`), so every client says the
     * same thing in every language. These return which strings and figures; the screen puts them into the UI language (`botAvailabilityText`, `teamBotLine`).
     *
     * The member projection is the sole health source; old ready/quotas fields do not invent a new projection. A Bot whose sharing stopped reads "Sharing
     * stopped" whatever its host reports (U1).
     */
    @StringRes
    fun availabilityRes(bot: TeamBotCard): Int = when (if (bot.state != "published") "paused" else bot.availability) {
        "ready" -> R.string.teams_bot_avail_ready
        "paused" -> R.string.teams_bot_disabled
        "offline" -> R.string.teams_bot_avail_offline
        "maintenance" -> R.string.teams_bot_avail_maintenance
        "quota_exhausted" -> R.string.teams_bot_avail_quota
        else -> R.string.teams_bot_avail_unverified
    }

    /** Wait and resume, and write operations, each Supported or Not supported. A Bot that has not reported them is Unknown for both. */
    fun capabilities(bot: TeamBotCard): List<TeamBotItem> {
        fun supported(on: Boolean?) = TeamBotValue.Words(when (on) { null -> R.string.chat_status_unknown; true -> R.string.teams_bot_supported; false -> R.string.teams_bot_not_supported })
        return listOf(
            TeamBotItem(R.string.teams_bot_wait_resume, supported(bot.capabilities?.durableWait)),
            TeamBotItem(R.string.teams_bot_write_ops, supported(bot.capabilities?.writeOperations)),
        )
    }

    /**
     * What a published Bot takes from other Bots and whom it may ask (Bot v3 B8): "Accepts delegation from: Planner, Travel · May delegate to: Travel". Nothing for a Bot that is not published, and no
     * "May delegate to" for a Bot that may ask none. The limit its maintainer set is not shown: a member never reads the limits that were configured.
     */
    fun delegation(bot: TeamBotCard, cards: List<TeamBotCard>): List<TeamBotItem> {
        if (bot.state != "published") return emptyList()
        val accepts = when (val kind = TeamDelegations.accepts(bot, cards)) {
            TeamDelegations.Accepts.Nobody -> TeamBotValue.Words(R.string.teams_bot_accepts_none)
            TeamDelegations.Accepts.TeamBots -> TeamBotValue.Words(R.string.teams_bot_accepts_team)
            is TeamDelegations.Accepts.Listed -> TeamBotValue.Figure(kind.names.joinToString(", "))
        }
        val asks = TeamDelegations.delegatesTo(bot, cards)
        return listOf(TeamBotItem(R.string.teams_bot_accepts_delegation, accepts)) +
            if (asks.isEmpty()) emptyList<TeamBotItem>() else listOf(TeamBotItem(R.string.teams_bot_delegates_to, TeamBotValue.Figure(asks.joinToString(", "))))
    }

    /** The member's own requests left today, or Unknown when the service has not said. */
    fun remaining(bot: TeamBotCard) = TeamBotItem(
        R.string.teams_bot_remaining_today, bot.dailyRemaining?.let { TeamBotValue.Figure(it.toString()) } ?: TeamBotValue.Words(R.string.chat_status_unknown),
    )

    private val resetFormat = DateTimeFormatter.ofPattern("uuuu-MM-dd HH:mm 'UTC'").withZone(ZoneOffset.UTC)

    /** When the daily quota resets, in UTC, or Unknown when the service has not said. */
    fun resets(bot: TeamBotCard) = TeamBotItem(
        R.string.teams_bot_resets, bot.quotaResetsAt?.let { TeamBotValue.Figure(resetFormat.format(Instant.ofEpochMilli(it))) } ?: TeamBotValue.Words(R.string.chat_status_unknown),
    )
}
