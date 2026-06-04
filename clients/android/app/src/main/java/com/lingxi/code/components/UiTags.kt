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

    /** The persistent, dismissible turn-error banner above the composer. */
    const val CHAT_ERROR = "tag.chatError"

    /** The error banner's dismiss (×) affordance. */
    const val CHAT_ERROR_DISMISS = "tag.chatErrorDismiss"

    /** A message bubble's share affordance that surfaces the native share chooser. */
    const val MESSAGE_SHARE = "tag.messageShare"

    /** The conversation's transient status row (engine tool activity / errors). */
    const val CHAT_STATUS = "tag.chatStatus"

    /** Prefix for the drawer section tabs; suffixed with the [DrawerSection] key. */
    const val DRAWER_TAB_PREFIX = "tag.drawerTab."

    fun drawerTab(sectionKey: String): String = DRAWER_TAB_PREFIX + sectionKey
}
