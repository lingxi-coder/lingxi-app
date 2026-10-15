package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.AudioSetupGuidance

@Composable
internal fun AudioSetupBanner(
    guidance: AudioSetupGuidance,
    onConfigure: () -> Unit,
    onDismiss: () -> Unit,
) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp)
            .clip(shape).background(t.accent.tint(0.08f)).border(0.5.dp, t.accent.tint(0.25f), shape)
            .padding(horizontal = 12.dp, vertical = 10.dp)
            .testTag("conversation.audio-setup")
            .semantics { liveRegion = LiveRegionMode.Polite },
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            LXIcon(name = LXIconName.Mic, size = 18.dp, color = t.accent)
            Text(stringResource(guidance.titleRes), color = t.text, fontSize = androidx.compose.material3.MaterialTheme.typography.bodyMedium.fontSize, fontWeight = FontWeight.SemiBold)
        }
        Text(stringResource(guidance.detailRes), color = t.text2, fontSize = androidx.compose.material3.MaterialTheme.typography.bodySmall.fontSize,
            lineHeight = 18.sp, modifier = Modifier.padding(top = 5.dp))
        Row(horizontalArrangement = Arrangement.End, modifier = Modifier.fillMaxWidth()) {
            TextButton(onClick = onDismiss, modifier = Modifier.testTag("conversation.audio-setup.dismiss")) {
                Text(stringResource(R.string.audio_guide_dismiss), color = t.text3)
            }
            TextButton(onClick = onConfigure, modifier = Modifier.testTag("conversation.audio-setup.configure")) {
                Text(stringResource(R.string.audio_guide_settings), color = t.accent)
            }
        }
    }
}
