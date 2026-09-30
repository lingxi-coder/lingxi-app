package com.lingxi.code.settings

import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.ManagedLocalAppMcpSource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Test

class LocalAppPluginSettingsTest {
    @Test
    fun localAppPlugin_toggleUpdatesState() {
        val store = SettingsStore()

        store.setLocalAppPluginEnabled(false)

        assertEquals(false, store.state.value.localAppPlugin.enabled)
    }

    @Test
    fun liveMcpRefresh_preservesManagedLocalAppMetadata() {
        val store = SettingsStore()
        val managed = ManagedLocalAppMcpSource(
            stableServerName = "local_app_demo",
            appName = "Demo",
            appId = "demo",
            buildDigestSummary = "bld:1234",
            catalogDigestSummary = "cat:5678",
            toolCount = 2,
            authoringRevision = "r7",
            uiVerification = "verified",
            mcpVerification = "pending",
            schemaSummary = "2 tools",
            annotationSummary = "ceiling/read-only",
            permissionCeiling = "allow_once",
        )
        store.setMcpServers(
            listOf(
                MCPServer(
                    id = "local_app_demo",
                    name = "local_app_demo",
                    url = "",
                    tools = 2,
                    status = ConnStatus.Connected,
                    enabled = true,
                    transport = "managed",
                    managedLocalApp = managed,
                ),
            ),
        )

        store.setMcpServers(
            listOf(
                MCPServer(
                    id = "local_app_demo",
                    name = "local_app_demo",
                    url = "",
                    tools = 0,
                    status = ConnStatus.Connected,
                    enabled = true,
                    transport = "managed",
                ),
            ),
        )

        val refreshed = store.state.value.mcpServers.single()
        assertEquals(2, refreshed.tools)
        assertNotNull(refreshed.managedLocalApp)
        assertEquals("Demo", refreshed.managedLocalApp?.appName)
    }

    @Test
    fun managedInventory_upsertsManagedRows() {
        val store = SettingsStore()

        store.setManagedLocalAppInventory(
            listOf(
                ManagedLocalAppMcpSource(
                    stableServerName = "local_app_inventory",
                    appName = "Inventory",
                    appId = "inventory",
                    buildDigestSummary = "build-1234",
                    catalogDigestSummary = "catalog-5678",
                    toolCount = 3,
                    authoringRevision = "r9",
                    uiVerification = "queued",
                    mcpVerification = "passed",
                    schemaSummary = "surface-1234",
                    annotationSummary = "{}",
                    permissionCeiling = "allow_session",
                ),
            ),
        )

        val managed = store.state.value.mcpServers.single { it.id == "local_app_inventory" }
        assertEquals("managed", managed.transport)
        assertEquals(3, managed.tools)
        assertEquals("Inventory", managed.managedLocalApp?.appName)
    }

    @Test
    fun managedInventory_rows_ignore_local_edit_and_remove_attempts() {
        val store = SettingsStore()
        store.setManagedLocalAppInventory(
            listOf(
                ManagedLocalAppMcpSource(
                    stableServerName = "local_app_inventory",
                    appName = "Inventory",
                    appId = "inventory",
                    buildDigestSummary = "build-1234",
                    catalogDigestSummary = "catalog-5678",
                    toolCount = 3,
                    authoringRevision = "r9",
                    uiVerification = "queued",
                    mcpVerification = "passed",
                    schemaSummary = "surface-1234",
                    annotationSummary = "{}",
                    permissionCeiling = "allow_session",
                ),
            ),
        )

        store.updateMcp("local_app_inventory") { it.copy(enabled = false, name = "forged") }
        store.removeMcp("local_app_inventory")

        val managed = store.state.value.mcpServers.single { it.id == "local_app_inventory" }
        assertEquals(true, managed.enabled)
        assertEquals("local_app_inventory", managed.name)
        assertNotNull(managed.managedLocalApp)
    }
}
