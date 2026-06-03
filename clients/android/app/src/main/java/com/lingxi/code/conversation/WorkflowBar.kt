package com.lingxi.code.conversation

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LingXiTheme

/** The lifecycle state of a workflow step chip. */
enum class StepState { Done, Running, Todo }

/** One workflow step shown as a chip in the [WorkflowBar]. */
data class WorkflowStep(val id: Int, val label: String, val state: StepState)

/** Mock workflow — ported verbatim from the iOS `WorkflowBar`. */
private val mockSteps: List<WorkflowStep> = listOf(
    WorkflowStep(1, "理解需求", StepState.Done),
    WorkflowStep(2, "检索 Claude iOS", StepState.Done),
    WorkflowStep(3, "抽屉/侧栏组件", StepState.Done),
    WorkflowStep(4, "语音模式集成", StepState.Running),
    WorkflowStep(5, "导出", StepState.Todo),
)

/**
 * Horizontally-scrolling row of workflow step chips (done / running / todo) —
 * the Android analog of the iOS `WorkflowBar`. The single "running" chip's dot
 * pulses via an infinite transition; "done" chips show a check, "todo" chips a
 * hollow ring with a dashed outline.
 */
@Composable
fun WorkflowBar(
    modifier: Modifier = Modifier,
    steps: List<WorkflowStep> = mockSteps,
) {
    val t = LingXiTheme.palette

    // Shared pulse for every running-chip dot (matches the 1.4s repeat in iOS).
    val transition = rememberInfiniteTransition(label = "workflow-pulse")
    val pulse by transition.animateFloat(
        initialValue = 0.35f,
        targetValue = 0.8f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 1400),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "workflow-pulse-alpha",
    )

    Row(
        modifier = modifier
            .fillMaxWidth()
            .background(t.windowBg)
            .drawBehind {
                // 0.5dp bottom hairline border.
                val h = 0.5.dp.toPx()
                drawRect(
                    color = t.border,
                    topLeft = androidx.compose.ui.geometry.Offset(0f, size.height - h),
                    size = androidx.compose.ui.geometry.Size(size.width, h),
                )
            }
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 16.dp)
            .padding(top = 8.dp, bottom = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        steps.forEach { step -> WorkflowChip(step = step, pulseAlpha = pulse) }
    }
}

@Composable
private fun WorkflowChip(step: WorkflowStep, pulseAlpha: Float) {
    val t = LingXiTheme.palette
    val fg: Color = when (step.state) {
        StepState.Done -> t.ok
        StepState.Running -> t.accent
        StepState.Todo -> t.text4
    }
    val bg: Color = when (step.state) {
        StepState.Done -> t.ok.tint(0.14f)
        StepState.Running -> t.accent.tint(0.15f)
        StepState.Todo -> Color.Transparent
    }
    val stroke: Color = when (step.state) {
        StepState.Done -> t.ok.tint(0.28f)
        StepState.Running -> t.accent.tint(0.35f)
        StepState.Todo -> t.text4.tint(0.30f)
    }

    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier
            .clip(CircleShape)
            .background(bg)
            .border(0.5.dp, stroke, CircleShape)
            .padding(horizontal = 9.dp, vertical = 4.dp),
    ) {
        when (step.state) {
            StepState.Done -> LXIcon(name = LXIconName.Check, size = 10.dp, color = fg, stroke = 2.5f)
            StepState.Running -> Box(
                modifier = Modifier
                    .size(5.dp)
                    .alpha(pulseAlpha)
                    .clip(CircleShape)
                    .background(t.accent),
            )
            StepState.Todo -> Box(
                modifier = Modifier
                    .size(5.dp)
                    .clip(CircleShape)
                    .border(1.dp, t.text4, CircleShape),
            )
        }
        Text(
            text = step.label,
            color = fg,
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            maxLines = 1,
        )
    }
}
