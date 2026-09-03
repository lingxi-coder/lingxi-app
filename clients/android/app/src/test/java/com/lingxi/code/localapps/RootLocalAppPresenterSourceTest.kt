package com.lingxi.code.localapps

import java.io.File
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
