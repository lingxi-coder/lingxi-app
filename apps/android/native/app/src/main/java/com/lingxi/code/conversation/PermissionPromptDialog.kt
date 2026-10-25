package com.lingxi.code.conversation

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.AutoModePromptDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.components.UiTags

/**
 * The Android permission sheet for an engine-parked tool request. The engine
 * request mapping and approve/deny callbacks are unchanged; this surface keeps
 * the requested details readable and the one-time approval visually primary.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun PermissionPromptDialog(
    state: PermissionPromptState?,
    onApprove: (requestId: ULong, response: PermissionResponseDto) -> Unit,
    onDeny: (requestId: ULong) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (state == null) return
    val colors = MaterialTheme.colorScheme
    val bypassDetail = stringResource(R.string.permission_bypass_confirmation_detail)
    val isBypass = !state.isPlan && state.toolName == null && state.detail == bypassDetail
    val summary = when {
        state.isPlan -> stringResource(R.string.permission_prompt_plan_summary)
        state.toolName != null -> stringResource(R.string.permission_prompt_tool_summary, state.toolName)
        else -> stringResource(R.string.permission_prompt_general_summary)
    }
    val riskCopy = when {
        isBypass -> stringResource(R.string.permission_prompt_bypass_risk)
        state.autoModePrompt != null && !state.suppressAlwaysAllowRule ->
            stringResource(R.string.permission_prompt_auto_risk)
        !state.suppressAlwaysAllowRule -> stringResource(R.string.permission_prompt_rule_risk)
        else -> stringResource(R.string.permission_prompt_once_risk)
    }
    val riskColor = if (colors.surface.luminance() < 0.5f) Color(0xFFFF9F66) else Color(0xFFC74300)
    val sheetState = rememberModalBottomSheetState(
        skipPartiallyExpanded = true,
        confirmValueChange = { target -> target != SheetValue.Hidden },
    )

    ModalBottomSheet(
        modifier = modifier.testTag(UiTags.PERMISSION_PROMPT),
        onDismissRequest = {},
        sheetState = sheetState,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        containerColor = colors.surface,
    ) {
        Surface(
            shape = RoundedCornerShape(24.dp),
            border = BorderStroke(1.dp, colors.outlineVariant),
            color = colors.surface,
            modifier = Modifier.padding(horizontal = 12.dp).padding(bottom = 24.dp),
        ) {
            Column(
                verticalArrangement = Arrangement.spacedBy(16.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 24.dp)
                    .padding(vertical = 22.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        imageVector = Icons.Rounded.Security,
                        contentDescription = null,
                        tint = if (isBypass) riskColor else colors.onSurfaceVariant,
                        modifier = Modifier.size(20.dp),
                    )
                    Text(
                        text = stringResource(if (isBypass) R.string.permission_elevated_risk else R.string.permission_request_title),
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (isBypass) riskColor else colors.onSurfaceVariant,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.padding(start = 10.dp),
                    )
                }

                Text(
                    text = state.title,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                    color = colors.onSurface,
                )
                Text(
                    text = summary,
                    style = MaterialTheme.typography.bodyMedium,
                    color = colors.onSurfaceVariant,
                )

                Column(
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    modifier = Modifier.weight(1f, fill = false).heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                ) {
                    state.worker?.let { worker ->
                        val dot = runCatching { Color(android.graphics.Color.parseColor(worker.color)) }
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
                                text = if (worker.team != null) "${worker.name} · ${worker.team}" else worker.name,
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
                            modifier = Modifier.fillMaxWidth().heightIn(max = 240.dp),
                            shape = RoundedCornerShape(14.dp),
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

                    Text(
                        text = riskCopy,
                        style = MaterialTheme.typography.bodyMedium,
                        color = colors.onSurfaceVariant,
                    )
                }

                if (!state.suppressAlwaysAllowRule && state.autoModePrompt == null) {
                    OutlinedButton(
                        onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ALWAYS) },
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag(UiTags.PERMISSION_ALLOW_ALWAYS),
                        shape = CircleShape,
                        border = BorderStroke(1.dp, colors.outlineVariant),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.onSurface),
                    ) {
                        Text(stringResource(R.string.permission_allow_always))
                    }
                }
                if (!state.suppressAlwaysAllowRule && state.autoModePrompt != null) {
                    OutlinedButton(
                        onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_AUTO) },
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag(UiTags.PERMISSION_ALLOW_AUTO),
                        shape = CircleShape,
                        border = BorderStroke(1.dp, colors.outlineVariant),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.onSurface),
                    ) {
                        Text(autoModeApprovalLabel(state.autoModePrompt))
                    }
                }

                FlowRow(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.End),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    OutlinedButton(
                        onClick = { onDeny(state.requestId) },
                        modifier = Modifier.heightIn(min = 48.dp).testTag(UiTags.PERMISSION_DENY),
                        shape = CircleShape,
                        border = BorderStroke(1.dp, colors.outlineVariant),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.onSurface),
                    ) {
                        Text(stringResource(R.string.permission_deny))
                    }
                    Button(
                        onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ONCE) },
                        modifier = Modifier.heightIn(min = 48.dp).testTag(UiTags.PERMISSION_ALLOW_ONCE),
                        shape = CircleShape,
                        colors = ButtonDefaults.buttonColors(containerColor = colors.onSurface, contentColor = colors.surface),
                    ) {
                        Text(stringResource(R.string.permission_allow_once))
                    }
                }
            }
        }
    }
}

private fun autoModeApprovalLabel(prompt: AutoModePromptDto): String =
    when (prompt) {
        AutoModePromptDto.WORKFLOW_BASH -> "Yes, and switch to auto mode"
        AutoModePromptDto.EXIT_PLAN_MODE -> "Yes, and use auto mode"
    }
