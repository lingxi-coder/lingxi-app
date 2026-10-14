package com.lingxi.code.conversation

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.GppGood
import androidx.compose.material.icons.outlined.GppMaybe
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme

internal data class ComposerContextUsage(val used: Long, val capacity: ULong) {
    val fraction: Double get() = (used.toDouble() / capacity.toDouble()).coerceIn(0.0, 1.0)
    val percent: Int get() = kotlin.math.floor(fraction * 100 + 0.5).toInt()

    companion object {
        fun from(usage: AgentRunUsage?, capacity: ULong?): ComposerContextUsage? {
            if (usage == null || capacity == null || capacity == 0uL) return null
            val counters = listOf(usage.inputTokens, usage.cacheReadTokens, usage.cacheCreationTokens)
            if (counters.any { it < 0 }) return null
            var used = 0L
            for (counter in counters) {
                if (counter > Long.MAX_VALUE - used) return null
                used += counter
            }
            return ComposerContextUsage(used, capacity)
        }
    }
}

@Composable
internal fun ComposerContextWindow(usage: AgentRunUsage?, capacity: ULong?) {
    val t = LingXiTheme.palette
    val snapshot = ComposerContextUsage.from(usage, capacity) ?: return
    val summary = "${snapshot.percent}% used (${100 - snapshot.percent}% left)"
    val tokens = "${snapshot.used} / ${snapshot.capacity} tokens used"
    var showsDetails by remember { mutableStateOf(false) }
    IconButton(
        onClick = { showsDetails = true },
        modifier = Modifier.testTag("composer.context-window")
            .semantics { contentDescription = "Context window (estimated): $summary. $tokens" },
    ) {
        Canvas(Modifier.size(16.dp)) {
            val stroke = Stroke(width = 3.dp.toPx(), cap = StrokeCap.Round)
            drawCircle(t.border, style = stroke)
            if (snapshot.used > 0) {
                drawArc(t.text3, -90f, (snapshot.fraction * 360).toFloat(), false, style = stroke)
            }
        }
    }
    if (showsDetails) {
        AlertDialog(
            onDismissRequest = { showsDetails = false },
            title = { Text("Context window (estimated)") },
            text = { Column { Text(summary); Text(tokens) } },
            confirmButton = { TextButton(onClick = { showsDetails = false }) { Text(stringResource(R.string.common_ok)) } },
        )
    }
}

@Composable
internal fun ComposerPermissionButton(mode: String, onClick: () -> Unit) {
    val t = LingXiTheme.palette
    val label = stringResource(R.string.settings_permission_mode_effective_fmt, mode)
    IconButton(onClick = onClick, modifier = Modifier.testTag("composer.permission")) {
        Icon(
            imageVector = if (mode == "bypassPermissions") Icons.Outlined.GppMaybe else Icons.Outlined.GppGood,
            contentDescription = label,
            tint = if (mode == "bypassPermissions") t.danger else t.text2,
        )
    }
}
