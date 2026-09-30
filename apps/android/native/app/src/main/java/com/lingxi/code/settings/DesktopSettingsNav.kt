package com.lingxi.code.settings

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.lingxi.code.model.ProviderKind
import com.lingxi.code.theme.LingXiTheme

data class DesktopSettingsEntry(val route: String, val title: String, val group: String, val keys: List<String> = emptyList())
val desktopSettingsEntries = listOf(
    DesktopSettingsEntry(SettingsRoutes.GENERAL, "General", "Personal", listOf("language", "notifications", "input")),
    DesktopSettingsEntry(SettingsRoutes.ACCOUNT, "Account", "Personal"),
    DesktopSettingsEntry(SettingsRoutes.APPEARANCE, "Appearance", "Personal", listOf("theme", "dark", "light")),
    DesktopSettingsEntry(SettingsRoutes.VOICE, "Voice", "Personal", listOf("tts", "stt", "audio")),
    DesktopSettingsEntry(SettingsRoutes.ARCHIVED, "Archived chats", "Personal", listOf("restore")),
    DesktopSettingsEntry(SettingsRoutes.PROJECTS, "Projects & trust", "Personal"),
    DesktopSettingsEntry(SettingsRoutes.CREDENTIALS, "Provider credentials", "Models & services", listOf("api key", "auth")),
    DesktopSettingsEntry(SettingsRoutes.CUSTOM_PROVIDERS, "Custom providers & routing", "Models & services", listOf("providers", "aliases", "fallback", "retry")),
    DesktopSettingsEntry(SettingsRoutes.ENGINE_PERMISSIONS, "Permissions", "Coding", listOf("allow", "deny", "ask")),
    DesktopSettingsEntry(SettingsRoutes.TOOLS_AGENT, "Tools & agent behavior", "Coding", listOf("enabledTools", "thinking", "outputStyle")),
    DesktopSettingsEntry(SettingsRoutes.ENGINE_SKILLS, "Skills", "Coding"),
    DesktopSettingsEntry(SettingsRoutes.ENGINE_MCP, "MCP servers", "Coding"),
    DesktopSettingsEntry(SettingsRoutes.HOOKS, "Hooks", "Coding", listOf("PreToolUse", "PostToolUse")),
    DesktopSettingsEntry(SettingsRoutes.PLUGINS, "Plugins & marketplace", "Coding"),
    DesktopSettingsEntry(SettingsRoutes.DIAGNOSTICS, "Diagnostics", "Advanced", listOf("settings.json", "logs")),
    DesktopSettingsEntry(SettingsRoutes.ABOUT, "About", "Advanced", listOf("version", "licenses")),
)
fun searchDesktopSettings(query: String): List<DesktopSettingsEntry> {
    val needle = query.trim()
    return desktopSettingsEntries.filter { needle.isEmpty() || (listOf(it.title, it.route) + it.keys).any { value -> value.contains(needle, ignoreCase = true) } }
}

@Composable
fun DesktopSettingsNavigation(onNavigate: (String) -> Unit, selected: String? = null) {
    var query by remember { mutableStateOf("") }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(query, { query = it }, label = { Text(settingsLabel("Search settings")) }, singleLine = true, modifier = Modifier.fillMaxWidth())
        val translated = desktopSettingsEntries.map { it to settingsLabel(it.title) }
        val entries = translated.filter { (page, title) -> query.isBlank() || title.contains(query.trim(), true) || page in searchDesktopSettings(query) }.map { it.first }
        if (entries.isEmpty()) Text(settingsLabel("No settings match “$query”"), color = LingXiTheme.palette.text4)
        entries.groupBy { it.group }.forEach { (group, pages) ->
            SettingsSection(label = settingsLabel(group)) {
                pages.forEach { page ->
                    TextButton(onClick = { onNavigate(page.route) }, modifier = Modifier.fillMaxWidth()) {
                        SettingsNavigationIcon(page.route)
                        Spacer(Modifier.width(10.dp))
                        Text(settingsLabel(page.title), color = LingXiTheme.palette.text, modifier = Modifier.weight(1f))
                        if (selected == page.route) Text(settingsLabel("•"))
                    }
                }
            }
        }
    }
}
