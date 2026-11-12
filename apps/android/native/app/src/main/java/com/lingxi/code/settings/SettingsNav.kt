package com.lingxi.code.settings

import com.lingxi.code.R
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.SettingsJumpState

/** Stable routes retain old deep links while new entries mirror desktop settings. */
object SettingsRoutes {
    const val PROVIDER_ACCOUNTS = "settings/provider-accounts"
    const val GENERAL = "settings/general"
    const val CUSTOM_PROVIDERS = "settings/custom-providers"
    const val FUSION = "settings/fusion"
    const val ENGINE_PERMISSIONS = "settings/permissions"
    const val TOOLS_AGENT = "settings/tools-agent"
    const val ENGINE_SKILLS = "settings/engine-skills"
    const val ENGINE_MCP = "settings/engine-mcp"
    const val HOOKS = "settings/hooks"
    const val PLUGINS = "settings/plugins"
    const val PLUGIN_DETAIL = "settings/plugin/{id}"
    /** The per-layer plugin configuration that [PLUGINS] used to be. */
    const val PLUGINS_ADVANCED = "settings/plugins-advanced"
    const val DIAGNOSTICS = "settings/diagnostics"
    const val ABOUT = "settings/about"
    const val ARCHIVED = "settings/archived"
    const val PROJECTS = "settings/projects"
    const val MAIN = "settings/main"
    const val ACCOUNT = "settings/account"

    /** One of the six groups on the settings home; `{group}` is a [SettingsGroup] key. */
    const val GROUP = "settings/group/{group}"

    // Providers and audio
    const val PROVIDER_LIST = "settings/providers/{kind}"
    const val PROVIDER_PICKER = "settings/providers/{kind}/add"
    const val PROVIDER_EDIT = "settings/providers/{kind}/edit/{id}"
    const val VOICE = "settings/voice"

    // Mobile capabilities and legacy routes
    const val SKILLS = "settings/skills"
    const val SKILL_DIRECTORY = "settings/skill-directory"
    const val SKILL_DETAIL = "settings/skills/{id}"
    const val MCP_LIST = "settings/mcp"
    const val MCP_EDIT = "settings/mcp/{id}/{scope}"
    const val LINUX_RUNTIME = "settings/linux-runtime"
    const val TYPESCRIPT_LSP = "settings/typescript-lsp"
    const val COMPUTER_USE = "settings/computer-use"
    const val BROWSER = "settings/browser"
    const val DREAM = "settings/dream"

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
    fun mcpEdit(id: String, scope: String? = null) = "settings/mcp/$id/${android.net.Uri.encode(scope ?: "_")}"
    fun pluginDetail(id: String) = "settings/plugin/${android.net.Uri.encode(id)}"
    fun group(key: String): String = "settings/group/$key"
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
    val BROWSER = R.string.settings_browser_title
    val DREAM = R.string.settings_dream_mode
}

/** Test tags of the Settings tab (this slice owns them; the shell only owns the four tab tags). */
internal object SettingsTags {
    const val TOP_BAR = "settings.topBar"
    /** The top bar's back arrow: there is none on the root, and no close or "Done" anywhere. */
    const val BACK = "settings.back"
    /** The wide layout's 280 dp page list. */
    const val DESKTOP_NAV = "settings.desktopNav"
}

/** What a Back press means to the Settings tab (SPEC-mobile-team-bot-ui section 2.6). */
internal sealed interface SettingsBackAction {
    /** Go up one page inside Settings. */
    data object Pop : SettingsBackAction

    /** A jump from [tab] brought the user here and this press ends it: clear the stack, then select [tab]. */
    data class LeaveTo(val tab: AppTab) : SettingsBackAction

    /** Nothing to pop and nowhere remembered to go: the root's Back belongs to the shell (it selects Chats). */
    data object ShellDecides : SettingsBackAction
}

/**
 * Back for [pagesAboveRoot] (the real navigation stack, bottom first) and the jump's [origin], by the
 * shared [SettingsJumpState] rules: on the page a jump opened, or on the root after a jump to it, Back
 * leaves for the origin; deeper pages pop; the root with no origin is the shell's.
 */
internal fun settingsBackAction(pagesAboveRoot: List<String>, origin: AppTab?): SettingsBackAction {
    val reduction = SettingsJumpState(stack = pagesAboveRoot, origin = origin).reduce(SettingsJumpState.Event.Back, SettingsRoutes.MAIN)
    return when {
        reduction.leaveTo != null -> SettingsBackAction.LeaveTo(reduction.leaveTo)
        reduction.state.stack.size < pagesAboveRoot.size -> SettingsBackAction.Pop
        else -> SettingsBackAction.ShellDecides
    }
}
