package com.lingxi.code.teams

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Send
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.AssistChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.InputChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.BotAvatar

/**
 * The composer of a conversation (SPEC-mobile-team-bot-ui section 6.3): a growing field, reply and edit bars, the `@` picker and
 * Bot chips for groups and channels, the lock explanations and the footnotes. The unsent text lives in the workspace's in-memory
 * draft of the conversation; [onSend] hands the text over and the caller clears the draft once the command is journaled.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TeamComposer(
    conversation: TeamMessageConversation, label: String, uid: String, messaging: TeamMessagingState, draft: ComposerDraft,
    replyTo: TeamMessage?, editing: TeamMessage?, replyAuthor: String,
    onDraft: (ComposerDraft) -> Unit, onCancelReply: () -> Unit, onCancelEdit: () -> Unit,
    onSend: (text: String, targets: List<String>, done: () -> Unit) -> Unit, modifier: Modifier = Modifier,
) {
    val bots = messaging.bots
    val lock = TeamsComposeLogic.composerLock(uid, conversation.kind, conversation.participants, bots, messaging.botsLoaded, messaging.botsFailed)
    if (lock != ComposerLock.None) {
        Surface(modifier.fillMaxWidth().testTag(TeamsTags.COMPOSER_LOCK), color = MaterialTheme.colorScheme.surfaceVariant) {
            Text(
                stringResource(when (lock) {
                    ComposerLock.OwnerReading -> R.string.teams_composer_lock_owner_reading
                    ComposerLock.BotUnverified -> R.string.teams_composer_lock_bot_unverified
                    ComposerLock.BotChecking -> R.string.teams_composer_lock_bot_checking
                    else -> R.string.teams_composer_lock_bot_unavailable
                }),
                Modifier.padding(16.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        return
    }
    var field by remember(conversation.id, editing?.id) {
        val start = if (editing != null) editing.text else draft.text
        mutableStateOf(TextFieldValue(start, TextRange(start.length)))
    }
    val askable = conversation.kind == "group" || conversation.kind == "channel"
    val mention = if (askable && editing == null) TeamsComposeLogic.mentionAt(field.text, field.selection.end) else null
    val candidates = if (mention != null && draft.targets.size < TeamsComposeLogic.MAX_TARGETS) TeamsComposeLogic.mentionCandidates(bots, conversation, draft.targets, mention.query) else emptyList()
    val staleTargets = if (editing == null && askable) composerStaleTargets(draft.targets, bots, conversation) else emptySet()
    val staleBotDm = conversation.kind == "bot_dm" && conversation.sharedBotIds.size != 1
    val tooLong = TeamsComposeLogic.byteLength(field.text) > TeamsComposeLogic.MESSAGE_BYTES
    val canSend = TeamsComposeLogic.canSendText(field.text) && !messaging.submitting && (editing != null || (staleTargets.isEmpty() && !staleBotDm))
    val hasAskableBots = askable && bots.any { it.selectable && "shared_bot:${it.sharedBotId}" in conversation.participants }
    fun update(value: TextFieldValue, targets: Set<String> = draft.targets) {
        field = value
        // While editing, the field holds the edited message; the unsent draft keeps what was typed before.
        if (editing == null) onDraft(draft.copy(text = value.text, targets = targets, replyToId = replyTo?.id))
    }

    Column(modifier.fillMaxWidth()) {
        HorizontalDivider()
        if (candidates.isNotEmpty()) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
                Text(stringResource(R.string.teams_composer_bots_can_ask), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                candidates.forEach { bot ->
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            val at = requireNotNull(mention)
                            val cleaned = TeamsComposeLogic.withoutMention(field.text, at.start, field.selection.end)
                            update(TextFieldValue(cleaned, TextRange(at.start.coerceAtMost(cleaned.length))), draft.targets + bot.sharedBotId)
                        }.padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        BotAvatar(bot.avatarId, 24.dp)
                        Text(bot.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
            HorizontalDivider()
        }
        if (editing != null || replyTo != null) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
                    Text(
                        if (editing != null) stringResource(R.string.teams_editing_message) else stringResource(R.string.teams_replying_to_fmt, replyAuthor),
                        style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary,
                    )
                    val quoted = editing ?: replyTo!!
                    Text(if (quoted.recalled) stringResource(R.string.teams_message_recalled) else TeamsComposeLogic.snippet(quoted.text), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                IconButton(onClick = { if (editing != null) onCancelEdit() else onCancelReply() }) { Icon(Icons.Outlined.Close, stringResource(R.string.teams_cancel_reply)) }
            }
        }
        if (draft.targets.isNotEmpty() && editing == null) {
            Text(stringResource(R.string.teams_composer_bots_asked), Modifier.padding(start = 16.dp, top = 6.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                draft.targets.sorted().forEach { key ->
                    val bot = bots.firstOrNull { it.sharedBotId == key }
                    val name = bot?.name ?: "Bot"
                    val remove = stringResource(R.string.teams_composer_remove_bot_fmt, name)
                    InputChip(
                        selected = false, onClick = { update(field, draft.targets - key) },
                        label = { Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        leadingIcon = { BotAvatar(bot?.avatarId ?: key, 20.dp) },
                        trailingIcon = { Icon(Icons.Outlined.Close, null, Modifier.size(16.dp)) },
                        modifier = Modifier.semantics { contentDescription = remove },
                    )
                }
            }
        }
        if (staleTargets.isNotEmpty() || staleBotDm) Text(stringResource(R.string.teams_error_bot_access_changed), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.Bottom) {
            OutlinedTextField(
                value = field, onValueChange = { update(it) }, minLines = 1, maxLines = 6, isError = tooLong, enabled = !messaging.submitting,
                placeholder = {
                    Text(
                        if (editing != null) stringResource(R.string.teams_composer_edit_hint)
                        else stringResource(R.string.teams_composer_hint_fmt, if (conversation.kind == "channel") "#$label" else label),
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                },
                modifier = Modifier.weight(1f).testTag(TeamsTags.COMPOSER_FIELD),
            )
            IconButton(
                enabled = canSend, modifier = Modifier.testTag(TeamsTags.SEND),
                onClick = { val submitted = field.text; onSend(submitted, if (editing == null && askable) draft.targets.sorted() else emptyList()) { if (field.text == submitted) field = TextFieldValue("") } },
            ) { Icon(Icons.AutoMirrored.Outlined.Send, stringResource(if (editing != null) R.string.teams_message_save else R.string.teams_message_send)) }
        }
        if (tooLong) Text(stringResource(R.string.teams_composer_too_long), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        if (hasAskableBots && editing == null) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                AssistChip(
                    onClick = {
                        val caret = field.selection.end
                        val before = field.text.substring(0, caret)
                        val text = before + (if (before.isEmpty() || before.last().isWhitespace()) "@" else " @") + field.text.substring(caret)
                        update(TextFieldValue(text, TextRange(before.length + (text.length - field.text.length))))
                    },
                    label = { Text(stringResource(R.string.teams_composer_ask_bot)) }, modifier = Modifier.testTag(TeamsTags.ASK_BOT),
                )
            }
        }
        val note = when {
            messaging.offline -> R.string.teams_composer_note_offline
            conversation.kind == "bot_dm" -> R.string.teams_composer_note_bot_dm
            hasAskableBots -> R.string.teams_composer_note_group
            else -> null
        }
        if (note != null) Text(stringResource(note), Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
