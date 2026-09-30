package com.lingxi.code.conversation

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
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
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.layout.width
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ImageRefDto
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
 * User turns are right-aligned in a uniformly rounded surface bubble;
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
                val shape = RoundedCornerShape(18.dp)
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
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {

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
                        PlanAwareText(markdown = message.text, onOpenLink = onOpenLink)
                    } else {
                        transcriptBlocks(message.blocks).forEach { block ->
                            when (block) {
                                is TranscriptBlock.Prose -> PlanAwareText(block.text, onOpenLink = onOpenLink)
                                is TranscriptBlock.Plan -> PlanDocumentCard(block.markdown, block.writing, onOpenLink)
                                is TranscriptBlock.Tools -> ToolGroupView(block, expandedToolCalls, onToggleToolCall)
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
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AttachedImages(images: List<ImageRefDto>) {
    val t = LingXiTheme.palette
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(7.dp),
        verticalArrangement = Arrangement.spacedBy(7.dp),
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

        is MdBlock.Table -> MarkdownTable(block, onOpenLink)
        is MdBlock.Heading -> Text(
            inlineSpans(block.spans, t.surfaceHover, t.text2, t.accent),
            color = t.text, fontWeight = FontWeight.SemiBold,
            fontSize = when (block.level) { 1 -> 25.sp; 2 -> 21.sp; else -> 18.sp },
            modifier = Modifier.padding(top = 8.dp, bottom = 3.dp),
        )
        is MdBlock.Quote -> Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.size(width = 3.dp, height = 24.dp).background(t.border))
            Text(inlineSpans(block.spans, t.surfaceHover, t.text2, t.accent),
                color = t.text3, fontSize = 15.sp, lineHeight = 23.sp)
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
    val clipboard = LocalClipboardManager.current
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
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = block.language.ifBlank { "text" },
                modifier = Modifier.weight(1f),
                color = t.text4,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
            )
            Box(Modifier.size(40.dp).clickable { clipboard.setText(AnnotatedString(block.code)) },
                contentAlignment = Alignment.Center) {
                LXIcon(LXIconName.Copy, size = 16.dp, color = t.text3, contentDescription = stringResource(R.string.terminal_copy_button))
            }
        }
        Text(
            text = block.code,
            modifier = Modifier.horizontalScroll(rememberScrollState()),
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
): androidx.compose.ui.text.AnnotatedString =
    buildAnnotatedString {
        spans.forEach { span ->
            when (span) {
                is MdInline.Text -> append(span.text)
                is MdInline.Bold -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) {
                    append(inlineSpans(parseInline(span.text), codeBg, codeColor, linkColor))
                }
                is MdInline.Italic -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) {
                    append(inlineSpans(parseInline(span.text), codeBg, codeColor, linkColor))
                }
                is MdInline.BoldItalic -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, fontStyle = FontStyle.Italic)) {
                    append(inlineSpans(parseInline(span.text), codeBg, codeColor, linkColor))
                }
                is MdInline.Strike -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) {
                    append(inlineSpans(parseInline(span.text), codeBg, codeColor, linkColor))
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

@Composable
private fun MarkdownTable(table: MdBlock.Table, onOpenLink: (String) -> Unit) {
    val palette = LingXiTheme.palette
    Column(Modifier.horizontalScroll(rememberScrollState()).border(0.5.dp, palette.border)) {
        (listOf(table.header) + table.rows).forEachIndexed { rowIndex, cells ->
            Row(Modifier.background(if (rowIndex == 0) palette.surfaceHover else Color.Transparent)) {
                cells.forEachIndexed { column, spans ->
                    val text = inlineSpans(spans, palette.surfaceHover, palette.text2, palette.accent)
                    ClickableText(text,
                        modifier = Modifier.width(180.dp).border(0.5.dp, palette.border).padding(10.dp),
                        style = androidx.compose.ui.text.TextStyle(
                            color = palette.text, fontSize = 14.sp, lineHeight = 22.sp,
                            fontWeight = if (rowIndex == 0) FontWeight.SemiBold else FontWeight.Normal,
                            textAlign = when (table.alignment[column]) {
                                TableAlignment.Left -> TextAlign.Start
                                TableAlignment.Center -> TextAlign.Center
                                TableAlignment.Right -> TextAlign.End
                            }),
                        onClick = { offset -> text.getStringAnnotations("url", offset, offset)
                            .firstOrNull()?.let { onOpenLink(it.item) } },
                    )
                }
            }
        }
    }
}
