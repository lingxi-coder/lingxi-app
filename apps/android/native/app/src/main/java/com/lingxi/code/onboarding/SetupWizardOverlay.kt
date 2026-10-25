package com.lingxi.code.onboarding

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.voice.OrbCanvas
import com.lingxi.code.voice.OrbPhase

// A single welcome page, mirroring iOS. [onFinish] only marks onboarding complete.

@Composable
fun SetupWizardOverlay(
    visible: Boolean,
    onFinish: () -> Unit,
    modifier: Modifier = Modifier,
) {
    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(tween(300)),
        exit = fadeOut(tween(300)),
        modifier = modifier,
    ) {
        SetupWizardContent(onFinish = onFinish)
    }
}

@Composable
private fun SetupWizardContent(onFinish: () -> Unit) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background),
    ) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars)) {
            // body
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 30.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                OrbCanvas(phase = OrbPhase.Idle, modifier = Modifier.fillMaxWidth().height(220.dp), cyFrac = 0.5f)
                WizH(stringResource(R.string.onboarding_welcome_title))
                WizSub(stringResource(R.string.onboarding_welcome_subtitle))
            }

            // footer
            Column(Modifier.padding(horizontal = 30.dp).padding(top = 14.dp, bottom = 42.dp)) {
                androidx.compose.material3.Button(
                    onClick = onFinish,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
                ) {
                    Text(stringResource(R.string.onboarding_cta_finish), style = MaterialTheme.typography.labelLarge)
                    Spacer(Modifier.width(8.dp))
                    LXIcon(LXIconName.ArrowRight,
                        color = MaterialTheme.colorScheme.onPrimary)
                }
            }
        }
    }
}

@Composable
private fun WizH(text: String) {
    Text(text, color = MaterialTheme.colorScheme.onSurface, style = androidx.compose.material3.MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold,
        textAlign = TextAlign.Center)
    Spacer(Modifier.height(10.dp))
}

@Composable
private fun WizSub(text: String) {
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = androidx.compose.material3.MaterialTheme.typography.bodyMedium, lineHeight = 23.sp,
        textAlign = TextAlign.Center, modifier = Modifier.widthIn(max = 290.dp))
    Spacer(Modifier.height(26.dp))
}
