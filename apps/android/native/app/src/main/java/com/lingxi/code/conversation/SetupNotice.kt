package com.lingxi.code.conversation

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName

/** One compact settings link, with room for supporting copy and an independent dismissal. */
@Composable
internal fun SetupNotice(
    title: String, detail: String, icon: LXIconName, action: String, onAction: () -> Unit,
    tag: String, actionTag: String, onDismiss: (() -> Unit)? = null,
    dismissLabel: String = "", dismissTag: String = "",
) {
    val colors = MaterialTheme.colorScheme
    Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 3.dp).testTag(tag)) {
        Surface(
            onClick = onAction, color = colors.surfaceContainerLow, shape = MaterialTheme.shapes.medium,
            modifier = Modifier.fillMaxWidth().testTag(actionTag)
                .semantics { liveRegion = LiveRegionMode.Polite },
        ) {
            Row(Modifier.padding(start = 12.dp, end = 4.dp, top = 10.dp, bottom = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                LXIcon(icon, size = 20.dp, color = colors.onSurfaceVariant)
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.labelLarge, color = colors.onSurface)
                    Text(detail, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                    Text(action, style = MaterialTheme.typography.labelMedium, color = colors.primary,
                        modifier = Modifier.padding(top = 2.dp))
                }
                LXIcon(LXIconName.ChevronR, size = 18.dp, color = colors.onSurfaceVariant)
                if (onDismiss != null) IconButton(onClick = onDismiss, modifier = Modifier.testTag(dismissTag)) {
                    LXIcon(LXIconName.X, size = 18.dp, color = colors.onSurfaceVariant, contentDescription = dismissLabel)
                }
            }
        }
    }
}
