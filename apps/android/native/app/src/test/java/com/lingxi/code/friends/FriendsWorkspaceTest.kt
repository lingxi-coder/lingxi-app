package com.lingxi.code.friends

import com.google.android.gms.tasks.Tasks
import com.google.firebase.Timestamp
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseUser
import com.google.firebase.auth.GetTokenResult
import com.google.firebase.firestore.DocumentSnapshot
import com.google.firebase.firestore.EventListener
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreException
import com.google.firebase.firestore.ListenerRegistration
import com.google.firebase.firestore.QuerySnapshot
import com.lingxi.code.teams.TeamMessageFailure
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.mockito.Mockito.*

/**
 * The app-level Friends workspace (docs/FRIENDS-CLIENTS.md D2, D4, D10, D11, D13, D14, D19) over mocked Firebase and a fake HTTP function:
 * the two owner-only listeners and nothing else, the badge they feed, the calls behind each button, and the teardown on an account change.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class FriendsWorkspaceTest {
    private data class Request(val url: String, val token: String, val body: Map<String, Any?>?)

    private class Harness(scope: CoroutineScope) {
        val auth: FirebaseAuth = mock(FirebaseAuth::class.java)
        val db: FirebaseFirestore = mock(FirebaseFirestore::class.java, RETURNS_DEEP_STUBS)
        lateinit var authListener: FirebaseAuth.AuthStateListener
        val cardListeners = mutableMapOf<String, EventListener<QuerySnapshot>>()
        val blockListeners = mutableMapOf<String, EventListener<QuerySnapshot>>()
        /** In the order they were made: cards then blocks for each account. */
        val registrations = mutableListOf<ListenerRegistration>()
        val requests = mutableListOf<Request>()
        var reply: suspend (Request) -> JSONObject = { JSONObject() }
        val workspace: FriendsWorkspace
        val state get() = workspace.state.value

        init {
            doAnswer { authListener = it.getArgument(0); null }.`when`(auth).addAuthStateListener(anyAuthListener())
            signIn(UID)
            workspace = FriendsWorkspace(scope, auth, db, "https://api.test") { url, token, body ->
                val request = Request(url, token, body?.asMap())
                requests += request
                reply(request)
            }
        }

        fun signIn(uid: String) {
            val user = mock(FirebaseUser::class.java)
            val token = mock(GetTokenResult::class.java)
            doReturn(uid).`when`(user).uid
            doReturn("token-$uid").`when`(token).token
            doReturn(Tasks.forResult(token)).`when`(user).getIdToken(false)
            doReturn(user).`when`(auth).currentUser
            for ((name, store) in listOf("friends" to cardListeners, "blocks" to blockListeners)) {
                val registration = mock(ListenerRegistration::class.java)
                registrations += registration
                val collection = db.collection("users").document(uid).collection(name)
                doAnswer { store[uid] = it.getArgument(0); registration }.`when`(collection).addSnapshotListener(anyListener<QuerySnapshot>())
            }
        }

        fun signOut() { doReturn(null).`when`(auth).currentUser }
        fun authChanged() = authListener.onAuthStateChanged(auth)
        fun cards(snapshot: QuerySnapshot, uid: String = UID) = cardListeners.getValue(uid).onEvent(snapshot, null)
        fun blocks(snapshot: QuerySnapshot, uid: String = UID) = blockListeners.getValue(uid).onEvent(snapshot, null)

        /** What `profiles/{uid}` holds for a friend. */
        fun profile(friendUid: String, displayName: String?, handle: String? = null) {
            val snapshot = mock(DocumentSnapshot::class.java)
            doReturn(displayName).`when`(snapshot).getString("displayName")
            doReturn(handle).`when`(snapshot).getString("handle")
            val document = db.collection("profiles").document(friendUid)
            doReturn(Tasks.forResult(snapshot)).`when`(document).get()
        }

        /** A failing read of `profiles/{uid}`. */
        fun profileFails(friendUid: String) {
            val document = db.collection("profiles").document(friendUid)
            doReturn(Tasks.forException<DocumentSnapshot>(RuntimeException("permission denied"))).`when`(document).get()
        }
    }

    private fun TestScope.started(): Harness = Harness(backgroundScope).also { it.workspace.start(); runCurrent() }

    // --- documents --------------------------------------------------------------------------------------------

    private fun snapshot(vararg documents: Pair<String, Map<String, Any?>>): QuerySnapshot {
        val snapshot = mock(QuerySnapshot::class.java)
        val list = documents.map { (id, data) ->
            mock(DocumentSnapshot::class.java).also { document ->
                doReturn(id).`when`(document).id
                doReturn(data).`when`(document).data
            }
        }
        doReturn(list).`when`(snapshot).documents
        return snapshot
    }

    private fun friendDoc(name: String = "Ann", handle: String? = "ann", lastSeq: Long = 0, lastRead: Long = 0, at: Long? = null): Map<String, Any?> {
        val pointer = at?.let {
            mapOf("lastMessage" to mapOf("id" to "m", "seq" to lastSeq, "at" to Timestamp(it, 0), "author" to mapOf("kind" to "user", "uid" to "x"), "preview" to "hi"))
        }.orEmpty()
        return mapOf(
            "state" to "friends", "displayName" to name, "handle" to handle, "teamId" to "f_$name", "generation" to 1L, "since" to Timestamp(1_762_000_000, 0),
            "lastSeq" to lastSeq, "lastReadSeq" to lastRead,
        ) + pointer
    }

    private fun requestDoc(state: String, name: String) = mapOf<String, Any?>("state" to state, "displayName" to name, "since" to Timestamp(1_762_000_100, 0))

    private fun profileJson(name: String = "Me", handle: String? = "me_1", code: String? = "c1") = JSONObject()
        .put("uid", UID).put("displayName", name).put("findByHandle", true).put("findByEmail", true)
        .also { json ->
            handle?.let { json.put("handle", it) }
            json.put("friendCode", code?.let { JSONObject().put("code", it).put("link", "https://lingxios.net/open/friend/add?code=$it") } ?: JSONObject.NULL)
        }

    // --- the two listeners (D2) -------------------------------------------------------------------------------

    @Test fun startAttachesTheTwoOwnerListenersAndNothingPerFriend() = runTest {
        val h = started()
        assertEquals(setOf(UID), h.cardListeners.keys)
        assertEquals(setOf(UID), h.blockListeners.keys)
        assertEquals(UID, h.state.uid)
        h.cards(snapshot("u2" to friendDoc("Ann"), "u3" to friendDoc("Bo")))
        runCurrent()
        verify(h.db, never()).collection("teams")
        verify(h.db, never()).collection("profiles")
        h.workspace.start()
        verify(h.auth, times(1)).addAuthStateListener(anyAuthListener())
    }

    @Test fun theCardsListenerFeedsTheListAndTheBadge() = runTest {
        val h = started()
        assertFalse(h.state.cardsLoaded)
        assertEquals(0, h.workspace.badge.value)
        h.cards(snapshot(
            "u3" to requestDoc("incoming", "Bo"), "u4" to requestDoc("outgoing", "Cy"),
            "u2" to friendDoc("Ann", lastSeq = 5, lastRead = 2, at = 1_762_000_009), "u5" to friendDoc("Dee", lastSeq = 1, lastRead = 1),
            "bad" to mapOf("state" to "blocked", "displayName" to "X"), "nopair" to mapOf("state" to "friends", "displayName" to "Y"),
        ))
        runCurrent()
        assertTrue(h.state.cardsLoaded)
        assertEquals("a card that cannot be placed is skipped, the rest survive", setOf("u2", "u3", "u4", "u5"), h.state.cards.map { it.uid }.toSet())
        assertEquals("one incoming request plus 3 + 0 unread", 4, h.workspace.badge.value)
        assertNotNull(h.state.friendCard("u2"))
        assertNull("a request is not a friend", h.state.friendCard("u3"))
        assertEquals("Bo", h.state.card("u3")?.displayName)
        assertEquals(1_762_000_009_000L, h.state.friendCard("u2")?.lastAtMs)
    }

    @Test fun theBlocksListenerFeedsTheBlockedList() = runTest {
        val h = started()
        h.blocks(snapshot("u7" to mapOf("at" to Timestamp(1_762_000_500, 0), "displayName" to "Fay", "handle" to "fay"), "u8" to mapOf("at" to Timestamp(1_762_000_600, 0))))
        runCurrent()
        assertTrue(h.state.blocksLoaded)
        assertEquals(setOf(BlockEntry("u7", 1_762_000_500_000L, "Fay", "fay"), BlockEntry("u8", 1_762_000_600_000L, "", null)), h.state.blocks.toSet())
    }

    @Test fun aListenerErrorKeepsTheLastCardsAndRetryStartsItAgain() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc()))
        val first = h.cardListeners.getValue(UID)
        first.onEvent(null, FirebaseFirestoreException("offline", FirebaseFirestoreException.Code.UNAVAILABLE))
        runCurrent()
        assertTrue(h.state.cardsFailed)
        assertTrue(h.state.cardsLoaded)
        assertEquals("the last snapshot stays", 1, h.state.cards.size)
        assertFalse(h.state.blocksFailed)
        h.workspace.retry()
        runCurrent()
        verify(h.registrations[0]).remove()
        assertNotSame("a new listener", first, h.cardListeners.getValue(UID))
        assertFalse(h.state.cardsFailed)
        h.cards(snapshot("u2" to friendDoc(), "u6" to friendDoc("Eve")))
        runCurrent()
        assertEquals(2, h.state.cards.size)
        // Retry with nothing failed does nothing.
        val second = h.cardListeners.getValue(UID)
        h.workspace.retry()
        assertSame(second, h.cardListeners.getValue(UID))
    }

    @Test fun theOpenChatCountsAsReadInTheListAndTheBadge() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc(lastSeq = 5, lastRead = 2), "u3" to requestDoc("incoming", "Bo")))
        runCurrent()
        assertEquals(4, h.workspace.badge.value)
        h.workspace.chatOpened("u2")
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
        assertEquals("u2", h.state.openChatUid)
        h.workspace.chatClosed("someone-else")
        runCurrent()
        assertEquals("closing another chat changes nothing", 1, h.workspace.badge.value)
        h.workspace.chatClosed("u2")
        runCurrent()
        assertEquals(4, h.workspace.badge.value)
    }

    // --- teardown (D19) ---------------------------------------------------------------------------------------

    @Test fun signingOutTearsEverythingDownAndALateSnapshotIsIgnored() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc(lastSeq = 2)))
        h.blocks(snapshot("u7" to mapOf("at" to Timestamp(1, 0))))
        h.reply = { profileJson() }
        h.workspace.loadProfile()
        h.workspace.search.lookup(FriendProof.Handle("ann"))
        runCurrent()
        assertNotNull(h.state.profile)
        val stale = h.cardListeners.getValue(UID)
        h.signOut()
        h.authChanged()
        runCurrent()
        assertEquals(FriendsState(), h.state)
        verify(h.registrations[0]).remove()
        verify(h.registrations[1]).remove()
        assertEquals(LookupState(), h.workspace.search.state.value)
        assertEquals(0, h.workspace.badge.value)
        stale.onEvent(snapshot("u2" to friendDoc()), null)
        runCurrent()
        assertEquals("a snapshot of the old account is ignored", FriendsState(), h.state)
    }

    @Test fun anotherAccountStartsFromNothing() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc()))
        h.reply = { profileJson() }
        h.workspace.loadProfile()
        runCurrent()
        h.signIn("u9")
        h.authChanged()
        runCurrent()
        assertEquals("u9", h.state.uid)
        assertTrue(h.state.cards.isEmpty())
        assertNull("no profile of the previous account", h.state.profile)
        verify(h.registrations[0]).remove()
        h.cards(snapshot("u8" to friendDoc("Zed")), "u9")
        runCurrent()
        assertEquals(listOf("u8"), h.state.cards.map { it.uid })
    }

    @Test fun theSameAccountAgainChangesNothing() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc()))
        val listener = h.cardListeners.getValue(UID)
        h.authChanged()
        runCurrent()
        assertSame(listener, h.cardListeners.getValue(UID))
        assertEquals(1, h.state.cards.size)
        verify(h.registrations[0], never()).remove()
    }

    @Test fun signedOutThereIsNothingToListenTo() = runTest {
        val h = Harness(backgroundScope)
        h.signOut()
        h.workspace.start()
        runCurrent()
        assertNull(h.state.uid)
        assertTrue(h.cardListeners.isEmpty())
        h.workspace.loadProfile()
        h.workspace.accept("u3")
        runCurrent()
        assertTrue("no call without an account", h.requests.isEmpty())
        assertEquals(0, h.workspace.badge.value)
    }

    // --- my profile and friend link (D2, D11, D6) -----------------------------------------------------------------

    @Test fun theProfileIsReadWithGetAndTheIdTokenAndNeverTwiceAtOnce() = runTest {
        val h = started()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.loadProfile()
        h.workspace.loadProfile()
        runCurrent()
        assertTrue(h.state.profileOp.busy)
        assertEquals(listOf(Request("https://api.test/v1/friends/profile", "token-$UID", null)), h.requests)
        gate.complete(profileJson())
        runCurrent()
        assertEquals(FriendsOp(), h.state.profileOp)
        assertEquals("Me", h.state.profile?.displayName)
        assertEquals("c1", h.state.profile?.friendCode?.code)
    }

    @Test fun aFailedProfileReadShowsItsErrorAndRetryRecovers() = runTest {
        val h = started()
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.loadProfile()
        runCurrent()
        assertNull(h.state.profile)
        assertEquals(FriendsOp(error = FriendsError.Network), h.state.profileOp)
        h.reply = { profileJson() }
        h.workspace.loadProfile()
        runCurrent()
        assertNotNull(h.state.profile)
        assertEquals(FriendsOp(), h.state.profileOp)
    }

    private fun TestScope.withProfile(): Harness = started().also { h ->
        h.reply = { profileJson() }
        h.workspace.loadProfile()
        runCurrent()
        h.requests.clear()
    }

    @Test fun saveSendsOnlyTheChangedFieldsAndKeepsTheAnswer() = runTest {
        val h = withProfile()
        h.reply = { profileJson(name = "Ann Lee") }
        h.workspace.saveProfile(ProfileForm("  Ann Lee ", "me_1", true, true))
        runCurrent()
        assertEquals(listOf(Request("https://api.test/v1/friends", "token-$UID", mapOf("action" to "profile", "displayName" to "Ann Lee"))), h.requests)
        assertEquals("Ann Lee", h.state.profile?.displayName)
        assertEquals(FriendsOp(), h.state.saveOp)
    }

    @Test fun nothingIsSavedWhenNothingChangedOrTheFormIsInvalid() = runTest {
        val h = withProfile()
        h.workspace.saveProfile(ProfileForm("Me", "me_1", true, true))
        h.workspace.saveProfile(ProfileForm("Me", "", true, true))
        h.workspace.saveProfile(ProfileForm("  ", "me_1", false, true))
        h.workspace.saveProfile(ProfileForm("Me", "no", false, true))
        runCurrent()
        assertTrue(h.requests.isEmpty())
    }

    @Test fun aTakenUsernameIsReportedAndTheProfileStaysAsItWas() = runTest {
        val h = withProfile()
        h.reply = { throw TeamMessageFailure("HANDLE_TAKEN") }
        h.workspace.saveProfile(ProfileForm("Me", "sam_1", true, true))
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.HandleTaken), h.state.saveOp)
        assertEquals("me_1", h.state.profile?.handle)
        h.reply = { profileJson(handle = "sam_1") }
        h.workspace.saveProfile(ProfileForm("Me", "sam_1", true, true))
        runCurrent()
        assertEquals("a new attempt starts without the old error", FriendsOp(), h.state.saveOp)
        assertEquals("sam_1", h.state.profile?.handle)
    }

    @Test fun theFriendLinkCanBeRotatedAndTurnedOff() = runTest {
        val h = withProfile()
        h.reply = { JSONObject().put("code", "c2").put("link", "https://lingxios.net/open/friend/add?code=c2") }
        h.workspace.rotateCode()
        runCurrent()
        assertEquals(mapOf("action" to "rotateCode"), h.requests.single().body)
        assertEquals(FriendCode("c2", "https://lingxios.net/open/friend/add?code=c2"), h.state.profile?.friendCode)
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.revokeCode()
        runCurrent()
        assertEquals(mapOf("action" to "revokeCode"), h.requests.last().body)
        assertNull(h.state.profile?.friendCode)
        assertEquals(FriendsOp(), h.state.linkOp)
        h.reply = { throw TeamMessageFailure("RATE_LIMITED") }
        h.workspace.rotateCode()
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.RateLimited), h.state.linkOp)
        assertNull("the link stays off", h.state.profile?.friendCode)
    }

    @Test fun linkActionsWaitForTheProfile() = runTest {
        val h = started()
        h.workspace.rotateCode()
        h.workspace.revokeCode()
        h.workspace.saveProfile(ProfileForm("Me", "me_1", true, true))
        runCurrent()
        assertTrue(h.requests.isEmpty())
    }

    // --- request rows, the friend menu and the blocked list (D4, D10, D12) -------------------------------------

    @Test fun acceptDisablesItsRowWhileInFlightAndShowsNowFriendsOnce() = runTest {
        val h = started()
        h.cards(snapshot("u3" to requestDoc("incoming", "Bo")))
        runCurrent()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.accept("u3")
        h.workspace.accept("u3")
        runCurrent()
        assertEquals(listOf(Request("https://api.test/v1/friends", "token-$UID", mapOf("action" to "accept", "otherUid" to "u3"))), h.requests)
        assertEquals(FriendsOp(busy = true), h.state.rowOps["u3"])
        gate.complete(JSONObject().put("state", "friends"))
        runCurrent()
        assertTrue(h.state.rowOps.isEmpty())
        assertEquals(FriendsNotice.NowFriends("Bo"), h.state.notice)
    }

    @Test fun eachRowActionSendsItsOwnAction() = runTest {
        val h = started()
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.decline("a")
        h.workspace.cancel("b")
        h.workspace.unblock("c")
        runCurrent()
        assertEquals(
            listOf(mapOf("action" to "decline", "otherUid" to "a"), mapOf("action" to "cancel", "otherUid" to "b"), mapOf("action" to "unblock", "otherUid" to "c")),
            h.requests.map { it.body },
        )
        assertTrue(h.state.rowOps.isEmpty())
        assertNull("only accept announces", h.state.notice)
    }

    @Test fun aFailedRowActionKeepsItsErrorUntilClearedOrTriedAgain() = runTest {
        val h = started()
        h.reply = { throw TeamMessageFailure("REQUEST_UNAVAILABLE") }
        h.workspace.decline("u3")
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.RequestUnavailable), h.state.rowOps["u3"])
        h.workspace.clearRowError("u3")
        assertTrue(h.state.rowOps.isEmpty())
        h.workspace.decline("u3")
        runCurrent()
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.decline("u3")
        runCurrent()
        assertTrue("a new attempt clears the error", h.state.rowOps.isEmpty())
    }

    @Test fun clearingARowErrorNeverClearsAnActionInFlight() = runTest {
        val h = started()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.cancel("u3")
        runCurrent()
        h.workspace.clearRowError("u3")
        assertEquals(FriendsOp(busy = true), h.state.rowOps["u3"])
        gate.complete(JSONObject().put("ok", true))
        runCurrent()
        assertTrue(h.state.rowOps.isEmpty())
    }

    @Test fun removeAndBlockRunTheirCallbackOnlyOnSuccess() = runTest {
        val h = started()
        val done = mutableListOf<String>()
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.remove("u2") { done += "remove" }
        h.workspace.block("u2") { done += "block" }
        runCurrent()
        assertTrue(done.isEmpty())
        assertEquals(FriendsOp(error = FriendsError.Network), h.state.rowOps["u2"])
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.remove("u2") { done += "remove" }
        runCurrent()
        h.workspace.block("u2") { done += "block" }
        runCurrent()
        assertEquals(listOf("remove", "block"), done)
        assertEquals(listOf("remove", "block"), h.requests.takeLast(2).map { it.body?.get("action") })
    }

    // --- a new friend (D7) --------------------------------------------------------------------------------------

    private fun lookupAnswer(request: Request) = when (request.body?.get("action")) {
        "lookup" -> JSONObject().put("uid", "u2").put("displayName", "Ann").put("handle", "ann").put("relation", "none")
        else -> JSONObject().put("uid", "u2").put("state", "friends").put("teamId", "f_Ann")
    }

    @Test fun aNewFriendIsAnnouncedOnceTheirCardHasArrived() = runTest {
        val h = started()
        h.reply = { lookupAnswer(it) }
        h.workspace.search.lookup(FriendProof.Code("a".repeat(32)))
        runCurrent()
        h.workspace.search.confirm("")
        runCurrent()
        assertEquals("redeem", h.requests.last().body?.get("action"))
        assertTrue("still waiting for the card the chat needs", h.workspace.search.state.value.confirming)
        assertNull(h.state.notice)
        h.cards(snapshot("u2" to friendDoc("Ann")))
        runCurrent()
        assertEquals(FriendOutcome.NowFriends("u2", "Ann"), h.workspace.search.state.value.outcome)
        assertEquals(FriendsNotice.NowFriends("Ann"), h.state.notice)
    }

    @Test fun theWaitForTheNewFriendsCardGivesUp() = runTest {
        val h = started()
        h.reply = { lookupAnswer(it) }
        h.workspace.search.lookup(FriendProof.Handle("ann"))
        runCurrent()
        h.workspace.search.confirm("")
        runCurrent()
        assertTrue(h.workspace.search.state.value.confirming)
        advanceTimeBy(7_999)
        runCurrent()
        assertTrue(h.workspace.search.state.value.confirming)
        advanceTimeBy(2)
        runCurrent()
        assertEquals(FriendOutcome.NowFriends("u2", "Ann"), h.workspace.search.state.value.outcome)
    }

    @Test fun aSentRequestIsNotAnnouncedByTheWorkspace() = runTest {
        val h = started()
        h.reply = { request -> if (request.body?.get("action") == "lookup") lookupAnswer(request) else JSONObject().put("uid", "u2").put("state", "outgoing") }
        h.workspace.member.lookup(FriendProof.Team("t1", "u2"))
        runCurrent()
        h.workspace.member.confirm("hi")
        runCurrent()
        assertEquals(FriendOutcome.RequestSent("Ann"), h.workspace.member.state.value.outcome)
        assertNull("the screen that sent it says so itself", h.state.notice)
        assertEquals("the other lookups are untouched", LookupState(), h.workspace.search.state.value)
        assertEquals(LookupState(), h.workspace.link.state.value)
    }

    // --- names (D10) --------------------------------------------------------------------------------------------

    @Test fun aFriendsProfileNameReplacesTheCardNameForTheSession() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc("Ann", "ann"), "u3" to requestDoc("incoming", "Bo")))
        runCurrent()
        h.profile("u2", "Ann Lee", "annlee")
        h.workspace.refreshName("u2")
        runCurrent()
        assertEquals(FriendName("Ann Lee", "annlee"), h.state.names["u2"])
        assertEquals("Ann Lee", h.state.friendCard("u2")?.displayName)
        assertEquals("annlee", h.state.friendCard("u2")?.handle)
        assertEquals("Ann Lee", h.state.resolvedCards().first { it.uid == "u2" }.displayName)
        assertEquals("the card itself is never written", "Ann", h.state.cards.first { it.uid == "u2" }.displayName)
        assertEquals("Bo", h.state.resolvedCards().first { it.uid == "u3" }.displayName)
    }

    @Test fun aProfileThatAgreesWithTheCardNeedsNoOverride() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc("Ann", "ann")))
        runCurrent()
        h.profile("u2", "Ann Lee", "annlee")
        h.workspace.refreshName("u2")
        runCurrent()
        assertEquals(1, h.state.names.size)
        h.profile("u2", "Ann", "ann")
        h.workspace.refreshName("u2")
        runCurrent()
        assertTrue(h.state.names.isEmpty())
        assertEquals("Ann", h.state.friendCard("u2")?.displayName)
    }

    @Test fun aFailedOrEmptyProfileReadChangesNothing() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc("Ann", "ann"), "u3" to friendDoc("Bo", "bo")))
        runCurrent()
        h.profileFails("u2")
        h.workspace.refreshName("u2")
        h.profile("u3", "")
        h.workspace.refreshName("u3")
        h.profile("nobody", "Zed")
        h.workspace.refreshName("nobody")
        runCurrent()
        assertTrue(h.state.names.isEmpty())
        assertEquals("Ann", h.state.friendCard("u2")?.displayName)
    }

    @Test fun anOverrideEndsWithTheFriendship() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc("Ann", "ann"), "u3" to friendDoc("Bo", "bo")))
        runCurrent()
        h.profile("u2", "Ann Lee", "annlee")
        h.profile("u3", "Bob", "bob")
        h.workspace.refreshName("u2")
        h.workspace.refreshName("u3")
        runCurrent()
        assertEquals(setOf("u2", "u3"), h.state.names.keys)
        h.cards(snapshot("u3" to friendDoc("Bo", "bo")))
        runCurrent()
        assertEquals(setOf("u3"), h.state.names.keys)
    }

    // --- notices ----------------------------------------------------------------------------------------------------

    @Test fun aNoticeIsShownOnceAndClearedOnlyByItself() = runTest {
        val h = started()
        h.workspace.chatUnavailable()
        assertEquals(FriendsNotice.ChatUnavailable, h.state.notice)
        h.workspace.clearNotice(FriendsNotice.NowFriends("Bo"))
        assertEquals("a different notice leaves it alone", FriendsNotice.ChatUnavailable, h.state.notice)
        h.workspace.clearNotice(FriendsNotice.ChatUnavailable)
        assertNull(h.state.notice)
    }

    private companion object {
        const val UID = "u1"
        fun JSONObject.asMap(): Map<String, Any?> = keys().asSequence().associateWith { opt(it) }
        fun <T> anyListener(): EventListener<T> = any<EventListener<T>>() ?: EventListener { _, _ -> }
        fun anyAuthListener(): FirebaseAuth.AuthStateListener = any<FirebaseAuth.AuthStateListener>() ?: FirebaseAuth.AuthStateListener { }
    }
}
