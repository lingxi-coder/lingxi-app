package com.lingxi.code.remote

import androidx.annotation.StringRes
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi

/*
 * What the Bots tab (`bots/BotsTab.kt`) needs to know of a computer's Bots and their chats, without any drawing:
 * the data models of the retired Bot page. The tree is built from them by `bots/BotsLogic.kt`.
 */

/** The Bots to list for [hostId]: none while the held state is another computer's, still unknown, or says the computer has no Bots to show. */
fun RemoteBotsState.listFor(hostId: String): List<RemoteBotSummaryFfi> =
    if (this.hostId == hostId) (bots as? RemoteBotsStateFfi.Bots)?.bots.orEmpty() else emptyList()

/** The name of [botId] among the Bots listed for [hostId]. */
fun RemoteBotsState.nameOf(hostId: String, botId: String): String? =
    listFor(hostId).firstOrNull { it.botId == botId }?.name

/** The summary of the chat [sessionId] among the chats held for the Bots of [hostId]; a Bot's chat is not among the sessions'. */
fun RemoteBotChatsState.summaryOf(hostId: String, sessionId: String): RemoteSessionSummaryFfi? =
    if (this.hostId != hostId) null
    else byBot.values.firstNotNullOfOrNull { held -> held.threads?.firstOrNull { it.sessionId == sessionId } }

/** What an open Bot shows. */
sealed interface RemoteBotPageUi {
    /** The Bots of the computer are not known yet. */
    data object Loading : RemoteBotPageUi

    /** The Bot is not there: its switch went off, it was deleted, or the computer does not know Bots. */
    data object Gone : RemoteBotPageUi

    data class Shown(val bot: RemoteBotSummaryFfi, val chats: RemoteBotChatsUi) : RemoteBotPageUi
}

/** The chats of a Bot: the list once it has been asked for, and whether the last asking failed. */
data class RemoteBotChatsUi(val threads: List<RemoteSessionSummaryFfi>?, val failed: Boolean) {
    val loading: Boolean get() = threads == null && !failed
    val empty: Boolean get() = threads?.isEmpty() == true
}

/** The codes of a refusal that mean the Bot is no longer reachable. */
private val BOT_GONE_CODES = setOf("unknown_session", "not_remote_enabled")

/** What an open [botId] on [hostId] shows for what is held of the computer's Bots and of the Bot's chats; the rules mirror the web viewer. */
fun remoteBotPageUi(hostId: String, botId: String, bots: RemoteBotsState, chats: RemoteBotChatsState): RemoteBotPageUi {
    val listed = when (val state = if (bots.hostId == hostId) bots.bots else RemoteBotsStateFfi.Unknown) {
        RemoteBotsStateFfi.Unknown -> return RemoteBotPageUi.Loading
        RemoteBotsStateFfi.Unsupported -> return RemoteBotPageUi.Gone
        is RemoteBotsStateFfi.Bots -> state.bots
    }
    val bot = listed.firstOrNull { it.botId == botId } ?: return RemoteBotPageUi.Gone
    val held = chats.takeIf { it.hostId == hostId }?.byBot?.get(botId)
    // The chats failed because the Bot is no longer there; the Bots list says so too, a moment later.
    if (held?.error?.code in BOT_GONE_CODES) return RemoteBotPageUi.Gone
    return RemoteBotPageUi.Shown(bot, RemoteBotChatsUi(held?.threads, failed = held?.error != null))
}

/** A Bot's status is one of a session's, less the one that only a session can be in. */
@StringRes
internal fun botStatusText(status: RemoteBotStatusFfi): Int = when (status) {
    RemoteBotStatusFfi.IDLE -> R.string.remote_status_idle
    RemoteBotStatusFfi.RUNNING -> R.string.remote_status_running
    RemoteBotStatusFfi.REQUIRES_ACTION -> R.string.remote_status_requires_action
}

/** Why starting a chat failed: the Bot going away is told as such, anything else as any refused request is. */
@StringRes
internal fun botCreateErrorText(error: Throwable): Int = when ((error as? RemoteFfiException.Rpc)?.code) {
    "unknown_session", "not_remote_enabled" -> R.string.remote_bot_gone
    "capacity" -> R.string.remote_err_capacity
    "forbidden" -> R.string.remote_err_forbidden
    else -> R.string.remote_err_generic
}
