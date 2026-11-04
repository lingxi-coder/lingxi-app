package com.lingxi.code.remote

import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.push.PushRoute
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteRouteTest {
    private val host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private val session = "11111111-2222-4333-8444-555555555555"

    /** A Bot id as the desktop makes them: 22 characters of base64url, so `-` and `_` occur. */
    private val bot = "A-_b1234567890123456Zz"

    @Test fun encodeAndDecodeRoundTripEveryRoute() {
        for (route in listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Bot(host, bot), RemoteRoute.Session(host, session))) {
            assertEquals(route, RemoteRoute.decode(route.encode()))
        }
    }

    @Test fun decodeRejectsMalformedText() {
        for (text in listOf("", "host", "host/", "session/a", "session//b", "session/a/b/c", "other/x", "bot", "bot/a", "bot//b", "bot/a/", "bot/a/b/c")) {
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

    @Test fun aBotPageKeepsAnHonestBackStack() {
        val target = RemoteRoute.Bot(host, bot)
        assertEquals(listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), target), RemoteRoute.stackFor(target))
    }

    @Test fun backFromAChatOpenedFromItsBotReturnsToTheBotOnce() {
        val stack = listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Bot(host, bot), RemoteRoute.Session(host, session))
        // The Bot is already beneath: back leads there, and is not pushed a second time.
        assertEquals(stack.dropLast(1), RemoteRoute.afterBack(stack) { _, _ -> bot })
        assertEquals(stack.dropLast(1), RemoteRoute.afterBack(stack) { _, _ -> null })
    }

    @Test fun backFromAChatOpenedByALinkGoesToItsBotWhenTheConnectionKnowsWhich() {
        val stack = RemoteRoute.stackFor(RemoteRoute.Session(host, session))
        val asked = mutableListOf<Pair<String, String>>()
        val after = RemoteRoute.afterBack(stack) { hostId, sessionId -> asked += hostId to sessionId; bot }
        assertEquals(listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Bot(host, bot)), after)
        assertEquals(listOf(host to session), asked)
        // From the Bot, back is the computer, as for any page.
        assertEquals(listOf(RemoteRoute.Hosts, RemoteRoute.Host(host)), RemoteRoute.afterBack(after) { _, _ -> bot })
    }

    @Test fun backFromASessionThatIsNoBotsChatBehavesAsItAlwaysDid() {
        val stack = RemoteRoute.stackFor(RemoteRoute.Session(host, session))
        assertEquals(listOf(RemoteRoute.Hosts, RemoteRoute.Host(host)), RemoteRoute.afterBack(stack) { _, _ -> null })
    }

    @Test fun backFromAnythingButASessionNeverAsksForABot() {
        var asked = false
        val lookup = { _: String, _: String -> asked = true; bot }
        val stack = listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Bot(host, bot))
        assertEquals(stack.dropLast(1), RemoteRoute.afterBack(stack, lookup))
        assertEquals(listOf(RemoteRoute.Hosts), RemoteRoute.afterBack(stack.dropLast(1), lookup))
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.afterBack(listOf(RemoteRoute.Hosts), lookup))
        assertTrue(!asked)
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
