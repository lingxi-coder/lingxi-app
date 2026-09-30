package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.MessageImageDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.toUi

/**
 * Lower one wire [MessageDto] to the UI [Message] model.
 *
 * The assistant bubble is no longer text-only: alongside the flattened prose in
 * [Message.text] the message now carries ORDERED [MessageContent] blocks, so a
 * tool call renders as its derived header + `⎿` result instead of collapsing to
 * the one-line `"调用工具 X…"` placeholder that discarded every diff and body.
 *
 * `ToolUse` and `ToolResult` are paired by id WITHIN this one message here; the
 * engine actually splits them across two messages (call in assistant N, result
 * in user N+1), which is why a whole transcript must go through
 * [transcriptFromDtos] instead of mapping each DTO independently.
 *
 * The wire `role` ("user" / "assistant" / "system") maps to [Role]: "user" →
 * [Role.User]; everything else → [Role.Ai]. PURE — no engine dependency.
 */
fun messageDtoToMessage(
    dto: MessageDto,
    strings: ConversationStrings = DefaultConversationStrings,
): Message {
    val build = MessageBuild(dto.role, messageImages(dto.images), dto.loopWakeup)
    val index = mutableMapOf<String, ToolBlockRef>()
    dto.blocks.forEach { block ->
        // One DTO in isolation has no preceding turn to hand an orphan result
        // to, and a user build can never render one (see [orphanHostFor]), so
        // an assistant DTO keeps its own orphans and a user DTO drops them.
        build.fold(block, strings, index) { build.takeIf { it.role == Role.Ai } }
    }
    return build.toMessage()
}

/**
 * Lower a WHOLE transcript, threading the tool-use index across messages.
 *
 * A `ToolUse` sits in assistant message N and its `ToolResult` in user message
 * N+1 — never in the same message (see `client-adapter`'s `ToolUseIndex`, which
 * keeps the same side-table for the same reason). Mapping each [MessageDto]
 * independently therefore renders every restored tool call as a header with no
 * result, and leaves a content-free user bubble holding the orphaned results.
 *
 * So: results are folded BACK into the message that made the call, and any
 * message left with neither prose nor a tool block is dropped rather than
 * rendered as an empty bubble. PURE — exercised on the JVM.
 */
fun transcriptFromDtos(
    dtos: List<MessageDto>,
    strings: ConversationStrings = DefaultConversationStrings,
): List<Message> {
    val builds = mutableListOf<MessageBuild>()
    val index = mutableMapOf<String, ToolBlockRef>()
    dtos.forEach { dto ->
        val build = MessageBuild(dto.role, messageImages(dto.images), dto.loopWakeup)
        builds.add(build)
        dto.blocks.forEach { block ->
            build.fold(block, strings, index) { orphanHostFor(builds, build) }
        }
    }
    val messages = mutableListOf<Message>()
    builds.filter { it.isRenderable() }.forEach { build ->
        var message = build.toMessage()
        build.loopWakeup?.let { wakeup ->
            val start = messages.indexOfLast { it.loopWakeupStreak != null }
            if (wakeup.streak > 0u && start >= 0) {
                message = message.copy(loopFoldedItemIds = messages.drop(start).map { it.id }.toSet())
            }
        }
        messages.add(message)
        build.loopWakeup?.companion?.let { messages.add(Message(role = Role.Ai, text = it)) }
    }
    return messages
}

/**
 * Which build an ORPHAN `ToolResult` — one whose `ToolUse` fell outside a torn
 * or compacted transcript window — is folded into.
 *
 * NEVER the user build it arrived in. [MessageBubble] renders a user turn from
 * [Message.text] alone and ignores its blocks, so a tool block parked there is
 * invisible AND makes [MessageBuild.isRenderable] answer `true`, producing the
 * empty bordered bubble that predicate exists to prevent. The old
 * `previous ?: this` fallback did exactly that whenever the orphan landed in
 * the FIRST message of the window (no `previous` at all) or right after another
 * user message.
 *
 * The host is the IMMEDIATELY preceding build when that is an assistant turn —
 * the one that made the call in every window torn only at the block level.
 * Otherwise the calling turn itself fell outside the window, and a build is
 * MINTED and spliced in right where it used to be, so the row renders in its
 * own position instead of being retro-fitted into an unrelated older bubble. A
 * minted build that never receives a block stays empty and is dropped by the
 * `isRenderable` filter, so this can never introduce a blank bubble of its own;
 * a second orphan in the same message finds the mint and reuses it.
 */
private fun orphanHostFor(builds: MutableList<MessageBuild>, current: MessageBuild): MessageBuild {
    if (current.role == Role.Ai) return current
    val at = builds.indexOf(current).coerceAtLeast(0)
    builds.getOrNull(at - 1)?.takeIf { it.role == Role.Ai }?.let { return it }
    val minted = MessageBuild(ASSISTANT_WIRE_ROLE)
    builds.add(at, minted)
    return minted
}

/**
 * The engine persists image bytes as a data URL in the resumed message DTO.
 * Keep the UI model's existing ImageRefDto shape so live and restored user
 * turns use the same renderer and resend path.
 */
private fun messageImages(images: List<MessageImageDto>): List<ImageRefDto> =
    images.mapNotNull { image ->
        val marker = ";base64,"
        val markerIndex = image.url.indexOf(marker)
        if (!image.url.startsWith("data:") || markerIndex <= "data:".length) return@mapNotNull null
        val mediaType = image.mediaType.ifBlank {
            image.url.substring("data:".length, markerIndex)
        }
        val base64 = image.url.substring(markerIndex + marker.length)
        if (mediaType.isBlank() || base64.isBlank()) null else ImageRefDto(mediaType, base64)
    }

/** The wire `role` a minted assistant build carries. */
private const val ASSISTANT_WIRE_ROLE = "assistant"

/** Where a recorded `ToolUse` block lives, so its later `ToolResult` can reach it. */
private class ToolBlockRef(val build: MessageBuild, val blockIndex: Int)

/** Mutable accumulator for one message's prose + ordered content blocks. */
private class MessageBuild(
    wireRole: String,
    val images: List<ImageRefDto> = emptyList(),
    val loopWakeup: com.lingxi.code.bindings.client.LoopWakeupDto? = null,
) {
    val role: Role = if (wireRole.equals("user", ignoreCase = true)) Role.User else Role.Ai
    val textParts = mutableListOf<String>().apply { loopWakeup?.let { add(it.message) } }
    val blocks = mutableListOf<MessageContent>()

    /**
     * True when the message has anything to show — i.e. exactly what
     * [MessageBubble] would actually draw for this role. A user turn carrying
     * ONLY the previous assistant turn's tool results has neither prose nor a
     * tool block of its own (they were folded back), and must not render as an
     * empty bubble.
     *
     * The role check is not redundant with [orphanHostFor]: the user branch of
     * the bubble renders [Message.text] and nothing else, so "has a tool block"
     * can only mean "renderable" for an assistant turn. Answering `true` for a
     * text-less user build is what produced the empty bordered bubble.
     */
    fun isRenderable(): Boolean =
        textParts.any { it.isNotBlank() } ||
            (role == Role.User && images.isNotEmpty()) ||
            (role == Role.Ai && blocks.any { it is MessageContent.Tool })

    fun toMessage(): Message = Message(
        role = role,
        text = textParts.filter { it.isNotBlank() }.joinToString("\n\n"),
        images = images,
        blocks = blocks.toList(),
        loopWakeupStreak = loopWakeup?.streak,
    )

    private fun addText(text: String) {
        textParts.add(text)
        if (text.isNotBlank()) blocks.add(MessageContent.Text(text))
    }

    /**
     * Fold one wire block in. The `when` is exhaustive over the generated
     * [MessageBlockDto] subclasses — a regen that adds a block kind is a compile
     * error here, mirroring the engine's exhaustive `ContentBlock` match.
     */
    fun fold(
        block: MessageBlockDto,
        strings: ConversationStrings,
        index: MutableMap<String, ToolBlockRef>,
        /**
         * Resolved LAZILY, and only for an orphan result, because resolving it
         * can MINT a build ([orphanHostFor]) — doing that eagerly per message
         * would splice an empty build ahead of every user turn.
         */
        orphanHost: () -> MessageBuild?,
    ) {
        when (block) {
            is MessageBlockDto.Text -> addText(block.text)
            // Only live reasoning has a Thinking row. Ignoring these blocks
            // also keeps tool groups contiguous across historical reasoning.
            is MessageBlockDto.Thinking, is MessageBlockDto.RedactedThinking -> Unit
            is MessageBlockDto.CompactBoundary ->
                addText(strings.resolve(R.string.chat_compacted_label, "对话已压缩"))
            is MessageBlockDto.ToolUse -> {
                index[block.id] = ToolBlockRef(this, blocks.size)
                blocks.add(
                    MessageContent.Tool(
                        ToolCallUi(
                            id = block.id,
                            tool = block.tool,
                            planMarkdown = toolPlanMarkdown(block.tool, block.inputJson),
                            header = block.header?.toUi(),
                            status = AgentToolStatus.Running,
                            // Older engine (no header): the legacy scrape is the floor.
                            fallbackSummary = if (block.header == null) {
                                summarizeToolInput(block.inputJson)
                            } else {
                                null
                            },
                        ),
                    ),
                )
            }
            is MessageBlockDto.ToolResult -> {
                val status =
                    if (block.isError) AgentToolStatus.Failed else AgentToolStatus.Completed
                val display = block.display?.toUi()
                val ref = index.remove(block.id)
                if (ref != null) {
                    val existing = ref.build.blocks[ref.blockIndex] as MessageContent.Tool
                    ref.build.blocks[ref.blockIndex] = MessageContent.Tool(
                        existing.call.copy(display = display, status = status,
                            planMarkdown = existing.call.planMarkdown ?: toolPlanMarkdown(block.tool, block.resultJson)),
                    )
                } else {
                    // ORPHAN result — a torn or compacted transcript window. It
                    // belongs to an ASSISTANT turn, never to the user bubble it
                    // arrived in (which renders text only).
                    orphanHost()?.blocks?.add(
                        MessageContent.Tool(
                            ToolCallUi(
                                id = block.id,
                                tool = block.tool,
                                planMarkdown = toolPlanMarkdown(block.tool, block.resultJson),
                                display = display,
                                status = status,
                            ),
                        ),
                    )
                }
            }
        }
    }
}

/**
 * LEGACY: flatten a message's ordered content [MessageBlockDto]s into ONE body
 * string, collapsing every tool call to a `"调用工具 X…"` placeholder.
 *
 * The bubble no longer renders this — [messageDtoToMessage] and
 * [transcriptFromDtos] now emit ordered [MessageContent] blocks so a tool call
 * keeps its derived header and `⎿` result instead of being erased into one line.
 * This remains as the plain-text projection of a message (and as the shape a
 * pre-structure client produced), so it must keep folding EVERY block kind:
 *  - Text             → the text verbatim.
 *  - Thinking         → the reasoning text (the bubble has no separate thinking
 *                       region for restored scrollback; it reads inline).
 *  - RedactedThinking → a placeholder marker (the payload is opaque).
 *  - CompactBoundary  → a visible boundary marker; its hidden summary is not
 *                       rendered as user-authored text.
 *  - ToolUse          → a compact "调用工具 <tool>" activity line.
 *  - ToolResult       → a compact "工具结果"/"工具失败" line.
 * Blocks are joined by blank lines and blanks are dropped so an empty trailing
 * block never leaves dangling whitespace. The `when` is exhaustive over the
 * generated [MessageBlockDto] subclasses — a regen that adds a new block kind is
 * a compile error here, mirroring the engine's exhaustive `ContentBlock` match.
 */
fun messageDtoText(
    blocks: List<MessageBlockDto>,
    strings: ConversationStrings = DefaultConversationStrings,
): String =
    blocks.mapNotNull { block ->
        val line: String = when (block) {
            is MessageBlockDto.Text -> block.text
            is MessageBlockDto.Thinking -> block.thinking
            is MessageBlockDto.RedactedThinking -> strings.resolve(R.string.chat_redacted_thinking, "[已折叠的思考]")
            is MessageBlockDto.CompactBoundary -> strings.resolve(R.string.chat_compacted_label, "对话已压缩")
            is MessageBlockDto.ToolUse ->
                strings.resolve(R.string.chat_tool_calling_label, "调用工具 %1\$s…", block.tool)
            is MessageBlockDto.ToolResult ->
                if (block.isError) {
                    strings.resolve(R.string.chat_tool_result_failed, "工具失败")
                } else {
                    strings.resolve(R.string.chat_tool_result_label, "工具结果")
                }
        }
        line.takeUnless { it.isBlank() }
    }.joinToString("\n\n")
