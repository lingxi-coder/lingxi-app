package com.lingxi.code.teams

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.InputChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.conversation.AudioSetupBanner
import com.lingxi.code.conversation.ComposerInputSurface
import com.lingxi.code.conversation.ComposerSendButton
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.PersonAvatar
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.channels.Channel

private data class TeamEditorReplacement(val original: TextRange, val insertedLength: Int)
private data class TeamEditorEdit(val text: String, val replacements: List<TeamEditorReplacement>)

/**
 * The composer of a conversation (SPEC-mobile-team-bot-ui section 6.3): a growing field, reply and edit bars, the `@` picker and
 * Bot chips for groups and channels, the lock explanations and the footnotes. The unsent text lives in the workspace's in-memory
 * draft of the conversation; [onSend] hands the text over and the caller clears the draft once the command is journaled.
 */
@OptIn(ExperimentalLayoutApi::class, ExperimentalFoundationApi::class)
@Composable
internal fun TeamComposer(
    conversation: TeamMessageConversation, label: String, uid: String, messaging: TeamMessagingState, draft: ComposerDraft,
    replyTo: TeamMessage?, editing: TeamMessage?, replyAuthor: String,
    onDraft: (ComposerDraft) -> Unit, onCancelReply: () -> Unit, onCancelEdit: () -> Unit,
    onSend: (text: String, targets: List<String>, mentions: List<TeamMentionEntity>, formats: List<TeamMessageFormat>, done: () -> Unit) -> Unit, modifier: Modifier = Modifier,
    members: List<TeamMember> = emptyList(),
    /** The files of the unsent message and the chosen ones that were not taken. Empty while a message is edited or a Bot's question is answered: neither carries files, and the files wait in the draft. */
    files: List<TeamDraftFile> = emptyList(), refusals: List<TeamFileRefusal> = emptyList(),
    /** Null where files cannot be added now. */
    onChooseFiles: ((List<TeamChosenFile>) -> Unit)? = null, onRemoveFile: (String) -> Unit = {}, onDismissRefusals: () -> Unit = {},
    /** What the unsent message belongs to (the workspace's draft key). Words spoken into the box are for it alone. Null: no microphone here. */
    voiceTarget: String? = null,
    readDraft: () -> ComposerDraft = { draft },
) {
    val bots = messaging.bots
    val lock = TeamsComposeLogic.composerLock(uid, conversation.kind, conversation.participants, bots, messaging.botsLoaded, messaging.botsFailed, conversation.readers)
    if (lock != ComposerLock.None) {
        Surface(modifier.fillMaxWidth().testTag(TeamsTags.COMPOSER_LOCK), color = MaterialTheme.colorScheme.surfaceVariant) {
            Text(
                stringResource(when (lock) {
                    ComposerLock.OwnerReading -> R.string.teams_composer_lock_owner_reading
                    ComposerLock.BotHandedOver -> R.string.teams_composer_lock_bot_handed_over
                    ComposerLock.BotUnverified -> R.string.teams_composer_lock_bot_unverified
                    ComposerLock.BotChecking -> R.string.teams_composer_lock_bot_checking
                    else -> R.string.teams_composer_lock_bot_unavailable
                }),
                Modifier.padding(16.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        return
    }
    val field = remember(conversation.id, editing?.id) {
        TextFieldState(editing?.text ?: draft.text)
    }
    val text = field.text.toString()
    var entities by remember(field) {
        mutableStateOf(if (editing != null) editing.mentionEntities else draft.mentionEntities)
    }
    var formats by remember(field) { mutableStateOf(editing?.formats ?: draft.formats) }
    var typingStyle by remember(field) { mutableStateOf<Pair<Int, Int>?>(null) }
    var trackedText by remember(field) { mutableStateOf(text) }
    val editEvents = remember(field) { Channel<TeamEditorEdit>(Channel.UNLIMITED) }
    val persistDraft by rememberUpdatedState<(String, Set<String>, List<TeamMentionEntity>, List<TeamMessageFormat>) -> Unit> { value, targets, mentions, styles ->
        if (editing == null) onDraft(readDraft().copy(text = value, targets = targets, mentionEntities = mentions, formats = styles))
    }
    val readLatestDraft by rememberUpdatedState(readDraft)

    fun applyEdit(edit: TeamEditorEdit) {
        val shifted = edit.replacements.fold(entities) { remaining, (range, inserted) ->
            TeamMentionEntity.replacing(remaining, range.min, range.max, inserted)
        }
        val pending = typingStyle
        var nextTyping: Pair<Int, Int>? = null
        for ((range, inserted) in edit.replacements) {
            val explicit = pending?.takeIf { it.first == range.min && range.collapsed }?.second
            formats = TeamMessageFormat.replace(formats, range.min, range.max, inserted,
                explicit ?: TeamMessageFormat.styleAt(formats, if (range.collapsed) range.min else range.min + 1))
            if (explicit != null) nextTyping = (range.min + inserted) to explicit
        }
        typingStyle = nextTyping
        entities = shifted
        trackedText = edit.text
        persistDraft(edit.text, readLatestDraft().targets, shifted, formats)
    }

    fun finishUserEdits() {
        while (true) applyEdit(editEvents.tryReceive().getOrNull() ?: break)
        // Some framework actions, such as undo, can update the state outside the input transformation.
        val latest = field.text.toString()
        if (trackedText != latest) {
            entities = TeamMentionEntity.shifted(entities, trackedText, latest, caretAfter = field.selection.end)
            formats = TeamMessageFormat.shift(formats, trackedText, latest)
            typingStyle = null
            trackedText = latest
            persistDraft(latest, readLatestDraft().targets, entities, formats)
        }
    }

    LaunchedEffect(field) {
        for (edit in editEvents) applyEdit(edit)
    }
    LaunchedEffect(field) {
        snapshotFlow { field.text.toString() }.collect { finishUserEdits() }
    }
    LaunchedEffect(draft.text, draft.mentionEntities, draft.formats, field) {
        if (editing == null) {
            finishUserEdits()
            val latest = readDraft()
            if (field.text.toString() != latest.text) field.edit {
                replace(0, length, latest.text)
                selection = TextRange(length)
            }
            entities = latest.mentionEntities
            formats = latest.formats
            trackedText = latest.text
        }
    }
    val askable = conversation.kind == "group" || conversation.kind == "channel"
    val viewer = messaging.cache?.let { TeamMemberRef(uid, it.generation) }
    val answeringWait = TeamTaskActions.answerableWait(replyTo, messaging.tasks, viewer, TeamTaskActions.pendingWaitIds(messaging.cache?.outbox.orEmpty())) != null
    val canMention = askable && !answeringWait
    val roster = remember(members, conversation.participants, uid, askable) {
        val participants = conversation.participants.toSet()
        if (askable) members.mapNotNull { member ->
            if (member.uid == uid || "user:${member.uid}" !in participants ||
                member.displayName.any { it == '\n' || it == '\r' }) return@mapNotNull null
            val key = TeamMentionEntity.searchKey(member.displayName)
            if (key.isBlank()) null else member to key
        } else emptyList()
    }
    val nameCounts = remember(roster) { roster.groupingBy { it.second }.eachCount() }
    val mention = if (canMention && !messaging.submitting && field.selection.collapsed) TeamMentionEntity.at(text, field.selection.end, entities) else null
    val people = if (mention != null && entities.size < TeamMentionEntity.LIMIT) {
        val needle = TeamMentionEntity.searchKey(mention.query)
        roster.filter { (_, key) -> key.contains(needle) }
    } else emptyList()
    val candidates = if (mention != null && editing == null && draft.targets.size < TeamsComposeLogic.MAX_TARGETS)
        TeamsComposeLogic.mentionCandidates(bots, conversation, draft.targets, mention.query) else emptyList()
    val staleTargets = if (editing == null && askable && !answeringWait) composerStaleTargets(draft.targets, bots, conversation) else emptySet()
    val staleBotDm = conversation.kind == "bot_dm" && conversation.sharedBotIds.size != 1
    val tooLong = TeamsComposeLogic.byteLength(text) > TeamsComposeLogic.MESSAGE_BYTES
    val requestTargets = if (conversation.kind == "bot_dm") conversation.sharedBotIds else draft.targets
    // A group or channel takes a message for a Bot whose Desktop is off, and its task waits for the Desktop (rule 6); a Bot DM says no at once.
    val waitsForHost = conversation.kind != "bot_dm"
    val blockedBots = TeamBotEntry.blockedRequests(requestTargets, bots, editing != null || answeringWait, waitsForHost)
    val validFormats = TeamMessageFormat.valid(formats, text)
    val canSend = validFormats && TeamAttachments.canSend(text, files) && !messaging.submitting && blockedBots.isEmpty() && (editing != null || (staleTargets.isEmpty() && !staleBotDm))
    val hasAskableBots = askable && bots.any { it.selectable && "shared_bot:${it.sharedBotId}" in conversation.participants }
    fun update(value: String, caret: TextRange = TextRange(value.length), targets: Set<String> = draft.targets,
               mentions: List<TeamMentionEntity> = entities, replaced: TextRange? = null) {
        if (replaced != null || trackedText != value) {
            formats = if (replaced != null) TeamMessageFormat.replace(formats, replaced.min, replaced.max,
                value.length - trackedText.length + replaced.length)
            else TeamMessageFormat.shift(formats, trackedText, value, typing = 0)
            typingStyle = null
        }
        field.edit {
            if (asCharSequence().toString() != value) replace(0, length, value)
            selection = caret
        }
        entities = mentions
        trackedText = value
        persistDraft(value, targets, mentions, formats)
    }

    // Spoken words are added to the box, to be read, changed and sent by hand. A message being changed is another box.
    val voice = if (voiceTarget == null) null else rememberTeamVoice("$voiceTarget:${editing?.id.orEmpty()}") { heard ->
        finishUserEdits()
        update(TeamsComposeLogic.appendingTranscript(heard, field.text.toString()))
    }

    Column(modifier.fillMaxWidth()) {
        HorizontalDivider()
        val openVoiceSettings = LocalOpenVoiceSettings.current
        voice?.guidance?.let { AudioSetupBanner(it, onConfigure = openVoiceSettings, onDismiss = voice.dismiss) }
        if (voice?.listening == true) TeamVoiceListening()
        if (people.isNotEmpty() || candidates.isNotEmpty()) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
                Text(stringResource(R.string.teams_composer_mention_options), Modifier.semantics { heading() }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                LazyColumn(Modifier.heightIn(max = 192.dp)) {
                    items(people, key = { "user:${it.first.uid}" }) { (member, key) ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) {
                                finishUserEdits()
                                val currentText = field.text.toString()
                                val at = TeamMentionEntity.at(currentText, field.selection.end, entities) ?: return@clickable
                                val label = "@" + member.displayName.trim()
                                val end = TeamMentionEntity.replacementEnd(currentText, at.start, field.selection.end, member.displayName)
                                val rest = currentText.substring(end)
                                val separator = if (TeamMentionEntity.needsSeparator(rest)) " " else ""
                                val next = currentText.substring(0, at.start) + label + separator + rest
                                val selected = TeamMentionEntity(member.uid, at.start, label.length)
                                val shifted = TeamMentionEntity.replacing(entities, at.start, end, label.length + separator.length)
                                val caret = at.start + label.length + if (separator.isNotEmpty() || rest.startsWith(' ')) 1 else 0
                                update(next, TextRange(caret), mentions = (shifted + selected).sortedBy { it.offset }, replaced = TextRange(at.start, end))
                            },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            PersonAvatar(member.uid, member.displayName, 24.dp)
                            Text(member.displayName, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (nameCounts.getOrDefault(key, 0) > 1) {
                                Text(member.uid, Modifier.weight(1f), style = MaterialTheme.typography.labelSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                    items(candidates, key = { "bot:${it.sharedBotId}" }) { bot ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) {
                                finishUserEdits()
                                val currentText = field.text.toString()
                                val at = TeamMentionEntity.at(currentText, field.selection.end, entities) ?: return@clickable
                                val end = TeamMentionEntity.replacementEnd(currentText, at.start, field.selection.end, bot.name)
                                val cleaned = TeamsComposeLogic.withoutMention(currentText, at.start, end)
                                val removed = currentText.length - cleaned.length
                                val shifted = TeamMentionEntity.replacing(entities, at.start, at.start + removed, 0)
                                update(cleaned, TextRange(at.start.coerceAtMost(cleaned.length)), draft.targets + bot.sharedBotId, shifted, replaced = TextRange(at.start, at.start + removed))
                            },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            BotAvatar(bot.avatarId, 24.dp)
                            Text(bot.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
            HorizontalDivider()
        }
        ComposerInputSurface(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
            if (editing != null || replyTo != null) {
                Row(Modifier.fillMaxWidth().padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
                        Text(
                            if (editing != null) stringResource(R.string.teams_editing_message) else stringResource(R.string.teams_replying_to_fmt, replyAuthor),
                            style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary,
                        )
                        val quoted = editing ?: replyTo!!
                        Text(if (quoted.recalled) stringResource(R.string.teams_message_recalled) else TeamsComposeLogic.snippet(quoted.text), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        // An edit never asks a Bot (rule 8): said while a message is edited where Bots can be.
                        if (editing != null && conversation.kind != "dm") Text(stringResource(R.string.teams_edit_no_notify_hint), Modifier.testTag("teams.composer.editnotify"), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    IconButton(onClick = { if (editing != null) onCancelEdit() else onCancelReply() }) { Icon(Icons.Outlined.Close, stringResource(R.string.teams_cancel_reply)) }
                }
            }
            if (draft.targets.isNotEmpty() && editing == null && !answeringWait) {
                Text(stringResource(R.string.teams_composer_bots_asked), Modifier.padding(start = 4.dp, top = 6.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                FlowRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    draft.targets.sorted().forEach { key ->
                        val bot = bots.firstOrNull { it.sharedBotId == key }
                        val name = bot?.name ?: stringResource(R.string.teams_bot_tag)
                        val remove = stringResource(R.string.teams_composer_remove_bot_fmt, name)
                        InputChip(
                            selected = false, onClick = {
                                finishUserEdits()
                                update(field.text.toString(), field.selection, draft.targets - key)
                            },
                            label = { Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            leadingIcon = { BotAvatar(bot?.avatarId ?: key, 20.dp) },
                            trailingIcon = { Icon(Icons.Outlined.Close, null, Modifier.size(16.dp)) },
                            modifier = Modifier.semantics { contentDescription = remove },
                        )
                    }
                }
            }
            TeamDraftFiles(files, refusals, onRemoveFile, onDismissRefusals)
            blockedBots.forEach { bot ->
                Text(bot.name + " · " + botAvailabilityText(bot), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            // The Bots this message asks whose Desktop is off, named so the person knows what happens to it.
            if (waitsForHost && editing == null && !answeringWait) bots.filter { it.sharedBotId in requestTargets && it.availability == "offline" }.forEach { bot ->
                Text(stringResource(R.string.teams_bot_offline_wait_fmt, bot.name), Modifier.padding(horizontal = 16.dp, vertical = 4.dp).testTag("teams.composer.hostwait.${bot.sharedBotId}"),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (staleTargets.isNotEmpty() || staleBotDm) Text(stringResource(R.string.teams_error_bot_access_changed), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            val palette = LingXiTheme.palette
            val focus = remember { FocusRequester() }
            val placeholder = if (editing != null) stringResource(R.string.teams_composer_edit_hint)
                else stringResource(R.string.teams_composer_hint_fmt, if (conversation.kind == "channel") "#$label" else label)
            BasicTextField(
                state = field, lineLimits = TextFieldLineLimits.MultiLine(minHeightInLines = 1, maxHeightInLines = 6),
                enabled = !messaging.submitting,
                inputTransformation = {
                    if (changes.changeCount > 0) {
                        // The edit log distinguishes a same-text replacement from a selection change.
                        val ranges = (changes.changeCount - 1 downTo 0).map { index ->
                            TeamEditorReplacement(changes.getOriginalRange(index), changes.getRange(index).length)
                        }
                        editEvents.trySend(TeamEditorEdit(asCharSequence().toString(), ranges))
                    }
                },
                outputTransformation = {
                    if (TeamMessageFormat.valid(formats, asCharSequence().toString())) {
                        for (run in formats) addStyle(run.span(), run.offset, run.end)
                    }
                },
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = palette.text),
                cursorBrush = SolidColor(palette.accent),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 2.dp)
                    .focusRequester(focus).testTag(TeamsTags.COMPOSER_FIELD),
                decorator = { inner ->
                    Box {
                        if (text.isEmpty()) Text(placeholder, color = palette.text4, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        inner()
                    }
                },
            )
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                val canInsertMention = canMention && ((roster.isNotEmpty() && entities.size < TeamMentionEntity.LIMIT) ||
                    (editing == null && hasAskableBots && draft.targets.size < TeamsComposeLogic.MAX_TARGETS))
                TeamAttachButton(
                    enabled = !messaging.submitting,
                    onChosen = onChooseFiles,
                    canAttach = files.size < TeamAttachments.MESSAGE_FILES,
                    onMention = if (canInsertMention) ({
                        finishUserEdits()
                        val currentText = field.text.toString()
                        val range = field.selection
                        val before = currentText.substring(0, range.min)
                        val token = if (before.isEmpty() || before.last().isWhitespace()) "@" else " @"
                        val shifted = TeamMentionEntity.replacing(entities, range.min, range.max, token.length)
                        update(before + token + currentText.substring(range.max), TextRange(range.min + token.length),
                            mentions = shifted, replaced = range)
                        focus.requestFocus()
                    }) else null,
                    onFormat = { style ->
                        finishUserEdits()
                        val range = field.selection
                        if (range.collapsed) {
                            val current = typingStyle?.takeIf { it.first == range.start }?.second
                                ?: TeamMessageFormat.styleAt(formats, range.start)
                            typingStyle = range.start to (current xor style.bit)
                        } else {
                            val next = TeamMessageFormat.toggle(formats, range.min, range.max, style.bit)
                            if (TeamMessageFormat.valid(next, field.text.toString())) {
                                formats = next
                                persistDraft(field.text.toString(), readLatestDraft().targets, entities, formats)
                            }
                            typingStyle = null
                        }
                        focus.requestFocus()
                    },
                )
                Spacer(Modifier.weight(1f))
                if (voice != null) TeamVoiceButton(voice, enabled = !messaging.submitting)
                ComposerSendButton(
                    label = stringResource(if (editing != null) R.string.teams_message_save else R.string.teams_message_send),
                    enabled = canSend,
                    icon = if (editing != null) LXIconName.Check else LXIconName.ArrowUp,
                    modifier = Modifier.testTag(TeamsTags.SEND),
                    onClick = {
                        finishUserEdits()
                        val submitted = field.text.toString()
                        val submittedEntities = entities
                        val submittedFormats = formats
                        if (!TeamMessageFormat.valid(submittedFormats, submitted)) return@ComposerSendButton
                        val mentions = if (canMention) TeamMentionEntity.valid(submittedEntities, submitted)
                            .filter { "user:${it.userId}" in conversation.participants } else emptyList()
                        onSend(submitted, if (editing == null && askable && !answeringWait) draft.targets.sorted() else emptyList(), mentions, submittedFormats) {
                            if (field.text.toString() == submitted && entities == submittedEntities && formats == submittedFormats) {
                                field.edit {
                                    replace(0, length, "")
                                    selection = TextRange.Zero
                                }
                                trackedText = ""
                                entities = emptyList()
                                formats = emptyList()
                                typingStyle = null
                            }
                        }
                    },
                )
            }
        }
        if (!validFormats) Text(stringResource(R.string.teams_format_limit), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        if (tooLong) Text(stringResource(R.string.teams_composer_too_long), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        val note = when {
            files.any { it.state == TeamDraftFile.State.Failed } -> R.string.teams_file_note_failed
            files.any { it.state == TeamDraftFile.State.Uploading } -> R.string.teams_file_note_uploading
            conversation.kind == "bot_dm" -> R.string.teams_composer_note_bot_dm
            hasAskableBots -> R.string.teams_composer_note_group
            else -> null
        }
        if (note != null) Text(stringResource(note), Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
