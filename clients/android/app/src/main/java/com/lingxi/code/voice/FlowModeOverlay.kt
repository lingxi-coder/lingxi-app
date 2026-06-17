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
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import android.speech.tts.TextToSpeech
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.oklch
import java.util.Locale
import kotlinx.coroutines.launch
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin

// MARK: - FlowMode voice orb ("心流" — living LLM presence)
//
// Port of the prototype's `OrbCanvas` + `VoiceOrb` (lingxi-iphone.html), mirroring
// the iOS `VoiceOrbView`. A Canvas render loop (driven by `withFrameNanos`)
// paints a deep-space starfield, an orbiting particle field, a fluid blob body
// with a radial core + inner glow blobs + specular highlight, three wobbling
// membrane rings, and a thinking arc. Each phase eases toward a SUSTAINED size.
// `FlowModeOverlay` wraps it with the scripted flow (idle → listening → thinking
// → speaking), char-by-char captions, a status label, and an optional pop-up
// text input. Opened by a TAP on the composer mic.

enum class OrbPhase { Idle, Listening, Thinking, Speaking }
private class OrbParticle(var a: Float, val rad: Float, val sp: Float, val sz: Float, val tw: Float)
private class OrbStar(val x: Float, val y: Float, val z: Float, val tw: Float)

/** Deterministic RNG so the particle/star field is stable across launches. */
private class Lcg(var s: Long = 0x9E3779B97F4A7C15uL.toLong()) {
    fun next(): Float {
        s = s * 6364136223846793005L + 1442695040888963407L
        return ((s ushr 11).toDouble() / (1L shl 53).toDouble()).toFloat()
    }
}

/** Mutable per-frame simulation state (advanced in the frame loop, read in draw). */
private class OrbSim {
    var tm = 0f
    var amp = 0.12f
    var hueShift = 0f
    var baseScale = 1f
    var breath = 0f
    var scale = 1f
    val parts: List<OrbParticle>
    val stars: List<OrbStar>

    init {
        val r = Lcg()
        val tau = (PI * 2).toFloat()
        parts = List(76) {
            OrbParticle(
                a = r.next() * tau,
                rad = 0.55f + r.next() * 1.5f,
                sp = (0.1f + r.next() * 0.5f) * (if (r.next() < 0.5f) 1f else -1f),
                sz = 0.6f + r.next() * 1.8f,
                tw = r.next() * tau,
            )
        }
        stars = List(90) { OrbStar(r.next(), r.next(), 0.25f + r.next() * 0.75f, r.next() * tau) }
    }

    fun advance(dt: Float, phase: OrbPhase) {
        tm += dt
        val baseTarget: Float; val breathDepth: Float; val breathSpeed: Float; val wobTarget: Float
        when (phase) {
            OrbPhase.Idle ->      { baseTarget = 1.0f;  breathDepth = 0.03f;  breathSpeed = 0.9f; wobTarget = 0.10f }
            OrbPhase.Listening -> { baseTarget = 1.07f; breathDepth = 0.04f;  breathSpeed = 1.5f; wobTarget = 0.16f }
            OrbPhase.Thinking ->  { baseTarget = 1.04f; breathDepth = 0.03f;  breathSpeed = 1.2f; wobTarget = 0.13f; hueShift += dt * 40f }
            OrbPhase.Speaking ->  { baseTarget = 1.26f; breathDepth = 0.035f; breathSpeed = 1.8f; wobTarget = 0.22f }
        }
        baseScale += (baseTarget - baseScale) * min(1f, dt * 4f)
        breath += dt * breathSpeed
        scale = baseScale + sin(breath) * breathDepth
        amp += (wobTarget - amp) * min(1f, dt * 3f)
        val spd = if (phase == OrbPhase.Idle) 0.4f else 1f
        for (p in parts) p.a += p.sp * dt * spd
    }

    private fun blob(cx: Float, cy: Float, base: Float, wob: Float, seed: Float, harm: Float): Path {
        val tau = (PI * 2).toFloat()
        val steps = 132
        val p = Path()
        for (i in 0..steps) {
            val a = i.toFloat() / steps * tau
            val rr = base +
                sin(a * 3 + tm * 1.3f + seed) * wob * 0.5f +
                sin(a * 5 - tm * 1.9f + seed * 1.7f) * wob * 0.32f +
                sin(a * 2 + tm * 0.9f + seed * 0.4f) * wob * 0.42f +
                sin(a * 7 + tm * 2.4f) * wob * 0.16f * harm
            val x = cx + cos(a) * rr; val y = cy + sin(a) * rr
            if (i == 0) p.moveTo(x, y) else p.lineTo(x, y)
        }
        p.close()
        return p
    }

    fun draw(scope: DrawScope, phase: OrbPhase, cyFrac: Float) = with(scope) {
        val tau = (PI * 2).toFloat()
        val w = size.width; val h = size.height
        val cx = w / 2f; val cy = h * cyFrac
        val baseR = min(w, h) * 0.165f
        val r = baseR * scale
        val a = if (amp < 0f) 0f else amp

        // deep-space starfield
        for (s in stars) {
            val sx = ((s.x + tm * 0.004f * s.z) % 1f) * w
            val sy = s.y * h
            val twk = 0.35f + 0.65f * (0.5f + 0.5f * sin(tm * (1 + s.z * 2) + s.tw))
            val ss = if (s.z > 0.8f) 1.6f else 1f
            drawRect(oklch(0.78f + s.z * 0.18f, 0.03f, 250f + s.z * 60f, twk * (0.12f + s.z * 0.38f)),
                topLeft = Offset(sx, sy), size = Size(ss, ss))
        }

        // orbiting particle field (additive)
        for (pt in parts) {
            val rr = r * pt.rad * (if (phase == OrbPhase.Thinking) 0.6f else 1f) * (1 + a * 0.25f)
            val x = cx + cos(pt.a) * rr * 1.15f
            val y = cy + sin(pt.a) * rr
            val tw = 0.4f + 0.6f * (sin(tm * 2 + pt.tw) * 0.5f + 0.5f)
            val hue = 250f + 80f * (pt.rad - 0.55f) / 1.5f + hueShift
            drawCircle(oklch(0.85f, 0.16f, hue, tw * (0.5f + a * 0.4f)),
                radius = pt.sz * (0.7f + a * 0.5f), center = Offset(x, y), blendMode = BlendMode.Plus)
        }

        val wob = r * (0.05f + a * 0.24f)

        // blob body: clipped core gradient + inner glow blobs + specular
        val bodyPath = blob(cx, cy, r * 0.94f, wob, 0f, 1f)
        val coreH = 282f + 30f * sin(tm * 0.5f) + (if (phase == OrbPhase.Thinking) hueShift * 0.3f else 0f)
        clipPath(bodyPath) {
            drawRect(
                brush = Brush.radialGradient(
                    0f to oklch(0.97f, 0.04f, coreH, 0.95f),
                    0.32f to oklch(0.80f, 0.18f, coreH, 0.92f),
                    0.7f to oklch(0.58f, 0.21f, coreH + 20f, 0.7f),
                    1f to oklch(0.40f, 0.18f, coreH + 30f, 0.15f),
                    center = Offset(cx, cy - r * 0.28f), radius = r * 1.15f,
                ),
                topLeft = Offset(cx - r * 2, cy - r * 2), size = Size(r * 4, r * 4),
            )
            val glowHues = floatArrayOf(260f, 320f, 195f)
            for (k in 0..2) {
                val ya = cy + sin(tm * (1.1f + k * 0.5f) + k) * r * 0.4f * (0.4f + a)
                drawCircle(
                    brush = Brush.radialGradient(
                        0f to oklch(0.92f, 0.14f, glowHues[k] + hueShift, 0.18f + a * 0.22f),
                        1f to oklch(0.90f, 0.10f, 270f, 0f),
                        center = Offset(cx, ya), radius = r * 0.9f,
                    ),
                    radius = r * 0.9f, center = Offset(cx, ya), blendMode = BlendMode.Plus,
                )
            }
            drawCircle(oklch(0.99f, 0.02f, 270f, 0.5f + a * 0.3f),
                radius = r * 0.14f, center = Offset(cx - r * 0.28f, cy - r * 0.34f))
        }

        // wobbling membrane rings (additive)
        val ringH = floatArrayOf(268f, 322f, 196f)
        for (k in 0..2) {
            val path = blob(cx, cy, r * (1.0f + k * 0.07f), wob * (1 + k * 0.35f), k * 2.3f, 1f)
            drawPath(path, oklch(0.85f, 0.17f, ringH[k] + hueShift * 0.3f, 0.55f - k * 0.14f + a * 0.2f),
                style = Stroke(width = 1.6f - k * 0.45f), blendMode = BlendMode.Plus)
        }

        // thinking arc
        if (phase == OrbPhase.Thinking) {
            val st = tm * 3.2f
            val rad = r * 1.32f
            drawArc(
                color = oklch(0.88f, 0.16f, 260f + hueShift, 0.85f),
                startAngle = Math.toDegrees(st.toDouble()).toFloat(),
                sweepAngle = Math.toDegrees((PI * 1.1).toDouble()).toFloat(),
                useCenter = false,
                topLeft = Offset(cx - rad, cy - rad), size = Size(rad * 2, rad * 2),
                style = Stroke(width = 2f, cap = StrokeCap.Round),
                blendMode = BlendMode.Plus,
            )
        }
    }
}

/** The orb renderer — a frame-driven [Canvas] reading a persistent [OrbSim]. */
@Composable
fun OrbCanvas(phase: OrbPhase, modifier: Modifier = Modifier, cyFrac: Float = 0.40f) {
    val sim = remember { OrbSim() }
    val phaseState = rememberUpdatedState(phase)
    var tick by remember { mutableLongStateOf(0L) }
    LaunchedFrameLoop { now, dt ->
        sim.advance(dt, phaseState.value)
        tick = now
    }
    Canvas(modifier) {
        tick // read to invalidate each frame
        sim.draw(this, phaseState.value, cyFrac)
    }
}

/** Run [onFrame] every animation frame with the monotonic time + clamped delta. */
@Composable
private fun LaunchedFrameLoop(onFrame: (now: Long, dt: Float) -> Unit) {
    val cb = rememberUpdatedState(onFrame)
    androidx.compose.runtime.LaunchedEffect(Unit) {
        var last = 0L
        while (true) {
            withFrameNanos { now ->
                if (last == 0L) last = now
                val dt = ((now - last) / 1_000_000_000.0).coerceAtMost(0.05).toFloat()
                last = now
                cb.value(now, dt)
            }
        }
    }
}

// MARK: - FlowMode overlay

/**
 * The full-screen FlowMode overlay. [visible] is hoisted (composer mic tap flips
 * it on; the close button flips it off). [assistantName] labels the speaking
 * phase + text input; [inputDialog] gates the pop-up text-input affordance.
 */
@Composable
fun FlowModeOverlay(
    visible: Boolean,
    assistantName: String,
    inputDialog: Boolean,
    voiceLang: String,
    streaming: Boolean,
    assistantText: String,
    onSend: (String) -> Unit,
    onCancel: () -> Unit,
    onListen: (onResult: (String?) -> Unit) -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(tween(300)),
        exit = fadeOut(tween(300)),
        modifier = modifier,
    ) {
        FlowModeContent(assistantName, inputDialog, voiceLang, streaming, assistantText, onSend, onCancel, onListen, onClose)
    }
}

@Composable
private fun FlowModeContent(
    assistantName: String,
    inputDialog: Boolean,
    voiceLang: String,
    streaming: Boolean,
    assistantText: String,
    onSend: (String) -> Unit,
    onCancel: () -> Unit,
    onListen: (onResult: (String?) -> Unit) -> Unit,
    onClose: () -> Unit,
) {
    var phase by remember { mutableStateOf(OrbPhase.Idle) }
    var userCaption by remember { mutableStateOf("") }
    var didSend by remember { mutableStateOf(false) }
    var typing by remember { mutableStateOf(false) }
    var draft by remember { mutableStateOf("") }
    val name = assistantName.ifBlank { "灵犀" }
    // Speak the reply with the OFFLINE sherpa TTS when its pack is downloaded,
    // else the system TextToSpeech.
    val (sysSpeak, sysStop) = rememberTts()
    val ttsScope = rememberCoroutineScope()
    val speak: (String) -> Unit = { text ->
        if (voiceLang.isNotBlank() && com.lingxi.code.voice.offline.SherpaVoice.ttsReady(voiceLang)) {
            ttsScope.launch { com.lingxi.code.voice.offline.SherpaVoice.speak(voiceLang, text) }
        } else sysSpeak(text)
    }
    val stopSpeak: () -> Unit = { com.lingxi.code.voice.offline.SherpaVoice.stopSpeak(); sysStop() }

    // Start a one-shot listen → send → (engine streams the reply) cycle.
    fun listen() {
        onCancel(); stopSpeak(); userCaption = ""; didSend = false; phase = OrbPhase.Listening
        onListen { text ->
            if (!text.isNullOrBlank()) {
                userCaption = text; didSend = true; phase = OrbPhase.Thinking; onSend(text)
            } else if (phase == OrbPhase.Listening) {
                phase = OrbPhase.Idle
            }
        }
    }
    fun submitTyped(text: String) {
        val t = text.trim()
        if (t.isEmpty()) return
        userCaption = t; didSend = true; phase = OrbPhase.Thinking; onSend(t)
    }

    // Auto-listen on open; cancel the turn + stop TTS on close.
    LaunchedEffect(Unit) { listen() }
    DisposableEffect(Unit) { onDispose { onCancel(); stopSpeak() } }
    // First assistant delta flips thinking → speaking.
    LaunchedEffect(assistantText) {
        if (phase == OrbPhase.Thinking && assistantText.isNotEmpty()) phase = OrbPhase.Speaking
    }
    // Turn end (streaming → false) settles the orb and speaks the reply aloud.
    LaunchedEffect(streaming) {
        if (streaming) {
            if (phase == OrbPhase.Thinking && assistantText.isNotEmpty()) phase = OrbPhase.Speaking
        } else if (phase == OrbPhase.Thinking || phase == OrbPhase.Speaking) {
            phase = OrbPhase.Idle; speak(assistantText)
        }
    }

    // The live caption: the user's recognized text while thinking, the streaming
    // assistant reply while speaking.
    val caption = when (phase) {
        OrbPhase.Listening -> ""
        OrbPhase.Thinking -> userCaption
        OrbPhase.Speaking -> assistantText
        OrbPhase.Idle -> if (didSend) assistantText else ""
    }
    val isAi = phase == OrbPhase.Speaking || (phase == OrbPhase.Idle && didSend)

    val label = when (phase) {
        OrbPhase.Idle, OrbPhase.Listening -> "聆听中"
        OrbPhase.Thinking -> "思考中"
        OrbPhase.Speaking -> name
    }
    val sub = when (phase) {
        OrbPhase.Idle -> ""
        OrbPhase.Listening -> "说完轻点收音 · 或继续"
        OrbPhase.Thinking -> "正在组织语言…"
        OrbPhase.Speaking -> "轻点光球可打断"
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.radialGradient(
                    colors = listOf(oklch(0.20f, 0.07f, 270f), Color(0xFF05050A)),
                ),
            ),
    ) {
        // The orb — tap to interrupt / re-listen.
        OrbCanvas(
            phase = phase,
            cyFrac = 0.40f,
            modifier = Modifier
                .fillMaxSize()
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                ) { listen() },
        )

        // top bar
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.systemBars)
                .padding(horizontal = 20.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CircleGlassButton(LXIconName.Chevron, "退出心流", onClick = onClose)
            if (inputDialog) {
                CircleGlassButton(LXIconName.Message, "文字输入") { typing = true }
            } else {
                Spacer(Modifier.size(40.dp))
            }
        }

        // status + caption
        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.systemBars)
                .padding(horizontal = 28.dp)
                .padding(bottom = 46.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                val dot = when (phase) {
                    OrbPhase.Listening -> oklch(0.72f, 0.18f, 150f)
                    OrbPhase.Speaking -> oklch(0.75f, 0.19f, 300f)
                    else -> oklch(0.70f, 0.16f, 260f)
                }
                Canvas(Modifier.size(7.dp)) { drawCircle(dot) }
                Spacer(Modifier.width(8.dp))
                Text(label, color = oklch(0.92f, 0.02f, 270f), fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
            Spacer(Modifier.height(14.dp))
            Box(modifier = Modifier.heightIn(min = 84.dp).widthIn(max = 320.dp), contentAlignment = Alignment.Center) {
                if (caption.isEmpty()) {
                    Text(sub, color = oklch(0.60f, 0.03f, 270f), fontSize = 14.sp, textAlign = TextAlign.Center)
                } else {
                    val shown = if (isAi) caption else "“$caption”"
                    Text(
                        shown,
                        color = if (isAi) oklch(0.96f, 0.02f, 290f) else oklch(0.80f, 0.03f, 270f),
                        fontSize = if (isAi) 18.sp else 17.sp,
                        fontWeight = if (isAi) FontWeight.Medium else FontWeight.Normal,
                        lineHeight = 26.sp,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }

        // pop-up text input (心流中的文字输入)
        if (typing) {
            FlowTextInput(
                name = name,
                value = draft,
                onValueChange = { draft = it },
                onCancel = { typing = false; draft = "" },
                onSend = {
                    val txt = draft.trim()
                    if (txt.isNotEmpty()) { typing = false; draft = ""; submitTyped(txt) }
                },
            )
        }
    }
}

@Composable
private fun CircleGlassButton(icon: LXIconName, label: String, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .size(40.dp)
            .background(Color.White.copy(alpha = 0.06f), CircleShape)
            .border(0.5.dp, Color.White.copy(alpha = 0.12f), CircleShape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        LXIcon(name = icon, size = 19.dp, color = oklch(0.90f, 0.01f, 270f), stroke = 2f, contentDescription = label)
    }
}

@Composable
private fun FlowTextInput(
    name: String,
    value: String,
    onValueChange: (String) -> Unit,
    onCancel: () -> Unit,
    onSend: () -> Unit,
) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.55f))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) { onCancel() },
        contentAlignment = Alignment.BottomCenter,
    ) {
        val canSend = value.isNotBlank()
        Column(
            modifier = Modifier
                .windowInsetsPadding(WindowInsets.systemBars)
                .padding(horizontal = 16.dp)
                .padding(bottom = 40.dp)
                .fillMaxWidth()
                .background(oklch(0.22f, 0.02f, 275f), RoundedCornerShape(22.dp))
                .border(0.5.dp, Color.White.copy(alpha = 0.14f), RoundedCornerShape(22.dp))
                // Swallow taps so the scrim's dismiss doesn't fire on the card.
                .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) {}
                .padding(14.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 4.dp, bottom = 10.dp)) {
                LXIcon(LXIconName.Message, size = 14.dp, color = oklch(0.78f, 0.12f, 288f), stroke = 2f)
                Spacer(Modifier.width(8.dp))
                Text("文字输入给 $name", color = oklch(0.78f, 0.05f, 285f), fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
            }
            BasicTextField(
                value = value,
                onValueChange = onValueChange,
                textStyle = TextStyle(color = oklch(0.95f, 0.02f, 285f), fontSize = 15.5.sp, lineHeight = 23.sp),
                cursorBrush = SolidColor(oklch(0.85f, 0.15f, 290f)),
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 56.dp)
                    .background(Color.White.copy(alpha = 0.05f), RoundedCornerShape(14.dp))
                    .border(0.5.dp, Color.White.copy(alpha = 0.12f), RoundedCornerShape(14.dp))
                    .padding(horizontal = 14.dp, vertical = 12.dp),
                decorationBox = { inner ->
                    if (value.isEmpty()) {
                        Text("输入消息…", color = oklch(0.55f, 0.03f, 280f), fontSize = 15.5.sp)
                    }
                    inner()
                },
            )
            Row(
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("取消", color = oklch(0.68f, 0.03f, 280f), fontSize = 14.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.clickable(onClick = onCancel).padding(horizontal = 16.dp, vertical = 8.dp))
                Spacer(Modifier.width(10.dp))
                Row(
                    modifier = Modifier
                        .height(38.dp)
                        .background(
                            if (canSend) Brush.linearGradient(listOf(oklch(0.70f, 0.19f, 270f), oklch(0.66f, 0.20f, 305f)))
                            else SolidColor(Color.White.copy(alpha = 0.08f)),
                            RoundedCornerShape(11.dp),
                        )
                        .clickable(enabled = canSend, onClick = onSend)
                        .padding(horizontal = 18.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val fg = if (canSend) Color.White else oklch(0.58f, 0.02f, 280f)
                    Text("发送", color = fg, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.width(6.dp))
                    LXIcon(LXIconName.ArrowUp, size = 15.dp, color = fg)
                }
            }
        }
    }
}

/**
 * A best-effort device TTS handle for speaking the assistant reply aloud.
 * Returns (speak, stop); the synthesizer is created/torn down with the caller.
 */
@Composable
private fun rememberTts(): Pair<(String) -> Unit, () -> Unit> {
    val context = LocalContext.current
    val holder = remember { mutableStateOf<TextToSpeech?>(null) }
    DisposableEffect(Unit) {
        var engine: TextToSpeech? = null
        engine = TextToSpeech(context.applicationContext) { status ->
            if (status == TextToSpeech.SUCCESS) engine?.language = Locale.CHINESE
        }
        holder.value = engine
        onDispose { engine?.stop(); engine?.shutdown(); holder.value = null }
    }
    val speak: (String) -> Unit = { text ->
        if (text.isNotBlank()) holder.value?.speak(text, TextToSpeech.QUEUE_FLUSH, null, "orb")
    }
    val stop: () -> Unit = { holder.value?.stop() }
    return speak to stop
}
