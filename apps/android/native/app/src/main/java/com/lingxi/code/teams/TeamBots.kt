package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest

internal fun teamBotTriggers(message: TeamMessage, conversation: TeamMessageConversation?): List<String> {
    message.triggerId?.let { return listOf(it) }
    if (message.authorKind != "user" || message.recalled) return emptyList()
    val keys = if (conversation?.kind == "bot_dm") conversation.botKeys else message.targets
    return keys.map { key -> MessageDigest.getInstance("SHA-256").digest(JSONArray(listOf(message.id, key)).toString().toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) } }
}

internal fun visibleTeamBotTriggers(messages: List<TeamMessage>, conversation: TeamMessageConversation?, limit: Int) =
    messages.takeLast(maxOf(100, limit)).flatMap { teamBotTriggers(it, conversation) }.distinct()

internal fun teamBotStateQuery(conversationId: String, triggerId: String, questionId: String? = null) =
    JSONObject().put("action", "botState").put("conversationId", conversationId).put("triggerId", triggerId).also { value -> questionId?.let { value.put("questionId", it) } }

internal fun canDiscardTeamBotIntent(intent: JSONObject, now: Long) =
    !(intent.optBoolean("attempted") && !intent.has("error")) && intent.optLong("leaseUntil") <= now

internal fun teamBotSettlementIntent(cache: TeamMessageCache, conversationId: String, triggerId: String, run: TeamBotRun, text: String, settlementId: String, now: Long): JSONObject {
    if (!run.canRecover(cache.uid, now) || text.isBlank() || text.toByteArray(Charsets.UTF_8).size > 8192) throw TeamMessageFailure("BOT_RECOVERY_REQUIRED")
    if (cache.settlements.any { it.getJSONObject("body").getString("conversationId") == conversationId && it.getJSONObject("body").getString("triggerId") == triggerId }) throw TeamMessageFailure("MESSAGE_IN_FLIGHT")
    val body = JSONObject().put("action", "recoverBot").put("teamId", cache.teamId).put("generation", cache.generation)
        .put("conversationId", conversationId).put("botKey", run.botKey).put("triggerId", triggerId).put("text", text)
    // The local identity is not a new backend parameter. The immutable wire body
    // is retried exactly; backend trigger + text digest supplies its receipt.
    return JSONObject().put("settlementId", settlementId).put("body", body)
}

internal data class TeamBotQuestion(val id: String, val questions: List<String>) {
    fun json() = JSONObject().put("id", id).put("questions", JSONArray(questions))
    companion object { fun parse(v: JSONObject) = TeamBotQuestion(v.getString("id"), v.getJSONArray("questions").strings()) }
}

internal data class TeamBotCard(val key: String, val ownerUid: String, val name: String, val description: String, val revision: Long, val use: String, val permitted: Boolean, val online: Boolean, val enabled: Boolean = true, val onlineUntil: Long = 0) {
    val selectable get() = enabled && permitted
    fun canUnbind(uid: String) = enabled && ownerUid == uid
    val inactiveLabel get() = "Inactive · revision $revision. Rebind on Desktop."
    companion object { fun parse(v: JSONObject) = TeamBotCard(v.getString("key"), v.getString("ownerUid"), v.getString("name"), v.getString("description"), v.getLong("revision"), v.getString("use"), v.getBoolean("permitted"), v.getBoolean("online"), v.optBoolean("enabled", true), v.optLong("onlineUntil")) }
}

/** Preserve selected keys until the human clears them; never downgrade a Bot draft to plain text. */
internal fun staleTeamBotSelections(selected: Collection<String>, bots: List<TeamBotCard>, participants: Collection<String>? = null): Set<String> =
    selected.filterTo(mutableSetOf()) { key -> bots.none { it.key == key && it.selectable } || (participants != null && key !in participants) }

internal fun requireTeamBotSelections(selected: Collection<String>, bots: List<TeamBotCard>, participants: Collection<String>? = null) {
    if (selected.size > 8 || staleTeamBotSelections(selected, bots, participants).isNotEmpty()) throw TeamMessageFailure("BOT_SELECTION_STALE")
}

/** Never persisted as authority: only a fresh authenticated observation enables an answer. */
internal data class TeamBotRun(val state: String, val ownerUid: String, val botKey: String, val originUid: String, val originGeneration: Long, val leaseUntil: Long, val questionId: String?, val answered: Boolean) {
    fun canAnswer(message: TeamMessage, uid: String, generation: Long, now: Long): Boolean =
        state == "running" && leaseUntil > now && !answered && questionId == message.question?.id &&
            originUid == uid && originGeneration == generation && message.originUid == uid && message.originGeneration == generation &&
            message.authorKind == "bot" && message.botKey == botKey && !message.recalled
    fun canRecover(uid: String, now: Long) = ownerUid == uid && (state == "recovery" || (state in setOf("running", "claimed") && leaseUntil <= now))
    fun canRetryAnswer(intent: JSONObject, uid: String, generation: Long, now: Long) =
        originUid == uid && originGeneration == generation && questionId == intent.getString("questionId") &&
            (answered || (state == "running" && leaseUntil > now))
    companion object {
        fun parse(v: JSONObject): TeamBotRun {
            val origin = v.getJSONObject("origin")
            return TeamBotRun(v.getString("state"), v.getString("ownerUid"), v.getString("botKey"), origin.getString("uid"), origin.getLong("generation"), v.optLong("leaseUntil"), v.stringOrNull("questionId"), v.getBoolean("answered"))
        }
    }
}

internal fun teamBotAnswerIntent(cache: TeamMessageCache, conversationId: String, message: TeamMessage, run: TeamBotRun, values: Map<String, String>, answerId: String, now: Long): JSONObject {
    val question = message.question ?: throw TeamMessageFailure("INVALID_ANSWER")
    if (!run.canAnswer(message, cache.uid, cache.generation, now) || values.keys != question.questions.toSet() || values.values.any { it.isBlank() || it.length > 2000 }) throw TeamMessageFailure("QUESTION_RECIPIENT_REQUIRED")
    if (cache.answers.any { it.getString("questionId") == question.id && it.getString("conversationId") == conversationId }) throw TeamMessageFailure("ANSWER_PENDING")
    return JSONObject().put("action", "botAnswer").put("teamId", cache.teamId).put("generation", cache.generation)
        .put("conversationId", conversationId).put("triggerId", requireNotNull(message.triggerId)).put("questionId", question.id)
        .put("answerId", answerId).put("answers", JSONObject(values.toSortedMap()))
}

/** After the first send, only the backend's immutable answer receipt can decide
 * whether delivery committed. Mutable public run state may already be revoked. */
internal fun teamBotAnswerRetryBody(cache: TeamMessageCache, intent: JSONObject, run: TeamBotRun?, now: Long): JSONObject {
    if (intent.getString("action") != "botAnswer" || intent.getString("teamId") != cache.teamId || intent.getLong("generation") != cache.generation) throw TeamMessageFailure("INVALID_SESSION")
    if (!intent.optBoolean("attempted") && run?.canRetryAnswer(intent, cache.uid, cache.generation, now) != true) throw TeamMessageFailure("QUESTION_RECIPIENT_REQUIRED")
    return JSONObject(intent.toString()).also { it.remove("attempted"); it.remove("error") }
}
