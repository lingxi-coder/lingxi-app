package com.lingxi.code.conversation

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.ui.graphics.asImageBitmap
import com.lingxi.code.vision.rememberCameraCapture
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SideChatSheet(manager: SideChatManager, isDark: Boolean) {
    val chats by manager.chats.collectAsState()
    val selected by manager.selected.collectAsState()
    val visible by manager.visible.collectAsState()
    val error by manager.error.collectAsState()
    val scope = rememberCoroutineScope()
    if (!visible) return
    ModalBottomSheet(onDismissRequest = { manager.visible.value = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxHeight(0.94f).imePadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                LazyRow(Modifier.weight(1f)) {
                    itemsIndexed(chats, key = { _, entry -> entry.source.sideId }) { index, entry ->
                        TextButton(onClick = { manager.selected.value = entry }) { Text(stringResource(R.string.side_chat_number, index + 1)) }
                    }
                }
                selected?.let { entry ->
                    val closing by entry.closing.collectAsState()
                    TextButton(onClick = { manager.end(entry) }) { Text(stringResource(if (closing) R.string.side_chat_retry_end else R.string.side_chat_end)) }
                }
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(12.dp)) }
            selected?.let { entry ->
                key(entry) {
                    val state by entry.viewModel.state.collectAsState()
                    val controls by entry.source.controls.collectAsState()
                    val closing by entry.closing.collectAsState()
                    var controlMenu by remember { mutableStateOf(false) }
                    Box {
                        TextButton(onClick = { controlMenu = true }, enabled = controls != null && !closing) {
                            Text(controls?.reasoning?.effective?.let(::reasoningLabel) ?: stringResource(R.string.side_chat_controls))
                        }
                        DropdownMenu(expanded = controlMenu, onDismissRequest = { controlMenu = false }) {
                            controls?.let { current ->
                                current.reasoning.spec.options.forEach { option ->
                                    DropdownMenuItem(text = { Text(reasoningLabel(option.selection)) },
                                        enabled = current.reasoning.spec.editable,
                                        onClick = {
                                            controlMenu = false
                                            scope.launch {
                                                runCatching { entry.source.submitClientCommand(ClientCommand.SetReasoningSelection(option.selection)) }
                                                    .onFailure { manager.error.value = it.message }
                                            }
                                        })
                                }
                                current.permission.options.forEach { option ->
                                    DropdownMenuItem(text = { Text(option.mode) }, enabled = option.available,
                                        onClick = {
                                            controlMenu = false
                                            scope.launch {
                                                runCatching { entry.source.setPermissionMode(option.mode) }
                                                    .onFailure { manager.error.value = it.message }
                                            }
                                        })
                                }
                            }
                        }
                    }
                    val permission by entry.viewModel.pendingPermission.collectAsState()
                    val draft by entry.draft.collectAsState()
                    val attachment by entry.attachment.collectAsState()
                    val onCameraClick = rememberCameraCapture { image ->
                        // Capture belongs to the entry that opened the camera, never the current selector.
                        if (entry in manager.chats.value && !entry.closing.value) {
                            BitmapFactory.decodeByteArray(image.jpegBytes, 0, image.jpegBytes.size)?.let { bitmap ->
                                entry.attachment.value = ComposerAttachment(
                                    thumb = bitmap.asImageBitmap(), width = image.width, height = image.height,
                                    mediaType = "image/jpeg", base64 = Base64.encodeToString(image.jpegBytes, Base64.NO_WRAP),
                                )
                            }
                        }
                    }
                    var submittedDraft by remember { mutableStateOf(entry.lastSubmission) }
                    LaunchedEffect(state.error, state.compaction?.status, submittedDraft) {
                        if (state.error != null || state.compaction?.status == CompactionProgressStatus.Failed) {
                            val restored = restoreSideComposerAfterFailure(
                                entry.draft.value, entry.attachment.value, submittedDraft, entry.lastSubmittedAttachment,
                            )
                            entry.draft.value = restored.first
                            entry.attachment.value = restored.second
                        }
                    }
                    ChatScreen(
                        state = if (closing) state.copy(sessionReady = false) else state, onSend = { submittedDraft = it; entry.lastSubmission = it; entry.viewModel.send(it) },
                        onSendWithAttachment = { text, image ->
                            submittedDraft = text
                            entry.lastSubmission = text
                            entry.lastSubmittedAttachment = image
                            entry.viewModel.send(text, images = image?.toImageRef()?.let(::listOf).orEmpty())
                        },
                        attachment = attachment,
                        onCameraClick = onCameraClick,
                        onRemoveAttachment = { entry.attachment.value = null },
                        onSendPreflight = { text, attachment -> entry.viewModel.admitComposerSend(text, attachment != null) },
                        onNewChat = { manager.visible.value = false },
                        onSelectModel = entry.viewModel::selectModel,
                        isDark = isDark, modifier = Modifier.weight(1f),
                        draft = draft, onDraftChange = { entry.draft.value = it },
                        pendingPermission = permission != null,
                        voiceInputSupported = false, voiceConversationSupported = false,
                        onStop = entry.viewModel::cancel,
                        onAnswerQuestion = entry.viewModel::answerQuestion,
                        onCancelQuestion = entry.viewModel::cancelQuestion,
                        onToggleToolCall = entry.viewModel::toggleToolCall,
                        onTogglePlan = entry.viewModel::togglePlanExpanded,
                        onDismissError = entry.viewModel::dismissError,
                    )
                    PermissionPromptDialog(state = permission,
                        onApprove = { id, response -> scope.launch { entry.viewModel.approvePermission(id, response) } },
                        onDeny = { id -> scope.launch { entry.viewModel.denyPermission(id) } })
                }
            }
        }
    }
}

private fun reasoningLabel(selection: ReasoningSelectionDto): String = when (selection) {
    is ReasoningSelectionDto.Automatic -> "Automatic"
    is ReasoningSelectionDto.Disabled -> "Thinking off"
    is ReasoningSelectionDto.Enabled -> "Thinking on"
    is ReasoningSelectionDto.Level -> selection.id
    is ReasoningSelectionDto.TokenBudget -> "${selection.tokens} tokens"
}

internal fun <T> restoreSideComposerAfterFailure(
    draft: String, attachment: T?, submittedDraft: String?, submittedAttachment: T?,
): Pair<String, T?> = (draft.ifEmpty { submittedDraft.orEmpty() }) to (attachment ?: submittedAttachment)
