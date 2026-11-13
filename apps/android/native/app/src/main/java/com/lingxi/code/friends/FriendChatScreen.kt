package com.lingxi.code.friends

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.RememberObserver
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.materialIcon
import com.lingxi.code.teams.FriendChatHeader
import com.lingxi.code.teams.PairTeam
import com.lingxi.code.teams.TeamBotCard
import com.lingxi.code.teams.TeamConversationScreen
import com.lingxi.code.teams.TeamDetail
import com.lingxi.code.teams.TeamSharedListDialog
import com.lingxi.code.teams.TeamsMessageState
import com.lingxi.code.teams.TeamsSheet
import com.lingxi.code.teams.TeamsState
import com.lingxi.code.teams.TeamsWorkspace
import com.lingxi.code.teams.formatDate
import com.lingxi.code.teams.messageBot
import com.lingxi.code.theme.PersonAvatar
import java.time.ZoneId

/**
 * A friend's chat (docs/FRIENDS-CLIENTS.md D9, D10): the Team conversation screen over a [TeamsWorkspace] pinned to the friendship's
 * pair team, with the friend in the header and no list, details, members or invites. The Bots action of the header (D21) opens the friendship's
 * Bots, and Message there opens a Bot DM (D22), on the same workspace, with Back returning to the chat. The friendship ending (the card goes
 * away, or the Teams layer reports the membership gone, whichever of the three views is open) replaces the whole area with "no longer
 * available" and a Back action (D25).
 */
@Composable
internal fun FriendChatScreen(
    workspace: FriendsWorkspace,
    state: FriendsState,
    uid: String,
    friendUid: String,
    showBack: Boolean,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val card = state.friendCard(friendUid)
    val pair = card?.pair
    when {
        !state.cardsLoaded -> Box(modifier.fillMaxSize().testTag(FriendsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        card == null || pair == null -> ChatUnavailable(showBack, onBack, modifier)
        else -> FriendChat(workspace, state, uid, card, pair, showBack, onBack, modifier)
    }
}

/** A [TeamsWorkspace] that lives exactly as long as it is remembered: started once composed, closed when forgotten or abandoned. */
private class PinnedChat(val workspace: TeamsWorkspace) : RememberObserver {
    override fun onRemembered() = workspace.start()
    override fun onForgotten() = workspace.close()
    override fun onAbandoned() = workspace.close()
}

@Composable
private fun FriendChat(
    workspace: FriendsWorkspace, state: FriendsState, uid: String, card: FriendCard, pair: PairRef, showBack: Boolean, onBack: () -> Unit, modifier: Modifier,
) {
    val context = LocalContext.current
    val friendUid = card.uid
    // The pair team's summary and its two-person directory come from the cards: no `GET /v1/teams` and no team-detail call (D9).
    val summary = remember(pair, card.displayName) { checkNotNull(FriendsLogic.pairSummary(card)) }
    val chat = remember(uid, pair) { PinnedChat(TeamsWorkspace.pinned(context.applicationContext, PairTeam(uid, summary))) }.workspace
    val myName = state.profile?.displayName.orEmpty()
    val teams = remember(uid, summary, myName) {
        val members = FriendsLogic.pairMembers(uid, myName, card)
        TeamsState(uid = uid, teams = listOf(summary), detail = TeamDetail(summary, members, emptyList()), selectedId = summary.id, loaded = true)
    }
    val messaging by chat.messaging.collectAsStateWithLifecycle()

    // What the area shows (D21, D22): the chat itself (empty), its Bots ("bots") or one Bot DM ("dm:<conversationId>"). One pinned workspace serves
    // all three, so a Bot command the server refuses because the friendship ended revokes the messaging of all of them at once (D25), and Back from
    // the Bots or from a Bot DM returns to the chat.
    var view by rememberSaveable(friendUid) { mutableStateOf("") }
    val inChat = view.isEmpty()
    BackHandler(enabled = !inChat && !messaging.revoked) { view = "" }

    // While the chat is open and in front it counts as read in the list and the badge (D13), which the Bots and a Bot DM do not; the name is
    // refreshed once per open (D10).
    val lifecycleState by LocalLifecycleOwner.current.lifecycle.currentStateFlow.collectAsState()
    val foreground = lifecycleState.isAtLeast(Lifecycle.State.RESUMED)
    DisposableEffect(friendUid, foreground, inChat) {
        if (foreground && inChat) workspace.chatOpened(friendUid)
        onDispose { workspace.chatClosed(friendUid) }
    }
    LaunchedEffect(friendUid) { workspace.refreshName(friendUid) }

    var dialog by rememberSaveable(friendUid) { mutableStateOf("") }
    // The shared list of one of the chat's Bots; it closes with the friendship's generation.
    var listOf by remember(friendUid, pair.generation) { mutableStateOf<TeamBotCard?>(null) }
    listOf?.let { bot -> TeamSharedListDialog(bot, chat.messagingModel) { listOf = null } }
    LaunchedEffect(dialog) { if (dialog == "profile") workspace.refreshName(friendUid) }

    if (messaging.revoked) {
        ChatUnavailable(showBack, onBack, modifier)
        return
    }
    when {
        view == "bots" -> FriendBotsScreen(
            messaging = messaging, members = teams.detail?.members.orEmpty(), uid = uid, generation = pair.generation, showBack = showBack, onBack = { view = "" },
            onMessage = { sharedBotId -> messageBot(chat, messaging, uid, sharedBotId) { view = "dm:$it" } },
            onStop = { bot -> chat.messagingModel.pause(bot) }, onReconnect = chat::reconnect, modifier = modifier, onList = { listOf = it },
        )
        view.startsWith("dm:") -> TeamConversationScreen(
            workspace = chat, uid = uid, team = summary, teams = teams, messaging = messaging,
            conversationId = view.removePrefix("dm:"), showBack = true, onBack = { view = "" }, modifier = modifier,
        )
        else -> Box(modifier.fillMaxSize().testTag(FriendsTags.CHAT)) {
            TeamConversationScreen(
                workspace = chat, uid = uid, team = summary, teams = teams, messaging = messaging,
                conversationId = FriendsLogic.CHAT_CONVERSATION_ID, showBack = showBack, onBack = onBack,
                friend = FriendChatHeader(friendUid, card.displayName, card.handle) {
                    IconButton(onClick = { view = "bots" }, modifier = Modifier.testTag(FriendsTags.BOTS)) {
                        Icon(LXIconName.Bot.materialIcon, stringResource(R.string.friends_bots_title))
                    }
                    FriendMenu(onProfile = { dialog = "profile" }, onRemove = { dialog = "remove" }, onBlock = { dialog = "block" })
                },
            )
        }
    }
    when (dialog) {
        "profile" -> FriendProfileSheet(card) { dialog = "" }
        "remove", "block" -> {
            val block = dialog == "block"
            EndFriendshipDialog(
                block = block, name = card.displayName.ifEmpty { stringResource(R.string.teams_member) }, op = state.rowOps[friendUid],
                onConfirm = {
                    val done = { dialog = ""; onBack() }
                    if (block) workspace.block(friendUid, done) else workspace.remove(friendUid, done)
                },
                onDismiss = { dialog = ""; workspace.clearRowError(friendUid) },
            )
        }
    }
}

@Composable
private fun ChatUnavailable(showBack: Boolean, onBack: () -> Unit, modifier: Modifier) {
    Column(modifier.fillMaxSize().testTag(FriendsTags.CHAT_UNAVAILABLE)) {
        FriendsPageHeader(stringResource(R.string.friends_title), showBack, onBack)
        TeamsMessageState(
            stringResource(R.string.friends_chat_unavailable),
            action = { Button(onClick = onBack) { Text(stringResource(R.string.remote_back)) } },
        )
    }
}

/** The chat header's menu (D10): View profile, Remove friend, Block. */
@Composable
private fun FriendMenu(onProfile: () -> Unit, onRemove: () -> Unit, onBlock: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, modifier = Modifier.testTag(FriendsTags.CHAT_MENU)) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.composer_more)) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(text = { Text(stringResource(R.string.friends_menu_view_profile)) }, onClick = { open = false; onProfile() })
            DropdownMenuItem(text = { Text(stringResource(R.string.friends_menu_remove), color = MaterialTheme.colorScheme.error) }, onClick = { open = false; onRemove() })
            DropdownMenuItem(text = { Text(stringResource(R.string.friends_menu_block), color = MaterialTheme.colorScheme.error) }, onClick = { open = false; onBlock() })
        }
    }
}

/** View profile (D10): the name, username and "friends since" of the friend, from the card and the session's profile read. */
@Composable
private fun FriendProfileSheet(card: FriendCard, onDismiss: () -> Unit) {
    val zone = remember { ZoneId.systemDefault() }
    val locale = LocalConfiguration.current.locales[0]
    TeamsSheet(stringResource(R.string.friends_menu_view_profile), onDismiss) {
        val name = card.displayName.ifEmpty { stringResource(R.string.teams_member) }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            PersonAvatar(card.uid, name, 56.dp)
            Column {
                Text(name, style = MaterialTheme.typography.titleLarge)
                card.handle?.let { Text("@$it", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
        if (card.sinceMs > 0) Text(
            stringResource(R.string.friends_since_fmt, formatDate(card.sinceMs, zone, locale, "yMMMd")),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** Remove friend and Block: the only alerts of Friends (D10, D18). The failure, if any, shows inside. */
@Composable
private fun EndFriendshipDialog(block: Boolean, name: String, op: FriendsOp?, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    val busy = op?.busy == true
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text(stringResource(if (block) R.string.friends_block_title_fmt else R.string.friends_remove_title_fmt, name)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(if (block) R.string.friends_block_body else R.string.friends_remove_body))
                op?.error?.let { FriendsErrorText(it) }
            }
        },
        confirmButton = {
            TextButton(onClick = onConfirm, enabled = !busy) {
                Text(stringResource(if (block) R.string.friends_menu_block else R.string.friends_menu_remove), color = MaterialTheme.colorScheme.error)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !busy) { Text(stringResource(R.string.common_cancel)) } },
    )
}
