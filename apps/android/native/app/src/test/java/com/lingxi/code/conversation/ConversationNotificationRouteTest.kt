package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import org.junit.Assert.*
import org.junit.Test

class ConversationNotificationRouteTest {
    @Test fun `notification produces the shared canonical contract`() {
        val url = ConversationNotificationRoute.canonicalUrl("sess:session-a")
        assertEquals("https://lingxios.net/open/session/session-a", url)
        assertEquals(ConversationLaunchRequest("session-a", null), ConversationNotificationRoute.parse(url))
    }
    @Test fun `canonical notification resolves workspace mode`() {
        assertEquals(ConversationLaunchRequest("session-a", null, "project.a", SessionMode.Chat), ConversationNotificationRoute.parse("lingxi://session/session-a?workspace=project.a&mode=chat"))
        assertNull(ConversationNotificationRoute.parse("https://evil.test/open/session/a"))
    }
    @Test fun `stop route remains a private service command`() {
        val route = ConversationNotificationRoute.cancelRouteSpec("session-a", 42L)
        assertEquals(ConversationTurnService.ACTION_CANCEL, route.action)
        assertEquals(ConversationTurnService::class.java.name, route.targetClassName)
        assertEquals(42L, route.turnId)
    }
}
