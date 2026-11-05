package com.lingxi.code.remote

import androidx.compose.runtime.saveable.SaverScope
import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.push.PushRoute
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteRouteTest {
    private val host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private val session = "11111111-2222-4333-8444-555555555555"

    /** A Bot id as the desktop makes them: 22 characters of base64url, so `-` and `_` occur. */
    private val bot = "A-_b1234567890123456Zz"

    @Test fun encodeAndDecodeRoundTripEveryRoute() {
        for (route in listOf(RemoteRoute.Hosts, RemoteRoute.Host(host), RemoteRoute.Session(host, session))) {
            assertEquals(route, RemoteRoute.decode(route.encode()))
        }
    }

    @Test fun decodeRejectsMalformedText() {
        for (text in listOf("", "host", "host/", "host/a/b", "session/a", "session//b", "session/a/", "session/a/b/c", "other/x", "hosts/x")) {
            assertNull(text, RemoteRoute.decode(text))
        }
    }

    @Test fun theRetiredBotPageIsNotReadBack() {
        // Nothing older is decoded: the Bot page no longer exists, so its saved text names nothing.
        for (text in listOf("bot", "bot/a", "bot//b", "bot/a/", "bot/a/b", "bot/$host/$bot", "bot/a/b/c")) {
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
        assertNull(RemoteRoute.fromLink(DeepLink.parse("lingxi://session/abc?mode=code")!!))
    }

    @Test fun malformedRemoteLinksDoNotParse() {
        for (raw in listOf(
            "lingxi://remote/host-only",
            "lingxi://remote/host/session/extra",
            "lingxi://remote/host/session#payload=%7B%22prompt%22%3A%22x%22%7D",
        )) assertNull(raw, DeepLink.parse(raw))
    }

    @Test fun theTreeIsTheRootAndNeverAStackEntry() {
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.stackFor(RemoteRoute.Hosts))
    }

    @Test fun directOpenKeepsAnHonestBackStack() {
        // Back from a page a link or a notification opened lands on the tree, which is the root beneath the stack.
        val target = RemoteRoute.Session(host, session)
        assertEquals(listOf(target), RemoteRoute.stackFor(target))
        assertEquals(listOf(RemoteRoute.Host(host)), RemoteRoute.stackFor(RemoteRoute.Host(host)))
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.afterBack(RemoteRoute.stackFor(target)))
    }

    @Test fun backPopsOnePageAtATime() {
        val stack = listOf(RemoteRoute.Host(host), RemoteRoute.Session(host, session))
        assertEquals(listOf(RemoteRoute.Host(host)), RemoteRoute.afterBack(stack))
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.afterBack(RemoteRoute.afterBack(stack)))
        // Back on the tree itself has nothing to pop.
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.afterBack(emptyList()))
    }

    private fun save(stack: List<RemoteRoute>): Any {
        val saver = RemoteRoute.stackSaver
        val saved = with(saver) { SaverScope { true }.save(stack) }
        return checkNotNull(saved) { "the stack was not saved" }
    }

    @Test fun theStackSurvivesBeingSavedAndRestored() {
        val stack = listOf(RemoteRoute.Host(host), RemoteRoute.Session(host, session))
        assertEquals(stack, RemoteRoute.stackSaver.restore(save(stack)))
        assertEquals(listOf(RemoteRoute.Session(host, session)), RemoteRoute.stackSaver.restore(save(listOf(RemoteRoute.Session(host, session)))))
    }

    @Test fun theTreeAloneSavesNothingAndRestoresAsTheTree() {
        // An empty stack is the root: nothing is stored for it, and a missing state starts there.
        val saved = with(RemoteRoute.stackSaver) { SaverScope { true }.save(emptyList()) }
        assertNull(saved)
    }

    @Test fun anOwnedStackRestoresOnlyForTheAccountThatSavedIt() {
        val stack = listOf(RemoteRoute.Host(host), RemoteRoute.Session(host, session))
        val saved = with(RemoteRoute.ownedStackSaver("user-a")) { SaverScope { true }.save(stack) }!!
        assertEquals(stack, RemoteRoute.ownedStackSaver("user-a").restore(saved))
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.ownedStackSaver("user-b").restore(saved))
        assertEquals(emptyList<RemoteRoute>(), RemoteRoute.ownedStackSaver(null).restore(saved))
    }

    @Test fun anUnownedOrMalformedSavedStackStartsAtTheTree() {
        val saver = RemoteRoute.ownedStackSaver("user-a")
        for (saved in listOf(listOf("session/$host/$session"), listOf("uid/user-a", "hosts"),
            listOf("uid/user-a", "host/$host", "nonsense"))) {
            assertEquals(emptyList<RemoteRoute>(), saver.restore(saved))
        }
    }

    @Test fun signedOutAndEmptyOwnedStacksSaveNothing() {
        assertNull(with(RemoteRoute.ownedStackSaver(null)) { SaverScope { true }.save(listOf(RemoteRoute.Host(host))) })
        assertNull(with(RemoteRoute.ownedStackSaver("user-a")) { SaverScope { true }.save(emptyList()) })
    }

    @Test fun aSavedStackThatDoesNotReadBackWholeStartsAtTheTree() {
        val saver = RemoteRoute.stackSaver
        // The root is never saved, and the Bot page is gone: such a state starts at the tree, not at half a stack.
        for (saved in listOf(
            listOf("hosts", "host/$host"),
            listOf("host/$host", "bot/$host/$bot"),
            listOf("bot/$host/$bot"),
            listOf("host/$host", "nonsense"),
        )) {
            val restored = saver.restore(saved)
            assertNotNull(saved.toString(), restored)
            assertEquals(saved.toString(), emptyList<RemoteRoute>(), restored)
        }
    }

    @Test fun pushRouteBecomesTheSameLinkTheInboxVets() {
        val link = PushRoute.link("remote/$host/$session")!!
        assertEquals("lingxi://remote/$host/$session", link)
        assertEquals(RemoteRoute.Session(host, session), RemoteRoute.fromLink(DeepLink.parse(link)!!))
    }

    @Test fun pushRoutePreservesTeamApprovalCapability() {
        val capability = "team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef"
        val link = PushRoute.link("remote/$host/$capability")!!
        val route = RemoteRoute.fromLink(DeepLink.parse(link)!!)!!
        assertEquals(RemoteRoute.Session(host, capability), route)
        assertEquals(route, RemoteRoute.decode(route.encode()))
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

    @Test fun everyRouteEncodesWithoutTheSeparatorInItsIds() {
        // Ids never contain `/`; the text of a route therefore splits back into exactly its parts.
        assertTrue(RemoteRoute.Session(host, session).encode().split('/').size == 3)
        assertTrue(RemoteRoute.Host(host).encode().split('/').size == 2)
    }
}
