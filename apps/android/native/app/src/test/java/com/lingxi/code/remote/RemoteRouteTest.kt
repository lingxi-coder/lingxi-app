package com.lingxi.code.remote

import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.push.PushRoute
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RemoteRouteTest {
    private val host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private val session = "11111111-2222-4333-8444-555555555555"

    @Test fun encodeAndDecodeRoundTripEveryRoute() {
        for (route in listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Session(host, session))) {
            assertEquals(route, RemoteRoute.decode(route.encode()))
        }
    }

    @Test fun decodeRejectsMalformedText() {
        for (text in listOf("", "host", "host/", "session/a", "session//b", "session/a/b/c", "other/x")) {
            assertNull(text, RemoteRoute.decode(text))
        }
    }

    @Test fun customSchemeLinkOpensTheSession() {
        val link = DeepLink.parse("lingxi://remote/$host/$session")!!
        assertEquals(RemoteRoute.Session(host, session), RemoteRoute.fromLink(link))
    }

    @Test fun universalLinkOpensTheSession() {
        val link = DeepLink.parse("https://lingxios.net/open/remote/$host/abc")!!
        assertEquals(RemoteRoute.Session(host, "abc"), RemoteRoute.fromLink(link))
    }

    @Test fun otherRoutesAreNotRemoteSessions() {
        assertNull(RemoteRoute.fromLink(DeepLink.parse("lingxi://new")!!))
        assertNull(RemoteRoute.fromLink(DeepLink.parse("lingxi://session/abc")!!))
    }

    @Test fun malformedRemoteLinksDoNotParse() {
        for (raw in listOf(
            "lingxi://remote/host-only",
            "lingxi://remote/host/session/extra",
            "lingxi://remote/host/session#payload=%7B%22prompt%22%3A%22x%22%7D",
        )) assertNull(raw, DeepLink.parse(raw))
    }

    @Test fun directOpenKeepsAnHonestBackStack() {
        val target = RemoteRoute.Session(host, session)
        assertEquals(listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), target), RemoteRoute.stackFor(target))
        assertEquals(listOf(RemoteRoute.Hosts), RemoteRoute.stackFor(RemoteRoute.Hosts))
    }

    @Test fun pushRouteBecomesTheSameLinkTheInboxVets() {
        val link = PushRoute.link("remote/$host/$session")!!
        assertEquals("lingxi://remote/$host/$session", link)
        assertEquals(RemoteRoute.Session(host, session), RemoteRoute.fromLink(DeepLink.parse(link)!!))
    }

    @Test fun pushRouteOnlyOpensRemoteSessions() {
        assertNull(PushRoute.link(null))
        assertNull(PushRoute.link(""))
        assertNull(PushRoute.link("new"))
        assertNull(PushRoute.link("session/abc"))
        assertNull(PushRoute.link("remote/host"))
        assertNull(PushRoute.link("remote/a/b/c"))
        assertNull(PushRoute.link("remote/a/b#payload=%7B%7D"))
    }
}
