package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme

@Composable
private fun CommandSurface(title: String, maxHeight: Dp, onClose: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    val t = LingXiTheme.palette
    Column(Modifier.fillMaxWidth().padding(bottom = 8.dp).heightIn(max = maxHeight)
        .background(t.surface, RoundedCornerShape(16.dp))
        .border(.5.dp, t.borderStrong, RoundedCornerShape(16.dp))) {
        Row(Modifier.fillMaxWidth().padding(start = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(title, color = t.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            IconButton(onClick = onClose) { LXIcon(LXIconName.X, size = 18.dp, color = t.text3, contentDescription = stringResource(R.string.common_close)) }
        }
        content()
    }
}

@Composable
internal fun SlashCommandResultPanel(output: ConversationCommandOutput, maxHeight: Dp, onClose: () -> Unit) {
    val t = LingXiTheme.palette
    CommandSurface(output.command.ifBlank { stringResource(R.string.slash_command_output_title) }, maxHeight, onClose) {
        SelectionContainer(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())
            .padding(horizontal = 14.dp).semantics { liveRegion = if (output.isError) LiveRegionMode.Assertive else LiveRegionMode.Polite }
            .testTag("composer.command-result")) {
            Text(output.text, color = if (output.isError) t.danger else t.text2, fontSize = 13.sp)
        }
        Text(stringResource(R.string.slash_result_temporary), color = t.text3, fontSize = 11.sp, modifier = Modifier.padding(14.dp))
    }
}

@Composable
internal fun SlashCommandSuggestionPanel(commands: List<ConversationSlashCommand>, maxHeight: Dp, onSelect: (ConversationSlashCommand) -> Unit, onClose: () -> Unit) {
    val t = LingXiTheme.palette
    CommandSurface(stringResource(R.string.slash_commands_title), maxHeight, onClose) {
        LazyColumn(Modifier.weight(1f, fill = false).testTag("composer.slash-command-list"), contentPadding = PaddingValues(6.dp)) {
            items(commands, key = { it.name }) { command ->
                Column(Modifier.fillMaxWidth().clickable { onSelect(command) }.padding(horizontal = 10.dp, vertical = 8.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("/${command.name}", color = t.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        Text(stringResource(when (command.messagePlacement()) {
                            SlashMessagePlacement.Messages -> R.string.slash_placement_messages
                            SlashMessagePlacement.Temporary -> R.string.slash_placement_temporary
                            SlashMessagePlacement.Conditional -> R.string.slash_placement_conditional
                        }), color = t.text3, fontSize = 10.sp)
                    }
                    if (command.description.isNotBlank()) Text(command.description, color = t.text3, fontSize = 12.sp, maxLines = 2)
                    command.argumentHint?.let { Text(it, color = t.text4, fontSize = 11.sp, maxLines = 1) }
                }
            }
        }
    }
}
