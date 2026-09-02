package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ConversationNotificationRouteTest {
    @Test
    fun `conversation route parses session and turn ids`() {
        assertEquals(
            ConversationLaunchRequest(
                sessionId = "session-a",
                turnId = 42L,
                workspaceKey = "app.weather",
                sessionMode = SessionMode.Chat,
            ),
            ConversationNotificationRoute.parse(
                "lingxi://open_conversation?sessionId=session-a&turnId=42&workspaceKey=app.weather&sessionMode=chat",
            ),
        )
    }

    @Test
    fun `conversation route rejects missing session id and bad turn id`() {
        assertNull(
            ConversationNotificationRoute.parse("lingxi://open_conversation?turnId=42"),
        )
        assertEquals(
            ConversationLaunchRequest(sessionId = "session-a", turnId = null),
            ConversationNotificationRoute.parse(
                "lingxi://open_conversation?sessionId=session-a&turnId=nope",
            ),
        )
    }

    @Test
    fun `legacy conversation route defaults to code mode`() {
        assertEquals(
            ConversationLaunchRequest(
                sessionId = "session-a",
                turnId = null,
                workspaceKey = null,
                sessionMode = SessionMode.Code,
            ),
            ConversationNotificationRoute.parse(
                "lingxi://open_conversation?sessionId=session-a",
            ),
        )
    }

    @Test
    fun `stop route is private service command and never an activity deep link`() {
        val route = ConversationNotificationRoute.cancelRouteSpec("session-a", 42L)

        assertEquals(ConversationTurnService.ACTION_CANCEL, route.action)
        assertEquals(ConversationTurnService::class.java.name, route.targetClassName)
        assertEquals("session-a", route.sessionId)
        assertEquals(42L, route.turnId)
    }
}
