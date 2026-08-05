package com.lingxi.code.conversation

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R

@Composable
internal fun ShellToolCard(
    state: ShellToolCardState,
    onOpenTerminal: (sessionId: String, initCommand: String) -> Unit,
    modifier: Modifier = Modifier,
) {
    var expanded by rememberSaveable(state.taskId) { mutableStateOf(state.status != ShellToolStatus.Completed) }
    val statusLabel = when (state.status) {
        ShellToolStatus.Running -> stringResource(R.string.chat_status_running)
        ShellToolStatus.Completed -> stringResource(R.string.chat_status_completed)
        ShellToolStatus.Failed -> stringResource(R.string.chat_status_failed)
        ShellToolStatus.TimedOut -> stringResource(R.string.chat_status_timed_out)
        ShellToolStatus.Cancelled -> stringResource(R.string.chat_status_cancelled)
    }
    Card(
        modifier = modifier
            .fillMaxWidth()
            .semantics { contentDescription = "Shell $statusLabel" }
            .clickable(role = Role.Button) { expanded = !expanded },
        shape = RoundedCornerShape(14.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
        ),
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text("Shell · $statusLabel", fontWeight = FontWeight.SemiBold)
                Text(
                    state.exitCode?.let { "exit $it" }
                        ?: state.durationMs?.let { "${it}ms" }
                        .orEmpty(),
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            Text(
                text = "$ ${state.command}",
                style = MaterialTheme.typography.bodyMedium,
                fontFamily = FontFamily.Monospace,
                maxLines = if (expanded) Int.MAX_VALUE else 2,
            )
            AnimatedVisibility(expanded) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    state.cwd?.let {
                        Text("cwd: $it", style = MaterialTheme.typography.labelSmall)
                    }
                    if (state.stdout.isNotEmpty()) {
                        TerminalOutput("stdout", state.stdout)
                    }
                    if (state.stderr.isNotEmpty()) {
                        TerminalOutput("stderr", state.stderr)
                    }
                    if (state.truncated) {
                        Text(stringResource(R.string.chat_output_truncated), style = MaterialTheme.typography.labelSmall)
                    }
                    TextButton(
                        onClick = { onOpenTerminal(state.sessionId, state.command) },
                    ) {
                        Text(stringResource(R.string.chat_open_in_terminal_button))
                    }
                }
            }
        }
    }
}

@Composable
private fun TerminalOutput(label: String, text: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelSmall)
        Text(
            text = text,
            style = MaterialTheme.typography.bodySmall,
            fontFamily = FontFamily.Monospace,
        )
    }
}
