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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.draw.clip
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.VOICE_PACKS
import com.lingxi.code.voice.offline.VoicePack
import com.lingxi.code.voice.offline.VoiceModelDownloader
import androidx.compose.runtime.mutableFloatStateOf
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.oklch
import com.lingxi.code.voice.OrbCanvas
import com.lingxi.code.voice.OrbPhase
import kotlinx.coroutines.delay
import kotlin.math.min

// MARK: - First-run setup wizard (心流 onboarding)
//
// Port of the prototype's `SetupWizard` (lingxi-iphone.html), mirroring the iOS
// `SetupWizardView`. 6 steps over the sci-fi orb backdrop: welcome → name the
// assistant (wake word) → your name → enroll a voiceprint (optional, simulated)
// → choose an offline voice pack → finish. Provider/model selection is deliberately
// deferred to the real provider catalog in Settings rather than showing prototype
// model rows here. On finish it hands the chosen values back via
// [onFinish] (MainActivity persists them + flips setupDone).

private const val TOTAL = 6
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

    var vpState by remember { mutableStateOf(if (initialVoiceprint) VpState.Done else VpState.Idle) }
    var vpPct by remember { mutableFloatStateOf(if (initialVoiceprint) 100f else 0f) }
    LaunchedEffect(vpState) {
        if (vpState == VpState.Rec) {
            var elapsed = 0L
            while (elapsed < 2600) { delay(40); elapsed += 40; vpPct = min(100f, elapsed / 2600f * 100f) }
            vpPct = 100f; vpState = VpState.Done
        }
    }

    val ctaDisabled = when (step) {
        1 -> assistantName.isBlank()
        2 -> userName.isBlank()
        else -> false
    }
    val cta = when (step) {
        0 -> "开始设置"
        3 -> if (vpState == VpState.Done) "继续" else "稍后再说"
        TOTAL - 1 -> "进入灵犀"
        else -> "继续"
    }
    val skip = if (step == 3 && vpState != VpState.Done) "跳过此步" else null

    fun next() {
        if (step < TOTAL - 1) step += 1
        else onFinish(assistantName.trim(), userName.trim(), vpState == VpState.Done, modelId, voiceLang)
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
                            LXIcon(LXIconName.ChevronR, size = 17.dp, color = oklch(0.88f, 0.02f, 275f), stroke = 2f, contentDescription = "返回")
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
                        WizH("欢迎使用灵犀")
                        WizSub("花一分钟，让它认识你——之后你只需开口，它就会回应。")
                    }
                    1 -> {
                        Badge(LXIconName.Sparkle)
                        WizH("给你的灵犀起个名字")
                        WizSub("这会成为它的唤醒词。之后你可以说「嘿，${assistantName.ifBlank { "灵犀" }}」随时唤醒它。")
                        WizField(assistantName, "灵犀") { assistantName = it.take(12) }
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
                            Text("嘿，${assistantName.ifBlank { "灵犀" }}", color = oklch(0.88f, 0.04f, 285f), fontSize = 14.sp, fontWeight = FontWeight.Medium)
                        }
                    }
                    2 -> {
                        Badge(LXIconName.Skill)
                        WizH("我该怎么称呼你？")
                        WizSub("灵犀会用这个名字称呼你，让对话更自然亲切。")
                        WizField(userName, "你的名字") { userName = it.take(16) }
                    }
                    3 -> VoiceprintStep(vpState, vpPct, userName) { if (vpState != VpState.Rec) vpState = VpState.Rec }
                    4 -> VoicePackStep(
                        states = modelStates,
                        selected = voiceLang,
                        onSelect = { voiceLang = it },
                        onDownload = { lang -> voiceLang = lang; VoiceModelDownloader.startPack(lang) },
                    )
                    else -> {
                        Badge(LXIconName.Brain)
                        WizH("基础设置完成")
                        WizSub("进入应用后，请在「设置 → AI 提供商」保存真实凭据并选择默认模型。模型列表会直接来自当前已配置的提供商。")
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
                Box(Modifier.fillMaxWidth().heightIn(min = 20.dp).padding(top = 12.dp), contentAlignment = Alignment.Center) {
                    if (skip != null) {
                        Text(skip, color = oklch(0.60f, 0.03f, 275f), fontSize = 13.5.sp, fontWeight = FontWeight.Medium,
                            modifier = Modifier.clickable { step += 1 })
                    }
                }
            }
        }
    }
}

@Composable
private fun VoiceprintStep(vp: VpState, pct: Float, userName: String, onRecord: () -> Unit) {
    Badge(LXIconName.Mic)
    WizH("录入你的声纹")
    WizSub("让灵犀听声识人，只对你的声音响应、唤起属于你的记忆。可稍后在设置里完成。")
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
        VpState.Idle -> "轻点麦克风，朗读下面这句话"
        VpState.Rec -> "正在聆听你的声音… ${pct.toInt()}%"
        VpState.Done -> "✓ 声纹已录入"
    }
    Text(status, color = if (vp == VpState.Done) oklch(0.74f, 0.15f, 155f) else oklch(0.78f, 0.04f, 280f),
        fontSize = 14.sp, fontWeight = FontWeight.Medium)
    if (vp != VpState.Done) {
        Spacer(Modifier.height(8.dp))
        Text("「你好灵犀，我是${userName.ifBlank { "我" }}。」", color = oklch(0.86f, 0.03f, 280f), fontSize = 15.sp, fontStyle = FontStyle.Italic)
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
    WizH("选择语音包")
    WizSub("下载离线语音模型后，听写与朗读完全在本机进行、不依赖网络。可稍后在设置里更换或删除。")
    Column(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxWidth()) {
        VOICE_PACKS.forEach { pack ->
            VoicePackRow(
                pack = pack,
                agg = aggregatePackState(states, pack),
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
                Text("暂不下载", color = oklch(0.95f, 0.02f, 285f), fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
                Text("使用系统语音 · 可稍后在设置下载", color = oklch(0.66f, 0.03f, 280f), fontSize = 12.5.sp)
            }
            RadioDot(selected == "")
        }
    }
}

/** Combine the pack's STT+TTS model states into one aggregate for the row. */
private fun aggregatePackState(states: Map<String, ModelState>, pack: VoicePack): ModelState {
    val ms = pack.models.map { states[it.id] ?: ModelState.NotInstalled }
    ms.firstOrNull { it is ModelState.Failed }?.let { return it }
    if (ms.all { it is ModelState.Ready }) return ModelState.Ready
    if (ms.any { it is ModelState.Verifying }) return ModelState.Verifying
    if (ms.any { it is ModelState.Extracting }) return ModelState.Extracting
    if (ms.any { it is ModelState.Downloading }) {
        val total = pack.models.sumOf { it.approxSizeBytes }
        val bytes = pack.models.sumOf { m ->
            when (val s = states[m.id]) {
                is ModelState.Downloading -> s.bytes
                is ModelState.Ready -> m.approxSizeBytes
                else -> 0L
            }
        }
        return ModelState.Downloading(bytes, total)
    }
    return ModelState.NotInstalled
}

@Composable
private fun VoicePackRow(
    pack: VoicePack,
    agg: ModelState,
    selected: Boolean,
    onSelect: () -> Unit,
    onDownload: () -> Unit,
) {
    val sizeMb = pack.totalBytes / (1024 * 1024)
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
                Text("${pack.subtitle} · ≈ $sizeMb MB", color = oklch(0.66f, 0.03f, 280f), fontSize = 12.5.sp)
            }
            RadioDot(selected)
        }
        Spacer(Modifier.height(10.dp))
        when (agg) {
            is ModelState.Ready -> Text("✓ 已下载到本机", color = oklch(0.74f, 0.15f, 155f), fontSize = 13.sp, fontWeight = FontWeight.Medium)
            is ModelState.Verifying -> Text("校验中…", color = oklch(0.78f, 0.04f, 280f), fontSize = 13.sp)
            is ModelState.Extracting -> Text("解压中…", color = oklch(0.78f, 0.04f, 280f), fontSize = 13.sp)
            is ModelState.Downloading -> {
                val pct = if (agg.total > 0) (agg.bytes.toFloat() / agg.total).coerceIn(0f, 1f) else 0f
                Box(
                    Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(99.dp)).background(Color.White.copy(alpha = 0.1f)),
                ) {
                    Box(Modifier.fillMaxWidth(pct).height(6.dp).clip(RoundedCornerShape(99.dp)).background(oklch(0.66f, 0.2f, 288f)))
                }
                Spacer(Modifier.height(6.dp))
                Text("下载中… ${(pct * 100).toInt()}%（可继续，下载在后台进行）", color = oklch(0.78f, 0.04f, 280f), fontSize = 12.sp)
            }
            is ModelState.Failed -> DownloadBtn("下载失败：${agg.message} · 点此重试", oklch(0.65f, 0.2f, 25f), onDownload)
            ModelState.NotInstalled -> DownloadBtn("下载 (≈ $sizeMb MB)", oklch(0.66f, 0.2f, 288f), onDownload)
        }
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
