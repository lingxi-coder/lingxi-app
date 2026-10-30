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
import kotlinx.coroutines.delay

@Composable
internal fun TeamMessagesScreen(uid: String, detail: TeamDetail, onClose: () -> Unit, model: TeamMessagingModel = viewModel(key = "teams-messages:$uid:${detail.team.id}:${detail.team.generation}")) {
    val state by model.state.collectAsStateWithLifecycle()
    var text by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    var kind by remember { mutableStateOf("dm") }
    var parties by remember { mutableStateOf(setOf<String>()) }
    var botParties by remember { mutableStateOf(setOf<String>()) }
    var targets by remember { mutableStateOf(setOf<String>()) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) { while (true) { now = System.currentTimeMillis(); delay(1000) } }
    var creationId by remember { mutableStateOf(UUID.randomUUID().toString()) }
    var editing by remember { mutableStateOf<TeamMessage?>(null) }
    var reply by remember { mutableStateOf<TeamMessage?>(null) }
    var recall by remember { mutableStateOf<TeamMessage?>(null) }
    var creating by remember { mutableStateOf(false) }
    var shown by remember { mutableIntStateOf(100) }
    DisposableEffect(model, uid, detail.team.id, detail.team.generation) { model.start(uid, detail.team); onDispose { model.stop() } }
    LaunchedEffect(state.selected) { editing = null; reply = null; text = ""; shown = 100; targets = emptySet() }
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
                    item { Text("Team Bots", style = MaterialTheme.typography.titleMedium); Text("Bind and configure a Bot on Desktop. Tool approvals and execution remain with its owner on Desktop.", style = MaterialTheme.typography.bodySmall) }
                    items(state.bots, key = { "bot-${it.key}" }) { bot ->
                        TeamBotCardView(bot, detail.members.find { it.uid == bot.ownerUid }?.displayName ?: bot.ownerUid, uid, !state.offline,
                            onMessage = { model.create("bot_dm", bot.name, emptyList(), UUID.randomUUID().toString(), listOf(bot.key)) {} }, onUnbind = { model.unbind(bot) })
                    }
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
                        if (messages.size > shown) item { TextButton(onClick = { shown += 100; model.showBotHistory(shown) }) { Text(stringResource(R.string.teams_messages_earlier)) } }
                        items(messages.takeLast(shown), key = { "message-${it.id}" }) { message ->
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(if (message.authorKind == "bot") state.bots.find { it.key == message.botKey }?.name ?: "Bot" else detail.members.find { it.uid == message.authorUid }?.displayName ?: stringResource(R.string.teams_member), style = MaterialTheme.typography.labelMedium)
                                message.replyTo?.let { id -> val source = messages.find { it.id == id }; Text(if (source?.recalled == true) stringResource(R.string.teams_message_recalled) else source?.text ?: stringResource(R.string.teams_message_reply), style = MaterialTheme.typography.bodySmall, maxLines = 2) }
                                Text(if (message.recalled) stringResource(R.string.teams_message_recalled) else message.text)
                                if (message.question != null && !message.recalled) TeamBotQuestionView(message, state.botRuns[message.triggerId], uid, detail.team.generation, now, state.offline, state.cache?.answers?.any { it.getString("conversationId") == selected.id && it.getString("questionId") == message.question.id && !it.has("error") } == true) { model.answer(message, it) }
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
                        items(state.botRuns.entries.toList(), key = { "run-${it.key}" }) { (trigger, run) ->
                            Column { Text("${state.bots.find { it.key == run.botKey }?.name ?: "Bot"}: ${teamBotStatus(run.state)}", style = MaterialTheme.typography.bodySmall)
                                Text("Requested by ${detail.members.find { it.uid == run.originUid }?.displayName ?: run.originUid}", style = MaterialTheme.typography.labelSmall)
                                messages.find { it.authorKind == "user" && trigger in teamBotTriggers(it, selected) }?.let { Text(it.text, maxLines = 2, style = MaterialTheme.typography.bodySmall) }
                                if (state.cache?.settlements?.none { it.getJSONObject("body").getString("conversationId") == selected.id && it.getJSONObject("body").getString("triggerId") == trigger } != false) TeamBotRecoveryView(trigger, run, uid, now, !state.offline) { model.recover(trigger, it) } }
                        }
                        items((state.cache?.settlements ?: emptyList()).filter { it.getJSONObject("body").getString("conversationId") == selected.id }, key = { "settlement-${it.getString("settlementId")}" }) { intent ->
                            Column {
                                Text(if (intent.has("error")) "Settlement could not be delivered: ${intent.getString("error")}" else if (intent.optBoolean("attempted")) "Settlement saved; confirming delivery." else "Settlement saved; awaiting delivery.", style = MaterialTheme.typography.bodySmall)
                                Text(intent.getJSONObject("body").getString("text"))
                                TextButton(enabled = canDiscardTeamBotIntent(intent, now), onClick = { model.discardSettlement(intent.getString("settlementId")) }) { Text(stringResource(R.string.teams_message_discard)) }
                            }
                        }
                        items((state.cache?.answers ?: emptyList()).filter { it.getString("conversationId") == selected.id }, key = { "answer-${it.getString("answerId")}" }) { intent ->
                            Column { Text(if (intent.has("error")) "Answer could not be delivered: ${intent.getString("error")}" else "Answer saved; awaiting confirmation.", style = MaterialTheme.typography.bodySmall); if (intent.has("error")) TextButton(onClick = { model.discardAnswer(intent.getString("answerId")) }) { Text("Dismiss") } }
                        }
                        items((state.cache?.outbox ?: emptyList()).filter { it.getString("conversationId") == selected.id }, key = { "pending-${it.getString("changeId")}" }) { intent ->
                            Column { Text(stringResource(if (intent.has("error")) messageError(intent.getString("error")) else if (intent.optBoolean("attempted")) R.string.teams_message_confirming else R.string.teams_message_pending), style = MaterialTheme.typography.labelSmall); Text(if (intent.getString("operation") == "recall") stringResource(R.string.teams_message_recall) else intent.getString("text")); TextButton(enabled = !(intent.optBoolean("attempted") && !intent.has("error")) && intent.optLong("leaseUntil") <= System.currentTimeMillis(), onClick = { model.discard(intent.getString("changeId")) }) { Text(stringResource(R.string.teams_message_discard)) } }
                        }
                        item {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                if (editing != null || reply != null) Row { Text(stringResource(if (editing != null) R.string.teams_message_edit else R.string.teams_message_reply)); TextButton(onClick = { editing = null; reply = null; text = "" }) { Text(stringResource(R.string.teams_cancel)) } }
                                OutlinedTextField(value = text, onValueChange = { text = it }, label = { Text(stringResource(R.string.teams_message_input)) }, enabled = !state.submitting, minLines = 3, maxLines = 8, modifier = Modifier.fillMaxWidth())
                                val staleTargets = staleTeamBotSelections(if (selected.kind == "bot_dm") selected.botKeys else targets, state.bots, selected.botKeys)
                                val staleBotDm = selected.kind == "bot_dm" && selected.botKeys.size != 1
                                if (editing == null && (staleTargets.isNotEmpty() || staleBotDm)) {
                                    Text("A selected Bot is unavailable or no longer a participant. Your message has not been sent.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                                    if (selected.kind != "bot_dm") TextButton(onClick = { targets = targets - staleTargets }) { Text("Clear unavailable Bot targets") }
                                }
                                if (editing == null && selected.kind in setOf("group", "channel")) {
                                    Text("Explicit Bot targets (typing @name does not run a Bot)", style = MaterialTheme.typography.bodySmall)
                                    state.bots.filter { it.selectable && it.key in selected.botKeys }.forEach { bot -> Row { Checkbox(checked = bot.key in targets, onCheckedChange = { checked -> targets = if (checked) targets + bot.key else targets - bot.key }); Text(bot.name) } }
                                }
                                Button(enabled = !state.submitting && (editing != null || (staleTargets.isEmpty() && !staleBotDm)) && text.isNotBlank() && text.toByteArray(Charsets.UTF_8).size <= 8192, onClick = { val submitted = text; val target = editing?.id; val replyId = reply?.id; model.queue(if (target == null) "create" else "edit", submitted, target, replyId, if (target == null) targets.sorted() else emptyList()) { if (text == submitted) text = ""; if (editing?.id == target) editing = null; if (reply?.id == replyId) reply = null } }) { Text(stringResource(if (editing == null) R.string.teams_message_send else R.string.teams_message_save)) }
                            }
                        }
                    }
                    item {
                        TextButton(onClick = { creating = !creating }) { Text(stringResource(R.string.teams_conversation_new)) }
                        if (creating) Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            val staleBotParties = staleTeamBotSelections(botParties, state.bots)
                            OutlinedTextField(value = title, onValueChange = { if (it.length <= 120) title = it }, label = { Text(stringResource(R.string.teams_conversation_title)) }, modifier = Modifier.fillMaxWidth())
                            Row { listOf("dm" to R.string.teams_conversation_dm, "group" to R.string.teams_conversation_group, "channel" to R.string.teams_conversation_channel).forEach { (value, label) -> FilterChip(selected = kind == value, onClick = { kind = value; parties = emptySet(); botParties = emptySet() }, label = { Text(stringResource(label)) }) } }
                            detail.members.filter { it.uid != uid }.forEach { member -> Row { Checkbox(checked = member.uid in parties, onCheckedChange = { checked -> parties = if (kind == "dm") if (checked) setOf(member.uid) else emptySet() else if (checked) parties + member.uid else parties - member.uid }); Text(member.displayName) } }
                            if (kind in setOf("group", "channel")) {
                                Text("Bot participants (up to 8)", style = MaterialTheme.typography.bodySmall)
                                state.bots.filter { it.selectable }.forEach { bot -> Row { Checkbox(checked = bot.key in botParties, enabled = bot.key in botParties || botParties.size < 8, onCheckedChange = { checked -> botParties = if (checked) botParties + bot.key else botParties - bot.key }); Text(bot.name) } }
                            }
                            if (staleBotParties.isNotEmpty()) {
                                Text("A selected Bot participant is unavailable. Your conversation has not been created.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                                TextButton(onClick = { botParties = botParties - staleBotParties }) { Text("Clear unavailable Bot participants") }
                            }
                            Button(enabled = !state.offline && staleBotParties.isEmpty() && title.isNotBlank() && (parties.isNotEmpty() || botParties.isNotEmpty()) && (kind != "dm" || parties.size == 1), onClick = { model.create(kind, title.trim(), parties.sorted(), creationId, botParties.sorted()) { creationId = UUID.randomUUID().toString(); title = ""; parties = emptySet(); botParties = emptySet(); creating = false } }) { Text(stringResource(R.string.teams_conversation_create)) }
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
    code in setOf("MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "BOT_SELECTION_STALE", "INVALID_INPUT") -> R.string.teams_message_failed
    else -> R.string.teams_error
}
