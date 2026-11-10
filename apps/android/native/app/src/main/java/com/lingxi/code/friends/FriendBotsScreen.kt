package com.lingxi.code.friends

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
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
import com.lingxi.code.teams.TeamMember
import com.lingxi.code.teams.TeamMessagingState
import com.lingxi.code.teams.TeamsErrorBanner
import com.lingxi.code.teams.TeamsOfflineBanner
import com.lingxi.code.teams.botAvailabilityText
import com.lingxi.code.theme.BotAvatar

/**
 * The Bots of a friendship (docs/FRIENDS-CLIENTS.md D21, D23): the Bot cards the pinned pair workspace's `listSharedBots` returned, which are
 * the friend's Bots the person may use and the person's own, then what a Bot runs on and the hint that sharing starts on Desktop. Nothing is
 * filtered here: the server decided. Message opens the Bot DM through [onMessage]; Stop sharing, on the person's own published Bot only, asks
 * first and then runs [onStop] (the Teams `pauseSharedBot`). It offers no other Bot management, and an Android phone cannot publish a Bot.
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
) {
    val online = !messaging.offline
    Column(modifier.fillMaxSize().testTag(FriendsTags.BOTS_SCREEN)) {
        FriendsPageHeader(stringResource(R.string.friends_bots_title), showBack, onBack)
        if (messaging.directoryReady && messaging.offline) TeamsOfflineBanner(onReconnect)
        if (messaging.error.isNotEmpty() && messaging.error != "SERVICE_UNAVAILABLE") TeamsErrorBanner(messaging.error, onRetry = onReconnect)
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (messaging.bots.isEmpty()) item(key = "status") {
                Text(
                    stringResource(if (messaging.botsFailed) R.string.teams_bots_unverified else if (!messaging.botsLoaded) R.string.teams_bots_loading else R.string.friends_bots_none),
                    Modifier.testTag(FriendsTags.BOTS_STATUS), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            items(messaging.bots, key = { "bot-${it.sharedBotId}" }) { bot ->
                FriendBotRow(bot, ownerName(bot, members, uid), uid, generation, online, onMessage = { onMessage(bot.sharedBotId) }, onStop = { onStop(bot) })
            }
            item(key = "note") {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(R.string.friends_bots_note), Modifier.testTag(FriendsTags.BOTS_NOTE), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(stringResource(R.string.friends_bots_share_hint), Modifier.testTag(FriendsTags.BOTS_SHARE_HINT), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
 * usable Bot and a connection; Stop sharing is the person's own published Bot only, and asks first (D23).
 */
@Composable
private fun FriendBotRow(bot: TeamBotCard, owner: String, uid: String, generation: Long, online: Boolean, onMessage: () -> Unit, onStop: () -> Unit) {
    var confirm by remember(bot.sharedBotId, bot.revision, bot.state) { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp).testTag(FriendsTags.bot(bot.sharedBotId)), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        BotAvatar(bot.avatarId, 40.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(bot.name, style = MaterialTheme.typography.titleMedium)
            if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
            Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, owner)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (!bot.permitted) Text(stringResource(R.string.teams_bot_not_permitted), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row {
                TextButton(enabled = online && bot.selectable, onClick = onMessage, modifier = Modifier.testTag(FriendsTags.botMessage(bot.sharedBotId))) { Text(stringResource(R.string.teams_bot_message)) }
                if (bot.canPause(uid, generation)) {
                    TextButton(enabled = online, onClick = { confirm = true }, modifier = Modifier.testTag(FriendsTags.botStop(bot.sharedBotId))) { Text(stringResource(R.string.friends_share_bot_stop)) }
                }
            }
        }
    }
    if (confirm) AlertDialog(
        onDismissRequest = { confirm = false },
        title = { Text(stringResource(R.string.friends_share_bot_stop)) },
        text = { Text(stringResource(R.string.teams_bot_unbind_detail)) },
        confirmButton = { TextButton(onClick = { confirm = false; onStop() }) { Text(stringResource(R.string.friends_share_bot_stop), color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text(stringResource(R.string.common_cancel)) } },
    )
}
