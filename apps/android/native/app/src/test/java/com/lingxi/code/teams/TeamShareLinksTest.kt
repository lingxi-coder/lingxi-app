package com.lingxi.code.teams

import com.lingxi.code.navigation.DeepLink
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TeamShareLinksTest {
    private fun conversation(id: String, kind: String, vararg participants: String) =
        TeamMessageConversation(id, kind, "", participants.toList(), 0, 0, 0)
    private fun card(id: String) = TeamBotCard.parse(JSONObject("""{"sharedBotId":"$id","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"""))

    @Test fun linksMatchTheWebAndDesktopOnes() {
        assertEquals("https://lingxios.net/open/team/chat?conversation=c-1&team=design-team",
            TeamShareLinks.conversation("design-team", conversation("c-1", "group", "user:a", "user:b")))
        assertEquals("https://lingxios.net/open/team/bot?bot=helper_01&team=design-team",
            TeamShareLinks.conversation("design-team", conversation("dm", "bot_dm", "user:a", "shared_bot:helper_01")))
        assertNull(TeamShareLinks.conversation("design-team", conversation("dm", "bot_dm", "user:a")))
        assertNull(TeamShareLinks.conversation("design team", conversation("c", "group")))
        assertEquals("https://lingxios.net/open/team/chat?conversation=friend_chat&team=f_pair", TeamShareLinks.friendChat("f_pair"))
    }

    @Test fun requestsNameTheirTeamAndAFriendshipGoesToFriends() {
        assertEquals(TeamShareLinks.Request.Conversation("design", "c1"), TeamShareLinks.request(DeepLink.parse("lingxi://team/chat?team=design&conversation=c1")))
        assertEquals(TeamShareLinks.Request.Bot("design", "helper"), TeamShareLinks.request(DeepLink.parse("lingxi://team/bot?team=design&bot=helper")))
        assertEquals(TeamShareLinks.Request.Pair("f_pair"), TeamShareLinks.request(DeepLink.parse("lingxi://team/chat?team=f_pair&conversation=friend_chat")))
        assertNull(TeamShareLinks.request(DeepLink.parse("lingxi://team/inbox")))
    }

    @Test fun aLinkOpensItsOwnTeamOnlyAndNeverCreatesABotChat() {
        val chat = TeamShareLinks.Request.Conversation("design", "c1")
        val bot = TeamShareLinks.Request.Bot("design", "helper")
        val teams = listOf("design", "other")
        val none = TeamMessagingState()
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(chat, null, true, teams, "other", none))
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(chat, "me", false, teams, "other", none))
        assertEquals(TeamShareLinks.Step.Unavailable, TeamShareLinks.step(chat, "me", true, listOf("other"), "other", none))
        assertEquals(TeamShareLinks.Step.SelectTeam, TeamShareLinks.step(chat, "me", true, teams, "other", none))
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(chat, "me", true, teams, "design", none))
        val mine = conversation("dm1", "bot_dm", "user:me", "shared_bot:helper")
        val theirs = conversation("dm2", "bot_dm", "user:them", "shared_bot:helper")
        val cached = TeamMessagingState(cache = TeamMessageCache("me", "design", 1, listOf(theirs, conversation("c1", "group"))))
        // Only the server's directory decides: a cached one, or none yet, waits.
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(chat, "me", true, teams, "design", cached))
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(chat, "me", true, teams, "design", cached.copy(directoryReady = true)))
        val open = cached.copy(directoryReady = true, directoryCached = false)
        assertEquals(TeamShareLinks.Step.Open("c1"), TeamShareLinks.step(chat, "me", true, teams, "design", open))
        assertEquals("an unseen conversation says so", TeamShareLinks.Step.Unavailable, TeamShareLinks.step(TeamShareLinks.Request.Conversation("design", "unseen"), "me", true, teams, "design", open))
        // A Bot list that loads before the directory does not decide "no chat yet" on an empty one.
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(bot, "me", true, teams, "design", cached.copy(botsLoaded = true, bots = listOf(card("helper")))))
        assertEquals(TeamShareLinks.Step.Wait, TeamShareLinks.step(bot, "me", true, teams, "design", open))
        assertEquals(TeamShareLinks.Step.Unavailable, TeamShareLinks.step(bot, "me", true, teams, "design", open.copy(botsLoaded = true)))
        val ready = open.copy(botsLoaded = true, bots = listOf(card("helper")))
        assertEquals(TeamShareLinks.Step.Bots, TeamShareLinks.step(bot, "me", true, teams, "design", ready))
        assertEquals(TeamShareLinks.Step.Open("dm1"), TeamShareLinks.step(bot, "me", true, teams, "design", ready.copy(cache = TeamMessageCache("me", "design", 1, listOf(theirs, mine)))))
    }
}
