package com.lingxi.code.settings

import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.SettingsJumpState
import com.lingxi.code.tabs.SharedVectors
import com.lingxi.code.tabs.objects
import com.lingxi.code.tabs.stringOrNull
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * What Back means inside the Settings tab (SPEC-mobile-team-bot-ui section 2.6): leave for the tab a jump
 * came from, pop a page, or leave it to the shell. The rules are `SettingsJumpState`'s; these tests pin
 * the three answers the host acts on, including against every Back of the shared cross-platform table.
 */
class SettingsBackActionTest {
    private fun pages(count: Int) = List(count) { "page$it" }

    @Test
    fun theRootWithNothingRememberedBelongsToTheShell() {
        assertEquals(SettingsBackAction.ShellDecides, settingsBackAction(pages(0), origin = null))
    }

    @Test
    fun theRootAfterAJumpToItLeavesForTheOrigin() {
        assertEquals(SettingsBackAction.LeaveTo(AppTab.Bots), settingsBackAction(pages(0), origin = AppTab.Bots))
    }

    @Test
    fun thePageAJumpOpenedLeavesForTheOrigin() {
        assertEquals(SettingsBackAction.LeaveTo(AppTab.Chats), settingsBackAction(pages(1), origin = AppTab.Chats))
    }

    @Test
    fun deeperPagesPopBeforeAnythingLeaves() {
        assertEquals(SettingsBackAction.Pop, settingsBackAction(pages(2), origin = AppTab.Chats))
        assertEquals(SettingsBackAction.Pop, settingsBackAction(pages(3), origin = AppTab.Teams))
    }

    @Test
    fun withoutAnOriginEveryPageBelowTheRootPops() {
        assertEquals(SettingsBackAction.Pop, settingsBackAction(pages(1), origin = null))
        assertEquals(SettingsBackAction.Pop, settingsBackAction(pages(4), origin = null))
    }

    @Test
    fun everyBackOfTheSharedJumpTableAgrees() {
        val cases = SharedVectors.mobile.getJSONArray("settingsJump").objects()
        var backs = 0
        cases.forEach { case ->
            var state = SettingsJumpState()
            val events = case.getJSONArray("events").objects()
            val trace = case.getJSONArray("trace").objects()
            events.zip(trace).forEach { (event, step) ->
                if (event.getString("type") == "back") {
                    backs++
                    val expectedLeave = AppTab.fromKey(step.stringOrNull("leaveTo"))
                    val expected = when {
                        expectedLeave != null -> SettingsBackAction.LeaveTo(expectedLeave)
                        step.getJSONArray("stack").length() < state.stack.size -> SettingsBackAction.Pop
                        else -> SettingsBackAction.ShellDecides
                    }
                    assertEquals("${case.getString("name")}: Back from ${state.stack} / ${state.origin}", expected, settingsBackAction(state.stack, state.origin))
                }
                state = state.reduce(eventOf(event)).state
            }
        }
        // The table has Backs in it; an empty loop would pass for the wrong reason.
        assertEquals(true, backs >= 8)
    }

    private fun eventOf(json: org.json.JSONObject): SettingsJumpState.Event = when (val type = json.getString("type")) {
        "jump" -> SettingsJumpState.Event.Jump(json.getString("page"), AppTab.fromKey(json.stringOrNull("origin")))
        "push" -> SettingsJumpState.Event.Push(json.getString("page"))
        "back" -> SettingsJumpState.Event.Back
        "userSelectedTab" -> SettingsJumpState.Event.UserSelectedTab
        else -> error("unknown event type $type")
    }
}
