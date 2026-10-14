package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.ReasoningControl
import com.lingxi.code.theme.LingXiTheme

/**
 * The effort row at the top of the model menu: Auto plus the provider's levels,
 * the same choices as iOS `ConversationEffortControl`. A locked control shows
 * its value and the engine's reason instead of choices.
 */
@Composable
internal fun EffortPicker(
    control: ReasoningControl,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Column(modifier = modifier.padding(start = 15.dp, end = 15.dp, top = 8.dp, bottom = 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Text(
                text = stringResource(R.string.composer_effort),
                color = t.text3,
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            Text(text = effortLabel(control.selection), color = t.text2, fontSize = 11.sp)
        }
        val choices = listOf(ReasoningControl.AUTOMATIC) + control.levels
        if (control.editable && control.disabledReason == null && control.levels.isNotEmpty()) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier
                    .padding(top = 6.dp)
                    .horizontalScroll(rememberScrollState())
                    .testTag(UiTags.COMPOSER_EFFORT),
            ) {
                choices.forEach { id ->
                    val active = id == control.selection
                    Text(
                        text = effortLabel(id),
                        color = if (active) t.accent else t.text2,
                        fontSize = 12.sp,
                        fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
                        modifier = Modifier
                            .clip(RoundedCornerShape(50))
                            .background(if (active) t.accent.copy(alpha = 0.16f) else t.surfaceHover)
                            .semantics { selected = active }
                            .clickable(role = Role.RadioButton) { onSelect(id) }
                            .heightIn(min = 32.dp)
                            .padding(horizontal = 12.dp, vertical = 7.dp),
                    )
                }
            }
        }
        control.disabledReason?.let { reason ->
            Text(
                text = reason,
                color = t.text3,
                fontSize = 11.sp,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}

/** A selection id as the user reads it; unknown provider levels keep their id. */
@Composable
internal fun effortLabel(id: String): String = when {
    id == ReasoningControl.AUTOMATIC || id.isEmpty() -> stringResource(R.string.composer_effort_auto)
    id == "disabled" -> stringResource(R.string.composer_effort_off)
    id == "enabled" -> stringResource(R.string.composer_effort_on)
    id.startsWith("budget:") -> stringResource(R.string.composer_effort_budget, id.removePrefix("budget:"))
    id == "minimal" -> stringResource(R.string.composer_effort_minimal)
    id == "low" -> stringResource(R.string.composer_effort_low)
    id == "medium" -> stringResource(R.string.composer_effort_medium)
    id == "high" -> stringResource(R.string.composer_effort_high)
    id == "xhigh" -> stringResource(R.string.composer_effort_xhigh)
    id == "max" -> stringResource(R.string.composer_effort_max)
    else -> id.replaceFirstChar { it.uppercase() }
}
