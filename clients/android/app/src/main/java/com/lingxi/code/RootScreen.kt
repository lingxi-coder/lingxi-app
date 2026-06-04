package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ChatViewModel
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.conversation.EngineConversationSource
import com.lingxi.code.conversation.MockConversationSource
import com.lingxi.code.drawer.DrawerContent
import com.lingxi.code.drawer.rememberDrawerUiState
import com.lingxi.code.model.MockData
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.share.rememberShare
import com.lingxi.code.vision.rememberCameraCapture
import com.lingxi.code.voice.VoiceFlowOverlay
import com.lingxi.code.voice.rememberVoiceCapture
import android.graphics.BitmapFactory
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.launch

/**
 * Root composable for the app shell.
 *
 * Wraps the conversation surface in a Material 3 [ModalNavigationDrawer] that
 * hosts the 对话/项目/定时 [DrawerContent] (the Android analog of the iOS
 * `RootView`, which overlaid the `Drawer` over `ChatView`). The conversation's
 * menu button opens the drawer; selecting a chat/session closes it and switches
 * the [ChatViewModel] to that session; the account row invokes [onOpenSettings]
 * (the settings nav graph is wired in a later phase). Drawer + conversation read
 * the same hoisted state, and the system back gesture closes an open drawer
 * (handled by [ModalNavigationDrawer]).
 *
 * @param onOpenSettings opens the settings surface — a stub until A6 lands.
 */
@Composable
fun RootScreen(
    isDark: Boolean,
    onToggleTheme: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenSettings: () -> Unit = {},
    viewModel: ChatViewModel? = null,
) {
    val context = LocalContext.current

    // Wire the chat to the REAL engine: build an EngineConversationSource (which
    // owns the MobileEngineHandle + its single event listener) once for this
    // shell, falling back to the canned MockConversationSource when the engine is
    // unavailable (JVM host / missing cdylib / PlatformUnavailable). This mirrors
    // the iOS ConversationSourceFactory.make() guard. The previous build-then-drop
    // `buildVoiceEngine` val is gone — the source is now the sole handle owner.
    val source: ConversationSource = remember(context) {
        EngineConversationSource.create(context) ?: MockConversationSource()
    }
    val chatViewModel: ChatViewModel = viewModel ?: viewModel(
        factory = viewModelFactory { initializer { ChatViewModel(source) } },
    )

    val state by chatViewModel.state.collectAsState()
    val drawerUi = rememberDrawerUiState()
    val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    // Voice-flow overlay visibility, hoisted here (the Android analog of the iOS
    // RootView's `voiceActive` flag): the composer's mic long-press flips it on,
    // releasing the held finger flips it off. The overlay renders above the
    // drawer + conversation.
    var voiceActive by remember { mutableStateOf(false) }

    // The composer draft is hoisted here so a voice transcription (the
    // hold-to-talk release) can route its recognized text straight into the
    // input the user is about to send.
    var draft by remember { mutableStateOf("") }

    // Hold-to-talk → live transcription, gated on RECORD_AUDIO. The recognized
    // utterance is appended to the composer draft on release.
    val (onVoiceHoldStart, onVoiceHoldRelease) = rememberVoiceCapture(
        onTranscript = { transcript ->
            draft = if (draft.isBlank()) transcript else "$draft $transcript"
        },
    )

    // The captured-photo attachment is hoisted here exactly like the voice draft:
    // tapping the composer's camera affordance drives an on-device capture through
    // the same CameraController the engine bridges onto `traits::CameraControl`,
    // and the resulting JPEG surfaces as a composer thumbnail (the device-vision
    // analog of how onTranscript surfaces a recognized utterance).
    var attachment by remember { mutableStateOf<ComposerAttachment?>(null) }

    val onCameraClick = rememberCameraCapture(
        onCaptured = { image ->
            val bmp = BitmapFactory.decodeByteArray(image.jpegBytes, 0, image.jpegBytes.size)
            if (bmp != null) {
                attachment = ComposerAttachment(
                    thumb = bmp.asImageBitmap(),
                    width = image.width,
                    height = image.height,
                )
            }
        },
    )

    // Device-share: tapping a message bubble's share affordance surfaces the
    // native chooser through the same ShareController the engine bridges onto
    // `traits::SharingService` (the device-share analog of how onCameraClick
    // reuses CameraController for both the UI affordance and `tool-camera`).
    val onShare = rememberShare()

    fun closeDrawer() = scope.launch { drawerState.close() }

    Box(modifier = modifier.fillMaxSize()) {
        ModalNavigationDrawer(
            modifier = Modifier.fillMaxSize(),
            drawerState = drawerState,
            scrimColor = Color.Black.copy(alpha = 0.4f),
            drawerContent = {
                ModalDrawerSheet(
                    drawerContainerColor = LingXiTheme.palette.sidebarBg,
                    drawerTonalElevation = 0.dp,
                    modifier = Modifier.width(320.dp),
                ) {
                    DrawerContent(
                        ui = drawerUi,
                        onSelectSession = { id ->
                            drawerUi.selectSession(id)
                            chatViewModel.openSession(MockData.session(id))
                            closeDrawer()
                        },
                        onOpenSettings = {
                            closeDrawer()
                            onOpenSettings()
                        },
                        onClose = { closeDrawer() },
                    )
                }
            },
        ) {
            Box(modifier = Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars)) {
                ChatScreen(
                    state = state,
                    onSend = chatViewModel::send,
                    onNewChat = chatViewModel::newChat,
                    onSelectModel = chatViewModel::selectModel,
                    isDark = isDark,
                    onToggleTheme = onToggleTheme,
                    onOpenDrawer = { scope.launch { drawerState.open() } },
                    onMicHoldStart = {
                        voiceActive = true
                        onVoiceHoldStart()
                    },
                    onMicHoldRelease = {
                        voiceActive = false
                        onVoiceHoldRelease()
                    },
                    draft = draft,
                    onDraftChange = { draft = it },
                    onCameraClick = onCameraClick,
                    attachment = attachment,
                    onRemoveAttachment = { attachment = null },
                    onShare = onShare,
                )
            }
        }

        // Immersive voice overlay (full-screen, above everything else).
        VoiceFlowOverlay(visible = voiceActive)
    }
}

@Preview(showBackground = true)
@Composable
private fun RootScreenPreview() {
    LingXiTheme {
        RootScreen(isDark = true, onToggleTheme = {})
    }
}
