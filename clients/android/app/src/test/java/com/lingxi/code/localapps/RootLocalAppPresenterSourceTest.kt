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
}
