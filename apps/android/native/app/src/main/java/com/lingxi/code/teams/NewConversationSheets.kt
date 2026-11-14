package com.lingxi.code.teams

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.TeamTile

/** The shared bottom sheet of the Teams tab: a title, a Close action and a keyboard-aware, scrollable body. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TeamsSheet(title: String, onDismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), modifier = Modifier.testTag(TeamsTags.SHEET)) {
        Column(Modifier.fillMaxWidth().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_close)) }
            }
            Column(Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}

/** Team switcher (SPEC section 6.2): one row per team with its unread sum, then Create / Join / Team details. */
@Composable
internal fun TeamSwitcherSheet(
    teams: List<TeamSummary>, selectedId: String, unreadByTeam: Map<String, Int>,
    onSelect: (String) -> Unit, onCreate: () -> Unit, onJoin: () -> Unit, onDetails: () -> Unit, onDismiss: () -> Unit, onServices: () -> Unit = {},
) {
    TeamsSheet(stringResource(R.string.teams_switch_team), onDismiss) {
        teams.forEach { team ->
            Row(
                Modifier.fillMaxWidth().clickable { onSelect(team.id) }.padding(vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                TeamTile(team.id, team.name, 36.dp)
                Column(Modifier.weight(1f)) {
                    Text(team.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text("${teamRoleLabel(team.role)} · ${stringResource(R.string.teams_members_count_fmt, team.memberCount)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                UnreadBadge(unreadByTeam[team.id] ?: 0)
                if (team.id == selectedId) Icon(Icons.Outlined.Check, null, tint = MaterialTheme.colorScheme.primary)
            }
        }
        HorizontalDivider()
        TextButton(onClick = onCreate) { Text(stringResource(R.string.teams_create)) }
        TextButton(onClick = onJoin) { Text(stringResource(R.string.teams_join)) }
        if (selectedId.isNotEmpty()) TextButton(onClick = onDetails) { Text(stringResource(R.string.teams_team_details)) }
        HorizontalDivider()
        // A Bot someone offers outside their Teams: not a Team, so not among them.
        TextButton(onClick = onServices, modifier = Modifier.testTag("teams.switcher.services")) { Text(stringResource(R.string.services_title)) }
    }
}

/** Create a team (mirrors the Desktop `CreateTeamDialog`): a name of at most 80 characters and the disclosure. */
@Composable
internal fun CreateTeamSheet(busy: Boolean, error: String?, onSubmit: (String) -> Unit, onDismiss: () -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    TeamsSheet(stringResource(R.string.teams_create), onDismiss) {
        OutlinedTextField(
            value = name, onValueChange = { if (it.length <= 80) name = it }, singleLine = true, enabled = !busy,
            label = { Text(stringResource(R.string.teams_name)) }, modifier = Modifier.fillMaxWidth(),
        )
        Text(stringResource(R.string.teams_disclosure), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (error != null) Text(stringResource(teamsErrorRes(error)), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        Button(onClick = { onSubmit(name) }, enabled = !busy && name.isNotBlank()) {
            if (busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.teams_create))
        }
    }
}

/**
 * Join a team with an invite link or the 32-character code. A link that opened the app only lands here, in [prefill]:
 * joining always takes this button.
 */
@Composable
internal fun JoinTeamSheet(prefill: String, busy: Boolean, error: String?, onSubmit: (String) -> Unit, onDismiss: () -> Unit) {
    var text by rememberSaveable(prefill) { mutableStateOf(prefill) }
    val code = TeamInvitation.code(text)
    TeamsSheet(stringResource(R.string.teams_join), onDismiss) {
        OutlinedTextField(
            value = text, onValueChange = { if (it.length <= 200) text = it }, singleLine = true, enabled = !busy,
            label = { Text(stringResource(R.string.teams_invite_input)) }, isError = text.isNotBlank() && code == null, modifier = Modifier.fillMaxWidth(),
        )
        if (text.isNotBlank() && code == null) Text(stringResource(R.string.teams_invite_invalid), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        Text(stringResource(R.string.teams_join_disclosure), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (error != null) Text(stringResource(teamsErrorRes(error)), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        Button(onClick = { code?.let(onSubmit) }, enabled = !busy && code != null) {
            if (busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.teams_join_confirm))
        }
    }
}

/**
 * New message (dm or group), new channel and message a Bot, with the rules of the Desktop `NewConversationDialog`.
 * [mode] is `newMessage`, `newChannel` or `newBot`. Everything is disabled offline.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun NewConversationSheet(
    mode: String, uid: String, members: List<TeamMember>, messaging: TeamMessagingState,
    onOpen: (String) -> Unit, onCreate: (kind: String, title: String, parties: List<String>, sharedBotIds: List<String>) -> Unit, onDismiss: () -> Unit,
) {
    val online = !messaging.offline
    val others = members.filter { it.uid != uid }
    val conversations = messaging.cache?.conversations.orEmpty()
    val usableBots = messaging.bots.filter { it.selectable }
    var group by rememberSaveable { mutableStateOf(false) }
    var peer by rememberSaveable { mutableStateOf("") }
    var people by rememberSaveable { mutableStateOf(setOf<String>()) }
    var sharedBotIds by rememberSaveable { mutableStateOf(setOf<String>()) }
    var name by rememberSaveable { mutableStateOf("") }
    val title = stringResource(when (mode) { "newChannel" -> R.string.teams_new_channel_title; "newBot" -> R.string.teams_new_bot_title; else -> R.string.teams_new_message_title })
    TeamsSheet(title, onDismiss) {
        if (!online) Text(stringResource(R.string.teams_new_offline), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        when (mode) {
            "newBot" -> NewBotList(uid, members, messaging, conversations, usableBots, online, onOpen) { key ->
                val bot = usableBots.first { it.sharedBotId == key }
                onCreate("bot_dm", bot.name, emptyList(), listOf(key))
            }
            else -> {
                val channel = mode == "newChannel"
                if (!channel) {
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                        SegmentedButton(selected = !group, onClick = { group = false }, shape = SegmentedButtonDefaults.itemShape(0, 2)) { Text(stringResource(R.string.teams_conversation_dm)) }
                        SegmentedButton(selected = group, onClick = { group = true }, shape = SegmentedButtonDefaults.itemShape(1, 2)) { Text(stringResource(R.string.teams_conversation_group)) }
                    }
                }
                if (channel) OutlinedTextField(value = name, onValueChange = { if (it.length <= 120) name = it }, singleLine = true, label = { Text(stringResource(R.string.teams_new_channel_name)) }, modifier = Modifier.fillMaxWidth())
                if (!channel && group) OutlinedTextField(value = name, onValueChange = { if (it.length <= 120) name = it }, singleLine = true, label = { Text(stringResource(R.string.teams_new_group_name_optional)) }, modifier = Modifier.fillMaxWidth())
                if (others.isEmpty()) Text(stringResource(R.string.teams_new_no_members), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(stringResource(if (!channel && !group) R.string.teams_new_to else R.string.teams_members), style = MaterialTheme.typography.labelLarge)
                others.forEach { member ->
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            if (!channel && !group) peer = member.uid else people = if (member.uid in people) people - member.uid else people + member.uid
                        },
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        if (!channel && !group) RadioButton(selected = peer == member.uid, onClick = { peer = member.uid })
                        else Checkbox(checked = member.uid in people, onCheckedChange = { people = if (it) people + member.uid else people - member.uid })
                        PersonAvatar(member.uid, member.displayName, 28.dp)
                        Text(member.displayName.ifEmpty { stringResource(R.string.teams_member) }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
                if (channel || group) {
                    if (usableBots.isNotEmpty()) {
                        Text(stringResource(R.string.teams_new_participating_bots), style = MaterialTheme.typography.labelLarge)
                        usableBots.forEach { bot ->
                            val chosen = bot.sharedBotId in sharedBotIds
                            Row(
                                Modifier.fillMaxWidth().clickable(enabled = chosen || sharedBotIds.size < TeamsComposeLogic.MAX_TARGETS) { sharedBotIds = if (chosen) sharedBotIds - bot.sharedBotId else sharedBotIds + bot.sharedBotId },
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Checkbox(checked = chosen, enabled = chosen || sharedBotIds.size < TeamsComposeLogic.MAX_TARGETS, onCheckedChange = { sharedBotIds = if (it) sharedBotIds + bot.sharedBotId else sharedBotIds - bot.sharedBotId })
                                BotAvatar(bot.avatarId, 28.dp)
                                Text(bot.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                    if (sharedBotIds.isNotEmpty()) Text(stringResource(R.string.teams_new_bot_owner_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                val chosenSharedBotIds = sharedBotIds.sorted()
                val valid = when {
                    channel -> name.isNotBlank() && people.size + sharedBotIds.size >= 1
                    group -> people.size + sharedBotIds.size >= 1 && (people.isNotEmpty() || name.isNotBlank())
                    else -> peer.isNotEmpty()
                }
                Button(
                    enabled = online && valid,
                    onClick = {
                        when {
                            channel -> onCreate("channel", name.trim(), people.sorted(), chosenSharedBotIds)
                            group -> onCreate("group", name.trim().ifEmpty { TeamsListLogic.defaultConversationTitle(people.sorted(), members, uid) }, people.sorted(), chosenSharedBotIds)
                            else -> {
                                val existing = TeamsListLogic.findDm(conversations, uid, peer)
                                if (existing != null) onOpen(existing.id) else onCreate("dm", TeamsListLogic.defaultConversationTitle(listOf(peer), members, uid), listOf(peer), emptyList())
                            }
                        }
                    },
                ) { Text(stringResource(when { channel -> R.string.teams_new_create_channel; group -> R.string.teams_new_create_group; else -> R.string.teams_new_open_dm })) }
            }
        }
    }
}

@Composable
private fun NewBotList(
    uid: String, members: List<TeamMember>, messaging: TeamMessagingState, conversations: List<TeamMessageConversation>, bots: List<TeamBotCard>,
    online: Boolean, onOpen: (String) -> Unit, onStart: (String) -> Unit,
) {
    if (bots.isEmpty()) {
        Text(
            stringResource(if (messaging.botsFailed) R.string.teams_bots_unverified else if (!messaging.botsLoaded) R.string.teams_bots_loading else R.string.teams_new_no_bots),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        return
    }
    Text(stringResource(R.string.teams_new_bots_pickable), style = MaterialTheme.typography.labelLarge)
    bots.forEach { bot ->
        val existing = TeamsListLogic.findBotDm(conversations, uid, bot.sharedBotId, bots)
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot.avatarId, 40.dp)
            Column(Modifier.weight(1f)) {
                Text(bot.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                val owner = members.firstOrNull { it.uid == bot.maintainer.uid }?.displayName ?: bot.maintainer.uid
                Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, owner)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (existing != null) {
                TextButton(enabled = online, onClick = { onOpen(existing.id) }) { Text(stringResource(R.string.teams_new_bot_open)) }
                OutlinedButton(enabled = online, onClick = { onStart(bot.sharedBotId) }) { Text(stringResource(R.string.remote_bot_new_chat)) }
            } else Button(enabled = online, onClick = { onStart(bot.sharedBotId) }) { Text(stringResource(R.string.teams_new_bot_start)) }
        }
        Spacer(Modifier.height(2.dp))
    }
}

/** Online / offline (requests are queued) / disabled, from the Bot card. */
@Composable
internal fun botAvailabilityText(bot: TeamBotCard): String = stringResource(
    when {
        bot.state != "published" -> R.string.teams_bot_disabled
        bot.ready -> R.string.remote_online
        else -> R.string.teams_bot_offline_queued
    },
)
