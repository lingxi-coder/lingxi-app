package com.lingxi.code.teams

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.ArrowDownward
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.lingxi.code.R
import com.lingxi.code.conversation.AIText
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.ConversationLead
import com.lingxi.code.theme.PersonAvatar
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.time.ZoneId

/** One entry of the stream list; the keys are stable across updates. */
private sealed interface StreamItem {
    val key: String
    data object Notice : StreamItem { override val key = "notice" }
    data object Earlier : StreamItem { override val key = "earlier" }
    data class Day(val row: StreamRow.Date) : StreamItem { override val key get() = row.key }
    data object NewMessages : StreamItem { override val key = "unread" }
    data class Msg(val message: TeamMessage, val grouped: Boolean) : StreamItem { override val key get() = "message:${message.id}" }
    data class Local(val intent: JSONObject, val kind: String) : StreamItem {
        override val key get() = "$kind:" + (intent.optJSONObject("payload") ?: intent).getString("changeId")
    }
}

/**
 * The Friends variant of the conversation header (docs/FRIENDS-CLIENTS.md D9): the friend stands in for the conversation's lead and
 * title, and [menu] for the details and mark-read actions, which a pair team's single conversation does not have.
 */
internal class FriendChatHeader(val uid: String, val name: String, val handle: String?, val menu: @Composable () -> Unit)

/**
 * The conversation screen (SPEC-mobile-team-bot-ui section 6.3): header, banners, the grouped stream with date separators and
 * the new-messages divider, run and question cards, local pending commands, scroll rules, the pill, automatic mark-read (D12)
 * and the composer. Pushed above the list on phones, the centre pane when wide. A [friend] turns it into a friend's chat.
 */
@Composable
internal fun TeamConversationScreen(
    workspace: TeamsWorkspace, uid: String, team: TeamSummary, teams: TeamsState, messaging: TeamMessagingState,
    conversationId: String, showBack: Boolean, onBack: () -> Unit, modifier: Modifier = Modifier, friend: FriendChatHeader? = null,
) {
    DisposableEffect(conversationId) {
        workspace.openConversation(conversationId)
        onDispose { if (workspace.messagingModel.state.value.selected == conversationId) workspace.openConversation("") }
    }
    val cache = messaging.cache
    val conversation = cache?.conversations?.firstOrNull { it.id == conversationId }
    val creation = cache?.creation?.takeIf { (it.optJSONObject("payload") ?: it).optString("conversationId") == conversationId }
    val members = teams.detail?.members.orEmpty()
    val strings = rememberTeamsListStrings()
    val bots = messaging.bots
    val label = friend?.name ?: conversation?.let { TeamsListLogic.conversationLabel(it, members, uid, bots, strings) }.orEmpty()
    var showDetails by rememberSaveable(conversationId) { mutableStateOf(false) }
    var recalling by remember(conversationId) { mutableStateOf<TeamMessage?>(null) }
    val online = !messaging.offline

    // A conversation that is neither listed nor being created is gone (left, removed, deleted): leave once the directory is sure.
    LaunchedEffect(conversation == null, creation == null, messaging.directoryReady) {
        if (conversation == null && creation == null && messaging.directoryReady) { delay(5_000); onBack() }
    }

    Column(modifier.fillMaxSize().testTag(TeamsTags.CONVERSATION)) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            if (showBack) IconButton(onClick = onBack, modifier = Modifier.testTag(TeamsTags.BACK)) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) }
            else Spacer(Modifier.width(12.dp))
            if (friend != null) {
                PersonAvatar(friend.uid, friend.name, 32.dp)
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(label, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    friend.handle?.let { Text("@$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                friend.menu()
            } else if (conversation != null) {
                ConversationLead(conversation, members, bots, uid, strings, 32.dp, label)
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(label, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val subtitle = conversationSubtitle(conversation, bots)
                    if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                var menu by remember { mutableStateOf(false) }
                IconButton(onClick = { showDetails = true }, modifier = Modifier.testTag(TeamsTags.INFO)) { Icon(Icons.Outlined.Info, stringResource(R.string.teams_conversations)) }
                val unread = if (messaging.readsReady) TeamsListLogic.unreadCount(conversation) else 0
                if (unread > 0 && online) Box {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.teams_message_actions)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.teams_mark_read)) }, onClick = { menu = false; workspace.messagingModel.markRead(conversation.id, conversation.lastSeq) })
                    }
                }
            } else Spacer(Modifier.weight(1f))
        }
        val invalid = messaging.error == "INVALID_DATA" || messaging.error == "INVALID_MESSAGE_SNAPSHOT"
        if (messaging.directoryReady && messaging.offline && !invalid) TeamsOfflineBanner(workspace::reconnect)
        if (messaging.error.isNotEmpty() && messaging.error != "SERVICE_UNAVAILABLE") TeamsErrorBanner(messaging.error, onRetry = workspace::reconnect)

        when {
            conversation == null && creation != null -> PendingConversation(creation, onDiscard = { workspace.messagingModel.discardCreation(); onBack() }, modifier = Modifier.weight(1f))
            conversation == null -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(28.dp)) }
            else -> ConversationBody(workspace, uid, team, teams, messaging, conversation, label, members, strings, online, onRecall = { recalling = it }, modifier = Modifier.weight(1f))
        }
    }

    if (showDetails && conversation != null) ConversationDetailsSheet(
        conversation = conversation, label = label, members = members, bots = bots, uid = uid, strings = strings,
        unread = if (messaging.readsReady) TeamsListLogic.unreadCount(conversation) else 0, online = online,
        onMarkRead = { workspace.messagingModel.markRead(conversation.id, conversation.lastSeq) }, onDismiss = { showDetails = false },
    )
    recalling?.let { message ->
        AlertDialog(
            onDismissRequest = { recalling = null },
            title = { Text(stringResource(R.string.teams_recall_confirm)) }, text = { Text(stringResource(R.string.teams_recall_confirm_detail)) },
            confirmButton = { TextButton(onClick = { workspace.messagingModel.queue("recall", "", message.id); recalling = null }) { Text(stringResource(R.string.teams_message_recall), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { recalling = null }) { Text(stringResource(R.string.common_cancel)) } },
        )
    }
}

@Composable
private fun conversationSubtitle(conversation: TeamMessageConversation, bots: List<TeamBotCard>): String? = when (conversation.kind) {
    "channel", "group" -> {
        val members = conversation.participants.count { it.startsWith("user:") }
        val botCount = conversation.participants.count { it.startsWith("shared_bot:") }
        listOfNotNull(
            members.takeIf { it > 0 }?.let { stringResource(R.string.teams_members_count_fmt, it) },
            botCount.takeIf { it > 0 }?.let { stringResource(if (it == 1) R.string.teams_bots_count_one_fmt else R.string.teams_bots_count_other_fmt, it) },
        ).joinToString(" · ").ifEmpty { null }
    }
    "bot_dm" -> bots.firstOrNull { it.sharedBotId == conversation.sharedBotId }?.let { botAvailabilityText(it) }
    else -> null
}

@Composable
private fun PendingConversation(creation: JSONObject, onDiscard: () -> Unit, modifier: Modifier) {
    val failed = creation.has("error")
    Column(modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        if (!failed) CircularProgressIndicator(Modifier.size(28.dp))
        Spacer(Modifier.height(12.dp))
        Text(
            stringResource(when { failed -> teamsErrorRes(creation.getString("error")); creation.optBoolean("attempted") -> R.string.teams_message_confirming; else -> R.string.teams_message_pending }),
            style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (failed || !creation.optBoolean("attempted")) TextButton(onClick = onDiscard) { Text(stringResource(R.string.teams_message_discard)) }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ConversationBody(
    workspace: TeamsWorkspace, uid: String, team: TeamSummary, teams: TeamsState, messaging: TeamMessagingState, conversation: TeamMessageConversation,
    label: String, members: List<TeamMember>, strings: TeamsListStrings, online: Boolean, onRecall: (TeamMessage) -> Unit, modifier: Modifier,
) {
    val conversationId = conversation.id
    val cache = messaging.cache
    val bots = messaging.bots
    val thread = cache?.threads?.get(conversationId)
    val messages = thread?.messages.orEmpty()
    val zone = remember { ZoneId.systemDefault() }
    val minute by rememberNowState(60_000L)
    val tick = rememberNowState(1_000L)

    // The divider stays where it was when the conversation opened, even while automatic mark-read advances the watermark.
    var anchor by remember(conversationId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(conversationId, messaging.readsReady, thread != null) {
        if (anchor == null && thread != null) {
            if (messaging.readsReady) anchor = conversation.lastReadSeq else { delay(4_000); if (anchor == null) anchor = Long.MAX_VALUE }
        }
    }
    var startId by remember(conversationId) { mutableStateOf<String?>(null) }
    val streamMessages = remember(messages) { messages.map(StreamMessage::of) }
    val effectiveAnchor = anchor ?: Long.MAX_VALUE
    val start = TeamsStreamLogic.visibleStart(streamMessages, startId, effectiveAnchor, uid)
    val rows = remember(streamMessages, start, effectiveAnchor, uid, minute) { TeamsStreamLogic.buildStreamRows(streamMessages.drop(start), effectiveAnchor, uid, minute, zone) }
    val byId = remember(messages) { messages.associateBy { it.id } }
    val outbox = cache?.outbox.orEmpty().filter { (it.optJSONObject("payload") ?: it).optString("conversationId") == conversationId }
    val stream: List<StreamItem> = remember(rows, byId, outbox, start) {
        buildList {
            add(if (start > 0) StreamItem.Earlier else StreamItem.Notice)
            for (row in rows) when (row) {
                is StreamRow.Date -> add(StreamItem.Day(row))
                StreamRow.Unread -> add(StreamItem.NewMessages)
                is StreamRow.Message -> byId[row.message.id]?.let { add(StreamItem.Msg(it, row.grouped)) }
            }
            outbox.forEach { add(StreamItem.Local(it, "pending")) }
        }
    }

    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val atBottom by remember { derivedStateOf { listState.isAtBottom() } }
    var initial by remember(conversationId) { mutableStateOf(false) }
    var unseen by remember(conversationId) { mutableIntStateOf(0) }
    var knownSeq by remember(conversationId) { mutableLongStateOf(0L) }
    val newestSeq = messages.lastOrNull()?.seq ?: 0L

    // Opening: the divider with three messages above it, else the bottom. Afterwards: follow arrivals at the bottom, count them otherwise.
    LaunchedEffect(conversationId, anchor != null, thread != null) {
        if (!initial && anchor != null && thread != null) {
            val divider = stream.indexOfFirst { it === StreamItem.NewMessages }
            listState.scrollToItem(if (divider >= 0) maxOf(0, divider - 1) else maxOf(0, stream.lastIndex))
            knownSeq = newestSeq
            initial = true
        }
    }
    LaunchedEffect(newestSeq, outbox.size) {
        if (!initial) return@LaunchedEffect
        val wasAtBottom = listState.isAtBottom()
        val arrived = messages.count { it.seq > knownSeq && !(it.authorKind == "user" && it.authorUid == uid) }
        knownSeq = newestSeq
        if (wasAtBottom) { listState.scrollToItem(maxOf(0, stream.lastIndex)); unseen = 0 } else unseen += arrived
    }
    LaunchedEffect(atBottom) { if (atBottom) unseen = 0 }

    // Automatic mark-read (D12): the newest message stays in view for AUTO_MARK_READ_MS while the app is in front, online and not on cache.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val lifecycleState by lifecycle.currentStateFlow.collectAsState()
    val foreground = lifecycleState.isAtLeast(Lifecycle.State.RESUMED)
    val threshold = TeamsReadLogic.AUTO_MARK_READ_MS
    LaunchedEffect(atBottom, initial, newestSeq, conversation.lastReadSeq, messaging.offline, messaging.directoryCached, foreground, messaging.readsReady) {
        if (threshold == null || !atBottom || !initial || !messaging.readsReady) return@LaunchedEffect
        delay(threshold)
        val seq = TeamsReadLogic.autoMarkReadSeq(!messaging.offline, messaging.directoryCached, foreground, threshold, newestSeq, conversation.lastReadSeq, threshold)
        if (seq != null) workspace.messagingModel.markRead(conversationId, seq)
    }

    // Reply and edit targets; the unsent text lives in the workspace draft of this conversation.
    var draft by remember(conversationId) { mutableStateOf(workspace.draft(conversationId)) }
    var editingId by remember(conversationId) { mutableStateOf<String?>(null) }
    val replyTo = draft.replyToId?.let { byId[it] }
    val editing = editingId?.let { byId[it] }
    fun setDraft(next: ComposerDraft) { draft = next; workspace.setDraft(conversationId, next) }
    fun personName(id: String) = members.firstOrNull { it.uid == id }?.displayName?.takeIf { it.isNotEmpty() } ?: if (id == uid) strings.you else strings.member
    fun authorOf(message: TeamMessage): String = if (message.authorKind == "shared_bot") message.authorDisplayName ?: bots.firstOrNull { it.sharedBotId == message.sharedBotId }?.name ?: strings.bot else personName(message.authorUid)
    val copyText = rememberCopyToClipboard("LingXi team message")

    Column(modifier) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (thread == null || anchor == null) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(stringResource(R.string.teams_loading_messages), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            else if (messages.isEmpty() && outbox.isEmpty()) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(stringResource(R.string.teams_empty_thread), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            else LazyColumn(Modifier.fillMaxSize().testTag(TeamsTags.STREAM), state = listState, contentPadding = PaddingValues(vertical = 8.dp)) {
                items(stream, key = { it.key }) { item ->
                    when (item) {
                        StreamItem.Notice -> Text(stringResource(R.string.teams_disclosure), Modifier.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        StreamItem.Earlier -> TextButton(onClick = { startId = messages.getOrNull(maxOf(0, start - TeamsStreamLogic.WINDOW_SIZE))?.id; workspace.messagingModel.refreshTasks() }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.teams_messages_earlier)) }
                        is StreamItem.Day -> Text(dayLabelText(item.row.kind, item.row.atMs), Modifier.fillMaxWidth().padding(vertical = 8.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
                        StreamItem.NewMessages -> NewMessagesDivider()
                        is StreamItem.Msg -> MessageCell(
                            item, uid, conversation, bots, members, strings, messaging, tick, byId, ::authorOf,
                            onReply = { setDraft(draft.copy(replyToId = it.id)); editingId = null },
                            onEdit = { editingId = it.id; setDraft(draft.copy(replyToId = null)) },
                            onRecall = onRecall, onCopy = { copyText(it.text) }, onArtifact = { workspace.messagingModel.openArtifact(it) }, onRefreshTask = { workspace.messagingModel.refreshTask(it) },
                        )
                        is StreamItem.Local -> LocalCommandRow(item, tick, onDiscard = {
                            workspace.messagingModel.discard((item.intent.optJSONObject("payload") ?: item.intent).getString("changeId"))
                        })
                    }
                }
            }
            if (initial && !atBottom) Surface(
                onClick = { scope.launch { listState.animateScrollToItem(maxOf(0, stream.lastIndex)); unseen = 0 } },
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 12.dp).testTag(TeamsTags.PILL),
                shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer, shadowElevation = 4.dp,
            ) { JumpPill(unseen) }
        }
        TeamComposer(
            conversation = conversation, label = label, uid = uid, messaging = messaging, draft = draft,
            replyTo = replyTo, editing = editing, replyAuthor = replyTo?.let(::authorOf).orEmpty(),
            onDraft = ::setDraft, onCancelReply = { setDraft(draft.copy(replyToId = null)) }, onCancelEdit = { editingId = null },
            onSend = { text, targets, done ->
                val editingMessage = editing
                workspace.messagingModel.queue(if (editingMessage == null) "create" else "edit", text, editingMessage?.id, if (editingMessage == null) draft.replyToId else null, targets) {
                    done()
                    if (editingMessage != null) editingId = null else setDraft(ComposerDraft())
                }
            },
        )
    }
}

private fun LazyListState.isAtBottom(): Boolean {
    val info = layoutInfo
    val last = info.visibleItemsInfo.lastOrNull() ?: return true
    return last.index >= info.totalItemsCount - 1 && last.offset + last.size <= info.viewportEndOffset + 4
}

@Composable
private fun JumpPill(unseen: Int) {
    Row(Modifier.padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(Icons.Outlined.ArrowDownward, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer)
        Text(
            if (unseen > 0) stringResource(R.string.teams_new_messages_pill_fmt, unseen) else stringResource(R.string.teams_jump_latest),
            style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onPrimaryContainer,
        )
    }
}

@Composable
private fun NewMessagesDivider() {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).testTag(TeamsTags.UNREAD_DIVIDER), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        HorizontalDivider(Modifier.weight(1f), color = MaterialTheme.colorScheme.primary)
        Text(stringResource(R.string.teams_new_messages_divider), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
        HorizontalDivider(Modifier.weight(1f), color = MaterialTheme.colorScheme.primary)
    }
}

/** One message: avatar, name and clock on the first of a run, reply quote, body, edited mark, the question and run cards. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MessageCell(
    item: StreamItem.Msg, uid: String, conversation: TeamMessageConversation, bots: List<TeamBotCard>, members: List<TeamMember>, strings: TeamsListStrings,
    messaging: TeamMessagingState, tick: androidx.compose.runtime.State<Long>, byId: Map<String, TeamMessage>, authorOf: (TeamMessage) -> String,
    onReply: (TeamMessage) -> Unit, onEdit: (TeamMessage) -> Unit, onRecall: (TeamMessage) -> Unit, onCopy: (TeamMessage) -> Unit,
    onArtifact: (String) -> Unit, onRefreshTask: (String) -> Unit,
) {
    val message = item.message
    var menu by remember { mutableStateOf(false) }
    val own = message.authorKind == "user" && message.authorUid == uid
    val canAct = !message.recalled
    val reply = stringResource(R.string.teams_message_reply)
    val edit = stringResource(R.string.teams_message_edit)
    val recall = stringResource(R.string.teams_message_recall)
    val copy = stringResource(R.string.chat_message_copy)
    val editable = canAct && own && message.kind == "text"
    val card = bots.firstOrNull { it.sharedBotId == message.sharedBotId }
    Box {
        Row(
            Modifier.fillMaxWidth()
                .combinedClickable(onClick = {}, onLongClick = if (canAct) ({ menu = true }) else null)
                .semantics {
                    if (canAct) customActions = buildList {
                        add(CustomAccessibilityAction(reply) { onReply(message); true })
                        if (editable) { add(CustomAccessibilityAction(edit) { onEdit(message); true }); add(CustomAccessibilityAction(recall) { onRecall(message); true }) }
                        add(CustomAccessibilityAction(copy) { onCopy(message); true })
                    }
                }
                .padding(horizontal = 16.dp, vertical = if (item.grouped) 1.dp else 6.dp).testTag(TeamsTags.message(message.id)),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (item.grouped) Spacer(Modifier.size(32.dp))
            else if (message.authorKind == "shared_bot") BotAvatar(card?.avatarId ?: message.sharedBotId ?: "shared_bot", 32.dp)
            else PersonAvatar(message.authorUid, authorOf(message), 32.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                if (!item.grouped) Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(authorOf(message), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                    Text(TeamsStreamLogic.formatClock(message.createdAtMs), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                message.replyTo?.let { id ->
                    val source = byId[id]
                    Row(Modifier.height(androidx.compose.foundation.layout.IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(Modifier.width(2.dp).fillMaxSize().background(MaterialTheme.colorScheme.outlineVariant))
                        Text(
                            when { source == null -> stringResource(R.string.teams_message_reply); source.recalled -> stringResource(R.string.teams_message_recalled); else -> TeamsComposeLogic.snippet(source.text) },
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                if (message.recalled) Text(stringResource(R.string.teams_message_recalled), style = MaterialTheme.typography.bodyMedium, fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.onSurfaceVariant)
                else if (TeamsBotText.rendersMarkdown(message.authorKind, message.recalled)) AIText(markdown = message.text, modifier = Modifier.fillMaxWidth().testTag("teams.message.body.${message.id}"), onOpenLink = rememberTeamBotLinkOpener())
                else Text(message.text, style = MaterialTheme.typography.bodyLarge)
                if (message.editedAt != null && !message.recalled) Text(stringResource(R.string.teams_message_edited), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                val taskIds = message.taskId?.let { listOf(it) } ?: message.taskIds
                taskIds.forEach { taskId ->
                    val task = messaging.tasks.firstOrNull { it.taskId == taskId }
                    if (task != null) TeamTaskCard(task, bots.firstOrNull { it.sharedBotId == task.sharedBotId }?.name ?: strings.bot, onArtifact, onRefreshTask)
                    else TextButton(onClick = { onRefreshTask(taskId) }) { Text(stringResource(R.string.teams_task_view)) }
                }
                if (taskIds.isEmpty()) message.artifactId?.let { id -> TextButton(onClick = { onArtifact(id) }) { Text(stringResource(R.string.teams_artifact_open)) } }

            }
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(text = { Text(reply) }, onClick = { menu = false; onReply(message) })
            if (editable) {
                DropdownMenuItem(text = { Text(edit) }, onClick = { menu = false; onEdit(message) })
                DropdownMenuItem(text = { Text(recall, color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; onRecall(message) })
            }
            DropdownMenuItem(text = { Text(copy) }, onClick = { menu = false; onCopy(message) })
        }
    }
}

/** A command saved on this device and not yet confirmed (or rejected): the existing outbox semantics, ids and journals unchanged. */
@Composable
private fun LocalCommandRow(item: StreamItem.Local, tick: androidx.compose.runtime.State<Long>, onDiscard: () -> Unit) {
    val intent = item.intent
    val failed = intent.has("error")
    val attempted = intent.optBoolean("attempted")
    val payload = intent.optJSONObject("payload") ?: intent
    val body = if (payload.optString("operation") == "recall") stringResource(R.string.teams_message_recall) else payload.getString("text")
    val status = when {
        failed -> stringResource(teamsErrorRes(intent.getString("error")))
        attempted -> stringResource(R.string.teams_message_confirming)
        else -> stringResource(R.string.teams_message_pending)
    }
    val canDiscard = !(attempted && !failed) && intent.optLong("leaseUntil") <= tick.value
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp).clip(MaterialTheme.shapes.medium).background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (body != null) Text(body, style = MaterialTheme.typography.bodyMedium)
        Text(status, style = MaterialTheme.typography.labelSmall, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
        if (canDiscard) TextButton(onClick = onDiscard) { Text(stringResource(R.string.teams_message_discard)) }
    }
}
