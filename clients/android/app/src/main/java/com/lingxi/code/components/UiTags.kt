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

    /** Prefix for the drawer section tabs; suffixed with the [DrawerSection] key. */
    const val DRAWER_TAB_PREFIX = "tag.drawerTab."

    fun drawerTab(sectionKey: String): String = DRAWER_TAB_PREFIX + sectionKey
}
