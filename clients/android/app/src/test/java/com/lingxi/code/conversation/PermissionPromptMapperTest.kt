package com.lingxi.code.conversation

import com.lingxi.code.bindings.PermissionKindDto
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.WorkerInfoDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Exhaustive coverage of the PURE [permissionRequestToPrompt] mapper — the seam
 * that turns an OUTBOUND engine [PermissionRequest] into the UI-facing
 * [PermissionPromptState] the Compose prompt renders (SHIP-BLOCKER #3).
 *
 * Like [ClientEventMapperTest], it has no engine / Android dependency, so it runs
 * on the plain JVM where `buildAndroidEngine` is unavailable. The fixtures only
 * CONSTRUCT generated UniFFI data types — they never call an exported function, so
 * no native `.so` is loaded. The tool-input preview uses a dependency-free string
 * scan (not Android's `org.json` stub), so its extraction is exercised faithfully
 * here.
 */
class PermissionPromptMapperTest {

    private fun request(
        kind: PermissionKindDto,
        requestId: ULong = 7uL,
        worker: WorkerInfoDto? = null,
    ) = PermissionRequest(requestId = requestId, kind = kind, worker = worker)

    // --- tool_use_confirm (the only live-sourced kind) --------------------

    @Test
    fun toolUseConfirm_titleNamesTool_correlatorPreserved() {
        val prompt = permissionRequestToPrompt(
            request(
                kind = PermissionKindDto.ToolUseConfirm(
                    toolName = "Bash",
                    toolInputJson = """{"command":"rm -rf build"}""",
                    defaultAllow = false,
                ),
                requestId = 42uL,
            ),
        )
        assertEquals(42uL, prompt.requestId)
        assertTrue(prompt.title.contains("Bash"))
        // The salient JSON field is surfaced as the detail preview.
        assertEquals("rm -rf build", prompt.detail)
        assertNull(prompt.worker)
    }

    @Test
    fun toolUseConfirm_prefersFilePath_whenNoCommand() {
        val prompt = permissionRequestToPrompt(
            request(
                PermissionKindDto.ToolUseConfirm(
                    toolName = "Write",
                    toolInputJson = """{"file_path":"/tmp/x.txt","content":"hi"}""",
                    defaultAllow = false,
                ),
            ),
        )
        assertEquals("/tmp/x.txt", prompt.detail)
    }

    @Test
    fun toolUseConfirm_rawJsonFallback_whenNoSalientField() {
        val raw = """{"foo":1,"bar":2}"""
        val prompt = permissionRequestToPrompt(
            request(
                PermissionKindDto.ToolUseConfirm(
                    toolName = "X",
                    toolInputJson = raw,
                    defaultAllow = false,
                ),
            ),
        )
        assertEquals(raw, prompt.detail)
    }

    @Test
    fun toolUseConfirm_emptyInput_emptyDetail_doesNotThrow() {
        val prompt = permissionRequestToPrompt(
            request(
                PermissionKindDto.ToolUseConfirm(
                    toolName = "X",
                    toolInputJson = "",
                    defaultAllow = false,
                ),
            ),
        )
        assertEquals("", prompt.detail)
    }

    @Test
    fun toolUseConfirm_decodesEscapesInPreview() {
        val prompt = permissionRequestToPrompt(
            request(
                PermissionKindDto.ToolUseConfirm(
                    toolName = "Bash",
                    toolInputJson = """{"command":"echo \"hi\"\nls"}""",
                    defaultAllow = false,
                ),
            ),
        )
        assertEquals("echo \"hi\"\nls", prompt.detail)
    }

    @Test
    fun toolUseConfirm_malformedJson_fallsBackToRaw_doesNotThrow() {
        val raw = "{not json"
        val prompt = permissionRequestToPrompt(
            request(
                PermissionKindDto.ToolUseConfirm(
                    toolName = "X",
                    toolInputJson = raw,
                    defaultAllow = false,
                ),
            ),
        )
        assertEquals(raw, prompt.detail)
    }

    // --- reserved kinds ----------------------------------------------------

    @Test
    fun exitPlanMode_showsPlanAsDetail() {
        val prompt = permissionRequestToPrompt(
            request(PermissionKindDto.ExitPlanMode(plan = "step 1\nstep 2")),
        )
        assertEquals("step 1\nstep 2", prompt.detail)
        assertTrue(prompt.title.isNotEmpty())
    }

    @Test
    fun bypassPermissionsMode_hasTitleAndDetail() {
        val prompt = permissionRequestToPrompt(
            request(PermissionKindDto.BypassPermissionsMode),
        )
        assertTrue(prompt.title.isNotEmpty())
        assertTrue(prompt.detail.isNotEmpty())
    }

    // --- worker attribution (reserved) ------------------------------------

    @Test
    fun worker_isMappedThrough_whenPresent() {
        val prompt = permissionRequestToPrompt(
            request(
                kind = PermissionKindDto.ToolUseConfirm(
                    toolName = "Bash",
                    toolInputJson = "{}",
                    defaultAllow = false,
                ),
                worker = WorkerInfoDto(name = "scout", color = "#ff8800", team = "alpha"),
            ),
        )
        assertEquals("scout", prompt.worker?.name)
        assertEquals("#ff8800", prompt.worker?.color)
        assertEquals("alpha", prompt.worker?.team)
    }

    @Test
    fun worker_nullTeam_isTolerated() {
        val prompt = permissionRequestToPrompt(
            request(
                kind = PermissionKindDto.ToolUseConfirm(
                    toolName = "Bash",
                    toolInputJson = "{}",
                    defaultAllow = false,
                ),
                worker = WorkerInfoDto(name = "scout", color = "#fff", team = null),
            ),
        )
        assertEquals("scout", prompt.worker?.name)
        assertNull(prompt.worker?.team)
    }
}
