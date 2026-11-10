package com.lingxi.code.friends

import com.lingxi.code.tabs.TabLogic
import com.lingxi.code.teams.TeamLastMessage
import com.lingxi.code.teams.TeamMember
import com.lingxi.code.teams.TeamMessageFailure
import com.lingxi.code.teams.TeamSummary
import com.lingxi.code.teams.stringOrNull
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Locale
import java.util.UUID

/**
 * The Friends rules of docs/FRIENDS-CLIENTS.md (D5 order and filter, D6, D7, D11, D13, D14, D18) against the shared vectors every client
 * loads (`packages/teams/fixtures/friends-vectors.json`). iOS runs the same rows in `FriendsLogicTests`, Web and Desktop in `friends.test.ts`.
 */
class FriendsLogicTest {
    private fun JSONObject.stringMap(): Map<String, String> = keys().asSequence().associateWith { getString(it) }

    // --- limits, request ids, handles, lengths -------------------------------------------------------------

    @Test fun limitsAreTheVectorLimits() {
        val limits = FriendsVectors.obj("limits")
        assertEquals(limits.getInt("sectionRows"), FriendsLogic.SECTION_ROWS)
        assertEquals(limits.getInt("displayNameMax"), FriendsLogic.DISPLAY_NAME_MAX)
        assertEquals(limits.getInt("noteMax"), FriendsLogic.NOTE_MAX)
        assertEquals(limits.getInt("codeLength"), FriendsLogic.CODE_LENGTH)
        assertEquals(limits.getString("chatConversationId"), FriendsLogic.CHAT_CONVERSATION_ID)
        assertEquals(limits.getInt("badgeCap"), TabLogic.BADGE_CAP)
    }

    @Test fun requestIdsFollowThePattern() {
        val section = FriendsVectors.obj("requestId")
        val valid = FriendsVectors.strings(section.getJSONArray("valid"))
        val invalid = FriendsVectors.strings(section.getJSONArray("invalid"))
        assertTrue(valid.isNotEmpty() && invalid.isNotEmpty())
        valid.forEach { assertTrue(it, FriendsLogic.isRequestId(it)) }
        invalid.forEach { assertFalse("'$it'", FriendsLogic.isRequestId(it)) }
        assertTrue("a UUID is fine", FriendsLogic.isRequestId(UUID.randomUUID().toString()))
        repeat(20) { assertTrue(FriendsLogic.isRequestId(FriendsLogic.newRequestId())) }
    }

    @Test fun handlesFollowThePattern() {
        val section = FriendsVectors.obj("handle")
        val valid = FriendsVectors.strings(section.getJSONArray("valid"))
        val invalid = FriendsVectors.strings(section.getJSONArray("invalid"))
        assertTrue(valid.isNotEmpty() && invalid.isNotEmpty())
        valid.forEach { assertTrue(it, FriendsLogic.validHandle(it)) }
        invalid.forEach { assertFalse("'$it'", FriendsLogic.validHandle(it)) }
    }

    @Test fun lengthsCountUtf16Units() {
        val rows = FriendsVectors.rows("lengthRule")
        assertEquals(9, rows.size)
        for (row in rows) {
            val field = row.getString("field")
            val text = row.getString("unit").repeat(row.getInt("count"))
            val valid = if (field == "displayName") FriendsLogic.validDisplayName(text) else FriendsLogic.noteFits(text)
            assertEquals("$field x${row.getInt("count")}", row.getBoolean("valid"), valid)
        }
    }

    @Test fun jsTrimTrimsTheWhitespaceOfJavaScript() {
        assertEquals("a b", FriendsLogic.jsTrim("  a b\t\n"))
        assertEquals("no-break, ideographic, em space and the byte order mark are whitespace", "a", FriendsLogic.jsTrim("\u00a0\u3000a\u2003\ufeff"))
        assertEquals("", FriendsLogic.jsTrim(" \u2028\u2029 "))
        assertEquals("control separators are not JavaScript whitespace", "\u001ca\u001c", FriendsLogic.jsTrim("\u001ca\u001c"))
        assertFalse("a name of byte order marks is blank", FriendsLogic.validDisplayName("\ufeff\ufeff"))
        assertTrue(FriendsLogic.validDisplayName("\u00a0Ann\u00a0"))
    }


    // --- D5 order ---------------------------------------------------------------------------------------------

    private fun cards(rows: List<JSONObject>, state: String? = null) = rows.map { FriendsVectors.card(it, state) }

    @Test fun friendsAreOrderedByActivityThenNameThenUid() {
        val vector = FriendsVectors.obj("sort", "friends")
        val input = cards(FriendsVectors.rows("sort", "friends", "input").also { assertEquals(18, it.size) }, "friends")
        for (locale in FriendsVectors.locales) {
            val ordered = input.sortedWith(FriendsLogic.compareFriends(FriendsLogic.friendCollator(locale))).map { it.uid }
            assertEquals(locale.toLanguageTag(), FriendsVectors.strings(vector.getJSONArray("expect")), ordered)
        }
    }

    @Test fun theWholeCollectionIsIncomingThenOutgoingThenFriends() {
        val vector = FriendsVectors.obj("sort", "cards")
        val input = cards(FriendsVectors.rows("sort", "cards", "input"))
        for (locale in FriendsVectors.locales) {
            val ordered = input.sortedWith(FriendsLogic.compareCards(FriendsLogic.friendCollator(locale)))
            assertEquals(locale.toLanguageTag(), FriendsVectors.strings(vector.getJSONArray("expect")), ordered.map { it.uid })
        }
        val ordered = input.sortedWith(FriendsLogic.compareCards(FriendsLogic.friendCollator(Locale.ENGLISH)))
        assertEquals(listOf("i2", "i3", "i1"), ordered.filter { it.state == FriendState.Incoming }.map { it.uid })
        assertEquals(listOf("o2", "o1"), ordered.filter { it.state == FriendState.Outgoing }.map { it.uid })
        assertEquals(listOf("f1", "f4", "f2", "f3"), ordered.filter { it.state == FriendState.Friends }.map { it.uid })
    }

    @Test fun requestsAreIncomingFirstThenNewestThenUid() {
        val vector = FriendsVectors.obj("sort", "requests")
        val input = cards(FriendsVectors.rows("sort", "requests", "input"))
        assertEquals(FriendsVectors.strings(vector.getJSONArray("expect")), input.sortedWith(FriendsLogic.compareRequests).map { it.uid })
    }

    @Test fun namesTieAtPrimaryStrengthSoTheUidDecides() {
        val collator = FriendsLogic.friendCollator(Locale.ENGLISH)
        assertEquals(0, collator.compare("Sam", "sam"))
        assertEquals(0, collator.compare("Alice", "alice"))
        assertTrue(collator.compare("adam", "Al") < 0)
        assertTrue(collator.compare("Zed", "bob") > 0)
        // "n10" before "n9": uids compare as text, not numerically.
        val same = listOf("n9", "n10", "n2").map { FriendsVectors.card(JSONObject().put("uid", it).put("displayName", "Sam"), "friends") }
        assertEquals(listOf("n10", "n2", "n9"), same.sortedWith(FriendsLogic.compareFriends(collator)).map { it.uid })
    }

    @Test fun uidsCompareByCodePointNotByUtf16Unit() {
        // U+FF5E is one UTF-16 unit above the surrogate range; U+1F600 is two units below it. By code point the emoji is last.
        val collator = FriendsLogic.friendCollator(Locale.ENGLISH)
        val cards = listOf("\ud83d\ude00", "\uff5e", "a").map { FriendsVectors.card(JSONObject().put("uid", it).put("displayName", "Sam"), "friends") }
        assertEquals(listOf("a", "\uff5e", "\ud83d\ude00"), cards.sortedWith(FriendsLogic.compareFriends(collator)).map { it.uid })
    }

    @Test fun aMessageWithoutATimeCountsAsNone() {
        val collator = FriendsLogic.friendCollator(Locale.ENGLISH)
        val none = FriendsVectors.card(JSONObject().put("uid", "a").put("displayName", "Zed"), "friends")
        val some = FriendsVectors.card(JSONObject().put("uid", "b").put("displayName", "Amy").put("lastAtMs", 5), "friends")
        val unstamped = FriendsVectors.card(JSONObject().put("uid", "c").put("displayName", "Bea"), "friends")
            .copy(lastMessage = TeamLastMessage("m", 1, null, "user", "x", null, "hi"))
        assertNull(unstamped.lastAtMs)
        assertEquals(listOf("b", "c", "a"), listOf(none, unstamped, some).sortedWith(FriendsLogic.compareFriends(collator)).map { it.uid })
    }

    @Test fun blocksAreNewestFirst() {
        val blocks = listOf(BlockEntry("b", 10, "", null), BlockEntry("a", 10, "", null), BlockEntry("c", 20, "", null))
        assertEquals(listOf("c", "a", "b"), blocks.sortedWith(FriendsLogic.compareBlocks).map { it.uid })
    }

    // --- D5 filter --------------------------------------------------------------------------------------------

    @Test fun filterFollowsTheVectors() {
        val friends = cards(FriendsVectors.rows("filter", "friends"), "friends")
        val cases = FriendsVectors.rows("filter", "cases")
        assertEquals(12, cases.size)
        for (case in cases) {
            val query = case.getString("query")
            assertEquals("'$query': ${case.optString("note")}", FriendsVectors.strings(case.getJSONArray("expect")), friends.filter { FriendsLogic.matchesFilter(it, query) }.map { it.uid })
        }
    }

    @Test fun foldRemovesCaseAccentsAndCompatibilityForms() {
        assertEquals("emile", FriendsLogic.foldName("\u00c9MILE"))
        assertEquals("zoe", FriendsLogic.foldName("Zo\u00eb"))
        assertEquals("abc", FriendsLogic.foldName("\uff21\uff22\uff23"))
        assertEquals("\u674e\u96f7", FriendsLogic.foldName("\u674e\u96f7"))
    }

    // --- D13, D14 ---------------------------------------------------------------------------------------------

    @Test fun unreadIsTheGapBetweenTheCounters() {
        val rows = FriendsVectors.rows("unread")
        assertEquals(8, rows.size)
        for (row in rows) {
            val data = mutableMapOf<String, Any?>("state" to "friends", "teamId" to "f_1", "generation" to 1L, "displayName" to "A")
            val card = row.getJSONObject("card")
            card.keys().forEach { data[it] = card.getLong(it) }
            val parsed = checkNotNull(FriendCard.fromDocument("u", data) { null })
            assertEquals(row.optString("note"), row.getInt("expect"), FriendsLogic.unreadOf(parsed))
        }
    }


    @Test fun badgeCountsRequestsAndUnread() {
        val rows = FriendsVectors.rows("badge")
        assertEquals(10, rows.size)
        for (row in rows) {
            val note = row.optString("note")
            val cards = (0 until row.getJSONArray("cards").length()).map { index -> FriendsVectors.card(row.getJSONArray("cards").getJSONObject(index).put("uid", "u$index")) }
            val count = FriendsLogic.badgeCount(cards)
            assertEquals(note, row.getInt("expectCount"), count)
            assertEquals(note, row.stringOrNull("expectText"), FriendsLogic.badgeText(count))
        }
    }


    @Test fun theOpenChatCountsAsRead() {
        val cards = listOf(
            JSONObject().put("uid", "a").put("state", "friends").put("lastSeq", 5).put("lastReadSeq", 2),
            JSONObject().put("uid", "b").put("state", "friends").put("lastSeq", 4),
            JSONObject().put("uid", "c").put("state", "incoming"),
            JSONObject().put("uid", "d").put("state", "outgoing"),
        ).map { FriendsVectors.card(it) }
        assertEquals(8, FriendsLogic.badgeCount(cards))
        assertEquals("3 unread of a are shown as read", 5, FriendsLogic.badgeCount(cards, "a"))
        assertEquals(8, FriendsLogic.badgeCount(cards, "nobody"))
        assertEquals("a request is not a chat", 8, FriendsLogic.badgeCount(cards, "c"))
        assertEquals(0, FriendsLogic.unreadShown(cards[0], "a"))
        assertEquals(3, FriendsLogic.unreadShown(cards[0], null))
    }

    // --- D6, D7 -----------------------------------------------------------------------------------------------

    @Test fun eachRelationHasOneAction() {
        val map = FriendsVectors.obj("relationAction")
        assertEquals(4, map.length())
        for (relation in map.keys()) {
            assertEquals(relation, map.getString(relation), FriendsLogic.relationAction(FriendRelation.of(relation)).key)
        }
    }

    @Test fun theSearchFieldIsClassifiedByTheVectors() {
        val rows = FriendsVectors.rows("query")
        assertTrue(rows.size >= 30)
        for (row in rows) {
            val input = row.getString("input")
            val expect = row.getJSONObject("expect")
            val proof = FriendsLogic.classifyQuery(input)
            when (expect.getString("kind")) {
                "handle" -> assertEquals(input, FriendProof.Handle(expect.getString("handle")), proof)
                "email" -> assertEquals(input, FriendProof.Email(expect.getString("email")), proof)
                "code" -> assertEquals(input, FriendProof.Code(expect.getString("code")), proof)
                else -> assertNull("$input: ${row.optString("note")}", proof)
            }
            if (proof == null) continue
            assertEquals(input, row.getJSONObject("lookup").stringMap(), FriendsLogic.lookupBody(proof).mapValues { it.value.toString() })
            val confirm = FriendsLogic.confirmBody(proof, "request-0001").mapValues { it.value.toString() }.toMutableMap()
            assertEquals("redeem and request carry the id minted at the tap", "request-0001", confirm.remove("requestId"))
            assertEquals(input, row.getJSONObject("confirm").stringMap(), confirm)
        }
    }

    @Test fun aTeamMemberIsRequestedThroughTheTeamProofWithATrimmedNote() {
        val proof = FriendProof.Team("t1", "u9")
        assertEquals(mapOf("action" to "lookup", "teamId" to "t1", "uid" to "u9"), FriendsLogic.lookupBody(proof))
        assertEquals(
            mapOf("action" to "request", "via" to "team", "teamId" to "t1", "uid" to "u9", "requestId" to "request-0001", "note" to "hello"),
            FriendsLogic.confirmBody(proof, "request-0001", "  hello  "),
        )
        assertFalse("a blank note is not sent", FriendsLogic.confirmBody(proof, "request-0001", " \n ").containsKey("note"))
    }

    @Test fun aFriendCodeAlwaysRedeemsAndTakesNoNote() {
        val code = "a".repeat(32)
        assertEquals(
            mapOf("action" to "redeem", "code" to code, "requestId" to "request-0001"),
            FriendsLogic.confirmBody(FriendProof.Code(code), "request-0001", "ignored"),
        )
    }

    @Test fun aFriendCodeIsOnlyRecognisedInTheThreeLinkForms() {
        val code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-"
        for (link in listOf(
            "lingxi://friend/add?code=$code", "https://lingxios.net/open/friend/add?code=$code", "https://lingxios.net/friends/add?code=$code",
            "HTTPS://LINGXIOS.NET/friends/add?code=$code", code,
        )) assertEquals(link, FriendProof.Code(code), FriendsLogic.classifyQuery(link))
        for (link in listOf(
            "https://lingxios.net:8443/open/friend/add?code=$code", "https://user@lingxios.net/open/friend/add?code=$code",
            "https://lingxios.net/open/friend/add?code=%61B0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-", "https://lingxios.net/open/friend/add?x=1&code=$code",
            "https://lingxios.net/open/friend/add/?code=$code", "lingxi://friend/add", "http://lingxios.net/open/friend/add?code=$code",
            "https://evil.example/open/friend/add?code=$code",
        )) {
            // Whatever else it is (a link with userinfo reads as an email), it is never a friend code.
            assertFalse(link, FriendsLogic.classifyQuery(link) is FriendProof.Code)
        }
    }

    @Test fun surroundingWhitespaceIsIgnoredWhenClassifying() {
        assertEquals(FriendProof.Handle("sam_1"), FriendsLogic.classifyQuery("  @sam_1\n"))
        assertEquals(FriendProof.Email("a@b.co"), FriendsLogic.classifyQuery("\u00a0A@B.co "))
        assertNull(FriendsLogic.classifyQuery("   "))
    }

    // --- D11 --------------------------------------------------------------------------------------------------

    private val profile = FriendProfile("u", "Ann", "Ann_1", findByHandle = true, findByEmail = true, friendCode = null)

    private fun changes(name: String = "Ann", handle: String = "Ann_1", byHandle: Boolean = true, byEmail: Boolean = true) =
        ProfileForm(name, handle, byHandle, byEmail).changes(profile)

    @Test fun onlyTheChangedProfileFieldsAreSent() {
        assertTrue("Save stays off", changes().isEmpty())
        assertEquals("Ann Lee", changes(name = "  Ann Lee ")["displayName"])
        assertTrue("trimmed before comparing", changes(name = " Ann ").isEmpty())
        assertTrue("an empty username means leave as is", changes(handle = "").isEmpty())
        assertTrue("so does a blank one", changes(handle = "  ").isEmpty())
        assertEquals("a change of case is a change", "ann_1", changes(handle = "ann_1")["handle"])
        assertEquals("the username is trimmed too", "ann_2", changes(handle = " ann_2 ")["handle"])
        val both = changes(byHandle = false, byEmail = false)
        assertEquals(setOf("findByHandle", "findByEmail"), both.keys)
        assertEquals(false, both["findByHandle"])
    }

    @Test fun theProfileFormIsValidWhenTheNameFitsAndTheUsernameIsEmptyOrWellFormed() {
        assertTrue(ProfileForm("Ann", "", true, true).valid)
        assertTrue(ProfileForm("Ann", " ann_1 ", true, true).valid)
        assertFalse(ProfileForm("  ", "ann_1", true, true).valid)
        assertFalse(ProfileForm("a".repeat(81), "ann_1", true, true).valid)
        assertFalse(ProfileForm("Ann", "an", true, true).valid)
        assertFalse(ProfileForm("Ann", "sam-1", true, true).valid)
        assertFalse(ProfileForm("Ann", "sam-1", true, true).handleValid)
        assertTrue(ProfileForm("", "ann_1", true, true).handleValid)
    }

    @Test fun theBackendsFallbackNameAsksTheRootToOfferProfileSettings() {
        assertTrue(FriendsLogic.needsName(profile.copy(displayName = "Member")))
        assertFalse(FriendsLogic.needsName(profile))
        assertFalse(FriendsLogic.needsName(null))
    }

    // --- D18 --------------------------------------------------------------------------------------------------

    @Test fun errorCodesMapToKeysAndKnownOutcomes() {
        val rows = FriendsVectors.rows("errors")
        assertEquals(21, rows.size)
        for (row in rows) {
            val code = row.getString("code")
            val error = FriendsError.of(code)
            assertEquals("'$code'", row.getString("key"), error.key)
            assertEquals("'$code'", row.getBoolean("definitive"), error.definitive)
        }
    }

    @Test fun aFailureOfTheTransportIsANetworkErrorThatMayBeRetried() {
        for (failure in listOf<Throwable>(java.io.IOException("offline"), org.json.JSONException("not json"), java.net.SocketTimeoutException())) {
            assertEquals(FriendsError.Network, FriendsError.of(failure))
        }
        assertEquals(FriendsError.Network, FriendsError.of(TeamMessageFailure("SERVICE_UNAVAILABLE")))
        assertEquals(FriendsError.HandleTaken, FriendsError.of(TeamMessageFailure("HANDLE_TAKEN")))
        assertFalse(FriendsError.Network.definitive)
        assertTrue(FriendsError.entries.filter { it != FriendsError.Network }.all { it.definitive })
    }

    // --- D9 ---------------------------------------------------------------------------------------------------

    private val friend = FriendCard("u2", FriendState.Friends, "Ann", "ann", null, 1, PairRef("f_abc", 3))

    @Test fun aFriendCardBecomesItsPairTeam() {
        assertEquals(TeamSummary("f_abc", "Ann", 2, "member", 3), FriendsLogic.pairSummary(friend))
        assertEquals(
            listOf(TeamMember("u2", "Ann", "member", 3), TeamMember("u1", "Me", "member", 3)),
            FriendsLogic.pairMembers("u1", "Me", friend),
        )
        assertEquals("the viewer joins once their profile name is known", listOf("u2"), FriendsLogic.pairMembers("u1", "", friend).map { it.uid })
    }

    @Test fun onlyAFriendHasAPairTeam() {
        val request = FriendCard("u3", FriendState.Incoming, "Bo", null, null, 1)
        assertNull(FriendsLogic.pairSummary(request))
        assertTrue(FriendsLogic.pairMembers("u1", "Me", request).isEmpty())
        assertNull(FriendsLogic.pairSummary(friend.copy(pair = null)))
    }
}
