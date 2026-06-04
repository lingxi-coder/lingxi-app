package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.Pill
import com.lingxi.code.components.UiTags
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
    onShare: (String) -> Unit = {},
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
                // Share affordance: surfaces the native chooser for this reply's
                // text through the same ShareController the engine bridges onto
                // `traits::SharingService` — so a bubble share and a `tool-share`
                // invocation are the identical launch path (mirrors how the
                // composer's camera affordance reuses CameraController).
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        modifier = Modifier
                            .size(28.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable { onShare(message.text) }
                            .testTag(UiTags.MESSAGE_SHARE),
                        contentAlignment = Alignment.Center,
                    ) {
                        LXIcon(name = LXIconName.Share, size = 15.dp, color = t.text3, stroke = 1.8f, contentDescription = "分享回复")
                    }
                }
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
 * Renders an assistant reply as Markdown. Beyond the prototype's `**bold**`, this
 * now renders fenced code blocks, inline code, and bullet / numbered lists — so
 * real coding replies are readable. The PURE parsing lives in `Markdown.kt`
 * ([parseMarkdownBlocks]); this composable only LAYS OUT the resulting blocks.
 */
@Composable
fun AIText(
    markdown: String,
    modifier: Modifier = Modifier,
) {
    val blocks = remember(markdown) { parseMarkdownBlocks(markdown) }
    Column(
        modifier = modifier,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        blocks.forEach { block -> MdBlockView(block) }
    }
}

/** Render one [MdBlock]. */
@Composable
private fun MdBlockView(block: MdBlock) {
    val t = LingXiTheme.palette
    when (block) {
        is MdBlock.Paragraph -> Text(
            text = inlineSpans(block.spans, t.surfaceHover, t.text2),
            color = t.text,
            fontSize = 15.5f.sp,
            lineHeight = (15.5f * 1.6f).sp,
        )

        is MdBlock.CodeBlock -> CodeBlockView(block)

        is MdBlock.BulletList -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEach { item ->
                ListRow(marker = "•", spans = item)
            }
        }

        is MdBlock.NumberedList -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEach { item ->
                ListRow(marker = item.marker, spans = item.spans)
            }
        }
    }
}

/** A single list row: a fixed-width marker gutter + the item's inline content. */
@Composable
private fun ListRow(marker: String, spans: List<MdInline>) {
    val t = LingXiTheme.palette
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
        Text(
            text = marker,
            color = t.text3,
            fontSize = 15.5f.sp,
            lineHeight = (15.5f * 1.6f).sp,
            modifier = Modifier.widthIn(min = 18.dp),
        )
        Text(
            text = inlineSpans(spans, t.surfaceHover, t.text2),
            color = t.text,
            fontSize = 15.5f.sp,
            lineHeight = (15.5f * 1.6f).sp,
            modifier = Modifier.weight(1f),
        )
    }
}

/** A fenced code block: a monospace surface with a subtle border + optional language label. */
@Composable
private fun CodeBlockView(block: MdBlock.CodeBlock) {
    val t = LingXiTheme.palette
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(t.surfaceHover)
            .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (block.language.isNotEmpty()) {
            Text(
                text = block.language,
                color = t.text4,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
            )
        }
        Text(
            text = block.code,
            color = t.text2,
            fontSize = 13.5f.sp,
            lineHeight = (13.5f * 1.5f).sp,
            fontFamily = FontFamily.Monospace,
        )
    }
}

/**
 * Build an [androidx.compose.ui.text.AnnotatedString] from [MdInline] spans:
 * `**bold**` → semibold, `` `code` `` → monospace on a tinted background.
 */
private fun inlineSpans(spans: List<MdInline>, codeBg: Color, codeColor: Color) =
    buildAnnotatedString {
        spans.forEach { span ->
            when (span) {
                is MdInline.Text -> append(span.text)
                is MdInline.Bold -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) {
                    append(span.text)
                }
                is MdInline.Code -> withStyle(
                    SpanStyle(
                        fontFamily = FontFamily.Monospace,
                        background = codeBg,
                        color = codeColor,
                        fontSize = 14.sp,
                    ),
                ) {
                    append(span.text)
                }
            }
        }
    }
