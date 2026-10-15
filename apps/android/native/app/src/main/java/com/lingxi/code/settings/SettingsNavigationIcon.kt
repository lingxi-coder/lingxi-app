package com.lingxi.code.settings

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable

/** One Material icon family for every settings destination. */
@Composable
internal fun SettingsNavigationIcon(route: String) {
    val icon = when (route) {
        SettingsRoutes.GENERAL -> Icons.Outlined.Settings
        SettingsRoutes.APPEARANCE -> Icons.Outlined.Palette
        SettingsRoutes.VOICE -> Icons.Outlined.Mic
        SettingsRoutes.PROJECTS -> Icons.Outlined.Folder
        SettingsRoutes.CUSTOM_PROVIDERS -> Icons.Outlined.Route
        SettingsRoutes.FUSION, SettingsRoutes.ENGINE_SKILLS -> Icons.Outlined.AutoAwesome
        SettingsRoutes.HOOKS -> Icons.Outlined.AccountTree
        SettingsRoutes.PLUGINS -> Icons.Outlined.Extension
        SettingsRoutes.DIAGNOSTICS -> Icons.Outlined.MonitorHeart
        SettingsRoutes.ACCOUNT -> Icons.Outlined.AccountCircle
        SettingsRoutes.CREDENTIALS -> Icons.Outlined.Key
        SettingsRoutes.ENGINE_PERMISSIONS -> Icons.Outlined.Security
        SettingsRoutes.ARCHIVED -> Icons.Outlined.Archive
        SettingsRoutes.ENGINE_MCP -> Icons.Outlined.Dns
        SettingsRoutes.TOOLS_AGENT -> Icons.Outlined.Tune
        else -> Icons.Outlined.Info
    }
    Icon(icon, contentDescription = null)
}
