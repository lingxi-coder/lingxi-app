package com.lingxi.code.settings

import com.lingxi.code.R

/**
 * Settings nav-graph routes — the Android analog of the iOS `SettingsPage` enum.
 * Each entry is a Navigation-Compose route string; pages that take an argument
 * (provider kind / id, skill id, mcp id) format it into the route and read it
 * back from the `NavBackStackEntry` arguments.
 *
 * A6 wires the account/appearance/language/notifications/input/privacy
 * destinations; the 智能 / 能力扩展 provider, voice, skill, MCP and Dream
 * destinations are routed to a placeholder seam until A7/A8 replace them
 * with the real editors.
 */
object SettingsRoutes {
    const val MAIN = "settings/main"
    const val ACCOUNT = "settings/account"

    // 智能 (A7)
    const val PROVIDER_LIST = "settings/providers/{kind}"
    const val PROVIDER_PICKER = "settings/providers/{kind}/add"
    const val PROVIDER_EDIT = "settings/providers/{kind}/edit/{id}"
    const val VOICE = "settings/voice"

    // 能力扩展 (A8 — placeholder for now)
    const val SKILLS = "settings/skills"
    const val SKILL_DETAIL = "settings/skills/{id}"
    const val MCP_LIST = "settings/mcp"
    const val MCP_EDIT = "settings/mcp/{id}"
    const val LINUX_RUNTIME = "settings/linux-runtime"
    const val COMPUTER_USE = "settings/computer-use"
    const val DREAM = "settings/dream"
    const val CRON = "settings/cron"
    const val CRON_TASK = "settings/cron/task/{task}"
    const val CRON_RUN = "settings/cron/run/{run}"

    // 应用 (A6)
    const val APPEARANCE = "settings/appearance"
    const val LANGUAGE = "settings/language"
    const val NOTIFICATIONS = "settings/notifications"
    const val INPUT = "settings/input"

    // 隐私与安全 (A6)
    const val PRIVACY = "settings/privacy"
    const val OPEN_SOURCE = "settings/open-source"

    fun providerList(kind: String) = "settings/providers/$kind"
    fun providerPicker(kind: String) = "settings/providers/$kind/add"
    fun providerEdit(kind: String, id: String) = "settings/providers/$kind/edit/$id"
    fun skillDetail(id: String) = "settings/skills/$id"
    fun mcpEdit(id: String) = "settings/mcp/$id"
    fun cron(taskKey: String? = null): String =
        taskKey?.takeIf(String::isNotBlank)
            ?.let { "$CRON/task/${android.net.Uri.encode(it)}" }
            ?: CRON
    fun cronRun(runId: String): String = "$CRON/run/${android.net.Uri.encode(runId)}"
}

/** The title resources shown in the [SettingsHost] TopAppBar (and the back chevron). */
object SettingsTitles {
    val MAIN = R.string.settings_title_main
    val ACCOUNT = R.string.settings_title_account
    val VOICE = R.string.settings_voice_audio
    val APPEARANCE = R.string.settings_appearance
    val LANGUAGE = R.string.settings_language_title
    val NOTIFICATIONS = R.string.settings_notifications
    val INPUT = R.string.settings_keyboard_input
    val PRIVACY = R.string.settings_data_privacy
    val OPEN_SOURCE = R.string.settings_title_open_source
    val SKILLS = R.string.settings_title_skills
    val MCP = R.string.settings_mcp_servers
    val LINUX_RUNTIME = R.string.settings_linux_runtime
    val COMPUTER_USE = R.string.settings_title_computer_use
    val DREAM = R.string.settings_dream_mode
    val CRON = R.string.settings_title_cron
}
