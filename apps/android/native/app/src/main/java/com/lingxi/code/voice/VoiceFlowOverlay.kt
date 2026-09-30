package com.lingxi.code.voice

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LingXiTheme
import kotlin.math.abs
import kotlin.math.sin

/**
 * VoiceFlowOverlay — the immersive "语音心流" recording surface, the Android
 * analog of the iOS `VoiceFlowView`.
 *
 * Per the prototype: holding the mic enters a full-screen immersive recording
 * state — the backdrop dims, three pulsing halo rings expand from a gradient
 * orb, a central waveform animates, and releasing the finger dismisses (the
 * "松开发送" action). Here we render it as a state-driven [AnimatedVisibility]
 * overlay so the parent owns `visible` (see [RootScreen]'s long-press wiring on
 * the mic); the dim + fade transition mirrors the iOS `.transition(.opacity)`.
 *
 * This is UI-shell only: it does not capture audio. [visible] is hoisted; the
 * caller flips it true on a mic long-press and false on release.
 *
 * @param visible whether the overlay is shown (driven by the held gesture).
 * @param modifier applied to the full-screen root.
 */
@Composable
fun VoiceFlowOverlay(
    visible: Boolean,
    modifier: Modifier = Modifier,
) {
    val captureState by VoiceCaptureStore.state.collectAsState()
    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(animationSpec = tween(durationMillis = 250)),
        exit = fadeOut(animationSpec = tween(durationMillis = 250)),
        modifier = modifier,
    ) {
        VoiceFlowContent(captureState = captureState)
    }
}

/**
 * The overlay's visual content — the dim backdrop plus the centered halo /
 * orb / waveform stack and the prompt labels. Kept separate from the
 * visibility wrapper so the infinite animations only spin while shown.
 */
@Composable
private fun VoiceFlowContent(captureState: VoiceCaptureUiState) {
    val t = LingXiTheme.palette

    Box(
        modifier = Modifier
            .fillMaxSize()
            // Dimmed backdrop (the iOS .ultraThinMaterial + black 0.35 stack;
            // approximated here with a heavy scrim that reads on both themes).
            .background(Color.Black.copy(alpha = if (t.isDark) 0.55f else 0.45f)),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            HaloOrb()
            Spacer(Modifier.height(28.dp))
            androidx.compose.material3.Text(
                text = captureState.statusText.ifBlank { stringResource(R.string.voice_hold_to_talk_hint) },
                color = Color.White,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(Modifier.height(6.dp))
            androidx.compose.material3.Text(
                text = captureState.partialTranscript.ifBlank { stringResource(R.string.voice_flow_release_hint) },
                color = Color.White.copy(alpha = 0.6f),
                fontSize = 13.sp,
            )
            captureState.errorMessage?.takeIf { it.isNotBlank() }?.let { message ->
                Spacer(Modifier.height(10.dp))
                androidx.compose.material3.Text(
                    text = message,
                    color = Color.White.copy(alpha = 0.75f),
                    fontSize = 12.sp,
                )
            }
        }
    }
}

/**
 * The pulsing halo rings + gradient orb + central [Waveform], mirroring the iOS
 * `ZStack` of three expanding `Circle().stroke(...)` rings over a gradient
 * `Circle` with a glow shadow.
 */
@Composable
private fun HaloOrb() {
    val t = LingXiTheme.palette
    val transition = rememberInfiniteTransition(label = "voice-halo")

    Box(
        modifier = Modifier.size(270.dp),
        contentAlignment = Alignment.Center,
    ) {
        // Three expanding rings, staggered by 0.4s, each scaling out + fading
        // (iOS: scaleEffect 0.9 → 1.0+i*0.04, opacity 0.6 → 0, 2s repeatForever).
        repeat(3) { i ->
            val progress by transition.animateFloat(
                initialValue = 0f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(
                    animation = tween(
                        durationMillis = 2000,
                        delayMillis = i * 400,
                        easing = LinearEasing,
                    ),
                    repeatMode = RepeatMode.Restart,
                ),
                label = "ring-$i",
            )
            val baseDp = 150 + i * 60
            val ringColor = t.accent.tint(0.4f).copy(alpha = (1f - progress) * 0.6f)
            Box(
                modifier = Modifier
                    .size(baseDp.dp)
                    .scale(0.9f + progress * (0.1f + i * 0.04f))
                    .drawRing(ringColor),
            )
        }

        // Soft accent glow behind the orb (stands in for the iOS colored shadow).
        Box(
            modifier = Modifier
                .size(196.dp)
                .background(
                    brush = Brush.radialGradient(
                        colors = listOf(t.accent.copy(alpha = 0.45f), Color.Transparent),
                    ),
                    shape = CircleShape,
                ),
        )

        // Gradient orb.
        Box(
            modifier = Modifier
                .size(132.dp)
                .clip(CircleShape)
                .background(
                    Brush.linearGradient(
                        colors = listOf(t.accent, t.accent2),
                        start = Offset.Zero,
                        end = Offset.Infinite,
                    ),
                ),
            contentAlignment = Alignment.Center,
        ) {
            Waveform()
        }
    }
}

/** Stroke a 1.5dp ring around a circular box (the iOS `Circle().stroke(...)`). */
private fun Modifier.drawRing(color: Color): Modifier =
    drawBehind {
        drawCircle(
            color = color,
            radius = size.minDimension / 2f - 0.75f.dp.toPx(),
            style = Stroke(width = 1.5.dp.toPx(), cap = StrokeCap.Round),
        )
    }

/**
 * Animated audio waveform of seven vertical capsule bars, ported from the iOS
 * `Waveform`: each bar's height follows `0.4 + 0.6 * |sin(t*3 + i*0.6)|`. A
 * single infinite phase drives the per-frame heights so the bars ripple.
 */
@Composable
private fun Waveform() {
    val transition = rememberInfiniteTransition(label = "voice-waveform")
    // A continuously advancing phase in radians (2π per ~2.1s ≈ the iOS t*3).
    val phase by transition.animateFloat(
        initialValue = 0f,
        targetValue = (2f * Math.PI).toFloat(),
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2100, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "phase",
    )

    val bars = 7
    val maxBarHeight = 40.dp
    val barWidth = 5.dp

    Canvas(modifier = Modifier.size(width = 70.dp, height = maxBarHeight)) {
        val gap = 5.dp.toPx()
        val w = barWidth.toPx()
        val totalWidth = bars * w + (bars - 1) * gap
        var x = (size.width - totalWidth) / 2f
        val maxH = maxBarHeight.toPx()
        for (i in 0 until bars) {
            val h = (0.4f + 0.6f * abs(sin(phase + i * 0.6f))) * maxH
            val top = (size.height - h) / 2f
            drawRoundRect(
                color = Color.White,
                topLeft = Offset(x, top),
                size = androidx.compose.ui.geometry.Size(w, h),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(w / 2f, w / 2f),
            )
            x += w + gap
        }
    }
}
