package com.lingxi.code.teams

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

    /** Explicit host availability blocks new Bot requests; device network state is a separate existing queue concern. */
    fun blockedRequests(targets: Collection<String>, bots: List<TeamBotCard>, continuation: Boolean): List<TeamBotCard> =
        if (continuation) emptyList() else bots.filter { it.sharedBotId in targets && (it.admissionBlockedPendingRefresh || it.availability in unavailableStatuses) }

    /** Called by queue before journaling a new Bot submit, as well as by the composer gate. */
    fun requireNewRequest(targets: Collection<String>, bots: List<TeamBotCard>) {
        val blocked = blockedRequests(targets, bots, continuation = false).firstOrNull() ?: return
        throw TeamMessageFailure(when {
            blocked.admissionBlockedPendingRefresh -> "BOT_STATUS_UNVERIFIED"
            blocked.availability == "quota_exhausted" -> "REQUEST_QUOTA_EXCEEDED"
            else -> "HOST_UNAVAILABLE"
        })
    }

    fun canSeed(draft: ComposerDraft, hasFiles: Boolean, example: String): Boolean =
        draft == ComposerDraft() && !hasFiles && TeamsComposeLogic.canSendText(example)

    /** The member projection is the sole health source; old ready/quotas fields do not invent a new projection. */
    fun availability(bot: TeamBotCard, chinese: Boolean): String = when (if (bot.state != "published") "paused" else bot.availability) {
        "ready" -> if (chinese) "可用" else "Ready"
        "paused" -> if (chinese) "已暂停" else "Paused"
        "offline" -> if (chinese) "宿主离线" else "Host offline"
        "maintenance" -> if (chinese) "维护中" else "Under maintenance"
        "quota_exhausted" -> if (chinese) "今日额度已用完" else "Daily quota exhausted"
        else -> if (chinese) "状态待核实" else "Status unverified"
    }

    fun capabilities(bot: TeamBotCard, chinese: Boolean): String {
        val value = bot.capabilities ?: return if (chinese) "能力待核实" else "Capabilities unverified"
        val labels = mutableListOf(if (chinese) "对话" else "Conversation")
        if (value.durableWait) labels += if (chinese) "可持续等待回复或批准" else "Waits for replies or approval"
        if (value.writeOperations) labels += if (chinese) "可写入，依权限执行" else "Writes with permission"
        return labels.joinToString(" · ")
    }

    fun quota(bot: TeamBotCard, chinese: Boolean): String {
        val remaining = bot.dailyRemaining?.let { if (chinese) "本人今日剩余 $it 次" else "Your remaining uses today: $it" }
            ?: if (chinese) "本人剩余额度待核实" else "Your remaining quota is unverified"
        val reset = bot.quotaResetsAt?.let { DateTimeFormatter.ofPattern("uuuu-MM-dd HH:mm 'UTC'").withZone(ZoneOffset.UTC).format(Instant.ofEpochMilli(it)) }
        return remaining + if (reset != null) { if (chinese) " · 重置 $reset" else " · Resets $reset" }
        else if (chinese) " · 重置时间待核实（UTC）" else " · Reset time unverified (UTC)"
    }
}
