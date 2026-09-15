package com.lingxi.code.conversation

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Security
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.bindings.AutoModePromptDto
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.components.UiTags

/**
 * The Android permission-prompt modal — the allow/deny surface for an
 * engine-parked tool request (SHIP-BLOCKER #3). Mirrors the Electron
 * `PermissionPrompt`: a Material 3 alert with the request title + a tool-input
 * preview and actions — 拒绝 / 始终允许 / 自动模式 / 允许一次 — wired to
 * [onApprove] ([PermissionResponseDto.ALLOW_ONCE] /
 * [PermissionResponseDto.ALLOW_ALWAYS] / [PermissionResponseDto.ALLOW_AUTO])
 * and [onDeny]. When the engine marks a request as requiring a fresh human
 * decision, the persistent actions are omitted. Renders nothing when [state]
 * is `null`.
 *
 * The state shape + the request→prompt mapping ([permissionRequestToPrompt]) are
 * the unit-tested seam; this composable is the thin render layer (covered by the
 * on-device UI build).
 */
@Composable
fun PermissionPromptDialog(
    state: PermissionPromptState?,
    onApprove: (requestId: ULong, response: PermissionResponseDto) -> Unit,
    onDeny: (requestId: ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (state == null) return
    val colors = MaterialTheme.colorScheme

    AlertDialog(
        modifier = modifier.testTag(UiTags.PERMISSION_PROMPT),
        onDismissRequest = {},
        properties = DialogProperties(
            dismissOnBackPress = false,
            dismissOnClickOutside = false,
        ),
        icon = {
            Icon(
                imageVector = Icons.Rounded.Security,
                contentDescription = null,
                tint = colors.primary,
            )
        },
        title = {
            Text(
                text = state.title,
                style = MaterialTheme.typography.headlineSmall,
            )
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                state.worker?.let { w ->
                    val dot = runCatching { Color(android.graphics.Color.parseColor(w.color)) }
                        .getOrDefault(colors.onSurfaceVariant)
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Surface(
                            modifier = Modifier.size(7.dp),
                            shape = CircleShape,
                            color = dot,
                            content = {},
                        )
                        Text(
                            text = if (w.team != null) "${w.name} · ${w.team}" else w.name,
                            color = dot,
                            style = MaterialTheme.typography.labelMedium,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
                if (state.isPlan && state.detail.isNotBlank()) {
                    PlanDocumentCard(state.detail, writing = false)
                } else if (state.detail.isNotEmpty()) {
                    Surface(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 240.dp),
                        shape = RoundedCornerShape(12.dp),
                        color = colors.surfaceContainerHighest,
                    ) {
                        SelectionContainer {
                            Text(
                                text = state.detail,
                                color = colors.onSurfaceVariant,
                                style = MaterialTheme.typography.bodyMedium,
                                fontFamily = FontFamily.Monospace,
                                modifier = Modifier
                                    .verticalScroll(rememberScrollState())
                                    .padding(16.dp),
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            FlowRow(horizontalArrangement = Arrangement.End) {
                if (!state.suppressAlwaysAllowRule && state.autoModePrompt == null) {
                    TextButton(
                        onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ALWAYS) },
                        modifier = Modifier.testTag(UiTags.PERMISSION_ALLOW_ALWAYS),
                        colors = ButtonDefaults.textButtonColors(contentColor = colors.onSurfaceVariant),
                    ) {
                        Text(stringResource(R.string.permission_allow_always))
                    }
                }
                if (!state.suppressAlwaysAllowRule && state.autoModePrompt != null) {
                    TextButton(
                        onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_AUTO) },
                        modifier = Modifier.testTag(UiTags.PERMISSION_ALLOW_AUTO),
                        colors = ButtonDefaults.textButtonColors(contentColor = colors.onSurfaceVariant),
                    ) {
                        Text(autoModeApprovalLabel(state.autoModePrompt))
                    }
                }
                TextButton(
                    onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ONCE) },
                    modifier = Modifier.testTag(UiTags.PERMISSION_ALLOW_ONCE),
                ) {
                    Text(stringResource(R.string.permission_allow_once))
                }
            }
        },
        dismissButton = {
            TextButton(
                onClick = { onDeny(state.requestId) },
                modifier = Modifier.testTag(UiTags.PERMISSION_DENY),
                colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
            ) {
                Text(stringResource(R.string.permission_deny))
            }
        },
    )
}

private fun autoModeApprovalLabel(prompt: AutoModePromptDto): String =
    when (prompt) {
        AutoModePromptDto.WORKFLOW_BASH -> "Yes, and switch to auto mode"
        AutoModePromptDto.EXIT_PLAN_MODE -> "Yes, and use auto mode"
    }
