package com.lingxi.code.remote

import android.app.Activity
import android.graphics.BitmapFactory
import android.widget.Toast
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.ConversationBackgroundExecution
import com.lingxi.code.conversation.PermissionPromptDialog
import com.lingxi.code.conversation.PermissionPromptState
import com.lingxi.code.conversation.REMOTE_MAX_PROMPT_IMAGES
import com.lingxi.code.conversation.RemoteConversationSource
import com.lingxi.code.conversation.conversationStrings
import com.lingxi.code.conversation.isTeamApprovalSessionId
import com.lingxi.code.conversation.sessionModeLabel
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.lifecycle.viewmodel.initializer
import kotlinx.coroutines.launch
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.TimeoutCancellationException
import com.lingxi.code.settings.ProviderAccountModelAvailability
import com.lingxi.code.settings.decodeProviderAccounts
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.LocalTabHost

/** Test tags of the session page. */
internal object RemoteSessionTags {
    const val BACK = "remote.session.back"
}

/** A followed session: its source and the view model that renders it. */
class RemoteChat internal constructor(
    val source: RemoteConversationSource,
    val chat: ChatViewModel,
    private val store: ViewModelStore,
) {
    internal var savedDraft by mutableStateOf("")
        private set
    internal var needsReattach: Boolean = false
    internal val attachments = mutableStateListOf<ComposerAttachment>()

    internal fun updateDraft(value: String) { savedDraft = value }

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
    private val releasing = mutableMapOf<String, Job>()

    /** The followed session for [key], made by [create] the first time. [created] is true then. */
    fun open(key: String, create: () -> RemoteConversationSource, onCreated: (RemoteChat) -> Unit, strings: com.lingxi.code.conversation.ConversationStrings): RemoteChat {
        releasing.remove(key)?.cancel()
        return open.getOrPut(key) {
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
    }

    fun release(key: String) {
        val remote = open[key] ?: return
        remote.needsReattach = true
        releasing.remove(key)?.cancel()
        // A submitted write and a failed or unsent draft belong to this owner
        // even after the screen disappears. Reopening cancels this cleanup.
        if (remote.savedDraft.isNotEmpty() || remote.attachments.isNotEmpty()) return
        val cleanup = viewModelScope.launch(start = CoroutineStart.LAZY) {
            combine(remote.chat.externalSendInFlight, remote.chat.externalSendFailures) { count, failed ->
                count == 0 && failed.isEmpty()
            }.first { it }
            if (open[key] === remote) {
                open.remove(key)
                remote.release()
            }
            releasing.remove(key)
        }
        releasing[key] = cleanup
        cleanup.start()
    }

    override fun onCleared() {
        releasing.values.forEach { it.cancel() }
        releasing.clear()
        open.values.forEach { it.release() }
        open.clear()
    }
}

/**
 * One session of a computer, with the conversation UI of the app. Side chat and voice are off.
 * [botName] is the Bot the session is a chat of, if it is one; it is only named under the title. Back leads to
 * what the page was opened from: the tree of computers and Bots, or the Computer page.
 */
@Composable
internal fun RemoteSessionScreen(
    manager: RemoteControlManager,
    hostId: String,
    hostName: String,
    sessionId: String,
    title: String?,
    botName: String?,
    isDark: Boolean,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val t = LingXiTheme.palette
    val ownerUid by manager.uid.collectAsState()
    // The parent tab's lifecycle-aware account collector can still hold its previous value.
    // Keep source creation safe at this boundary too, including while a retained page leaves.
    if (ownerUid == null) {
        val tabs = LocalTabHost.current
        RemotePage(stringResource(R.string.remote_bots), onBack) {
            RemoteSignedOut { tabs.openSettings(SettingsRoutes.ACCOUNT, origin = AppTab.Bots) }
        }
        return
    }
    val holder: RemoteSessionHolder = viewModel()
    val key = "$ownerUid/$hostId/$sessionId"
    val strings = remember(context) { conversationStrings(context.applicationContext) }
    val deviceName = remember { RemoteControl.deviceName() }
    val remote = remember(key) {
        holder.open(
            key = key,
            create = { RemoteConversationSource(manager.sessionPort(hostId, sessionId), sessionId, deviceName, strings) },
            onCreated = { it.chat.beginExternalLoading(SessionRef(sessionId, title ?: sessionId)) },
            strings = strings,
        )
    }
    val draft = remote.savedDraft
    val attachments = remote.attachments
    val activity = context as? Activity
    DisposableEffect(key) {
        onDispose {
            if (activity?.isChangingConfigurations != true) holder.release(key)
        }
    }

    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var accountAvailability by remember(remote) { mutableStateOf(emptyList<ProviderAccountModelAvailability>()) }
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
    val approvalOnly = remember(sessionId) { isTeamApprovalSessionId(sessionId) }
    var reattaching by remember(remote) { mutableStateOf(remote.needsReattach) }
    var reattachError by remember(remote) { mutableStateOf<String?>(null) }
    val mayReadAccounts = !approvalOnly && !reattaching && meta.sessionMode != null && meta.phase == RemoteSessionPhaseFfi.LIVE
    LaunchedEffect(remote, lifecycle, mayReadAccounts) {
        if (!mayReadAccounts) {
            accountAvailability = emptyList()
            return@LaunchedEffect
        }
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                try {
                    accountAvailability = decodeProviderAccounts(withTimeout(10_000) { remote.source.providerAccountsSnapshot() }).modelAvailability
                } catch (_: TimeoutCancellationException) {
                    accountAvailability = emptyList()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    accountAvailability = emptyList()
                }
                delay(15_000)
            }
        }
    }
    LaunchedEffect(remote) {
        if (reattaching) {
            remote.source.prepareForReattach()
            chat.beginExternalLoading(SessionRef(sessionId, title ?: sessionId))
            try {
                manager.reattachSession(hostId, sessionId, ownerUid)
                remote.needsReattach = false
                reattaching = false
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                reattachError = error.message ?: context.getString(R.string.remote_err_generic)
            }
        }
    }

    fun reportPermissionError(error: Throwable) {
        val message = error.message ?: context.getString(R.string.remote_err_generic)
        if (approvalOnly) Toast.makeText(context, message, Toast.LENGTH_LONG).show() else chat.reportHostError(message)
    }
    val approve: (ULong, PermissionResponseDto) -> Unit = { requestId, response ->
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try { chat.approvePermission(requestId, response) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { reportPermissionError(error) }
        }
    }
    val deny: (ULong) -> Unit = { requestId ->
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try { chat.denyPermission(requestId) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { reportPermissionError(error) }
        }
    }

    val failedSends by chat.externalSendFailures.collectAsState()
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
            // Back leads to the tree of computers and Bots (section 4.6), whichever Bot the chat belongs to.
            IconButton(onClick = onBack, modifier = Modifier.testTag(RemoteSessionTags.BACK)) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.remote_back),
                    tint = t.text2,
                )
            }
            Column(Modifier.weight(1f)) {
                Text(if (approvalOnly) stringResource(R.string.permission_request_title) else state.session.title,
                    color = t.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                val modeLabel = meta.sessionMode?.let { sessionModeLabel(it) }
                Text(listOfNotNull(botName, hostName, modeLabel).joinToString(" · "), color = t.text3, fontSize = 12.sp, maxLines = 1)
            }
        }
        val ownerMode = meta.sessionMode
        if (reattaching) {
            Text(reattachError ?: stringResource(R.string.remote_session_loading), color = t.text3,
                modifier = Modifier.weight(1f).padding(16.dp))
        } else if (approvalOnly) {
            TeamApprovalContent(
                phase = meta.phase,
                statusLine = state.statusLine,
                permission = pendingPermission,
                onApprove = approve,
                onDeny = deny,
                modifier = Modifier.weight(1f),
            )
        } else if (ownerMode == null) {
            Text(state.statusLine ?: stringResource(R.string.remote_session_loading), color = t.text3,
                modifier = Modifier.weight(1f).padding(16.dp))
        } else {
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
                remoteModelSelection = true,
                accountModelAvailability = accountAvailability,
                onSelectReasoning = chat::selectReasoning,
                isDark = isDark,
                sessionMode = ownerMode,
                showTopBar = false,
                showNewChatControls = false,
                // Voice belongs to the local engine; a remote session takes typed text.
                voiceInputSupported = false,
                voiceConversationSupported = false,
                draft = draft,
                onDraftChange = remote::updateDraft,
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
                hasUnsentDraft = failedSends.isNotEmpty(),
                canRestoreUnsentDraft = draft.isEmpty() && attachments.isEmpty(),
                onRestoreUnsentDraft = {
                    val failed = failedSends.firstOrNull()
                    if (failed != null && remote.savedDraft.isEmpty() && attachments.isEmpty()) {
                        val restored = runCatching {
                            failed.images.map { image ->
                                val bytes = Base64.getDecoder().decode(image.base64)
                                val bitmap = checkNotNull(BitmapFactory.decodeByteArray(bytes, 0, bytes.size))
                                ComposerAttachment(bitmap.asImageBitmap(), bitmap.width, bitmap.height, image.mediaType, image.base64)
                            }
                        }.getOrNull()
                        if (restored == null) chat.reportHostError(rejected)
                        else {
                            remote.updateDraft(failed.text)
                            attachments.addAll(restored)
                            chat.acknowledgeExternalSendFailure(failed)
                        }
                    }
                },
                onStop = chat::cancel,
                onDismissError = chat::dismissError,
                onDismissCommandResult = chat::dismissCommandResult,
                onDismissCompactRejection = chat::dismissCompactRejection,
                onAnswerQuestion = chat::answerQuestion,
                onCancelQuestion = chat::cancelQuestion,
                onToggleToolCall = chat::toggleToolCall,
                onTogglePlan = chat::togglePlanExpanded,
            )
        }
    }

    // Allow once or deny only: the desktop does not take "always allow" from a phone.
    if (!approvalOnly && !reattaching) PermissionPromptDialog(state = pendingPermission, onApprove = approve, onDeny = deny)

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

/** A Team approval is a capability, so this surface has no composer, files, or session controls. */
@Composable
internal fun TeamApprovalContent(
    phase: RemoteSessionPhaseFfi,
    statusLine: String?,
    permission: PermissionPromptState?,
    onApprove: (ULong, PermissionResponseDto) -> Unit,
    onDeny: (ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    Text(
        statusLine ?: stringResource(R.string.remote_needs_permission),
        color = LingXiTheme.palette.text3,
        modifier = modifier.fillMaxWidth().padding(16.dp),
    )
    PermissionPromptDialog(
        state = if (phase == RemoteSessionPhaseFfi.LIVE) permission?.copy(suppressAlwaysAllowRule = true, autoModePrompt = null) else null,
        onApprove = onApprove,
        onDeny = onDeny,
    )
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
