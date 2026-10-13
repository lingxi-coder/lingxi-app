package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.modeColor
import com.lingxi.code.model.SessionMode
import com.lingxi.code.theme.LingXiTheme

/** The mode names, in the order the mode bar offers them. */
internal val NewChatModes: List<SessionMode> = listOf(SessionMode.Chat, SessionMode.Code)

@Composable
internal fun sessionModeLabel(mode: SessionMode): String = stringResource(
    when (mode) {
        SessionMode.Chat -> R.string.session_mode_chat
        SessionMode.Code -> R.string.session_mode_code
    },
)

/**
 * The new-chat page's mode bar, just above the composer: one tap starts a fresh
 * conversation in that mode in the current workspace (a mode never changes
 * under a running conversation — that is a fork). The capsule next to it shows
 * the workspace the conversation will run in and opens the workspace picker.
 */
@Composable
internal fun NewChatBar(
    mode: SessionMode,
    workspaceName: String,
    onSelectMode: (SessionMode) -> Unit,
    onPickWorkspace: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 14.dp, vertical = 6.dp)
            .testTag(UiTags.NEW_CHAT_BAR),
    ) {
        NewChatModes.forEach { option ->
            val tint = t.modeColor(option)
            val active = option == mode
            val shape = RoundedCornerShape(50)
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier
                    .clip(shape)
                    .background(if (active) tint.copy(alpha = 0.16f) else t.surface)
                    .border(0.5.dp, if (active) tint.copy(alpha = 0.45f) else t.border, shape)
                    .testTag(UiTags.newChatMode(option.wireKey))
                    .clickable { onSelectMode(option) }
                    .padding(horizontal = 14.dp, vertical = 8.dp),
            ) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(tint))
                Text(
                    text = sessionModeLabel(option),
                    color = if (active) tint else t.text2,
                    fontSize = 13.5f.sp,
                    fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                )
            }
        }
        val capsuleShape = RoundedCornerShape(50)
        val capsuleLabel = stringResource(R.string.new_chat_workspace_a11y, workspaceName)
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier
                .clip(capsuleShape)
                .border(0.5.dp, t.border, capsuleShape)
                .testTag(UiTags.NEW_CHAT_WORKSPACE)
                .semantics { contentDescription = capsuleLabel }
                .clickable(onClick = onPickWorkspace)
                .padding(horizontal = 12.dp, vertical = 8.dp),
        ) {
            LXIcon(name = LXIconName.Folder, size = 14.dp, color = t.text3, stroke = 1.7f)
            Text(
                text = workspaceName,
                color = t.text2,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(end = 2.dp),
            )
            LXIcon(name = LXIconName.Chevron, size = 12.dp, color = t.text4, stroke = 1.8f)
        }
    }
}
