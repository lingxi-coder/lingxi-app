package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.Pill
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.theme.LingXiTheme

/**
 * A single conversation turn — ported from the iOS `MessageBubble`.
 *
 * User turns are right-aligned in a surface bubble with one sharpened corner;
 * assistant turns are left-aligned next to the gradient [AssistantAvatar], with
 * an optional "思考了 N 秒" thinking [Pill] above the (bold-markdown) text.
 *
 * @param dimmed fades the most-recent AI reply during voice flow
 *   ("上下文已记入"); wired by the voice overlay in a later phase.
 */
@Composable
fun MessageBubble(
    message: Message,
    modifier: Modifier = Modifier,
    dimmed: Boolean = false,
) {
    val t = LingXiTheme.palette
    if (message.role == Role.User) {
        Row(
            modifier = modifier.padding(bottom = 22.dp),
            horizontalArrangement = Arrangement.End,
        ) {
            Spacer(Modifier.weight(1f))
            val shape = BubbleShape(topRightSharp = true)
            Text(
                text = message.text,
                color = t.text,
                fontSize = 15.5f.sp,
                lineHeight = (15.5f * 1.5f).sp,
                modifier = Modifier
                    .widthIn(max = 300.dp)
                    .clip(shape)
                    .background(t.surface)
                    .border(0.5.dp, t.border, shape)
                    .padding(horizontal = 16.dp, vertical = 12.dp),
            )
        }
    } else {
        Row(
            modifier = modifier
                .padding(bottom = 26.dp)
                .graphicsLayer { alpha = if (dimmed) 0.4f else 1f },
            horizontalArrangement = Arrangement.spacedBy(11.dp),
            verticalAlignment = Alignment.Top,
        ) {
            AssistantAvatar()
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                message.tag?.let { Pill(text = it, color = t.accent) }
                AIText(markdown = message.text)
            }
        }
    }
}

/**
 * The AI bubble corner geometry: 18dp rounding with the top-right corner
 * sharpened to 6dp when [topRightSharp] (mirrors the iOS `BubbleShape`).
 */
@Suppress("FunctionName")
private fun BubbleShape(topRightSharp: Boolean): RoundedCornerShape {
    val big = 18.dp
    val small = 6.dp
    return RoundedCornerShape(
        topStart = big,
        topEnd = if (topRightSharp) small else big,
        bottomEnd = big,
        bottomStart = big,
    )
}

/** Gradient sparkle avatar shown beside every assistant turn. */
@Composable
fun AssistantAvatar(
    modifier: Modifier = Modifier,
    size: Dp = 30.dp,
    corner: Dp = 9.dp,
    glyph: Dp = 15.dp,
) {
    val t = LingXiTheme.palette
    Box(
        modifier = modifier
            .size(size)
            .clip(RoundedCornerShape(corner))
            .background(Brush.linearGradient(colors = listOf(t.accent, t.accent2))),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(name = LXIconName.Sparkle, size = glyph, color = Color.White, stroke = 2f)
    }
}

/**
 * Renders the assistant text with `**bold**` spans — the only markdown the
 * prototype uses — preserving paragraph breaks. Mirrors the iOS `AIText`
 * `AttributedString` parser.
 */
@Composable
fun AIText(
    markdown: String,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Text(
        text = parseBoldMarkdown(markdown),
        color = t.text,
        fontSize = 15.5f.sp,
        lineHeight = (15.5f * 1.6f).sp,
        modifier = modifier,
    )
}

/** Parse `**bold**` inline spans into an [androidx.compose.ui.text.AnnotatedString]. */
private fun parseBoldMarkdown(s: String) = buildAnnotatedString {
    var i = 0
    while (i < s.length) {
        val open = s.indexOf("**", i)
        if (open < 0) {
            append(s.substring(i))
            break
        }
        append(s.substring(i, open))
        val close = s.indexOf("**", open + 2)
        if (close < 0) {
            // Unterminated — emit the rest literally.
            append(s.substring(open))
            break
        }
        withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) {
            append(s.substring(open + 2, close))
        }
        i = close + 2
    }
}
