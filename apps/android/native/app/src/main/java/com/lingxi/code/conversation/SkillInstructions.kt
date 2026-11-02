package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.R
import org.json.JSONObject

internal fun parseSkillBody(tool: String, json: String): String? =
    if (!tool.equals("Skill", ignoreCase = true)) null else runCatching {
        JSONObject(json).optString("body").takeIf { it.isNotBlank() }
    }.getOrNull()

internal fun matchesSkillInstructions(call: ToolCallUi, text: String): Boolean {
    if (call.status != AgentToolStatus.Completed || !call.tool.equals("Skill", ignoreCase = true)) return false
    val body = call.skillBody?.trim().orEmpty()
    val prompt = text.replace(Regex("^Base directory for this skill: [^\\n]*\\r?\\n\\r?\\n"), "").trim()
    return (body.isNotEmpty() && (prompt == body || prompt.startsWith("$body\n"))) ||
        (call.header?.primary == "local-app" && prompt.startsWith("# Desktop Local Apps\n"))
}

@Composable
internal fun SkillInstructionsView(name: String, text: String, expanded: Boolean, onToggle: () -> Unit, onOpenLink: (String) -> Unit) {
    val t = LingXiTheme.palette
    val state = stringResource(if (expanded) R.string.chat_run_collapse else R.string.chat_run_expand)
    Column {
        Row(Modifier.heightIn(min = 44.dp).clickable(role = Role.Button, onClick = onToggle)
            .semantics { stateDescription = state },
            verticalAlignment = Alignment.CenterVertically) {
            LXIcon(LXIconName.Skill, size = 16.dp, color = t.text3)
            Spacer(Modifier.width(8.dp))
            Text(name, color = t.text2)
        }
        if (expanded) AIText(markdown = text, modifier = Modifier.padding(start = 24.dp, bottom = 8.dp), onOpenLink = onOpenLink)
    }
}
