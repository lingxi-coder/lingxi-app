package com.lingxi.code.teams

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.tabs.TabLogic
import com.lingxi.code.theme.ConversationLead
import com.lingxi.code.theme.TeamTile

/** The Teams root (phone) or the left pane (wide): team switcher, search, new conversation, banners and the three groups. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun TeamsListScreen(
    uid: String,
    teams: TeamsState,
    messaging: TeamMessagingState,
    collapsed: Set<String>,
    unreadByTeam: Map<String, Int>,
    openConversationId: String,
    listState: LazyListState,
    onOpenConversation: (String) -> Unit,
    onSwitchTeam: () -> Unit,
    onNew: (String) -> Unit,
    onToggleSection: (String) -> Unit,
    onMarkRead: (TeamMessageConversation) -> Unit,
    onReconnect: () -> Unit,
    onDiscardCreation: () -> Unit,
    onMessageBot: (String) -> Unit = {},
    onArtifact: (String) -> Unit = {},
    onRefreshTask: (String) -> Unit = {},
    onCancelTask: (String) -> Unit = {},
    onDiscardCommand: (String) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val team = teams.teams.firstOrNull { it.id == teams.selectedId }
    val members = teams.detail?.members.orEmpty()
    val strings = rememberTeamsListStrings()
    val conversations = messaging.cache?.conversations.orEmpty()
    val bots = messaging.bots
    val now = rememberNowMs()
    var searching by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var newMenu by remember { mutableStateOf(false) }
    val labels = remember(conversations, members, bots, uid, strings) { conversations.associate { it.id to TeamsListLogic.conversationLabel(it, members, uid, bots, strings) } }
    val visible = remember(conversations, labels, query) { conversations.filter { TeamsListLogic.conversationMatches(labels[it.id].orEmpty(), query) } }
    val groups = remember(visible, uid) { TeamsListLogic.groupConversations(visible, uid) }
    val unreadOf = { c: TeamMessageConversation -> if (messaging.readsReady) TeamsListLogic.unreadCount(c) else 0 }
    val online = !messaging.offline
    val otherTeamsUnread = unreadByTeam.filterKeys { it != teams.selectedId }.values.sum()
    val invalid = messaging.error == "INVALID_DATA" || messaging.error == "INVALID_MESSAGE_SNAPSHOT"
    val listError = messaging.error.isNotEmpty() && !invalid && !messaging.revoked && messaging.error != "SERVICE_UNAVAILABLE"

    Column(modifier.fillMaxSize().testTag(TeamsTags.LIST)) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (searching) {
                val focus = remember { FocusRequester() }
                LaunchedEffect(Unit) { focus.requestFocus() }
                OutlinedTextField(
                    value = query, onValueChange = { query = it }, singleLine = true,
                    placeholder = { Text(stringResource(R.string.teams_search_placeholder)) },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { }),
                    modifier = Modifier.weight(1f).focusRequester(focus).testTag(TeamsTags.SEARCH_FIELD),
                )
                IconButton(onClick = { searching = false; query = "" }) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
            } else {
                Row(
                    Modifier.weight(1f, fill = false).clip(MaterialTheme.shapes.medium).clickable(onClick = onSwitchTeam).padding(horizontal = 8.dp, vertical = 6.dp).testTag(TeamsTags.TEAM_CHIP),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    if (team != null) TeamTile(team.id, team.name, 28.dp)
                    Text(team?.name ?: stringResource(R.string.teams_choose_team), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                    Icon(Icons.Outlined.KeyboardArrowDown, null, Modifier.size(20.dp))
                    if (otherTeamsUnread > 0) Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
                }
                Spacer(Modifier.weight(1f))
                IconButton(onClick = { searching = true }, modifier = Modifier.testTag(TeamsTags.SEARCH)) { Icon(Icons.Outlined.Search, stringResource(R.string.teams_search_placeholder)) }
                Box {
                    IconButton(onClick = { newMenu = true }, modifier = Modifier.testTag(TeamsTags.NEW)) { Icon(Icons.Outlined.Edit, stringResource(R.string.teams_conversation_new)) }
                    DropdownMenu(expanded = newMenu, onDismissRequest = { newMenu = false }) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_new_message_title)) }, onClick = { newMenu = false; onNew("newMessage") })
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_new_channel_title)) }, onClick = { newMenu = false; onNew("newChannel") })
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_new_bot_title)) }, onClick = { newMenu = false; onNew("newBot") })
                    }
                }
            }
        }
        if (messaging.directoryReady && messaging.offline && !invalid && !messaging.revoked) TeamsOfflineBanner(onReconnect)
        if (listError) TeamsErrorBanner(messaging.error, onRetry = onReconnect)

        Box(Modifier.weight(1f).fillMaxWidth()) {
            when {
                invalid -> TeamsMessageState(stringResource(R.string.teams_error_invalid_data), Modifier.testTag(TeamsTags.INVALID_DATA), action = { TextButton(onClick = onReconnect) { Text(stringResource(R.string.common_retry)) } })
                messaging.revoked -> TeamsMessageState(stringResource(R.string.teams_stale_error), action = { TextButton(onClick = onReconnect) { Text(stringResource(R.string.common_retry)) } })
                !messaging.directoryReady -> {
                    if (messaging.error.isNotEmpty()) TeamsMessageState(stringResource(teamsErrorRes(messaging.error)), action = { TextButton(onClick = onReconnect) { Text(stringResource(R.string.common_retry)) } })
                    else Box(Modifier.fillMaxSize().testTag(TeamsTags.LOADING), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(28.dp)) }
                }
                conversations.isEmpty() && messaging.cache?.creation == null && bots.isEmpty() && messaging.tasks.isEmpty() -> TeamsMessageState(
                    stringResource(R.string.teams_no_conversations), Modifier.testTag(TeamsTags.NO_CONVERSATIONS), body = stringResource(R.string.teams_no_conversations_hint),
                    action = { TextButton(onClick = { onNew("newMessage") }) { Text(stringResource(R.string.teams_conversation_new)) } },
                )
                visible.isEmpty() && query.isNotBlank() -> TeamsMessageState(stringResource(R.string.teams_no_matches), Modifier.testTag(TeamsTags.NO_MATCHES))
                else -> LazyColumn(Modifier.fillMaxSize(), state = listState) {
                    if (query.isBlank() && bots.any { it.selectable }) {
                        item(key = "assistants") { Text(stringResource(R.string.teams_assistants), Modifier.padding(16.dp), style = MaterialTheme.typography.titleSmall) }
                        items(bots.filter { it.selectable }, key = { "assistant-${it.sharedBotId}" }) { bot ->
                            TextButton(onClick = { onMessageBot(bot.sharedBotId) }, modifier = Modifier.fillMaxWidth().testTag("teams.assistant.${bot.sharedBotId}")) {
                                Text(bot.name + " · " + botAvailabilityText(bot))
                            }
                        }
                    }
                    if (query.isBlank() && messaging.tasks.isNotEmpty()) {
                        item(key = "tasks") { Text(stringResource(R.string.teams_recent_tasks), Modifier.padding(16.dp), style = MaterialTheme.typography.titleSmall) }
                        items(messaging.tasks, key = { "task-${it.taskId}" }) { task ->
                            val bot = bots.firstOrNull { it.sharedBotId == task.sharedBotId }
                            TeamTaskCard(
                                task, bot?.name ?: strings.bot, onArtifact, onRefreshTask, viewer = messaging.cache?.let { TeamMemberRef(uid, it.generation) }, maintainer = bot?.maintainer,
                                recipientName = task.wait?.let { wait -> members.firstOrNull { it.uid == wait.recipient.uid }?.displayName?.takeIf { it.isNotEmpty() } } ?: strings.member, onCancel = onCancelTask,
                            )
                        }
                    }
                    items(messaging.cache?.commands.orEmpty().filter { it.has("error") }, key = { "command-${it.getString("commandId")}" }) { command ->
                        Column(Modifier.padding(16.dp)) {
                            Text(stringResource(teamsErrorRes(command.getString("error"))), style = MaterialTheme.typography.bodySmall)
                            TextButton(onClick = { onDiscardCommand(command.getString("commandId")) }) { Text(stringResource(R.string.teams_message_discard)) }
                        }
                    }
                    messaging.cache?.creation?.let { creation ->
                        item(key = "creation") { PendingCreationRow(creation, now, onDiscardCreation) }
                    }
                    val sections = listOf(
                        Triple(TeamsWorkspace.SECTION_CHANNELS, R.string.teams_group_channels, groups.channels),
                        Triple(TeamsWorkspace.SECTION_DIRECT, R.string.teams_group_direct, groups.direct),
                        Triple(TeamsWorkspace.SECTION_BOTS, R.string.teams_group_bot_dm, groups.botDirect),
                    )
                    for ((id, title, rows) in sections) {
                        if (rows.isEmpty()) continue
                        val isCollapsed = id in collapsed
                        item(key = "section-$id") { SectionHeader(id, stringResource(title), isCollapsed, rows.sumOf { unreadOf(it) }, onClick = { onToggleSection(id) }) }
                        if (!isCollapsed) items(rows, key = { "row-${it.id}" }) { conversation ->
                            val unread = unreadOf(conversation)
                            val last = conversation.lastMessage
                            val preview = TeamsListLogic.conversationPreview(conversation, uid, strings) { message ->
                                if (message.authorKind == "shared_bot") bots.firstOrNull { it.sharedBotId == message.authorSharedBotId }?.name ?: strings.bot
                                else members.firstOrNull { it.uid == message.authorUid }?.displayName?.takeIf { it.isNotEmpty() } ?: strings.member
                            }
                            ConversationRow(
                                conversation, labels[conversation.id].orEmpty(), preview, listTimeText(last?.atMs, now), unread, selected = conversation.id == openConversationId,
                                canMarkRead = unread > 0 && online, members = members, bots = bots, uid = uid, strings = strings,
                                onClick = { onOpenConversation(conversation.id) }, onMarkRead = { onMarkRead(conversation) },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SectionHeader(id: String, title: String, collapsed: Boolean, unread: Int, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp).testTag(TeamsTags.section(id)),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(title, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
        if (collapsed && unread > 0) UnreadBadge(unread)
        Icon(
            Icons.Outlined.KeyboardArrowDown, stringResource(if (collapsed) R.string.chat_run_expand else R.string.chat_run_collapse),
            Modifier.size(20.dp).rotate(if (collapsed) -90f else 0f), tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ConversationRow(
    conversation: TeamMessageConversation, label: String, preview: String, time: String, unread: Int, selected: Boolean, canMarkRead: Boolean,
    members: List<TeamMember>, bots: List<TeamBotCard>, uid: String, strings: TeamsListStrings, onClick: () -> Unit, onMarkRead: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val markRead = stringResource(R.string.teams_mark_read)
    val weight = if (unread > 0) FontWeight.Bold else FontWeight.Normal
    Box {
        Row(
            Modifier.fillMaxWidth()
                .background(if (selected) MaterialTheme.colorScheme.surfaceVariant else Color.Transparent)
                .combinedClickable(onClick = onClick, onLongClick = if (canMarkRead) ({ menu = true }) else null)
                .semantics { if (canMarkRead) customActions = listOf(CustomAccessibilityAction(markRead) { onMarkRead(); true }) }
                .padding(horizontal = 16.dp, vertical = 10.dp).testTag(TeamsTags.row(conversation.id)),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            ConversationLead(conversation, members, bots, uid, strings, 40.dp, label)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, fontWeight = if (unread > 0) FontWeight.Bold else FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (time.isNotEmpty()) Text(time, Modifier.padding(start = 8.dp), style = MaterialTheme.typography.labelSmall, color = if (unread > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(preview, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, fontWeight = weight, color = if (unread > 0) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (unread > 0) { Spacer(Modifier.width(8.dp)); UnreadBadge(unread) }
                }
            }
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(text = { Text(markRead) }, onClick = { menu = false; onMarkRead() })
        }
    }
}

/** The unread pill; the number stops at `99+`. */
@Composable
internal fun UnreadBadge(count: Int, modifier: Modifier = Modifier) {
    val text = TabLogic.tabBadgeText(count) ?: return
    Box(
        modifier.defaultMinSize(minWidth = 20.dp, minHeight = 20.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).padding(horizontal = 6.dp),
        contentAlignment = Alignment.Center,
    ) { Text(text, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.SemiBold, maxLines = 1) }
}

/** A conversation being created: kept in the command journal and retried until the server answers. */
@Composable
private fun PendingCreationRow(creation: org.json.JSONObject, now: Long, onDiscard: () -> Unit) {
    val failed = creation.has("error")
    val attempted = creation.optBoolean("attempted")
    val status = when {
        failed -> stringResource(teamsErrorRes(creation.getString("error")))
        attempted -> stringResource(R.string.teams_message_confirming)
        else -> stringResource(R.string.teams_message_pending)
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text((creation.optJSONObject("payload") ?: creation).optString("title"), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(status, style = MaterialTheme.typography.bodySmall, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
        TextButton(onClick = onDiscard, enabled = !(attempted && !failed) && creation.optLong("leaseUntil") <= now) { Text(stringResource(R.string.teams_message_discard)) }
        HorizontalDivider()
    }
}
