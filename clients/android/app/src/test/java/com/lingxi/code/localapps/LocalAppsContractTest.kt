package com.lingxi.code.localapps

import com.lingxi.code.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LocalAppsContractTest {
    private fun state(
        query: String = "",
    ) = LocalAppsUiState(
        loading = false,
        apps = listOf(
            LocalAppItem("a", "客户跟进", brief = "记录客户跟进情况", workflow = LocalAppWorkflow.PublishedUnverified, updatedAtMs = 2, scaffolded = true),
            LocalAppItem("b", "运营看板", brief = "运营数据看板", workflow = LocalAppWorkflow.Draft, updatedAtMs = 1, scaffolded = true),
        ),
        query = query,
        distributionMode = LocalAppRuntimeMode.StaticExport,
    )

    @Test
    fun `filteredApps narrows by name search`() {
        assertEquals(listOf("a"), state(query = "客户").filteredApps.map { it.id })
        assertEquals(emptyList<LocalAppItem>(), state(query = "不存在").filteredApps)
    }

    /**
     * A shell is listed while the box is empty and drops out once the user
     * types — its stored name is the engine's `"untitled"` placeholder, so
     * matching on it would surface that placeholder indirectly and pretend a
     * shell has a searchable identity it has not been given yet.
     */
    @Test
    fun `a draft is listed unfiltered but never matches a search`() {
        val withDraft = state().copy(
            apps = state().apps + LocalAppItem(
                id = "shell",
                name = "untitled",
                brief = "",
                workflow = LocalAppWorkflow.Draft,
                updatedAtMs = 3,
                scaffolded = false,
            ),
        )

        assertEquals(
            listOf("a", "b", "shell"),
            withDraft.filteredApps.map { it.id },
        )
        assertEquals(
            emptyList<String>(),
            withDraft.copy(query = "untitled").filteredApps.map { it.id },
        )
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
    fun `missing managed MCP inventory falls back to needs-setup and disabled`() {
        val managed = LocalAppsUiState(
            apps = state().apps,
            distributionMode = LocalAppRuntimeMode.StaticExport,
        ).managedMcp("a")

        assertEquals(LocalAppManagedMcpStatus.NeedsSetup, managed.status)
        assertEquals(false, managed.enabled)
        assertEquals("a", managed.appId)
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
                    LocalAppWorkflow.PublishedUnverified,
                    runtime = running,
                    updatedAtMs = 2,
                    scaffolded = true,
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

    @Test
    fun `published unverified apps remain searchable and published`() {
        val published = state().apps.first()

        assertEquals(LocalAppWorkflow.PublishedUnverified, published.workflow)
        assertEquals(true, published.workflow.isPublished)
        assertEquals(listOf("a"), state(query = "客户").filteredApps.map { it.id })
    }

    @Test
    fun `published badges separate verification pending from runtime errors`() {
        assertEquals(
            listOf(
                LocalAppStatusBadgeKind.PublishedUnverified,
                LocalAppStatusBadgeKind.VerificationPending,
            ),
            localAppStatusBadges(
                workflow = LocalAppWorkflow.PublishedUnverified,
                runtimeError = null,
                runtimeProfileStatus = null,
                mcpVerification = null,
                uiVerification = null,
            ),
        )
        assertEquals(
            listOf(
                LocalAppStatusBadgeKind.PublishedVerified,
                LocalAppStatusBadgeKind.Error,
            ),
            localAppStatusBadges(
                workflow = LocalAppWorkflow.PublishedVerified,
                runtimeError = "build failed",
                runtimeProfileStatus = null,
                mcpVerification = null,
                uiVerification = null,
            ),
        )
    }

    /**
     * The engine's verification `summary` is a fixed English sentence and
     * `code` is the localization key for it. Existing MCP summary codes and
     * the three Host UI-verification codes are localized here.
     * `active_state_corrupt` deliberately has no client key and is covered by
     * the fallback test below, the way iOS covers it in LocalAppsStoreTests.swift.
     *
     * The distinctness assertion is a vacuity guard: multiple arms resolving to
     * the same id (or to 0) would satisfy every equality below individually.
     */
    @Test
    fun `every verification code maps to its own localized summary`() {
        val ids = listOf(
            R.string.local_apps_verification_summary_needs_setup,
            R.string.local_apps_verification_summary_needs_revalidation,
            R.string.local_apps_verification_summary_verification_unavailable,
            R.string.local_apps_verification_summary_ui_verification_required,
            R.string.local_apps_verification_summary_ui_verification_passed,
            R.string.local_apps_verification_summary_ui_verification_corrupt,
            R.string.local_apps_verification_summary_passed,
        )
        assertEquals("vacuity guard: all seven keys must be distinct ids", 7, ids.toSet().size)

        assertEquals(
            R.string.local_apps_verification_summary_needs_setup,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Unverified, "needs_setup"),
        )
        assertEquals(
            R.string.local_apps_verification_summary_needs_revalidation,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Unverified, "needs_revalidation"),
        )
        assertEquals(
            R.string.local_apps_verification_summary_verification_unavailable,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Unavailable, "verification_unavailable"),
        )
        assertEquals(
            "a null code with status Passed is the legacy MCP pass, not a missing one",
            R.string.local_apps_verification_summary_passed,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Passed, null),
        )
        assertEquals(
            R.string.local_apps_verification_summary_ui_verification_required,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Unverified, "ui_verification_required"),
        )
        assertEquals(
            R.string.local_apps_verification_summary_ui_verification_passed,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Passed, "ui_verification_passed"),
        )
        assertEquals(
            R.string.local_apps_verification_summary_ui_verification_corrupt,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Failed, "ui_verification_corrupt"),
        )
        assertNotEquals(
            "a UI QA pass must never use the nil-code MCP verification sentence",
            R.string.local_apps_verification_summary_passed,
            localAppVerificationSummaryRes(LocalAppVerificationStatus.Passed, "ui_verification_passed"),
        )
    }

    /**
     * `active_state_corrupt` is the engine's FIFTH code (local_apps_host.rs:2025,
     * status Failed) and the one it ships with no client key on purpose, so it
     * is the real production case for this arm — not a hypothetical. iOS pins
     * the same code in LocalAppsStoreTests.swift. The arbitrary code after it
     * keeps the future-proofing half honest, and the two null cases guard the
     * other direction: guessing the "passed" copy for a null code whose status
     * is NOT Passed would put words in the engine's mouth.
     */
    @Test
    fun `an unknown verification code falls back to the engine sentence`() {
        assertNull(localAppVerificationSummaryRes(LocalAppVerificationStatus.Failed, "active_state_corrupt"))
        assertNull(localAppVerificationSummaryRes(LocalAppVerificationStatus.Failed, "some_future_code"))
        assertNull(localAppVerificationSummaryRes(LocalAppVerificationStatus.Pending, null))
        assertNull(localAppVerificationSummaryRes(LocalAppVerificationStatus.Unverified, null))
    }

    /**
     * The two gate ids the host defines today, and the fallback for a third.
     * Mirrors iOS's `localizedGateLabel` / `localizedGateDetail`.
     */
    @Test
    fun `gate ids map to localized labels and the runner detail`() {
        assertEquals(
            "vacuity guard: the two gate labels must be distinct ids",
            2,
            setOf(
                R.string.local_apps_create_confirm_gate_mcp_qa_label,
                R.string.local_apps_create_confirm_gate_ui_runner_label,
            ).size,
        )
        assertEquals(
            R.string.local_apps_create_confirm_gate_mcp_qa_label,
            localAppGateLabelRes("mcp_qa"),
        )
        assertEquals(
            R.string.local_apps_create_confirm_gate_ui_runner_label,
            localAppGateLabelRes("ui_runner"),
        )
        assertNull("an unknown id renders the engine's own label", localAppGateLabelRes("ui-smoke"))
        assertNull("an unidentified gate renders the engine's own label", localAppGateLabelRes(""))

        assertEquals(
            R.string.local_apps_create_confirm_gate_ui_runner_unavailable_detail,
            localAppGateDetailRes("ui_runner", available = false),
        )
        assertNull(
            "an AVAILABLE runner sends no detail worth translating",
            localAppGateDetailRes("ui_runner", available = true),
        )
        assertNull(localAppGateDetailRes("mcp_qa", available = false))
    }

    /**
     * The unavailable-runner marker is id-INDEPENDENT, unlike the detail line
     * above. iOS renders it on `if !gate.available` in both sheets
     * (LocalAppApprovalSheets.swift:140 and :315) without looking at the id, so
     * an unavailable `mcp_qa` gate must say so on Android too — before this the
     * key existed in all five locales with ZERO Android consumers and the
     * Android sheets showed only the status badge.
     */
    @Test
    fun `an unavailable gate is marked whatever its id`() {
        assertEquals(
            R.string.local_apps_create_confirm_gate_runner_unavailable,
            localAppGateUnavailableRes(available = false),
        )
        assertNull(
            "an available runner needs no marker",
            localAppGateUnavailableRes(available = true),
        )
        assertNotEquals(
            "the id-independent marker and the ui_runner detail are different copy, " +
                "and a missing UI runner renders both",
            R.string.local_apps_create_confirm_gate_runner_unavailable,
            R.string.local_apps_create_confirm_gate_ui_runner_unavailable_detail,
        )
    }
}
