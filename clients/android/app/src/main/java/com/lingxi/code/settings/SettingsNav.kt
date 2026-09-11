package com.lingxi.code.settings

import com.lingxi.code.R

/** Stable routes retain old deep links while new entries mirror desktop settings. */
object SettingsRoutes {
    const val CREDENTIALS = "settings/provider-credentials"
    const val GENERAL = "settings/general"
    const val CUSTOM_PROVIDERS = "settings/custom-providers"
    const val FUSION = "settings/fusion"
    const val ENGINE_PERMISSIONS = "settings/permissions"
    const val TOOLS_AGENT = "settings/tools-agent"
    const val ENGINE_SKILLS = "settings/engine-skills"
    const val ENGINE_MCP = "settings/engine-mcp"
    const val HOOKS = "settings/hooks"
    const val PLUGINS = "settings/plugins"
    const val DIAGNOSTICS = "settings/diagnostics"
    const val ABOUT = "settings/about"
    const val ARCHIVED = "settings/archived"
    const val PROJECTS = "settings/projects"
    const val MAIN = "settings/main"
    const val ACCOUNT = "settings/account"

    // Providers and audio
    const val PROVIDER_LIST = "settings/providers/{kind}"
    const val PROVIDER_PICKER = "settings/providers/{kind}/add"
    const val PROVIDER_EDIT = "settings/providers/{kind}/edit/{id}"
    const val VOICE = "settings/voice"

    // Mobile capabilities and legacy routes
    const val SKILLS = "settings/skills"
    const val SKILL_DETAIL = "settings/skills/{id}"
    const val MCP_LIST = "settings/mcp"
    const val MCP_EDIT = "settings/mcp/{id}"
    const val LINUX_RUNTIME = "settings/linux-runtime"
    const val TYPESCRIPT_LSP = "settings/typescript-lsp"
    const val COMPUTER_USE = "settings/computer-use"
    const val DREAM = "settings/dream"
    const val CRON = "settings/cron"
    const val CRON_TASK = "settings/cron/task/{task}"
    const val CRON_RUN = "settings/cron/run/{run}"

    // Device settings
    const val APPEARANCE = "settings/appearance"
    const val LANGUAGE = "settings/language"
    const val NOTIFICATIONS = "settings/notifications"
    const val INPUT = "settings/input"

    // Privacy and permissions
    const val PRIVACY = "settings/privacy"
    const val PERMISSION_MODE = "settings/permission-mode"
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
    val PERMISSION_MODE = R.string.settings_permission_mode_title
    val OPEN_SOURCE = R.string.settings_title_open_source
    val SKILLS = R.string.settings_title_skills
    val MCP = R.string.settings_mcp_servers
    val LINUX_RUNTIME = R.string.settings_linux_runtime
    val TYPESCRIPT_LSP = R.string.settings_typescript_lsp_title
    val COMPUTER_USE = R.string.settings_title_computer_use
    val DREAM = R.string.settings_dream_mode
    val CRON = R.string.settings_title_cron
}
