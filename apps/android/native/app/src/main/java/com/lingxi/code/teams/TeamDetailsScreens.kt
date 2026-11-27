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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PersonAdd
import androidx.compose.material.icons.outlined.Remove
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
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
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
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.friends.FriendsLogic
import com.lingxi.code.theme.ConfirmDialog
import com.lingxi.code.theme.ConversationLead
import com.lingxi.code.theme.IdentityAvatar
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.TeamTile
import java.time.ZoneId
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
    friendUids: Set<String>, onAddFriend: (TeamMember) -> Unit, onInviteFriends: () -> Unit, onClose: () -> Unit, onMessageBot: (String) -> Unit, modifier: Modifier = Modifier, onBotExample: (String, String) -> Unit = { _, _ -> },
    /** Opens a Chat of this Team: where "Replies waiting" takes the person to the Chat a reply was made in. */
    onOpenConversation: (String) -> Unit = {},
) {
    val detail = teams.detail?.takeIf { it.team.id == team.id }
    val current = detail?.team ?: team
    val privileged = current.role != "member"
    var segment by rememberSaveable { mutableIntStateOf(0) }
    var confirmation by remember { mutableStateOf<TeamConfirmation?>(null) }
    // The shared list of one Bot of this team; a change of account, team or membership closes it.
    var listOf by remember(uid, team.id, current.generation) { mutableStateOf<TeamBotCard?>(null) }
    listOf?.let { bot -> TeamSharedListDialog(bot, workspace.messagingModel, detail?.members.orEmpty()) { listOf = null } }
    // The auto-reply log of one Bot of this team, and the Bots the service said this person does not maintain after all: their entry is not offered again.
    // A change of account, team or membership closes it.
    var triageOf by remember(uid, team.id, current.generation) { mutableStateOf<TeamBotCard?>(null) }
    var triageDenied by remember(uid, team.id, current.generation) { mutableStateOf(emptySet<String>()) }
    triageOf?.let { bot -> TeamTriageLogDialog(bot, null, workspace.messagingModel, onDenied = { triageDenied = triageDenied + it }) { triageOf = null } }
    val model = workspace.teamsModel
    val busy = teams.busy

    Column(modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) {
                if (showBack) Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) else Icon(Icons.Outlined.Close, stringResource(R.string.common_close))
            }
            Text(stringResource(R.string.teams_team_details), Modifier.semantics { heading() }, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        }
        if (busy || teams.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
        teams.error?.let { TeamsErrorBanner(it) }
        DetailIdentity(
            current.name,
            "${teamRoleLabel(current.role)} · ${stringResource(if (current.memberCount == 1) R.string.teams_members_count_one_fmt else R.string.teams_members_count_other_fmt, current.memberCount)}",
            Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        ) { TeamTile(current.id, current.name, 56.dp) }
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
                R.string.teams_invites -> invitesSection(detail, current, teams, busy, model, onInviteFriends) { confirmation = it }
                else -> botsSection(messaging, detail, uid, onMessageBot, workspace, onBotExample, triageDenied, { triageOf = it }, onOpenConversation) { listOf = it }
            }
            item {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                TextButton(
                    enabled = !busy,
                    onClick = { confirmation = if (current.role == "owner") TeamConfirmations.delete(current) else TeamConfirmations.leave(current) },
                ) { Text(stringResource(if (current.role == "owner") R.string.teams_delete else R.string.teams_leave), color = MaterialTheme.colorScheme.error) }
            }
        }
    }
    confirmation?.let { confirm ->
        ConfirmDialog(
            title = if (confirm.subject != null) stringResource(confirm.title, confirm.subject) else stringResource(confirm.title),
            body = stringResource(confirm.body), confirm = stringResource(confirm.button), destructive = confirm.destructive, enabled = !busy,
            onConfirm = {
                model.execute(confirm.command) { if (confirm.closesDetails) onClose() }
                confirmation = null
            },
            onDismiss = { confirmation = null },
        )
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.membersSection(
    detail: TeamDetail?, team: TeamSummary, uid: String, busy: Boolean, friendUids: Set<String>, onAddFriend: (TeamMember) -> Unit, confirm: (TeamConfirmation) -> Unit,
) {
    items(detail?.members.orEmpty(), key = { "member-${it.uid}" }) { member ->
        val name = member.displayName.ifEmpty { stringResource(R.string.teams_member) }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            PersonAvatar(member.uid, member.displayName, 40.dp)
            Column(Modifier.weight(1f)) {
                Text(name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                // A role is a label, not a (disabled) button: the old assist chip read as "Admin, button, dimmed".
                Text(teamRoleLabel(member.role), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // Add friend (docs/FRIENDS-CLIENTS.md D15): every other person; Bots are not members here.
                if (member.uid != uid) {
                    if (member.uid in friendUids) Text(stringResource(R.string.friends_relation_friends), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    else TextButton(onClick = { onAddFriend(member) }, modifier = Modifier.testTag(TeamsTags.addFriend(member.uid))) { Text(stringResource(R.string.friends_add)) }
                }
            }
            if (member.uid != uid && member.role != "owner" && (team.role == "owner" || (team.role == "admin" && member.role == "member"))) {
                // The owner has three things to decide about a member; they are behind one button so the name keeps its room. Each one asks first (U8).
                if (team.role == "owner") Box {
                    var menu by remember(member.uid) { mutableStateOf(false) }
                    IconButton(enabled = !busy, onClick = { menu = true }, modifier = Modifier.testTag(TeamsTags.memberActions(member.uid))) {
                        Icon(Icons.Outlined.MoreVert, stringResource(R.string.teams_list_item_options_fmt, name))
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text(stringResource(if (member.role == "admin") R.string.teams_demote else R.string.teams_promote)) },
                            onClick = { menu = false; confirm(TeamConfirmations.changeRole(team, member, name)) })
                        // Owning the team is given on its own: Bots stay with their maintainers and chats with their readers.
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_make_owner)) }, onClick = { menu = false; confirm(TeamConfirmations.makeOwner(team, member, name)) })
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_remove), color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; confirm(TeamConfirmations.remove(team, member, name)) })
                    }
                } else TextButton(enabled = !busy, onClick = { confirm(TeamConfirmations.remove(team, member, name)) }) {
                    Text(stringResource(R.string.teams_remove), color = MaterialTheme.colorScheme.error)
                }
            }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.invitesSection(
    detail: TeamDetail?, team: TeamSummary, teams: TeamsState, busy: Boolean, model: TeamsViewModel, onInviteFriends: () -> Unit, confirm: (TeamConfirmation) -> Unit,
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
        val copyText = rememberCopyToClipboard(stringResource(R.string.teams_clip_invite))
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
                IconButton(enabled = !busy && uses > 1, onClick = { uses-- }) { Icon(Icons.Outlined.Remove, stringResource(R.string.teams_uses_decrease)) }
                // The count is a polite live region, so a screen reader says the new number after each press; it widens with the font scale.
                Text(uses.toString(), Modifier.widthIn(min = 32.dp).semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
                IconButton(enabled = !busy && uses < 50, onClick = { uses++ }) { Icon(Icons.Outlined.Add, stringResource(R.string.teams_uses_increase)) }
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
                    if (copied) Text(stringResource(R.string.teams_link_copied), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
                }
            }
        }
    }
    val invites = detail?.invites.orEmpty()
    if (invites.isEmpty()) item { Text(stringResource(R.string.teams_invite_none), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    items(invites, key = { "invite-${it.id}" }) { invite ->
        val locale = LocalConfiguration.current.locales[0]
        val zone = remember { ZoneId.systemDefault() }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(teamRoleLabel(invite.role), style = MaterialTheme.typography.bodyLarge)
                // "uses/limit · expiry": the expiry as the device formats it, or as sent when it is not a date.
                val expiry = TeamsStreamLogic.parseInstantMs(invite.expiresAt)?.let { formatDate(it, zone, locale, "yMMMdjm") } ?: invite.expiresAt
                Text("${invite.uses}/${invite.maxUses} · $expiry", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (invite.revoked) Text(stringResource(R.string.teams_revoked), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            else TextButton(enabled = !busy && (team.role == "owner" || invite.role == "member"), onClick = { confirm(TeamConfirmations.revokeInvite(team, invite)) }) { Text(stringResource(R.string.teams_revoke)) }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.botsSection(
    messaging: TeamMessagingState, detail: TeamDetail?, uid: String, onMessageBot: (String) -> Unit, workspace: TeamsWorkspace, onBotExample: (String, String) -> Unit,
    triageDenied: Set<String>, onOpenTriage: (TeamBotCard) -> Unit, onOpenChat: (String) -> Unit, onOpenList: (TeamBotCard) -> Unit,
) {
    item { Text(stringResource(R.string.teams_bots_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    when {
        // Loading or unverified: one line of status. Loaded and empty (U9): what is empty, then how a Bot gets here.
        messaging.bots.isEmpty() -> item {
            if (messaging.botsFailed || !messaging.botsLoaded) {
                Text(
                    stringResource(if (messaging.botsFailed) R.string.teams_bots_unverified else R.string.teams_bots_loading),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                TeamsMessageState(stringResource(R.string.teams_bots_none), body = stringResource(R.string.teams_bots_share_hint), fill = false)
            }
        }
        else -> {
            item { Text(stringResource(R.string.teams_bots_share_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            items(messaging.bots, key = { "bot-${it.sharedBotId}" }) { bot ->
                TeamBotCardView(
                    bot, messaging.bots, detail?.members?.firstOrNull { it.uid == bot.maintainer.uid }?.displayName ?: bot.maintainer.uid, uid, messaging.cache?.generation ?: 0, !messaging.offline,
                    onMessage = { onMessageBot(bot.sharedBotId) }, onStopSharing = { workspace.messagingModel.pause(bot) },
                    onExample = { onBotExample(bot.sharedBotId, it) },
                    onList = if (TeamSharedListRules.canOpen(bot, uid, messaging.cache?.generation ?: 0)) ({ onOpenList(bot) }) else null,
                    // The maintainer's log of what the Bot decided about the messages nobody asked it about: only for a Bot that answers on its own.
                    onTriageLog = if (TeamTriageRules.maintains(bot, TeamMemberRef(uid, messaging.cache?.generation ?: 0)) && bot.autoReply != null && bot.sharedBotId !in triageDenied) ({ onOpenTriage(bot) }) else null,
                    // The replies of the Bot that wait for their maintainer to let them go, across every Chat of this Team.
                    heldModel = if (TeamTriageRules.maintains(bot, TeamMemberRef(uid, messaging.cache?.generation ?: 0))) workspace.messagingModel else null,
                    heldSignals = messaging.botSignals, onHeldChat = onOpenChat,
                )
            }
        }
    }
}

/** Conversation details (SPEC section 6.4): everyone in the conversation, who owns each Bot, the Bot DM note and Mark read. */
@Composable
internal fun ConversationDetailsSheet(
    conversation: TeamMessageConversation, label: String, members: List<TeamMember>, bots: List<TeamBotCard>, uid: String, strings: TeamsListStrings,
    unread: Int, online: Boolean, onMarkRead: () -> Unit, onDismiss: () -> Unit,
    /**
     * Sends the Bots of this Chat that answer on their own, with the setting that changed beside them: how many answer one message and whether a message that mentions
     * only people is judged (`judge` or `quiet`). What is null is left as it is.
     */
    onAutoReply: (targets: List<String>, maxReplies: Int?, humanMentions: String?) -> Unit = { _, _, _ -> }, busy: Boolean = false,
    /** Opens what the Bots of this Chat may use here; null where the Chat has no such control (a friend chat). */
    onBotScope: (() -> Unit)? = null,
    /** The Bots of this Chat whose auto-reply log this person reads here (the ones they maintain that answer on their own in it), and what opens one of them; null where the Chat has none. */
    triageBots: List<TeamBotCard> = emptyList(), onTriageLog: ((TeamBotCard) -> Unit)? = null,
) {
    TeamsSheet(stringResource(R.string.teams_conversation_details), onDismiss) {
        DetailIdentity(label, conversationSubtitle(conversation, bots)) {
            ConversationLead(conversation, members, bots, uid, strings, 56.dp, label)
        }
        if (unread > 0 && online) TextButton(onClick = { onMarkRead(); onDismiss() }) { Text(stringResource(R.string.teams_mark_read)) }
        val participants = TeamsListLogic.conversationParticipants(conversation, members, bots, uid, strings)
        if (participants.isNotEmpty()) Text(
            stringResource(R.string.teams_members), Modifier.semantics { heading() }, style = MaterialTheme.typography.titleSmall,
        )
        participants.forEach { person ->
            Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) { }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IdentityAvatar(person, 40.dp)
                Column(Modifier.weight(1f)) {
                    Text(person.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (person.isBot) {
                        val card = bots.firstOrNull { it.avatarId == person.id || it.sharedBotId == person.id }
                        if (card != null) {
                            val owner = members.firstOrNull { it.uid == card.maintainer.uid }?.displayName ?: card.maintainer.uid
                            Text(stringResource(R.string.teams_bot_owner_fmt, owner), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            // What this Chat leaves of what the Bot's maintainer shared.
                            TeamBotScopeSummary(card, conversation)
                        }
                    }
                }
            }
        }
        if (conversation.kind == "bot_dm") Text(stringResource(R.string.teams_composer_note_bot_dm), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        val auto = TeamAutoReply.bots(conversation, bots, uid)
        if (auto.bots.isNotEmpty()) {
            Text(stringResource(R.string.teams_auto_reply_title), style = MaterialTheme.typography.titleSmall)
            Text(stringResource(R.string.teams_auto_reply_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            auto.bots.forEach { card ->
                val on = card.sharedBotId in auto.on
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(card.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Switch(checked = on, enabled = online && !busy && (on || TeamAutoReply.available(card)), onCheckedChange = { onAutoReply(TeamAutoReply.targets(auto, card.sharedBotId, it), null, null) },
                        modifier = Modifier.testTag("teams.conversation.details.autoreply.${card.sharedBotId}"))
                }
                // Said before it is switched on: where this Chat's messages go.
                val limit = card.autoReply
                val owner = if (card.maintainer.uid == uid) strings.you else members.firstOrNull { it.uid == card.maintainer.uid }?.displayName ?: card.maintainer.uid
                Text(if (limit != null && TeamAutoReply.available(card)) stringResource(R.string.teams_auto_reply_disclosure_fmt, owner, card.name, limit.daily)
                    else stringResource(R.string.teams_auto_reply_unavailable_fmt, card.name), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            // A group or channel also says how many of its Bots answer one message and whether a message to people alone is judged; a Bot DM has the one Bot, always the one asked.
            if (TeamAutoReply.hasRounds(conversation)) {
                val editable = online && !busy
                val most = TeamAutoReply.maxReplies(conversation)
                Text(stringResource(R.string.teams_auto_reply_max_replies), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodyLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    (1..TeamExecutionContract.MAX_REPLIES_LIMIT).forEach { count ->
                        FilterChip(selected = count == most, enabled = editable, onClick = { if (count != most) onAutoReply(TeamAutoReply.targets(auto), count, null) },
                            label = { Text(count.toString()) }, modifier = Modifier.testTag("teams.conversation.details.maxreplies.$count"))
                    }
                }
                Text(stringResource(R.string.teams_auto_reply_rank_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.teams_auto_reply_human_mentions), Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                    Switch(checked = TeamAutoReply.judgesHumanMentions(conversation), enabled = editable,
                        onCheckedChange = { onAutoReply(TeamAutoReply.targets(auto), null, if (it) "judge" else "quiet") },
                        modifier = Modifier.testTag("teams.conversation.details.humanmentions"))
                }
                Text(stringResource(R.string.teams_auto_reply_human_mentions_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        // Next to the auto-reply control: what the Bots this person maintains decided about this Chat's messages, and why. A maintainer need not be in the Chat for it.
        if (onTriageLog != null) triageBots.forEach { card ->
            val named = triageBots.size > 1
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (named) Text(card.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                OutlinedButton(onClick = { onTriageLog(card) }, modifier = (if (named) Modifier else Modifier.weight(1f)).testTag("teams.conversation.details.triage.${card.sharedBotId}")) {
                    Text(stringResource(R.string.teams_triage_title))
                }
            }
        }
        // Next to the auto-reply control: what the Bots of this Chat may use of their maintainers' computers, here. Only for a Bot that shares something.
        if (onBotScope != null && TeamBotScopeRules.limitable(conversation, bots, uid).isNotEmpty()) {
            OutlinedButton(onClick = onBotScope, Modifier.fillMaxWidth().testTag("teams.conversation.details.botscope")) { Text(stringResource(R.string.teams_bot_scope_title)) }
        }

        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun DetailIdentity(label: String, subtitle: String?, modifier: Modifier = Modifier, avatar: @Composable () -> Unit) {
    Surface(modifier.fillMaxWidth(), color = MaterialTheme.colorScheme.surfaceContainer, shape = MaterialTheme.shapes.large) {
        Row(
            Modifier.padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            avatar()
            Column(Modifier.weight(1f)) {
                Text(label, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}
