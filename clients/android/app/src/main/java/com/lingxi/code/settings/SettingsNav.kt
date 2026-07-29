package com.lingxi.code.settings

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

/** The titles shown in the [SettingsHost] TopAppBar (and the back chevron). */
object SettingsTitles {
    const val MAIN = "设置"
    const val ACCOUNT = "账户"
    const val VOICE = "语音 TTS"
    const val APPEARANCE = "外观"
    const val LANGUAGE = "语言"
    const val NOTIFICATIONS = "通知"
    const val INPUT = "键盘与输入"
    const val PRIVACY = "数据与隐私"
    const val OPEN_SOURCE = "开源许可与对应源码"
    const val SKILLS = "Skills"
    const val MCP = "MCP 服务器"
    const val LINUX_RUNTIME = "Linux 运行时"
    const val COMPUTER_USE = "Computer Use"
    const val DREAM = "Dream 模式"
    const val CRON = "定时任务"
}
