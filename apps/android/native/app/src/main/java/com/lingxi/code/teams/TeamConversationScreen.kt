package com.lingxi.code.teams

import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.ArrowDownward
import androidx.compose.material.icons.outlined.Info
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
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
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
import com.lingxi.code.theme.ConfirmDialog
import com.lingxi.code.theme.ConversationLead
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.rememberReduceMotion
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
internal class FriendChatHeader(
    val uid: String,
    val name: String,
    val handle: String?,
    val onOpenProfile: () -> Unit,
    val menu: @Composable () -> Unit,
)

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
    val inboxRef = remember(team.id, conversationId, team.generation) { com.lingxi.code.inbox.InboxRef("team", team.id, conversationId, team.generation) }
    com.lingxi.code.inbox.InboxPresence(workspace.inbox, inboxRef)
    DisposableEffect(workspace, team.id, conversationId, team.generation) {
        workspace.openConversation(conversationId)
        onDispose {
            workspace.inbox.flushRead(inboxRef.key)
            if (workspace.messagingModel.state.value.selected == conversationId) workspace.openConversation("")
        }
    }
    val inboxProjection by workspace.inbox.projectedRows.collectAsState()
    val inboxRows by workspace.inbox.state.collectAsState()
    val unread = (inboxRows.rows.firstOrNull { it.ref == inboxRef } ?: inboxProjection[inboxRef.key])
        ?.takeIf { it.ref == inboxRef }?.let { maxOf(it.unreadCount, if (it.manualUnread) 1 else 0) } ?: 0
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
        Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            if (showBack) IconButton(onClick = onBack, modifier = Modifier.testTag(TeamsTags.BACK)) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) }
            else Spacer(Modifier.width(12.dp))
            if (friend != null) {
                ConversationHeaderIdentity(
                    label = label,
                    subtitle = friend.handle?.let { "@$it" },
                    actionLabel = stringResource(R.string.friends_menu_view_profile),
                    onOpen = friend.onOpenProfile,
                    modifier = Modifier.weight(1f),
                ) { PersonAvatar(friend.uid, friend.name, 40.dp) }
                friend.menu()
            } else if (conversation != null) {
                ConversationHeaderIdentity(
                    label = label,
                    subtitle = conversationSubtitle(conversation, bots),
                    actionLabel = stringResource(R.string.teams_conversation_details),
                    onOpen = { showDetails = true },
                    modifier = Modifier.weight(1f),
                ) { ConversationLead(conversation, members, bots, uid, strings, 40.dp, label) }
                IconButton(onClick = { showDetails = true }, modifier = Modifier.testTag(TeamsTags.INFO)) { Icon(Icons.Outlined.Info, stringResource(R.string.teams_conversation_details)) }
            } else {
                Text(
                    creation?.let { (it.optJSONObject("payload") ?: it).optString("title").takeIf(String::isNotBlank) }
                        ?: team.name,
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            TeamsConnectionMenu(
                error = messaging.error.takeIf { it.isNotEmpty() && it != "SERVICE_UNAVAILABLE" },
                offline = messaging.directoryReady && messaging.offline,
                onReconnect = workspace::reconnect,
                onMarkRead = if (friend == null && conversation != null && unread > 0 && online) {
                    { workspace.messagingModel.markRead(conversation.id, conversation.lastSeq) }
                } else null,
            )
        }

        when {
            conversation == null && creation != null -> PendingConversation(creation, onDiscard = { workspace.messagingModel.discardCreation(); onBack() }, modifier = Modifier.weight(1f))
            conversation == null -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(28.dp)) }
            else -> ConversationBody(workspace, uid, team, teams, messaging, conversation, label, members, strings, online, onRecall = { recalling = it }, modifier = Modifier.weight(1f), disclosure = friend == null)
        }
    }

    // The auto-reply switches wait for their own change: until the listener brings the next revision, another toggle would send the old one and conflict.
    var autoReplySent by remember(conversation?.id) { mutableStateOf<Long?>(null) }
    if (showDetails && conversation != null) ConversationDetailsSheet(
        conversation = conversation, label = label, members = members, bots = bots, uid = uid, strings = strings,
        unread = unread, online = online,
        onMarkRead = { workspace.messagingModel.markRead(conversation.id, conversation.lastSeq) }, onDismiss = { showDetails = false },
        onAutoReply = { targets -> val revision = conversation.autoReply?.revision ?: 0; autoReplySent = revision
            workspace.messagingModel.setConversationAutoReply(conversation.id, revision, targets, onFailed = { autoReplySent = null }) },
        busy = autoReplySent != null && autoReplySent == (conversation.autoReply?.revision ?: 0),
    )
    recalling?.let { message ->
        ConfirmDialog(
            title = stringResource(R.string.teams_recall_confirm), body = stringResource(R.string.teams_recall_confirm_detail),
            confirm = stringResource(R.string.teams_message_recall), destructive = true,
            onConfirm = { workspace.messagingModel.queue("recall", "", message.id); recalling = null }, onDismiss = { recalling = null },
        )
    }
}

@Composable
private fun ConversationHeaderIdentity(
    label: String,
    subtitle: String?,
    actionLabel: String,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
    avatar: @Composable () -> Unit,
) {
    Row(
        modifier.clip(RoundedCornerShape(12.dp)).clickable(role = Role.Button, onClickLabel = actionLabel, onClick = onOpen)
            .heightIn(min = 48.dp).padding(horizontal = 6.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        avatar()
        Column(Modifier.weight(1f)) {
            Text(label, Modifier.semantics { heading() }, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
internal fun conversationSubtitle(conversation: TeamMessageConversation, bots: List<TeamBotCard>): String? = when (conversation.kind) {
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

@Composable
private fun ConversationBody(
    workspace: TeamsWorkspace, uid: String, team: TeamSummary, teams: TeamsState, messaging: TeamMessagingState, conversation: TeamMessageConversation,
    label: String, members: List<TeamMember>, strings: TeamsListStrings, online: Boolean, onRecall: (TeamMessage) -> Unit, modifier: Modifier,
    disclosure: Boolean = true,
) {
    val conversationId = conversation.id
    val cache = messaging.cache
    val bots = messaging.bots
    val thread = cache?.threads?.get(conversationId)
    val messages = thread?.messages.orEmpty()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val lifecycleState by lifecycle.currentStateFlow.collectAsState()
    val foreground = lifecycleState.isAtLeast(Lifecycle.State.RESUMED)
    val inboxRef = remember(team.id, conversationId, team.generation) { com.lingxi.code.inbox.InboxRef("team", team.id, conversationId, team.generation) }
    val inboxState by workspace.inbox.state.collectAsState()
    val row = inboxState.rows.firstOrNull { it.ref == inboxRef }
    val readThrough = maxOf(row?.readThrough ?: 0, conversation.lastReadSeq)
    var receipt by remember(uid, inboxRef) { mutableStateOf<Long?>(null) }
    LaunchedEffect(uid, inboxRef, inboxState.dmReceipts, conversation.lastSeq, messaging.offline, foreground) {
        receipt = null
        if (!foreground || !inboxState.dmReceipts || messaging.offline || conversation.kind != "dm" ||
            conversation.participants.size != 2 || conversation.participants.any { !it.startsWith("user:") }) return@LaunchedEffect
        while (true) {
            receipt = try { workspace.inbox.peerReceipt(inboxRef) }
            catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (_: Exception) { null }
            delay(10_000)
        }
    }
    val receiptMessage = messages.lastOrNull { it.authorKind == "user" && it.authorUid == uid && it.seq <= (receipt ?: -1) && !it.recalled }?.id
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
    var loadOlderAt by remember(conversationId) { mutableStateOf<Long?>(null) }
    LaunchedEffect(messages.firstOrNull()?.seq, messaging.loadingHistory) {
        val before = loadOlderAt ?: return@LaunchedEffect
        if (!messaging.loadingHistory) {
            messages.firstOrNull()?.let { if (it.seq < before) startId = it.id }
            loadOlderAt = null
        }
    }
    val streamMessages = remember(messages) { messages.map(StreamMessage::of) }
    val effectiveAnchor = anchor ?: Long.MAX_VALUE
    val start = TeamsStreamLogic.visibleStart(streamMessages, startId, effectiveAnchor, uid)
    val rows = remember(streamMessages, start, effectiveAnchor, uid, minute) { TeamsStreamLogic.buildStreamRows(streamMessages.drop(start), effectiveAnchor, uid, minute, zone) }
    val byId = remember(messages) { messages.associateBy { it.id } }
    val outbox = cache?.outbox.orEmpty().filter { (it.optJSONObject("payload") ?: it).optString("conversationId") == conversationId }
    val stream: List<StreamItem> = remember(rows, byId, outbox, start, disclosure, thread?.hasEarlier) {
        buildList {
            // A friend chat omits the Teams disclosure, as on iOS and Web.
            if (start > 0 || thread?.hasEarlier == true) add(StreamItem.Earlier) else if (disclosure) add(StreamItem.Notice)
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
    val reduceMotion = rememberReduceMotion()
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
    LaunchedEffect(atBottom) { if (atBottom && thread?.hasLater != true) unseen = 0 }

    val visibleKeys by remember(listState) {
        derivedStateOf {
            val info = listState.layoutInfo
            info.visibleItemsInfo.filter { it.offset < info.viewportEndOffset && it.offset + it.size > info.viewportStartOffset }
                .mapNotNull { it.key as? String }.toSet()
        }
    }
    val visibleSeq = remember(messages, visibleKeys) { messages.filter { "message:${it.id}" in visibleKeys }.map { it.seq } }
    LaunchedEffect(initial, visibleSeq, readThrough, foreground, messaging.readsReady) {
        if (!initial || !foreground || !messaging.readsReady) return@LaunchedEffect
        val through = com.lingxi.code.inbox.contiguousVisibleRead(readThrough, visibleSeq)
        if (through <= readThrough) return@LaunchedEffect
        delay(300)
        workspace.messagingModel.markRead(conversationId, through, automatic = true)
    }
    var jumpAfterLoad by remember(conversationId) { mutableStateOf(false) }
    LaunchedEffect(jumpAfterLoad, messaging.loadingHistory, stream.lastOrNull()?.key) {
        if (jumpAfterLoad && !messaging.loadingHistory && thread?.hasLater != true) {
            listState.scrollToItem(maxOf(0, stream.lastIndex))
            jumpAfterLoad = false
        }
    }

    // Reply and edit targets; the unsent text lives in the workspace draft of this conversation.
    val draft = workspace.draft(conversationId)
    var editingId by remember(conversationId) { mutableStateOf<String?>(null) }
    val replyTo = draft.replyToId?.let { byId[it] }
    val editing = editingId?.let { byId[it] }
    fun setDraft(next: ComposerDraft) { workspace.setDraft(conversationId, next) }
    fun personName(id: String) = members.firstOrNull { it.uid == id }?.displayName?.takeIf { it.isNotEmpty() } ?: if (id == uid) strings.you else strings.member
    // U3: the signed-in person reads "You" in the stream and in "Replying to", whatever their display name.
    val you = stringResource(R.string.teams_you)
    fun authorOf(message: TeamMessage): String = when {
        message.authorKind == "shared_bot" -> message.authorDisplayName ?: bots.firstOrNull { it.sharedBotId == message.sharedBotId }?.name ?: strings.bot
        message.authorKind == "user" && message.authorUid == uid -> you
        else -> personName(message.authorUid)
    }
    val copyText = rememberCopyToClipboard(stringResource(R.string.teams_clip_message))
    val viewer = TeamMemberRef(uid, team.generation)
    var reviewing by remember(conversation.id) { mutableStateOf<String?>(null) }
    reviewing?.let { taskId ->
        TeamOperationsDialog(taskId, workspace.messagingModel, TeamOperationRules.maintains(taskId, messaging.tasks, bots, viewer), members) { reviewing = null }
    }
    val pendingWaits = remember(outbox) { TeamTaskActions.pendingWaitIds(outbox) }
    // The draft's files: a new message carries them, an edit and an answer to a Bot's question do not (the files wait in the draft).
    val answering = if (editing == null) TeamTaskActions.answerableWait(replyTo, messaging.tasks, viewer, pendingWaits) else null
    val carriesFiles = editing == null && answering == null
    val draftFiles by workspace.draftFiles.collectAsState()
    val fileRefusals by workspace.fileRefusals.collectAsState()
    val fileKey = workspace.draftKey(conversationId)
    val files = if (carriesFiles) draftFiles[fileKey].orEmpty() else emptyList()
    // Small copies of the pictures shown, in memory, for as long as this conversation is open.
    val pictures = remember(conversationId) { mutableStateMapOf<String, androidx.compose.ui.graphics.ImageBitmap>() }

    Column(modifier) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (thread == null || anchor == null) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(stringResource(R.string.teams_loading_messages), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            else if (messages.isEmpty() && outbox.isEmpty()) TeamsMessageState(stringResource(R.string.teams_preview_none), body = stringResource(R.string.teams_empty_thread_hint))
            else LazyColumn(
                Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerLowest).testTag(TeamsTags.STREAM),
                state = listState,
                contentPadding = PaddingValues(vertical = 8.dp),
                verticalArrangement = Arrangement.Bottom,
            ) {
                items(stream, key = { it.key }) { item ->
                    when (item) {
                        StreamItem.Notice -> Text(stringResource(R.string.teams_disclosure), Modifier.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        StreamItem.Earlier -> TextButton(
                            onClick = {
                                if (start > 0) startId = messages.getOrNull(maxOf(0, start - 50))?.id
                                else { loadOlderAt = messages.firstOrNull()?.seq; workspace.messagingModel.loadEarlier(conversationId) }
                            },
                            enabled = !messaging.loadingHistory,
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(stringResource(R.string.teams_messages_earlier)) }
                        is StreamItem.Day -> Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                            Surface(shape = CircleShape, color = MaterialTheme.colorScheme.surfaceContainer) {
                                Text(
                                    dayLabelText(item.row.kind, item.row.atMs),
                                    Modifier.padding(horizontal = 11.dp, vertical = 4.dp).semantics { heading() },
                                    style = MaterialTheme.typography.labelMedium,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    textAlign = TextAlign.Center,
                                )
                            }
                        }
                        StreamItem.NewMessages -> NewMessagesDivider()
                        is StreamItem.Msg -> Column {
                            MessageCell(
                                item, uid, viewer, pendingWaits, ::personName, conversation, bots, members, strings, messaging, tick, byId, ::authorOf, workspace, pictures,
                                onReply = { setDraft(draft.copy(replyToId = it.id)); editingId = null },
                                onEdit = { editingId = it.id; setDraft(draft.copy(replyToId = null)) },
                                onRecall = onRecall, onCopy = { copyText(it.text) }, onArtifact = { workspace.messagingModel.openArtifact(it) }, onRefreshTask = { workspace.messagingModel.refreshTask(it) }, onCancelTask = { workspace.messagingModel.cancelTask(it) }, onReviewTask = { reviewing = it },
                                onDecideWait = { waitId, taskId, approve, ids -> workspace.messagingModel.decideWait(waitId, taskId, approve, ids) },
                            )
                            if (item.message.id == receiptMessage) Text(
                                stringResource(R.string.inbox_read_receipt),
                                Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                                style = MaterialTheme.typography.labelSmall, textAlign = TextAlign.End,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        is StreamItem.Local -> LocalCommandRow(item, tick, onDiscard = {
                            workspace.messagingModel.discard((item.intent.optJSONObject("payload") ?: item.intent).getString("changeId"))
                        })
                    }
                }
            }
            if (initial && (!atBottom || thread?.hasLater == true)) Surface(
                onClick = {
                    workspace.messagingModel.markRead(conversationId, conversation.lastSeq)
                    if (thread?.hasLater == true) {
                        workspace.messagingModel.jumpLatest(conversationId)
                        jumpAfterLoad = true
                        startId = null
                    } else scope.launch {
                        if (reduceMotion) listState.scrollToItem(maxOf(0, stream.lastIndex)) else listState.animateScrollToItem(maxOf(0, stream.lastIndex))
                        unseen = 0
                    }
                },
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 12.dp).testTag(TeamsTags.PILL),
                shape = CircleShape, color = MaterialTheme.colorScheme.primaryContainer, shadowElevation = 4.dp,
            ) { JumpPill(unseen) }
        }
        if (conversation.kind == "bot_dm") bots.firstOrNull { it.sharedBotId == conversation.sharedBotId }?.let { bot ->
            val lock = TeamsComposeLogic.composerLock(uid, conversation.kind, conversation.participants, bots, messaging.botsLoaded, messaging.botsFailed, conversation.readers)
            TeamBotMemberInfo(
                bot, enabled = editing == null && draft == ComposerDraft() && files.isEmpty() && lock == ComposerLock.None,
                onExample = { example -> workspace.botEntryContext(uid)?.let { workspace.seedBotExample(it, conversationId, example) } },
                modifier = Modifier.padding(horizontal = 16.dp),
            )
        }
        TeamComposer(
            conversation = conversation, label = label, uid = uid, messaging = messaging, draft = draft,
            replyTo = replyTo, editing = editing, replyAuthor = replyTo?.let(::authorOf).orEmpty(),
            onDraft = ::setDraft, onCancelReply = { setDraft(draft.copy(replyToId = null)) }, onCancelEdit = { editingId = null },
            voiceTarget = workspace.draftKey(conversationId),
            onSend = { text, targets, done ->
                val editingMessage = editing
                // A reply to a Bot's question that still waits for the viewer is its answer: it continues that task and asks no Bot.
                val waitId = if (editingMessage == null) TeamTaskActions.answerableWait(draft.replyToId?.let { byId[it] }, messaging.tasks, viewer, pendingWaits) else null
                // The files are this conversation's draft's, read at the moment of sending: no other conversation's are ever sent here.
                val sending = if (editingMessage == null && waitId == null) workspace.draftFiles.value[workspace.draftKey(conversationId)].orEmpty() else emptyList()
                workspace.messagingModel.queue(if (editingMessage == null) "create" else "edit", text, editingMessage?.id, if (editingMessage == null) draft.replyToId else null, if (waitId == null) targets else emptyList(), waitId,
                    files = sending, conversationId = conversationId) {
                    done()
                    workspace.filesSent(conversationId, sending.map { it.uploadId })
                    if (editingMessage != null) editingId = null else setDraft(ComposerDraft())
                }
            },
            files = files, refusals = if (carriesFiles) fileRefusals[fileKey].orEmpty() else emptyList(),
            onChooseFiles = if (carriesFiles) ({ chosen -> workspace.addFiles(conversationId, chosen) }) else null,
            onRemoveFile = { workspace.removeFile(conversationId, it) }, onDismissRefusals = { workspace.dismissRefusals(conversationId) },
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

/**
 * Text and attachments share a bubble. Bot replies use the Agent Chat Markdown renderer, with their
 * task and approval cards in the same reading column. Interactive attachments and cards retain their own semantics.
 */
@Composable
private fun MessageCell(
    item: StreamItem.Msg, uid: String, viewer: TeamMemberRef, pendingWaits: Set<String>, personName: (String) -> String, conversation: TeamMessageConversation, bots: List<TeamBotCard>, members: List<TeamMember>, strings: TeamsListStrings,
    messaging: TeamMessagingState, tick: androidx.compose.runtime.State<Long>, byId: Map<String, TeamMessage>, authorOf: (TeamMessage) -> String,
    workspace: TeamsWorkspace, pictures: androidx.compose.runtime.snapshots.SnapshotStateMap<String, androidx.compose.ui.graphics.ImageBitmap>,
    onReply: (TeamMessage) -> Unit, onEdit: (TeamMessage) -> Unit, onRecall: (TeamMessage) -> Unit, onCopy: (TeamMessage) -> Unit,
    onArtifact: (String) -> Unit, onRefreshTask: (String) -> Unit, onCancelTask: (String) -> Unit, onReviewTask: (String) -> Unit,
    onDecideWait: (waitId: String, taskId: String, approve: Boolean, ids: TeamDecisionIds) -> Unit,
) {
    val message = item.message
    var menu by remember { mutableStateOf(false) }
    val own = message.authorKind == "user" && message.authorUid == uid
    val canAct = !message.recalled
    val reply = stringResource(R.string.teams_message_reply)
    val edit = stringResource(R.string.teams_message_edit)
    val recall = stringResource(R.string.teams_message_recall)
    val copy = stringResource(R.string.chat_message_copy)
    // A decision on a Bot's request is not text to rewrite or take back.
    val editable = canAct && own && message.kind == "text" && message.decision == null
    val request = message.approvalRequest
    // The service writes an approval request and the reply to it in English; they are shown in the viewer's language.
    val body = when {
        request != null -> stringResource(R.string.teams_approval_request_fmt, request.title)
        message.authorKind == "user" && message.waitId != null && message.decision != null -> stringResource(if (message.decision) R.string.teams_approval_reply_approved else R.string.teams_approval_reply_declined)
        else -> message.text
    }
    val card = bots.firstOrNull { it.sharedBotId == message.sharedBotId }
    val bot = message.authorKind == "shared_bot"
    val haptics = LocalHapticFeedback.current
    Box {
        Row(
            Modifier.widthIn(max = 832.dp).fillMaxWidth().align(Alignment.TopCenter)
                .padding(horizontal = 12.dp).padding(top = if (item.grouped) 1.dp else 8.dp, bottom = 1.dp)
                .testTag(TeamsTags.message(message.id)),
            horizontalArrangement = if (own) Arrangement.End else Arrangement.spacedBy(8.dp),
        ) {
            if (own) Spacer(Modifier.width(36.dp))
            else if (item.grouped) Spacer(Modifier.size(32.dp))
            else if (bot) BotAvatar(card?.avatarId ?: message.sharedBotId ?: "shared_bot", 32.dp)
            else PersonAvatar(message.authorUid, authorOf(message), 32.dp)
            Column(
                Modifier.weight(1f, fill = bot)
                    .widthIn(max = if (bot) 704.dp else 544.dp),
                horizontalAlignment = if (own) Alignment.End else Alignment.Start,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Column(
                    Modifier
                        .then(if (bot) Modifier.padding(vertical = 3.dp) else Modifier.teamBubble(own, item.grouped))
                        .then(if (canAct) Modifier.pointerInput(message.id) {
                            detectTapGestures(onLongPress = {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                menu = true
                            })
                        } else Modifier),
                    verticalArrangement = Arrangement.spacedBy(3.dp),
                ) {
                    Column(
                        Modifier.semantics(mergeDescendants = true) {
                            if (canAct) customActions = buildList {
                                add(CustomAccessibilityAction(reply) { onReply(message); true })
                                if (editable) { add(CustomAccessibilityAction(edit) { onEdit(message); true }); add(CustomAccessibilityAction(recall) { onRecall(message); true }) }
                                if (message.text.isNotEmpty()) add(CustomAccessibilityAction(copy) { onCopy(message); true })
                            }
                        },
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        if (!item.grouped) Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(authorOf(message), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                            if (message.authorKind == "shared_bot") BotTag()
                        }
                        message.replyTo?.let { id ->
                            val source = byId[id]
                            Row(
                                Modifier.clip(RoundedCornerShape(5.dp))
                                    .background(MaterialTheme.colorScheme.primary.copy(alpha = OWN_MESSAGE_TINT))
                                    .padding(horizontal = 6.dp, vertical = 3.dp)
                                    .height(androidx.compose.foundation.layout.IntrinsicSize.Min),
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                Box(Modifier.width(2.dp).fillMaxSize().background(MaterialTheme.colorScheme.primary))
                                Column {
                                    if (source != null) Text(
                                        authorOf(source), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold,
                                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                                    )
                                    Text(
                                        when {
                                            source == null -> stringResource(R.string.teams_message_reply)
                                            source.recalled -> stringResource(R.string.teams_message_recalled)
                                            else -> TeamsComposeLogic.snippet(TeamAttachments.said(source.text, source.files.map { it.name }))
                                        },
                                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        maxLines = 2, overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                        }
                        if (message.recalled) Text(stringResource(R.string.teams_message_recalled), style = MaterialTheme.typography.bodyMedium, fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        else if (message.text.isEmpty() && message.files.isNotEmpty()) Unit // A message of files alone has no text line.
                        else if (TeamsBotText.rendersMarkdown(message.authorKind, message.recalled)) AIText(markdown = body, modifier = Modifier.fillMaxWidth().testTag("teams.message.body.${message.id}"), onOpenLink = rememberTeamBotLinkOpener())
                        else Text(body, style = MaterialTheme.typography.bodyLarge)
                        val edited = message.editedAt != null && !message.recalled
                        val time = TeamsStreamLogic.formatClock(message.createdAtMs)
                        if (edited || time.isNotEmpty()) Row(
                            Modifier.align(if (bot) Alignment.Start else Alignment.End),
                            horizontalArrangement = Arrangement.spacedBy(5.dp),
                        ) {
                            val style = MaterialTheme.typography.labelSmall
                            val color = MaterialTheme.colorScheme.onSurfaceVariant
                            if (edited) Text(stringResource(R.string.teams_message_edited), style = style, color = color)
                            if (time.isNotEmpty()) Text(time, style = style, color = color)
                        }
                    }
                    // Attachments and their controls remain reachable independently of the message text.
                    if (message.files.isNotEmpty()) TeamMessageFiles(message, own, workspace, pictures)
                    // What the Bot's model was given, as the service checked it.
                    if (!message.recalled && message.readFiles.isNotEmpty()) Text(stringResource(R.string.teams_files_read_fmt, message.readFiles.joinToString(", ") { it.name }), Modifier.testTag("teams.message.read.${message.id}"), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                val requestTask = message.taskId
                if (request != null && message.waitId != null && requestTask != null) {
                    val state = TeamTaskActions.approvalState(message.waitId, messaging.tasks.firstOrNull { it.taskId == requestTask }, viewer, messaging.cache?.threads?.get(conversation.id)?.messages ?: emptyList())
                    val decider = when (state) { is TeamApprovalState.Waiting -> personName(state.uid); is TeamApprovalState.Approved -> personName(state.uid); is TeamApprovalState.Declined -> personName(state.uid); else -> "" }
                    TeamApprovalCard(request, message.waitId, state, decider) { approve, ids -> onDecideWait(message.waitId, requestTask, approve, ids) }
                }
                val taskIds = message.taskId?.let { listOf(it) } ?: message.taskIds
                taskIds.forEach { taskId ->
                    val task = messaging.tasks.firstOrNull { it.taskId == taskId }
                    if (task != null) {
                        val taskBot = bots.firstOrNull { it.sharedBotId == task.sharedBotId }
                        val answerable = TeamTaskActions.answerableWait(message, messaging.tasks, viewer, pendingWaits) != null
                        TeamTaskCard(
                            task, taskBot?.name ?: strings.bot, onArtifact, onRefreshTask, viewer = viewer, maintainer = taskBot?.maintainer,
                            recipientName = task.wait?.let { personName(it.recipient.uid) }.orEmpty(), onCancel = onCancelTask, onReply = if (answerable) ({ onReply(message) }) else null, onReview = onReviewTask,
                        )
                    }
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
            if (message.text.isNotEmpty()) DropdownMenuItem(text = { Text(copy) }, onClick = { menu = false; onCopy(message) })
        }
    }
}

@Composable
private fun Modifier.teamBubble(own: Boolean, grouped: Boolean): Modifier {
    val colors = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(
        topStart = if (grouped && !own) 6.dp else 16.dp,
        topEnd = if (grouped && own) 6.dp else 16.dp,
        bottomEnd = 16.dp,
        bottomStart = 16.dp,
    )
    return clip(shape).background(colors.surfaceContainer)
        .then(if (own) Modifier.background(colors.primary.copy(alpha = OWN_MESSAGE_TINT)) else Modifier)
        .padding(horizontal = 11.dp, vertical = 6.dp)
}

/** "Bot" beside a Bot's name, on its secondary container so the two never read as one word. */
@Composable
private fun BotTag() {
    Text(
        stringResource(R.string.teams_bot_tag), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSecondaryContainer, maxLines = 1,
        modifier = Modifier.clip(RoundedCornerShape(4.dp)).background(MaterialTheme.colorScheme.secondaryContainer).padding(horizontal = 4.dp, vertical = 1.dp),
    )
}

/** A command saved on this device and not yet confirmed (or rejected): the existing outbox semantics, ids and journals unchanged. */
@Composable
private fun LocalCommandRow(item: StreamItem.Local, tick: androidx.compose.runtime.State<Long>, onDiscard: () -> Unit) {
    val intent = item.intent
    val failed = intent.has("error")
    val attempted = intent.optBoolean("attempted")
    val payload = intent.optJSONObject("payload") ?: intent
    val body = if (payload.optString("operation") == "recall") stringResource(R.string.teams_message_recall) else TeamAttachments.said(payload.getString("text"), TeamAttachments.names(intent))
    val status = when {
        failed -> stringResource(teamsErrorRes(intent.getString("error")))
        attempted -> stringResource(R.string.teams_message_confirming)
        else -> stringResource(R.string.teams_message_pending)
    }
    val canDiscard = !(attempted && !failed) && intent.optLong("leaseUntil") <= tick.value
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
        Box(Modifier.widthIn(max = 832.dp).fillMaxWidth().padding(start = 48.dp, end = 12.dp, top = 8.dp, bottom = 1.dp)) {
            Column(
                Modifier.widthIn(max = 544.dp).align(Alignment.CenterEnd).teamBubble(own = true, grouped = false),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                if (body.isNotEmpty()) Text(body, style = MaterialTheme.typography.bodyMedium)
                Text(status, style = MaterialTheme.typography.labelSmall, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                if (canDiscard) TextButton(onClick = onDiscard) { Text(stringResource(R.string.teams_message_discard)) }
            }
        }
    }
}
