package com.lingxi.code.settings

import androidx.annotation.StringRes
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.model.ProviderKind

/**
 * The groups of the phone settings home: 账户卡片 + 7 组 + 搜索 instead of the
 * desktop's sixteen-row list. A group page lists the existing settings pages
 * unchanged; nothing here adds or edits a page.
 *
 * A group with a [directRoute] has no list page of its own: the home row opens
 * that page straight away (the model hub and the voice page are pages, not
 * menus of one or two rows).
 */
internal enum class SettingsGroup(
    val key: String,
    @StringRes val title: Int,
    @StringRes val summary: Int,
    val icon: LXIconName,
    val directRoute: String? = null,
) {
    Models(
        "models", R.string.settings_group_models_hub, R.string.settings_group_models_hub_summary, LXIconName.Sparkle,
        directRoute = SettingsRoutes.providerList(ProviderKind.Llm.name),
    ),
    Voice(
        "voice", R.string.settings_group_voice, R.string.settings_group_voice_summary, LXIconName.Mic,
        directRoute = SettingsRoutes.VOICE,
    ),
    Modes("modes", R.string.settings_group_modes, R.string.settings_group_modes_summary, LXIconName.Workflow),
    Phone("phone", R.string.settings_group_phone, R.string.settings_group_phone_summary, LXIconName.Terminal),
    General("general", R.string.settings_group_general, R.string.settings_group_general_summary, LXIconName.Cog),
    Data("data", R.string.settings_group_data, R.string.settings_group_data_summary, LXIconName.Folder),
    About("about", R.string.settings_group_about, R.string.settings_group_about_summary, LXIconName.Book),
    ;

    companion object {
        fun fromKey(key: String?): SettingsGroup? = entries.firstOrNull { it.key == key }
    }
}

/**
 * One settings page reachable from a group.
 *
 * [title] is the English page title; the UI localises it through [settingsLabel],
 * so the group page and the desktop navigation name a page the same way.
 * [keys] are extra search terms (the engine settings keys the page edits).
 */
internal data class SettingsGroupEntry(
    val group: SettingsGroup,
    val route: String,
    val title: String,
    val keys: List<String> = emptyList(),
)

/**
 * Every page, once, in display order. The mock-only `SKILLS` list and the
 * placeholder `FUSION`/`DREAM` routes are deliberately not linked: the real,
 * engine-backed pages are `ENGINE_SKILLS` and the layered engine pages.
 * Scheduled tasks live in the drawer, not in settings.
 */
private val allEntries = listOf(
    SettingsGroupEntry(SettingsGroup.Models, SettingsRoutes.providerList(ProviderKind.Llm.name), "Models & providers", listOf("model", "llm", "provider", "vision")),
    SettingsGroupEntry(SettingsGroup.Models, SettingsRoutes.CREDENTIALS, "Provider credentials", listOf("api key", "auth")),
    SettingsGroupEntry(SettingsGroup.Models, SettingsRoutes.CUSTOM_PROVIDERS, "Custom providers & routing", listOf("providers", "aliases", "fallback", "retry")),
    SettingsGroupEntry(SettingsGroup.Models, SettingsRoutes.providerList(ProviderKind.Search.name), "Web search providers", listOf("search")),
    SettingsGroupEntry(SettingsGroup.Models, SettingsRoutes.providerList(ProviderKind.Fetch.name), "Web fetch providers", listOf("fetch")),

    SettingsGroupEntry(SettingsGroup.Voice, SettingsRoutes.VOICE, "Voice & audio", listOf("tts", "stt", "audio", "speech", "microphone")),

    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.ENGINE_SKILLS, "Skills"),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.PLUGINS, "Plugins & marketplace"),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.ENGINE_MCP, "MCP servers"),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.HOOKS, "Hooks", listOf("PreToolUse", "PostToolUse")),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.TOOLS_AGENT, "Tools & agent behavior", listOf("promptSuggestionEnabled", "enabledTools", "thinking", "outputStyle")),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.ENGINE_PERMISSIONS, "Permissions", listOf("allow", "deny", "ask")),
    SettingsGroupEntry(SettingsGroup.Modes, SettingsRoutes.PERMISSION_MODE, "Permission mode"),

    SettingsGroupEntry(SettingsGroup.Phone, SettingsRoutes.LINUX_RUNTIME, "Linux runtime", listOf("terminal", "rootfs")),
    SettingsGroupEntry(SettingsGroup.Phone, SettingsRoutes.COMPUTER_USE, "Computer use", listOf("accessibility")),
    SettingsGroupEntry(SettingsGroup.Phone, SettingsRoutes.TYPESCRIPT_LSP, "TypeScript language server", listOf("lsp")),

    SettingsGroupEntry(SettingsGroup.General, SettingsRoutes.APPEARANCE, "Appearance", listOf("theme", "dark", "light")),
    SettingsGroupEntry(SettingsGroup.General, SettingsRoutes.LANGUAGE, "Language"),
    SettingsGroupEntry(SettingsGroup.General, SettingsRoutes.NOTIFICATIONS, "Notifications", listOf("background")),
    SettingsGroupEntry(SettingsGroup.General, SettingsRoutes.INPUT, "Keyboard & input", listOf("keyboard")),

    SettingsGroupEntry(SettingsGroup.Data, SettingsRoutes.ARCHIVED, "Archived chats", listOf("restore")),
    SettingsGroupEntry(SettingsGroup.Data, SettingsRoutes.PROJECTS, "Projects & trust", listOf("trust")),
    SettingsGroupEntry(SettingsGroup.Data, SettingsRoutes.PRIVACY, "Data & privacy"),
    SettingsGroupEntry(SettingsGroup.Data, SettingsRoutes.DIAGNOSTICS, "Diagnostics", listOf("settings.json", "logs")),

    SettingsGroupEntry(SettingsGroup.About, SettingsRoutes.ABOUT, "About", listOf("version")),
    SettingsGroupEntry(SettingsGroup.About, SettingsRoutes.OPEN_SOURCE, "Open-source licenses", listOf("licenses")),
)

/**
 * The pages to list. Computer use is dropped when the device cannot offer it,
 * exactly like the device-capabilities page did before the regrouping.
 */
internal fun settingsGroupEntries(computerUseAvailable: Boolean): List<SettingsGroupEntry> =
    allEntries.filter { it.route != SettingsRoutes.COMPUTER_USE || computerUseAvailable }

/**
 * Case-insensitive search over the localised title, the English title, the
 * group's own name and the entry's search keys. A blank [query] keeps
 * everything. Pure: the caller resolves the localised labels.
 */
internal fun searchSettingsEntries(
    entries: List<SettingsGroupEntry>,
    query: String,
    labelOf: (SettingsGroupEntry) -> String,
    groupLabelOf: (SettingsGroup) -> String,
): List<SettingsGroupEntry> {
    val needle = query.trim()
    if (needle.isEmpty()) return entries
    return entries.filter { entry ->
        labelOf(entry).contains(needle, ignoreCase = true) ||
            entry.title.contains(needle, ignoreCase = true) ||
            groupLabelOf(entry.group).contains(needle, ignoreCase = true) ||
            entry.keys.any { it.contains(needle, ignoreCase = true) }
    }
}
