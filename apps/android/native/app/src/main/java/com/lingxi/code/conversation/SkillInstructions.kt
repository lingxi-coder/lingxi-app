package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.R
import org.json.JSONObject

internal fun parseSkillBody(tool: String, json: String): String? =
    if (!tool.equals("Skill", ignoreCase = true)) null else runCatching {
        JSONObject(json).optString("body").takeIf { it.isNotBlank() }
    }.getOrNull()

private val skillDirectoryPreamble = Regex("^Base directory for this skill: [^\\n]*\\n(?:Resolve relative references and assets from this directory; use their exact paths instead of searching the application home directory\\. Use read-only file tools for skill assets; keep project writes in the project directory\\.\\n)?\\n")
private fun normalizeSkillNewlines(text: String) = text.replace("\r\n", "\n").replace('\r', '\n')

internal fun skillInstructionText(text: String): String =
    normalizeSkillNewlines(text).replace(skillDirectoryPreamble, "").trim()

internal fun matchesSkillInstructions(call: ToolCallUi, text: String): Boolean {
    if (call.status != AgentToolStatus.Completed || !call.tool.equals("Skill", ignoreCase = true)) return false
    val body = call.skillBody?.let(::normalizeSkillNewlines)?.trim().orEmpty()
    val prompt = skillInstructionText(text)
    return (body.isNotEmpty() && (prompt == body || prompt.startsWith("$body\n"))) ||
        (call.header?.primary == "local-app" && (prompt.startsWith("# Desktop Local Apps\n") || prompt.startsWith("# Local apps\n")))
}

@Composable
internal fun SkillInstructionsView(name: String, text: String, expanded: Boolean, onToggle: () -> Unit, onOpenLink: (String) -> Unit) {
    val t = LingXiTheme.palette
    val state = stringResource(if (expanded) R.string.chat_run_collapse else R.string.chat_run_expand)
    Column {
        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onToggle)
            .semantics { stateDescription = state }.testTag("conversation.skill-instructions.toggle"),
            verticalAlignment = Alignment.CenterVertically) {
            LXIcon(LXIconName.Skill, size = 16.dp, color = t.text3)
            Spacer(Modifier.width(8.dp))
            Text(name, modifier = Modifier.weight(1f, fill = false), color = if (expanded) t.text else t.text2,
                fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.width(8.dp))
            LXIcon(if (expanded) LXIconName.Chevron else LXIconName.ChevronR, size = 12.dp, color = t.text3)
        }
        if (expanded) ToolDetailPanel(label = name, modifier = Modifier.testTag("conversation.skill-instructions.body")) {
            AIText(markdown = skillInstructionText(text), compact = true, onOpenLink = onOpenLink)
        }
    }
}
