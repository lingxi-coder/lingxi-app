package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.CostDto
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
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
    fun thinkingDelta_mapsToThinking() {
        val r = clientEventToReply(ClientEvent.ThinkingDelta(thinking = "reasoning…", signature = null))
        assertEquals(ReplyEvent.Thinking, r)
    }

    @Test
    fun turnStarted_mapsToThinking() {
        assertEquals(ReplyEvent.Thinking, clientEventToReply(ClientEvent.TurnStarted(turnId = null)))
    }

    // --- tool activity ----------------------------------------------------

    @Test
    fun toolUseStarted_mapsToToolActivity_withToolName() {
        val r = clientEventToReply(
            ClientEvent.ToolUseStarted(id = "t1", tool = "bash", inputJson = "{}"),
        )
        assertTrue(r is ReplyEvent.ToolActivity)
        assertTrue((r as ReplyEvent.ToolActivity).label.contains("bash"))
    }

    @Test
    fun toolUseResult_success_mapsToToolActivity_notError() {
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(id = "t1", tool = "read", resultJson = "{}", isError = false),
        )
        assertTrue(r is ReplyEvent.ToolActivity)
        val label = (r as ReplyEvent.ToolActivity).label
        assertTrue(label.contains("read"))
        assertTrue(label.contains("完成"))
    }

    @Test
    fun toolUseResult_error_mapsToToolActivity_failureLabel() {
        val r = clientEventToReply(
            ClientEvent.ToolUseResult(id = "t1", tool = "write", resultJson = "{}", isError = true),
        )
        assertTrue(r is ReplyEvent.ToolActivity)
        val label = (r as ReplyEvent.ToolActivity).label
        assertTrue(label.contains("write"))
        assertTrue(label.contains("失败"))
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

    // --- ignored events ---------------------------------------------------

    @Test
    fun messageComplete_withMessage_mapsToCompleted() {
        val r = clientEventToReply(
            ClientEvent.MessageComplete(
                stopReason = "end_turn",
                message = MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("done"))),
            ),
        )
        assertTrue(r is ReplyEvent.Completed)
        val completed = r as ReplyEvent.Completed
        assertEquals(Role.Ai, completed.message.role)
        assertEquals("done", completed.message.text)
    }

    @Test
    fun messageComplete_withoutMessage_mapsToEnd() {
        val r = clientEventToReply(ClientEvent.MessageComplete(stopReason = "end_turn", message = null))
        assertEquals(ReplyEvent.End, r)
    }

    @Test
    fun usageUpdate_isIgnored() {
        val r = clientEventToReply(
            ClientEvent.UsageUpdate(
                inputTokens = 1u, outputTokens = 2u, cacheReadTokens = 0u, cacheCreationTokens = 0u,
            ),
        )
        assertNull(r)
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
}
