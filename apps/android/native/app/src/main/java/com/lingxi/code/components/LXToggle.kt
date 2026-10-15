package com.lingxi.code.components

import androidx.compose.material3.Switch
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics

/** Settings rows supply the visible label to their native switch. */
val LocalControlLabel = staticCompositionLocalOf<String?> { null }

@Composable
fun LXToggle(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val label = LocalControlLabel.current
    Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled,
        modifier = modifier.then(if (label != null) Modifier.semantics { contentDescription = label } else Modifier))
}
