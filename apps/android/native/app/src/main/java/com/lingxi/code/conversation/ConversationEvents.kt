package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.CodeSegmentDto
import com.lingxi.code.bindings.client.DiffLineKindDto
import com.lingxi.code.bindings.client.DiffRowDto
import com.lingxi.code.bindings.client.ErrorKindDto
import com.lingxi.code.bindings.client.HeadlineKindDto
import com.lingxi.code.bindings.client.StructuredDiffDto
import com.lingxi.code.bindings.client.SyntaxClassDto
import com.lingxi.code.bindings.client.ToolHeaderDto
import com.lingxi.code.bindings.client.ToolIconDto
import com.lingxi.code.bindings.client.ToolResultDisplayDto
import com.lingxi.code.bindings.client.ToolSubLineDto
import com.lingxi.code.bindings.client.ToolVerbDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.toUi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.transformWhile
import org.json.JSONObject

/**
 * Streamed assistant-reply events — the UI-facing analog of engine
 * [ClientEvent]s. [clientEventToReply] maps the wire events onto these; the
 * [ChatViewModel] reduces them into [ChatState].
 */
sealed interface ReplyEvent {
    /** The model is thinking; starts the live run trace before text arrives. */
    data object Thinking : ReplyEvent

    /** An incremental model-reasoning delta, rendered in the live run trace. */
    data class ReasoningDelta(val text: String) : ReplyEvent

    /** An incremental assistant-text delta (the engine's streamed tokens). */
    data class Delta(val text: String) : ReplyEvent

    /** Correlated tool activity surfaced in both the status row and run trace. */
    data class ToolActivity(
        val label: String,
        val id: String? = null,
        val tool: String? = null,
        val status: AgentToolStatus? = null,
        /** Command text for a locally synthesized shell row with no engine start header. */
        val syntheticShellCommand: String? = null,
        val planMarkdown: String? = null,
        val questionAnswers: List<AnsweredQuestion>? = null,
        val localApp: PublishedLocalApp? = null,
        val elapsedMs: Long? = null,
        /**
         * The engine's PRE-DERIVED call header, carried straight through from
         * `ToolUseStarted.header`. Result and heartbeat events carry no header,
         * so the reducer keeps the one the start event already delivered.
         */
        val header: ToolHeaderUi? = null,
        /**
         * The engine's PRE-DERIVED `⎿` block from `ToolUseResult.display`. This
         * is the payload the non-shell result arm used to THROW AWAY entirely.
         */
        val display: ToolResultDisplayUi? = null,
    ) : ReplyEvent

    /** Correlated shell lifecycle update rendered as an expandable terminal card. */
    data class ShellTool(val update: ShellToolUpdate) : ReplyEvent

    /** A non-terminal engine notice. */
    data class Notice(val message: String, val isError: Boolean) : ReplyEvent

    /** A completed local slash command, which may have no TurnEnded event. */
    data class CommandOutput(
        val display: String,
        val isError: Boolean,
        val turnId: ULong?,
    ) : ReplyEvent

    /** Incremental token accounting for the latest API call. */
    data class Usage(val usage: AgentRunUsage) : ReplyEvent

    /** A provider retry/backoff that keeps the turn alive. */
    data class Retry(
        val message: String,
        val attempt: Int,
        val maxRetries: Int,
        val delayMs: Long,
    ) : ReplyEvent

    /** The engine's pre-formatted cumulative session cost. */
    data class Cost(val formatted: String) : ReplyEvent

    /** A completed context compaction. */
    data class Compaction(
        val messagesBefore: Int,
        val messagesAfter: Int,
        val bytesSaved: Long,
    ) : ReplyEvent

    /** Current coordinator/team worker activity. */
    data class Coordinator(val activeWorkers: Int, val team: String?) : ReplyEvent

    /** Internal-only: the matching raw event already reduced and can advance. */
    data class DurableTurnReplayAcknowledged(
        val turnId: Long,
        val sequence: Long,
    ) : ReplyEvent

    /** A terminal error to surface (engine `Error`, or a build/submit failure). */
    data class Error(val message: String) : ReplyEvent

    /** One assistant API-response boundary; the enclosing turn can continue. */
    data class MessageComplete(val message: Message?) : ReplyEvent
    data class MessageIdentity(val messageId: String) : ReplyEvent
    data class MessageRetracted(val messageId: String) : ReplyEvent

    /** A visualization slot in the assistant stream started, resolved or was discarded. */
    data class Visualization(
        val status: VisualizationBlockStatus,
        val reference: VisualizationRef?,
    ) : ReplyEvent

    /** The final assistant message used by sources without engine boundaries. */
    data class Completed(val message: Message) : ReplyEvent

    /** Terminal marker: the turn ended cleanly. The stream completes after this. */
    data object End : ReplyEvent
}

private fun shouldAwaitDurableTurnReplayAck(event: ClientEvent, reply: ReplyEvent): Boolean =
    when (event) {
        is ClientEvent.TurnStarted,
        is ClientEvent.TextDelta,
        is ClientEvent.VisualizationBlock,
        is ClientEvent.ThinkingDelta,
        is ClientEvent.SystemNotice,
        is ClientEvent.ToolUseStarted,
        is ClientEvent.ToolHeartbeat,
        is ClientEvent.ToolUseResult,
        is ClientEvent.UsageUpdate,
        is ClientEvent.ApiRetry,
        is ClientEvent.CostUpdate,
        is ClientEvent.CompactionCompleted,
        is ClientEvent.CoordinatorStatus,
        -> reply !is ReplyEvent.End &&
            reply !is ReplyEvent.Error &&
            reply !is ReplyEvent.Completed
        else -> false
    }

/**
 * PURE mapping from one inbound engine [ClientEvent] to a [ReplyEvent], or
 * `null` to ignore listing / configuration events that ride an out-of-band
 * state path. Live thinking, tools, retries, usage and cost remain on this path
 * so Android can render the same execution progress as the CLI/TUI.
 *
 * This is deliberately a free function with NO engine / Android dependencies so
 * it is exhaustively unit-testable on the JVM (where `buildAndroidEngine` is
 * unavailable). Keep it total over the variants we render and `null`-tolerant of
 * everything else — `ClientEvent` is `#[non_exhaustive]`, so an `else` is
 * required and must mean "ignore, don't break the stream".
 */
fun clientEventToReply(
    event: ClientEvent,
    strings: ConversationStrings = DefaultConversationStrings,
): ReplyEvent? = when (event) {
    is ClientEvent.TurnStarted -> ReplyEvent.Thinking
    is ClientEvent.TextDelta -> ReplyEvent.Delta(event.text.displayText())
    is ClientEvent.VisualizationBlock ->
        ReplyEvent.Visualization(event.status.toUi(), event.reference?.toUi())
    is ClientEvent.ThinkingDelta -> ReplyEvent.ReasoningDelta(event.thinking)
    // Reduced on the session-level raw event stream, including before TurnStarted.
    is ClientEvent.ScheduledTaskFire -> null
    is ClientEvent.SystemNotice -> ReplyEvent.Notice(event.message.displayText(), event.isError)
    is ClientEvent.SlashCommandResult -> ReplyEvent.CommandOutput(
        display = event.display.displayText(),
        isError = event.isError,
        turnId = event.turnId,
    )
    is ClientEvent.ToolUseStarted ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellStarted(event.id, event.inputJson))
        } else {
            ReplyEvent.ToolActivity(
                label = strings.resolve(R.string.chat_tool_calling_label, "调用工具 %1\$s…", event.tool),
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                planMarkdown = toolPlanMarkdown(event.tool, event.inputJson),
                header = event.header?.toUi(),
            )
        }
    is ClientEvent.ToolHeartbeat ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(
                ShellToolUpdate.Heartbeat(event.id, event.elapsedMs.toLong()),
            )
        } else {
            ReplyEvent.ToolActivity(
                label = strings.resolve(R.string.chat_tool_running_label, "工具 %1\$s 运行中…", event.tool),
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                elapsedMs = event.elapsedMs.toLong(),
            )
        }
    is ClientEvent.ToolUseResult ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellFinished(event.id, event.resultJson, event.isError))
        } else {
            // This arm used to DISCARD the whole payload — a completed tool call
            // rendered as one dim status line and nothing else. The engine's
            // pre-derived `display` (headline, structured diff, clamped body,
            // collapse verdict) now rides through to the renderer intact.
            ReplyEvent.ToolActivity(
                label = if (event.isError) {
                    strings.resolve(R.string.chat_tool_failed_label, "工具 %1\$s 失败", event.tool)
                } else {
                    strings.resolve(R.string.chat_tool_completed_label, "工具 %1\$s 完成", event.tool)
                },
                id = event.id,
                tool = event.tool,
                status = if (event.isError) AgentToolStatus.Failed else AgentToolStatus.Completed,
                display = event.display?.toUi(),
                planMarkdown = if (!event.isError) toolPlanMarkdown(event.tool, event.resultJson) else null,
                questionAnswers = if (!event.isError) parseQuestionAnswers(event.tool, event.resultJson) else null,
                localApp = if (!event.isError) parseLocalAppResult(event.tool, event.resultJson) else null,
            )
        }
    is ClientEvent.UsageUpdate -> ReplyEvent.Usage(
        AgentRunUsage(
            inputTokens = event.inputTokens.toLong(),
            outputTokens = event.outputTokens.toLong(),
            cacheReadTokens = event.cacheReadTokens.toLong(),
            cacheCreationTokens = event.cacheCreationTokens.toLong(),
        ),
    )
    is ClientEvent.ApiRetry -> ReplyEvent.Retry(
        message = event.message,
        attempt = event.attempt.toInt(),
        maxRetries = event.maxRetries.toInt(),
        delayMs = event.delayMs.toLong(),
    )
    is ClientEvent.CostUpdate -> ReplyEvent.Cost(event.formatted)
    is ClientEvent.CompactionCompleted -> ReplyEvent.Compaction(
        messagesBefore = event.messagesBefore.toInt(),
        messagesAfter = event.messagesAfter.toInt(),
        bytesSaved = event.bytesSaved.toLong(),
    )
    is ClientEvent.CoordinatorStatus ->
        ReplyEvent.Coordinator(event.activeWorkers.toInt(), event.team)
    is ClientEvent.MessageComplete ->
        ReplyEvent.MessageComplete(event.message?.let { messageDtoToMessage(it, strings) })
    is ClientEvent.MessageIdentity -> ReplyEvent.MessageIdentity(event.messageId)
    is ClientEvent.MessageRetracted -> ReplyEvent.MessageRetracted(event.messageId)
    is ClientEvent.TurnEnded -> ReplyEvent.End
    is ClientEvent.Error -> if (isCompactTurnRejection(event.message.displayText())) null else ReplyEvent.Error(
        userFacingEngineError(event.kind, event.message, strings),
    )
    else -> null // model / session / permission / listings ride out-of-band flows
}

/**
 * Lower one retained durable-turn event into the same reducer input used by the
 * live stream. The durable envelope intentionally carries JSON (rather than a
 * recursive `ClientEvent`) so this decoder stays small and forward-compatible:
 * unknown event types are ignored, while narrative/tool/terminal events needed
 * to reconstruct the visible in-flight response are restored.
 */
internal fun retainedTurnEventToReply(
    eventJson: String,
    strings: ConversationStrings = DefaultConversationStrings,
): ReplyEvent? = runCatching {
    val event = JSONObject(eventJson)
    when (event.optString("type")) {
        "turn_started" -> ReplyEvent.Thinking
        "text_delta" -> ReplyEvent.Delta(event.optString("text"))
        "visualization_block" -> visualizationBlockStatus(event.optString("status"))?.let { status ->
            val reference = event.optJSONObject("reference")?.let { ref ->
                val id = ref.optString("id")
                val revision = ref.optLong("revision")
                if (id.isEmpty() || revision !in 1L..UInt.MAX_VALUE.toLong()) null
                else VisualizationRef(id, revision.toUInt())
            }
            ReplyEvent.Visualization(status, reference)
        }
        "thinking_delta" -> ReplyEvent.ReasoningDelta(event.optString("thinking"))
        "system_notice" -> ReplyEvent.Notice(
            message = event.optString("message"),
            isError = event.optBoolean("is_error"),
        )
        "tool_use_started" -> clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = event.optString("id"),
                tool = event.optString("tool"),
                inputJson = event.optString("input_json", "{}"),
                header = retainedToolHeader(event.getJSONObject("header")) ?: return@runCatching null,
            ),
            strings,
        )
        "tool_heartbeat" -> clientEventToReply(
            ClientEvent.ToolHeartbeat(
                id = event.optString("id"),
                tool = event.optString("tool"),
                elapsedMs = event.optLong("elapsed_ms").coerceAtLeast(0L).toULong(),
            ),
            strings,
        )
        "tool_use_result" -> clientEventToReply(
            ClientEvent.ToolUseResult(
                id = event.optString("id"),
                tool = event.optString("tool"),
                resultJson = event.optString("result_json", "{}"),
                isError = event.optBoolean("is_error"),
                display = retainedToolResultDisplay(event.getJSONObject("display")) ?: return@runCatching null,
            ),
            strings,
        )
        "error" -> event.optString("message").takeUnless(::isCompactTurnRejection)?.let { ReplyEvent.Error(it) }
        // `outcome` is an internally TAGGED OBJECT on the wire, not a string:
        // client-protocol's `TurnOutcomeDto` carries `#[serde(tag = "type")]`,
        // and the blessed fixture
        // `Harness crates/client/snapshots/event/turn_ended.json` pins
        // `"outcome": {"type": "end_turn"}`. Reading it with
        // `optString("outcome")` can never yield "end_turn" under ANY org.json
        // build — AOSP hands back the fallback, the reference implementation
        // hands back the object's own `{"type":"end_turn"}` text — so the
        // `when` always fell to `else -> null` and a REPLAYED turn_ended never
        // produced `ReplyEvent.End`: `streaming` stayed true and the composer
        // stayed locked on every recovered turn. Read the nested tag instead.
        "turn_ended" -> when (retainedTurnOutcome(event)) {
            "end_turn" -> ReplyEvent.End
            else -> null
        }
        else -> null
    }
}.getOrNull()

private fun retainedToolHeader(value: JSONObject): ToolHeaderDto? {
    val verb = value.enumValue<ToolVerbDto>("verb") ?: return null
    val label = value.requiredString("label") ?: return null
    val title = value.requiredString("title") ?: return null
    val icon = value.optionalEnumValue<ToolIconDto>("icon")
    if (value.has("icon") && !value.isNull("icon") && icon == null) return null
    val count = value.optionalUInt("count")
    if (value.has("count") && !value.isNull("count") && count == null) return null
    val subLine = value.optJSONObject("sub_line")?.let { line ->
        val prefix = line.requiredString("prefix") ?: return null
        val text = line.requiredString("text") ?: return null
        ToolSubLineDto(prefix = prefix, text = text)
    }
    if (value.has("sub_line") && !value.isNull("sub_line") && value.optJSONObject("sub_line") == null) return null
    val primary = value.optionalString("primary")
    if (value.has("primary") && !value.isNull("primary") && primary == null) return null
    val qualifier = value.optionalString("qualifier")
    if (value.has("qualifier") && !value.isNull("qualifier") && qualifier == null) return null
    return ToolHeaderDto(
        verb = verb,
        icon = icon,
        label = label,
        primary = primary,
        qualifier = qualifier,
        count = count,
        subLine = subLine,
        title = title,
    )
}

private fun retainedToolResultDisplay(value: JSONObject): ToolResultDisplayDto? {
    val bodyLines = value.requiredUInt("body_lines") ?: return null
    val headline = value.optionalString("headline")
    if (value.has("headline") && !value.isNull("headline") && headline == null) return null
    val headlineKind = value.optionalEnumValue<HeadlineKindDto>("headline_kind")
    if (value.has("headline_kind") && !value.isNull("headline_kind") && headlineKind == null) return null
    val headlineArgs = if (!value.has("headline_args")) {
        emptyList()
    } else {
        val array = value.optJSONArray("headline_args") ?: return null
        buildList {
            for (index in 0 until array.length()) add(array.optUInt(index) ?: return null)
        }
    }
    val diff = value.optJSONObject("diff")?.let(::retainedStructuredDiff)
    if (value.has("diff") && !value.isNull("diff") && diff == null) return null
    val body = value.optionalString("body")
    if (value.has("body") && !value.isNull("body") && body == null) return null
    val bodyTruncated = value.booleanOrDefault("body_truncated") ?: return null
    val collapsed = value.booleanOrDefault("collapsed") ?: return null
    return ToolResultDisplayDto(
        headline = headline,
        headlineKind = headlineKind,
        headlineArgs = headlineArgs,
        diff = diff,
        body = body,
        bodyLines = bodyLines,
        bodyTruncated = bodyTruncated,
        collapsed = collapsed,
    )
}

private fun retainedStructuredDiff(value: JSONObject): StructuredDiffDto? {
    val gutterWidth = value.requiredUInt("gutter_width") ?: return null
    val additions = value.requiredUInt("additions") ?: return null
    val removals = value.requiredUInt("removals") ?: return null
    val truncatedRows = value.requiredUInt("truncated_rows") ?: return null
    val rows = value.optJSONArray("rows") ?: return null
    val parsedRows = buildList {
        for (rowIndex in 0 until rows.length()) {
            val row = rows.optJSONObject(rowIndex) ?: return null
            val kind = row.enumValue<DiffLineKindDto>("kind") ?: return null
            val lineNo = row.requiredUInt("line_no") ?: return null
            val hunk = row.requiredUInt("hunk") ?: return null
            val wordDiffed = row.booleanOrDefault("word_diffed") ?: return null
            val segments = row.optJSONArray("segments") ?: return null
            val parsedSegments = buildList {
                for (segmentIndex in 0 until segments.length()) {
                    val segment = segments.optJSONObject(segmentIndex) ?: return null
                    val text = segment.requiredString("text") ?: return null
                    val syntax = segment.enumValue<SyntaxClassDto>("class") ?: return null
                    val rgb = segment.optionalUInt("rgb")
                    if (segment.has("rgb") && !segment.isNull("rgb") && rgb == null) return null
                    val bold = segment.booleanOrDefault("bold") ?: return null
                    val italic = segment.booleanOrDefault("italic") ?: return null
                    val underline = segment.booleanOrDefault("underline") ?: return null
                    val emph = segment.booleanOrDefault("emph") ?: return null
                    add(CodeSegmentDto(text, syntax, rgb, bold, italic, underline, emph))
                }
            }
            add(DiffRowDto(kind, lineNo, hunk, wordDiffed, parsedSegments))
        }
    }
    return StructuredDiffDto(
        filePath = value.optionalString("file_path").also {
            if (value.has("file_path") && !value.isNull("file_path") && it == null) return null
        },
        language = value.optionalString("language").also {
            if (value.has("language") && !value.isNull("language") && it == null) return null
        },
        gutterWidth = gutterWidth,
        additions = additions,
        removals = removals,
        truncatedRows = truncatedRows,
        rows = parsedRows,
    )
}

private inline fun <reified T : Enum<T>> JSONObject.enumValue(key: String): T? {
    val value = requiredString(key) ?: return null
    return enumValues<T>().firstOrNull { it.name.equals(value, ignoreCase = true) || it.name.lowercase() == value }
}

private inline fun <reified T : Enum<T>> JSONObject.optionalEnumValue(key: String): T? =
    if (!has(key) || isNull(key)) null else enumValue(key)

private fun JSONObject.optionalString(key: String): String? =
    if (!has(key) || isNull(key)) null else optString(key).takeIf { opt(key) is String }

private fun JSONObject.requiredString(key: String): String? =
    if (!has(key) || isNull(key)) null else opt(key) as? String

private fun JSONObject.requiredUInt(key: String): UInt? =
    if (!has(key) || isNull(key)) null else optUInt(key)

private fun JSONObject.optionalUInt(key: String): UInt? =
    if (!has(key) || isNull(key)) null else optUInt(key)

private fun JSONObject.optUInt(key: String): UInt? = optUInt(opt(key))

private fun org.json.JSONArray.optUInt(index: Int): UInt? = optUInt(opt(index))

private fun optUInt(value: Any?): UInt? {
    val number = value as? Number ?: return null
    val double = number.toDouble()
    if (!double.isFinite() || double < 0.0 || double > UInt.MAX_VALUE.toDouble() || double % 1.0 != 0.0) return null
    return number.toLong().toUInt()
}

private fun JSONObject.booleanOrDefault(key: String): Boolean? =
    if (!has(key)) false else opt(key) as? Boolean

/**
 * The `type` tag of a retained `turn_ended` envelope's `outcome`.
 *
 * The object form is the only shape the engine emits today; a bare string is
 * still accepted so a journal retained by an older build stays readable.
 */
private fun retainedTurnOutcome(event: JSONObject): String? {
    event.optJSONObject("outcome")?.let { outcome ->
        return outcome.optString("type").takeUnless(String::isEmpty)
    }
    return (event.opt("outcome") as? String)?.takeUnless(String::isEmpty)
}

/**
 * Convert transport diagnostics into concise, actionable mobile copy.
 *
 * The Android HTTP backend opts into reqwest's nested cause chain, so DNS,
 * timeout, and TLS failures are identifiable here. Provider responses such as
 * 401/404 are not rewritten: their original message still reaches the existing
 * auth/model error handling.
 */
internal fun userFacingEngineError(
    kind: ErrorKindDto,
    message: ClientTextDto,
    strings: ConversationStrings = DefaultConversationStrings,
): String {
    val displayMessage = message.displayText()
    if (kind != ErrorKindDto.TRANSPORT) return displayMessage
    val normalized = displayMessage.lowercase()
    return when {
        listOf(
            "dns",
            "unknown host",
            "no such host",
            "failed to lookup",
            "name or service not known",
            "nodename nor servname",
        ).any(normalized::contains) ->
            strings.resolve(R.string.chat_error_dns, "无法解析模型服务地址。请检查 VPN、私人 DNS 或当前网络后重试。")

        listOf("certificate", "tls", "ssl").any(normalized::contains) ->
            strings.resolve(R.string.chat_error_tls, "模型服务安全连接失败。请检查系统时间、VPN 或证书设置后重试。")

        listOf("timeout", "timed out").any(normalized::contains) ->
            strings.resolve(R.string.chat_error_connect_timeout, "连接模型服务超时。请检查当前网络或 VPN 后重试。")

        listOf(
            "connection failed",
            "connect error",
            "error sending request",
        ).any(normalized::contains) ->
            strings.resolve(R.string.chat_error_connect_failed, "无法连接模型服务。请检查当前网络或 VPN 后重试。")

        else -> displayMessage
    }
}

/**
 * PURE flow transform: turn an inbound [ClientEvent] stream into the UI-facing
 * [ReplyEvent] stream the [ChatViewModel] reduces. Prepends a leading
 * [ReplyEvent.Thinking] (so the run trace shows the instant a turn is armed,
 * before the first engine event), maps each event through [clientEventToReply]
 * (dropping ignored ones), and COMPLETES after the first terminal reply
 * ([ReplyEvent.End] / [ReplyEvent.Error] / [ReplyEvent.Completed], plus
 * [ReplyEvent.CommandOutput] when opted in) — emitting a trailing
 * [ReplyEvent.End] after an `Error` so the ViewModel always sees a clean turn
 * boundary. A local slash command result is terminal itself because it can
 * finish without `TurnEnded`.
 *
 * @param acceptSlashCommandResults enable only for a submission path that
 * issued `RunSlashCommand`; ordinary prompt streams ignore command results.
 *
 * Extracted from [EngineConversationSource.submit] as a free function with NO
 * engine / Android dependency so the streaming/ordering contract is exercised on
 * the plain JVM (see `EngineReplyStreamTest`) — including the subscribe-before-
 * submit guarantee, which a flow-level test can prove without a native engine.
 */
fun mapReplyStream(
    events: Flow<ClientEvent>,
    strings: ConversationStrings = DefaultConversationStrings,
    acceptSlashCommandResults: Boolean = false,
): Flow<ReplyEvent> = flow {
    emit(ReplyEvent.Thinking)
    var awaitingDurableReplayAck = false
    emitAll(
        events.transformWhile { event ->
            if (event is ClientEvent.TurnEventReplay) {
                if (awaitingDurableReplayAck) {
                    emit(
                        ReplyEvent.DurableTurnReplayAcknowledged(
                            turnId = event.turnId.toLong(),
                            sequence = event.sequence.toLong(),
                        ),
                    )
                    awaitingDurableReplayAck = false
                }
                return@transformWhile true
            }
            if (event is ClientEvent.SlashCommandResult && !acceptSlashCommandResults) {
                return@transformWhile true
            }
            val reply = clientEventToReply(event, strings) ?: return@transformWhile true
            emit(reply)
            awaitingDurableReplayAck = shouldAwaitDurableTurnReplayAck(event, reply)
            val terminal = reply is ReplyEvent.End ||
                reply is ReplyEvent.Error ||
                reply is ReplyEvent.Completed ||
                reply is ReplyEvent.CommandOutput
            if (reply is ReplyEvent.Error) emit(ReplyEvent.End)
            !terminal // keep collecting until a terminal reply
        },
    )
}
