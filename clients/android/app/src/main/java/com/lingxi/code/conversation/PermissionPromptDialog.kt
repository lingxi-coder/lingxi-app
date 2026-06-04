package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

/**
 * The Android permission-prompt modal — the allow/deny surface for an
 * engine-parked tool request (SHIP-BLOCKER #3). Mirrors the Electron
 * `PermissionPrompt`: a scrim-backed card with the request title + a tool-input
 * preview and three actions — 拒绝 / 始终允许 / 允许一次 — wired to
 * [onApprove] ([PermissionResponseDto.ALLOW_ONCE] / [PermissionResponseDto.ALLOW_ALWAYS])
 * and [onDeny]. Renders nothing when [state] is `null`.
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
    val t = LingXiTheme.palette

    Box(
        modifier = modifier
            .testTag(UiTags.PERMISSION_PROMPT)
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.32f)),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier
                .widthIn(max = 420.dp)
                .padding(horizontal = 24.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(t.windowBg)
                .border(0.5.dp, t.border, RoundedCornerShape(14.dp)),
        ) {
            Column(modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 14.dp)) {
                state.worker?.let { w ->
                    val dot = runCatching { Color(android.graphics.Color.parseColor(w.color)) }
                        .getOrDefault(t.text3)
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        modifier = Modifier.padding(bottom = 8.dp),
                    ) {
                        Box(modifier = Modifier.size(7.dp).clip(CircleShape).background(dot))
                        Text(
                            text = if (w.team != null) "${w.name} · ${w.team}" else w.name,
                            color = dot,
                            fontSize = 11.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
                Text(
                    text = state.title,
                    color = t.text,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                if (state.detail.isNotEmpty()) {
                    Spacer(Modifier.size(8.dp))
                    Text(
                        text = state.detail,
                        color = t.text2,
                        fontSize = 12.sp,
                        lineHeight = (12f * 1.5f).sp,
                        fontFamily = FontFamily.Monospace,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 180.dp)
                            .clip(RoundedCornerShape(8.dp))
                            .background(t.surface)
                            .border(0.5.dp, t.border, RoundedCornerShape(8.dp))
                            .verticalScroll(rememberScrollState())
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                    )
                }
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .background(t.surface)
                    .padding(horizontal = 16.dp, vertical = 12.dp),
            ) {
                PromptButton(
                    label = "拒绝",
                    fg = t.danger,
                    bg = Color.Transparent,
                    borderColor = t.border,
                    tag = UiTags.PERMISSION_DENY,
                    onClick = { onDeny(state.requestId) },
                    modifier = Modifier.weight(1f),
                )
                PromptButton(
                    label = "始终允许",
                    fg = t.text2,
                    bg = Color.Transparent,
                    borderColor = t.border,
                    tag = UiTags.PERMISSION_ALLOW_ALWAYS,
                    onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ALWAYS) },
                    modifier = Modifier.weight(1f),
                )
                PromptButton(
                    label = "允许一次",
                    fg = Color.White,
                    bg = t.accent,
                    borderColor = t.borderStrong,
                    tag = UiTags.PERMISSION_ALLOW_ONCE,
                    onClick = { onApprove(state.requestId, PermissionResponseDto.ALLOW_ONCE) },
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

@Composable
private fun PromptButton(
    label: String,
    fg: Color,
    bg: Color,
    borderColor: Color,
    tag: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .testTag(tag)
            .clip(RoundedCornerShape(8.dp))
            .background(bg)
            .border(0.5.dp, borderColor, RoundedCornerShape(8.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            color = fg,
            fontSize = 12.5f.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
        )
    }
}
