package com.lingxi.code.teams

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import java.util.UUID

@Composable
internal fun TeamMessagesScreen(uid: String, detail: TeamDetail, onClose: () -> Unit) {
    val model: TeamMessagingModel = viewModel(key = "teams-messages:$uid:${detail.team.id}:${detail.team.generation}")
    val state by model.state.collectAsStateWithLifecycle()
    var text by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf("dm") }
    var parties by remember { mutableStateOf(setOf<String>()) }
    var creationId by remember { mutableStateOf(UUID.randomUUID().toString()) }
    var editing by remember { mutableStateOf<TeamMessage?>(null) }
    var reply by remember { mutableStateOf<TeamMessage?>(null) }
    var recall by remember { mutableStateOf<TeamMessage?>(null) }
    var creating by remember { mutableStateOf(false) }
    var shown by remember { mutableIntStateOf(100) }
    DisposableEffect(model, uid, detail.team.id, detail.team.generation) { model.start(uid, detail.team); onDispose { model.stop() } }
    LaunchedEffect(state.selected) { editing = null; reply = null; text = ""; shown = 100 }
    BackHandler(onBack = onClose)
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                TextButton(onClick = onClose) { Text(stringResource(R.string.teams_cancel)) }
                Text(stringResource(R.string.teams_conversations), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = model::refresh) { Text(stringResource(R.string.teams_refresh)) }
            }
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp), contentPadding = PaddingValues(bottom = 32.dp)) {
                item { Text(stringResource(R.string.teams_disclosure), style = MaterialTheme.typography.bodySmall); Text(stringResource(if (state.offline) R.string.teams_messages_offline else R.string.teams_messages_connected), style = MaterialTheme.typography.bodySmall) }
                if (state.error.isNotEmpty()) item { Text(stringResource(messageError(state.error)), color = MaterialTheme.colorScheme.error) }
                if (!state.revoked) {
                    state.cache?.creation?.let { creation -> item { Text(creation.getString("title")); Text(stringResource(if (creation.has("error")) messageError(creation.getString("error")) else if (creation.optBoolean("attempted")) R.string.teams_message_confirming else R.string.teams_message_pending)); TextButton(enabled = !(creation.optBoolean("attempted") && !creation.has("error")) && creation.optLong("leaseUntil") <= System.currentTimeMillis(), onClick = model::discardCreation) { Text(stringResource(R.string.teams_message_discard)) } } }
                    items(state.cache?.conversations ?: emptyList(), key = { "conversation-${it.id}" }) { conversation ->
                        OutlinedCard(onClick = { model.select(conversation.id) }, modifier = Modifier.fillMaxWidth()) {
                            Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.SpaceBetween) { Text(conversation.title, modifier = Modifier.weight(1f)); Text(conversation.unread.toString()); if (conversation.id == state.selected) Text(" ✓") }
                        }
                    }
                    val selected = state.cache?.conversations?.find { it.id == state.selected }
                    if (selected != null) {
                        val messages = state.cache?.threads?.get(selected.id)?.messages ?: emptyList()
                        item { HorizontalDivider(); Text(selected.title, style = MaterialTheme.typography.headlineSmall); TextButton(onClick = model::markRead, enabled = !state.offline) { Text(stringResource(R.string.teams_mark_read)) } }
                        if (messages.size > shown) item { TextButton(onClick = { shown += 100 }) { Text(stringResource(R.string.teams_messages_earlier)) } }
                        items(messages.takeLast(shown), key = { "message-${it.id}" }) { message ->
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(detail.members.find { it.uid == message.authorUid }?.displayName ?: stringResource(R.string.teams_member), style = MaterialTheme.typography.labelMedium)
                                message.replyTo?.let { id -> val source = messages.find { it.id == id }; Text(if (source?.recalled == true) stringResource(R.string.teams_message_recalled) else source?.text ?: stringResource(R.string.teams_message_reply), style = MaterialTheme.typography.bodySmall, maxLines = 2) }
                                Text(if (message.recalled) stringResource(R.string.teams_message_recalled) else message.text)
                                if (message.editedAt != null) Text(stringResource(R.string.teams_message_edited), style = MaterialTheme.typography.labelSmall)
                                if (!message.recalled) Row {
                                    TextButton(onClick = { reply = message; editing = null }) { Text(stringResource(R.string.teams_message_reply)) }
                                    if (message.authorUid == uid && message.authorKind == "user") {
                                        TextButton(onClick = { editing = message; reply = null; text = message.text }) { Text(stringResource(R.string.teams_message_edit)) }
                                        TextButton(onClick = { recall = message }) { Text(stringResource(R.string.teams_message_recall), color = MaterialTheme.colorScheme.error) }
                                    }
                                }
                                HorizontalDivider()
                            }
                        }
                        items((state.cache?.outbox ?: emptyList()).filter { it.getString("conversationId") == selected.id }, key = { "pending-${it.getString("changeId")}" }) { intent ->
                            Column { Text(stringResource(if (intent.has("error")) messageError(intent.getString("error")) else if (intent.optBoolean("attempted")) R.string.teams_message_confirming else R.string.teams_message_pending), style = MaterialTheme.typography.labelSmall); Text(if (intent.getString("operation") == "recall") stringResource(R.string.teams_message_recall) else intent.getString("text")); TextButton(enabled = !(intent.optBoolean("attempted") && !intent.has("error")) && intent.optLong("leaseUntil") <= System.currentTimeMillis(), onClick = { model.discard(intent.getString("changeId")) }) { Text(stringResource(R.string.teams_message_discard)) } }
                        }
                        item {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                if (editing != null || reply != null) Row { Text(stringResource(if (editing != null) R.string.teams_message_edit else R.string.teams_message_reply)); TextButton(onClick = { editing = null; reply = null; text = "" }) { Text(stringResource(R.string.teams_cancel)) } }
                                OutlinedTextField(value = text, onValueChange = { text = it }, label = { Text(stringResource(R.string.teams_message_input)) }, enabled = !state.submitting, minLines = 3, maxLines = 8, modifier = Modifier.fillMaxWidth())
                                Button(enabled = !state.submitting && text.isNotBlank() && text.toByteArray(Charsets.UTF_8).size <= 8192, onClick = { val submitted = text; val target = editing?.id; val replyId = reply?.id; model.queue(if (target == null) "create" else "edit", submitted, target, replyId) { if (text == submitted) text = ""; if (editing?.id == target) editing = null; if (reply?.id == replyId) reply = null } }) { Text(stringResource(if (editing == null) R.string.teams_message_send else R.string.teams_message_save)) }
                            }
                        }
                    }
                    item {
                        TextButton(onClick = { creating = !creating }) { Text(stringResource(R.string.teams_conversation_new)) }
                        if (creating) Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedTextField(value = title, onValueChange = { if (it.length <= 120) title = it }, label = { Text(stringResource(R.string.teams_conversation_title)) }, modifier = Modifier.fillMaxWidth())
                            Row { listOf("dm" to R.string.teams_conversation_dm, "group" to R.string.teams_conversation_group, "channel" to R.string.teams_conversation_channel).forEach { (value, label) -> FilterChip(selected = kind == value, onClick = { kind = value; parties = emptySet() }, label = { Text(stringResource(label)) }) } }
                            detail.members.filter { it.uid != uid }.forEach { member -> Row { Checkbox(checked = member.uid in parties, onCheckedChange = { checked -> parties = if (kind == "dm") if (checked) setOf(member.uid) else emptySet() else if (checked) parties + member.uid else parties - member.uid }); Text(member.displayName) } }
                            Button(enabled = !state.offline && title.isNotBlank() && parties.isNotEmpty() && (kind != "dm" || parties.size == 1), onClick = { model.create(kind, title.trim(), parties.sorted(), creationId) { creationId = UUID.randomUUID().toString(); title = ""; parties = emptySet(); creating = false } }) { Text(stringResource(R.string.teams_conversation_create)) }
                        }
                    }
                }
            }
        }
        recall?.let { value -> AlertDialog(onDismissRequest = { recall = null }, title = { Text(stringResource(R.string.teams_recall_confirm)) }, confirmButton = { TextButton(onClick = { model.queue("recall", "", value.id); recall = null }) { Text(stringResource(R.string.teams_message_recall)) } }, dismissButton = { TextButton(onClick = { recall = null }) { Text(stringResource(R.string.teams_cancel)) } }) }
    }
}

private fun messageError(code: String): Int = when {
    code in setOf("MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED") -> R.string.teams_stale_error
    code.contains("STORAGE") -> R.string.teams_messages_storage_error
    code in setOf("MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "INVALID_INPUT") -> R.string.teams_message_failed
    else -> R.string.teams_error
}
