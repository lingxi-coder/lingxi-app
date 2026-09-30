package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class LocalAppsScreenTest {

    // The create form is gone: 「+」 creates an empty shell outright and the
    // conversation settles the rest, so there is no brief, no name field, no
    // surface picker and no workflow-model row left to test. What the screen
    // still owns pure-function-wise is the draft/formed label split and the
    // session catalog's display order.

    /**
     * The approval dialog's dismissal polarity (outside tap / back press must
     * NOT reject a create/MCP/profile approval) is asserted by no runtime
     * test: `androidx.compose.ui.test` is instrumented-only in this module
     * (see `DrawerCreateEntryTest`'s header for why), and every dismissal test
     * elsewhere in this package drives the ViewModel directly, never the
     * dialog. Source-level, in the same style as
     * `RootLocalAppPresenterSourceTest`.
     */
    @Test
    fun `the approval sheet dialog cannot be dismissed by an outside tap or back press`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        val start = source.indexOf("fun LocalAppApprovalSheetDialog(")
        assertTrue("read the wrong file: LocalAppApprovalSheetDialog not found", start >= 0)
        val end = source.indexOf("\nprivate fun LocalAppApprovalFact(", start)
        assertTrue("read the wrong file: the function after the dialog was not found", end > start)
        val dialog = source.substring(start, end)

        assertTrue(
            "vacuity guard: the sliced region must still be the AlertDialog call",
            "AlertDialog(" in dialog,
        )
        assertTrue(
            "an outside tap or back press must not be able to reject the sheet: " +
                "onDismissRequest must be a no-op, matching iOS's " +
                "`.interactiveDismissDisabled()` on the same prompt",
            "onDismissRequest = {}" in dialog,
        )
        assertTrue(
            "AlertDialog must be given DialogProperties disabling both interactive " +
                "dismissal paths, not just a no-op onDismissRequest (which alone still " +
                "lets a back press pop the dialog without calling it)",
            Regex(
                "DialogProperties\\(\\s*dismissOnBackPress\\s*=\\s*false\\s*,\\s*" +
                    "dismissOnClickOutside\\s*=\\s*false\\s*\\)",
            ).containsMatchIn(dialog),
        )
        assertTrue(
            "rejection must still be reachable through the explicit dismiss button, " +
                "keyed to the sheet's own requestId so a stale tap cannot resolve a " +
                "sheet that has since been superseded",
            "TextButton(onClick = { onAction(LocalAppsAction.ResolveApprovalSheet(sheet.requestId, false)) })" in dialog,
        )
    }

    /**
     * A DRAFT card must show neither the stored name nor the stored brief.
     *
     * The fixture below is exactly what the engine writes for a shell: the
     * non-localized `"untitled"` placeholder and an empty brief. Asserting
     * "the title is the draft copy" alone would still pass if the subtitle
     * leaked, so both are pinned, and the placeholder is pinned by ABSENCE.
     */
    @Test
    fun `a draft card renders the placeholder copy and leaks neither name nor brief`() {
        val shell = app(scaffolded = false, name = "untitled", brief = "")

        val text = localAppCardText(shell, draftTitle = "新应用", draftSubtitle = "创建中")

        assertEquals("新应用", text.title)
        assertEquals("创建中", text.subtitle)
        assertTrue("the placeholder name must not reach the card", "untitled" !in text.title)
        assertEquals(
            "新应用",
            localAppDisplayName(shell, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /** A formed app is unaffected: it still renders its own identity. */
    @Test
    fun `a formed card renders the app's own name and brief`() {
        val formed = app(scaffolded = true, name = "喝水记录", brief = "记录每天喝水量")

        val text = localAppCardText(formed, draftTitle = "新应用", draftSubtitle = "创建中")

        assertEquals("喝水记录", text.title)
        assertEquals("记录每天喝水量", text.subtitle)
        assertEquals(
            "喝水记录",
            localAppDisplayName(formed, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /**
     * "No app" is NOT "a draft". A top bar with no record yet falls back to its
     * own screen title, never to the draft copy.
     */
    @Test
    fun `an absent app falls back rather than borrowing the draft copy`() {
        assertEquals(
            "回退",
            localAppDisplayName(null, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /** The widget snapshot drops shells outright — it does not relabel them. */
    @Test
    fun `the widget snapshot keeps only scaffolded apps`() {
        val apps = listOf(
            app(id = "formed", scaffolded = true),
            app(id = "shell", scaffolded = false),
        )

        assertEquals(listOf("formed"), appsForWidgetSnapshot(apps).map { it.id })
    }

    @Test
    fun `the init session is pinned first regardless of where the engine paged it`() {
        val rows = listOf(
            row("s-newest"),
            row("s-init", isInit = true),
            row("s-older"),
        )
        assertEquals(
            listOf("s-init", "s-newest", "s-older"),
            sessionRowsForDisplay(rows).map { it.uuid },
        )
    }

    @Test
    fun `non-init rows keep the engine's modified-descending order`() {
        val rows = listOf(row("s-3"), row("s-2"), row("s-1"))
        assertEquals(
            listOf("s-3", "s-2", "s-1"),
            sessionRowsForDisplay(rows).map { it.uuid },
        )
    }

    @Test
    fun `a catalog with no init row is displayed unchanged`() {
        val rows = listOf(row("s-2"), row("s-1"))
        assertEquals(rows, sessionRowsForDisplay(rows))
    }

    /**
     * The widget request's PERMANENT entry: the app card's overflow menu,
     * offered only for a FORMED app.
     *
     * Asserted against the screen's source because unit tests here render no
     * Compose (no Robolectric, no compose-ui-test on this source set), so
     * nothing can observe which items a card's `DropdownMenu` offers. What it
     * can pin is the wiring — and the wiring is the half that actually broke.
     * The widget request used to exist ONLY as an `addWidget` checkbox inside
     * the create dialog, so deleting that dialog retired the feature outright
     * while every ViewModel test stayed green: `LocalAppsAction.RequestWidget`
     * was still handled, still tested, and dispatched from nowhere.
     * `LocalAppsViewModelTest."RequestWidget pins a formed app and refuses a
     * shell"` covers what the action does; this covers that something still
     * sends it.
     *
     * The `scaffolded` gate is asserted separately from the ViewModel's own
     * refusal, and is not redundant with it: the ViewModel refusing a shell is
     * what keeps an empty tile off the home screen, while the gate here is
     * what keeps the user from being offered a menu item that silently does
     * nothing when tapped.
     */
    @Test
    fun `the app card offers the widget entry only for a formed app`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        // Prove the file was really read before trusting anything asserted
        // about its contents.
        assertTrue(
            "read the wrong file — every assertion below would be vacuous",
            "private fun LocalAppCard(" in source,
        )

        assertTrue(
            "nothing on the local-apps screen sends RequestWidget any more: the " +
                "home-screen widget has no entry left in the product",
            "LocalAppsAction.RequestWidget(" in source,
        )
        assertTrue(
            "the widget entry must be offered only for a formed app — a shell " +
                "would get a menu item the ViewModel refuses to act on",
            isEnclosed(
                needle = "LocalAppsAction.RequestWidget(",
                byBlockOpenedBy = "if (app.scaffolded) {",
                source = source,
            ),
        )
    }

    @Test
    fun `the details screen includes a dedicated MCP tab and avoids raw transport fields`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        assertTrue("the Local App details tabs must include MCP", "LocalAppDetailsTab.Mcp" in source)
        assertTrue(
            "the MCP tab must drive app-local commands instead of settings MCP editing",
            "LocalAppsAction.StartMcpAuthoring(" in source,
        )
        assertTrue(
            "the Local App MCP tab must not expose raw transport fields",
            "mcp_endpoint" !in source && "mcp_command" !in source && "mcp_arguments" !in source,
        )
    }

    /**
     * Whether [needle] appears inside the braced block that some occurrence of
     * [byBlockOpenedBy] starts — that opener ending at the block's `{`.
     *
     * Brace-matched rather than line-counted, so moving the entry around inside
     * its gate keeps passing while removing the gate, or moving the entry out
     * from under it, fails.
     */
    private fun isEnclosed(needle: String, byBlockOpenedBy: String, source: String): Boolean {
        var from = 0
        while (true) {
            val opener = source.indexOf(byBlockOpenedBy, from)
            if (opener < 0) return false
            val open = opener + byBlockOpenedBy.length - 1 // the `{`
            var depth = 0
            var end = -1
            var i = open
            while (i < source.length) {
                when (source[i]) {
                    '{' -> depth++
                    '}' -> {
                        depth--
                        if (depth == 0) end = i
                    }
                }
                if (end >= 0) break
                i++
            }
            if (end > open && source.substring(open + 1, end).contains(needle)) return true
            from = opener + byBlockOpenedBy.length
        }
    }

    private fun app(
        id: String = "a",
        name: String = "客户跟进",
        brief: String = "记录客户跟进情况",
        scaffolded: Boolean,
    ) = LocalAppItem(
        id = id,
        name = name,
        brief = brief,
        workflow = LocalAppWorkflow.Draft,
        updatedAtMs = 1,
        scaffolded = scaffolded,
    )

    private fun row(uuid: String, isInit: Boolean = false) = LocalAppSessionRow(
        uuid = uuid,
        title = "会话 $uuid",
        relativeTime = "刚刚",
        messageCount = 2,
        isInit = isInit,
    )

    /**
     * Both create entry points must be GATED on the in-flight twin, not merely
     * able to read it.
     *
     * `createInFlight` is computed in the view model and published on the ui
     * state; a screen that never reads it leaves the finding exactly where it
     * was — named, computed, and never wired. Source-level for the same reason
     * as the dialog test above: `androidx.compose.ui.test` is instrumented-only
     * in this module, so there is no runtime way here to observe a disabled
     * button.
     */
    @Test
    fun `both create entry points are disabled while a create is in flight`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        val start = source.indexOf("IconButton(\n                        onClick = { onAction(LocalAppsAction.Create) },")
        assertTrue(
            "read the wrong file, or the top bar's create button was rewritten: " +
                "the multi-line IconButton call was not found",
            start >= 0,
        )
        val toolbar = source.substring(start, start + 400)
        assertTrue(
            "the library top bar's 「+」 must be disabled while a create is unresolved — " +
                "the create resolves out of band up to CREATE_RESULT_TIMEOUT_MS later, " +
                "and a second tap is otherwise answered with an error toast",
            "enabled = !state.createInFlight," in toolbar,
        )

        val emptyStart = source.indexOf("private fun EmptyApps(")
        assertTrue("read the wrong file: EmptyApps not found", emptyStart >= 0)
        val emptyEnd = source.indexOf("\nprivate fun ", emptyStart + 1)
        assertTrue("read the wrong file: the function after EmptyApps was not found", emptyEnd > emptyStart)
        val empty = source.substring(emptyStart, emptyEnd)
        assertTrue(
            "vacuity guard: the sliced region must still contain the create button",
            "Button(onClick = onCreate" in empty,
        )
        assertTrue(
            "the empty state's create button is the OTHER way into a create and must be " +
                "gated on the same latch",
            "enabled = !createInFlight" in empty,
        )
        assertTrue(
            "EmptyApps must be handed the flag by its caller",
            "createInFlight = state.createInFlight," in source,
        )
    }

    /**
     * The localized copy must actually be RENDERED.
     *
     * `localAppVerificationSummaryRes` / `localAppGateLabelRes` are pure and
     * unit-tested next door, which proves they map correctly and proves
     * nothing about whether any screen calls them. These two call sites are
     * the whole point of the mapping: the verification row, and the MCP
     * proposal approval sheet's gate list.
     */
    @Test
    fun `the screen renders localized verification summaries and gate labels`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        val rowStart = source.indexOf("private fun VerificationSummaryRow(")
        assertTrue("read the wrong file: VerificationSummaryRow not found", rowStart >= 0)
        val rowEnd = source.indexOf("\n@Composable", rowStart + 1)
        assertTrue("read the wrong file: the composable after the row was not found", rowEnd > rowStart)
        val row = source.substring(rowStart, rowEnd)
        assertTrue(
            "vacuity guard: the sliced region must still be the row's Text call",
            "Text(" in row,
        )
        assertTrue(
            "the engine sends `summary` as fixed English and `code` as the key for it: " +
                "the row must render the mapped copy, not the raw sentence",
            "localAppVerificationSummaryRes(summary.status, summary.code)" in row,
        )
        assertTrue(
            "an unrecognized (or absent) code must still render the engine's sentence",
            "else summary.summary" in row,
        )

        assertEquals(
            "the approval sheet's gate list must render the localized label",
            1,
            Regex("append\\(localAppGateLabel\\(gate, context\\)\\)").findAll(source).count(),
        )
        assertEquals(
            "and the localized detail",
            1,
            Regex("localAppGateDetail\\(gate, context\\)").findAll(source).count(),
        )
        assertTrue(
            "the raw English label must no longer be appended directly",
            "append(gate.name)" !in source,
        )
    }
}
