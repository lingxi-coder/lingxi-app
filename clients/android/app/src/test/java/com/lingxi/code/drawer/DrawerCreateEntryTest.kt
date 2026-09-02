package com.lingxi.code.drawer

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Guards WP1 (P0-4): a first-time Android user with an EMPTY local-app catalog
 * must still have an in-app way to create one.
 *
 * `47d92dc28` deleted the `DrawerSection.Apps` enum case, leaving `AppsSection`
 * (the composable that rendered the create/browse rows) with zero call sites —
 * `when (ui.section)` only covered Chat/Code/Cron, and every other
 * `showingApps = true` route required an existing app or a widget deep-link.
 * iOS never had this hole: `Drawer.swift`'s `conversationActions` renders the
 * create/library rows for every section except cron.
 *
 * `androidx.compose.ui.test` is only wired for `androidTest` (instrumented,
 * needs a device/emulator) in this module — `app/build.gradle.kts` has no
 * `testImplementation("androidx.compose.ui:ui-test-junit4")` for this plain-JVM
 * `test` source set. So this asserts at the source level, naming the exact
 * composable and branch, per the WP1 gate spec's documented fallback. The
 * existing `RootLocalAppPresenterSourceTest` / `LocalAppRuntimeAssetsTest`
 * establish the same File(path).readText() pattern in this module.
 */
class DrawerCreateEntryTest {
    private val source = File("src/main/java/com/lingxi/code/drawer/DrawerContent.kt").readText()

    private fun branchBody(sectionArm: String, nextArm: String): String {
        val start = source.indexOf(sectionArm)
        assertTrue("expected to find `$sectionArm` in DrawerContent.kt", start >= 0)
        val end = source.indexOf(nextArm, start)
        assertTrue("expected to find `$nextArm` after `$sectionArm`", end >= 0)
        return source.substring(start, end)
    }

    @Test
    fun `chat tab renders the create-app quick actions above an empty workspace list`() {
        val chatBranch = branchBody("DrawerSection.Chat ->", "DrawerSection.Code ->")

        assertTrue(
            "DrawerContent's DrawerSection.Chat branch must call DrawerAppQuickActions(...) so " +
                "a user with zero local apps still has an in-app create entry (mirrors iOS " +
                "Drawer.swift's conversationActions, rendered whenever section != .cron)",
            "DrawerAppQuickActions(" in chatBranch,
        )
        assertTrue(
            "the Chat tab's quick actions must be wired to the live onCreateApp callback, not a no-op",
            "onCreateApp = onCreateApp" in chatBranch,
        )
        assertTrue(
            "the Chat tab's quick actions must be wired to the live onOpenApps callback, not a no-op",
            "onOpenApps = onOpenApps" in chatBranch,
        )
    }

    @Test
    fun `code tab renders the create-app quick actions above an empty workspace list`() {
        val codeBranch = branchBody("DrawerSection.Code ->", "DrawerSection.Cron ->")

        assertTrue(
            "DrawerContent's DrawerSection.Code branch must call DrawerAppQuickActions(...) so " +
                "a user with zero local apps still has an in-app create entry (mirrors iOS " +
                "Drawer.swift's conversationActions, rendered whenever section != .cron)",
            "DrawerAppQuickActions(" in codeBranch,
        )
        assertTrue(
            "the Code tab's quick actions must be wired to the live onCreateApp callback, not a no-op",
            "onCreateApp = onCreateApp" in codeBranch,
        )
        assertTrue(
            "the Code tab's quick actions must be wired to the live onOpenApps callback, not a no-op",
            "onOpenApps = onOpenApps" in codeBranch,
        )
    }

    @Test
    fun `the orphaned full-tab AppsSection composable is gone, not left as a second dead function`() {
        assertTrue(
            "AppsSection (the old dedicated-Apps-tab composable, zero call sites since " +
                "DrawerSection.Apps was removed in 47d92dc28) must be deleted once its two " +
                "action rows are reused by DrawerAppQuickActions above — leaving it in place " +
                "alongside the new composable would be a second dead function",
            "private fun AppsSection(" !in source,
        )
    }

    @Test
    fun `SectionTabs no longer carries the unused apps count parameter`() {
        assertTrue(
            "SectionTabs never rendered a 4th tab from its `apps: Int` parameter (only " +
                "Chat/Code/Cron SectionTab rows were emitted) — once DrawerContent stops " +
                "passing `apps = appsCount` to it, the dead parameter must be removed too",
            "apps: Int" !in source,
        )
    }
}
