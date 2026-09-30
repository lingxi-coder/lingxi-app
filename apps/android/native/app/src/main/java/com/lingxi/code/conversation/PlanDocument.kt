package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme

internal fun toolPlanMarkdown(tool: String, input: String): String? {
    if (tool.lowercase().replace("_", "") != "exitplanmode") return null
    return extractJsonStringValue(input, "plan")?.takeIf { it.isNotBlank() }
}

internal data class PlanTextPart(val text: String, val plan: Boolean = false, val writing: Boolean = false)

/** Only standalone protocol tags are recognized, never examples inside fenced code. */
internal fun planTextParts(text: String): List<PlanTextPart> {
    if (!text.contains("<proposed_plan>")) return listOf(PlanTextPart(text))
    val parts = mutableListOf<PlanTextPart>()
    val buffer = StringBuilder()
    var plan = false
    var fence: String? = null
    fun flush(writing: Boolean = false) {
        if (buffer.isNotBlank()) parts.add(PlanTextPart(buffer.toString().trim(), plan, writing))
        buffer.clear()
    }
    text.lineSequence().forEach { line ->
        val trimmed = line.trim()
        val marker = when { trimmed.startsWith("```") -> "```"; trimmed.startsWith("~~~") -> "~~~"; else -> null }
        if (marker != null) fence = if (fence == marker) null else fence ?: marker
        when {
            fence == null && trimmed == "<proposed_plan>" && !plan -> { flush(); plan = true }
            fence == null && trimmed == "</proposed_plan>" && plan -> { flush(); plan = false }
            else -> buffer.append(line).append('\n')
        }
    }
    flush(plan)
    return parts
}

@Composable
internal fun PlanAwareText(markdown: String, onOpenLink: (String) -> Unit = {}) {
    val parts = remember(markdown) { planTextParts(markdown) }
    parts.forEach { part ->
        if (part.plan) PlanDocumentCard(part.text, part.writing, onOpenLink)
        else AIText(part.text, onOpenLink = onOpenLink)
    }
}

@Suppress("DEPRECATION")
@Composable
internal fun PlanDocumentCard(markdown: String, writing: Boolean, onOpenLink: (String) -> Unit = {}) {
    val palette = LingXiTheme.palette
    var open by rememberSaveable { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    val label = stringResource(if (writing) R.string.chat_plan_document_writing else R.string.chat_plan_document_title)
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp))
        .border(1.dp, palette.border, RoundedCornerShape(16.dp))
        .clickable { open = true }.testTag("conversation.plan.preview").padding(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            LXIcon(LXIconName.Lightbulb, size = 18.dp, color = palette.text3)
            Text(label, color = palette.text3, fontSize = 13.sp)
        }
        Spacer(Modifier.height(20.dp))
        Box(Modifier.heightIn(max = 224.dp).clip(RoundedCornerShape(2.dp))) {
            AIText(markdown, onOpenLink = onOpenLink)
            Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(40.dp)
                .background(Brush.verticalGradient(listOf(Color.Transparent, palette.surface))))
        }
    }
    if (open) Dialog(onDismissRequest = { open = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Column(Modifier.fillMaxSize().background(palette.surface).safeDrawingPadding().padding(20.dp)
            .testTag("conversation.plan.detail")) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.chat_plan_document_title), modifier = Modifier.weight(1f), color = palette.text, fontSize = 18.sp)
                Box(Modifier.size(48.dp).clickable { clipboard.setText(AnnotatedString(markdown)) }, contentAlignment = Alignment.Center) {
                    LXIcon(LXIconName.Copy, size = 20.dp, color = palette.text3, contentDescription = stringResource(R.string.terminal_copy_button))
                }
                Box(Modifier.size(48.dp).clickable { open = false }, contentAlignment = Alignment.Center) {
                    LXIcon(LXIconName.X, size = 20.dp, color = palette.text3, contentDescription = stringResource(R.string.common_close))
                }
            }
            SelectionContainer(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                AIText(markdown, Modifier.fillMaxWidth().padding(top = 20.dp), onOpenLink)
            }
        }
    }
}
