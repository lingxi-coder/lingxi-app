package com.lingxi.code.conversation

import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins [retainedTurnEventToReply] to the shape the ENGINE actually retains.
 *
 * The durable journal stores `serde_json::to_string(&ClientEvent)`, so the
 * Android decoder's only contract is the Rust wire form. `outcome` is an
 * internally tagged object (`TurnOutcomeDto` carries
 * `#[serde(tag = "type", rename_all = "snake_case")]`), and the decoder used to
 * read it with `optString("outcome")` — which matches "end_turn" under NO
 * org.json implementation. A replayed `turn_ended` therefore never produced
 * `ReplyEvent.End`, `streaming` stayed true, and the composer stayed locked on
 * every recovered turn.
 *
 * The fixture is not transcribed here: it is READ from the blessed
 * client-protocol snapshot, so the shape cannot drift on one side only.
 */
class RetainedTurnEventDecoderTest {

    @Test
    fun blessedSnapshotStillCarriesOutcomeAsATaggedObject() {
        val snapshot = JSONObject(blessedTurnEndedSnapshot().readText())
        assertEquals("turn_ended", snapshot.optString("type"))
        val outcome = snapshot.optJSONObject("outcome")
        assertNotNull(
            "Pinned Harness client-protocol/snapshots/event/turn_ended.json no longer carries " +
                "`outcome` as a tagged OBJECT; retainedTurnEventToReply's decoder must be " +
                "re-derived from the new shape before this test is relaxed.",
            outcome,
        )
        assertEquals("end_turn", outcome!!.optString("type"))
        // The exact call the decoder used to make, spelled out so the reason
        // this test exists survives: it cannot yield "end_turn".
        assertTrue(
            "optString(\"outcome\") must not be used to read a tagged outcome",
            snapshot.optString("outcome") != "end_turn",
        )
    }

    @Test
    fun blessedTurnEndedSnapshotDecodesToReplyEventEnd() {
        val decoded = retainedTurnEventToReply(blessedTurnEndedSnapshot().readText())
        assertEquals(ReplyEvent.End, decoded)
    }

    @Test
    fun legacyBareStringOutcomeStillDecodes() {
        assertEquals(
            ReplyEvent.End,
            retainedTurnEventToReply("""{"type":"turn_ended","outcome":"end_turn"}"""),
        )
    }

    @Test
    fun nonEndTurnOutcomesDoNotSettleTheTurn() {
        assertNull(
            retainedTurnEventToReply("""{"type":"turn_ended","outcome":{"type":"cancelled"}}"""),
        )
        assertNull(
            retainedTurnEventToReply("""{"type":"turn_ended","outcome":{"type":"max_turns"}}"""),
        )
        assertNull(retainedTurnEventToReply("""{"type":"turn_ended"}"""))
    }

    @Test
    fun retainedToolEventsPreserveRequiredHeaderAndDisplay() {
        val started = retainedTurnEventToReply(
            """{"type":"tool_use_started","id":"tool-1","tool":"Read","input_json":"{}","header":{"verb":"read","label":"Read","title":"Read"}}""",
        ) as ReplyEvent.ToolActivity
        assertEquals("Read", started.header?.title)

        val result = retainedTurnEventToReply(
            """{"type":"tool_use_result","id":"tool-1","tool":"Read","result_json":"{}","is_error":false,"display":{"headline":"Read complete","body":"🚀 source","body_lines":1,"diff":{"file_path":"a.rs","language":"rust","gutter_width":1,"additions":1,"removals":0,"truncated_rows":0,"rows":[{"kind":"add","line_no":1,"hunk":0,"word_diffed":false,"segments":[{"text":"🚀 source","class":"plain","bold":false,"italic":false,"underline":false,"emph":false}]}]}}}""",
        ) as ReplyEvent.ToolActivity
        assertEquals("Read complete", result.display?.headline)
        assertEquals("🚀 source", result.display?.body)
        assertEquals("🚀 source", result.display?.diff?.rows?.single()?.text)
    }

    @Test
    fun retainedToolEventsWithoutCurrentCarriersAreNotProjected() {
        assertNull(
            retainedTurnEventToReply(
                """{"type":"tool_use_started","id":"tool-1","tool":"Read","input_json":"{}"}""",
            ),
        )
        assertNull(
            retainedTurnEventToReply(
                """{"type":"tool_use_result","id":"tool-1","tool":"Read","result_json":"{}","is_error":false}""",
            ),
        )
    }

    private fun blessedTurnEndedSnapshot(): File =
        RuntimeProtocolFixtures.snapshot("event/turn_ended.json")
}
