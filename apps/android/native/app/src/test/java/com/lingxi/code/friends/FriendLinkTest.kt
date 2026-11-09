package com.lingxi.code.friends

import com.lingxi.code.navigation.DeepLink
import com.lingxi.code.push.PushRoute
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The Friends links and push routes of docs/FRIENDS-CLIENTS.md D16, against the vectors every client
 * loads (`packages/teams/fixtures/friends-vectors.json`, `links.typed` and `links.push`).
 */
class FriendLinkTest {
    private val vectors: JSONObject = run {
        val root = generateSequence(File(checkNotNull(System.getProperty("user.dir")))) { it.parentFile }
            .map { File(it, "packages/teams/fixtures/friends-vectors.json") }.first { it.isFile }
        JSONObject(root.readText())
    }

    private fun rows(name: String) = vectors.getJSONObject("links").getJSONArray(name).let { array -> (0 until array.length()).map { array.getJSONObject(it) } }

    private fun FriendLink.describe(): Triple<String, String?, String?> = when (this) {
        is FriendLink.Add -> Triple("add", code, null)
        FriendLink.Requests -> Triple("requests", null, null)
        is FriendLink.Chat -> Triple("chat", null, uid)
    }

    @Test fun typedLinksFollowTheSharedVectors() {
        val typed = rows("typed")
        assertTrue("vectors not loaded", typed.size >= 25)
        for (row in typed) {
            val url = row.getString("url")
            val link = FriendLink.parse(url)
            if (row.isNull("expect")) { assertNull(url, link); continue }
            val expect = row.getJSONObject("expect")
            assertNotNull(url, link)
            val (kind, code, uid) = link!!.describe()
            assertEquals(url, expect.getString("kind"), kind)
            assertEquals(url, expect.optString("code").ifEmpty { null }, code)
            assertEquals(url, expect.optString("uid").ifEmpty { null }, uid)
        }
    }

    @Test fun pushRoutesFollowTheSharedVectors() {
        val push = rows("push")
        assertTrue("vectors not loaded", push.size >= 20)
        for (row in push) {
            val route = row.getString("route")
            val expected = if (row.isNull("expect")) null else row.getString("expect")
            assertEquals(route, expected, PushRoute.link(route))
        }
    }

    @Test fun everyAcceptedPushRouteIsAFriendOrTeamLinkTheInboxVets() {
        val link = PushRoute.link("friend/chat?uid=Hk3xQ9vR2mT7wL1pZ8cN4aB0cD2e")!!
        assertEquals(FriendLink.Chat("Hk3xQ9vR2mT7wL1pZ8cN4aB0cD2e"), FriendLink.from(DeepLink.parse(link)))
        assertEquals(FriendLink.Requests, FriendLink.parse(PushRoute.link("friend/requests")))
        assertNull(FriendLink.parse(PushRoute.link("team/inbox")))
    }

    @Test fun existingPushRoutesStayAccepted() {
        assertEquals("lingxi://team/inbox", PushRoute.link("team/inbox"))
        assertEquals("lingxi://remote/host_1/session_1", PushRoute.link("remote/host_1/session_1"))
        assertNull(PushRoute.link(null))
        assertNull(PushRoute.link("settings/account"))
    }

    @Test fun friendRoutesAreNotConfusedWithOtherRoutes() {
        val code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-"
        assertNull(DeepLink.parse("lingxi://session/abc?mode=chat&uid=x"))
        assertNull(DeepLink.parse("lingxi://new?uid=x"))
        assertNull(DeepLink.parse("lingxi://team/join?code=$code&uid=x"))
        assertEquals("friend/add", DeepLink.parse("lingxi://friend/add?code=$code")?.route)
        assertEquals(FriendLink.Add(code), FriendLink.parse("https://lingxios.net/open/friend/add?code=$code"))
        assertNull(FriendLink.parse(null))
    }
}
