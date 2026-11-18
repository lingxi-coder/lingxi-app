package com.lingxi.code.teams

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import com.lingxi.code.conversation.AIText
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.ConfirmDialog
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.rememberReduceMotion
import kotlinx.coroutines.launch
import java.util.UUID

/**
 * Bot services (Bots offered outside Teams) over the Teams tab: the services the person uses, a service looked up by its
 * link with its terms, and an open service's sessions. Nothing is offered from a phone. [link] is looked up once and never
 * entered by itself. The model reads nothing while the app is in the background.
 */
@Composable
internal fun HostedServicesScreen(
    onClose: () -> Unit,
    link: String? = null,
    modelOverride: HostedServicesModel? = null,
    inboxTarget: com.lingxi.code.inbox.InboxRef? = null,
) {
    val scope = rememberCoroutineScope()
    val model = modelOverride ?: remember { HostedServicesModel.create(scope) }
    val state by model.state.collectAsStateWithLifecycle()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val lifecycleState by lifecycle.currentStateFlow.collectAsState()
    val foreground = lifecycleState.isAtLeast(Lifecycle.State.STARTED)
    var followed by rememberSaveable { mutableStateOf(false) }
    var inboxTargetOpened by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(inboxTarget, state.listed, state.entered, foreground) {
        if (foreground && inboxTarget != null && state.listed && !inboxTargetOpened) {
            val service = state.entered.firstOrNull { it.serviceId == inboxTarget.sourceId && it.generation == inboxTarget.generation }
            if (service != null) {
                inboxTargetOpened = true
                model.show(service, inboxTarget.conversationId)
            } else onClose()
        }
    }
    LaunchedEffect(foreground) {
        if (foreground) { model.start(if (followed) null else link); followed = true } else model.pause()
    }
    DisposableEffect(model) { onDispose { model.pause() } }

    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)) {
        Surface(Modifier.fillMaxSize().testTag("services.screen"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding().imePadding()) {
                val open = state.open
                if (open == null) ServiceList(model, state, onClose) else ServiceSession(model, state, open, if (inboxTarget != null) onClose else model::closeService)
            }
        }
    }
    state.candidate?.let { candidate -> ServiceTerms(candidate, state.busy, onDismiss = model::dismissCandidate) { scope.launch { model.enter() } } }
}

@Composable
private fun ServiceList(model: HostedServicesModel, state: HostedServicesState, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var link by rememberSaveable { mutableStateOf("") }
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onClose) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
        Text(stringResource(R.string.services_title), Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge, maxLines = 1)
    }
    if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.services_intro), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        val lookup = { model.clearError(); scope.launch { model.lookup(link) }; Unit }
        OutlinedTextField(
            value = link, onValueChange = { link = it }, singleLine = true, label = { Text(stringResource(R.string.services_lookup_label)) },
            placeholder = { Text("https://…/services/s_…") }, modifier = Modifier.fillMaxWidth().testTag("services.link"),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Search),
            keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSearch = { if (link.isNotBlank()) lookup() }),
        )
        Button(onClick = lookup, enabled = !state.busy && link.isNotBlank(), modifier = Modifier.testTag("services.lookup")) { Text(stringResource(R.string.services_lookup_action)) }
        state.error?.let { Text(stringResource(HostedService.errorRes(it)), Modifier.testTag("services.error").semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error) }
        HorizontalDivider()
        Text(stringResource(R.string.services_used), Modifier.semantics { heading() }, style = MaterialTheme.typography.titleMedium)
        if (state.listed && state.entered.isEmpty()) Text(stringResource(R.string.services_none), color = MaterialTheme.colorScheme.onSurfaceVariant)
        state.entered.forEach { service ->
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) { model.show(service) }.padding(vertical = 6.dp).testTag("services.entered.${service.serviceId}"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                BotAvatar(service.sharedBotId, 40.dp)
                Column(Modifier.weight(1f)) {
                    Text(service.name, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(stringResource(R.string.services_offered_by_fmt, service.publisherName), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                    Text(stringResource(HostedService.availabilityRes(service.availability)), style = MaterialTheme.typography.labelSmall,
                        color = if (service.available) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

/** What a person is told before they enter. "Agree and open" is the acknowledgement the service records. */
@Composable
private fun ServiceTerms(service: HostedServiceView, busy: Boolean, onDismiss: () -> Unit, onAgree: () -> Unit) {
    val who = service.publisherName
    AlertDialog(
        onDismissRequest = onDismiss, modifier = Modifier.testTag("services.terms"),
        icon = { BotAvatar(service.sharedBotId, 56.dp) },
        title = { Text(service.name) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(HostedService.availabilityRes(service.availability)), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (service.description.isNotEmpty()) Text(service.description)
                Text(stringResource(R.string.services_terms_scope_fmt, who))
                Text(stringResource(R.string.services_terms_readers_fmt, who))
                Text(stringResource(R.string.services_terms_cost_fmt, who))
                Text("${stringResource(R.string.services_terms_retention)}: ${service.retention}")
                Text(service.userDaily?.let { stringResource(R.string.services_terms_daily_fmt, it, service.userConcurrent) } ?: stringResource(R.string.services_terms_concurrent_fmt, service.userConcurrent))
                Text(stringResource(R.string.services_terms_reach))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.services_terms_decline)) } },
        confirmButton = {
            Button(onClick = onAgree, enabled = !busy && !service.withdrawn, modifier = Modifier.testTag("services.agree")) { Text(stringResource(R.string.services_terms_agree)) }
        },
    )
}

@Composable
private fun ColumnScope.ServiceSession(model: HostedServicesModel, state: HostedServicesState, service: HostedServiceView, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    val reduceMotion = rememberReduceMotion()
    var text by rememberSaveable(service.serviceId) { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    var reporting by remember { mutableStateOf(false) }
    var leaving by remember { mutableStateOf(false) }
    var choosing by remember { mutableStateOf(false) }
    val list = rememberLazyListState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val inbox = remember(context) { com.lingxi.code.inbox.InboxController.get(context.applicationContext) }
    val ref = state.session?.let { session -> service.generation?.let { com.lingxi.code.inbox.InboxRef("service", service.serviceId, session, it) } }
    com.lingxi.code.inbox.InboxPresence(inbox, ref)
    LaunchedEffect(ref) { if (ref != null) inbox.prime(ref) }
    val inboxState by inbox.state.collectAsStateWithLifecycle()
    val projection by inbox.projectedRows.collectAsStateWithLifecycle()
    val readThrough = ref?.let { candidate -> inboxState.rows.firstOrNull { it.ref == candidate } ?: projection[candidate.key] }?.readThrough ?: 0
    val lifecycle by LocalLifecycleOwner.current.lifecycle.currentStateFlow.collectAsState()
    val visible by androidx.compose.runtime.remember(list) {
        androidx.compose.runtime.derivedStateOf { list.layoutInfo.visibleItemsInfo.mapNotNull { it.key as? String }.toSet() }
    }
    LaunchedEffect(ref, visible, state.messages, readThrough, lifecycle) {
        if (ref == null || !lifecycle.isAtLeast(Lifecycle.State.RESUMED)) return@LaunchedEffect
        val through = com.lingxi.code.inbox.contiguousVisibleRead(readThrough, state.messages.filter { it.id in visible }.map { it.seq })
        if (through > readThrough) {
            kotlinx.coroutines.delay(300)
            inbox.mutate(com.lingxi.code.inbox.InboxOperation.read(ref, through, inbox.snapshot(ref)?.manualUnreadRevision ?: 0, inbox.snapshot(ref)?.pendingActionRevision), automatic = true)
        }
    }
    BackHandler(onBack = onClose)
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onClose) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) }
        Column(Modifier.weight(1f)) {
            Text(service.name, Modifier.semantics { heading() }, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(stringResource(HostedService.availabilityRes(service.availability)), style = MaterialTheme.typography.labelSmall,
                color = if (service.available) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Box {
            IconButton(onClick = { menu = true }, modifier = Modifier.testTag("services.menu")) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.services_menu)) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                if (state.sessions.isNotEmpty()) DropdownMenuItem(text = { Text(stringResource(R.string.services_sessions)) }, onClick = { menu = false; choosing = true })
                DropdownMenuItem(text = { Text(stringResource(R.string.inbox_mark_read)) }, enabled = ref != null, onClick = {
                    menu = false
                    ref?.let { inbox.mutate(com.lingxi.code.inbox.InboxOperation.read(it, state.lastSeq, inbox.snapshot(it)?.manualUnreadRevision ?: 0, inbox.snapshot(it)?.pendingActionRevision)) }
                })
                DropdownMenuItem(text = { Text(stringResource(R.string.services_new_session)) }, enabled = !state.busy, onClick = {
                    menu = false; scope.launch { model.newSession(java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT).format(java.util.Date())) }
                })
                DropdownMenuItem(text = { Text(stringResource(R.string.services_report)) }, onClick = { menu = false; reporting = true }, modifier = Modifier.testTag("services.menu.report"))
                DropdownMenuItem(text = { Text(stringResource(R.string.services_leave), color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; leaving = true },
                    modifier = Modifier.testTag("services.menu.leave"))
            }
        }
    }
    if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
    if (!service.available) Text(stringResource(HostedService.availabilityRes(service.availability)) + if (service.withdrawn) "" else " · " + stringResource(R.string.services_cannot_answer),
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    val unanswered = state.tasks.filter { it.state != "completed" }
    LaunchedEffect(state.lastSeq, unanswered.size, state.olderWindow) {
        if (state.olderWindow) return@LaunchedEffect
        val count = 1 + state.messages.size + unanswered.size + if (state.earlier != null) 1 else 0
        if (count > 0) { if (reduceMotion) list.scrollToItem(count - 1) else list.animateScrollToItem(count - 1) }
    }
    LazyColumn(Modifier.weight(1f).fillMaxWidth().testTag("services.stream"), state = list) {
        item {
            Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                Text(stringResource(R.string.services_note_fmt, service.publisherName), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                service.userDailyLeft?.let { Text(stringResource(R.string.services_left_today_fmt, it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
        // Nothing here yet: the box below is the action (U9).
        if (state.messages.isEmpty() && state.tasks.isEmpty()) item("empty") { TeamsMessageState(stringResource(R.string.teams_preview_none), body = stringResource(R.string.services_empty), fill = false) }
        if (state.earlier != null) item("earlier") {
            TextButton(onClick = model::loadEarlier, enabled = !state.loadingHistory) { Text(stringResource(R.string.teams_messages_earlier)) }
        }
        items(state.messages, key = { it.id }) { message -> ServiceMessage(message, service, state.uid) }
        items(unanswered, key = { "task:" + it.taskId }) { task -> ServiceTaskLine(task, state, model) }
    }
    if (state.olderWindow && ref != null) {
        TextButton(onClick = {
            inbox.mutate(com.lingxi.code.inbox.InboxOperation.read(ref, state.lastSeq, inbox.snapshot(ref)?.manualUnreadRevision ?: 0, inbox.snapshot(ref)?.pendingActionRevision))
            model.jumpToLatest()
        }) { Text(stringResource(R.string.inbox_jump_latest)) }
    }
    state.error?.let { Text(stringResource(HostedService.errorRes(it)), Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp).testTag("services.session.error").semantics { liveRegion = LiveRegionMode.Polite },
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        val tooLong = TeamsComposeLogic.byteLength(text) > TeamsComposeLogic.MESSAGE_BYTES
        OutlinedTextField(value = text, onValueChange = { text = it }, enabled = !service.withdrawn, maxLines = 6, modifier = Modifier.weight(1f).testTag("services.composer"),
            placeholder = { Text(if (state.answering) stringResource(R.string.services_answer_hint) else stringResource(R.string.services_ask_hint_fmt, service.name)) })
        IconButton(enabled = !state.busy && !service.withdrawn && text.isNotBlank() && !tooLong, modifier = Modifier.testTag("services.send"), onClick = {
            val sent = text
            scope.launch { if (model.send(sent) && text == sent) text = "" }
        }) { Icon(Icons.AutoMirrored.Outlined.Send, stringResource(R.string.teams_message_send)) }
    }
    if (choosing) AlertDialog(
        onDismissRequest = { choosing = false }, title = { Text(stringResource(R.string.services_sessions)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                state.sessions.forEach { session ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).selectable(session.id == state.session, role = Role.RadioButton) { model.select(session.id); choosing = false }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = session.id == state.session, onClick = null); Text(session.title, Modifier.padding(start = 8.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { choosing = false }) { Text(stringResource(R.string.common_close)) } },
    )
    if (leaving) ConfirmDialog(
        title = stringResource(R.string.services_leave_title_fmt, service.name), body = stringResource(R.string.services_leave_body),
        confirm = stringResource(R.string.services_leave), destructive = true, confirmTag = "services.leave.confirm", dialogTag = "services.leave",
        onConfirm = { leaving = false; scope.launch { model.leave() } }, onDismiss = { leaving = false },
    )
    if (reporting) ServiceReport(model, state) { reporting = false }
}

/**
 * One message of a service's session, laid out as a Team's stream (docs/TEAMS-UI-POLISH.md U3): the person's own words read "You" on the faint accent
 * tint of the row, never a bubble, and the service's carry its name. The name, the Question tag and the text are one accessibility element.
 */
@Composable
private fun ServiceMessage(message: TeamMessage, service: HostedServiceView, uid: String) {
    val bot = message.authorKind != "user"
    Row(
        Modifier.fillMaxWidth().then(if (bot) Modifier else Modifier.background(MaterialTheme.colorScheme.primary.copy(alpha = OWN_MESSAGE_TINT)))
            .padding(horizontal = 16.dp, vertical = 6.dp).testTag("services.message.${message.id}"),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (bot) BotAvatar(service.sharedBotId, 32.dp) else PersonAvatar(uid, "", 32.dp)
        Column(Modifier.weight(1f).semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(if (bot) service.name else stringResource(R.string.teams_you), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (bot && message.waitId != null) Text(stringResource(R.string.services_question), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            when {
                message.recalled -> Text(stringResource(R.string.teams_message_recalled), fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // The Bot writes Markdown: the chat's renderer, web links only. What the person typed is shown as typed.
                TeamsBotText.rendersMarkdown(message.authorKind, message.recalled) -> AIText(markdown = message.text, modifier = Modifier.fillMaxWidth(), onOpenLink = rememberTeamBotLinkOpener())
                else -> Text(message.text, style = MaterialTheme.typography.bodyLarge)
            }
        }
    }
}

/**
 * A request the service is working on. The words are the state: the glyph beside them is decoration, and under Reduce motion the spinner is a static
 * glyph (U10). The buttons wrap onto a second line when the font scale leaves no room for them side by side.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ServiceTaskLine(task: TeamTask, state: HostedServicesState, model: HostedServicesModel) {
    val scope = rememberCoroutineScope()
    val reduceMotion = rememberReduceMotion()
    val words = HostedService.taskRes(task) ?: return
    val side = with(LocalDensity.current) { 14.sp.toDp() }
    Column(Modifier.fillMaxWidth().padding(start = 58.dp, end = 16.dp, top = 4.dp, bottom = 4.dp).testTag("services.task.${task.taskId}")) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            if (task.busy) {
                if (reduceMotion) Icon(Icons.Outlined.HourglassEmpty, contentDescription = null, modifier = Modifier.size(side), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                else CircularProgressIndicator(Modifier.size(side).clearAndSetSemantics { }, strokeWidth = 2.dp)
            }
            Text(stringResource(words), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (state.approval?.taskId == task.taskId) {
                Button(enabled = !state.busy, onClick = { scope.launch { model.decide(true) } }) { Text(stringResource(R.string.teams_approval_approve)) }
                OutlinedButton(enabled = !state.busy, onClick = { scope.launch { model.decide(false) } }) { Text(stringResource(R.string.teams_approval_decline)) }
            }
            if (!task.terminal) TextButton(enabled = !state.busy && !task.cancelling, onClick = { scope.launch { model.cancel(task.taskId) } }) { Text(stringResource(R.string.services_task_cancel)) }
        }
    }
}

/** A report goes to LingXi, not to the publisher. */
@Composable
private fun ServiceReport(model: HostedServicesModel, state: HostedServicesState, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var reason by rememberSaveable { mutableStateOf("harmful") }
    var text by rememberSaveable { mutableStateOf("") }
    var withSession by rememberSaveable { mutableStateOf(false) }
    var sent by rememberSaveable { mutableStateOf(false) }
    // One id per report, so a report sent again after a lost answer is the same report.
    val reportId = rememberSaveable { UUID.randomUUID().toString() }
    AlertDialog(
        onDismissRequest = onClose, modifier = Modifier.testTag("services.report"),
        title = { Text(stringResource(R.string.services_report_title)) },
        text = {
            if (sent) Text(stringResource(R.string.services_report_sent), Modifier.testTag("services.report.sent").semantics { liveRegion = LiveRegionMode.Polite })
            else Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(stringResource(R.string.services_report_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(stringResource(R.string.services_report_reason), style = MaterialTheme.typography.labelLarge)
                HostedService.REPORT_REASONS.forEach { value ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).selectable(reason == value, role = Role.RadioButton) { reason = value }.padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = reason == value, onClick = null); Text(stringResource(HostedService.reasonRes(value)), Modifier.padding(start = 8.dp))
                    }
                }
                OutlinedTextField(value = text, onValueChange = { if (it.length <= 1000) text = it }, label = { Text(stringResource(R.string.services_report_text)) }, minLines = 2, maxLines = 6, modifier = Modifier.fillMaxWidth())
                if (state.session != null) Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).toggleable(withSession, role = Role.Checkbox) { withSession = it }, verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = withSession, onCheckedChange = null); Text(stringResource(R.string.services_report_session), Modifier.padding(start = 8.dp))
                }
                state.error?.let { Text(stringResource(HostedService.errorRes(it)), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        dismissButton = { if (!sent) TextButton(onClick = onClose) { Text(stringResource(R.string.common_cancel)) } },
        confirmButton = {
            if (sent) TextButton(onClick = onClose) { Text(stringResource(R.string.common_close)) }
            else TextButton(enabled = !state.busy, modifier = Modifier.testTag("services.report.send"),
                onClick = { scope.launch { sent = model.report(reason, text, withSession, reportId) } }) { Text(stringResource(R.string.services_report_send)) }
        },
    )
}
