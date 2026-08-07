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
            LocalAppItem("a", "客户跟进", brief = "记录客户跟进情况", LocalAppWorkflow.Ready, updatedAtMs = 2),
            LocalAppItem("b", "运营看板", brief = "运营数据看板", LocalAppWorkflow.Generating, updatedAtMs = 1),
        ),
        query = query,
        distributionMode = LocalAppRuntimeMode.StaticExport,
    )

    // Was `filteredApps_combinesNameSearchAndTemplateFilter`: the template
    // filter half of that name is gone along with `templateFilter`/
    // `LocalAppItem.templateKind` (local-apps#questionnaire, Task 18) — the
    // step 1 brief's `an app item carries the brief instead of a template
    // kind` / `filtering no longer depends on a template kind` tests below
    // cover the replacement directly. This migrates the surviving half: name
    // search still narrows `filteredApps`.
    @Test
    fun `filteredApps narrows by name search`() {
        assertEquals(listOf("a"), state(query = "客户").filteredApps.map { it.id })
        assertEquals(emptyList<LocalAppItem>(), state(query = "不存在").filteredApps)
    }

    @Test
    fun `an app item carries the brief instead of a template kind`() {
        val item = LocalAppItem(id = "a", name = "记事本", brief = "一个记事本 app", workflow = LocalAppWorkflow.Ready, updatedAtMs = 0)
        assertEquals("一个记事本 app", item.brief)
    }

    @Test
    fun `filtering no longer depends on a template kind`() {
        val state = LocalAppsUiState(
            apps = listOf(LocalAppItem(id = "a", name = "N", brief = "b", workflow = LocalAppWorkflow.Ready, updatedAtMs = 0)),
            distributionMode = LocalAppRuntimeMode.StaticExport,
        )
        assertEquals(1, state.filteredApps.size)
    }

    @Test
    fun previewUrl_prefersRuntimeUrlBecauseTheGateAnnouncesNone() {
        val running = LocalAppRuntime(state = LocalAppRuntimeState.Running, url = "http://127.0.0.1:3100")
        val gated = state().copy(
            apps = listOf(
                LocalAppItem(
                    "a",
                    "客户跟进",
                    "记录客户跟进情况",
                    LocalAppWorkflow.AwaitingPreviewConfirmation,
                    runtime = running,
                    updatedAtMs = 2,
                ),
            ),
            previews = mapOf("a" to LocalAppPreview("a", 1u, "gate-1", url = null)),
        )
        assertEquals("http://127.0.0.1:3100", gated.previewUrl("a"))

        val announced = gated.copy(
            apps = gated.apps.map { it.copy(runtime = LocalAppRuntime()) },
            previews = mapOf("a" to LocalAppPreview("a", 1u, "gate-1", url = "http://127.0.0.1:3200")),
        )
        assertEquals("http://127.0.0.1:3200", announced.previewUrl("a"))
        assertNull(gated.previewUrl("missing"))
    }

    @Test
    fun readable_structuredValuesAreStableForSuggestionDiffs() {
        assertEquals("名称、状态", LocalAppDesignValue.StringList(listOf("名称", "状态")).readable())
        assertEquals("是", LocalAppDesignValue.Toggle(true).readable())
    }
}
