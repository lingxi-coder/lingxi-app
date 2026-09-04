package com.lingxi.code.drawer

import java.io.File
import org.junit.Assert.assertFalse
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

    /**
     * True when `DrawerAppQuickActions(` is reached from the branch arm's own
     * opening brace without first entering a nested block.
     *
     * Deliberately NOT a regex over the wrapper's condition. The obvious
     * spelling -- an `(if|when)` followed by a bracketed condition, a brace and
     * then the call -- cannot span a condition that itself contains
     * parentheses. So the single most likely regression this assertion exists
     * to catch, `if (chatWorkspaceGroups.isNotEmpty()) { DrawerAppQuickActions(
     * ...) }` gating the row on a non-empty app list, slipped through it in
     * silence. That is measured, not theorised: planting exactly that wrap into
     * the Chat arm left the regex form of this assertion GREEN.
     *
     * Counting braces is spelling-independent instead. The head of an
     * unconditional arm holds exactly the arm's own `{` and no `}` at all; any
     * wrapper -- `if`, `when`, `apps.forEach`, `AnimatedVisibility`, a `?.let`
     * -- opens a second one, and a call moved BELOW the workspace list drags
     * that list's own lambda braces into the head.
     */
    private fun rendersQuickActionsUnconditionally(branch: String): Boolean {
        val call = branch.indexOf("DrawerAppQuickActions(")
        if (call < 0) return false
        val head = branch.substring(0, call)
        return head.count { it == '{' } == 1 && head.none { it == '}' }
    }

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
        assertTrue(
            "the quick actions must render ABOVE the workspace list (the row's own name — " +
                "\"quick actions\" ahead of the session list, matching iOS) and UNCONDITIONALLY: a " +
                "call that appears in the branch but after WorkspaceGroupsSection( — or behind an " +
                "`if` — would satisfy every assertion above while still leaving a zero-apps user " +
                "without the row on first render",
            chatBranch.indexOf("DrawerAppQuickActions(") in 0 until chatBranch.indexOf("WorkspaceGroupsSection("),
        )
        assertTrue(
            "quick actions must not be wrapped in a conditional (e.g. gated on a non-empty app " +
                "list) — the whole point of this row is to be there when the list is EMPTY",
            rendersQuickActionsUnconditionally(chatBranch),
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
        assertTrue(
            "the quick actions must render ABOVE the workspace list and UNCONDITIONALLY, same " +
                "reasoning as the Chat tab's sibling assertion above",
            codeBranch.indexOf("DrawerAppQuickActions(") in 0 until codeBranch.indexOf("WorkspaceGroupsSection("),
        )
        assertTrue(
            "quick actions must not be wrapped in a conditional",
            rendersQuickActionsUnconditionally(codeBranch),
        )
    }

    @Test
    fun `the quick actions row wires its two visible affordances to the create and browse callbacks`() {
        val body = branchBody("private fun DrawerAppQuickActions(", "private fun SectionTab(")

        assertTrue(
            "vacuity guard: the sliced region must still be DrawerAppQuickActions' body",
            "onCreateApp: () -> Unit" in body,
        )
        assertTrue(
            "the create row must be clickable via the onCreateApp callback",
            ".clickable(onClick = onCreateApp)" in body,
        )
        assertTrue(
            "the create row must use the generated create-app string, not a raw literal",
            "R.string.drawer_create_app" in body,
        )
        assertTrue(
            "the create row must carry a stable testTag for instrumented tests, sourced from " +
                "UiTags rather than a raw literal that could silently drift from its sibling in " +
                "androidTest",
            "UiTags.DRAWER_CREATE_APP" in body,
        )
        assertTrue(
            "the browse row must be clickable via the onOpenApps callback",
            ".clickable(onClick = onOpenApps)" in body,
        )
        assertTrue(
            "the browse row must use the generated apps-library string, not a raw literal, and " +
                "must name the SAME key iOS's equivalent row uses (Drawer.swift renders " +
                "`String(localized: \"drawer_apps_library\")` on the matching browse affordance) " +
                "so the one affordance does not read differently per platform",
            "R.string.drawer_apps_library" in body,
        )
        assertTrue(
            "the browse row must carry a stable testTag sourced from UiTags",
            "UiTags.DRAWER_OPEN_APPS_LIBRARY" in body,
        )
    }

    @Test
    fun `the orphaned full-tab AppsSection composable is gone, not left as a second dead function`() {
        assertTrue(
            "AppsSection (the old dedicated-Apps-tab composable, zero call sites since " +
                "DrawerSection.Apps was removed in 47d92dc28) must be deleted once its two " +
                "action rows are reused by DrawerAppQuickActions above — leaving it in place " +
                "alongside the new composable would be a second dead function. Keyed on the " +
                "declaration shape rather than one exact visibility-modifier spelling, so a " +
                "reformat (or `internal fun`) cannot silently defeat this by no longer matching " +
                "the literal `private fun AppsSection(`",
            !Regex("fun\\s+AppsSection\\s*\\(").containsMatchIn(source),
        )
    }

    @Test
    fun `SectionTabs no longer carries the unused apps count parameter`() {
        val start = source.indexOf("private fun SectionTabs(")
        assertTrue("expected to find SectionTabs' declaration in DrawerContent.kt", start >= 0)
        val end = source.indexOf(") {", start)
        assertTrue("expected to find the end of SectionTabs' parameter list", end > start)
        val declaration = source.substring(start, end)

        assertTrue(
            "SectionTabs never rendered a 4th tab from its `apps: Int` parameter (only " +
                "Chat/Code/Cron SectionTab rows were emitted) — once DrawerContent stops " +
                "passing `apps = appsCount` to it, the dead parameter must be removed too. Scoped " +
                "to SectionTabs' own declaration (not the whole file), so an unrelated `apps: Int` " +
                "elsewhere in this file cannot make this pass without the dead parameter actually " +
                "being gone",
            "apps: Int" !in declaration,
        )
    }

    @Test
    fun `DrawerContent no longer computes or forwards the dead apps count`() {
        assertTrue(
            "DrawerContent's own `appsCount` parameter has zero readers in its body (SectionTabs " +
                "never rendered a 4th tab from it) — once RootScreen stops passing " +
                "`appsCount = localAppsState.apps.size`, the dead parameter must be deleted here too",
            "appsCount" !in source,
        )
        val rootScreen = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()
        assertTrue(
            "RootScreen must stop computing and passing the now-deleted appsCount argument",
            "appsCount" !in rootScreen,
        )
    }

    /**
     * Kotlin line comments stripped. Both slices below span the production
     * comments that explain WHY no mode is set in those lambdas, and the most
     * natural wording of such a comment names the very symbol the assertions
     * require to be absent -- a maintainer writing "do not call
     * setConversationMode here" would otherwise turn this gate red on correct
     * code. (It already happened once while this test was being written.)
     */
    private fun String.codeOnly(): String = lines()
        .filterNot { it.trimStart().startsWith("//") }
        .joinToString("\n")

    /**
     * RootScreen's own wiring of the two rows: `onCreateApp` must not flip the
     * active conversation mode (already fixed — the drawer's create row lands
     * the user inside the new app's OWN conversation, so forcing a Code-mode
     * switch first is redundant), and `onOpenApps` — the plain "browse the
     * library" row — must not either, matching iOS's equivalent entry (which
     * changes no mode) and the sibling `onOpenLocalAppDetails` handler
     * elsewhere in this file, which opens the apps cover with no mode flip.
     *
     * Sliced from RootScreen.kt's own source rather than exercised via Compose:
     * this module's plain-JVM `test` source set has no
     * `androidx.compose.ui:ui-test-junit4` dependency (see the class doc
     * above), so every gate in this file reads the composable's source text.
     *
     * Bounded on a call inside each lambda's own body rather than the next
     * `},` (which drifts with reformatting/reindentation) — `closeDrawer()` for
     * the create row, `Refresh` for the open-apps row.
     */
    @Test
    fun `neither drawer quick-action row force-switches the active conversation mode`() {
        val rootScreen = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        val createStart = rootScreen.indexOf("onCreateApp = {\n                            closeDrawer()")
        assertTrue("expected to find the drawer's onCreateApp wiring in RootScreen.kt", createStart >= 0)
        val createEnd = rootScreen.indexOf("createAppFromDrawer()", createStart)
        assertTrue(createEnd > createStart)
        val createBody = rootScreen.substring(createStart, createEnd)
        assertFalse(
            "the create row must not force a conversation-mode switch: it lands the user " +
                "directly in the new app's OWN conversation, and iOS's equivalent entry changes " +
                "no mode",
            "setConversationMode" in createBody.codeOnly(),
        )

        val openStart = rootScreen.indexOf("onOpenApps = {", createEnd)
        assertTrue("expected to find the drawer's onOpenApps wiring in RootScreen.kt", openStart >= 0)
        val openEnd = rootScreen.indexOf(
            "localAppsViewModel.onAction(LocalAppsAction.Refresh)",
            openStart,
        )
        assertTrue(openEnd > openStart)
        val openBody = rootScreen.substring(openStart, openEnd)
        assertTrue(
            "vacuity guard: the sliced region must still be the browse row's own body",
            "showingApps = true" in openBody && "closeDrawer()" in openBody,
        )
        assertFalse(
            "the browse row only opens the library to look at it — forcing " +
                "setConversationMode(SessionMode.Code) here swaps the user's live Chat " +
                "conversation to a Code session as a side effect of browsing, which iOS's " +
                "equivalent entry does not do (and the sibling onOpenLocalAppDetails handler " +
                "elsewhere in this file already opens the apps cover with no mode flip)",
            "setConversationMode" in openBody.codeOnly(),
        )
    }
}
