package com.lingxi.code.remote

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteBotStatusFfi
import com.lingxi.code.bindings.android.RemoteBotSummaryFfi
import com.lingxi.code.bindings.android.RemoteBotsStateFfi
import com.lingxi.code.bindings.android.RemoteFfiException
import com.lingxi.code.bindings.android.RemoteSessionSummaryFfi
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

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

/** What the page of one Bot shows. */
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

/** The page of [botId] on [hostId] for what is held of the computer's Bots and of the Bot's chats; the rules mirror the web viewer. */
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

/** What to say above a Bot's page while the computer is not connected, or null while it is. */
@StringRes
internal fun botConnectionNote(connection: RemoteConnectionUi): Int? = when (connection) {
    is RemoteConnectionUi.Ready -> null
    is RemoteConnectionUi.Failed -> connection.message
    is RemoteConnectionUi.Pairing -> R.string.remote_pairing_waiting
    is RemoteConnectionUi.Connecting, is RemoteConnectionUi.Reconnecting, RemoteConnectionUi.Idle -> R.string.remote_reconnecting
}

/**
 * One Bot of the connected computer: what it is, its chats newest first, and a way to start a new one.
 * The chats are asked for while this is shown. A chat the user starts is made on the computer whether
 * or not the screen waits for it; a screen that was left is not taken to it afterwards.
 */
@Composable
internal fun RemoteBotScreen(
    manager: RemoteControlManager,
    hostId: String,
    botId: String,
    onBack: () -> Unit,
    onOpenSession: (String) -> Unit,
) {
    val connection by manager.connection.collectAsState()
    val bots by manager.bots.collectAsState()
    val chats by manager.botChats.collectAsState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(manager, hostId, botId) { runCatching { manager.showBotChats(hostId, botId) } }
    var creating by remember(hostId, botId) { mutableStateOf(false) }
    var createError by remember(hostId, botId) { mutableStateOf<Int?>(null) }
    RemoteBotPage(
        page = remoteBotPageUi(hostId, botId, bots, chats),
        note = botConnectionNote(remoteConnectionUi(connection?.takeIf { it.hostId == hostId })),
        creating = creating,
        createError = createError,
        onBack = onBack,
        onNewChat = {
            if (!creating) {
                creating = true
                createError = null
                scope.launch {
                    try {
                        onOpenSession(manager.createBotThread(hostId, botId))
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (error: Exception) {
                        creating = false
                        createError = botCreateErrorText(error)
                    }
                }
            }
        },
        onRetry = { scope.launch { runCatching { manager.retryBotChats(hostId, botId) } } },
        onOpenChat = { onOpenSession(it.sessionId) },
    )
}

/** The page of one Bot for what [RemoteBotScreen] holds; stateless, so it can be shown in every state. */
@Composable
internal fun RemoteBotPage(
    page: RemoteBotPageUi,
    @StringRes note: Int?,
    creating: Boolean,
    @StringRes createError: Int?,
    onBack: () -> Unit,
    onNewChat: () -> Unit,
    onRetry: () -> Unit,
    onOpenChat: (RemoteSessionSummaryFfi) -> Unit,
) {
    val t = LingXiTheme.palette
    RemotePage((page as? RemoteBotPageUi.Shown)?.bot?.name ?: stringResource(R.string.remote_bots), onBack = onBack) {
        LazyColumn(
            Modifier.fillMaxSize().padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            note?.let { item(key = "note") { Text(stringResource(it), color = t.text3, fontSize = 13.sp) } }
            when (page) {
                RemoteBotPageUi.Loading -> item(key = "loading") {
                    Text(stringResource(R.string.remote_loading), color = t.text3, fontSize = 14.sp)
                }
                RemoteBotPageUi.Gone -> item(key = "gone") {
                    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(stringResource(R.string.remote_bot_gone), color = t.text, fontSize = 14.sp)
                        OutlinedButton(onClick = onBack) { Text(stringResource(R.string.remote_back)) }
                    }
                }
                is RemoteBotPageUi.Shown -> {
                    val bot = page.bot
                    val chats = page.chats
                    item(key = "head") {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                                if (bot.description.isNotBlank()) Text(bot.description, color = t.text2, fontSize = 14.sp)
                                if (bot.waitingThreads > 0u) {
                                    Text(
                                        stringResource(R.string.remote_bot_waiting_fmt, bot.waitingThreads.toInt()),
                                        color = t.accent, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                                    )
                                }
                            }
                            Text(stringResource(botStatusText(bot.status)), color = t.text3, fontSize = 13.sp)
                        }
                    }
                    item(key = "new") {
                        Button(onClick = onNewChat, enabled = note == null && !creating) {
                            Text(stringResource(R.string.remote_bot_new_chat))
                        }
                    }
                    createError?.let { item(key = "new-error") { Text(stringResource(it), color = t.danger, fontSize = 14.sp) } }
                    if (chats.loading) item(key = "chats-loading") {
                        Text(stringResource(R.string.remote_loading), color = t.text3, fontSize = 14.sp)
                    }
                    if (chats.failed) item(key = "chats-error") {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(stringResource(R.string.remote_err_generic), color = t.danger, fontSize = 14.sp)
                            TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
                        }
                    }
                    if (chats.empty) item(key = "chats-empty") {
                        Text(stringResource(R.string.remote_bot_no_chats), color = t.text, fontWeight = FontWeight.Medium)
                    }
                    items(chats.threads.orEmpty(), key = { it.sessionId }) { chat -> RemoteSessionRow(chat, onOpenChat) }
                }
            }
        }
    }
}
