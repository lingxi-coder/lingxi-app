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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
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
        Blurb(stringResource(R.string.mcp_description_blurb))

        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().padding(bottom = 18.dp),
        ) {
            StatCard(stringResource(R.string.mcp_stat_connected), connectedCount, Modifier.weight(1f))
            StatCard(stringResource(R.string.mcp_stat_available_tools), totalTools, Modifier.weight(1f))
        }

        SettingsSection(label = stringResource(R.string.mcp_section_servers_fmt, servers.size)) {
            servers.forEachIndexed { i, s ->
                MCPServerRow(
                    server = s,
                    store = store,
                    isLast = i == servers.size - 1,
                    onTap = { onEdit(s.id) },
                )
            }
        }

        DashedAddButton(title = stringResource(R.string.mcp_add_server))
        Text(
            stringResource(R.string.mcp_transport_description_prefix) + "mcp.directory",
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
                    server.managedLocalApp?.stableServerName ?: server.name,
                    color = t.text,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                    Box(Modifier.size(6.dp).clip(CircleShape).background(server.status.dot(t)))
                    Text(
                        server.managedLocalApp?.let { managed ->
                            buildString {
                                append(managed.appName)
                                append(" · ")
                                append(stringResource(R.string.local_apps_authorization_app_id, managed.appId))
                                append(" · ")
                                append(stringResource(R.string.mcp_stat_available_tools))
                                append(" ")
                                append(managed.toolCount)
                            }
                        } ?: stringResource(
                            R.string.mcp_server_status_tools_fmt,
                            server.status.label,
                            server.tools,
                            server.transport,
                        ),
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
                enabled = server.managedLocalApp == null,
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
                stringResource(R.string.mcp_edit_status_tools_fmt, s.status.label, s.tools),
                color = t.text2,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.Medium,
                modifier = Modifier.padding(start = 8.dp).weight(1f),
            )
            Text(
                stringResource(R.string.mcp_reconnect),
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

        val managed = s.managedLocalApp
        if (managed == null) {
            FieldLabel(stringResource(R.string.settings_display_name))
            SettingsField(
                value = s.name,
                onValueChange = { v -> store.updateMcp(mcpId) { it.copy(name = v) } },
                mono = false,
                modifier = Modifier.padding(bottom = 14.dp),
            )

            FieldLabel(stringResource(R.string.mcp_endpoint))
            SettingsField(
                value = s.url,
                onValueChange = { v -> store.updateMcp(mcpId) { it.copy(url = v) } },
            )
            FieldHint(stringResource(R.string.mcp_endpoint_hint))

            SettingsSection(label = stringResource(R.string.mcp_section_transport_auth)) {
                SettingsRow(label = stringResource(R.string.mcp_transport_mode), value = s.transport, onTap = {})
                SettingsRow(label = stringResource(R.string.mcp_auth), value = s.auth ?: stringResource(R.string.common_none), isLast = true, onTap = {})
            }
        } else {
            SettingsSection(label = stringResource(R.string.local_apps_plugin_managed_mcp_source)) {
                SettingsRow(
                    label = stringResource(R.string.settings_display_name),
                    value = managed.stableServerName,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_name),
                    sub = managed.appId,
                    value = managed.appName,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_plugin_bundle_digest),
                    sub = managed.catalogDigestSummary,
                    value = managed.buildDigestSummary,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.mcp_stat_available_tools),
                    sub = managed.authoringRevision,
                    value = managed.toolCount.toString(),
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_verification_ui),
                    value = managed.uiVerification,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_verification_mcp),
                    value = managed.mcpVerification,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_mcp_proposal_field_input_schema),
                    value = managed.schemaSummary,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_mcp_proposal_field_annotations),
                    value = managed.annotationSummary,
                    chevron = false,
                )
                SettingsRow(
                    label = stringResource(R.string.local_apps_mcp_proposal_field_permission_ceiling),
                    value = managed.permissionCeiling,
                    chevron = false,
                    isLast = true,
                )
            }
        }

        SettingsSection(label = stringResource(R.string.mcp_section_tool_permissions), footer = stringResource(R.string.mcp_tool_permissions_footer)) {
            val tools = if (managed == null) {
                McpToolNames.take(minOf(6, s.tools)).map { it to null }
            } else {
                managed.toolSchemas.map { it.name to it.permissionSummary }
            }
            tools.forEachIndexed { i, tn ->
                SettingsRow(
                    label = tn.first,
                    sub = tn.second ?: if (i % 2 == 0) {
                        stringResource(R.string.mcp_tool_readonly)
                    } else {
                        stringResource(R.string.mcp_tool_writable)
                    },
                    chevron = false,
                    isLast = i == tools.size - 1,
                    trailing = if (managed == null) {
                        { LocalToggle(seed = i < 4) }
                    } else {
                        {}
                    },
                )
            }
        }

        if (managed == null) {
            SettingsSection {
                SettingsRow(
                    label = stringResource(R.string.mcp_enable_server),
                    chevron = false,
                    trailing = {
                        LXToggle(
                            checked = s.enabled,
                            onCheckedChange = { v -> store.updateMcp(mcpId) { it.copy(enabled = v) } },
                        )
                    },
                )
                SettingsRow(
                    label = stringResource(R.string.mcp_auto_start),
                    chevron = false,
                    isLast = true,
                    trailing = { LocalToggle(seed = true) },
                )
            }
        }

        if (managed == null) {
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
                Text(stringResource(R.string.mcp_remove_server), color = t.danger, fontSize = 13.5f.sp, fontWeight = FontWeight.Medium)
            }
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
