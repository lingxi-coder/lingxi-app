package com.lingxi.code.conversation

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.ReasoningControl
import com.lingxi.code.theme.LingXiTheme

/**
 * Native radio choices, disclosed from the model sheet's settings row.
 * A locked control shows its value and the engine's reason instead of choices.
 */
@Composable
internal fun EffortPicker(
    control: ReasoningControl,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val choices = (listOf(ReasoningControl.AUTOMATIC) + control.levels).distinct()
    Column(modifier = modifier.fillMaxWidth().testTag(UiTags.COMPOSER_EFFORT)) {
        Text(
            stringResource(R.string.composer_effort),
            color = t.text3,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(horizontal = 24.dp, vertical = 12.dp),
        )
        if (control.editable && control.disabledReason == null && control.levels.isNotEmpty()) {
            Column(Modifier.selectableGroup()) {
                choices.forEach { id ->
                    val active = id == control.selection
                    ListItem(
                        headlineContent = { Text(effortLabel(id)) },
                        leadingContent = { RadioButton(selected = active, onClick = null) },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = Modifier.selectable(selected = active, role = Role.RadioButton) { onSelect(id) },
                    )
                }
            }
        } else {
            Text(effortLabel(control.selection), color = t.text2, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
        }
        control.disabledReason?.let { reason ->
            Text(reason, color = t.text3, style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
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
