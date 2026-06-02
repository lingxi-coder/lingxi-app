package com.lingxi.code.conversation

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.ModelOption
import com.lingxi.code.theme.LingXiTheme

/**
 * The conversation surface — the Android analog of the iOS `ChatView`.
 *
 * Composes the top bar (menu / title / theme-toggle / new-chat), the
 * [WorkflowBar], a scrolling message list (with the streaming "thinking" dots
 * row and the new-chat empty state), and the [Composer]. The conversation
 * itself is owned by [state]; user intents are hoisted to the caller's
 * [ChatViewModel] via the callbacks.
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
) {
    val t = LingXiTheme.palette
    var draft by rememberSaveable { mutableStateOf("") }
    val listState = rememberLazyListState()

    // Auto-scroll to the latest turn / when streaming toggles (mirrors the iOS
    // ScrollViewReader.scrollTo("bottom")).
    LaunchedEffect(state.messages.size, state.streaming) {
        val count = state.messages.size + if (state.streaming) 1 else 0
        if (count > 0) listState.animateScrollToItem(count - 1)
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
            WorkflowBar()
            MessageList(
                state = state,
                listState = listState,
                modifier = Modifier.weight(1f),
            )
            Composer(
                text = draft,
                onTextChange = { draft = it },
                model = state.model,
                onModelChange = onSelectModel,
                onSend = {
                    onSend(draft)
                    draft = ""
                },
                onMicClick = onMicClick,
                onMicHoldStart = onMicHoldStart,
                onMicHoldRelease = onMicHoldRelease,
            )
        }
    }
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
        IconButton(name = LXIconName.Menu, color = t.text, size = 20.dp, onClick = onOpenDrawer)
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
        )
        IconButton(name = LXIconName.Edit, color = t.accent, size = 18.dp, onClick = onNewChat)
    }
}

/** A 38×38 tappable icon button (the iOS `iconButton` frame). */
@Composable
private fun IconButton(name: LXIconName, color: Color, size: Dp, onClick: () -> Unit) {
    Box(
        modifier = Modifier.size(38.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(name = name, size = size, color = color, stroke = 1.8f)
    }
}

@Composable
private fun MessageList(
    state: ChatState,
    listState: androidx.compose.foundation.lazy.LazyListState,
    modifier: Modifier = Modifier,
) {
    LazyColumn(
        state = listState,
        modifier = modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            start = 16.dp, end = 16.dp, top = 18.dp, bottom = 8.dp,
        ),
    ) {
        if (state.isNew && state.messages.isEmpty() && !state.streaming) {
            item(key = "empty") { EmptyState() }
        }
        items(state.messages, key = { it.id }) { m ->
            MessageBubble(message = m)
        }
        if (state.streaming) {
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
        Text("开启新对话", color = t.text, fontSize = 21.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.size(7.dp))
        Text(
            "随便说点什么，或按住屏幕进入语音心流模式。",
            color = t.text4,
            fontSize = 14.sp,
            lineHeight = (14f * 1.5f).sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 260.dp),
        )
    }
}

/** The pulsing three-dot row shown while the assistant reply streams. */
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
