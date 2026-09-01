package com.lingxi.code.components

/**
 * Stable `Modifier.testTag` keys for the handful of UI affordances that the
 * instrumented Compose UI tests drive but that carry no user-visible text (icon
 * buttons, tappable tab rows). Everything with a visible Chinese label is found
 * by text in the tests; only these icon/row anchors need an explicit tag.
 *
 * Keeping the keys here (rather than inline string literals) lets the test
 * module reference the same constants, so a rename can't silently desync the
 * source and its test.
 */
object UiTags {
    /** The conversation top-bar hamburger that opens the 对话/项目/定时 drawer. */
    const val OPEN_DRAWER = "tag.openDrawer"

    /** The composer's camera affordance that triggers an on-device photo capture. */
    const val COMPOSER_CAMERA = "tag.composerCamera"

    /** The composer's accent Send button (shown idle, with text). */
    const val COMPOSER_SEND = "tag.composerSend"

    /** The composer's Stop button (replaces Send while a turn streams). */
    const val COMPOSER_STOP = "tag.composerStop"

    /** The composer's discard action for a recovered parked turn. */
    const val COMPOSER_DISCARD = "tag.composerDiscard"

    /** The persistent, dismissible turn-error banner above the composer. */
    const val CHAT_ERROR = "tag.chatError"

    /** The error banner's dismiss (×) affordance. */
    const val CHAT_ERROR_DISMISS = "tag.chatErrorDismiss"

    /** The actionable banner shown when no enabled LLM model is configured. */
    const val MODEL_SETUP_BANNER = "tag.modelSetupBanner"

    /** The model-setup banner's direct link to LLM Provider settings. */
    const val MODEL_SETUP_ACTION = "tag.modelSetupAction"

    /** The actionable banner shown while Direct Computer Use is not ready. */
    const val COMPUTER_USE_SETUP_BANNER = "tag.computerUseSetupBanner"

    /** The Computer Use setup banner's direct link to its settings page. */
    const val COMPUTER_USE_SETUP_ACTION = "tag.computerUseSetupAction"

    /** The Computer Use setup banner's dismiss (×) affordance. */
    const val COMPUTER_USE_SETUP_DISMISS = "tag.computerUseSetupDismiss"

    /** The composer chip that becomes a setup action when no model exists. */
    const val MODEL_SETUP_CHIP = "tag.modelSetupChip"

    /** The normal composer model chip that opens the searchable model picker. */
    const val MODEL_PICKER_CHIP = "tag.modelPickerChip"

    /** The model picker's inline search field. */
    const val MODEL_PICKER_SEARCH = "tag.modelPickerSearch"

    /** A message bubble's share affordance that surfaces the native share chooser. */
    const val MESSAGE_SHARE = "tag.messageShare"

    /** The conversation's transient status row (engine tool activity / errors). */
    const val CHAT_STATUS = "tag.chatStatus"

    /** Event-driven workflow/subagent progress panel above the composer. */
    const val WORKFLOW_STATUS_PANEL = "tag.workflowStatusPanel"
    const val EXECUTION_STATUS_PANEL = "tag.executionStatusPanel"

    /** The latest turn's expandable reasoning/tool/retry/usage timeline. */
    const val AGENT_RUN_TIMELINE = "tag.agentRunTimeline"

    /** The permission-prompt modal (allow/deny for an engine-parked tool). */
    const val PERMISSION_PROMPT = "tag.permissionPrompt"

    /** The permission prompt's Allow-once action. */
    const val PERMISSION_ALLOW_ONCE = "tag.permissionAllowOnce"

    /** The permission prompt's Allow-always action. */
    const val PERMISSION_ALLOW_ALWAYS = "tag.permissionAllowAlways"

    /** The permission prompt's Allow-auto action. */
    const val PERMISSION_ALLOW_AUTO = "tag.permissionAllowAuto"

    /** The permission prompt's Deny action. */
    const val PERMISSION_DENY = "tag.permissionDeny"

    /** The drawer's editable search field that filters the section lists. */
    const val DRAWER_SEARCH = "tag.drawerSearch"

    /** The persistent drawer shortcut that opens the full-screen Android shell. */
    const val DRAWER_TERMINAL = "tag.drawerTerminal"

    /** The dismissible offline banner shown in the chat scaffold when offline. */
    const val OFFLINE_BANNER = "tag.offlineBanner"

    /** The offline banner's retry affordance. */
    const val OFFLINE_RETRY = "tag.offlineRetry"

    /** The offline banner's dismiss (×) affordance. */
    const val OFFLINE_DISMISS = "tag.offlineDismiss"

    /** A tool call's show-more / show-less affordance for its body or diff. */
    const val TOOL_CALL_TOGGLE = "tag.toolCallToggle"

    /** The model-managed plan checklist pinned above the composer. */
    const val PLAN_TASKS_PANEL = "tag.planTasksPanel"

    /** Prefix for the drawer section tabs; suffixed with the [DrawerSection] key. */
    const val DRAWER_TAB_PREFIX = "tag.drawerTab."

    fun drawerTab(sectionKey: String): String = DRAWER_TAB_PREFIX + sectionKey
}
