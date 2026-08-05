package com.lingxi.code.onboarding

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.imePadding
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.draw.clip
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.VOICE_PACKS
import com.lingxi.code.voice.offline.VoicePack
import com.lingxi.code.voice.offline.VoicePackProgress
import com.lingxi.code.voice.offline.VoiceModelDownloader
import com.lingxi.code.voice.offline.voicePackProgress
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.oklch
import com.lingxi.code.voice.OrbCanvas
import com.lingxi.code.voice.OrbPhase
import java.util.Locale

// MARK: - First-run setup wizard (心流 onboarding)
//
// Port of the prototype's `SetupWizard` (lingxi-iphone.html), mirroring the iOS
// `SetupWizardView`. The active Android flow has 5 steps over the sci-fi orb
// backdrop: welcome → name the assistant (wake word) → your name → choose an
// offline voice pack → finish. Voiceprint enrollment is temporarily excluded
// from onboarding; an existing voiceprint preference is preserved unchanged.
// Provider/model selection is deliberately deferred to the real provider catalog
// in Settings rather than showing prototype model rows here. On finish it hands
// the chosen values back via [onFinish] (MainActivity persists them + flips setupDone).

private const val TOTAL = 5
private enum class VpState { Idle, Rec, Done }

@Composable
fun SetupWizardOverlay(
    visible: Boolean,
    initialAssistantName: String,
    initialUserName: String,
    initialVoiceprint: Boolean,
    initialModelId: String,
    initialVoiceLang: String,
    onFinish: (assistantName: String, userName: String, voiceprint: Boolean, modelId: String, voiceLang: String) -> Unit,
    modifier: Modifier = Modifier,
) {
    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(tween(300)),
        exit = fadeOut(tween(300)),
        modifier = modifier,
    ) {
        SetupWizardContent(
            initialAssistantName, initialUserName, initialVoiceprint, initialModelId, initialVoiceLang, onFinish,
        )
    }
}

@Composable
private fun SetupWizardContent(
    initialAssistantName: String,
    initialUserName: String,
    initialVoiceprint: Boolean,
    initialModelId: String,
    initialVoiceLang: String,
    onFinish: (String, String, Boolean, String, String) -> Unit,
) {
    var step by remember { mutableIntStateOf(0) }
    var assistantName by remember { mutableStateOf(initialAssistantName) }
    var userName by remember { mutableStateOf(initialUserName) }
    var modelId by remember { mutableStateOf(initialModelId) }
    var voiceLang by remember { mutableStateOf(initialVoiceLang) }
    val modelStates by VoiceModelDownloader.states.collectAsState()

    // System back goes to the previous step instead of dismissing the whole
    // wizard (only step 0 lets back fall through to exit).
    BackHandler(enabled = step > 0) { step = (step - 1).coerceAtLeast(0) }

    val ctaDisabled = when (step) {
        1 -> assistantName.isBlank()
        2 -> userName.isBlank()
        else -> false
    }
    val cta = when (step) {
        0 -> stringResource(R.string.onboarding_cta_start)
        TOTAL - 1 -> stringResource(R.string.onboarding_cta_finish)
        else -> stringResource(R.string.onboarding_cta_continue)
    }

    fun next() {
        if (step < TOTAL - 1) step += 1
        else onFinish(assistantName.trim(), userName.trim(), initialVoiceprint, modelId, voiceLang)
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Brush.radialGradient(listOf(oklch(0.19f, 0.07f, 275f), Color(0xFF050509)))),
    ) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).imePadding()) {
            // header — back chevron + segmented progress
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 8.dp).heightIn(min = 40.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (step > 0) {
                    Box(
                        modifier = Modifier
                            .size(36.dp)
                            .background(Color.White.copy(alpha = 0.06f), CircleShape)
                            .border(0.5.dp, Color.White.copy(alpha = 0.12f), CircleShape)
                            .clickable { step = (step - 1).coerceAtLeast(0) },
                        contentAlignment = Alignment.Center,
                    ) {
                        // ChevronR mirrored → a left chevron (back).
                        Box(Modifier.rotate(180f)) {
                            LXIcon(
                                LXIconName.ChevronR,
                                size = 17.dp,
                                color = oklch(0.88f, 0.02f, 275f),
                                stroke = 2f,
                                contentDescription = stringResource(R.string.onboarding_back),
                            )
                        }
                    }
                } else {
                    Spacer(Modifier.size(36.dp))
                }
                Spacer(Modifier.width(12.dp))
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    repeat(TOTAL) { k ->
                        Box(
                            Modifier
                                .weight(1f)
                                .height(3.dp)
                                .background(
                                    if (k <= step) Brush.horizontalGradient(listOf(oklch(0.72f, 0.18f, 268f), oklch(0.70f, 0.18f, 318f)))
                                    else androidx.compose.ui.graphics.SolidColor(Color.White.copy(alpha = 0.12f)),
                                    RoundedCornerShape(99.dp),
                                ),
                        )
                    }
                }
                Spacer(Modifier.width(12.dp))
                Spacer(Modifier.size(36.dp))
            }

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
                when (step) {
                    0 -> {
                        OrbCanvas(phase = OrbPhase.Idle, modifier = Modifier.fillMaxWidth().height(220.dp), cyFrac = 0.5f)
                        WizH(stringResource(R.string.onboarding_welcome_title))
                        WizSub(stringResource(R.string.onboarding_welcome_subtitle))
                    }
                    1 -> {
                        Badge(LXIconName.Sparkle)
                        WizH(stringResource(R.string.onboarding_assistant_title))
                        val wakeWordName = assistantName.ifBlank { stringResource(R.string.app_name) }
                        WizSub(stringResource(R.string.onboarding_assistant_wake_hint, wakeWordName))
                        WizField(assistantName, stringResource(R.string.app_name)) { assistantName = it.take(12) }
                        Spacer(Modifier.height(18.dp))
                        Row(
                            modifier = Modifier
                                .background(oklch(0.70f, 0.18f, 285f, 0.14f), CircleShape)
                                .border(0.5.dp, oklch(0.70f, 0.18f, 285f, 0.3f), CircleShape)
                                .padding(horizontal = 16.dp, vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Canvas(Modifier.size(7.dp)) { drawCircle(oklch(0.72f, 0.18f, 150f)) }
                            Spacer(Modifier.width(8.dp))
                            Text(
                                stringResource(R.string.onboarding_wake_word_preview, wakeWordName),
                                color = oklch(0.88f, 0.04f, 285f),
                                fontSize = 14.sp,
                                fontWeight = FontWeight.Medium,
                            )
                        }
                    }
                    2 -> {
                        Badge(LXIconName.Skill)
                        WizH(stringResource(R.string.onboarding_user_title))
                        WizSub(stringResource(R.string.onboarding_user_subtitle))
                        WizField(userName, stringResource(R.string.onboarding_user_placeholder)) { userName = it.take(16) }
                    }
                    3 -> VoicePackStep(
                        states = modelStates,
                        selected = voiceLang,
                        onSelect = { voiceLang = it },
                        onDownload = { lang -> voiceLang = lang; VoiceModelDownloader.startPack(lang) },
                    )
                    else -> {
                        Badge(LXIconName.Brain)
                        WizH(stringResource(R.string.onboarding_done_title))
                        WizSub(stringResource(R.string.onboarding_done_subtitle_detail))
                    }
                }
            }

            // footer
            Column(Modifier.padding(horizontal = 30.dp).padding(top = 14.dp, bottom = 42.dp)) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(54.dp)
                        .background(
                            if (ctaDisabled) androidx.compose.ui.graphics.SolidColor(Color.White.copy(alpha = 0.1f))
                            else Brush.linearGradient(listOf(oklch(0.70f, 0.19f, 270f), oklch(0.66f, 0.20f, 305f))),
                            RoundedCornerShape(16.dp),
                        )
                        .clickable(enabled = !ctaDisabled) { next() },
                    horizontalArrangement = Arrangement.Center,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val fg = if (ctaDisabled) oklch(0.60f, 0.02f, 280f) else Color.White
                    Text(cta, color = fg, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.width(8.dp))
                    LXIcon(if (step == TOTAL - 1) LXIconName.Sparkle else LXIconName.ArrowRight, size = 17.dp, color = fg, stroke = 2f)
                }
            }
        }
    }
}

// Retained for the future Settings-based enrollment entry point; deliberately
// absent from the active onboarding step mapping above.
@Composable
private fun VoiceprintStep(vp: VpState, pct: Float, userName: String, onRecord: () -> Unit) {
    Badge(LXIconName.Mic)
    WizH(stringResource(R.string.onboarding_voiceprint_title))
    WizSub(stringResource(R.string.onboarding_voiceprint_subtitle))
    Box(Modifier.size(140.dp), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(108.dp)) {
            val sw = 4.dp.toPx()
            drawCircle(Color.White.copy(alpha = 0.1f), radius = size.minDimension / 2f - sw / 2f, style = Stroke(width = sw))
            drawArc(
                color = oklch(0.70f, 0.19f, 290f),
                startAngle = -90f,
                sweepAngle = 360f * (pct / 100f),
                useCenter = false,
                topLeft = Offset(sw / 2f, sw / 2f),
                size = Size(size.width - sw, size.height - sw),
                style = Stroke(width = sw, cap = StrokeCap.Round),
            )
        }
        Box(
            modifier = Modifier
                .size(88.dp)
                .background(
                    if (vp == VpState.Done) Brush.linearGradient(listOf(oklch(0.70f, 0.16f, 150f), oklch(0.64f, 0.16f, 165f)))
                    else Brush.linearGradient(listOf(oklch(0.66f, 0.20f, 270f), oklch(0.62f, 0.21f, 312f))),
                    CircleShape,
                )
                .clickable(enabled = vp != VpState.Rec, onClick = onRecord),
            contentAlignment = Alignment.Center,
        ) {
            LXIcon(if (vp == VpState.Done) LXIconName.Check else LXIconName.Mic, size = 34.dp, color = Color.White, stroke = 2f)
        }
    }
    Spacer(Modifier.height(20.dp))
    val status = when (vp) {
        VpState.Idle -> stringResource(R.string.onboarding_voiceprint_status_idle)
        VpState.Rec -> stringResource(R.string.onboarding_voiceprint_status_recording, pct.toInt())
        VpState.Done -> stringResource(R.string.onboarding_voiceprint_status_done)
    }
    Text(status, color = if (vp == VpState.Done) oklch(0.74f, 0.15f, 155f) else oklch(0.78f, 0.04f, 280f),
        fontSize = 14.sp, fontWeight = FontWeight.Medium)
    if (vp != VpState.Done) {
        Spacer(Modifier.height(8.dp))
        val speaker = userName.ifBlank { stringResource(R.string.onboarding_voiceprint_self_pronoun) }
        Text(
            stringResource(R.string.onboarding_voiceprint_sample_phrase, speaker),
            color = oklch(0.86f, 0.03f, 280f),
            fontSize = 15.sp,
            fontStyle = FontStyle.Italic,
        )
    }
}

@Composable
private fun Badge(icon: LXIconName) {
    Box(
        modifier = Modifier
            .size(66.dp)
            .background(Brush.linearGradient(listOf(oklch(0.66f, 0.20f, 270f), oklch(0.62f, 0.21f, 312f))), RoundedCornerShape(20.dp)),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(icon, size = 30.dp, color = Color.White, stroke = 1.9f)
    }
    Spacer(Modifier.height(22.dp))
}

@Composable
private fun WizH(text: String) {
    Text(text, color = oklch(0.97f, 0.02f, 285f), fontSize = 25.sp, fontWeight = FontWeight.Bold,
        textAlign = TextAlign.Center)
    Spacer(Modifier.height(10.dp))
}

@Composable
private fun WizSub(text: String) {
    Text(text, color = oklch(0.70f, 0.03f, 275f), fontSize = 14.5.sp, lineHeight = 23.sp,
        textAlign = TextAlign.Center, modifier = Modifier.widthIn(max = 290.dp))
    Spacer(Modifier.height(26.dp))
}

@Composable
private fun WizField(value: String, placeholder: String, onChange: (String) -> Unit) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        singleLine = true,
        textStyle = TextStyle(color = oklch(0.96f, 0.02f, 285f), fontSize = 21.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center),
        cursorBrush = androidx.compose.ui.graphics.SolidColor(oklch(0.85f, 0.15f, 290f)),
        modifier = Modifier
            .fillMaxWidth()
            .background(Color.White.copy(alpha = 0.05f), RoundedCornerShape(15.dp))
            .border(0.5.dp, Color.White.copy(alpha = 0.18f), RoundedCornerShape(15.dp))
            .padding(horizontal = 18.dp, vertical = 16.dp),
        decorationBox = { inner ->
            Box(contentAlignment = Alignment.Center, modifier = Modifier.fillMaxWidth()) {
                if (value.isEmpty()) {
                    Text(placeholder, color = oklch(0.50f, 0.02f, 285f), fontSize = 21.sp, fontWeight = FontWeight.SemiBold)
                }
                inner()
            }
        },
    )
}

// MARK: - Offline voice language-pack step ------------------------------------

@Composable
private fun VoicePackStep(
    states: Map<String, ModelState>,
    selected: String,
    onSelect: (String) -> Unit,
    onDownload: (String) -> Unit,
) {
    Badge(LXIconName.Mic)
    WizH(stringResource(R.string.onboarding_voice_pack_title))
    WizSub(stringResource(R.string.onboarding_voice_pack_subtitle))
    Column(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxWidth()) {
        VOICE_PACKS.forEach { pack ->
            VoicePackRow(
                pack = pack,
                progress = voicePackProgress(states, pack),
                selected = selected == pack.language,
                onSelect = { onSelect(pack.language) },
                onDownload = { onDownload(pack.language) },
            )
        }
        // "暂不下载" — keep the system voice; download later from settings.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(if (selected == "") oklch(0.70f, 0.18f, 285f, 0.16f) else Color.White.copy(alpha = 0.04f), RoundedCornerShape(15.dp))
                .border(1.dp, if (selected == "") oklch(0.70f, 0.18f, 285f, 0.55f) else Color.White.copy(alpha = 0.1f), RoundedCornerShape(15.dp))
                .clickable { onSelect("") }
                .padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.onboarding_voice_pack_skip), color = oklch(0.95f, 0.02f, 285f), fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
                Text(stringResource(R.string.onboarding_voice_pack_skip_detail), color = oklch(0.66f, 0.03f, 280f), fontSize = 12.5.sp)
            }
            RadioDot(selected == "")
        }
    }
}

@Composable
private fun VoicePackRow(
    pack: VoicePack,
    progress: VoicePackProgress,
    selected: Boolean,
    onSelect: () -> Unit,
    onDownload: () -> Unit,
) {
    val sizeText = formatDownloadSize(pack.totalBytes)
    val agg = progress.state
    val activeModel = progress.activeModel
    val activeLabel = activeModel?.localizedDisplayName("zh").orEmpty()
    val activePosition = if (activeModel != null && progress.activeModelIndex >= 0) {
        "（${progress.activeModelIndex + 1}/${pack.models.size}）"
    } else {
        ""
    }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (selected) oklch(0.70f, 0.18f, 285f, 0.16f) else Color.White.copy(alpha = 0.04f), RoundedCornerShape(15.dp))
            .border(1.dp, if (selected) oklch(0.70f, 0.18f, 285f, 0.55f) else Color.White.copy(alpha = 0.1f), RoundedCornerShape(15.dp))
            .clickable { onSelect() }
            .padding(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(pack.title, color = oklch(0.95f, 0.02f, 285f), fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
                Text("${pack.subtitle} · $sizeText", color = oklch(0.66f, 0.03f, 280f), fontSize = 12.5.sp)
            }
            RadioDot(selected)
        }
        Spacer(Modifier.height(10.dp))
        val activeModelLabel = "$activeLabel$activePosition"
        when (agg) {
            is ModelState.Ready -> Text(
                stringResource(R.string.onboarding_voice_pack_ready),
                color = oklch(0.74f, 0.15f, 155f),
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
            )
            is ModelState.Queued -> VoicePackProgressStatus(
                progress = progress,
                status = stringResource(R.string.onboarding_voice_pack_queued, activeModelLabel),
            )
            is ModelState.Verifying -> VoicePackProgressStatus(
                progress = progress,
                status = stringResource(R.string.onboarding_voice_pack_verifying, activeModelLabel),
            )
            is ModelState.Extracting -> {
                val next = progress.nextModel?.localizedDisplayName("zh")
                val extracting = stringResource(R.string.onboarding_voice_pack_extracting, activeModelLabel)
                val extractingNext = next?.let { stringResource(R.string.onboarding_voice_pack_extracting_next, it) }
                VoicePackProgressStatus(
                    progress = progress,
                    status = buildString {
                        append(extracting)
                        extractingNext?.let(::append)
                    },
                )
            }
            is ModelState.Downloading -> {
                val status = if (progress.downloadedBytes == 0L) {
                    stringResource(R.string.onboarding_voice_pack_connecting, activeModelLabel)
                } else {
                    stringResource(R.string.onboarding_voice_pack_downloading, activeModelLabel)
                }
                VoicePackProgressStatus(progress = progress, status = status)
            }
            is ModelState.Failed -> DownloadBtn(
                stringResource(R.string.onboarding_voice_pack_failed, activeModelLabel, agg.message),
                oklch(0.65f, 0.2f, 25f),
                onDownload,
            )
            ModelState.NotInstalled -> DownloadBtn(
                stringResource(R.string.onboarding_voice_pack_download_action, sizeText),
                oklch(0.66f, 0.2f, 288f),
                onDownload,
            )
        }
    }
}

@Composable
private fun VoicePackProgressStatus(
    progress: VoicePackProgress,
    status: String,
) {
    val pct = if (progress.totalBytes > 0L) {
        (progress.downloadedBytes.toFloat() / progress.totalBytes).coerceIn(0f, 1f)
    } else {
        0f
    }
    Box(
        Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(99.dp)).background(Color.White.copy(alpha = 0.1f)),
    ) {
        Box(
            Modifier.fillMaxWidth(pct).height(6.dp).clip(RoundedCornerShape(99.dp))
                .background(oklch(0.66f, 0.2f, 288f)),
        )
    }
    Spacer(Modifier.height(6.dp))
    Text(
        "$status · ${formatDownloadPercent(progress.downloadedBytes, progress.totalBytes)}" +
            "（${formatDownloadSize(progress.downloadedBytes)} / ${formatDownloadSize(progress.totalBytes)}）",
        color = oklch(0.78f, 0.04f, 280f),
        fontSize = 12.sp,
    )
}

private fun formatDownloadSize(bytes: Long): String {
    val kib = bytes.toDouble() / 1024.0
    val mib = kib / 1024.0
    val gib = bytes.toDouble() / (1024.0 * 1024.0 * 1024.0)
    return when {
        gib >= 1.0 -> String.format(Locale.US, "%.1f GB", gib)
        mib >= 1.0 -> String.format(Locale.US, "%.1f MB", mib)
        kib >= 1.0 -> String.format(Locale.US, "%.0f KB", kib)
        else -> "$bytes B"
    }
}

private fun formatDownloadPercent(bytes: Long, total: Long): String {
    if (total <= 0L) return "0%"
    val percent = (bytes.toDouble() / total * 100.0).coerceIn(0.0, 100.0)
    return when {
        percent < 0.1 -> String.format(Locale.US, "%.2f%%", percent)
        percent < 10.0 -> String.format(Locale.US, "%.1f%%", percent)
        else -> String.format(Locale.US, "%.0f%%", percent)
    }
}

@Composable
private fun RadioDot(on: Boolean) {
    Box(
        modifier = Modifier
            .size(22.dp)
            .background(if (on) oklch(0.66f, 0.2f, 288f) else Color.Transparent, CircleShape)
            .border(1.5.dp, if (on) oklch(0.70f, 0.18f, 285f) else Color.White.copy(alpha = 0.25f), CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (on) LXIcon(LXIconName.Check, size = 13.dp, color = Color.White, stroke = 3f)
    }
}

@Composable
private fun DownloadBtn(text: String, color: Color, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(11.dp))
            .background(color)
            .clickable { onClick() }
            .padding(vertical = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, color = Color.White, fontSize = 13.5.sp, fontWeight = FontWeight.SemiBold)
    }
}
