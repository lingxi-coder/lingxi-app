package com.lingxi.code.remote

import android.app.Activity
import android.graphics.BitmapFactory
import androidx.compose.ui.graphics.asImageBitmap
import java.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
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
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.ConversationBackgroundExecution
import com.lingxi.code.conversation.PermissionPromptDialog
import com.lingxi.code.conversation.REMOTE_MAX_PROMPT_IMAGES
import com.lingxi.code.conversation.RemoteConversationSource
import com.lingxi.code.conversation.conversationStrings
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.lifecycle.viewmodel.initializer
import kotlinx.coroutines.launch

/** A followed session: its source and the view model that renders it. */
class RemoteChat internal constructor(
    val source: RemoteConversationSource,
    val chat: ChatViewModel,
    private val store: ViewModelStore,
) {
    /** Stops following (the feed collection ends with the view model) and drops the state. */
    internal fun release() = store.clear()
}

/**
 * Keeps the followed sessions across rotation: an Activity-scoped holder, with one
 * [ViewModelStore] per session so leaving the session clears exactly its view
 * model (and with it the source's feed collection, which detaches the session).
 */
class RemoteSessionHolder : ViewModel() {
    private val open = mutableMapOf<String, RemoteChat>()

    /** The followed session for [key], made by [create] the first time. [created] is true then. */
    fun open(key: String, create: () -> RemoteConversationSource, onCreated: (RemoteChat) -> Unit, strings: com.lingxi.code.conversation.ConversationStrings): RemoteChat =
        open.getOrPut(key) {
            val source = create()
            val store = ViewModelStore()
            val chat = ViewModelProvider(
                store,
                viewModelFactory {
                    initializer {
                        ChatViewModel(
                            source = source,
                            savedState = null,
                            strings = strings,
                            backgroundExecution = ConversationBackgroundExecution.None,
                        )
                    }
                },
            )[ChatViewModel::class.java]
            RemoteChat(source, chat, store).also(onCreated)
        }

    fun release(key: String) {
        open.remove(key)?.release()
    }

    override fun onCleared() {
        open.values.forEach { it.release() }
        open.clear()
    }
}

/** One session of a computer, with the conversation UI of the app. Side chat and voice are off. */
@Composable
internal fun RemoteSessionScreen(
    manager: RemoteControlManager,
    hostId: String,
    hostName: String,
    sessionId: String,
    title: String?,
    isDark: Boolean,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val t = LingXiTheme.palette
    val holder: RemoteSessionHolder = viewModel()
    val ownerUid by manager.uid.collectAsState()
    val key = "$ownerUid/$hostId/$sessionId"
    val strings = remember(context) { conversationStrings(context.applicationContext) }
    val deviceName = remember { RemoteControl.deviceName() }
    val remote = remember(key) {
        holder.open(
            key = key,
            create = { RemoteConversationSource(manager.sessionPort(hostId, sessionId), deviceName, strings) },
            onCreated = { it.chat.beginExternalLoading(SessionRef(sessionId, title ?: sessionId)) },
            strings = strings,
        )
    }
    val activity = context as? Activity
    DisposableEffect(key) {
        onDispose { if (activity?.isChangingConfigurations != true) holder.release(key) }
    }

    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(remote, lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> scope.launch { runCatching { remote.source.setForeground(true) } }
                Lifecycle.Event.ON_PAUSE -> scope.launch { runCatching { remote.source.setForeground(false) } }
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            scope.launch { runCatching { remote.source.setForeground(false) } }
        }
    }

    val chat = remote.chat
    val state by chat.state.collectAsState()
    val pendingPermission by chat.pendingPermission.collectAsState()
    val meta by remote.source.meta.collectAsState()

    var draft by rememberSaveable(key) { mutableStateOf("") }
    val attachments = remember(key) { mutableStateListOf<ComposerAttachment>() }
    LaunchedEffect(chat) {
        chat.externalSendFailures.collect { failed ->
            if (draft.isEmpty() && attachments.isEmpty()) {
                draft = failed.text
                failed.images.forEach { image ->
                    val bytes = Base64.getDecoder().decode(image.base64)
                    BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { bitmap ->
                        attachments += ComposerAttachment(bitmap.asImageBitmap(), bitmap.width, bitmap.height, image.mediaType, image.base64)
                    }
                }
            }
        }
    }

    val rejected = stringResource(R.string.remote_image_unusable)
    val pick = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(REMOTE_MAX_PROMPT_IMAGES),
    ) { uris ->
        scope.launch {
            for (uri in uris) {
                if (attachments.size >= REMOTE_MAX_PROMPT_IMAGES) break
                try {
                    attachments += shrinkPickedImage(context, uri)
                } catch (_: RemoteImageRejected) {
                    chat.reportHostError(rejected)
                }
            }
        }
    }
    val imagesWhileRunning = stringResource(R.string.remote_images_only_when_idle)
    var modeDialog by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().background(t.windowBg)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onBack) {
                LXIcon(LXIconName.X, size = 20.dp, color = t.text2, contentDescription = stringResource(R.string.remote_back))
            }
            Column(Modifier.weight(1f)) {
                Text(state.session.title, color = t.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                Text(hostName, color = t.text3, fontSize = 12.sp, maxLines = 1)
            }
        }
        if (meta.hasOlder) {
            Text(
                stringResource(if (meta.loadingOlder) R.string.remote_loading_older else R.string.remote_load_older),
                color = t.accent, fontSize = 13.sp,
                modifier = Modifier.fillMaxWidth().clickable(enabled = !meta.loadingOlder) {
                    scope.launch { runCatching { remote.source.loadOlder() } }
                }.padding(vertical = 8.dp),
            )
        }
        RemoteFilesPanel(manager, hostId, sessionId, enabled = meta.phase == com.lingxi.code.bindings.android.RemoteSessionPhaseFfi.LIVE)
        ChatScreen(
            modifier = Modifier.weight(1f),
            state = state,
            onSend = { chat.send(it) },
            onSendWithAttachment = { text, _ ->
                chat.send(text, images = attachments.map { ImageRefDto(it.mediaType, it.base64) })
            },
            onSendPreflight = { text, _ -> chat.admitComposerSend(text, attachments.isNotEmpty()) },
            onNewChat = {},
            onSelectModel = chat::selectModel,
            onSelectReasoning = chat::selectReasoning,
            isDark = isDark,
            showTopBar = false,
            showNewChatControls = false,
            // Voice belongs to the local engine; a remote session takes typed text.
            voiceInputSupported = false,
            voiceConversationSupported = false,
            draft = draft,
            onDraftChange = { draft = it },
            pendingPermission = pendingPermission != null,
            effectivePermissionMode = meta.controls?.permission?.effective ?: "default",
            onOpenPermissionSettings = { modeDialog = true },
            onCameraClick = {
                if (state.isStreaming) chat.reportHostError(imagesWhileRunning)
                else pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            attachments = attachments.toList(),
            onRemoveAttachmentAt = { if (it in attachments.indices) attachments.removeAt(it) },
            onRemoveAttachment = { attachments.clear() },
            onStop = chat::cancel,
            onDismissError = chat::dismissError,
            onAnswerQuestion = chat::answerQuestion,
            onCancelQuestion = chat::cancelQuestion,
            onToggleToolCall = chat::toggleToolCall,
            onTogglePlan = chat::togglePlanExpanded,
        )
    }

    // Allow once or deny only: the desktop does not take "always allow" from a phone.
    PermissionPromptDialog(
        state = pendingPermission,
        onApprove = { requestId, response ->
            scope.launch {
                runCatching { chat.approvePermission(requestId, response) }
                    .onFailure { chat.reportHostError(it.message ?: it.toString()) }
            }
        },
        onDeny = { requestId -> scope.launch { runCatching { chat.denyPermission(requestId) } } },
    )

    if (modeDialog) {
        RemotePermissionModeDialog(
            current = meta.controls?.permission?.effective,
            available = meta.controls?.permission?.options.orEmpty().filter { it.available }.map { it.mode },
            onSelect = { mode ->
                modeDialog = false
                scope.launch {
                    runCatching { remote.source.setPermissionMode(mode) }.onFailure { chat.reportHostError(it.message ?: it.toString()) }
                }
            },
            onDismiss = { modeDialog = false },
        )
    }
}

/** The permission modes a phone may pick (never bypass), among those the session offers. */
@Composable
private fun RemotePermissionModeDialog(
    current: String?,
    available: List<String>,
    onSelect: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val modes = remoteSelectablePermissionModes(available)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_permission_mode_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                for (mode in modes) {
                    TextButton(onClick = { onSelect(mode) }, modifier = Modifier.fillMaxWidth()) {
                        Text(
                            stringResource(permissionModeLabel(mode)) + if (mode == current) "  ✓" else "",
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}

/** Mode ids a remote device may select, in a stable order; an empty [available] means the default five. */
internal fun remoteSelectablePermissionModes(available: List<String>): List<String> {
    val all = listOf("default", "acceptEdits", "plan", "auto", "dontAsk")
    return all.filter { available.isEmpty() || it in available }
}

internal fun permissionModeLabel(mode: String): Int = when (mode) {
    "acceptEdits" -> R.string.settings_permission_mode_accept_edits
    "plan" -> R.string.settings_permission_mode_plan
    "auto" -> R.string.settings_permission_mode_auto
    "dontAsk" -> R.string.settings_permission_mode_dont_ask
    else -> R.string.settings_permission_mode_default
}
