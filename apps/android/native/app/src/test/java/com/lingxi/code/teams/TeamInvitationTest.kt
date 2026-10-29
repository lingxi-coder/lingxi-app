package com.lingxi.code.teams

import org.junit.Assert.*
import org.junit.Test
import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.push.PushRoute
import com.lingxi.code.navigation.DeepLinkInbox
import com.lingxi.code.navigation.DeepLinkReadiness
import androidx.lifecycle.SavedStateHandle

class TeamInvitationTest {
    private val code = "AbCdEfGhIjKlMnOpQrStUvWxYz_12345"
    @Test fun invitationPreservesIdentityAcrossSchemeAndHttps() {
        for (raw in listOf(TeamInvitation.link(code), "lingxi://team/join?code=$code")) {
            assertEquals(code, TeamInvitation.code(raw))
            assertEquals("team/join", DeepLink.parse(raw)!!.route)
            assertEquals(code, DeepLink.parse(raw)!!.params["code"])
        }
    }
    @Test fun rejectsAmbiguousInvitationParameters() {
        for (raw in listOf("lingxi://team/join", "lingxi://team/join?code=short", "lingxi://team/join?code=$code&code=$code",
            "lingxi://team/join?code=$code&workspace=secret", "lingxi://team/join?code=$code#payload=x",
            "https://lingxios.net.evil/open/team/join?code=$code", "https://user:password@lingxios.net/open/team/join?code=$code")) {
            assertNull(raw, TeamInvitation.code(raw)); assertNull(raw, DeepLink.parse(raw))
        }
        assertNull(DeepLink.parse("lingxi://session/abc?code=$code"))
    }
    @Test fun invitationCanNavigateWithoutTheLocalEngineButWaitsForBlockingSurfaces() {
        val inbox = DeepLinkInbox(SavedStateHandle())
        val raw = TeamInvitation.link(code)
        assertTrue(inbox.enqueue(raw))
        val ready = DeepLinkReadiness(true, true, false, true, false)
        assertEquals(raw, inbox.next(ready))
        assertNull(inbox.next(ready.copy(blockingSurfaceOpen = true)))
        assertNull(inbox.next(ready.copy(foreground = false)))
    }
    @Test fun pushInboxBypassesEngineAndRejectsParameters() {
        val inbox = DeepLinkInbox(SavedStateHandle())
        val link = "lingxi://team/inbox"
        assertEquals("team/inbox", DeepLink.parse(link)?.route)
        assertEquals(link, PushRoute.link("team/inbox"))
        assertNull(PushRoute.link("team/inbox?workspace=x"))
        assertTrue(inbox.enqueue(link))
        assertEquals(link, inbox.next(DeepLinkReadiness(true, true, false, true, false)))
        assertNull(DeepLink.parse("lingxi://team/inbox?workspace=x"))
        assertNull(DeepLink.parse("lingxi://team/inbox#payload=%7B%7D"))
    }
}
