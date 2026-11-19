package com.lingxi.code.teams

import android.content.Context
import androidx.core.app.ShareCompat
import com.lingxi.code.navigation.DeepLink

/**
 * Shared conversation links (`team/chat`, `team/bot`), the same rules as `packages/teams/share.ts`: a link opens the
 * conversation in the LingXi app where it is installed and on the web otherwise.
 */
internal object TeamShareLinks {
    /**
     * The link of one conversation. A Bot's direct chat is shared as the Bot, since each person has their own chat with it;
     * a friend chat is its pair team's conversation, which both friends can open. Null for a Bot chat without its Bot, or an
     * id a link cannot carry.
     */
    fun conversation(teamId: String, conversation: TeamMessageConversation): String? {
        val bot = if (conversation.kind == "bot_dm") conversation.participants.firstOrNull { it.startsWith("shared_bot:") }?.removePrefix("shared_bot:") ?: return null else null
        return runCatching {
            if (bot != null) DeepLink.build("team/bot", mapOf("team" to teamId, "bot" to bot))
            else DeepLink.build("team/chat", mapOf("team" to teamId, "conversation" to conversation.id))
        }.getOrNull()
    }

    /** The system share sheet for a conversation's link, titled "Open … in LingXi". */
    fun share(context: Context, link: String, title: String) {
        ShareCompat.IntentBuilder(context).setType("text/plain").setText(link)
            .setSubject(context.getString(com.lingxi.code.R.string.teams_share_text_fmt, title))
            .setChooserTitle(context.getString(com.lingxi.code.R.string.teams_share_link)).startChooser()
    }

    /** The friend chat of a friendship's pair team, which is the conversation every friend link names. */
    fun friendChat(pairTeamId: String): String? = runCatching { DeepLink.build("team/chat", mapOf("team" to pairTeamId, "conversation" to "friend_chat")) }.getOrNull()

    /** What a shared conversation link asks for; a friendship's pair team (`f_…`) is opened in Friends. */
    sealed interface Request {
        val teamId: String
        data class Conversation(override val teamId: String, val conversationId: String) : Request
        data class Bot(override val teamId: String, val botId: String) : Request
        data class Pair(override val teamId: String) : Request
    }

    fun request(link: DeepLink?): Request? {
        val team = link?.params?.get("team") ?: return null
        return when {
            link.route != "team/chat" && link.route != "team/bot" -> null
            team.startsWith("f_") -> Request.Pair(team)
            link.route == "team/chat" -> link.params["conversation"]?.let { Request.Conversation(team, it) }
            else -> link.params["bot"]?.let { Request.Bot(team, it) }
        }
    }

    /** The next step toward a team link. A team the viewer is not in is [Step.Unavailable], never another team. */
    sealed interface Step {
        data object Wait : Step
        data object Unavailable : Step
        data object SelectTeam : Step
        data class Open(val conversationId: String) : Step
        /** A Bot without a chat of the viewer's: its team's details, where starting one is the viewer's choice. */
        data object Bots : Step
    }

    fun step(request: Request, uid: String?, loaded: Boolean, teamIds: List<String>, selectedId: String?, messaging: TeamMessagingState): Step {
        if (uid == null || !loaded) return Step.Wait
        if (request.teamId !in teamIds) return Step.Unavailable
        if (selectedId != request.teamId) return Step.SelectTeam
        if (messaging.cache?.teamId != request.teamId) return Step.Wait
        return when (request) {
            is Request.Conversation -> Step.Open(request.conversationId)
            is Request.Pair -> Step.Unavailable
            is Request.Bot -> when {
                !messaging.botsLoaded -> Step.Wait
                messaging.bots.none { it.sharedBotId == request.botId } -> Step.Unavailable
                else -> TeamsListLogic.findBotDm(messaging.cache?.conversations.orEmpty(), uid, request.botId, messaging.bots)?.let { Step.Open(it.id) } ?: Step.Bots
            }
        }
    }
}
