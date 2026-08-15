package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.PermissionKindDto
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.WorkerInfoDto

/**
 * SHIP-BLOCKER #3 — the Android permission-prompt surface.
 *
 * The engine parks any write/Bash tool that needs approval and emits a
 * [PermissionRequest] OUTBOUND through the `AndroidPermissionSink` callback
 * interface. The host pushes that request into a UI-facing [PermissionPromptState]
 * (the head of the queue), renders an allow/deny prompt, and resolves it by
 * submitting `ClientCommand.ApprovePermission{request_id, response}` /
 * `DenyPermission{request_id}` back through the [MobileEngineHandle]. Without this
 * round-trip the turn hangs forever (the prior `NoopPermissionSink` dropped the
 * request).
 *
 * This file holds the PURE mapping ([permissionRequestToPrompt]) + the UI-facing
 * state shape ([PermissionPromptState]) ONLY — no engine / Android dependency — so
 * the request→prompt-state contract is exhaustively unit-testable on the plain JVM
 * (the data-class fixtures only CONSTRUCT generated UniFFI types; they never call
 * an exported function, so no native `.so` is loaded). Mirrors the Electron
 * renderer's `PermissionPrompt.describe` / `previewToolInput`.
 */

/**
 * The UI-facing render model for one parked permission request — the Android
 * analog of the Electron prompt's `{ title, detail }` plus the correlator the
 * resolution command echoes back.
 */
data class PermissionPromptState(
    /** Correlator echoed back in `ApprovePermission` / `DenyPermission`. */
    val requestId: ULong,
    /** Human title (e.g. "允许 Bash？"). */
    val title: String,
    /** One-line / multi-line detail (a tool-input preview, the plan text, …). */
    val detail: String,
    /** Worker attribution when the request originates from a sub-agent (reserved). */
    val worker: WorkerPrompt? = null,
    /** Immutable engine ownership; presentation remains global across navigation. */
    val originSessionId: String? = null,
    val ownerTurnId: ULong? = null,
)

/** Worker attribution rendered on a [PermissionPromptState] (reserved). */
data class WorkerPrompt(
    val name: String,
    val color: String,
    val team: String? = null,
)

/**
 * PURE mapping from one inbound [PermissionRequest] to the UI-facing
 * [PermissionPromptState]. Total over [PermissionKindDto]'s live + reserved
 * variants; an unknown future kind (the enum is `#[non_exhaustive]`, so an `else`
 * is required) falls back to a generic prompt rather than a blank dialog. Mirrors
 * the Electron `describe(request)` switch.
 */
fun permissionRequestToPrompt(
    request: PermissionRequest,
    strings: ConversationStrings = DefaultConversationStrings,
): PermissionPromptState {
    // The generated `PermissionKindDto` is a closed sealed class (UniFFI renders
    // the Rust `#[non_exhaustive]` enum as exactly its known variants), so this
    // `when` is exhaustive without an `else`. If a future binding adds a kind, this
    // fails to compile here — a deliberate signal to render it, not silently drop
    // a request and hang the turn.
    val (title, detail) = when (val kind = request.kind) {
        is PermissionKindDto.ToolUseConfirm ->
            strings.resolve(R.string.permission_allow_tool, "允许 %1\$s？", kind.toolName) to
                previewToolInput(kind.toolInputJson)
        is PermissionKindDto.ExitPlanMode ->
            strings.resolve(R.string.permission_exit_plan_mode, "退出计划模式并继续？") to kind.plan
        is PermissionKindDto.BypassPermissionsMode ->
            strings.resolve(R.string.permission_bypass_mode_title, "开启免确认（绕过权限）模式？") to
                strings.resolve(R.string.permission_bypass_confirmation_detail, "智能体将不再就后续操作征求确认。")
    }
    return PermissionPromptState(
        requestId = request.requestId,
        title = title,
        detail = detail,
        worker = request.worker?.toPrompt()
            ?: request.owner?.workerName?.takeIf(String::isNotBlank)?.let {
                WorkerPrompt(name = it, color = DEFAULT_WORKER_COLOR)
            },
        originSessionId = request.owner?.sessionId,
        ownerTurnId = request.owner?.turnId,
    )
}

private const val DEFAULT_WORKER_COLOR = "#7C8CF8"

/** Map the reserved [WorkerInfoDto] onto the UI-facing [WorkerPrompt]. */
private fun WorkerInfoDto.toPrompt(): WorkerPrompt =
    WorkerPrompt(name = name, color = color, team = team)

/** Tool-input fields worth previewing, in priority order (mirrors Electron). */
private val SALIENT_FIELDS = listOf("command", "file_path", "path", "pattern", "query")

/**
 * Best-effort, never-throwing one-line preview of a tool's JSON input — the same
 * shape the Electron prompt shows: the most salient field (command / file_path /
 * path / pattern / query) when present, otherwise the raw JSON, otherwise empty.
 *
 * Implemented as a tiny dependency-free string scan (no `org.json`) so it is
 * exercised faithfully on the plain JVM unit host, where Android's bundled
 * `org.json` is a return-default stub. It reads the FIRST non-empty string value
 * for a salient key in a flat top-level object; anything it can't extract falls
 * back to the raw JSON.
 */
internal fun previewToolInput(inputJson: String): String {
    if (inputJson.isBlank()) return ""
    for (field in SALIENT_FIELDS) {
        extractJsonStringValue(inputJson, field)?.let { if (it.isNotEmpty()) return it }
    }
    return inputJson
}

/**
 * Pull the string value for a top-level `"key": "value"` pair out of a flat JSON
 * object, decoding the common `\"` / `\\` / `\n` / `\t` escapes. Returns `null`
 * when the key is absent or its value isn't a JSON string. Best-effort and
 * never-throwing — not a general JSON parser, just enough for a preview line.
 */
private fun extractJsonStringValue(json: String, key: String): String? {
    val needle = "\"$key\""
    var i = json.indexOf(needle)
    if (i < 0) return null
    i += needle.length
    // Skip whitespace + the colon.
    while (i < json.length && json[i].isWhitespace()) i++
    if (i >= json.length || json[i] != ':') return null
    i++
    while (i < json.length && json[i].isWhitespace()) i++
    // Value must be a JSON string for a clean preview.
    if (i >= json.length || json[i] != '"') return null
    i++
    val sb = StringBuilder()
    while (i < json.length) {
        val c = json[i]
        when {
            c == '\\' && i + 1 < json.length -> {
                when (val esc = json[i + 1]) {
                    'n' -> sb.append('\n')
                    't' -> sb.append('\t')
                    'r' -> sb.append('\r')
                    '"' -> sb.append('"')
                    '\\' -> sb.append('\\')
                    '/' -> sb.append('/')
                    else -> sb.append(esc)
                }
                i += 2
            }
            c == '"' -> return sb.toString()
            else -> {
                sb.append(c)
                i++
            }
        }
    }
    // Unterminated string — no clean value.
    return null
}
