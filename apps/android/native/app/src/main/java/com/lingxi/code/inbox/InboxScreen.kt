package com.lingxi.code.inbox

import androidx.compose.foundation.clickable
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Badge
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lingxi.code.R
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@Composable
internal fun InboxScreen(
    controller: InboxController,
    onOpen: (InboxItem) -> Unit,
    onOpenProjects: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val state by controller.state.collectAsStateWithLifecycle()
    InboxScreenContent(state, controller, onOpen, onOpenProjects, modifier)
}

@Composable
internal fun InboxScreenContent(
    state: InboxState,
    controller: InboxController,
    onOpen: (InboxItem) -> Unit,
    onOpenProjects: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var filter by rememberSaveable(state.uid) { mutableStateOf(InboxFilter.All) }
    var search by rememberSaveable(state.uid) { mutableStateOf("") }
    var searchOpen by rememberSaveable(state.uid) { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    val searchFocus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val closeSearch = {
        keyboard?.hide()
        search = ""
        searchOpen = false
    }
    BackHandler(enabled = searchOpen, onBack = closeSearch)
    LaunchedEffect(searchOpen) {
        if (searchOpen) {
            searchFocus.requestFocus()
            keyboard?.show()
        }
    }
    val query = InboxQuery(filter, category = "messages", search = search)
    val listState = rememberLazyListState()
    LaunchedEffect(controller, filter, query.searchPrefix) {
        if (query.searchPrefix.isNotEmpty()) kotlinx.coroutines.delay(250)
        controller.show(query)
        listState.scrollToItem(0)
    }
    val rows = remember(state.rows, query) {
        val matching = state.rows.filter { it.matches(query) }
        if (query.searchPrefix.isEmpty()) matching else matching.sortedWith(compareBy<InboxItem> { inboxTitleSort(it.title) }.thenBy { it.key })
    }

    Surface(modifier.fillMaxSize().testTag("inbox.root")) {
        Column(Modifier.windowInsetsPadding(WindowInsets.statusBars)) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.session_details_messages), Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
                Box {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.inbox_options)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        for ((candidate, label) in listOf(
                            InboxFilter.All to R.string.inbox_all, InboxFilter.Unread to R.string.inbox_unread,
                            InboxFilter.Attention to R.string.inbox_attention,
                        )) {
                            DropdownMenuItem(
                                text = { Text(stringResource(label)) },
                                trailingIcon = if (filter == candidate) ({ Icon(Icons.Default.Check, null) }) else null,
                                onClick = { filter = candidate; menu = false },
                            )
                        }
                        HorizontalDivider()
                        DropdownMenuItem(text = { Text(stringResource(R.string.drawer_entry_projects)) }, onClick = {
                            menu = false
                            onOpenProjects()
                        })
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.inbox_mark_all_read)) },
                            enabled = state.uid != null && state.online && state.readBatchReady && !state.readBatchBusy,
                            onClick = { menu = false; controller.markAllRead() },
                        )
                        DropdownMenuItem(text = { Text(stringResource(R.string.inbox_mark_shown_read)) }, onClick = {
                            menu = false
                            val snapshot = rows.filter { it.unread }.map { it to InboxOperation.read(it) }
                            snapshot.forEach { (row, operation) -> controller.mutate(operation, row) }
                        })
                        DropdownMenuItem(text = { Text(stringResource(if (filter == InboxFilter.Archived) R.string.inbox_all else R.string.inbox_archived)) }, onClick = {
                            filter = if (filter == InboxFilter.Archived) InboxFilter.All else InboxFilter.Archived
                            menu = false
                        })
                        if (state.uid != null) {
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.inbox_sync_assistant)) },
                                trailingIcon = { Switch(state.syncAssistant, onCheckedChange = null) },
                                onClick = { controller.setAssistantSync(!state.syncAssistant) },
                            )
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.inbox_receipts)) },
                                trailingIcon = { Switch(state.dmReceipts, onCheckedChange = null) },
                                onClick = { controller.setReceipts(!state.dmReceipts) },
                            )
                        }
                    }
                }
                IconButton(onClick = { if (searchOpen) closeSearch() else searchOpen = true }) {
                    Icon(
                        if (searchOpen) Icons.Default.Close else Icons.Default.Search,
                        stringResource(if (searchOpen) R.string.common_close else R.string.inbox_search),
                    )
                }
            }
            if (searchOpen) {
                OutlinedTextField(
                    value = search, onValueChange = { search = it }, singleLine = true,
                    label = { Text(stringResource(R.string.inbox_search)) },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)
                        .focusRequester(searchFocus).testTag("inbox.search"),
                )
            }
            if (filter == InboxFilter.Archived) Text(stringResource(R.string.inbox_archived), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.labelLarge)
            if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (state.readBatchBusy) Text(
                stringResource(R.string.inbox_read_batch_running), Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                style = MaterialTheme.typography.labelMedium,
            )
            if (state.failed || state.writeError != null || state.rejected) {
                Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        stringResource(when {
                            state.writeError == "capacity" -> R.string.inbox_outbox_full
                            state.writeError == "batchMissing" -> R.string.inbox_read_batch_missing
                            state.writeError == "preferenceConflict" -> R.string.inbox_preference_conflict
                            state.writeError != null -> R.string.inbox_write_failed
                            state.rejected -> R.string.inbox_changes_rejected
                            state.cached -> R.string.inbox_offline_summary
                            else -> R.string.inbox_load_failed
                        }),
                        Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error,
                    )
                    TextButton(onClick = if (state.rejected) controller::clearRejected else controller::retry) {
                        Text(stringResource(if (state.rejected) R.string.inbox_dismiss else R.string.common_retry))
                    }
                }
            }
            val pinned = remember(state.rows) { state.rows.filter { it.category != "assistant" && !it.archived && it.pinOrder != null }.sortedWith(InboxItem.ordering) }
            val pinPositions = remember(pinned) { pinned.mapIndexed { index, row -> row.key to index }.toMap() }
            LazyColumn(state = listState, modifier = Modifier.fillMaxSize().testTag("inbox.rows")) {
                items(rows, key = { it.key }, contentType = { "conversation" }) { row ->
                    val pinIndex = pinPositions[row.key]
                    InboxRow(
                        row, onOpen = { onOpen(row) }, onMutation = { controller.mutate(it, row) },
                        onSummarySync = if (state.uid != null && row.conversationSummarySync != null) {
                            { controller.setConversationSummarySync(row, !row.conversationSummarySync) }
                        } else null,
                        onMoveUp = if (pinIndex != null && pinIndex > 0) ({ controller.movePin(row, -1) }) else null,
                        onMoveDown = if (pinIndex != null && pinIndex < pinned.lastIndex) ({ controller.movePin(row, 1) }) else null,
                    )
                    HorizontalDivider()
                }
                if (state.hasMore) {
                    item("load-more") {
                        TextButton(onClick = controller::nextPage, enabled = !state.loading, modifier = Modifier.fillMaxWidth()) {
                            Text(stringResource(R.string.inbox_more))
                        }
                    }
                } else if (rows.isEmpty() && !state.loading) {
                    item("empty") {
                        Text(stringResource(R.string.inbox_empty), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

@Composable
private fun InboxRow(
    row: InboxItem,
    onOpen: () -> Unit,
    onMutation: (InboxOperation) -> Unit,
    onSummarySync: (() -> Unit)?,
    onMoveUp: (() -> Unit)?,
    onMoveDown: (() -> Unit)?,
) {
    var menu by remember { mutableStateOf(false) }
    val fallback = stringResource(R.string.inbox_untitled)
    val source = stringResource(inboxCategoryRes(row.category))
    val stateLabel = when {
        row.offline -> stringResource(R.string.inbox_offline_summary)
        row.pendingActionCount > 0 -> stringResource(R.string.inbox_attention)
        row.attentionKind == "failure" -> stringResource(R.string.inbox_failure)
        row.notificationMode == "none" -> stringResource(R.string.inbox_muted)
        row.preview.isBlank() -> when (row.state) {
            "completed" -> stringResource(R.string.inbox_completed)
            else -> ""
        }
        else -> ""
    }
    val detail = listOf(row.context.ifBlank { source }, stateLabel, row.preview).filter { it.isNotBlank() }.joinToString(" · ")
    val time = remember(row.lastActivityAt) {
        row.lastActivityAt.takeIf { it > 0 }?.let {
            Instant.ofEpochMilli(it).atZone(ZoneId.systemDefault()).toLocalDate().format(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT))
        }.orEmpty()
    }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable(onClick = onOpen).padding(start = 16.dp, end = 4.dp)
            .testTag("inbox.row.${row.key}"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(row.title.ifBlank { fallback }, Modifier.weight(1f),
                    style = MaterialTheme.typography.bodyLarge, fontWeight = if (row.unread) FontWeight.SemiBold else FontWeight.Normal,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (time.isNotBlank()) Text(time, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (row.unread) {
            Badge(Modifier.padding(start = 8.dp)) {
                Text(if (row.unreadCount >= 100) "99+" else maxOf(row.unreadCount, 1).toString())
            }
        }
        Box {
            IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.inbox_conversation_options)) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                fun change(operation: InboxOperation) { menu = false; onMutation(operation) }
                if (row.unread || row.attention) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.inbox_mark_read)) }, onClick = { change(InboxOperation.read(row)) })
                }
                if (!row.unread) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.inbox_mark_unread)) }, onClick = { change(InboxOperation.unread(row)) })
                }
                DropdownMenuItem(text = { Text(stringResource(if (row.pinOrder == null) R.string.inbox_pin else R.string.inbox_unpin)) },
                    onClick = { change(InboxOperation.preferences(row, JSONObject().put("pinOrder", if (row.pinOrder == null) System.currentTimeMillis() else JSONObject.NULL))) })
                onMoveUp?.let { move -> DropdownMenuItem(text = { Text(stringResource(R.string.inbox_pin_move_up)) }, onClick = { menu = false; move() }) }
                onMoveDown?.let { move -> DropdownMenuItem(text = { Text(stringResource(R.string.inbox_pin_move_down)) }, onClick = { menu = false; move() }) }
                onSummarySync?.let { update ->
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.inbox_sync_conversation_summary)) },
                        trailingIcon = { Switch(row.conversationSummarySync == true, onCheckedChange = null) },
                        onClick = { menu = false; update() },
                    )
                }
                DropdownMenuItem(text = { Text(stringResource(if (row.notificationMode == "none") R.string.inbox_unmute else R.string.inbox_mute)) },
                    onClick = { change(InboxOperation.preferences(row, JSONObject().put("notificationMode", if (row.notificationMode == "none") "mentions" else "none"))) })
                DropdownMenuItem(text = { Text(stringResource(if (row.archived) R.string.inbox_unarchive else R.string.inbox_archive)) },
                    onClick = { change(InboxOperation.preferences(row, JSONObject().put("archived", !row.archived))) })
            }
        }
    }
}

private fun inboxCategoryRes(category: String): Int = when (category) {
    "assistant" -> R.string.inbox_assistant
    "friend" -> R.string.friends_title
    "team" -> R.string.teams_title
    else -> R.string.inbox_service
}

@Composable
internal fun InboxNoticeDialog(
    preview: InboxNoticePreview?,
    loading: Boolean,
    onDismiss: () -> Unit,
    onOpen: (InboxNoticePreview) -> Unit,
    onMarkRead: (InboxNoticePreview) -> Unit,
) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(preview?.row?.title?.takeIf(String::isNotBlank) ?: stringResource(R.string.inbox_notification_title)) },
        text = {
            when {
                loading -> LinearProgressIndicator(Modifier.fillMaxWidth())
                preview == null -> Text(stringResource(R.string.inbox_notice_unavailable))
                preview.messages.none(String::isNotBlank) -> Text(stringResource(R.string.inbox_notification_body))
                else -> Column(
                    Modifier.heightIn(max = 320.dp).verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    preview.messages.filter(String::isNotBlank).forEach { Text(it) }
                }
            }
        },
        confirmButton = {
            if (preview != null) TextButton(onClick = { onOpen(preview) }) { Text(stringResource(R.string.inbox_open_conversation)) }
            else TextButton(onClick = onDismiss) { Text(stringResource(R.string.inbox_dismiss)) }
        },
        dismissButton = {
            if (preview != null) TextButton(onClick = { onMarkRead(preview) }) { Text(stringResource(R.string.inbox_mark_read)) }
        },
    )
}
