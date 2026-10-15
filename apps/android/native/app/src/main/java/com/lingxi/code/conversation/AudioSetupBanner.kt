package com.lingxi.code.conversation

import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.voice.AudioSetupGuidance

@Composable
internal fun AudioSetupBanner(guidance: AudioSetupGuidance, onConfigure: () -> Unit, onDismiss: () -> Unit) {
    SetupNotice(
        title = stringResource(guidance.titleRes), detail = stringResource(guidance.detailRes),
        icon = LXIconName.Mic, action = stringResource(R.string.audio_guide_settings),
        onAction = onConfigure, tag = "conversation.audio-setup",
        actionTag = "conversation.audio-setup.configure", onDismiss = onDismiss,
        dismissLabel = stringResource(R.string.audio_guide_dismiss),
        dismissTag = "conversation.audio-setup.dismiss",
    )
}
