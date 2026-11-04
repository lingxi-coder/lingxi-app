package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import org.junit.Assert.*
import org.junit.Test

class ConversationNotificationRouteTest {
    @Test fun `internal notification requires its exact captured transcript path`() {
        val generation = "19587a33-0725-48db-abca-8a2aed345f6b"
        val url = "lingxi://session/session-a?workspace=global&mode=chat&generation=$generation"
        assertNull(ConversationNotificationRoute.parseNotification(url, null))
        assertNull(ConversationNotificationRoute.parseNotification(url, "relative.jsonl"))
        val request = requireNotNull(ConversationNotificationRoute.parseNotification(url, "/a/session.jsonl"))
        assertEquals("/a/session.jsonl", request.transcriptPath)
        assertFalse(selectedTranscriptPathMatches(request.transcriptPath, "/b/session.jsonl"))
    }
    @Test fun `notification produces the shared canonical contract`() {
        val generation = "19587a33-0725-48db-abca-8a2aed345f6b"
        val spec = ConversationRecoverySpec(null, null, SessionMode.Code,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "global", runtimeGeneration = generation)
        val url = ConversationNotificationRoute.canonicalUrl("sess:session-a", spec)
        assertEquals(ConversationLaunchRequest("session-a", null, "global", SessionMode.Code,
            runtimeGeneration = generation), ConversationNotificationRoute.parse(url))
        assertNull(ConversationNotificationRoute.parse("https://lingxios.net/open/session/session-a"))
        assertNull(ConversationNotificationRoute.parse("https://lingxios.net/open/session/session-a?mode=unknown"))
        assertNull(ConversationNotificationRoute.parse("https://lingxios.net/open/session/session-a?mode=chat&generation=$generation"))
        assertNull(ConversationNotificationRoute.parse("https://lingxios.net/open/session/session-a?workspace=global&mode=chat"))
    }
    @Test fun `canonical notification resolves workspace mode`() {
        val generation = "19587a33-0725-48db-abca-8a2aed345f6b"
        assertEquals(ConversationLaunchRequest("session-a", null, "project.a", SessionMode.Chat, generation),
            ConversationNotificationRoute.parse("lingxi://session/session-a?workspace=project.a&mode=chat&generation=$generation"))
        assertNull(ConversationNotificationRoute.parse("https://evil.test/open/session/a"))
    }
    @Test fun `stop route remains a private service command`() {
        val route = ConversationNotificationRoute.cancelRouteSpec("session-a", 42L)
        assertEquals(ConversationTurnService.ACTION_CANCEL, route.action)
        assertEquals(ConversationTurnService::class.java.name, route.targetClassName)
        assertEquals(42L, route.turnId)
    }
    @Test fun `notification keeps the exact turn and runtime generation`() {
        val spec = ConversationRecoverySpec("a", "/home/a", SessionMode.Chat,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "project.a",
            runtimeGeneration = "19587a33-0725-48db-abca-8a2aed345f6b")
        val url = ConversationNotificationRoute.canonicalUrl("session-a", spec, 42L)
        assertEquals(ConversationLaunchRequest("session-a", 42L, "project.a", SessionMode.Chat,
            "19587a33-0725-48db-abca-8a2aed345f6b"), ConversationNotificationRoute.parse(url))
    }
}
