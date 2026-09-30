package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

/**
 * The model-managed working plan, pinned above the composer.
 *
 * Driven by `ClientEvent.PlanUpdated`, which is a FULL-LIST REPLACE emitted on
 * the TodoWrite CALL (not its result) — an empty list therefore clears the panel
 * entirely, and this composable renders nothing for it.
 *
 * At most [MAX_VISIBLE_PLAN_TASKS] rows show; the remainder collapses into one
 * overflow clause counting the HIDDEN tasks by state, in progress → pending →
 * completed, exactly as `tui_core::tool_display::plan::overflow_summary` does
 * for the terminal. Tapping the panel expands it to the full list.
 */
@Composable
internal fun PlanTasksPanel(
    tasks: List<PlanTaskUi>,
    expanded: Boolean,
    onToggleExpanded: () -> Unit,
    modifier: Modifier = Modifier,
) {
    if (tasks.isEmpty()) return
    val t = LingXiTheme.palette
    val window = planWindow(tasks, max = if (expanded) tasks.size else MAX_VISIBLE_PLAN_TASKS)
    val toggleHint = if (expanded) {
        stringResource(R.string.chat_plan_collapse_hint)
    } else {
        stringResource(R.string.chat_plan_expand_hint)
    }

    Column(
        modifier = modifier
            .testTag(UiTags.PLAN_TASKS_PANEL)
            .fillMaxWidth()
            .padding(horizontal = 14.dp)
            .padding(bottom = 6.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
            .semantics {
                contentDescription = toggleHint
                role = Role.Button
            }
            .clickable(onClick = onToggleExpanded)
            .heightIn(min = 48.dp)
            .padding(horizontal = 12.dp, vertical = 9.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = stringResource(R.string.chat_plan_title),
                color = t.text3,
                fontSize = 11.5f.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            Text(
                text = "${tasks.count { it.state == PlanTaskStateUi.Completed }}/${tasks.size}",
                color = t.text4,
                fontSize = 11.5f.sp,
            )
        }
        window.visible.forEach { task -> PlanTaskRow(task) }
        overflowClause(window)?.let { clause ->
            Text(
                text = stringResource(R.string.chat_plan_overflow_prefix_label, clause),
                color = t.text4,
                fontSize = 12.sp,
            )
        }
    }
}

@Composable
private fun PlanTaskRow(task: PlanTaskUi) {
    val t = LingXiTheme.palette
    val stateLabel = when (task.state) {
        PlanTaskStateUi.Pending -> stringResource(R.string.chat_plan_state_pending)
        PlanTaskStateUi.InProgress -> stringResource(R.string.chat_plan_state_in_progress)
        PlanTaskStateUi.Completed -> stringResource(R.string.chat_plan_state_completed)
    }
    val glyphColor = when (task.state) {
        PlanTaskStateUi.Pending -> t.text4
        PlanTaskStateUi.InProgress -> t.accent
        PlanTaskStateUi.Completed -> t.ok
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .semantics { contentDescription = "$stateLabel ${task.subject}" },
        verticalAlignment = Alignment.Top,
    ) {
        Text(text = task.state.glyph, color = glyphColor, fontSize = 12.5f.sp)
        Spacer(Modifier.width(8.dp))
        Text(
            text = task.subject,
            color = if (task.state == PlanTaskStateUi.Completed) t.text4 else t.text2,
            fontSize = 12.5f.sp,
            lineHeight = (12.5f * 1.45f).sp,
            textDecoration = if (task.state == PlanTaskStateUi.Completed) {
                TextDecoration.LineThrough
            } else {
                null
            },
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
    }
}

/**
 * `"2 进行中, 1 待办"` — the localized hidden-remainder clause, or `null` when
 * nothing is hidden. Zero counts are omitted and the order is fixed at in
 * progress → pending → completed, matching the terminal's `overflow_summary`.
 */
@Composable
private fun overflowClause(window: PlanWindow): String? {
    if (!window.hasOverflow) return null
    val inProgress = stringResource(R.string.chat_plan_overflow_in_progress_label, window.hiddenInProgress)
    val pending = stringResource(R.string.chat_plan_overflow_pending_label, window.hiddenPending)
    val completed = stringResource(R.string.chat_plan_overflow_completed_label, window.hiddenCompleted)
    return buildList {
        if (window.hiddenInProgress > 0) add(inProgress)
        if (window.hiddenPending > 0) add(pending)
        if (window.hiddenCompleted > 0) add(completed)
    }.joinToString(", ").takeIf { it.isNotEmpty() }
}
