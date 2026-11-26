package com.lingxi.code.teams

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.TimerOff
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.conversation.AIText
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.ConfirmDialog
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * What a screen keeps of the held replies it shows (`packages/teams/held-hooks.ts`, iOS `TeamHeldKeeper`): the list as of now ([list], state the screen reads) and the decisions on
 * it. [make] builds the board once, with what to call when the list changes; it makes none for a screen whose person maintains no Bot, which then has an empty, ready list and reads
 * nothing. [decided] is called after the service took a decision, so that the screen can fetch what it changed. Everything runs on the main thread.
 */
@Stable
internal class TeamHeldKeeper(private val scope: CoroutineScope, private val decided: () -> Unit, make: (changed: (TeamHeldList) -> Unit) -> TeamHeldBoard?) {
    var list by mutableStateOf(TeamHeldList(TeamHeldList.Status.Ready))
        private set
    private val board: TeamHeldBoard? = make { list = it }

    init { board?.let { list = it.list } }

    suspend fun load() { board?.load() }
    suspend fun keepTime() { board?.keepTime() }
    fun reload() { board?.let { scope.launch { it.load() } } }
    /** Lets the reply go as the Bot made it, or with [text] as the maintainer changed it to. */
    fun approve(taskId: String, text: String? = null) { board?.let { scope.launch { if (it.approve(taskId, text)) decided() } } }
    fun reject(taskId: String) { board?.let { scope.launch { if (it.reject(taskId)) decided() } } }
    /** The person has read what became of a reply; it goes from the screen. */
    fun dismiss(taskId: String) { board?.dismiss(taskId) }
    fun close() { board?.close() }
}

/**
 * The replies of [bots] that wait for this person on one screen: in one Chat ([conversationId]) or, without it, in every Chat of the Team. They are read when the screen opens,
 * whenever [watch] changes (the screen says it with whatever tells it the Chat's tasks moved) and when the connection returns, never on a timer of their own, so no oftener than the
 * Chat's tasks are; nothing is read while [online] is false, and what was read stays. The time left is kept while the screen is composed. A change of model, Chat or Bots starts the
 * list over, and what was read for the old one is never shown for the new one. No Bots (the person maintains none here): nothing is read and the list is empty.
 */
@Composable
internal fun rememberTeamHeld(model: TeamMessagingModel, bots: List<String>, conversationId: String?, online: Boolean, watch: String): TeamHeldKeeper {
    val scope = rememberCoroutineScope()
    val connected = rememberUpdatedState(online)
    val keeper = remember(model, bots, conversationId) {
        TeamHeldKeeper(scope, model::refreshTasks) { changed ->
            if (bots.isEmpty()) null else TeamHeldBoard(
                scope, read = { TeamHeldRules.read(bots) { id -> model.listHeldReplies(id, conversationId) } },
                decide = TeamHeldDecide(approve = model::approveReply, reject = model::rejectReply), changed = changed, online = { connected.value },
            )
        }
    }
    DisposableEffect(keeper) { onDispose { keeper.close() } }
    LaunchedEffect(keeper, online, watch) { keeper.load() }
    LaunchedEffect(keeper) { keeper.keepTime() }
    return keeper
}

/**
 * A reply of a Bot that waits for its maintainer, as that person reads it (`packages/teams/HeldReplyCard.tsx`, iOS `TeamHeldCard`): the Bot's words and the document that goes with
 * them, the time they have to decide, and what they can do. They approve the reply as it is, change its text and send that, or reject it after being asked once more. Nobody else
 * is shown any of it. [botName] names the Bot where the card sits under the message that asked it (the card of a reply nobody asked for is under the Bot's name already).
 *
 * Once the reply is no longer there to decide (it was decided, its time ran out, a newer message dropped it) the card says so in its place, without the words and without the
 * buttons, and goes when it is closed. [inList]: the card of the list of a Bot's replies, which is already under the Chat it belongs to and leaves out the line that says what it is.
 */
@Composable
internal fun TeamHeldCard(row: TeamHeldRow, now: Long, keeper: TeamHeldKeeper, modifier: Modifier = Modifier, botName: String? = null, inList: Boolean = false) {
    val id = row.reply.taskId
    val over = row.over(now)
    Column(
        modifier.fillMaxWidth()
            .then(if (inList) Modifier else Modifier.clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(10.dp))
            .testTag("teams.held.$id"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (over != null) TeamHeldOver(row, over, keeper) else TeamHeldWaiting(row, now, keeper, botName, inList)
    }
}

/** The time to decide is over, or the service said the reply is not there: what became of it, and Close once the service said it. */
@Composable
private fun TeamHeldOver(row: TeamHeldRow, code: String, keeper: TeamHeldKeeper) {
    val id = row.reply.taskId
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(Icons.Outlined.TimerOff, contentDescription = null, modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(stringResource(TeamHeldRules.errorRes(code)), Modifier.testTag("teams.held.over.$id"), style = MaterialTheme.typography.bodySmall)
    }
    if (row.ended != null) TextButton(onClick = { keeper.dismiss(id) }, modifier = Modifier.testTag("teams.held.close.$id")) { Text(stringResource(R.string.common_close)) }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TeamHeldWaiting(row: TeamHeldRow, now: Long, keeper: TeamHeldKeeper, botName: String?, inList: Boolean) {
    val reply = row.reply
    val id = reply.taskId
    val busy = row.busy != null
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    // What the maintainer is typing survives a turn of the screen; it is theirs until they send it or take it back.
    var editing by rememberSaveable(id) { mutableStateOf<String?>(null) }
    var rejecting by remember(id) { mutableStateOf(false) }
    val edit = editing?.let { TeamHeldRules.edit(reply.text, it) }
    if (!inList) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(Icons.Outlined.Schedule, contentDescription = null, modifier = Modifier.size(16.dp), tint = muted)
        Text(stringResource(R.string.teams_held_title), Modifier.semantics { heading() }, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
    }
    if (botName != null) Text(botName, style = MaterialTheme.typography.titleSmall)
    Row(Modifier.testTag("teams.held.expires.$id"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(stringResource(R.string.teams_held_expires_fmt, teamExpiryText(reply.expiresAt, now)), style = MaterialTheme.typography.labelMedium, color = muted)
        if (reply.unasked) Text(
            stringResource(R.string.teams_held_unasked), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, color = muted,
            modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surfaceContainerHighest).padding(horizontal = 6.dp, vertical = 1.dp),
        )
    }
    val draft = editing
    if (draft != null) {
        val problem = edit?.let(TeamHeldRules::problemRes)
        HeldEditor(id, draft, enabled = !busy, problem = problem?.let { stringResource(it) }, onChange = { editing = it })
    } else {
        AIText(markdown = reply.text, modifier = Modifier.fillMaxWidth().testTag("teams.held.text.$id"), onOpenLink = rememberTeamBotLinkOpener())
    }
    reply.document?.let { document ->
        Row(Modifier.testTag("teams.held.document.$id"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(Icons.Outlined.Description, contentDescription = null, modifier = Modifier.size(16.dp), tint = muted)
            Text(stringResource(R.string.teams_held_document_fmt, document.title), style = MaterialTheme.typography.labelMedium, color = muted)
        }
    }
    // The buttons wrap under each other at a large font size rather than run out of the card.
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (draft != null) {
            val send = edit as? TeamHeldEdit.Send
            Button(onClick = { send?.let { keeper.approve(id, it.text) } }, enabled = send != null && !busy, modifier = Modifier.testTag("teams.held.send.$id")) { Text(stringResource(R.string.teams_message_send)) }
            OutlinedButton(onClick = { editing = null }, enabled = !busy, modifier = Modifier.testTag("teams.held.cancel.$id")) { Text(stringResource(R.string.common_cancel)) }
        } else {
            Button(onClick = { keeper.approve(id) }, enabled = !busy, modifier = Modifier.testTag("teams.held.approve.$id")) { Text(stringResource(R.string.teams_approval_approve)) }
            OutlinedButton(onClick = { editing = reply.text }, enabled = !busy, modifier = Modifier.testTag("teams.held.edit.$id")) { Text(stringResource(R.string.teams_message_edit)) }
            OutlinedButton(onClick = { rejecting = true }, enabled = !busy, modifier = Modifier.testTag("teams.held.reject.$id")) {
                Text(stringResource(R.string.teams_held_reject), color = MaterialTheme.colorScheme.error)
            }
        }
    }
    row.error?.let { code ->
        Text(
            stringResource(TeamHeldRules.errorRes(code)), Modifier.testTag("teams.held.error.$id").semantics { liveRegion = LiveRegionMode.Polite },
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error,
        )
    }
    // The question is answered once it is asked: what comes of the decision is told on the card, which is the one thing on screen that can say it.
    if (rejecting) ConfirmDialog(
        title = stringResource(R.string.teams_held_reject_confirm), body = stringResource(R.string.teams_held_reject_detail), confirm = stringResource(R.string.teams_held_reject),
        destructive = true, confirmTag = "teams.held.reject.confirm.$id", dialogTag = "teams.held.reject.dialog.$id",
        onConfirm = { rejecting = false; keeper.reject(id) }, onDismiss = { rejecting = false },
    )
}

/** The editor of the maintainer's own text, with the focus as soon as it opens; [problem] says why what is typed cannot be sent. */
@Composable
private fun HeldEditor(id: String, text: String, enabled: Boolean, problem: String?, onChange: (String) -> Unit) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    OutlinedTextField(
        value = text, onValueChange = onChange, enabled = enabled, isError = problem != null, minLines = 3, maxLines = 12,
        label = { Text(stringResource(R.string.teams_held_edit_label)) },
        supportingText = problem?.let { message -> { Text(message, Modifier.testTag("teams.held.problem.$id")) } },
        modifier = Modifier.fillMaxWidth().focusRequester(focus).testTag("teams.held.editor.$id"),
    )
}

/**
 * A reply nobody asked for has no task in the Chat (the service keeps such a task out of every member's list), so it is a row of its own at the end of the stream, from the Bot that
 * made it. [card] is that Bot when its card is known.
 */
@Composable
internal fun TeamHeldCell(row: TeamHeldRow, now: Long, keeper: TeamHeldKeeper, card: TeamBotCard?, strings: TeamsListStrings) {
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
        Row(
            Modifier.widthIn(max = 832.dp).fillMaxWidth().padding(horizontal = 12.dp).padding(top = 8.dp, bottom = 1.dp).testTag("teams.held.row.${row.reply.taskId}"),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            BotAvatar(card?.avatarId ?: row.reply.sharedBotId, 32.dp)
            Column(Modifier.weight(1f).widthIn(max = 704.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(card?.name ?: strings.bot, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                    BotTag()
                }
                TeamHeldCard(row, now, keeper)
            }
        }
    }
}

/** The replies could not be read: what is wrong, and Retry. Shown only for a failure the Chat is not already saying. */
@Composable
internal fun TeamHeldReadFailure(code: String, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Surface(modifier.fillMaxWidth().testTag("teams.held.read.error").semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.errorContainer) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Outlined.ErrorOutline, contentDescription = null, modifier = Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onErrorContainer)
            Text(stringResource(TeamHeldRules.errorRes(code)), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onErrorContainer)
            TextButton(onClick = onRetry, modifier = Modifier.testTag("teams.held.read.retry")) { Text(stringResource(R.string.common_retry)) }
        }
    }
}

/**
 * Every reply of one Bot that waits for its maintainer, grouped by the Chat it was made in, the Chat of the newest reply first; each is the card of the Chat, with the same
 * decisions, and each Chat can be opened ([onOpenChat]). When the service says the person does not maintain the Bot after all, the list closes.
 */
@Composable
internal fun TeamHeldListDialog(bot: TeamBotCard, keeper: TeamHeldKeeper, onOpenChat: (String) -> Unit, onClose: () -> Unit) {
    val list = keeper.list
    LaunchedEffect(list.status) { if (list.status == TeamHeldList.Status.Denied) onClose() }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().testTag("teams.held.list"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding()) {
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose, modifier = Modifier.testTag("teams.held.list.close")) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
                    Text(stringResource(R.string.teams_held_waiting), Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge, maxLines = 1)
                    IconButton(enabled = list.status != TeamHeldList.Status.Loading, onClick = keeper::reload, modifier = Modifier.testTag("teams.held.list.refresh")) {
                        Icon(Icons.Outlined.Refresh, stringResource(R.string.teams_list_refresh))
                    }
                }
                if (list.status == TeamHeldList.Status.Loading) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
                LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    item("intro") { Text(stringResource(R.string.teams_held_intro), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    item("bot") { Text(bot.name, Modifier.padding(vertical = 4.dp).testTag("teams.held.list.bot"), style = MaterialTheme.typography.titleSmall) }
                    if (list.status == TeamHeldList.Status.Loading) item("loading") {
                        Text(stringResource(R.string.teams_loading), Modifier.testTag("teams.held.list.loading"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (list.status == TeamHeldList.Status.Ready && list.rows.isEmpty()) item("empty") {
                        Text(stringResource(R.string.teams_held_none), Modifier.testTag("teams.held.list.empty"), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    for (chat in TeamHeldRules.byChat(list.rows)) {
                        item("chat:${chat.id}") {
                            Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(chat.title ?: stringResource(R.string.teams_triage_chat_untitled), Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                TextButton(onClick = { onOpenChat(chat.id) }, modifier = Modifier.testTag("teams.held.list.open.${chat.id}")) { Text(stringResource(R.string.friends_open_chat)) }
                            }
                        }
                        for (row in chat.rows) item("held:${row.reply.taskId}") { TeamHeldCard(row, list.now, keeper, inList = true, modifier = Modifier.padding(bottom = 4.dp)) }
                    }
                    if (list.status == TeamHeldList.Status.Failed && list.error != null) item("error") {
                        Column {
                            Text(stringResource(TeamHeldRules.errorRes(list.error)), Modifier.testTag("teams.held.list.error").semantics { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.error)
                            if (list.rows.isEmpty()) TextButton(onClick = keeper::reload, modifier = Modifier.testTag("teams.held.list.retry")) { Text(stringResource(R.string.common_retry)) }
                        }
                    }
                }
            }
        }
    }
}

/**
 * "Replies waiting" on the card of a Bot its viewer maintains: a button with the number of replies of the Bot that wait for them across every Chat, only while there are some (and
 * while its list is open). [signals] counts the service's Bot change signals: the replies are read again on each. [onOpenChat] takes the person to the Chat a reply was made in.
 */
@Composable
internal fun TeamHeldEntry(bot: TeamBotCard, model: TeamMessagingModel, signals: Long, online: Boolean, onOpenChat: (String) -> Unit) {
    val keeper = rememberTeamHeld(model, listOf(bot.sharedBotId), null, online, signals.toString())
    var open by remember(bot.sharedBotId) { mutableStateOf(false) }
    val list = keeper.list
    val waiting = TeamHeldRules.waiting(list)
    if (list.status != TeamHeldList.Status.Denied && (waiting > 0 || open)) {
        TextButton(onClick = { open = true }, modifier = Modifier.testTag("teams.bot.held.${bot.sharedBotId}")) { TeamHeldEntryLabel(waiting) }
    }
    if (open) TeamHeldListDialog(bot, keeper, onOpenChat = { open = false; onOpenChat(it) }, onClose = { open = false })
}

@Composable
private fun TeamHeldEntryLabel(count: Int) {
    val label = stringResource(R.string.teams_held_waiting)
    val shown = if (count > 99) "99+" else count.toString()
    Row(Modifier.semantics(mergeDescendants = true) { contentDescription = "$label, $count" }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label)
        Text(
            shown, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Bold,
            modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)).padding(horizontal = 6.dp, vertical = 1.dp).clearAndSetSemantics { },
        )
    }
}
