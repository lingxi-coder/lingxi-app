package com.lingxi.code.localapps

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RootLocalAppPresenterSourceTest {
    @Test
    fun `root screen hosts the local app approval sheet presenter`() {
        val source = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        assertTrue(
            "RootScreen must own the Local App approval presenter so tokenized receipts survive page switches",
            "LocalAppApprovalSheetDialog(" in source,
        )
        assertTrue(
            "the presenter must be driven by the hoisted pendingApprovalSheet state",
            "localAppsState.pendingApprovalSheet?.let" in source,
        )
    }

    @Test
    fun `phase8 surfaces do not keep raw placeholder copy`() {
        val localAppsScreen = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()
        val skillsPage = File("src/main/java/com/lingxi/code/settings/SkillsPages.kt").readText()
        val mcpPages = File("src/main/java/com/lingxi/code/settings/MCPPages.kt").readText()

        assertTrue("approval sheet must use generated create-confirm title", "R.string.local_apps_create_confirm_title" in localAppsScreen)
        assertTrue("approval sheet must use generated MCP-proposal title", "R.string.local_apps_mcp_proposal_title" in localAppsScreen)
        assertTrue("approval sheet must not keep raw fact labels", "LocalAppApprovalFact(\"" !in localAppsScreen)
        assertTrue("approval sheet must not keep hardcoded approve text", "Text(\"批准\")" !in localAppsScreen)
        assertTrue("skills page must use generated plugin title", "R.string.local_apps_plugin_title" in skillsPage)
        assertTrue("skills page must not keep raw plugin section label", "SettingsSection(label = \"Plugin\")" !in skillsPage)
        assertTrue("MCP pages must use generated managed-source title", "R.string.local_apps_plugin_managed_mcp_source" in mcpPages)
        assertTrue("MCP pages must not keep the raw managed-source warning", "Managed source cannot edit transport or remove this server." !in mcpPages)
    }

    @Test
    fun `the create-confirmation dependency status is localized against the tokens it actually receives`() {
        val source = File("src/main/java/com/lingxi/code/localapps/LocalAppsScreen.kt").readText()

        assertTrue(
            "the dependency-status call site (inside buildString, a non-@Composable context) must " +
                "use the runtime-profile-token localizer, not the old `may_be_required` / " +
                "`not_required` / `not_needed` one — nothing feeding it ever emits those tokens, so " +
                "every one of that mapping's arms was unreachable and it silently returned the raw " +
                "token instead",
            "dependency.downloadStatus?.localizedRuntimeProfileStatus(context)" in source,
        )
        assertTrue(
            "the now-dead may_be_required/not_required/not_needed, context-taking overload must be " +
                "deleted, not left with zero callers",
            "\"may_be_required\" -> context.getString(R.string.local_apps_dependency_change_download_may_be_required)" !in source,
        )
        assertEquals(
            "there must be exactly ONE localizer for the runtime-profile token table. A " +
                "@Composable twin with zero call sites used to sit beside the live one; two " +
                "spellings of one table is how the next divergence gets introduced (an arm added " +
                "to one, the sheet still rendering the raw token)",
            1,
            Regex("fun String\\.localizedRuntimeProfileStatus\\(").findAll(source).count(),
        )
        val viewModel = File("src/main/java/com/lingxi/code/localapps/LocalAppsViewModel.kt").readText()
        assertTrue(
            "the identity function that \"localized\" nothing (every arm returned its own input) " +
                "must be deleted, not left as dead code that looks like it does something",
            "private fun String.localizedTokenOrSelf" !in viewModel,
        )
    }

    @Test
    fun `drawer onCreateApp does not flip session mode before the create lands`() {
        val source = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        val createStart = source.indexOf("onCreateApp = {")
        assertTrue("read the wrong file: onCreateApp callback not found", createStart >= 0)
        val createEnd = source.indexOf("onOpenApps = {", createStart)
        assertTrue("read the wrong file: onOpenApps callback not found after onCreateApp", createEnd > createStart)
        val onCreateAppBody = source.substring(createStart, createEnd)

        assertTrue(
            "vacuity guard: onCreateApp callback body must still forward to createAppFromDrawer()",
            "localAppsViewModel.createAppFromDrawer()" in onCreateAppBody,
        )
        assertTrue(
            "onCreateApp must not set SessionMode.Code before the create starts: that Compose state " +
                "write triggers the mode-reconciliation LaunchedEffect, which rebinds the engine source " +
                "and clears the in-flight create before the landing collector (which sets the mode on " +
                "the correct side, after createdAppLandings emits) ever runs",
            "setConversationMode(SessionMode.Code)" !in onCreateAppBody,
        )

        // Sibling regression guard: onOpenApps starts no create, so its own mode
        // flip is a different code path and must be left alone by this fix.
        val onOpenAppsEnd = source.indexOf("},", createEnd)
        assertTrue("read the wrong file: onOpenApps callback body not found", onOpenAppsEnd > createEnd)
        val onOpenAppsBody = source.substring(createEnd, onOpenAppsEnd)
        assertTrue(
            "sibling onOpenApps callback should be left untouched by the onCreateApp fix",
            "setConversationMode(SessionMode.Code)" in onOpenAppsBody,
        )
        assertTrue(
            "onOpenApps must reset the library destination via openLibrary(): a library-origin " +
                "create (the '+' on a library card) parks the cover on the new shell's Details page, " +
                "and Refresh alone never resets it, so browsing here afterwards would reopen onto " +
                "that armed, unscaffolded shell instead of the library",
            "localAppsViewModel.openLibrary()" in onOpenAppsBody,
        )
    }

    @Test
    fun `created-app landing re-arms on cancellation and reports a stalled session-ready wait`() {
        val source = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        val landingStart = source.indexOf("localAppsViewModel.createdAppLandings.collect")
        assertTrue("read the wrong file: created-app landing collector not found", landingStart >= 0)
        val landingEnd = source.indexOf("// Tapping a DRAFT card", landingStart)
        assertTrue(
            "read the wrong file: the draft-card collector that ends the landing collector was not found",
            landingEnd > landingStart,
        )
        val landing = source.substring(landingStart, landingEnd)

        assertTrue(
            "the collector body must be wrapped so a CancellationException re-arms the landing: " +
                "`receiveAsFlow()` hands the element to this body before it runs, and " +
                "repeatOnLifecycle(RESUMED) cancels this coroutine the instant the user backgrounds " +
                "the app — ordinary while this body is suspended in the retry loop's delay or either " +
                "withTimeoutOrNull wait, which together can span several seconds. Without a re-arm " +
                "the just-created app's landing is dropped for good, with no retry and no error",
            "catch (c: CancellationException)" in landing,
        )
        assertTrue(
            "the CancellationException handler must put the SAME landing back on the channel before " +
                "rethrowing, not swallow it",
            Regex("catch \\(c: CancellationException\\) \\{\\s*localAppsViewModel\\.rearmCreatedAppLanding\\(landing\\)\\s*\\n\\s*throw c").containsMatchIn(landing),
        )
        assertEquals(
            "a session-ready timeout after a SUCCESSFUL scope switch must be reported the same way " +
                "the switch-exhausted branch already is (twice: once per branch) — previously the " +
                "switch succeeding but the session never reporting ready silently dropped the kickoff " +
                "with no error and no retry",
            2,
            Regex("reportCreatedAppLandingExhausted\\(\\)").findAll(landing).count(),
        )
    }

    @Test
    fun `draft-card landing does not close the library before a refused scope switch, and re-arms on cancellation`() {
        val source = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        val landingStart = source.indexOf("localAppsViewModel.draftSessionLandings.collect")
        assertTrue("read the wrong file: draft-session landing collector not found", landingStart >= 0)
        val landingEnd = source.indexOf("// Recover the last active Project", landingStart)
        assertTrue(
            "read the wrong file: the block after the draft-session landing collector was not found",
            landingEnd > landingStart,
        )
        val landing = source.substring(landingStart, landingEnd)
        // Whitespace-normalized copy for the two structural checks below, so
        // they pin the CODE SHAPE (what closes over what) rather than one
        // exact indentation — reformatting must not silently defeat them, but
        // a real restructuring (dropping the gate, reordering the branches)
        // must still turn them red.
        val normalized = landing.replace(Regex("\\s+"), " ")

        assertTrue(
            "vacuity guard: the sliced region must still be the out-of-place switch call",
            "switchEngineScope(" in landing,
        )
        assertTrue(
            "the out-of-place branch's scope switch is refusable (a streaming turn, a pending " +
                "transition) and must gate `showingApps = false` on its own return value — closing " +
                "the library before a refusal is known strands the user outside it with no way back " +
                "to the draft they just tapped. Pinned via the code shape: the switch call's own " +
                "closing paren, the enclosing `if (`'s closing paren, then the brace and the " +
                "assignment, with nothing else between",
            "replacePendingTransition = true, ) ) { showingApps = false" in normalized,
        )
        assertTrue(
            "the out-of-place switch must pass replacePendingTransition = true, matching every " +
                "drawer call site, so a transient pending-transition refusal does not out-rank a " +
                "fresh tap",
            "replacePendingTransition = true" in landing,
        )
        val inPlaceCondition = "chatViewModel.sourceScope.value == ConversationScope.LocalApp(landing.appId)) {"
        val inPlaceStart = landing.indexOf(inPlaceCondition)
        assertTrue("read the wrong file: in-place branch condition not found", inPlaceStart >= 0)
        val elseStart = landing.indexOf("} else {", inPlaceStart)
        assertTrue("read the wrong file: else branch not found after the in-place condition", elseStart > inPlaceStart)
        assertTrue(
            "the in-place branch (already inside this app) cannot be refused, so it may still close " +
                "the library unconditionally, ahead of the switch/session calls rather than gated " +
                "behind them",
            "showingApps = false" in landing.substring(inPlaceStart, elseStart),
        )
        assertTrue(
            "this collector shares the created-app landing's one-shot-Channel-under-" +
                "repeatOnLifecycle(RESUMED) hazard and must re-arm on cancellation the same way",
            "catch (c: CancellationException)" in landing && "rearmDraftSessionLanding(landing)" in landing,
        )
    }

    @Test
    fun `created-app landing retries the scope switch and only sends kickoff once landed`() {
        val source = File("src/main/java/com/lingxi/code/RootScreen.kt").readText()

        // Slice the created-app landing collector out before asserting anything.
        // Two of the needles below have a SECOND, unrelated home in this same
        // file -- `switched = switchEngineScope(` also matches
        // `val switched = switchEngineScope(` in the fork-transition helper, and
        // `chatViewModel.sourceScope.value ==` also appears in the draft-resume
        // collector that follows this one. A whole-file containment check for
        // either could therefore never fail on the defect its own message names.
        val landingStart = source.indexOf("localAppsViewModel.createdAppLandings.collect")
        assertTrue("read the wrong file: created-app landing collector not found", landingStart >= 0)
        // The draft-card collector's leading comment is the first thing after
        // the landing collector's LaunchedEffect closes.
        val landingEnd = source.indexOf("// Tapping a DRAFT card", landingStart)
        assertTrue(
            "read the wrong file: the draft-card collector that ends the landing collector was not found",
            landingEnd > landingStart,
        )
        val landing = source.substring(landingStart, landingEnd)

        // Everything below is a source-text containment check INSIDE that slice.
        // It proves the named code is written in the landing collector; it does
        // not execute the collector and does not prove the runtime behaviour.
        assertTrue(
            "vacuity guard: the sliced landing collector must still name the retry bound constant, " +
                "otherwise the slice is empty or landed on the wrong region",
            "LANDING_SWITCH_ATTEMPTS" in landing,
        )
        assertTrue(
            "the landing retry loop must be bounded by attempt count, not a single one-shot switch",
            "while (!switched && attempt < LANDING_SWITCH_ATTEMPTS)" in landing,
        )
        assertTrue(
            "each retry attempt must consume switchEngineScope's return value into `switched`, " +
                "not fire-and-forget it: the switch REFUSES while a turn streams and returns false " +
                "without transitioning, so a discarded result spends all 40 attempts and always " +
                "falls through to the give-up branch",
            "switched = switchEngineScope(" in landing,
        )
        assertTrue(
            "before sending the kickoff, the landing collector must re-check the CURRENT scope " +
                "against the landing's app id (readiness alone says WHEN, never WHICH conversation " +
                "became ready)",
            "chatViewModel.sourceScope.value ==" in landing,
        )
        assertTrue(
            "the kickoff send must be guarded on the transcript still being empty, so a landing " +
                "that raced a real message never overwrites it",
            "messages.isEmpty()" in landing,
        )
        assertTrue(
            "when every retry is exhausted the give-up path must be named against the app via " +
                "reportCreatedAppLandingExhausted(), not silently dropped",
            "reportCreatedAppLandingExhausted()" in landing,
        )
    }
}
