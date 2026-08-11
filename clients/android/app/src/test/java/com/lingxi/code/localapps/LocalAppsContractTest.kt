package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LocalAppsContractTest {
    private fun state(
        query: String = "",
    ) = LocalAppsUiState(
        loading = false,
        apps = listOf(
            LocalAppItem("a", "客户跟进", brief = "记录客户跟进情况", workflow = LocalAppWorkflow.Ready, updatedAtMs = 2),
            LocalAppItem("b", "运营看板", brief = "运营数据看板", workflow = LocalAppWorkflow.Draft, updatedAtMs = 1),
        ),
        query = query,
        distributionMode = LocalAppRuntimeMode.StaticExport,
    )

    @Test
    fun `filteredApps narrows by name search`() {
        assertEquals(listOf("a"), state(query = "客户").filteredApps.map { it.id })
        assertEquals(emptyList<LocalAppItem>(), state(query = "不存在").filteredApps)
    }

    @Test
    fun `an app item carries the brief and its workspace-relative path`() {
        val item = LocalAppItem(
            id = "a",
            name = "记事本",
            brief = "一个记事本 app",
            workflow = LocalAppWorkflow.Ready,
            updatedAtMs = 0,
            workspaceRel = "apps/a/workspace",
            initSessionId = "11111111-1111-4111-8111-111111111111",
        )
        assertEquals("一个记事本 app", item.brief)
        assertEquals("apps/a/workspace", item.workspaceRel)
    }

    @Test
    fun `the details screen defaults to the sessions tab`() {
        assertEquals(
            LocalAppDetailsTab.Sessions,
            LocalAppsUiState(distributionMode = LocalAppRuntimeMode.StaticExport).selectedDetailsTab,
        )
        assertEquals(LocalAppDetailsTab.Sessions, LocalAppsDestination.Details("a").tab)
    }

    @Test
    fun `previewUrl follows the runtime loopback url`() {
        val running = LocalAppRuntime(state = LocalAppRuntimeState.Running, url = "http://127.0.0.1:3100")
        val gated = state().copy(
            apps = listOf(
                LocalAppItem(
                    "a",
                    "客户跟进",
                    "记录客户跟进情况",
                    LocalAppWorkflow.Ready,
                    runtime = running,
                    updatedAtMs = 2,
                ),
            ),
        )
        assertEquals("http://127.0.0.1:3100", gated.previewUrl("a"))
        assertNull(gated.previewUrl("missing"))

        val stopped = gated.copy(
            apps = gated.apps.map { it.copy(runtime = LocalAppRuntime()) },
            details = mapOf(
                "a" to LocalAppDetails(
                    appId = "a",
                    workspaceRelativePath = "apps/a/workspace",
                    runtime = LocalAppRuntime(url = "http://127.0.0.1:3200"),
                ),
            ),
        )
        assertEquals(
            "the details snapshot's runtime url is the fallback",
            "http://127.0.0.1:3200",
            stopped.previewUrl("a"),
        )
    }
}
