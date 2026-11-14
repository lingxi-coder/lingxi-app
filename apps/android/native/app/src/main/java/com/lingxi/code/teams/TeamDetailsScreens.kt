package com.lingxi.code.teams

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PersonAdd
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.friends.FriendsLogic
import com.lingxi.code.theme.IdentityAvatar
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.TeamTile
import java.util.UUID

/**
 * Team details (SPEC-mobile-team-bot-ui section 6.4): Members | Invites | Bots, and the danger zone. A pushed page on phones,
 * the trailing pane when wide. Commands are the existing HTTP actions (`create`, `role`, `remove`, `invite`, `revokeInvite`,
 * `leave`, `delete`) with the rules of the old Teams page; Bot binding and permissions stay on the Desktop.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TeamDetailsScreen(
    workspace: TeamsWorkspace, uid: String, team: TeamSummary, teams: TeamsState, messaging: TeamMessagingState, showBack: Boolean,
    friendUids: Set<String>, onAddFriend: (TeamMember) -> Unit, onInviteFriends: () -> Unit, onClose: () -> Unit, onMessageBot: (String) -> Unit, modifier: Modifier = Modifier,
) {
    val detail = teams.detail?.takeIf { it.team.id == team.id }
    val current = detail?.team ?: team
    val privileged = current.role != "member"
    var segment by rememberSaveable { mutableIntStateOf(0) }
    var confirmation by remember { mutableStateOf<Pair<Int, Map<String, Any>>?>(null) }
    // The shared list of one Bot of this team; a change of account, team or membership closes it.
    var listOf by remember(uid, team.id, current.generation) { mutableStateOf<TeamBotCard?>(null) }
    listOf?.let { bot -> TeamSharedListDialog(bot, workspace.messagingModel, detail?.members.orEmpty()) { listOf = null } }
    val model = workspace.teamsModel
    val busy = teams.busy

    Column(modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) {
                if (showBack) Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) else Icon(Icons.Outlined.Close, stringResource(R.string.common_close))
            }
            Text(stringResource(R.string.teams_team_details), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        }
        if (busy || teams.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        teams.error?.let { TeamsErrorBanner(it) }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            TeamTile(current.id, current.name, 44.dp)
            Column(Modifier.weight(1f)) {
                Text(current.name, style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text("${teamRoleLabel(current.role)} · ${stringResource(R.string.teams_members_count_fmt, current.memberCount)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        val segments = buildList { add(R.string.teams_members); if (privileged) add(R.string.teams_invites); add(R.string.teams_bots_title) }
        val selected = segment.coerceIn(0, segments.lastIndex)
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
            segments.forEachIndexed { index, label ->
                SegmentedButton(selected = selected == index, onClick = { segment = index }, shape = SegmentedButtonDefaults.itemShape(index, segments.size)) { Text(stringResource(label)) }
            }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when (segments[selected]) {
                R.string.teams_members -> membersSection(detail, current, uid, busy, friendUids, onAddFriend) { confirmation = it }
                R.string.teams_invites -> invitesSection(detail, current, teams, busy, model, onInviteFriends)
                else -> botsSection(messaging, detail, uid, onMessageBot, workspace) { listOf = it }
            }
            item {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                TextButton(
                    enabled = !busy,
                    onClick = {
                        confirmation = if (current.role == "owner") R.string.teams_confirm_delete to mapOf("action" to "delete", "teamId" to current.id)
                        else R.string.teams_confirm_leave to mapOf("action" to "leave", "teamId" to current.id, "generation" to current.generation)
                    },
                ) { Text(stringResource(if (current.role == "owner") R.string.teams_delete else R.string.teams_leave), color = MaterialTheme.colorScheme.error) }
            }
        }
    }
    confirmation?.let { (title, command) ->
        AlertDialog(
            onDismissRequest = { confirmation = null }, title = { Text(stringResource(title)) },
            confirmButton = {
                TextButton(enabled = !busy, onClick = {
                    val leaving = command["action"] in setOf("leave", "delete")
                    model.execute(command) { if (leaving) onClose() }
                    confirmation = null
                }) { Text(stringResource(R.string.teams_confirm)) }
            },
            dismissButton = { TextButton(onClick = { confirmation = null }) { Text(stringResource(R.string.teams_cancel)) } },
        )
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.membersSection(
    detail: TeamDetail?, team: TeamSummary, uid: String, busy: Boolean, friendUids: Set<String>, onAddFriend: (TeamMember) -> Unit, confirm: (Pair<Int, Map<String, Any>>) -> Unit,
) {
    items(detail?.members.orEmpty(), key = { "member-${it.uid}" }) { member ->
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            PersonAvatar(member.uid, member.displayName, 40.dp)
            Column(Modifier.weight(1f)) {
                Text(member.displayName.ifEmpty { stringResource(R.string.teams_member) }, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                AssistChip(onClick = {}, enabled = false, label = { Text(teamRoleLabel(member.role)) })
                // Add friend (docs/FRIENDS-CLIENTS.md D15): every other person; Bots are not members here.
                if (member.uid != uid) {
                    if (member.uid in friendUids) Text(stringResource(R.string.friends_relation_friends), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    else TextButton(onClick = { onAddFriend(member) }, modifier = Modifier.testTag(TeamsTags.addFriend(member.uid))) { Text(stringResource(R.string.friends_add)) }
                }
            }
            if (member.uid != uid && member.role != "owner" && (team.role == "owner" || (team.role == "admin" && member.role == "member"))) {
                val target: Map<String, Any> = mapOf("teamId" to team.id, "uid" to member.uid, "generation" to member.generation)
                // The owner has three things to decide about a member; they are behind one button so the name keeps its room.
                if (team.role == "owner") Box {
                    var menu by remember(member.uid) { mutableStateOf(false) }
                    IconButton(enabled = !busy, onClick = { menu = true }, modifier = Modifier.testTag(TeamsTags.memberActions(member.uid))) { Icon(Icons.Outlined.MoreVert, member.displayName) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text(stringResource(if (member.role == "admin") R.string.teams_demote else R.string.teams_promote)) },
                            onClick = { menu = false; confirm(R.string.teams_confirm_change to target + mapOf("action" to "role", "role" to if (member.role == "admin") "member" else "admin")) })
                        // Owning the team is given on its own: Bots stay with their maintainers and chats with their readers.
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_make_owner)) }, onClick = { menu = false; confirm(R.string.teams_confirm_make_owner to target + mapOf("action" to "handOver")) })
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_remove), color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; confirm(R.string.teams_confirm_change to target + mapOf("action" to "remove")) })
                    }
                } else TextButton(enabled = !busy, onClick = { confirm(R.string.teams_confirm_change to target + mapOf("action" to "remove")) }) {
                    Text(stringResource(R.string.teams_remove), color = MaterialTheme.colorScheme.error)
                }
            }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.invitesSection(
    detail: TeamDetail?, team: TeamSummary, teams: TeamsState, busy: Boolean, model: TeamsViewModel, onInviteFriends: () -> Unit,
) {
    // Invite friends (docs/FRIENDS-CLIENTS.md D26): an owner or an admin, whatever the member count (a full team still shows it and the server answers
    // MEMBER_LIMIT); the picker lists the friends who are not in the member list, so it waits for that list.
    if (FriendsLogic.canInviteFriends(team.role)) item(key = "invite-friends") {
        OutlinedButton(enabled = detail != null, onClick = onInviteFriends, modifier = Modifier.fillMaxWidth().testTag(TeamsTags.INVITE_FRIENDS)) {
            Icon(Icons.Outlined.PersonAdd, null, Modifier.size(18.dp))
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.friends_invite_action))
        }
    }
    item {
        var admin by rememberSaveable { mutableStateOf(false) }
        var uses by rememberSaveable { mutableIntStateOf(1) }
        var ttl by rememberSaveable { mutableIntStateOf(86_400) }
        var copied by remember(teams.invitationLink) { mutableStateOf(false) }
        val copyText = rememberCopyToClipboard("LingXi team invitation")
        val context = LocalContext.current
        val shareTitle = stringResource(R.string.teams_invite_share)
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (team.role == "owner") {
                Text(stringResource(R.string.teams_invite_role), style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip(selected = !admin, onClick = { admin = false }, label = { Text(stringResource(R.string.teams_member)) })
                    FilterChip(selected = admin, onClick = { admin = true }, label = { Text(stringResource(R.string.teams_admin)) })
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.teams_uses), Modifier.weight(1f), style = MaterialTheme.typography.labelLarge)
                TextButton(enabled = !busy && uses > 1, onClick = { uses-- }) { Text("−") }
                Text(uses.toString(), Modifier.width(32.dp), style = MaterialTheme.typography.bodyLarge)
                TextButton(enabled = !busy && uses < 50, onClick = { uses++ }) { Text("+") }
            }
            Text(stringResource(R.string.teams_expiry), style = MaterialTheme.typography.labelLarge)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(3_600 to R.string.teams_hour, 86_400 to R.string.teams_day, 604_800 to R.string.teams_week).forEach { (value, label) ->
                    FilterChip(selected = ttl == value, enabled = !busy, onClick = { ttl = value }, label = { Text(stringResource(label)) })
                }
            }
            Button(enabled = !busy, onClick = {
                model.execute(mapOf("action" to "invite", "teamId" to team.id, "requestId" to UUID.randomUUID().toString(), "role" to if (team.role == "owner" && admin) "admin" else "member", "maxUses" to uses, "ttlSeconds" to ttl))
            }) { Text(stringResource(R.string.teams_invite_create)) }
            if (teams.invitationLink.isNotEmpty()) {
                Text(teams.invitationLink, style = MaterialTheme.typography.bodySmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedButton(onClick = { copyText(teams.invitationLink); copied = true }) { Text(stringResource(R.string.teams_copy)) }
                    OutlinedButton(onClick = {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, teams.invitationLink)
                        runCatching { context.startActivity(Intent.createChooser(send, shareTitle)) }
                    }) { Text(shareTitle) }
                    if (copied) Text(stringResource(R.string.teams_link_copied), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
                }
            }
        }
    }
    val invites = detail?.invites.orEmpty()
    if (invites.isEmpty()) item { Text(stringResource(R.string.teams_invite_none), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    items(invites, key = { "invite-${it.id}" }) { invite ->
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(teamRoleLabel(invite.role), style = MaterialTheme.typography.bodyLarge)
                Text("${invite.uses}/${invite.maxUses} · ${invite.expiresAt}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (invite.revoked) Text(stringResource(R.string.teams_revoked), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            else TextButton(enabled = !busy && (team.role == "owner" || invite.role == "member"), onClick = { model.execute(mapOf("action" to "revokeInvite", "teamId" to team.id, "inviteId" to invite.id)) }) { Text(stringResource(R.string.teams_revoke)) }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.botsSection(messaging: TeamMessagingState, detail: TeamDetail?, uid: String, onMessageBot: (String) -> Unit, workspace: TeamsWorkspace, onOpenList: (TeamBotCard) -> Unit) {
    item {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(stringResource(R.string.teams_bots_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(stringResource(R.string.teams_bots_share_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    when {
        messaging.bots.isEmpty() -> item {
            Text(
                stringResource(if (messaging.botsFailed) R.string.teams_bots_unverified else if (!messaging.botsLoaded) R.string.teams_bots_loading else R.string.teams_bots_none),
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        else -> items(messaging.bots, key = { "bot-${it.sharedBotId}" }) { bot ->
            TeamBotCardView(
                bot, detail?.members?.firstOrNull { it.uid == bot.maintainer.uid }?.displayName ?: bot.maintainer.uid, uid, messaging.cache?.generation ?: 0, !messaging.offline,
                onMessage = { onMessageBot(bot.sharedBotId) }, onPause = { workspace.messagingModel.pause(bot) },
                onList = if (TeamSharedListRules.canOpen(bot, uid, messaging.cache?.generation ?: 0)) ({ onOpenList(bot) }) else null,
            )
        }
    }
}

/** Conversation details (SPEC section 6.4): everyone in the conversation, who owns each Bot, the Bot DM note and Mark read. */
@Composable
internal fun ConversationDetailsSheet(
    conversation: TeamMessageConversation, label: String, members: List<TeamMember>, bots: List<TeamBotCard>, uid: String, strings: TeamsListStrings,
    unread: Int, online: Boolean, onMarkRead: () -> Unit, onDismiss: () -> Unit,
) {
    TeamsSheet(label, onDismiss) {
        TeamsListLogic.conversationParticipants(conversation, members, bots, uid, strings).forEach { person ->
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IdentityAvatar(person, 40.dp)
                Column(Modifier.weight(1f)) {
                    Text(person.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (person.isBot) {
                        val card = bots.firstOrNull { it.avatarId == person.id || it.sharedBotId == person.id }
                        if (card != null) {
                            val owner = members.firstOrNull { it.uid == card.maintainer.uid }?.displayName ?: card.maintainer.uid
                            Text(stringResource(R.string.teams_bot_owner_fmt, owner), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            }
        }
        if (conversation.kind == "bot_dm") Text(stringResource(R.string.teams_composer_note_bot_dm), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (unread > 0 && online) TextButton(onClick = { onMarkRead(); onDismiss() }) { Text(stringResource(R.string.teams_mark_read)) }
        Spacer(Modifier.height(8.dp))
    }
}
