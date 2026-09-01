package com.lingxi.code.conversation

import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.CostDto
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.TurnOutcomeDto
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Flow-level coverage of the engine reply stream — the streaming/ordering half of
 * the engine conversation path (the per-event mapping is covered by
 * [ClientEventMapperTest], the reducer by [ChatViewModelReducerTest]).
 *
 * The unit under test is [mapReplyStream], the PURE transform
 * [EngineConversationSource.submit] is built from. These tests have NO engine /
 * Android dependency, so they run on the plain JVM where `buildAndroidEngine` is
 * unavailable — the data-class fixtures only CONSTRUCT generated UniFFI types,
 * never call an exported function, so no native `.so` is loaded.
 *
 * The headline test ([noDeltaDroppedAcrossSubmitWindow]) reproduces the
 * subscribe-before-submit race the prior `flow { submit(); emitAll(events…) }`
 * structure had: with a `replay = 0` SharedFlow, a `TextDelta` emitted in the
 * window between "submit returns" and "the collector subscribes" was dropped. We
 * model the engine's listener thread by emitting the FIRST delta from inside the
 * `onSubscription` action (the same hook `submit` fires the engine command from) —
 * if that delta survives to the collector, the ordering guarantee holds.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class EngineReplyStreamTest {

    private val cost = CostDto(
        totalUsd = 0.0,
        inputTokens = 0u,
        outputTokens = 0u,
        apiCalls = 0u,
        sessionDurationSecs = 0u,
        formatted = "$0.00",
    )

    @Test
    fun losslessRelay_preservesBurstAndTerminalEvent_forSlowCollector() = runTest {
        val relay = LosslessEventRelay<ClientEvent>(backgroundScope)
        val received = async {
            relay.events
                .take(1_001)
                .toList()
        }
        runCurrent() // collector is subscribed before the callback burst starts

        repeat(1_000) { relay.offer(ClientEvent.TextDelta("$it")) }
        relay.offer(
            ClientEvent.TurnEnded(
                outcome = TurnOutcomeDto.END_TURN,
                stopReason = "end_turn",
                cost = cost,
            ),
        )
        runCurrent()

        val values = received.await()
        assertEquals(1_001, values.size)
        assertEquals("0", (values.first() as ClientEvent.TextDelta).text)
        assertEquals("999", (values[999] as ClientEvent.TextDelta).text)
        assertTrue(values.last() is ClientEvent.TurnEnded)
        relay.close()
    }

    /**
     * The race regression test. A `replay = 0` SharedFlow (exactly the engine
     * source's buffer config) plus `onSubscription` — which is where production
     * code fires `SendPrompt` — means the very first delta, emitted synchronously
     * as the turn is armed, must still reach the collector because the collector
     * is already subscribed before that emission happens.
     */
    @Test
    fun noDeltaDroppedAcrossSubmitWindow() = runTest {
        val events = MutableSharedFlow<ClientEvent>(
            replay = 0,
            extraBufferCapacity = 256,
        )
        // `onSubscription` runs AFTER this collector is registered but BEFORE any
        // upstream value is delivered — the exact ordering submit-before-subscribe
        // would violate. Emitting here models the engine spawning the turn and the
        // listener thread firing the first token in the old race window.
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.TextDelta("first")) // the would-be-dropped delta
                emit(ClientEvent.TextDelta("-second"))
                emit(ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost))
            },
        )

        val replies = stream.toList()

        // Leading Thinking, both deltas IN ORDER, then End. The "first" delta is
        // present — the race is closed.
        assertEquals(
            listOf(
                ReplyEvent.Thinking,
                ReplyEvent.Delta("first"),
                ReplyEvent.Delta("-second"),
                ReplyEvent.End,
            ),
            replies,
        )
    }

    /** The stream always opens with a Thinking beat before any engine event. */
    @Test
    fun streamOpensWithThinking() = runTest {
        val events = MutableSharedFlow<ClientEvent>(replay = 0, extraBufferCapacity = 8)
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = null, cost = cost))
            },
        )
        assertEquals(ReplyEvent.Thinking, stream.toList().first())
    }

    /** A terminal Error is followed by a synthesized End, then the stream completes. */
    @Test
    fun errorTerminatesWithTrailingEnd() = runTest {
        val events = MutableSharedFlow<ClientEvent>(replay = 0, extraBufferCapacity = 8)
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.TextDelta("partial"))
                emit(ClientEvent.Error(kind = ErrorKindDto.TRANSPORT, message = "boom"))
                // Anything after the terminal must NOT appear — the stream has stopped.
                emit(ClientEvent.TextDelta("late"))
            },
        )

        val replies = stream.toList()
        assertEquals(
            listOf(
                ReplyEvent.Thinking,
                ReplyEvent.Delta("partial"),
                ReplyEvent.Error("boom"),
                ReplyEvent.End,
            ),
            replies,
        )
        assertTrue("no events after terminal", replies.none { it == ReplyEvent.Delta("late") })
    }

    /** Live usage appears while out-of-band configuration events stay filtered. */
    @Test
    fun telemetryFlowsWithoutLeakingOutOfBandEvents() = runTest {
        val events = MutableSharedFlow<ClientEvent>(replay = 0, extraBufferCapacity = 8)
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.UsageUpdate(inputTokens = 1u, outputTokens = 2u, cacheReadTokens = 0u, cacheCreationTokens = 0u))
                emit(ClientEvent.TextDelta("hi"))
                emit(ClientEvent.ModelChanged(model = "opus"))
                emit(ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = null, cost = cost))
            },
        )

        assertEquals(
            listOf(
                ReplyEvent.Thinking,
                ReplyEvent.Usage(AgentRunUsage(1, 2, 0, 0)),
                ReplyEvent.Delta("hi"),
                ReplyEvent.End,
            ),
            stream.toList(),
        )
    }

    @Test
    fun messageCompleteDoesNotTerminateBeforeTurnEnded() = runTest {
        val events = MutableSharedFlow<ClientEvent>(replay = 0, extraBufferCapacity = 8)
        val message = com.lingxi.code.bindings.MessageDto(
            role = "assistant",
            blocks = listOf(com.lingxi.code.bindings.MessageBlockDto.Text("first")),
            images = emptyList(),
        )
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.TextDelta("first"))
                emit(ClientEvent.MessageComplete(stopReason = "end_turn", message = message))
                emit(ClientEvent.TextDelta("second"))
                emit(ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost))
            },
        )

        val replies = stream.toList()
        assertEquals(5, replies.size)
        assertEquals(ReplyEvent.Thinking, replies[0])
        assertEquals(ReplyEvent.Delta("first"), replies[1])
        val complete = replies[2] as ReplyEvent.MessageComplete
        assertEquals("first", complete.message?.text)
        assertEquals(listOf(MessageContent.Text("first")), complete.message?.blocks)
        assertEquals(ReplyEvent.Delta("second"), replies[3])
        assertEquals(ReplyEvent.End, replies[4])
    }

    @Test
    fun durableReplayEnvelopeAcksOnlyAfterRenderableRawEvent() = runTest {
        val events = MutableSharedFlow<ClientEvent>(replay = 0, extraBufferCapacity = 8)
        val stream = mapReplyStream(
            events.onSubscription {
                emit(ClientEvent.TextDelta("live"))
                emit(
                    ClientEvent.TurnEventReplay(
                        sessionId = "session-a",
                        turnId = 9u,
                        sequence = 1u,
                        eventJson = """{"type":"text_delta","text":"live"}""",
                    ),
                )
                emit(ClientEvent.ModelChanged(model = "ignored"))
                emit(
                    ClientEvent.TurnEventReplay(
                        sessionId = "session-a",
                        turnId = 9u,
                        sequence = 2u,
                        eventJson = """{"type":"model_changed","model":"ignored"}""",
                    ),
                )
                emit(ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = null, cost = cost))
            },
        )

        assertEquals(
            listOf(
                ReplyEvent.Thinking,
                ReplyEvent.Delta("live"),
                ReplyEvent.DurableTurnReplayAcknowledged(turnId = 9L, sequence = 1L),
                ReplyEvent.End,
            ),
            stream.toList(),
        )
    }
}
