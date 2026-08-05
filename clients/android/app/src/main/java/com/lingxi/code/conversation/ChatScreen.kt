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
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
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
@Composable
fun ChatScreen(
    state: ChatState,
    onSend: (String) -> Unit,
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
    flowModePanel: (@Composable () -> Unit)? = null,
    draft: String = "",
    onDraftChange: (String) -> Unit = {},
    onCameraClick: () -> Unit = {},
    attachment: ComposerAttachment? = null,
    onRemoveAttachment: () -> Unit = {},
    onShare: (String) -> Unit = {},
    onStop: () -> Unit = {},
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
) {
    val t = LingXiTheme.palette
    val listState = rememberLazyListState()
    val followsLatest by remember {
        derivedStateOf {
            val layout = listState.layoutInfo
            val lastVisible = layout.visibleItemsInfo.lastOrNull()?.index
            layout.totalItemsCount == 0 ||
                (lastVisible != null && lastVisible >= layout.totalItemsCount - 3)
        }
    }

    // Follow new output only while the user is already at the bottom. Forcing
    // an animated jump from old history on every update is expensive on long
    // transcripts and prevents the user from reading earlier messages.
    LaunchedEffect(
        state.messages.size,
        state.streamingMessage?.id,
        state.shellTools.size,
        state.agentRun?.revision,
        state.streaming,
    ) {
        if (!followsLatest) return@LaunchedEffect
        val streamingMessageRows = if (state.streamingMessage != null) 1 else 0
        val activityRows = if (state.agentRun != null || state.streaming) 1 else 0
        val count = state.messages.size + streamingMessageRows + state.shellTools.size + activityRows
        if (count > 0) listState.scrollToItem(count - 1)
    }

    Box(modifier = modifier.fillMaxSize().background(t.windowBg)) {
        // Ambient radial glow at the top (non-interactive).
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 360.dp)
                .background(
                    Brush.radialGradient(
                        colors = listOf(t.ambient.top, t.ambient.bottom),
                        radius = 900f,
                    ),
                ),
        )

        Column(modifier = Modifier.fillMaxSize()) {
            TopBar(
                title = state.session.title,
                isDark = isDark,
                onOpenDrawer = onOpenDrawer,
                onToggleTheme = onToggleTheme,
                onNewChat = onNewChat,
            )
            MessageList(
                state = state,
                listState = listState,
                onShare = onShare,
                onOpenTerminal = onOpenTerminal,
                modifier = Modifier.weight(1f),
            )
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
            state.statusLine?.let { StatusRow(text = it) }
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
                        onSend(draft)
                        onDraftChange("")
                    }
                },
                onMicClick = onMicClick,
                onMicHoldStart = onMicHoldStart,
                onMicHoldRelease = onMicHoldRelease,
                onFlowModeClick = onFlowModeClick,
                flowModeActive = flowModeActive,
                onCameraClick = onCameraClick,
                attachment = attachment,
                onRemoveAttachment = onRemoveAttachment,
                isStreaming = state.isStreaming,
                enabled = modelSetupRequired || (state.sessionReady && !state.sessionTransitioning),
                modelSetupRequired = modelSetupRequired,
                onOpenModelSettings = onOpenModelSettings,
                modelProviderStatuses = modelProviderStatuses,
                onOpenProviderSettings = onOpenProviderSettings,
                onStop = onStop,
            )
        }
    }
}

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

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun MessageList(
    state: ChatState,
    listState: androidx.compose.foundation.lazy.LazyListState,
    onShare: (String) -> Unit = {},
    onOpenTerminal: (sessionId: String, initCommand: String) -> Unit = { _, _ -> },
    modifier: Modifier = Modifier,
) {
    val currentOnShare by rememberUpdatedState(onShare)
    val stableOnShare = remember { { text: String -> currentOnShare(text) } }
    val currentOnOpenTerminal by rememberUpdatedState(onOpenTerminal)
    val currentSessionId by rememberUpdatedState(state.session.id)
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
        if (state.isNew && state.messages.isEmpty() && !state.streaming) {
            item(key = "empty") { EmptyState() }
        }
        items(
            items = state.messages,
            key = { it.id },
            contentType = { "message" },
        ) { m ->
            MessageBubble(
                message = m,
                onShare = stableOnShare,
                onOpenLink = stableOnOpenLink,
            )
        }
        state.streamingMessage?.let { message ->
            item(key = message.id, contentType = "message") {
                MessageBubble(
                    message = message,
                    onShare = stableOnShare,
                    onOpenLink = stableOnOpenLink,
                )
            }
        }
        items(state.shellTools, key = { "shell-${it.taskId}" }) { shell ->
            ShellToolCard(
                state = shell,
                onOpenTerminal = onOpenTerminal,
                modifier = Modifier.padding(vertical = 4.dp),
            )
        }
        state.agentRun?.let { run ->
            item(key = "agent-run-${run.turnId}") {
                AgentRunTimeline(
                    state = run,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            }
        }
        if (state.streaming && state.agentRun == null) {
            item(key = "streaming") { StreamingRow() }
        }
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
