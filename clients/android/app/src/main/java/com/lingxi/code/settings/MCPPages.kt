package com.lingxi.code.settings

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.LXToggle
import com.lingxi.code.components.mix
import com.lingxi.code.components.tint
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.theme.LXFont
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.delay

/**
 * MCP (Model Context Protocol) surface, ported 1:1 from the iOS `MCPPages.swift`.
 *
 * The list shows a "connected / available-tools" stat-card pair, then each server
 * as a row with a status dot + tool count + transport and an enable toggle;
 * tapping pushes the edit page. Edit has a live status banner with a 重新连接
 * (reconnect) animation, name + endpoint fields, transport / auth rows, a per-tool
 * permission section, enable / auto-start, and a remove action.
 *
 * Mock-only: enable + reconnect mutate the hoisted [SettingsStore]; transport /
 * auth pickers and per-tool toggles are local seams.
 */

private val McpGreen = Color(red = 0f, green = 0.78f, blue = 0.55f) // oklch(70% ~0.18 165)

// The pool of tool names the edit page slices from (matches the iOS list).
private val McpToolNames = listOf("list_files", "read_file", "write_file", "create_issue", "list_issues", "comment")

// MARK: - Server list --------------------------------------------------------

@Composable
fun MCPListPage(
    state: SettingsUiState,
    store: SettingsStore,
    onEdit: (id: String) -> Unit,
) {
    val t = LingXiTheme.palette
    val servers = state.mcpServers
    val connectedCount = servers.count { it.status == ConnStatus.Connected }
    val totalTools = servers
        .filter { it.enabled && it.status == ConnStatus.Connected }
        .sumOf { it.tools }

    Column(Modifier.fillMaxWidth()) {
        Blurb(
            "MCP (Model Context Protocol) 是连接外部工具的开放协议。" +
                "已连接的服务器会向 AI 暴露工具调用能力。",
        )

        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().padding(bottom = 18.dp),
        ) {
            StatCard("已连接", connectedCount, Modifier.weight(1f))
            StatCard("可用工具", totalTools, Modifier.weight(1f))
        }

        SettingsSection(label = "服务器 · ${servers.size}") {
            servers.forEachIndexed { i, s ->
                MCPServerRow(
                    server = s,
                    store = store,
                    isLast = i == servers.size - 1,
                    onTap = { onEdit(s.id) },
                )
            }
        }

        DashedAddButton(title = "添加 MCP 服务器")
        Text(
            "支持 stdio / SSE / Streamable HTTP 三种传输；OAuth、API Key、本地子进程多种鉴权。\n" +
                "浏览公开服务器目录：mcp.directory",
            color = t.text4,
            fontSize = 11.sp,
            lineHeight = 16.sp,
            modifier = Modifier.fillMaxWidth().padding(top = 14.dp),
        )
    }
}

/**
 * One server row: a status-tinted plug tile, the name, a monospaced dot + status
 * · tool-count · transport sub line, and an enable toggle. Tapping the row opens
 * the edit page; the toggle mutates the hoisted store without navigating.
 */
@Composable
private fun MCPServerRow(
    server: com.lingxi.code.model.MCPServer,
    store: SettingsStore,
    isLast: Boolean,
    onTap: () -> Unit,
) {
    val t = LingXiTheme.palette
    val iconColor = when (server.status) {
        ConnStatus.Connected -> McpGreen
        ConnStatus.Error -> t.statusError
        else -> t.text4
    }
    Column(Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onTap)
                .padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(28.dp)
                    .clip(RoundedCornerShape(7.dp))
                    .background(iconColor.mix(t.surface, 0.16f))
                    .border(0.5.dp, iconColor.tint(0.28f), RoundedCornerShape(7.dp)),
            ) {
                LXIcon(name = LXIconName.Plug, size = 14.dp, color = iconColor, stroke = 1.8f)
            }
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    server.name,
                    color = t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    Box(Modifier.size(6.dp).clip(CircleShape).background(server.status.dot(t)))
                    Text(
                        "${server.status.label} · ${server.tools} 工具 · ${server.transport}",
                        color = t.text4,
                        fontSize = 11.5f.sp,
                        fontFamily = LXFont.mono,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            LXToggle(
                checked = server.enabled,
                onCheckedChange = { v -> store.updateMcp(server.id) { it.copy(enabled = v) } },
            )
        }
        if (!isLast) Box(Modifier.fillMaxWidth().size(0.5.dp).background(t.border))
    }
}

/** A small KPI card: uppercase label over a bold value. */
@Composable
private fun StatCard(label: String, value: Int, modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    Column(
        verticalArrangement = Arrangement.spacedBy(2.dp),
        modifier = modifier
            .clip(RoundedCornerShape(10.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(10.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Text(label.uppercase(), color = t.text4, fontSize = 10.5f.sp, letterSpacing = 0.5.sp)
        Text("$value", color = t.text, fontSize = 18.sp, fontWeight = FontWeight.Bold)
    }
}

// MARK: - Server edit --------------------------------------------------------

@Composable
fun MCPEditPage(
    mcpId: String,
    state: SettingsUiState,
    store: SettingsStore,
    onPop: () -> Unit,
) {
    val t = LingXiTheme.palette
    val s = state.mcpServers.firstOrNull { it.id == mcpId }
    if (s == null) {
        LaunchedEffect(mcpId) { onPop() }
        return
    }
    val dot = s.status.dot(t)

    Column(Modifier.fillMaxWidth()) {
        // Status banner with reconnect ----------------------------------------
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 18.dp)
                .clip(RoundedCornerShape(10.dp))
                .background(dot.mix(t.surface, 0.08f))
                .border(0.5.dp, dot.tint(0.24f), RoundedCornerShape(10.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            PulsingDot(color = dot, pulsing = s.status == ConnStatus.Testing)
            Text(
                "${s.status.label} · ${s.tools} 工具可用",
                color = t.text2,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.Medium,
                modifier = Modifier.padding(start = 8.dp).weight(1f),
            )
            Text(
                "重新连接",
                color = t.text2,
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                modifier = Modifier
                    .clip(RoundedCornerShape(7.dp))
                    .background(t.windowBg)
                    .border(0.5.dp, t.border, RoundedCornerShape(7.dp))
                    .clickable { store.setMcpStatus(mcpId, ConnStatus.Testing) }
                    .padding(horizontal = 10.dp, vertical = 6.dp),
            )
        }

        // Reconnect animation: settle to connected ~1.1s after entering Testing.
        LaunchedEffect(s.status) {
            if (s.status == ConnStatus.Testing) {
                delay(1100)
                store.setMcpStatus(mcpId, ConnStatus.Connected)
            }
        }

        FieldLabel("显示名称")
        SettingsField(
            value = s.name,
            onValueChange = { v -> store.updateMcp(mcpId) { it.copy(name = v) } },
            mono = false,
            modifier = Modifier.padding(bottom = 14.dp),
        )

        FieldLabel("端点")
        SettingsField(
            value = s.url,
            onValueChange = { v -> store.updateMcp(mcpId) { it.copy(url = v) } },
        )
        FieldHint("支持 `stdio://`、`https://`、`sse://`")

        SettingsSection(label = "传输与鉴权") {
            SettingsRow(label = "传输方式", value = s.transport, onTap = {})
            SettingsRow(label = "鉴权", value = s.auth ?: "无", isLast = true, onTap = {})
        }

        SettingsSection(label = "工具权限", footer = "灵犀只会调用你允许的工具。每次首次调用会请求确认。") {
            val tools = McpToolNames.take(minOf(6, s.tools))
            tools.forEachIndexed { i, tn ->
                SettingsRow(
                    label = tn,
                    sub = if (i % 2 == 0) "只读" else "可写",
                    chevron = false,
                    isLast = i == tools.size - 1,
                    trailing = { LocalToggle(seed = i < 4) },
                )
            }
        }

        SettingsSection {
            SettingsRow(
                label = "启用服务器",
                chevron = false,
                trailing = {
                    LXToggle(
                        checked = s.enabled,
                        onCheckedChange = { v -> store.updateMcp(mcpId) { it.copy(enabled = v) } },
                    )
                },
            )
            SettingsRow(
                label = "自动启动",
                chevron = false,
                isLast = true,
                trailing = { LocalToggle(seed = true) },
            )
        }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
            modifier = Modifier
                .fillMaxWidth()
                .padding(top = 8.dp)
                .clip(RoundedCornerShape(11.dp))
                .border(0.5.dp, t.border, RoundedCornerShape(11.dp))
                .clickable { store.removeMcp(mcpId); onPop() }
                .padding(12.dp),
        ) {
            Text("移除此服务器", color = t.danger, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
        }
    }
}

/** An 8dp status dot with an optional pulsing ring (reconnect animation). */
@Composable
private fun PulsingDot(color: Color, pulsing: Boolean) {
    val transition = rememberInfiniteTransition(label = "mcpDot")
    val ringScale by transition.animateFloat(
        initialValue = 1.0f, targetValue = 1.9f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Restart),
        label = "ringScale",
    )
    val ringAlpha by transition.animateFloat(
        initialValue = 0.55f, targetValue = 0f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Restart),
        label = "ringAlpha",
    )
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(8.dp)) {
        if (pulsing) {
            Box(
                Modifier
                    .size(8.dp)
                    .scale(ringScale)
                    .alpha(ringAlpha)
                    .clip(CircleShape)
                    .background(color),
            )
        }
        Box(Modifier.size(8.dp).clip(CircleShape).background(color))
    }
}
