package com.lingxi.code.friends

import com.lingxi.code.teams.TeamLastMessage
import com.lingxi.code.teams.TeamMessageConversation
import com.lingxi.code.teams.TeamsListLogic
import com.lingxi.code.teams.TeamsListStrings
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * How a Firestore card, a block entry and the HTTP results become the app's types (docs/FRIENDS-CLIENTS.md D2, D12, D13, appendix A).
 * The `Timestamp` of a real document is stood in for by a number of epoch milliseconds. iOS runs the same cases in `FriendsModelsTests`.
 */
class FriendsModelsTest {
    private val millis: (Any) -> Long? = { (it as? Number)?.toLong() }

    private fun pointer(seq: Any, at: Any? = 1_762_000_009_123L, preview: String = "hi", author: String = "u2"): MutableMap<String, Any?> =
        mutableMapOf("id" to "m$seq", "seq" to seq, "at" to at, "author" to mapOf("kind" to "user", "uid" to author), "preview" to preview)

    private fun friend(vararg extra: Pair<String, Any?>): Map<String, Any?> =
        mapOf("state" to "friends", "displayName" to "Ann", "handle" to "ann", "teamId" to "f_abc", "generation" to 1L, "since" to 1_762_000_000_000L) + extra

    private fun parse(data: Map<String, Any?>, uid: String = "u2") = FriendCard.fromDocument(uid, data, millis)

    // --- cards ----------------------------------------------------------------------------------------------

    @Test fun aFriendCardCarriesTheChatSummary() {
        val card = checkNotNull(parse(friend("lastSeq" to 7L, "lastReadSeq" to 4L, "lastChangeSeq" to 9L, "lastMessage" to pointer(7L))))
        assertEquals(FriendState.Friends, card.state)
        assertEquals("Ann", card.displayName)
        assertEquals("ann", card.handle)
        assertEquals(PairRef("f_abc", 1), card.pair)
        assertEquals("whole epoch milliseconds, beyond 32 bits", 1_762_000_000_000L, card.sinceMs)
        assertEquals(3, FriendsLogic.unreadOf(card))
        assertEquals(1_762_000_009_123L, card.lastAtMs)
        assertEquals("hi", card.lastMessage?.preview)
        assertEquals("u2", card.lastMessage?.authorUid)
    }

    @Test fun aNewFriendshipHasNoSummary() {
        val card = checkNotNull(parse(friend()))
        assertEquals(0, FriendsLogic.unreadOf(card))
        assertNull(card.lastMessage)
        assertNull(card.lastAtMs)
        assertNull("an explicit null is no message", checkNotNull(parse(friend("lastMessage" to null))).lastMessage)
    }

    @Test fun aPointerThatIsNotWholeCountsAsNoMessageAndNeverDropsTheCard() {
        val withoutPreview = pointer(3L).also { it.remove("preview") }
        for (bad in listOf<Any?>(withoutPreview, "text", 5L, pointer(3L, at = "yesterday"), pointer(1.5), pointer(3L).also { it["author"] = "u2" })) {
            val card = checkNotNull(parse(friend("lastSeq" to 3L, "lastMessage" to bad))) { "$bad" }
            assertNull("$bad", card.lastMessage)
            assertEquals("the counters still count", 3, FriendsLogic.unreadOf(card))
        }
        val unstamped = checkNotNull(parse(friend("lastMessage" to pointer(1L, at = null))))
        assertNotNull(unstamped.lastMessage)
        assertNull("a pointer without a readable time counts as none for the order", unstamped.lastAtMs)
    }

    @Test fun aRecalledNewestMessageKeepsItsFlag() {
        val recalled = pointer(2L, preview = "").also { it["recalled"] = true }
        val card = checkNotNull(parse(friend("lastSeq" to 2L, "lastMessage" to recalled)))
        assertEquals(true, card.lastMessage?.recalled)
        assertEquals("", card.lastMessage?.preview)
    }

    @Test fun requestsKeepTheirNoteAndNeedNoTeam() {
        val incoming = checkNotNull(parse(mapOf("state" to "incoming", "displayName" to "Bo", "note" to "from the book club", "since" to 5L), "u3"))
        assertEquals(FriendState.Incoming, incoming.state)
        assertEquals("from the book club", incoming.note)
        assertNull(incoming.pair)
        assertEquals(0, FriendsLogic.unreadOf(incoming))
        val outgoing = checkNotNull(parse(mapOf("state" to "outgoing", "displayName" to "Cy", "handle" to ""), "u4"))
        assertEquals(FriendState.Outgoing, outgoing.state)
        assertNull("an empty handle is none", outgoing.handle)
        assertEquals(0, outgoing.sinceMs)
    }

    @Test fun oneBadCardIsDroppedAndTheRestSurvive() {
        assertNull(parse(mapOf("state" to "blocked", "displayName" to "X"), "u"))
        assertNull(parse(mapOf("displayName" to "X"), "u"))
        assertNull(parse(friend(), ""))
        assertNull("a friend without its pair team cannot be opened", parse(friend() - "teamId", "u"))
        assertNull(parse(friend("teamId" to ""), "u"))
        assertNull(parse(friend("generation" to "1"), "u"))
        assertNull(parse(friend() - "generation", "u"))
    }

    @Test fun countersThatAreNotWholeNumbersCountAsZero() {
        val card = checkNotNull(parse(friend("lastSeq" to "9", "lastReadSeq" to -4L)))
        assertEquals(0, card.lastSeq)
        assertEquals(0, card.lastReadSeq)
    }

    // --- team offers (D27) ----------------------------------------------------------------------------------

    private fun offerDocument(vararg extra: Pair<String, Any?>): Map<String, Any?> =
        mapOf("teamId" to "t1", "teamName" to "Book club", "fromUid" to "u2", "fromName" to "Ann", "role" to "member", "expiresAt" to 1_762_604_800_000L) + extra

    @Test fun anOfferDocumentBecomesAnOfferAndTheDocumentIdIsTheTeamId() {
        assertEquals(TeamOffer("t1", "Book club", "u2", "Ann", 1_762_604_800_000L), TeamOffer.fromDocument("t1", offerDocument(), millis))
        assertEquals("t9", TeamOffer.fromDocument("t9", offerDocument(), millis)?.teamId)
        assertEquals("the role is always member and is not kept", TeamOffer("t1", "Book club", "u2", "Ann", 5L), TeamOffer.fromDocument("t1", offerDocument("role" to "admin", "expiresAt" to 5L), millis))
    }

    @Test fun anOfferWithoutItsFourStringsAndATimeIsSkippedNeverFailingTheList() {
        for (key in listOf("teamId", "teamName", "fromUid", "fromName", "expiresAt")) assertNull("$key missing", TeamOffer.fromDocument("t1", offerDocument() - key, millis))
        for (key in listOf("teamId", "teamName", "fromUid", "fromName")) assertNull("$key as a number", TeamOffer.fromDocument("t1", offerDocument(key to 5L), millis))
        assertNull("an explicit null time", TeamOffer.fromDocument("t1", offerDocument("expiresAt" to null), millis))
        assertNull("a time that is not readable", TeamOffer.fromDocument("t1", offerDocument("expiresAt" to "soon"), millis))
        assertNull("no document id", TeamOffer.fromDocument("", offerDocument(), millis))
        assertNotNull(TeamOffer.fromDocument("t1", offerDocument("extra" to "ignored"), millis))
    }

    @Test fun theNamesOfAnOfferAreKeptAsPlainText() {
        val offer = checkNotNull(TeamOffer.fromDocument("t1", offerDocument("teamName" to "**<b>Club</b>** [x](https://e.example)", "fromName" to "  Ann  "), millis))
        assertEquals("**<b>Club</b>** [x](https://e.example)", offer.teamName)
        assertEquals("  Ann  ", offer.fromName)
    }

    // --- blocks ---------------------------------------------------------------------------------------------

    @Test fun aBlockEntryKeepsThePersonsName() {
        val entry = BlockEntry.fromDocument("u5", mapOf("at" to 42L, "displayName" to "  Dee ", "handle" to "dee"), millis)
        assertEquals(BlockEntry("u5", 42, "Dee", "dee"), entry)
        val bare = BlockEntry.fromDocument("u6", emptyMap(), millis)
        assertEquals("the screen shows Member", "", bare.displayName)
        assertNull(bare.handle)
        assertEquals("", BlockEntry.fromDocument("u7", mapOf("displayName" to "   "), millis).displayName)
    }

    // --- HTTP results ---------------------------------------------------------------------------------------

    @Test fun theProfileParsesWithAndWithoutAFriendLink() {
        val on = FriendProfile.parse(JSONObject(
            """{"uid":"u1","displayName":"Me","handle":"me_1","findByHandle":true,"findByEmail":false,"friendCode":{"code":"abc","link":"https://lingxios.net/open/friend/add?code=abc"}}""",
        ))
        assertEquals("me_1", on.handle)
        assertFalse(on.findByEmail)
        assertEquals(FriendCode("abc", "https://lingxios.net/open/friend/add?code=abc"), on.friendCode)
        val off = FriendProfile.parse(JSONObject("""{"uid":"u1","displayName":"Member","findByHandle":true,"findByEmail":true,"friendCode":null}"""))
        assertNull(off.friendCode)
        assertNull(off.handle)
        assertEquals(FriendsLogic.FALLBACK_NAME, off.displayName)
        assertTrue(FriendsLogic.needsName(off))
        assertNull(FriendProfile.parse(JSONObject("""{"uid":"u1","displayName":"A","findByHandle":true,"findByEmail":true}""")).friendCode)
    }

    @Test fun aLookupNamesTheRelation() {
        for (relation in listOf("none", "friends", "incoming", "outgoing")) {
            val result = LookupResult.parse(JSONObject("""{"uid":"u2","displayName":"Ann","relation":"$relation"}"""))
            assertEquals(relation, result.relation.wire)
            assertNull(result.handle)
        }
        assertEquals("ann", LookupResult.parse(JSONObject("""{"uid":"u2","displayName":"Ann","handle":"ann","relation":"none"}""")).handle)
        assertThrows(Exception::class.java) { LookupResult.parse(JSONObject("""{"uid":"u2","displayName":"Ann","relation":"blocked"}""")) }
        assertThrows(Exception::class.java) { LookupResult.parse(JSONObject("""{"uid":"u2","displayName":"Ann"}""")) }
    }

    // --- the preview a row prints -----------------------------------------------------------------------------

    /** A Friends row prints the Teams text: the pointer alone gives the same line as the conversation it came from. */
    @Test fun aFriendRowPreviewIsTheTeamsPreview() {
        val strings = TeamsListStrings.ENGLISH
        assertEquals(strings.noMessages, TeamsListLogic.pointerPreview(null, "u1", strings))
        data class Case(val preview: String, val author: String, val recalled: Boolean, val expect: String)
        val cases = listOf(
            Case("hello", "u2", false, "hello"), Case("hello", "u1", false, "You: hello"), Case("", "u2", false, strings.nonText),
            Case("", "u1", true, "You: " + strings.recalled), Case("", "u2", true, strings.recalled),
        )
        for (case in cases) {
            val last = TeamLastMessage("m", 1, 1, "user", case.author, null, case.preview, case.recalled)
            assertEquals(case.expect, TeamsListLogic.pointerPreview(last, "u1", strings))
            val dm = TeamMessageConversation("friend_chat", "dm", "", listOf("user:u1", "user:u2"), 1, 1, 0, emptyList(), last)
            assertEquals(case.expect, TeamsListLogic.conversationPreview(dm, "u1", strings) { "someone" })
        }
    }
}
