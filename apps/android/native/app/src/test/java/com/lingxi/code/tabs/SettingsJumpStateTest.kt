package com.lingxi.code.tabs

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Section 8.6 `SettingsJumpState`: replays each case of the shared table and checks every step of its trace. */
class SettingsJumpStateTest {
    private fun eventOf(json: JSONObject): SettingsJumpState.Event = when (val type = json.getString("type")) {
        "jump" -> SettingsJumpState.Event.Jump(json.getString("page"), AppTab.fromKey(json.stringOrNull("origin")))
        "push" -> SettingsJumpState.Event.Push(json.getString("page"))
        "back" -> SettingsJumpState.Event.Back
        "userSelectedTab" -> SettingsJumpState.Event.UserSelectedTab
        else -> error("unknown event type $type")
    }

    @Test fun everySharedCaseReplaysToItsTrace() {
        val cases = SharedVectors.mobile.getJSONArray("settingsJump").objects()
        assertEquals(8, cases.size)
        cases.forEach { case ->
            val name = case.getString("name")
            val events = case.getJSONArray("events").objects()
            val trace = case.getJSONArray("trace").objects()
            assertEquals(name, events.size, trace.size)
            var state = SettingsJumpState()
            events.zip(trace).forEachIndexed { index, (eventJson, step) ->
                assertEquals("$name: trace event $index", eventJson.getString("type"), step.getJSONObject("event").getString("type"))
                val reduction = state.reduce(eventOf(eventJson))
                state = reduction.state
                val expectedStack = step.getJSONArray("stack").let { array -> (0 until array.length()).map(array::getString) }
                assertEquals("$name: stack after step $index", expectedStack, state.stack)
                assertEquals("$name: origin after step $index", AppTab.fromKey(step.stringOrNull("origin")), state.origin)
                assertEquals("$name: leaveTo after step $index", AppTab.fromKey(step.stringOrNull("leaveTo")), reduction.leaveTo)
            }
        }
    }

    @Test fun theAppsRealSettingsRootCountsAsMain() {
        // The shell passes the real route; the vectors use the page name "main".
        val root = "settings/main"
        val jumped = SettingsJumpState().reduce(SettingsJumpState.Event.Jump(root, AppTab.Bots), mainPage = root)
        assertTrue(jumped.state.stack.isEmpty())
        assertEquals(AppTab.Bots, jumped.state.origin)
        val back = jumped.state.reduce(SettingsJumpState.Event.Back, mainPage = root)
        assertEquals(AppTab.Bots, back.leaveTo)
        assertEquals(SettingsJumpState(), back.state)
    }

    @Test fun aLeavingBackAlwaysEndsWithAnEmptyStackAndNoOrigin() {
        val deep = SettingsJumpState(stack = listOf("a"), origin = AppTab.Teams)
        val left = deep.reduce(SettingsJumpState.Event.Back)
        assertEquals(AppTab.Teams, left.leaveTo)
        assertTrue(left.state.stack.isEmpty())
        assertEquals(null, left.state.origin)
        // A second Back at the root with nothing remembered is the shell's decision, not a leave.
        val again = left.state.reduce(SettingsJumpState.Event.Back)
        assertEquals(null, again.leaveTo)
    }
}
