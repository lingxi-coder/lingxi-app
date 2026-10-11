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
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imeNestedScroll
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
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
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.connectivity.OfflineBanner
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.delay

/**
 * The conversation surface — the Android analog of the iOS `ChatView`.
 *
 * Composes the top bar (menu / title / theme-toggle / new-chat), a scrolling
 * message list (with the live execution trace and the new-chat empty state),
 * and the [Composer]. The conversation itself is owned by [state]; user
 * intents are hoisted to the caller's [ChatViewModel] via the callbacks.
 *
 * @param onOpenDrawer opens the 对话/项目/定时 drawer (wired by the root shell).
 * @param isDark current appearance, drives the sun/moon toggle glyph.
 * @param onToggleTheme flips the persisted theme.
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
    onNewChat: () -> Unit,
    onSelectModel: (ModelOption) -> Unit,
    isDark: Boolean,
    onToggleTheme: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenDrawer: () -> Unit = {},
    onMicClick: () -> Unit = {},
    onMicHoldStart: () -> Unit = {},
    onMicHoldRelease: () -> Unit = {},
    onFlowModeClick: () -> Unit = {},
    flowModeActive: Boolean = false,
    voiceInputSupported: Boolean = true,
    voiceConversationSupported: Boolean = true,
    flowModePanel: (@Composable () -> Unit)? = null,
    draft: String = "",
    onDraftChange: (String) -> Unit = {},
    onCameraClick: () -> Unit = {},
    attachment: ComposerAttachment? = null,
    onRemoveAttachment: () -> Unit = {},
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
    /** Settings/credential state displayed alongside each provider section. */
    modelProviderStatuses: List<ModelProviderStatus> = emptyList(),
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
    onAnswerQuestion: (requestId: ULong, answers: Map<String, String>) -> Unit = { _, _ -> },
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
    /** A widget drafted a follow-up question for the composer. */
    onVisualizationFollowup: (VisualizationFollowup) -> Unit = {},
    /** The composer took the offered follow-up; its chip rides the next send. */
    onAcceptVisualizationFollowup: (VisualizationFollowup) -> Unit = {},
    onRemoveVisualizationChip: () -> Unit = {},
) {
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
    val listState = rememberLazyListState()
    // The ONE ordered transcript list. Every row the LazyColumn shows comes
    // from this builder, so ordering and item keys live in a single testable
    // place (see ChatRenderItem.kt).
    val renderItems = buildChatRenderItems(state)
    val slackPx = with(LocalDensity.current) { TranscriptFollow.BOTTOM_SLACK.roundToPx() }
    // Whether the reader is parked at the tail, measured from the real layout.
    // Only the FINAL row's end says anything about the end of the content: the
    // last row on screen always ends at the viewport edge whether or not the
    // transcript does, and a final row taller than the viewport keeps its newest
    // lines below the fold even with its top aligned.
    val followsLatest by remember(listState, slackPx) {
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
            )
        }
    }
    // The tail of the content is a zero-height row past the last real one.
    // Scrolling to THAT is scrolling to the true end, which a tall final row
    // cannot provide — aligning its top still leaves its newest lines below the
    // fold.
    val bottomAnchor = renderItems.size
    // An explicit re-engagement — the reader's own send, or the jump control —
    // forces exactly one scroll to the tail. Nothing re-arms the follow on a
    // timer: a reader who stopped scrolling is reading.
    var jumpRequest by remember { mutableStateOf(0) }
    val rearmTail: () -> Unit = { jumpRequest += 1 }

    // Follow new output only while the reader is already at the tail. Forcing a
    // jump from old history on every update is expensive on long transcripts and
    // prevents the reader from reading earlier messages.
    LaunchedEffect(
        state.messages.size,
        state.streamingMessage?.id,
        state.shellTools.size,
        state.agentRun?.revision,
        state.streaming,
        jumpRequest,
    ) {
        val requested = jumpRequest != 0
        if (!followsLatest && !requested) return@LaunchedEffect
        if (renderItems.isNotEmpty()) listState.scrollToItem(bottomAnchor)
        // Consumed: the forced scroll happens once, then the measured follow
        // takes over (it is true by now, because we just landed on the tail).
        if (requested) jumpRequest = 0
    }

    ConversationDetailHost(state) {
    Box(modifier = modifier.fillMaxSize().background(t.windowBg)) {
        Column(modifier = Modifier.fillMaxSize()) {
            TopBar(
                title = state.session.title,
                isDark = isDark,
                onOpenDrawer = onOpenDrawer,
                onToggleTheme = onToggleTheme,
                onNewChat = onNewChat,
            )
            Box(modifier = Modifier.weight(1f)) {
                val currentOnVisualizationFollowup by rememberUpdatedState(onVisualizationFollowup)
                val visualizationContext = remember(visualizationHost, state.session.id, isDark) {
                    VisualizationScreenContext(
                        host = visualizationHost,
                        sessionId = state.session.id,
                        dark = isDark,
                        onFollowup = { currentOnVisualizationFollowup(it) },
                    )
                }
                CompositionLocalProvider(LocalVisualizationContext provides visualizationContext) {
                    MessageList(
                        state = state,
                        items = renderItems,
                        listState = listState,
                        onShare = onShare,
                        onOpenTerminal = onOpenTerminal,
                        onToggleToolCall = onToggleToolCall,
                        modifier = Modifier.fillMaxSize(),
                    )
                }
                // The way back for a reader who scrolled up.
                if (!followsLatest) {
                    JumpToLatestButton(
                        onClick = rearmTail,
                        modifier = Modifier
                            .align(Alignment.BottomEnd)
                            .padding(end = 16.dp, bottom = 16.dp),
                    )
                }
            }
            OfflineBanner(
                visible = showOfflineBanner,
                onDismiss = onDismissOffline,
                onRetry = onRetryOffline,
            )
            computerUseSetup?.let {
                ComputerUseSetupBanner(
                    status = it,
                    onOpenComputerUseSettings = onOpenComputerUseSettings,
                    onDismiss = onDismissComputerUseSetup,
                )
            }
            if (modelSetupRequired) {
                ModelSetupBanner(onOpenModelSettings = onOpenModelSettings)
            } else {
                state.error?.let { ErrorBanner(error = it, onDismiss = onDismissError) }
            }
            state.compaction?.let { CompactionProgressRow(it) }
            state.statusLine?.let { StatusRow(text = it) }
            ExecutionStatusPanel(
                tasks = state.backgroundTasks.values.toList(),
                workflows = state.workflowRuns.values.toList(),
                agents = state.sessionAgents,
                planTasks = state.planTasks,
                planExpanded = state.planExpanded,
                onTogglePlan = onTogglePlan,
                onResumeWorkflow = onResumeWorkflow,
                resumeState = state.workflowResumeState,
                agentRun = agentRunForBottomPanel(state),
                expandedToolCalls = state.expandedToolCalls,
                onToggleToolCall = onToggleToolCall,
            )
            flowModePanel?.invoke()
            Composer(
                text = draft,
                onTextChange = onDraftChange,
                model = state.model,
                availableModels = state.availableModels,
                onModelChange = onSelectModel,
                onSend = {
                    if (modelSetupRequired) {
                        onOpenModelSettings()
                    } else if (state.sessionReady && !state.sessionTransitioning) {
                        onSendWithAttachment(draft, attachment)
                        onDraftChange("")
                        onRemoveAttachment()
                        // Sending is an explicit re-engagement: a reader who
                        // scrolled up to re-read something must not have their
                        // own prompt land off screen.
                        rearmTail()
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
                visualizationChip = state.visualizationChip,
                onRemoveVisualizationChip = onRemoveVisualizationChip,
                isStreaming = state.isStreaming,
                showDiscardRecovery = state.durableRecoveryBlocked,
                enabled = !state.durableRecoveryBlocked &&
                    (modelSetupRequired || (state.sessionReady && !state.sessionTransitioning)),
                modelSetupRequired = modelSetupRequired,
                onOpenModelSettings = onOpenModelSettings,
                modelProviderStatuses = modelProviderStatuses,
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

@Composable
private fun TopBar(
    title: String,
    isDark: Boolean,
    onOpenDrawer: () -> Unit,
    onToggleTheme: () -> Unit,
    onNewChat: () -> Unit,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp)
            .padding(top = 6.dp, bottom = 10.dp),
    ) {
        IconButton(
            name = LXIconName.Menu,
            color = t.text,
            size = 20.dp,
            onClick = onOpenDrawer,
            contentDescription = stringResource(R.string.chat_open_side_drawer),
            modifier = Modifier.testTag(UiTags.OPEN_DRAWER),
        )
        Spacer(Modifier.weight(1f))
        Text(
            text = title,
            color = t.text,
            fontSize = 14.5f.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.weight(1f))
        IconButton(
            name = if (isDark) LXIconName.Sun else LXIconName.Moon,
            color = t.text2,
            size = 18.dp,
            onClick = onToggleTheme,
            contentDescription = if (isDark) {
                stringResource(R.string.chat_toggle_theme_light)
            } else {
                stringResource(R.string.chat_toggle_theme_dark)
            },
        )
        IconButton(
            name = LXIconName.Edit,
            color = t.accent,
            size = 18.dp,
            onClick = onNewChat,
            contentDescription = stringResource(R.string.chat_new_chat),
        )
    }
}

/** A 38×38 tappable icon button (the iOS `iconButton` frame). */
@Composable
private fun IconButton(
    name: LXIconName,
    color: Color,
    size: Dp,
    onClick: () -> Unit,
    contentDescription: String? = null,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier.size(38.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(name = name, size = size, color = color, stroke = 1.8f, contentDescription = contentDescription)
    }
}

/** Stable key for the zero-height row that marks the end of the transcript. */
private const val TRANSCRIPT_BOTTOM_ANCHOR = "transcript.bottom-anchor"

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun MessageList(
    state: ChatState,
    items: List<ChatRenderItem>,
    listState: androidx.compose.foundation.lazy.LazyListState,
    onShare: (String) -> Unit = {},
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

    LazyColumn(
        state = listState,
        modifier = modifier
            .fillMaxWidth()
            .imeNestedScroll(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            start = 16.dp, end = 16.dp, top = 18.dp, bottom = 8.dp,
        ),
    ) {
        // ONE list, ONE items() block: order and keys come from
        // buildChatRenderItems, which preserves the exact visual order and
        // stable keys of the old hand-stitched blocks.
        items(
            items = items,
            key = { it.key },
            contentType = { it.contentType },
        ) { item ->
            when (item) {
                ChatRenderItem.Empty -> EmptyState()
                is ChatRenderItem.Tools -> ToolGroupView(
                    TranscriptBlock.Tools(item.calls), state.expandedToolCalls, stableOnToggleToolCall,
                )
                is ChatRenderItem.Message -> MessageBubble(
                    message = item.message,
                    onShare = stableOnShare,
                    onOpenLink = stableOnOpenLink,
                    expandedToolCalls = state.expandedToolCalls,
                    onToggleToolCall = stableOnToggleToolCall,
                )
                is ChatRenderItem.Streaming -> MessageBubble(
                    message = item.message,
                    onShare = stableOnShare,
                    onOpenLink = stableOnOpenLink,
                    expandedToolCalls = state.expandedToolCalls,
                    onToggleToolCall = stableOnToggleToolCall,
                )
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
            .size(34.dp)
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

/** Hero shown for a brand-new (empty) chat. */
@Composable
private fun EmptyState() {
    val t = LingXiTheme.palette
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 420.dp)
            .padding(horizontal = 24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Box(
            modifier = Modifier
                .size(52.dp)
                .clip(RoundedCornerShape(15.dp))
                .background(Brush.linearGradient(listOf(t.accent, t.accent2)))
                .padding(bottom = 0.dp),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.Sparkle, size = 26.dp, color = Color.White, stroke = 1.8f)
        }
        Spacer(Modifier.size(18.dp))
        Text(
            stringResource(R.string.chat_start_new_conversation),
            color = t.text,
            fontSize = 21.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Spacer(Modifier.size(7.dp))
        Text(
            stringResource(R.string.chat_new_chat_hint),
            color = t.text4,
            fontSize = 14.sp,
            lineHeight = (14f * 1.5f).sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 260.dp),
        )
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
            .padding(horizontal = 20.dp, vertical = 6.dp)
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
            Text(title, color = color, fontSize = 12.5f.sp, fontWeight = FontWeight.Medium)
            if (detail.isNotEmpty()) {
                Text(detail, color = t.text4, fontSize = 10.5f.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
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

/**
 * A dim, single-line status row above the composer — surfaces engine tool
 * activity ("调用工具 …") and errors ("错误：…"). The Android analog of the iOS
 * `ConversationModel.statusLine`. Hidden when [ChatState.statusLine] is `null`.
 */
@Composable
private fun StatusRow(text: String) {
    val t = LingXiTheme.palette
    Text(
        text = text,
        color = t.text4,
        fontSize = 12.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier
            .testTag(UiTags.CHAT_STATUS)
            .fillMaxWidth()
            .padding(horizontal = 20.dp)
            .padding(bottom = 4.dp),
    )
}

/** An actionable configuration state, kept separate from runtime failures. */
@Composable
private fun ModelSetupBanner(onOpenModelSettings: () -> Unit) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .testTag(UiTags.MODEL_SETUP_BANNER)
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(top = 4.dp, bottom = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(t.accent.tint(0.10f))
            .border(0.5.dp, t.accent.tint(0.35f), RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(28.dp)
                .clip(CircleShape)
                .background(t.accent.tint(0.16f)),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.Cog, size = 15.dp, color = t.accent, stroke = 2f)
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = stringResource(R.string.chat_model_not_configured_title),
                color = t.text,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = stringResource(R.string.chat_model_not_configured_detail),
                color = t.text2,
                fontSize = 12.5f.sp,
                lineHeight = (12.5f * 1.4f).sp,
            )
        }
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(3.dp),
            modifier = Modifier
                .clip(RoundedCornerShape(8.dp))
                .background(t.accent)
                .clickable(onClick = onOpenModelSettings)
                .testTag(UiTags.MODEL_SETUP_ACTION)
                .padding(horizontal = 10.dp, vertical = 7.dp),
        ) {
            Text(
                text = stringResource(R.string.chat_go_to_settings),
                color = Color.White,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            LXIcon(name = LXIconName.ChevronR, size = 11.dp, color = Color.White, stroke = 2f)
        }
    }
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
    val t = LingXiTheme.palette
    val dismissComputerUseSetupDescription = stringResource(R.string.chat_computer_use_dismiss)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        modifier = Modifier
            .testTag(UiTags.COMPUTER_USE_SETUP_BANNER)
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(top = 4.dp, bottom = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(t.accent.tint(0.10f))
            .border(0.5.dp, t.accent.tint(0.35f), RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(28.dp)
                .clip(CircleShape)
                .background(t.accent.tint(0.16f)),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.Sparkle,
                size = 15.dp,
                color = t.accent,
                stroke = 2f,
            )
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = stringResource(R.string.chat_computer_use_unavailable_title),
                color = t.text,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = stringResource(R.string.chat_computer_use_unavailable_detail, missingStepsLabel(status)),
                color = t.text2,
                fontSize = 12.5f.sp,
                lineHeight = (12.5f * 1.4f).sp,
            )
        }
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(3.dp),
            modifier = Modifier
                .clip(RoundedCornerShape(8.dp))
                .background(t.accent)
                .clickable(onClick = onOpenComputerUseSettings)
                .semantics { role = Role.Button }
                .testTag(UiTags.COMPUTER_USE_SETUP_ACTION)
                .heightIn(min = 48.dp)
                .padding(horizontal = 10.dp, vertical = 7.dp),
        ) {
            Text(
                text = stringResource(R.string.chat_go_enable),
                color = Color.White,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            LXIcon(
                name = LXIconName.ChevronR,
                size = 11.dp,
                color = Color.White,
                stroke = 2f,
            )
        }
        Box(
            modifier = Modifier
                .size(48.dp)
                .clip(CircleShape)
                .clickable(onClick = onDismiss)
                .semantics {
                    contentDescription = dismissComputerUseSetupDescription
                    role = Role.Button
                }
                .testTag(UiTags.COMPUTER_USE_SETUP_DISMISS),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(
                name = LXIconName.X,
                size = 15.dp,
                color = t.text3,
                stroke = 1.9f,
            )
        }
    }
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
            .padding(horizontal = 14.dp)
            .padding(top = 4.dp, bottom = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(t.danger.tint(0.12f))
            .border(0.5.dp, t.danger.tint(0.4f), RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(18.dp)
                .clip(CircleShape)
                .background(t.danger),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(name = LXIconName.X, size = 11.dp, color = Color.White, stroke = 2.4f)
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = headline,
                color = t.danger,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = error.message,
                color = t.text2,
                fontSize = 12.5f.sp,
                lineHeight = (12.5f * 1.4f).sp,
                maxLines = 4,
                overflow = TextOverflow.Ellipsis,
            )
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
