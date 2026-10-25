package com.lingxi.code.conversation

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imeNestedScroll
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.modeColor
import com.lingxi.code.model.SessionMode
import com.lingxi.code.components.UiTags
import com.lingxi.code.connectivity.OfflineBanner
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest

/**
 * The conversation surface — the Android analog of the iOS `ChatView`.
 *
 * Composes the top bar (menu / title with a 「模式 · 工作区」 subtitle / new-chat), a scrolling
 * message list (with the live execution trace and the new-chat empty state),
 * and the [Composer]. The conversation itself is owned by [state]; user
 * intents are hoisted to the caller's [ChatViewModel] via the callbacks.
 *
 * @param onOpenDrawer opens the drawer (wired by the root shell).
 * @param isDark current appearance (drives inline visualization theming).
 * @param sessionMode / [workspaceName] the conversation's mode and workspace, shown
 *   under the title and used by the new-chat mode bar.
 * @param onOpenSessionPanel opens the merged session panel; the title is only
 *   tappable when this is non-null.
 * @param onMicClick a plain mic tap (reserved; the future short-press action).
 * @param onMicHoldStart / onMicHoldRelease hook the voice-flow overlay — held
 *   past the threshold opens it, releasing dismisses (the iOS "松开发送").
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(
    state: ChatState,
    onSend: (String) -> Unit,
    onSendWithAttachment: (String, ComposerAttachment?) -> Unit = { text, _ -> onSend(text) },
    onSendPreflight: (String, ComposerAttachment?) -> Boolean = { _, _ -> true },
    onNewChat: () -> Unit,
    onSelectModel: (ModelOption) -> Unit,
    onSelectReasoning: (String) -> Unit = {},
    isDark: Boolean,
    modifier: Modifier = Modifier,
    onOpenDrawer: () -> Unit = {},
    sessionMode: SessionMode = SessionMode.Code,
    workspaceName: String = "",
    onOpenSessionPanel: (() -> Unit)? = null,
    onOpenFiles: (() -> Unit)? = null,
    onOpenSideChats: (() -> Unit)? = null,
    showTopBar: Boolean = true,
    showNewChatControls: Boolean = true,
    /** New-chat mode bar: start a fresh conversation in this mode. */
    onSelectNewChatMode: (SessionMode) -> Unit = {},
    /** New-chat workspace capsule: choose where the conversation runs. */
    onPickWorkspace: () -> Unit = {},
    onMicClick: () -> Unit = {},
    onMicHoldStart: () -> Unit = {},
    onMicHoldRelease: () -> Unit = {},
    onFlowModeClick: () -> Unit = {},
    flowModeActive: Boolean = false,
    voiceInputSupported: Boolean = true,
    voiceConversationSupported: Boolean = true,
    audioGuidance: com.lingxi.code.voice.AudioSetupGuidance? = null,
    onOpenVoiceSettings: () -> Unit = {},
    onDismissAudioGuidance: () -> Unit = {},
    flowModePanel: (@Composable () -> Unit)? = null,
    draft: String = "",
    onDraftChange: (String) -> Unit = {},
    onSuggestionContext: (String, String, Boolean, Boolean) -> Unit = { _, _, _, _ -> },
    onSuggestionFeedback: (String, String, String) -> Unit = { _, _, _ -> },
    pendingPermission: Boolean = false,
    onCameraClick: () -> Unit = {},
    attachment: ComposerAttachment? = null,
    onRemoveAttachment: () -> Unit = {},
    attachments: List<ComposerAttachment> = emptyList(),
    onRemoveAttachmentAt: (Int) -> Unit = {},
    onShare: (String) -> Unit = {},
    onStop: () -> Unit = {},
    onDiscardRecoveredTurn: () -> Unit = {},
    onDismissError: () -> Unit = {},
    /** True to surface the dismissible offline banner above the composer. */
    showOfflineBanner: Boolean = false,
    onDismissOffline: () -> Unit = {},
    onRetryOffline: (() -> Unit)? = null,
    /** True when the user has not selected an enabled Provider/model yet. */
    modelSetupRequired: Boolean = false,
    /** Opens settings directly at the LLM Provider page. */
    onOpenModelSettings: () -> Unit = {},
    effectivePermissionMode: String = "auto",
    onOpenPermissionSettings: () -> Unit = {},
    /** Settings/credential state displayed alongside each provider section. */
    modelProviderStatuses: List<ModelProviderStatus> = emptyList(),
    /** Remote choices are validated by the host, independently of local credentials. */
    remoteModelSelection: Boolean = false,
    /** Opens the matching provider editor, or the provider list for null. */
    onOpenProviderSettings: (String?) -> Unit = { onOpenModelSettings() },
    /** Missing Direct-build Computer Use prerequisites, or null when ready/unavailable. */
    computerUseSetup: ComputerUseSetupStatus? = null,
    /** Opens settings directly at the Computer Use configuration page. */
    onOpenComputerUseSettings: () -> Unit = {},
    /** Hides the setup banner until readiness changes or a new android_use call needs it. */
    onDismissComputerUseSetup: () -> Unit = {},
    /** Opens the full-screen terminal with [initCommand] prefilled, not run. */
    onOpenTerminal: (sessionId: String, initCommand: String) -> Unit = { _, _ -> },
    /** Submit the answers for the pending questionnaire card. */
    onAnswerQuestion: (requestId: ULong, answers: AskUserQuestionAnswersDto) -> Unit = { _, _ -> },
    /** Dismiss the pending questionnaire card. */
    onCancelQuestion: (requestId: ULong) -> Unit = {},
    /** Resume a paused workflow directly from the execution card. */
    onResumeWorkflow: (String) -> Unit = {},
    /**
     * Expand / collapse one tool call's result body or diff, by tool-use id.
     * The expanded set lives in [ChatState] (not row-local state) because every
     * list rendering a tool call recycles its rows.
     */
    onToggleToolCall: (String) -> Unit = {},
    /** Expand / collapse the pinned plan checklist above the composer. */
    onTogglePlan: () -> Unit = {},
    /** Serves inline visualization widgets; null renders them unavailable. */
    visualizationHost: com.lingxi.code.bindings.runtime.VisualizationHost? = null,
    /** Opens the apps `LocalApp` published; null draws their cards without an Open button. */
    localAppCardLinks: LocalAppCardLinks? = null,
    /** A widget drafted a follow-up question for the composer. */
    onVisualizationFollowup: (VisualizationFollowup) -> Unit = {},
    /** The composer took the offered follow-up; its chip rides the next send. */
    onAcceptVisualizationFollowup: (VisualizationFollowup) -> Unit = {},
    onRemoveVisualizationChip: () -> Unit = {},
    linkedDetailKey: String? = null,
    onLinkedDetailHandled: () -> Unit = {},
    linkedMessageUuid: String? = null,
    onMessageLinkHandled: (Boolean) -> Unit = {},
) {
    val suggestionLifecycle = LocalLifecycleOwner.current.lifecycle
    val suggestionInputEmpty = draft.isEmpty() && attachment == null && state.visualizationChip == null
    val currentSuggestionContext by rememberUpdatedState(onSuggestionContext)
    val currentSuggestionInputEmpty by rememberUpdatedState(suggestionInputEmpty)
    val currentSuggestionPermission by rememberUpdatedState(pendingPermission || state.pendingQuestions.isNotEmpty())
    DisposableEffect(suggestionLifecycle, state.session.id) {
        val sessionId = state.session.id
        val observer = LifecycleEventObserver { _, _ ->
            currentSuggestionContext(sessionId,
                if (suggestionLifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) "focused" else "blurred",
                currentSuggestionInputEmpty, currentSuggestionPermission)
        }
        suggestionLifecycle.addObserver(observer)
        onDispose {
            suggestionLifecycle.removeObserver(observer)
            currentSuggestionContext(sessionId, "blurred", currentSuggestionInputEmpty, currentSuggestionPermission)
        }
    }
    LaunchedEffect(state.session.id, suggestionInputEmpty, pendingPermission, state.pendingQuestions.isNotEmpty()) {
        onSuggestionContext(state.session.id,
            if (suggestionLifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) "focused" else "blurred",
            suggestionInputEmpty, pendingPermission || state.pendingQuestions.isNotEmpty())
    }
    val t = LingXiTheme.palette
    val currentDraft by rememberUpdatedState(draft)
    val currentOnDraftChange by rememberUpdatedState(onDraftChange)
    val offeredFollowup = state.visualizationFollowup
    LaunchedEffect(offeredFollowup) {
        val followup = offeredFollowup ?: return@LaunchedEffect
        currentOnDraftChange(
            if (currentDraft.isBlank()) followup.text else "$currentDraft ${followup.text}",
        )
        onAcceptVisualizationFollowup(followup)
    }
    val listState = key(state.session.id) { rememberLazyListState() }
    var followState by rememberSaveable(state.session.id, stateSaver = TranscriptFollowStateSaver) {
        mutableStateOf(TranscriptFollowState())
    }
    // The ONE ordered transcript list. Every row the LazyColumn shows comes
    // from this builder, so ordering and item keys live in a single testable
    // place (see ChatRenderItem.kt).
    var retainedMessageAnchor by remember(state.session.id) { mutableStateOf<String?>(null) }
    val renderItems = buildChatRenderItems(state, linkedMessageUuid ?: retainedMessageAnchor)
    val slackPx = with(LocalDensity.current) { TranscriptFollow.BOTTOM_SLACK.roundToPx() }
    // Whether the reader is parked at the tail, measured from the real layout.
    // Only the FINAL row's end says anything about the end of the content: the
    // last row on screen always ends at the viewport edge whether or not the
    // transcript does, and a final row taller than the viewport keeps its newest
    // lines below the fold even with its top aligned.
    val isAtBottom by remember(listState, slackPx) {
        derivedStateOf {
            val layout = listState.layoutInfo
            val lastVisible = layout.visibleItemsInfo.lastOrNull()
            TranscriptFollow.isAtBottom(
                totalItemsCount = layout.totalItemsCount,
                lastVisibleIndex = lastVisible?.index,
                lastVisibleEndOffset = lastVisible?.let { it.offset + it.size },
                viewportEndOffset = layout.viewportEndOffset,
                slackPx = slackPx,
                canScrollForward = listState.canScrollForward,
                hasBottomAnchor = true,
                afterContentPadding = layout.afterContentPadding,
            )
        }
    }
    val rearmTail: () -> Unit = { followState = followState.resume() }
    val currentBottomAnchor by rememberUpdatedState(renderItems.size)
    val currentCanFollow by rememberUpdatedState(
        state.sessionReady && !state.sessionTransitioning && linkedMessageUuid == null,
    )
    val transcriptGestures = remember(listState) {
        object : NestedScrollConnection {
            private var firstIndex = 0
            private var firstOffset = 0

            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                firstIndex = listState.firstVisibleItemIndex
                firstOffset = listState.firstVisibleItemScrollOffset
                return Offset.Zero
            }

            override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
                // A nested tool/terminal can consume the gesture itself. Only
                // scrolling this transcript changes its follow intent.
                if (source == NestedScrollSource.UserInput &&
                    (listState.firstVisibleItemIndex != firstIndex ||
                        listState.firstVisibleItemScrollOffset != firstOffset)
                ) {
                    followState = followState.userScroll(consumed.y)
                } else if (source == NestedScrollSource.UserInput && available.y < 0 && !listState.canScrollForward) {
                    // A user can deliberately rejoin after a collapsed row has
                    // brought the tail into view, even with no distance left.
                    followState = followState.userScroll(available.y)
                }
                return Offset.Zero
            }
        }
    }
    // Observe measured content, including same-row streaming, image/tool
    // expansion and IME/composer resizing. Measuring a larger bottom gap must
    // not itself detach the reader. collectLatest cancels a pending correction
    // as soon as a touch or a scroll changes the user's intent.
    LaunchedEffect(listState, slackPx) {
        snapshotFlow {
            val layout = listState.layoutInfo
            TranscriptViewport(
                totalItems = layout.totalItemsCount,
                lastVisibleIndex = layout.visibleItemsInfo.lastOrNull()?.index,
                lastVisibleEnd = layout.visibleItemsInfo.lastOrNull()?.let { it.offset + it.size },
                viewportStart = layout.viewportStartOffset,
                viewportEnd = layout.viewportEndOffset,
                afterContentPadding = layout.afterContentPadding,
                canScrollForward = listState.canScrollForward,
                scrolling = listState.isScrollInProgress,
                follow = followState,
                bottomAnchor = currentBottomAnchor,
                canFollow = currentCanFollow,
            )
        }.collectLatest { viewport ->
            followState = followState.measured(
                atBottom = viewport.isAtBottom(slackPx),
                scrolling = viewport.scrolling,
            )
            if (viewport.canFollow && followState.shouldPin(viewport.scrolling) &&
                viewport.totalItems == viewport.bottomAnchor + 1
            ) {
                // The zero-height row after all real rows reaches the true end,
                // including when the final message is taller than the viewport.
                // Explicit send/jump also cancels an old fling; passive layout
                // correction waits until user scrolling has finished.
                if (viewport.canScrollForward || followState.explicitResume) {
                    listState.scrollToItem(viewport.bottomAnchor)
                }
                followState = followState.resumeHandled()
            }
        }
    }

    var linkedRowHighlight by remember(state.session.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(listState, linkedMessageUuid, state.sessionReady, state.sessionTransitioning, state.messages) {
        val uuid = linkedMessageUuid ?: return@LaunchedEffect
        retainedMessageAnchor = uuid
        if (!state.sessionReady || state.sessionTransitioning) return@LaunchedEffect
        val index = messageLinkRenderIndex(renderItems, state.messages, uuid)
        if (index >= 0) {
            followState = followState.navigateToMessage()
            linkedRowHighlight = renderItems[index].key
            listState.scrollToItem(index)
        }
        onMessageLinkHandled(index >= 0)
    }

    ConversationDetailHost(state, linkedDetailKey, onLinkedDetailHandled) {
    Box(modifier = modifier.fillMaxSize().background(t.windowBg)) {
        Column(modifier = Modifier.fillMaxSize()) {
            val isNewChat = renderItems.any { it is ChatRenderItem.Empty }
            val currentOnVisualizationFollowup by rememberUpdatedState(onVisualizationFollowup)
            val visualizationContext = remember(visualizationHost, state.session.id, isDark) {
                VisualizationScreenContext(
                    host = visualizationHost,
                    sessionId = state.session.id,
                    dark = isDark,
                    onFollowup = { currentOnVisualizationFollowup(it) },
                )
            }
            if (showTopBar) CompositionLocalProvider(LocalAppCardHost provides localAppCardLinks,
                LocalVisualizationContext provides visualizationContext) {
                TopBar(
                    title = state.session.title,
                    subtitle = listOf(sessionModeLabel(sessionMode), workspaceName)
                        .filter(String::isNotBlank)
                        .joinToString(" · "),
                    subtitleColor = t.modeColor(sessionMode),
                    onOpenDrawer = onOpenDrawer,
                    onTitleClick = onOpenSessionPanel?.takeIf { !isNewChat && state.session.id != "new" },
                    onNewChat = onNewChat,
                    onOpenSideChats = onOpenSideChats,
                    onOpenFiles = onOpenFiles,
                    state = state,
                    onResumeWorkflow = onResumeWorkflow,
                    onToggleToolCall = onToggleToolCall,
                )
            }
            OfflineBanner(visible = showOfflineBanner, onDismiss = onDismissOffline, onRetry = onRetryOffline)
            Box(modifier = Modifier.weight(1f)) {
                CompositionLocalProvider(
                    LocalVisualizationContext provides visualizationContext,
                    LocalAppCardHost provides localAppCardLinks,
                ) {
                    MessageList(
                        state = state,
                        items = renderItems,
                        listState = listState,
                        onShare = onShare,
                        linkedRowHighlight = linkedRowHighlight,
                        onOpenTerminal = onOpenTerminal,
                        onToggleToolCall = onToggleToolCall,
                        modifier = Modifier.fillMaxSize()
                            .nestedScroll(transcriptGestures)
                            .pointerInput(listState) {
                                try {
                                    awaitPointerEventScope {
                                        while (true) {
                                            val event = awaitPointerEvent(PointerEventPass.Initial)
                                            followState = if (event.changes.any { it.pressed }) {
                                                followState.pointerDown()
                                            } else {
                                                followState.pointerUp()
                                            }
                                        }
                                    }
                                } finally {
                                    followState = followState.pointerUp()
                                }
                            },
                    )
                }
                // The way back for a reader who scrolled up. An empty chat has
                // nothing to jump to, even when its hero overflows a short screen.
                if (!followState.followsLatest && !isAtBottom && renderItems.none { it is ChatRenderItem.Empty }) {
                    JumpToLatestButton(
                        onClick = rearmTail,
                        modifier = Modifier
                            .align(Alignment.BottomEnd)
                            .padding(end = 16.dp, bottom = 16.dp),
                    )
                }
            }
            computerUseSetup?.let {
                ComputerUseSetupBanner(
                    status = it,
                    onOpenComputerUseSettings = onOpenComputerUseSettings,
                    onDismiss = onDismissComputerUseSetup,
                )
            }
            if (modelSetupRequired) {
                ModelSetupBanner(onOpenModelSettings = onOpenModelSettings)
            }
            // Match iOS: one runtime footer, with execution details in summary sheets.
            state.compaction?.let { CompactionProgressRow(it) }
                ?: state.error?.takeUnless { modelSetupRequired }?.let {
                    ErrorBanner(error = it, onDismiss = onDismissError)
                }
            if (audioGuidance != null) {
                AudioSetupBanner(audioGuidance, onOpenVoiceSettings, onDismissAudioGuidance)
            } else {
                flowModePanel?.invoke()
            }
            if (isNewChat && showNewChatControls) {
                NewChatBar(
                    mode = sessionMode,
                    workspaceName = workspaceName,
                    onSelectMode = onSelectNewChatMode,
                    onPickWorkspace = onPickWorkspace,
                )
            }
            Composer(
                text = draft,
                slashCommands = state.slashCommands,
                promptSuggestion = state.promptSuggestion,
                promptSuggestionNotice = state.promptSuggestionNotice,
                suggestionTurnKey = "${state.session.id}:${state.promptSuggestionUuid}",
                onSuggestionFeedback = { action -> state.promptSuggestionUuid?.let { uuid ->
                    onSuggestionFeedback(state.session.id, uuid, action)
                } },
                onTextChange = onDraftChange,
                model = state.model,
                availableModels = state.availableModels,
                contextUsage = state.contextUsage.takeUnless { state.sessionTransitioning },
                effectivePermissionMode = effectivePermissionMode,
                onOpenPermissionSettings = onOpenPermissionSettings,
                onModelChange = onSelectModel,
                reasoning = state.reasoning,
                onReasoningChange = onSelectReasoning,
                onSend = {
                    if (modelSetupRequired) {
                        onOpenModelSettings()
                    } else if (state.sessionReady && !state.sessionTransitioning) {
                        sendComposerIfAdmitted(
                            admitted = onSendPreflight(draft, attachment),
                            send = { onSendWithAttachment(draft, attachment) },
                            clearDraft = { onDraftChange("") },
                            removeAttachment = onRemoveAttachment,
                            followTail = rearmTail,
                        )
                    }
                },
                onMicClick = onMicClick,
                onMicHoldStart = onMicHoldStart,
                onMicHoldRelease = onMicHoldRelease,
                onFlowModeClick = onFlowModeClick,
                flowModeActive = flowModeActive,
                voiceInputSupported = voiceInputSupported,
                voiceConversationSupported = voiceConversationSupported,
                onCameraClick = onCameraClick,
                attachment = attachment,
                onRemoveAttachment = onRemoveAttachment,
                attachments = attachments,
                onRemoveAttachmentAt = onRemoveAttachmentAt,
                visualizationChip = state.visualizationChip,
                onRemoveVisualizationChip = onRemoveVisualizationChip,
                isStreaming = state.isStreaming && sideChatInput(draft) == null,
                showDiscardRecovery = state.durableRecoveryBlocked,
                enabled = (sideChatInput(draft) != null || !state.durableRecoveryBlocked) &&
                    (modelSetupRequired || (state.sessionReady && !state.sessionTransitioning)),
                // Loading preserves the live editor's selection and IME composition.
                inputEnabled = !state.durableRecoveryBlocked,
                modelSetupRequired = modelSetupRequired,
                onOpenModelSettings = onOpenModelSettings,
                modelProviderStatuses = modelProviderStatuses,
                remoteModelSelection = remoteModelSelection,
                onOpenProviderSettings = onOpenProviderSettings,
                onStop = onStop,
                onDiscardRecovery = onDiscardRecoveredTurn,
            )
        }
    }

    }

    pendingQuestionForSheet(state)?.let { request ->
        val sheetState = rememberModalBottomSheetState(
            confirmValueChange = ::pendingQuestionSheetAllowsTransition,
        )
        ModalBottomSheet(
            onDismissRequest = {},
            sheetState = sheetState,
        ) {
            AskUserQuestionCard(
                request = request,
                onSubmit = onAnswerQuestion,
                onCancel = onCancelQuestion,
                modifier = Modifier
                    .padding(horizontal = 16.dp)
                    .padding(bottom = 24.dp),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
internal fun pendingQuestionSheetAllowsTransition(target: SheetValue): Boolean =
    target != SheetValue.Hidden

/**
 * Real Direct-build Computer Use readiness projected into the conversation UI.
 *
 * The feature is usable only when Accessibility is connected, a browser is
 * authorized, and the user-started session is active.
 */
data class ComputerUseSetupStatus(
    val accessibilityEnabled: Boolean,
    val browserAuthorized: Boolean,
    val sessionActive: Boolean,
) {
    val ready: Boolean
        get() = accessibilityEnabled && browserAuthorized && sessionActive

    /**
     * Human-readable list of the still-missing setup steps, joined by "、".
     *
     * A plain function (not a property) because it needs [strings] to resolve
     * real localized copy at the ONE production render site
     * ([ComputerUseSetupBanner]); [strings] defaults to
     * [DefaultConversationStrings] (the exact zh-Hans literals) so
     * `ComputerUseSetupStatusTest` — which asserts this from a plain JVM test
     * with no Android `Context` — keeps passing unmodified but for the added
     * `()` call syntax.
     */
    fun missingSteps(strings: ConversationStrings = DefaultConversationStrings): String =
        buildList {
            if (!accessibilityEnabled) {
                add(strings.resolve(R.string.chat_computer_use_missing_accessibility, "启用 LingXi 无障碍服务"))
            }
            if (!browserAuthorized) {
                add(strings.resolve(R.string.chat_computer_use_missing_browser, "授权浏览器"))
            }
            if (!sessionActive) {
                add(strings.resolve(R.string.settings_cu_start_session, "启动控制会话"))
            }
        }.joinToString("、")
}

@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun TopBar(
    title: String, subtitle: String, subtitleColor: Color,
    onOpenDrawer: () -> Unit, onTitleClick: (() -> Unit)?, onNewChat: () -> Unit,
    onOpenSideChats: (() -> Unit)?,
    onOpenFiles: (() -> Unit)?,
    state: ChatState,
    onResumeWorkflow: (String) -> Unit,
    onToggleToolCall: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    val sessionPanelLabel = stringResource(R.string.chat_session_panel_a11y)
    androidx.compose.material3.TopAppBar(
        title = {
            Column(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                .then(if (onTitleClick != null) Modifier.testTag(UiTags.SESSION_TITLE)
                    .semantics { contentDescription = sessionPanelLabel }.clickable(onClick = onTitleClick) else Modifier),
                verticalArrangement = Arrangement.Center) {
                Text(title, color = t.text, style = androidx.compose.material3.MaterialTheme.typography.titleMedium,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (subtitle.isNotBlank()) Text(subtitle, color = subtitleColor,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        },
        navigationIcon = {
            IconButton(LXIconName.Menu, t.text, 22.dp, onOpenDrawer,
                stringResource(R.string.chat_open_side_drawer), Modifier.testTag(UiTags.OPEN_DRAWER))
        },
        actions = {
            onOpenSideChats?.let { open ->
                IconButton(LXIconName.SideChat, t.text, 22.dp, open,
                    stringResource(R.string.side_chats_title), Modifier.testTag("conversation.side-chats"))
            }
            IconButton(LXIconName.Edit, t.text, 22.dp, onNewChat, stringResource(R.string.chat_new_chat))
            ConversationSummaryMenu(state, onResumeWorkflow, onToggleToolCall, onOpenFiles)
        },
        colors = androidx.compose.material3.TopAppBarDefaults.topAppBarColors(
            containerColor = t.windowBg, scrolledContainerColor = t.windowBg,
        ),
        expandedHeight = maxOf(64.dp, (48 * androidx.compose.ui.platform.LocalDensity.current.fontScale).dp),
        windowInsets = androidx.compose.foundation.layout.WindowInsets(0, 0, 0, 0),
    )
}

/** Material supplies press feedback, semantics and a 48dp touch target. */
@Composable
private fun IconButton(
    name: LXIconName, color: Color, size: Dp, onClick: () -> Unit,
    contentDescription: String? = null, modifier: Modifier = Modifier,
) {
    androidx.compose.material3.IconButton(onClick = onClick, modifier = modifier) {
        LXIcon(name = name, size = size, color = color, contentDescription = contentDescription)
    }
}

/** Stable key for the zero-height row that marks the end of the transcript. */
private const val TRANSCRIPT_BOTTOM_ANCHOR = "transcript.bottom-anchor"

// LazyListState saves its reading position across Activity recreation. Save
// the matching intent too, without restoring a stale finger/fling interaction.
private val TranscriptFollowStateSaver = Saver<TranscriptFollowState, Boolean>(
    save = { it.followsLatest },
    restore = { TranscriptFollowState(followsLatest = it) },
)

private data class TranscriptViewport(
    val totalItems: Int,
    val lastVisibleIndex: Int?,
    val lastVisibleEnd: Int?,
    val viewportStart: Int,
    val viewportEnd: Int,
    val afterContentPadding: Int,
    val canScrollForward: Boolean,
    val scrolling: Boolean,
    val follow: TranscriptFollowState,
    val bottomAnchor: Int,
    val canFollow: Boolean,
) {
    fun isAtBottom(slackPx: Int) = TranscriptFollow.isAtBottom(
        totalItemsCount = totalItems,
        lastVisibleIndex = lastVisibleIndex,
        lastVisibleEndOffset = lastVisibleEnd,
        viewportEndOffset = viewportEnd,
        slackPx = slackPx,
        canScrollForward = canScrollForward,
        hasBottomAnchor = true,
        afterContentPadding = afterContentPadding,
    )
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun MessageList(
    state: ChatState,
    items: List<ChatRenderItem>,
    listState: androidx.compose.foundation.lazy.LazyListState,
    onShare: (String) -> Unit = {},
    linkedRowHighlight: String? = null,
    onOpenTerminal: (sessionId: String, initCommand: String) -> Unit = { _, _ -> },
    onToggleToolCall: (String) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    val currentOnShare by rememberUpdatedState(onShare)
    val currentOnToggleToolCall by rememberUpdatedState(onToggleToolCall)
    val stableOnToggleToolCall = remember { { id: String -> currentOnToggleToolCall(id) } }
    val stableOnShare = remember { { text: String -> currentOnShare(text) } }
    val currentOnOpenTerminal by rememberUpdatedState(onOpenTerminal)
    val currentSessionId by rememberUpdatedState(state.session.id)
    val currentUriHandler by rememberUpdatedState(androidx.compose.ui.platform.LocalUriHandler.current)
    val stableOnOpenLink = remember {
        { link: String ->
            val uri = runCatching { android.net.Uri.parse(link) }.getOrNull()
            if (uri?.scheme == "lingxi" && uri.host == "open_terminal") {
                currentOnOpenTerminal(
                    uri.getQueryParameter("sessionId")
                        ?.takeIf(String::isNotBlank)
                        ?: currentSessionId,
                    uri.getQueryParameter("initCommand").orEmpty(),
                )
            } else if (uri?.scheme?.lowercase() in setOf("https", "http", "mailto")) {
                runCatching { currentUriHandler.openUri(link) }
            }
        }
    }
    // Reading history must not request control of a hidden keyboard at a list
    // edge. Keep native drag-to-dismiss only while the keyboard is presented.
    val keyboardVisible = WindowInsets.isImeVisible

    LazyColumn(
        state = listState,
        modifier = modifier
            .fillMaxWidth()
            .then(if (keyboardVisible) Modifier.imeNestedScroll() else Modifier),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            start = 16.dp, end = 16.dp, top = 18.dp, bottom = 8.dp,
        ),
    ) {
        // ONE list, ONE itemsIndexed() block: order and keys come from
        // buildChatRenderItems, which preserves the exact visual order and
        // stable keys of the old hand-stitched blocks.
        itemsIndexed(
            items = items,
            key = { _, item -> item.key },
            contentType = { _, item -> item.contentType },
        ) { index, item ->
            Column(Modifier.fillMaxWidth().padding(top = transcriptSpacingBefore(items.getOrNull(index - 1), item).dp)) {
                when (item) {
                    ChatRenderItem.Empty -> BoxWithConstraints(Modifier.fillParentMaxHeight().fillMaxWidth()) {
                        val fontScale = androidx.compose.ui.platform.LocalDensity.current.fontScale
                        EmptyState(compact = maxHeight < (200 * fontScale).dp,
                            modifier = Modifier.fillMaxWidth().height(minOf(maxHeight, 360.dp)))
                    }
                    is ChatRenderItem.Tools -> Column(Modifier.background(if (item.key == linkedRowHighlight) LingXiTheme.palette.accent.copy(alpha = 0.12f) else androidx.compose.ui.graphics.Color.Transparent)) {
                        ToolGroupView(TranscriptBlock.Tools(item.calls, item.anchor), state.expandedToolCalls, stableOnToggleToolCall)
                    }
                    is ChatRenderItem.Message, is ChatRenderItem.Streaming -> Column(Modifier.background(if (item.key == linkedRowHighlight) LingXiTheme.palette.accent.copy(alpha = 0.12f) else androidx.compose.ui.graphics.Color.Transparent)) {
                        MessageBubble(
                            message = when (item) {
                                is ChatRenderItem.Message -> item.message
                                is ChatRenderItem.Streaming -> item.message
                                else -> error("Expected a message row")
                            },
                            bottomSpacing = 0.dp,
                            onShare = stableOnShare,
                            onOpenLink = stableOnOpenLink,
                            expandedToolCalls = state.expandedToolCalls,
                            onToggleToolCall = stableOnToggleToolCall,
                        )
                    }
                    is ChatRenderItem.AgentRun -> AgentRunTimeline(
                        state = item.run,
                        modifier = Modifier.padding(vertical = 4.dp),
                        expandedToolCalls = state.expandedToolCalls,
                        onToggleToolCall = stableOnToggleToolCall,
                    )
                    is ChatRenderItem.Shell -> ShellToolCard(
                        state = item.shell,
                        onOpenTerminal = onOpenTerminal,
                        modifier = Modifier.padding(vertical = 4.dp),
                    )
                    ChatRenderItem.StreamingIndicator -> StreamingRow()
                    is ChatRenderItem.Visualization -> VisualizationCard(item.status, item.reference)
                }
            }
        }
        // Zero-height anchor past the last real row, so scrolling to the latest
        // reaches the true end of the content. Aligning a tall final row's top
        // is not the same thing: its newest lines stay below the fold.
        item(key = TRANSCRIPT_BOTTOM_ANCHOR) { Spacer(Modifier.height(0.dp)) }
    }
}

/**
 * Floating "back to the newest output" control. Shown only while the reader is
 * detached from the tail, and it is the only path back besides sending a prompt.
 */
@Composable
private fun JumpToLatestButton(onClick: () -> Unit, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    val label = stringResource(R.string.chat_jump_to_latest)
    Box(
        contentAlignment = Alignment.Center,
        modifier = modifier
            .size(48.dp)
            .clip(CircleShape)
            .background(t.surface)
            .border(0.5.dp, t.border, CircleShape)
            .clickable(onClick = onClick)
            .semantics {
                role = Role.Button
                contentDescription = label
            }
            .testTag(UiTags.CHAT_JUMP_TO_LATEST),
    ) {
        LXIcon(name = LXIconName.Chevron, size = 16.dp, color = t.text2, stroke = 1.8f)
    }
}

/** The empty chat uses the remaining viewport; configuration banners can shrink it. */
@Composable
private fun EmptyState(compact: Boolean, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp, vertical = if (compact) 8.dp else 16.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        if (!compact) {
            LXIcon(name = LXIconName.Sparkle, size = 24.dp, color = t.accent)
            Spacer(Modifier.size(10.dp))
        }
        Text(
            stringResource(R.string.chat_start_new_conversation),
            color = t.text,
            fontSize = 20.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
        )
        if (!compact) {
            Spacer(Modifier.size(10.dp))
            Text(
                stringResource(R.string.chat_new_chat_hint),
                color = t.text3,
                style = androidx.compose.material3.MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 300.dp),
            )
        }
    }
}

@Composable
private fun CompactionProgressRow(progress: CompactionProgressUi) {
    val t = LingXiTheme.palette
    val startedAt = progress.startedAtMillis
    var elapsedMs by remember(startedAt, progress.phaseStartedAtMillis, progress.status) {
        mutableLongStateOf(startedAt?.let { (compactionClockMillis() - it).coerceAtLeast(0L) } ?: 0L)
    }
    LaunchedEffect(progress.startedAtMillis, progress.phaseStartedAtMillis, progress.status) {
        if (progress.status != CompactionProgressStatus.Running || startedAt == null) return@LaunchedEffect
        while (true) {
            elapsedMs = (compactionClockMillis() - startedAt).coerceAtLeast(0L)
            delay(1_000)
        }
    }
    val percent = compactProgressPercent(if (progress.unknownPhase) "unknown" else progress.phase, (startedAt ?: 0L) + elapsedMs - (progress.phaseStartedAtMillis ?: 0L))
    val color = when (progress.status) {
        CompactionProgressStatus.Running -> t.accent
        CompactionProgressStatus.Completed, CompactionProgressStatus.Skipped -> t.ok
        CompactionProgressStatus.Failed -> t.danger
    }
    val title = when (progress.status) {
        CompactionProgressStatus.Running -> stringResource(when (if (progress.unknownPhase) "unknown" else progress.phase) {
            "queued" -> R.string.chat_compaction_waiting
            "preparing" -> R.string.chat_compaction_preparing
            "summarizing" -> R.string.chat_compaction_summarizing
            "restoring" -> R.string.chat_compaction_restoring
            else -> R.string.chat_compacting_context
        })
        CompactionProgressStatus.Completed -> stringResource(R.string.chat_compacted_label)
        CompactionProgressStatus.Skipped -> stringResource(R.string.chat_compaction_skipped)
        CompactionProgressStatus.Failed -> stringResource(R.string.chat_compaction_failed)
    }
    val detail = when (progress.status) {
        CompactionProgressStatus.Running -> if (percent == null) "" else stringResource(
            R.string.chat_compaction_progress,
            percent,
            elapsedMs / 1_000,
        )
        CompactionProgressStatus.Completed -> if (progress.messagesBefore == null ||
            progress.messagesAfter == null || progress.bytesSaved == null
        ) "100%" else "100% · " + stringResource(
            R.string.chat_compaction_status,
            progress.messagesBefore ?: 0,
            progress.messagesAfter ?: 0,
            formatCompactBytes(progress.bytesSaved ?: 0L),
        )
        CompactionProgressStatus.Failed -> progress.detail.orEmpty()
        CompactionProgressStatus.Skipped -> ""
    }

    Row(
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(9.dp),
        modifier = Modifier
            .fillMaxWidth()
            .testTag("conversation.compaction-status")
            .padding(horizontal = 14.dp, vertical = 6.dp)
            .semantics { contentDescription = "$title. $detail" },
    ) {
        LXIcon(
            name = when (progress.status) {
                CompactionProgressStatus.Running -> LXIconName.Book
                CompactionProgressStatus.Completed, CompactionProgressStatus.Skipped -> LXIconName.Check
                CompactionProgressStatus.Failed -> LXIconName.X
            },
            size = 17.dp,
            color = color,
        )
        Column(verticalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.weight(1f)) {
            Text(title, color = color, style = androidx.compose.material3.MaterialTheme.typography.bodySmall, fontWeight = FontWeight.Medium)
            if (detail.isNotEmpty()) {
                Text(detail, color = t.text4, style = androidx.compose.material3.MaterialTheme.typography.labelSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (progress.status == CompactionProgressStatus.Running && percent != null) {
                LinearProgressIndicator(
                    progress = { percent / 100f },
                    color = color,
                    trackColor = t.surfaceActive,
                    modifier = Modifier.fillMaxWidth(),
                )
            } else if (progress.status == CompactionProgressStatus.Running) {
                LinearProgressIndicator(
                    color = color,
                    trackColor = t.surfaceActive,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

/** An actionable configuration state, kept separate from runtime failures. */
@Composable
private fun ModelSetupBanner(onOpenModelSettings: () -> Unit) {
    SetupNotice(
        title = stringResource(R.string.chat_model_not_configured_title),
        detail = stringResource(R.string.chat_model_not_configured_detail),
        icon = LXIconName.Cog,
        action = stringResource(R.string.chat_go_to_settings),
        onAction = onOpenModelSettings,
        tag = UiTags.MODEL_SETUP_BANNER,
        actionTag = UiTags.MODEL_SETUP_ACTION,
    )
}

/**
 * Composable-context localized rendering of [ComputerUseSetupStatus.missingSteps] —
 * the real [stringResource] lookup counterpart, since the pure member function
 * has no `Context` to resolve the user's actual selected language from.
 */
@Composable
private fun missingStepsLabel(status: ComputerUseSetupStatus): String {
    val accessibility = stringResource(R.string.chat_computer_use_missing_accessibility)
    val browser = stringResource(R.string.chat_computer_use_missing_browser)
    val session = stringResource(R.string.settings_cu_start_session)
    return buildList {
        if (!status.accessibilityEnabled) add(accessibility)
        if (!status.browserAuthorized) add(browser)
        if (!status.sessionActive) add(session)
    }.joinToString("、")
}

/** Direct link from an unavailable Computer Use state to its complete setup page. */
@Composable
internal fun ComputerUseSetupBanner(
    status: ComputerUseSetupStatus,
    onOpenComputerUseSettings: () -> Unit,
    onDismiss: () -> Unit,
) {
    SetupNotice(
        title = stringResource(R.string.chat_computer_use_unavailable_title),
        detail = stringResource(R.string.chat_computer_use_unavailable_detail, missingStepsLabel(status)),
        icon = LXIconName.Sparkle,
        action = stringResource(R.string.chat_go_enable),
        onAction = onOpenComputerUseSettings,
        tag = UiTags.COMPUTER_USE_SETUP_BANNER,
        actionTag = UiTags.COMPUTER_USE_SETUP_ACTION,
        onDismiss = onDismiss,
        dismissLabel = stringResource(R.string.chat_computer_use_dismiss),
        dismissTag = UiTags.COMPUTER_USE_SETUP_DISMISS,
    )
}

/**
 * A persistent, dismissible, kind-aware error banner above the composer.
 *
 * Unlike [StatusRow] (a dim, single-line, auto-overwritten status), this is a
 * filled danger-tinted row with a kind-specific headline and a × dismiss button,
 * driven by [ChatState.error]. It survives until the user dismisses it or starts
 * a new turn — so a failed reply is never lost to a transient flash (spec item 4).
 */
@Composable
private fun ErrorBanner(error: ChatError, onDismiss: () -> Unit) {
    val t = LingXiTheme.palette
    val headline = when (error.kind) {
        ChatErrorKind.AUTH -> stringResource(R.string.chat_error_auth_headline)
        ChatErrorKind.NETWORK -> stringResource(R.string.chat_error_transport)
        ChatErrorKind.GENERIC -> stringResource(R.string.chat_error_generic_headline)
    }
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .testTag(UiTags.CHAT_ERROR)
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 5.dp),
    ) {
        Box(
            modifier = Modifier
                .size(18.dp)
                .clip(CircleShape)
                .background(t.danger),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.X, size = 11.dp, color = androidx.compose.material3.MaterialTheme.colorScheme.onError, stroke = 2.4f)
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = headline,
                color = t.danger,
                style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                fontWeight = FontWeight.SemiBold,
            )
            androidx.compose.foundation.text.selection.SelectionContainer {
                Text(
                    text = error.message,
                    color = t.text2,
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    lineHeight = (12.5f * 1.4f).sp,
                )
            }
        }
        Box(
            modifier = Modifier
                .size(28.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(onClick = onDismiss)
                .testTag(UiTags.CHAT_ERROR_DISMISS),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.X,
                size = 14.dp,
                color = t.text3,
                stroke = 2f,
                contentDescription = stringResource(R.string.chat_dismiss_error),
            )
        }
    }
}

/** Minimal fallback shown only if a source streams without a run trace. */
@Composable
private fun StreamingRow() {
    val t = LingXiTheme.palette
    val transition = rememberInfiniteTransition(label = "streaming")
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(11.dp),
        modifier = Modifier.fillMaxWidth().padding(bottom = 26.dp),
    ) {
        AssistantAvatar()
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            repeat(3) { i ->
                val alpha by transition.animateFloat(
                    initialValue = 0.35f,
                    targetValue = 0.8f,
                    animationSpec = infiniteRepeatable(
                        animation = tween(durationMillis = 1200, delayMillis = i * 150),
                        repeatMode = RepeatMode.Reverse,
                    ),
                    label = "dot-$i",
                )
                Box(
                    modifier = Modifier
                        .size(5.dp)
                        .alpha(alpha)
                        .clip(CircleShape)
                        .background(t.accent),
                )
            }
        }
    }
}

/** A rejection must not clear either half of the editable composer. */
internal fun sendComposerIfAdmitted(
    admitted: Boolean,
    send: () -> Unit,
    clearDraft: () -> Unit,
    removeAttachment: () -> Unit,
    followTail: () -> Unit,
) {
    if (!admitted) return
    send()
    clearDraft()
    removeAttachment()
    followTail()
}
