package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.CodeSegmentDto
import com.lingxi.code.bindings.CostDto
import com.lingxi.code.bindings.DiffLineKindDto
import com.lingxi.code.bindings.DiffRowDto
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.HeadlineKindDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
import com.lingxi.code.bindings.PlanTaskDto
import com.lingxi.code.bindings.PlanTaskStateDto
import com.lingxi.code.bindings.SessionModeDto
import com.lingxi.code.bindings.StructuredDiffDto
import com.lingxi.code.bindings.SyntaxClassDto
import com.lingxi.code.bindings.ToolHeaderDto
import com.lingxi.code.bindings.ToolResultDisplayDto
import com.lingxi.code.bindings.ToolVerbDto
import com.lingxi.code.bindings.TurnOutcomeDto
import com.lingxi.code.model.Role
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Exhaustive coverage of the PURE [clientEventToReply] mapper — the single seam
 * that turns inbound engine [ClientEvent]s into UI-facing [ReplyEvent]s. This is
 * the unit-testable core of the engine conversation path: it has no engine /
 * Android dependency, so it runs on the plain JVM where `buildAndroidEngine` is
 * unavailable (we deliberately never build the engine here).
 *
 * The data-class fixtures below only CONSTRUCT generated UniFFI types — they
 * never call an exported function — so no native `.so` is loaded.
 */
class ClientEventMapperTest {

    private val cost = CostDto(
        totalUsd = 0.0,
        inputTokens = 0u,
        outputTokens = 0u,
        apiCalls = 0u,
        sessionDurationSecs = 0u,
        formatted = "$0.00",
    )

    // --- text / thinking --------------------------------------------------

    @Test fun restoredLoopWakeupsKeepMetadataAndFoldQuietMessages() {
        val messages = transcriptFromDtos(listOf(
            MessageDto(role = "system", blocks = emptyList(), loopWakeup = com.lingxi.code.bindings.LoopWakeupDto("first", null, 0u, 0uL)),
            MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("quiet"))),
            MessageDto(role = "system", blocks = emptyList(), loopWakeup = com.lingxi.code.bindings.LoopWakeupDto("second", "healthy", 1u, 1uL)),
        ))
        assertEquals(listOf("first", "quiet", "second", "healthy"), messages.map { it.text })
        assertEquals(messages.take(2).map { it.id }.toSet(), messages[2].loopFoldedItemIds)
        assertEquals(1u, messages[2].loopWakeupStreak)
    }

    @Test
    fun textDelta_mapsToDelta_preservingText() {
        val r = clientEventToReply(ClientEvent.TextDelta("hello"))
        assertEquals(ReplyEvent.Delta("hello"), r)
    }

    @Test
    fun textDelta_emptyString_stillDelta() {
        assertEquals(ReplyEvent.Delta(""), clientEventToReply(ClientEvent.TextDelta("")))
    }

    @Test
    fun thinkingDelta_preservesReasoningText() {
        val r = clientEventToReply(ClientEvent.ThinkingDelta(thinking = "reasoning…", signature = null))
        assertEquals(ReplyEvent.ReasoningDelta("reasoning…"), r)
    }

    @Test
    fun turnStarted_mapsToThinking() {
        assertEquals(ReplyEvent.Thinking, clientEventToReply(ClientEvent.TurnStarted(turnId = null)))
    }

    // --- tool activity ----------------------------------------------------

    @Test
    fun shellToolUseStarted_mapsToStructuredCardUpdate() {
        val r = clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = "t1",
                tool = "bash",
                inputJson = """{"command":"echo ok","cwd":"/workspace"}""",
                header = null,
            ),
        )
        assertTrue(r is ReplyEvent.ShellTool)
        val started = (r as ReplyEvent.ShellTool).update as ShellToolUpdate.Started
        assertEquals("t1", started.taskId)
        assertEquals("echo ok", started.command)
        assertEquals("/workspace", started.cwd)
    }

    @Test
    fun toolUseResult_success_mapsToToolActivity_notError() {
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(
                id = "t1", tool = "read", resultJson = "{}", isError = false, display = null,
            ),
        )
        assertTrue(r is ReplyEvent.ToolActivity)
        val activity = r as ReplyEvent.ToolActivity
        assertTrue(activity.label.contains("read"))
        assertTrue(activity.label.contains("完成"))
        assertEquals("t1", activity.id)
        assertEquals("read", activity.tool)
        assertEquals(AgentToolStatus.Completed, activity.status)
    }

    @Test
    fun toolUseResult_error_mapsToToolActivity_failureLabel() {
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(
                id = "t1", tool = "write", resultJson = "{}", isError = true, display = null,
            ),
        )
        assertTrue(r is ReplyEvent.ToolActivity)
        val activity = r as ReplyEvent.ToolActivity
        assertTrue(activity.label.contains("write"))
        assertTrue(activity.label.contains("失败"))
        assertEquals(AgentToolStatus.Failed, activity.status)
    }

    @Test
    fun genericToolStart_preservesSafeInputSummary() {
        val r = clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = "read-1",
                tool = "Read",
                inputJson = """{"path":"/workspace/index.html","api_key":"secret"}""",
                header = null, // older engine: the legacy scrape is the only source
            ),
        ) as ReplyEvent.ToolActivity

        assertEquals("read-1", r.id)
        assertEquals("Read", r.tool)
        assertEquals(AgentToolStatus.Running, r.status)
        assertEquals("/workspace/index.html", r.inputSummary)
    }

    // --- terminal events --------------------------------------------------

    @Test
    fun turnEnded_mapsToEnd() {
        val r = clientEventToReply(
            ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost),
        )
        assertEquals(ReplyEvent.End, r)
    }

    @Test
    fun turnEnded_maxTurns_stillEnd() {
        val r = clientEventToReply(
            ClientEvent.TurnEnded(outcome = TurnOutcomeDto.MAX_TURNS, stopReason = null, cost = cost),
        )
        assertEquals(ReplyEvent.End, r)
    }

    @Test
    fun error_mapsToError_preservingMessage() {
        val r = clientEventToReply(
            ClientEvent.Error(kind = ErrorKindDto.TRANSPORT, message = "boom"),
        )
        assertEquals(ReplyEvent.Error("boom"), r)
    }

    @Test
    fun error_internalKind_stillMapsMessage() {
        val r = clientEventToReply(
            ClientEvent.Error(kind = ErrorKindDto.INTERNAL, message = "internal failure"),
        )
        assertEquals(ReplyEvent.Error("internal failure"), r)
    }

    @Test
    fun transportDnsError_mapsToActionableNetworkMessage() {
        val r = clientEventToReply(
            ClientEvent.Error(
                kind = ErrorKindDto.TRANSPORT,
                message = "connection failed: error sending request for url: " +
                    "client error (Connect): dns error: failed to lookup address information",
            ),
        )

        assertEquals(
            ReplyEvent.Error("无法解析模型服务地址。请检查 VPN、私人 DNS 或当前网络后重试。"),
            r,
        )
    }

    @Test
    fun transportTlsAndTimeoutErrors_haveSpecificGuidance() {
        assertEquals(
            "模型服务安全连接失败。请检查系统时间、VPN 或证书设置后重试。",
            userFacingEngineError(
                ErrorKindDto.TRANSPORT,
                "connection failed: TLS certificate verify failed",
            ),
        )
        assertEquals(
            "连接模型服务超时。请检查当前网络或 VPN 后重试。",
            userFacingEngineError(
                ErrorKindDto.TRANSPORT,
                "connection failed: request timed out",
            ),
        )
    }

    // --- completion / telemetry ------------------------------------------

    @Test
    fun assistantIdentityAndRetraction_preserveEngineId() {
        assertEquals(ReplyEvent.MessageIdentity("msg:failed"), clientEventToReply(ClientEvent.MessageIdentity("msg:failed")))
        assertEquals(ReplyEvent.MessageRetracted("msg:failed"), clientEventToReply(ClientEvent.MessageRetracted("msg:failed")))
    }

    @Test
    fun messageComplete_withMessage_mapsToNonTerminalBoundary() {
        val r = clientEventToReply(
            ClientEvent.MessageComplete(
                stopReason = "end_turn",
                message = MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("done"))),
            ),
        )
        assertTrue(r is ReplyEvent.MessageComplete)
        val completed = r as ReplyEvent.MessageComplete
        assertEquals(Role.Ai, completed.message?.role)
        assertEquals("done", completed.message?.text)
    }

    @Test
    fun messageComplete_withoutMessage_staysNonTerminal() {
        val r = clientEventToReply(ClientEvent.MessageComplete(stopReason = "end_turn", message = null))
        assertEquals(ReplyEvent.MessageComplete(null), r)
    }

    @Test
    fun usageUpdate_mapsToLiveUsage() {
        val r = clientEventToReply(
            ClientEvent.UsageUpdate(
                inputTokens = 1u, outputTokens = 2u, cacheReadTokens = 0u, cacheCreationTokens = 0u,
            ),
        )
        assertEquals(
            ReplyEvent.Usage(AgentRunUsage(1, 2, 0, 0)),
            r,
        )
    }

    @Test fun fixedScheduleNoticeDoesNotDuplicateThroughTheActiveReplyStream() {
        assertNull(clientEventToReply(ClientEvent.ScheduledTaskFire("Fixed task is ready")))
    }

    @Test
    fun retryAndSystemNotice_mapToLiveTraceEvents() {
        assertEquals(
            ReplyEvent.Retry("rate limited", 2, 5, 1_500),
            clientEventToReply(
                ClientEvent.ApiRetry(
                    message = "rate limited",
                    attempt = 2u,
                    maxRetries = 5u,
                    delayMs = 1_500u,
                ),
            ),
        )
        assertEquals(
            ReplyEvent.Notice("上下文即将压缩", false),
            clientEventToReply(
                ClientEvent.SystemNotice(message = "上下文即将压缩", isError = false),
            ),
        )
    }

    @Test
    fun modelChanged_isIgnored() {
        assertNull(clientEventToReply(ClientEvent.ModelChanged(model = "opus")))
    }

    @Test
    fun sessionResumed_isIgnoredByPerTurnMapper_ridesOutOfBandPath() {
        // SessionResumed carries the restored transcript on the separate
        // out-of-band path (sessionActivationFrom → activeSessionState), NOT the
        // per-turn reply stream — so the reply mapper drops it.
        val r = clientEventToReply(
            ClientEvent.SessionResumed(
                sessionId = "s",
                mode = SessionModeDto.CODE,
                messages = listOf(MessageDto(role = "user", blocks = listOf(MessageBlockDto.Text("hi")))),
            ),
        )
        assertNull(r)
    }

    // --- MessageDto → UI Message lowering (resume scrollback) --------------

    @Test
    fun messageDtoToMessage_userRole_mapsToUser_withFlatText() {
        val m = messageDtoToMessage(
            MessageDto(role = "user", blocks = listOf(MessageBlockDto.Text("你好"))),
        )
        assertEquals(Role.User, m.role)
        assertEquals("你好", m.text)
    }

    @Test
    fun messageDtoToMessage_assistantRole_mapsToAi() {
        val m = messageDtoToMessage(
            MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("回答"))),
        )
        assertEquals(Role.Ai, m.role)
        assertEquals("回答", m.text)
    }

    @Test
    fun messageDtoToMessage_systemRole_mapsToAi() {
        // System messages have no dedicated UI role — they render as the
        // assistant (avatar+markdown) bubble.
        val m = messageDtoToMessage(
            MessageDto(role = "system", blocks = listOf(MessageBlockDto.Text("系统提示"))),
        )
        assertEquals(Role.Ai, m.role)
        assertEquals("系统提示", m.text)
    }

    // --- pre-derived tool presentation ------------------------------------

    @Test
    fun toolUseStarted_carriesTheDerivedHeaderThrough_insteadOfRebuildingIt() {
        val r = clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = "e1",
                tool = "Edit",
                inputJson = """{"file_path":"src/host.rs","old_string":"a","new_string":"b"}""",
                header = ToolHeaderDto(
                    verb = ToolVerbDto.UPDATE,
                    icon = null,
                    label = "Update",
                    primary = "src/host.rs",
                    qualifier = " (3 edits)",
                    count = null,
                    subLine = null,
                    title = "Update(src/host.rs) (3 edits)",
                ),
            ),
        ) as ReplyEvent.ToolActivity

        val header = assertNotNull(r.header)
        assertEquals(ToolVerbUi.Update, header.verb)
        assertEquals("src/host.rs", header.primary)
        assertEquals(" (3 edits)", header.qualifier)
        assertEquals("Update(src/host.rs) (3 edits)", header.title)
    }

    @Test
    fun toolUseResult_carriesTheDerivedDisplayThrough_theArmUsedToDiscardIt() {
        // This arm previously threw the ENTIRE payload away: a completed tool
        // call became one dim status line with no headline, diff, or body.
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(
                id = "e1",
                tool = "Edit",
                resultJson = """"applied"""",
                isError = false,
                display = ToolResultDisplayDto(
                    headline = "Added 18 lines, removed 4 lines",
                    headlineKind = HeadlineKindDto.ADDED_REMOVED,
                    headlineArgs = listOf(18u, 4u),
                    diff = StructuredDiffDto(
                        filePath = "src/host.rs",
                        language = "rust",
                        gutterWidth = 3u,
                        additions = 18u,
                        removals = 4u,
                        truncatedRows = 0u,
                        rows = listOf(
                            DiffRowDto(
                                kind = DiffLineKindDto.ADD,
                                lineNo = 12u,
                                hunk = 0u,
                                wordDiffed = false,
                                segments = listOf(
                                    CodeSegmentDto(
                                        text = "fn main() {}",
                                        `class` = SyntaxClassDto.FUNCTION,
                                        rgb = null,
                                        bold = false,
                                        italic = false,
                                        underline = false,
                                        emph = false,
                                    ),
                                ),
                            ),
                        ),
                    ),
                    body = null,
                    bodyLines = 0u,
                    bodyTruncated = false,
                    collapsed = false,
                ),
            ),
        ) as ReplyEvent.ToolActivity

        assertEquals(AgentToolStatus.Completed, r.status)
        val display = assertNotNull(r.display)
        assertEquals(HeadlineKindUi.AddedRemoved, display.headlineKind)
        assertEquals(listOf(18, 4), display.headlineArgs)
        assertEquals("src/host.rs", display.diff?.filePath)
        assertEquals("fn main() {}", display.diff?.rows?.single()?.text)
    }

    @Test
    fun toolUseResult_failure_stillCarriesItsDisplay() {
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(
                id = "e2",
                tool = "Read",
                resultJson = """"nope"""",
                isError = true,
                display = ToolResultDisplayDto(
                    headline = "No such file",
                    headlineKind = HeadlineKindDto.FAILED,
                    headlineArgs = emptyList(),
                    diff = null,
                    body = null,
                    bodyLines = 0u,
                    bodyTruncated = false,
                    collapsed = false,
                ),
            ),
        ) as ReplyEvent.ToolActivity

        assertEquals(AgentToolStatus.Failed, r.status)
        // FAILED carries its message in `headline` — there is no catalog key.
        assertEquals("No such file", r.display?.headline)
        assertEquals(HeadlineKindUi.Failed, r.display?.headlineKind)
    }

    @Test
    fun olderEngineWithoutHeaderOrDisplay_stillMapsWithTheLegacyFallback() {
        val started = clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = "t9", tool = "Read", inputJson = """{"file_path":"/a/b.txt"}""", header = null,
            ),
        ) as ReplyEvent.ToolActivity
        assertNull(started.header)
        assertEquals("/a/b.txt", started.inputSummary) // the old summarizer survives

        val finished = clientEventToReply(
            ClientEvent.ToolUseResult(
                id = "t9", tool = "Read", resultJson = "{}", isError = false, display = null,
            ),
        ) as ReplyEvent.ToolActivity
        assertNull(finished.display)
        assertEquals(AgentToolStatus.Completed, finished.status)
    }

    // --- PlanUpdated (out-of-band) ----------------------------------------

    @Test
    fun planUpdated_ridesTheOutOfBandPath_notThePerTurnStream() {
        val event = ClientEvent.PlanUpdated(
            tasks = listOf(
                PlanTaskDto(id = null, subject = "写代码", activeForm = "正在写代码", state = PlanTaskStateDto.IN_PROGRESS),
                PlanTaskDto(id = "v2-1", subject = "跑测试", activeForm = null, state = PlanTaskStateDto.PENDING),
            ),
        )
        // The per-turn mapper must ignore it entirely…
        assertNull(clientEventToReply(event))
        // …while the out-of-band recognizer lowers the full list.
        val tasks = assertNotNull(planTasksFrom(event))
        assertEquals(listOf("写代码", "跑测试"), tasks.map { it.subject })
        assertEquals(PlanTaskStateUi.InProgress, tasks[0].state)
        assertEquals("v2-1", tasks[1].id)
    }

    @Test
    fun planUpdated_emptyList_isARealClearNotANoOp() {
        // A full-list replace with no rows CLEARS the panel; `null` (any other
        // event) means "not a plan event at all". The two must stay distinct.
        assertEquals(emptyList<PlanTaskUi>(), planTasksFrom(ClientEvent.PlanUpdated(tasks = emptyList())))
        assertNull(planTasksFrom(ClientEvent.TextDelta("hi")))
    }

    // --- structured transcript --------------------------------------------

    @Test
    fun transcriptFromDtos_pairsAToolResultBackIntoTheMessageThatCalledIt() {
        // The engine puts the ToolUse in assistant message N and its ToolResult
        // in user message N+1 — never the same message. Mapping each DTO alone
        // would render a header with no result and an empty user bubble.
        val transcript = transcriptFromDtos(
            listOf(
                MessageDto(role = "user", blocks = listOf(MessageBlockDto.Text("改一下"))),
                MessageDto(
                    role = "assistant",
                    blocks = listOf(
                        MessageBlockDto.Text("好的"),
                        MessageBlockDto.ToolUse(
                            id = "e1",
                            tool = "Edit",
                            inputJson = "{}",
                            header = ToolHeaderDto(
                                verb = ToolVerbDto.UPDATE,
                                icon = null,
                                label = "Update",
                                primary = "src/host.rs",
                                qualifier = null,
                                count = null,
                                subLine = null,
                                title = "Update(src/host.rs)",
                            ),
                        ),
                    ),
                ),
                MessageDto(
                    role = "user",
                    blocks = listOf(
                        MessageBlockDto.ToolResult(
                            id = "e1",
                            tool = "Edit",
                            resultJson = """"ok"""",
                            isError = false,
                            oldString = null,
                            newString = null,
                            filePath = null,
                            display = ToolResultDisplayDto(
                                headline = "Added 2 lines",
                                headlineKind = HeadlineKindDto.ADDED,
                                headlineArgs = listOf(2u),
                                diff = null,
                                body = null,
                                bodyLines = 0u,
                                bodyTruncated = false,
                                collapsed = false,
                            ),
                        ),
                    ),
                ),
            ),
        )

        // The result-only user message carries nothing of its own and is dropped.
        assertEquals(2, transcript.size)
        assertEquals(Role.User, transcript[0].role)
        assertEquals("改一下", transcript[0].text)

        val assistant = transcript[1]
        assertEquals(Role.Ai, assistant.role)
        assertEquals(listOf("好的"), assistant.blocks.filterIsInstance<MessageContent.Text>().map { it.text })
        val call = assistant.blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals("e1", call.id)
        assertEquals(ToolVerbUi.Update, call.header?.verb)
        assertEquals(HeadlineKindUi.Added, call.display?.headlineKind)
        assertEquals(AgentToolStatus.Completed, call.status)
    }

    @Test
    fun transcriptFromDtos_errorResult_marksTheCallFailed() {
        val transcript = transcriptFromDtos(
            listOf(
                MessageDto(
                    role = "assistant",
                    blocks = listOf(
                        MessageBlockDto.ToolUse(id = "r1", tool = "Read", inputJson = "{}", header = null),
                    ),
                ),
                MessageDto(
                    role = "user",
                    blocks = listOf(
                        MessageBlockDto.ToolResult(
                            id = "r1", tool = "Read", resultJson = """"boom"""", isError = true,
                            oldString = null, newString = null, filePath = null, display = null,
                        ),
                    ),
                ),
            ),
        )
        val call = transcript.single().blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals(AgentToolStatus.Failed, call.status)
    }

    @Test
    fun transcriptFromDtos_orphanResult_attachesToThePrecedingTurn_notAUserBubble() {
        // A compacted / torn transcript window can start with a result whose call
        // is gone. It still belongs to the turn before it.
        val transcript = transcriptFromDtos(
            listOf(
                MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("继续"))),
                MessageDto(
                    role = "user",
                    blocks = listOf(
                        MessageBlockDto.ToolResult(
                            id = "gone", tool = "Grep", resultJson = """"x"""", isError = false,
                            oldString = null, newString = null, filePath = null, display = null,
                        ),
                    ),
                ),
            ),
        )
        assertEquals(1, transcript.size)
        val call = transcript.single().blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals("gone", call.id)
        assertEquals("Grep", call.tool)
        assertNull(call.header)
    }

    @Test
    fun transcriptFromDtos_orphanResultInTheFirstMessage_neverBecomesAnEmptyUserBubble() {
        // The window can OPEN on the result-carrying user message, so there is no
        // preceding turn at all. Parking the tool block on that user build made
        // `isRenderable()` say yes while the user branch of the bubble renders
        // `text` only — an empty bordered bubble AND a lost tool call.
        val transcript = transcriptFromDtos(
            listOf(
                MessageDto(
                    role = "user",
                    blocks = listOf(
                        MessageBlockDto.ToolResult(
                            id = "gone", tool = "Grep", resultJson = """"x"""", isError = false,
                            oldString = null, newString = null, filePath = null, display = null,
                        ),
                    ),
                ),
                MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("继续"))),
            ),
        )

        assertTrue(
            "no bubble may be empty",
            transcript.none { it.text.isBlank() && it.blocks.isEmpty() },
        )
        assertTrue(
            "a user bubble renders text only, so it must never hold a tool block",
            transcript.none { it.role == Role.User && it.blocks.any { b -> b is MessageContent.Tool } },
        )
        // The call itself survives, in an assistant bubble ahead of the reply.
        assertEquals(listOf(Role.Ai, Role.Ai), transcript.map { it.role })
        val call = transcript[0].blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals("gone", call.id)
        assertEquals("Grep", call.tool)
        assertEquals("继续", transcript[1].text)
    }

    @Test
    fun transcriptFromDtos_orphanResultAfterAnotherUserMessage_landsInAnAssistantTurnOfItsOwn() {
        // `previous` being a USER build is the same defect in a different shape.
        // Here the CALLING assistant turn is what the window dropped, so the row
        // belongs in an assistant bubble at that spot — never in the user message
        // that happens to sit in front of it.
        val transcript = transcriptFromDtos(
            listOf(
                MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("在查"))),
                MessageDto(role = "user", blocks = listOf(MessageBlockDto.Text("好"))),
                MessageDto(
                    role = "user",
                    blocks = listOf(
                        MessageBlockDto.ToolResult(
                            id = "gone", tool = "Grep", resultJson = """"x"""", isError = false,
                            oldString = null, newString = null, filePath = null, display = null,
                        ),
                    ),
                ),
            ),
        )

        assertEquals(listOf(Role.Ai, Role.User, Role.Ai), transcript.map { it.role })
        assertTrue(
            transcript.none { it.role == Role.User && it.blocks.any { b -> b is MessageContent.Tool } },
        )
        assertEquals("在查", transcript[0].text)
        assertEquals("好", transcript[1].text)
        assertEquals("gone", transcript[2].blocks.filterIsInstance<MessageContent.Tool>().single().call.id)
    }

    @Test
    fun messageDtoToMessage_textOnlyMessage_hasNoStructuredToolBlocks() {
        val m = messageDtoToMessage(
            MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("纯文本"))),
        )
        assertEquals("纯文本", m.text)
        assertTrue(m.blocks.none { it is MessageContent.Tool })
    }

    private fun <T> assertNotNull(value: T?): T {
        org.junit.Assert.assertNotNull(value)
        return value!!
    }
}
