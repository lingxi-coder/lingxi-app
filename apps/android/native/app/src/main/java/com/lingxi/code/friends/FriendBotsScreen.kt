package com.lingxi.code.friends

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.teams.TeamBotCard
import com.lingxi.code.teams.TeamMemberRef
import com.lingxi.code.teams.TeamSharedListRules
import com.lingxi.code.teams.TeamTriageRules
import com.lingxi.code.teams.TeamMember
import com.lingxi.code.teams.StopSharingDialog
import com.lingxi.code.teams.TeamHeldEntry
import com.lingxi.code.teams.TeamMessagingModel
import com.lingxi.code.teams.TeamMessagingState
import com.lingxi.code.teams.TeamsMessageState
import com.lingxi.code.teams.TeamsConnectionMenu
import com.lingxi.code.teams.botAvailabilityText
import com.lingxi.code.theme.BotAvatar

/**
 * The Bots of a friendship (docs/FRIENDS-CLIENTS.md D21, D23): the Bot cards the pinned pair workspace's `listSharedBots` returned, which are
 * the friend's Bots the person may use and the person's own, then what a Bot runs on and the hint that sharing starts on Desktop. Nothing is
 * filtered here: the server decided. Message opens the Bot DM through [onMessage]; Stop sharing, on the person's own published Bot only, asks
 * first and then runs [onStop] (the Teams `pauseSharedBot`). It offers no other Bot management, and an Android phone cannot publish a Bot.
 *
 * [onTriageLog] opens the auto-reply log of one of the person's own Bots that answers on its own; [triageDenied] holds the Bots the service said the person does not
 * maintain after all, whose entry is not offered again. [heldModel] reads the replies of the person's own Bots that wait for them to let them go ("Replies waiting"), and
 * [onHeldChat] opens the Bot chat one of them was made in.
 *
 * [members] is the pair's two-person directory, for the maintainer's name; [generation] is the friendship's. A Bot command that ends the chat
 * (`messaging.revoked`) is handled by the chat that hosts this page, which replaces the whole area.
 */
@Composable
internal fun FriendBotsScreen(
    messaging: TeamMessagingState,
    members: List<TeamMember>,
    uid: String,
    generation: Long,
    showBack: Boolean,
    onBack: () -> Unit,
    onMessage: (String) -> Unit,
    onStop: (TeamBotCard) -> Unit,
    onReconnect: () -> Unit,
    modifier: Modifier = Modifier,
    onList: (TeamBotCard) -> Unit = {},
    onTriageLog: (TeamBotCard) -> Unit = {},
    triageDenied: Set<String> = emptySet(),
    heldModel: TeamMessagingModel? = null,
    onHeldChat: (String) -> Unit = {},
) {
    val online = !messaging.offline
    // The read has not settled: it failed, or it is still out. A settled empty list is the empty state.
    val unsettled = messaging.botsFailed || !messaging.botsLoaded
    Column(modifier.fillMaxSize().testTag(FriendsTags.BOTS_SCREEN)) {
        FriendsPageHeader(stringResource(R.string.friends_bots_title), showBack, onBack) {
            TeamsConnectionMenu(
                error = messaging.error.takeIf { it.isNotEmpty() && it != "SERVICE_UNAVAILABLE" },
                offline = messaging.directoryReady && messaging.offline,
                onReconnect = onReconnect,
            )
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (messaging.bots.isEmpty()) item(key = "status") {
                if (unsettled) {
                    Text(
                        stringResource(if (messaging.botsFailed) R.string.teams_bots_unverified else R.string.teams_bots_loading),
                        Modifier.testTag(FriendsTags.BOTS_STATUS), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    // U9: loaded and empty says what is empty, then how a Bot gets here.
                    TeamsMessageState(stringResource(R.string.friends_bots_none), Modifier.testTag(FriendsTags.BOTS_STATUS), body = stringResource(R.string.friends_bots_share_hint), fill = false)
                }
            }
            items(messaging.bots, key = { "bot-${it.sharedBotId}" }) { bot ->
                FriendBotRow(bot, ownerName(bot, members, uid), uid, generation, online, onMessage = { onMessage(bot.sharedBotId) }, onStop = { onStop(bot) },
                    onList = if (TeamSharedListRules.canOpen(bot, uid, generation)) ({ onList(bot) }) else null,
                    onTriageLog = if (TeamTriageRules.maintains(bot, TeamMemberRef(uid, generation)) && bot.autoReply != null && bot.sharedBotId !in triageDenied) ({ onTriageLog(bot) }) else null,
                    heldModel = heldModel.takeIf { TeamTriageRules.maintains(bot, TeamMemberRef(uid, generation)) }, heldSignals = messaging.botSignals, onHeldChat = onHeldChat)
            }
            item(key = "note") {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(R.string.friends_bots_note), Modifier.testTag(FriendsTags.BOTS_NOTE), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    // With nothing to list, the hint is the body of the empty state above instead of a second copy here.
                    if (messaging.bots.isNotEmpty() || unsettled) {
                        Text(stringResource(R.string.friends_bots_share_hint), Modifier.testTag(FriendsTags.BOTS_SHARE_HINT), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

/** The maintainer's name from the pair's directory; the person's own Bot reads "You" while the directory does not know their name yet. */
@Composable
private fun ownerName(bot: TeamBotCard, members: List<TeamMember>, uid: String): String =
    members.firstOrNull { it.uid == bot.maintainer.uid }?.displayName?.takeIf { it.isNotEmpty() }
        ?: stringResource(if (bot.maintainer.uid == uid) R.string.teams_preview_you else R.string.teams_member)

/**
 * One Bot of the friendship: the Teams Bot card without its audience and quota lines, which a friendship does not have (D21). Message needs a
 * usable Bot and a connection; the auto-reply log is the maintainer's, for a Bot that answers on its own, and so are the replies that wait for them; Stop sharing is the person's own
 * published Bot only, and asks first (D23). The (up to five) actions wrap under each other rather than run out of the row at a large font size.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FriendBotRow(
    bot: TeamBotCard, owner: String, uid: String, generation: Long, online: Boolean, onMessage: () -> Unit, onStop: () -> Unit, onList: (() -> Unit)? = null, onTriageLog: (() -> Unit)? = null,
    heldModel: TeamMessagingModel? = null, heldSignals: Long = 0, onHeldChat: (String) -> Unit = {},
) {
    var confirm by remember(bot.sharedBotId, bot.revision, bot.state) { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp).testTag(FriendsTags.bot(bot.sharedBotId)), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        BotAvatar(bot.avatarId, 56.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(bot.name, style = MaterialTheme.typography.titleMedium)
            if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
            Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, owner)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (!bot.permitted) Text(stringResource(R.string.teams_bot_not_permitted), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            FlowRow {
                TextButton(enabled = online && bot.selectable, onClick = onMessage, modifier = Modifier.testTag(FriendsTags.botMessage(bot.sharedBotId))) { Text(stringResource(R.string.teams_bot_message)) }
                if (onList != null) TextButton(enabled = online, onClick = onList, modifier = Modifier.testTag("teams.bot.list.${bot.sharedBotId}")) { Text(stringResource(R.string.teams_list_open)) }
                if (onTriageLog != null) TextButton(enabled = online, onClick = onTriageLog, modifier = Modifier.testTag("teams.bot.triage.${bot.sharedBotId}")) { Text(stringResource(R.string.teams_triage_title)) }
                if (heldModel != null) TeamHeldEntry(bot, heldModel, heldSignals, online, onHeldChat)
                if (bot.canPause(uid, generation)) {
                    TextButton(enabled = online, onClick = { confirm = true }, modifier = Modifier.testTag(FriendsTags.botStop(bot.sharedBotId))) { Text(stringResource(R.string.friends_share_bot_stop)) }
                }
            }
        }
    }
    // U8: the one Stop sharing dialog of Teams and Friends, with the Bot's name in the title.
    if (confirm) StopSharingDialog(bot.name, onConfirm = { confirm = false; onStop() }, onDismiss = { confirm = false })
}
