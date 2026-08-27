package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.Image
import androidx.compose.foundation.text.ClickableText
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.ImageRefDto
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
    onOpenLink: (String) -> Unit = {},
    /** Tool-use ids whose result body/diff is expanded — owned by `ChatState`. */
    expandedToolCalls: Set<String> = emptySet(),
    onToggleToolCall: (String) -> Unit = {},
) {
    val t = LingXiTheme.palette
    if (message.role == Role.User) {
        // A subagent's dispatch prompt arrives as the first USER bubble of its
        // child transcript and runs to thousands of characters. Same policy and
        // the same two strings as the assistant branch below.
        val isCollapsible = remember(message.text) {
            AssistantMessageCollapsePolicy.shouldCollapse(message.text)
        }
        var expanded by rememberSaveable(message.id) { mutableStateOf(false) }
        Column(
            modifier = modifier.padding(bottom = 22.dp),
            horizontalAlignment = Alignment.End,
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            if (message.images.isNotEmpty()) {
                AttachedImages(images = message.images)
            }
            Row(horizontalArrangement = Arrangement.End) {
                Spacer(Modifier.weight(1f))
                val shape = BubbleShape(topRightSharp = true)
                if (message.text.isNotBlank()) {
                    Text(
                        text = message.text,
                        color = t.text,
                        fontSize = 15.5f.sp,
                        lineHeight = (15.5f * 1.5f).sp,
                        // `maxLines`, not the assistant branch's `heightIn`: this
                        // bubble is one Text clipped to a BubbleShape, so a height
                        // clip would square off the rounded bottom corners.
                        // `maxLines` also gives a trailing ellipsis for free.
                        maxLines = if (isCollapsible && !expanded) {
                            AssistantMessageCollapsePolicy.COLLAPSED_LINE_LIMIT
                        } else {
                            Int.MAX_VALUE
                        },
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier
                            .widthIn(max = 300.dp)
                            .clip(shape)
                            .background(t.surface)
                            .border(0.5.dp, t.border, shape)
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                    )
                }
            }
            if (isCollapsible) {
                val toggleLabel = stringResource(
                    if (expanded) R.string.chat_run_collapse else R.string.chat_run_expand,
                )
                Row(
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable { expanded = !expanded }
                        .testTag("conversation.message.user.toggle")
                        .padding(horizontal = 6.dp, vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    LXIcon(
                        name = LXIconName.Chevron,
                        size = 11.dp,
                        color = t.accent,
                        stroke = 2f,
                        contentDescription = null,
                    )
                    Text(
                        text = toggleLabel,
                        color = t.accent,
                        fontSize = 11.5f.sp,
                        fontWeight = FontWeight.Medium,
                    )
                }
            }
        }
    } else {
        val isCollapsible = remember(message.text) {
            AssistantMessageCollapsePolicy.shouldCollapse(message.text)
        }
        var expanded by rememberSaveable(message.id) { mutableStateOf(false) }
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
                // A message the engine supplied structure for renders its blocks
                // IN ORDER — prose as Markdown, tool calls as their derived
                // header + `⎿` result. Everything else (a user turn, a message
                // still streaming) has no blocks and renders its text as before.
                Column(
                    modifier = if (isCollapsible && !expanded) {
                        Modifier.heightIn(max = 360.dp).clipToBounds()
                    } else {
                        Modifier
                    },
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (message.blocks.isEmpty()) {
                        AIText(markdown = message.text, onOpenLink = onOpenLink)
                    } else {
                        message.blocks.forEach { block ->
                            when (block) {
                                is MessageContent.Text ->
                                    AIText(markdown = block.text, onOpenLink = onOpenLink)
                                is MessageContent.Tool -> ToolCallView(
                                    call = block.call,
                                    expanded = block.call.id in expandedToolCalls,
                                    onToggleExpanded = { onToggleToolCall(block.call.id) },
                                )
                            }
                        }
                    }
                }
                // Share affordance: surfaces the native chooser for this reply's
                // text through the same ShareController the engine bridges onto
                // `traits::SharingService` — so a bubble share and a `tool-share`
                // invocation are the identical launch path (mirrors how the
                // composer's camera affordance reuses CameraController).
                Row(
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (isCollapsible) {
                        val toggleLabel = stringResource(
                            if (expanded) R.string.chat_run_collapse else R.string.chat_run_expand,
                        )
                        Row(
                            modifier = Modifier
                                .clip(RoundedCornerShape(8.dp))
                                .clickable { expanded = !expanded }
                                .testTag("conversation.message.assistant.toggle")
                                .padding(horizontal = 6.dp, vertical = 6.dp),
                            horizontalArrangement = Arrangement.spacedBy(4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            LXIcon(
                                name = LXIconName.Chevron,
                                size = 11.dp,
                                color = t.accent,
                                stroke = 2f,
                                contentDescription = null,
                            )
                            Text(
                                text = toggleLabel,
                                color = t.accent,
                                fontSize = 11.5f.sp,
                                fontWeight = FontWeight.Medium,
                            )
                        }
                    }
                    Box(
                        modifier = Modifier
                            .size(28.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .clickable { onShare(message.text) }
                            .testTag(UiTags.MESSAGE_SHARE),
                        contentAlignment = Alignment.Center,
                    ) {
                        LXIcon(
                            name = LXIconName.Share,
                            size = 15.dp,
                            color = t.text3,
                            stroke = 1.8f,
                            contentDescription = stringResource(R.string.chat_share_reply),
                        )
                    }
                }
            }
        }
    }
}

/** Render user media before the prompt, matching the CLI's UserImage → UserText order. */
@Composable
private fun AttachedImages(images: List<ImageRefDto>) {
    val t = LingXiTheme.palette
    Row(
        horizontalArrangement = Arrangement.spacedBy(7.dp),
        verticalAlignment = Alignment.Bottom,
        modifier = Modifier.widthIn(max = 300.dp),
    ) {
        images.forEachIndexed { index, image ->
            val bitmap = remember(image.mediaType, image.base64) {
                runCatching {
                    val bytes = android.util.Base64.decode(image.base64, android.util.Base64.DEFAULT)
                    android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
                }.getOrNull()
            }
            if (bitmap != null) {
                Image(
                    bitmap = bitmap,
                    contentDescription = "Attached image ${index + 1}",
                    contentScale = androidx.compose.ui.layout.ContentScale.Crop,
                    modifier = Modifier
                        .size(116.dp)
                        .clip(RoundedCornerShape(11.dp))
                        .background(t.surfaceActive)
                        .border(0.5.dp, t.border, RoundedCornerShape(11.dp)),
                )
            } else {
                Text(
                    text = "[Image #${index + 1}]",
                    color = t.text3,
                    fontSize = 11.sp,
                    modifier = Modifier
                        .clip(RoundedCornerShape(11.dp))
                        .background(t.surfaceActive)
                        .padding(horizontal = 10.dp, vertical = 12.dp),
                )
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
    onOpenLink: (String) -> Unit = {},
) {
    val blocks = remember(markdown) { parseMarkdownBlocks(markdown) }
    Column(
        modifier = modifier,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        blocks.forEach { block -> MdBlockView(block, onOpenLink) }
    }
}

/** Render one [MdBlock]. */
@Composable
private fun MdBlockView(block: MdBlock, onOpenLink: (String) -> Unit) {
    val t = LingXiTheme.palette
    when (block) {
        is MdBlock.Paragraph -> {
            val text = inlineSpans(block.spans, t.surfaceHover, t.text2, t.accent)
            ClickableText(
                text = text,
                style = androidx.compose.ui.text.TextStyle(
                    color = t.text,
                    fontSize = 15.5f.sp,
                    lineHeight = (15.5f * 1.6f).sp,
                ),
                onClick = { offset ->
                    text.getStringAnnotations("url", offset, offset)
                        .firstOrNull()
                        ?.let { onOpenLink(it.item) }
                },
            )
        }

        is MdBlock.CodeBlock -> CodeBlockView(block)

        is MdBlock.BulletList -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEach { item ->
                ListRow(marker = "•", spans = item, onOpenLink = onOpenLink)
            }
        }

        is MdBlock.NumberedList -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEach { item ->
                ListRow(marker = item.marker, spans = item.spans, onOpenLink = onOpenLink)
            }
        }
    }
}

/** A single list row: a fixed-width marker gutter + the item's inline content. */
@Composable
private fun ListRow(
    marker: String,
    spans: List<MdInline>,
    onOpenLink: (String) -> Unit,
) {
    val t = LingXiTheme.palette
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
        Text(
            text = marker,
            color = t.text3,
            fontSize = 15.5f.sp,
            lineHeight = (15.5f * 1.6f).sp,
            modifier = Modifier.widthIn(min = 18.dp),
        )
        val text = inlineSpans(spans, t.surfaceHover, t.text2, t.accent)
        ClickableText(
            text = text,
            style = androidx.compose.ui.text.TextStyle(
                color = t.text,
                fontSize = 15.5f.sp,
                lineHeight = (15.5f * 1.6f).sp,
            ),
            modifier = Modifier.weight(1f),
            onClick = { offset ->
                text.getStringAnnotations("url", offset, offset)
                    .firstOrNull()
                    ?.let { onOpenLink(it.item) }
            },
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
private fun inlineSpans(
    spans: List<MdInline>,
    codeBg: Color,
    codeColor: Color,
    linkColor: Color,
) =
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
                is MdInline.Link -> {
                    pushStringAnnotation(tag = "url", annotation = span.url)
                    withStyle(
                        SpanStyle(
                            color = linkColor,
                            textDecoration = TextDecoration.Underline,
                        ),
                    ) {
                        append(span.label)
                    }
                    pop()
                }
            }
        }
    }
