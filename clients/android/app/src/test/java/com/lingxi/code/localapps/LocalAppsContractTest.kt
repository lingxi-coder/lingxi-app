package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LocalAppsContractTest {
    private val template = LocalAppTemplate(
        kind = "crud_tracker",
        version = 1u,
        name = "CRUD Tracker",
        description = "Tracker",
        steps = emptyList(),
    )

    private fun state(
        query: String = "",
        filter: String? = null,
    ) = LocalAppsUiState(
        loading = false,
        templatesLoading = false,
        templates = listOf(template),
        apps = listOf(
            LocalAppItem("a", "客户跟进", "crud_tracker", "CRUD Tracker", LocalAppWorkflow.Ready, updatedAtMs = 2),
            LocalAppItem("b", "运营看板", "dashboard", "Dashboard", LocalAppWorkflow.Generating, updatedAtMs = 1),
        ),
        query = query,
        templateFilter = filter,
        distributionMode = LocalAppRuntimeMode.StaticExport,
    )

    @Test
    fun filteredApps_combinesNameSearchAndTemplateFilter() {
        assertEquals(listOf("a"), state(query = "客户", filter = "crud_tracker").filteredApps.map { it.id })
        assertEquals(emptyList<LocalAppItem>(), state(query = "看板", filter = "crud_tracker").filteredApps)
    }

    @Test
    fun previewUrl_prefersRuntimeUrlBecauseTheGateAnnouncesNone() {
        val running = LocalAppRuntime(state = LocalAppRuntimeState.Running, url = "http://127.0.0.1:3100")
        val gated = state().copy(
            apps = listOf(
                LocalAppItem(
                    "a",
                    "客户跟进",
                    "crud_tracker",
                    "CRUD Tracker",
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
