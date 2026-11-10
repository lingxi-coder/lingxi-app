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
 * The app-level Friends workspace (docs/FRIENDS-CLIENTS.md D2, D4, D10, D11, D13, D14, D19, D26 to D28, D30) over mocked Firebase and a fake HTTP
 * function: the three owner-only listeners and nothing else, the badge they feed (with a clock the tests move), the calls behind each button,
 * and the teardown on an account change.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class FriendsWorkspaceTest {
    private data class Request(val url: String, val token: String, val body: Map<String, Any?>?)

    private class Harness(scope: CoroutineScope, clock: () -> Long) {
        val auth: FirebaseAuth = mock(FirebaseAuth::class.java)
        val db: FirebaseFirestore = mock(FirebaseFirestore::class.java, RETURNS_DEEP_STUBS)
        lateinit var authListener: FirebaseAuth.AuthStateListener
        val cardListeners = mutableMapOf<String, EventListener<QuerySnapshot>>()
        val blockListeners = mutableMapOf<String, EventListener<QuerySnapshot>>()
        val offerListeners = mutableMapOf<String, EventListener<QuerySnapshot>>()
        /** In the order they were made: cards, blocks, then offers for each account. */
        val registrations = mutableListOf<ListenerRegistration>()
        val requests = mutableListOf<Request>()
        var reply: suspend (Request) -> JSONObject = { JSONObject() }
        val workspace: FriendsWorkspace
        val state get() = workspace.state.value

        init {
            doAnswer { authListener = it.getArgument(0); null }.`when`(auth).addAuthStateListener(anyAuthListener())
            signIn(UID)
            workspace = FriendsWorkspace(scope, auth, db, "https://api.test", clock) { url, token, body ->
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
            for ((name, store) in listOf("friends" to cardListeners, "blocks" to blockListeners, "teamOffers" to offerListeners)) {
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
        fun offers(snapshot: QuerySnapshot, uid: String = UID) = offerListeners.getValue(uid).onEvent(snapshot, null)

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

    /** The clock of the workspace is the test's virtual time on top of [NOW], plus whatever [skew] says the device clock jumped. */
    private fun TestScope.started(skew: () -> Long = { 0L }): Harness = Harness(backgroundScope) { NOW + testScheduler.currentTime + skew() }.also { it.workspace.start(); runCurrent() }

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

    // --- the three listeners (D2) -----------------------------------------------------------------------------

    @Test fun startAttachesTheThreeOwnerListenersAndNothingPerFriend() = runTest {
        val h = started()
        assertEquals(setOf(UID), h.cardListeners.keys)
        assertEquals(setOf(UID), h.blockListeners.keys)
        assertEquals(setOf(UID), h.offerListeners.keys)
        assertEquals(3, h.registrations.size)
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
        assertEquals(FriendsState(nowMs = NOW), h.state)
        verify(h.registrations[0]).remove()
        verify(h.registrations[1]).remove()
        verify(h.registrations[2]).remove()
        assertEquals(LookupState(), h.workspace.search.state.value)
        assertEquals(0, h.workspace.badge.value)
        stale.onEvent(snapshot("u2" to friendDoc()), null)
        runCurrent()
        assertEquals("a snapshot of the old account is ignored", FriendsState(nowMs = NOW), h.state)
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
        val h = Harness(backgroundScope) { NOW }
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

    // --- team offers: the listener, the badge and its clock (D27, D28) ----------------------------------------------

    /** `users/{uid}/teamOffers/{teamId}`: the offer a friend made of an ordinary team; [expiresAtMs] is whole epoch milliseconds. */
    private fun offerDoc(teamName: String = "Book club", fromName: String = "Ann", expiresAtMs: Long): Map<String, Any?> = mapOf(
        "teamName" to teamName, "fromUid" to "u2", "fromName" to fromName, "role" to "member",
        "expiresAt" to Timestamp(expiresAtMs / 1000, ((expiresAtMs % 1000) * 1_000_000).toInt()),
    ) + mapOf("teamId" to "")

    private fun offerSnapshot(vararg offers: Pair<String, Map<String, Any?>>) =
        snapshot(*offers.map { (id, doc) -> id to (doc + ("teamId" to id)) }.toTypedArray())

    @Test fun theOffersListenerFeedsTheOffersAndSkipsADocumentThatIsNotWhole() = runTest {
        val h = started()
        assertTrue(h.state.offers.isEmpty())
        h.offers(offerSnapshot(
            "t1" to offerDoc(expiresAtMs = NOW + 3_600_000), "t2" to offerDoc("Chess", "Bo", NOW + 7_200_000),
            "bad" to mapOf("teamName" to "X", "teamId" to "bad", "fromUid" to "u2", "fromName" to "Ann"),
        ))
        runCurrent()
        assertEquals(listOf(TeamOffer("t1", "Book club", "u2", "Ann", NOW + 3_600_000), TeamOffer("t2", "Chess", "u2", "Bo", NOW + 7_200_000)), h.state.offers)
        assertFalse(h.state.offersFailed)
        verify(h.db, never()).collection("teams")
    }

    @Test fun theBadgeCountsEveryLiveOfferBesideRequestsAndUnread() = runTest {
        val h = started()
        h.cards(snapshot("u3" to requestDoc("incoming", "Bo"), "u2" to friendDoc("Ann", lastSeq = 3, lastRead = 1)))
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 1_000), "t2" to offerDoc(expiresAtMs = NOW - 1), "t3" to offerDoc(expiresAtMs = NOW), "t4" to offerDoc(expiresAtMs = NOW + 86_400_000)))
        runCurrent()
        assertEquals("1 request + 2 unread + 2 live offers; an offer that expired and one that expires now add nothing", 5, h.workspace.badge.value)
        h.workspace.chatOpened("u2")
        runCurrent()
        assertEquals(3, h.workspace.badge.value)
    }

    @Test fun aTimerDropsEachOfferFromTheBadgeAtItsExpiry() = runTest {
        val h = started()
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 60_000), "t2" to offerDoc(expiresAtMs = NOW + 180_000), "t3" to offerDoc(expiresAtMs = NOW - 5)))
        runCurrent()
        assertEquals(2, h.workspace.badge.value)
        advanceTimeBy(59_999)
        runCurrent()
        assertEquals(2, h.workspace.badge.value)
        assertEquals("nothing read the clock before the timer", NOW, h.state.nowMs)
        advanceTimeBy(2)
        runCurrent()
        assertEquals("the earliest offer expired with no document changing", 1, h.workspace.badge.value)
        assertEquals("the timer read the clock at the expiry", NOW + 60_000, h.state.nowMs)
        advanceTimeBy(120_000)
        runCurrent()
        assertEquals(0, h.workspace.badge.value)
        assertEquals("the offers themselves stay until the server prunes them", 3, h.state.offers.size)
    }

    @Test fun aNewSnapshotMovesTheTimerToTheNewEarliestExpiry() = runTest {
        val h = started()
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 120_000)))
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 120_000), "t2" to offerDoc(expiresAtMs = NOW + 30_000)))
        runCurrent()
        assertEquals(2, h.workspace.badge.value)
        advanceTimeBy(30_001)
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
        h.offers(offerSnapshot())
        runCurrent()
        assertEquals("no offer, no timer", 0, h.workspace.badge.value)
        advanceTimeBy(500_000)
        runCurrent()
        assertEquals(0, h.workspace.badge.value)
    }

    @Test fun showingTheViewReadsTheClockAgain() = runTest {
        var skew = 0L
        val h = started { skew }
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 60_000), "t2" to offerDoc(expiresAtMs = NOW + 600_000)))
        runCurrent()
        assertEquals(2, h.workspace.badge.value)
        skew = 120_000
        runCurrent()
        assertEquals("the clock moved but nothing read it", 2, h.workspace.badge.value)
        h.workspace.refreshClock()
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
        assertEquals(NOW + 120_000, h.state.nowMs)
    }

    @Test fun aFailedOffersListenerShowsAndCountsNothingAndRetryRestartsOnlyIt() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc()))
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 60_000)))
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
        val cardsListener = h.cardListeners.getValue(UID)
        val first = h.offerListeners.getValue(UID)
        first.onEvent(null, FirebaseFirestoreException("denied", FirebaseFirestoreException.Code.PERMISSION_DENIED))
        runCurrent()
        assertTrue(h.state.offersFailed)
        assertTrue("it ends that watch and shows nothing", h.state.offers.isEmpty())
        assertEquals(0, h.workspace.badge.value)
        assertFalse(h.state.cardsFailed)
        h.workspace.retry()
        runCurrent()
        verify(h.registrations[2]).remove()
        assertNotSame("a new offers listener", first, h.offerListeners.getValue(UID))
        assertSame("the cards listener was fine and is left alone", cardsListener, h.cardListeners.getValue(UID))
        assertFalse(h.state.offersFailed)
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 60_000)))
        runCurrent()
        assertEquals(1, h.workspace.badge.value)
    }

    @Test fun anAccountChangeDropsTheOffersTheTimerAndEveryRequestId() = runTest {
        val h = started()
        h.cards(snapshot("u2" to friendDoc("Ann")))
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 60_000)))
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.acceptOffer("t1")
        h.workspace.offerTeam("t9", "u2")
        runCurrent()
        val before = h.requests.map { it.body?.get("requestId") }
        assertEquals(2, before.size)
        h.signIn("u9")
        h.authChanged()
        runCurrent()
        assertTrue(h.state.offers.isEmpty() && h.state.offerOps.isEmpty() && h.state.invites.isEmpty())
        verify(h.registrations[2]).remove()
        advanceTimeBy(120_000)
        runCurrent()
        assertEquals("the old account's timer is gone", 0, h.workspace.badge.value)
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 600_000)), "u9")
        h.cards(snapshot("u2" to friendDoc("Ann")), "u9")
        runCurrent()
        h.reply = { JSONObject().put("teamId", "t1").put("generation", 1).put("alreadyMember", false) }
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertTrue("a new account mints new request ids", h.requests.last().body?.get("requestId") !in before)
    }

    // --- team offers: join, decline and dismiss (D27, D30) ----------------------------------------------------------

    private fun joinedJson(alreadyMember: Boolean = false) = JSONObject().put("teamId", "t1").put("generation", 2).put("alreadyMember", alreadyMember)

    private fun TestScope.withOffers(): Harness = started().also { h ->
        h.offers(offerSnapshot("t1" to offerDoc(expiresAtMs = NOW + 3_600_000), "old" to offerDoc("Old club", "Cy", NOW - 1_000)))
        runCurrent()
    }

    @Test fun joiningCallsAcceptOfferWithARequestIdAndAnnouncesTheTeamOnce() = runTest {
        val h = withOffers()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        var joined = 0
        h.workspace.acceptOffer("t1") { joined++ }
        h.workspace.acceptOffer("t1") { joined++ }
        runCurrent()
        val request = h.requests.single()
        assertEquals("https://api.test/v1/friends", request.url)
        assertEquals("token-$UID", request.token)
        assertEquals(setOf("action", "offerId", "requestId"), request.body?.keys)
        assertEquals("acceptOffer", request.body?.get("action"))
        assertEquals("t1", request.body?.get("offerId"))
        assertTrue(FriendsLogic.isRequestId(request.body?.get("requestId") as String))
        assertEquals("the row is disabled while it runs", FriendsOp(busy = true), h.state.offerOps["t1"])
        assertNull(h.state.notice)
        gate.complete(joinedJson())
        runCurrent()
        assertEquals(1, joined)
        assertTrue(h.state.offerOps.isEmpty())
        assertEquals(FriendsNotice.OfferJoined("Book club"), h.state.notice)
        assertEquals("the row goes through the listener", 2, h.state.offers.size)
    }

    @Test fun aReplayOfAnEarlierJoinIsASuccess() = runTest {
        val h = withOffers()
        h.reply = { joinedJson(alreadyMember = true) }
        var joined = 0
        h.workspace.acceptOffer("t1") { joined++ }
        runCurrent()
        assertEquals(1, joined)
        assertEquals(FriendsNotice.OfferJoined("Book club"), h.state.notice)
        assertTrue(h.state.offerOps.isEmpty())
    }

    @Test fun anOfferThatLooksExpiredIsNeverSent() = runTest {
        val h = withOffers()
        h.reply = { joinedJson() }
        h.workspace.acceptOffer("old")
        h.workspace.acceptOffer("nobody")
        runCurrent()
        assertTrue(h.requests.isEmpty())
        h.offers(offerSnapshot("edge" to offerDoc(expiresAtMs = NOW), "near" to offerDoc(expiresAtMs = NOW + 1)))
        runCurrent()
        h.workspace.acceptOffer("edge")
        runCurrent()
        assertTrue("one that expires exactly now is expired", h.requests.isEmpty())
        h.workspace.acceptOffer("near")
        runCurrent()
        assertEquals("near", h.requests.single().body?.get("offerId"))
    }

    @Test fun aJoinKeepsItsRequestIdOnlyAfterAnUnknownOutcome() = runTest {
        val h = withOffers()
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.Network), h.state.offerOps["t1"])
        h.workspace.acceptOffer("t1")
        runCurrent()
        val first = h.requests[0].body?.get("requestId")
        assertEquals("an unknown outcome is retried with the same id", first, h.requests[1].body?.get("requestId"))
        h.reply = { throw TeamMessageFailure("MEMBER_LIMIT") }
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertEquals("the retry still carries it", first, h.requests[2].body?.get("requestId"))
        assertEquals(FriendsOp(error = FriendsError.MemberLimit), h.state.offerOps["t1"])
        h.reply = { joinedJson() }
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertNotEquals("a definite answer drops the id: the next tap is a new request", first, h.requests[3].body?.get("requestId"))
        assertEquals(FriendsNotice.OfferJoined("Book club"), h.state.notice)
        assertTrue("a new attempt starts without the old error", h.state.offerOps.isEmpty())
    }

    @Test fun aJoinThatFailsKeepsTheOfferAndShowsTheOfferTextUnderTheRow() = runTest {
        val h = withOffers()
        for ((code, error) in listOf("TEAM_LIMIT" to FriendsError.TeamLimit, "MEMBER_LIMIT" to FriendsError.MemberLimit, "STALE_MEMBERSHIP" to FriendsError.OfferUnavailable, "MEMBERSHIP_REQUIRED" to FriendsError.Generic)) {
            h.reply = { throw TeamMessageFailure(code) }
            h.workspace.acceptOffer("t1")
            runCurrent()
            assertEquals(code, FriendsOp(error = error), h.state.offerOps["t1"])
            assertNull("no notice for a failure", h.state.notice)
        }
        assertEquals("Join stays available: the offer is still there", 2, h.state.offers.size)
        assertTrue("not read as the end of a chat", h.state.offerOps.values.none { it.error == FriendsError.ChatUnavailable })
    }

    @Test fun anOfferTheServerDeletedLeavesWithItsRowAndItsError() = runTest {
        val h = withOffers()
        h.reply = { throw TeamMessageFailure("OFFER_UNAVAILABLE") }
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.OfferUnavailable), h.state.offerOps["t1"])
        h.offers(offerSnapshot("old" to offerDoc("Old club", "Cy", NOW - 1_000)))
        runCurrent()
        assertEquals(listOf("old"), h.state.offers.map { it.teamId })
        assertTrue("the error went with the row", h.state.offerOps.isEmpty())
        h.reply = { throw TeamMessageFailure("OFFER_UNAVAILABLE") }
        h.workspace.declineOffer("t1")
        runCurrent()
        assertTrue("nothing to hang an error on", h.state.offerOps.isEmpty() && h.requests.size == 1)
    }

    @Test fun anAnswerThatArrivesAfterTheRowWentLeavesNoError() = runTest {
        val h = withOffers()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.acceptOffer("t1")
        runCurrent()
        h.offers(offerSnapshot("old" to offerDoc("Old club", "Cy", NOW - 1_000)))
        runCurrent()
        gate.completeExceptionally(TeamMessageFailure("OFFER_UNAVAILABLE"))
        runCurrent()
        assertTrue(h.state.offerOps.isEmpty())
    }

    @Test fun decliningAndDismissingCallDeclineOfferWithNoRequestId() = runTest {
        val h = withOffers()
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.declineOffer("t1")
        h.workspace.declineOffer("old")
        h.workspace.declineOffer("nobody")
        runCurrent()
        assertEquals(
            listOf(mapOf("action" to "declineOffer", "offerId" to "t1"), mapOf("action" to "declineOffer", "offerId" to "old")),
            h.requests.map { it.body },
        )
        assertTrue(h.state.offerOps.isEmpty())
        assertNull("declining announces nothing", h.state.notice)
        assertEquals("the listener removes the rows", 2, h.state.offers.size)
    }

    @Test fun aDeclineIsDisabledWhileItRunsFailsWithTheCommonRowsAndCanBeRepeated() = runTest {
        val h = withOffers()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.declineOffer("t1")
        h.workspace.declineOffer("t1")
        h.workspace.acceptOffer("t1")
        runCurrent()
        assertEquals("one call, and a busy row sends nothing else", 1, h.requests.size)
        assertEquals(FriendsOp(busy = true), h.state.offerOps["t1"])
        gate.completeExceptionally(TeamMessageFailure("SERVICE_UNAVAILABLE"))
        runCurrent()
        assertEquals(FriendsOp(error = FriendsError.Network), h.state.offerOps["t1"])
        h.reply = { throw TeamMessageFailure("OFFER_UNAVAILABLE") }
        h.workspace.declineOffer("t1")
        runCurrent()
        assertEquals("a code of another action is generic", FriendsOp(error = FriendsError.Generic), h.state.offerOps["t1"])
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.declineOffer("t1")
        runCurrent()
        assertTrue(h.state.offerOps.isEmpty())
        assertEquals("the busy taps sent nothing", 3, h.requests.size)
    }

    @Test fun offerActionsNeedAnAccount() = runTest {
        val h = Harness(backgroundScope) { NOW }
        h.signOut()
        h.workspace.start()
        runCurrent()
        h.workspace.acceptOffer("t1")
        h.workspace.declineOffer("t1")
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertTrue(h.requests.isEmpty())
    }

    // --- team offers: inviting friends (D26, D30) -------------------------------------------------------------------

    private fun TestScope.withFriends(): Harness = started().also { h ->
        h.cards(snapshot("u2" to friendDoc("Ann"), "u3" to friendDoc("Bo"), "u4" to requestDoc("incoming", "Cy")))
        runCurrent()
    }

    @Test fun invitingSendsOfferTeamAndMarksTheRowUntilThePickerCloses() = runTest {
        val h = withFriends()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.offerTeam("t1", "u2")
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        val request = h.requests.single()
        assertEquals(setOf("action", "teamId", "friendUid", "requestId"), request.body?.keys)
        assertEquals("offerTeam", request.body?.get("action"))
        assertEquals("t1", request.body?.get("teamId"))
        assertEquals("u2", request.body?.get("friendUid"))
        assertTrue(FriendsLogic.isRequestId(request.body?.get("requestId") as String))
        assertEquals("disabled while it runs", InviteRow(busy = true), h.state.invites["u2"])
        gate.complete(JSONObject().put("ok", true))
        runCurrent()
        assertEquals(InviteRow(sentTo = "Ann"), h.state.invites["u2"])
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertEquals("an invited row stays disabled", 1, h.requests.size)
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.offerTeam("t1", "u3")
        runCurrent()
        assertEquals("the friends invited meanwhile, in the order they were invited", listOf("Ann", "Bo"), h.workspace.closeInvites())
        assertTrue(h.state.invites.isEmpty())
        assertNull("the one-shot belongs to the Teams tab, not to the Friends notice", h.state.notice)
        assertEquals("nothing is left to announce twice", emptyList<String>(), h.workspace.closeInvites())
    }

    @Test fun anInviteUsesTheNameTheFriendHasNow() = runTest {
        val h = withFriends()
        h.profile("u2", "Ann Lee", "annlee")
        h.workspace.refreshName("u2")
        runCurrent()
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertEquals(listOf("Ann Lee"), h.workspace.closeInvites())
    }

    @Test fun onlyAFriendIsInvited() = runTest {
        val h = withFriends()
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.offerTeam("t1", "u4")
        h.workspace.offerTeam("t1", "stranger")
        runCurrent()
        assertTrue(h.requests.isEmpty() && h.state.invites.isEmpty())
    }

    @Test fun anInviteKeepsItsRequestIdOnlyAfterAnUnknownOutcome() = runTest {
        val h = withFriends()
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertEquals(InviteRow(error = FriendsError.Network), h.state.invites["u2"])
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        val first = h.requests[0].body?.get("requestId")
        assertEquals(first, h.requests[1].body?.get("requestId"))
        h.reply = { throw TeamMessageFailure("OFFER_LIMIT") }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertEquals(first, h.requests[2].body?.get("requestId"))
        assertEquals(InviteRow(error = FriendsError.OfferLimit), h.state.invites["u2"])
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertNotEquals("a definite answer drops the id", first, h.requests[3].body?.get("requestId"))
        assertEquals(InviteRow(sentTo = "Ann"), h.state.invites["u2"])
        assertEquals("another team is another request", "t2", run { h.workspace.closeInvites(); h.workspace.offerTeam("t2", "u3"); runCurrent(); h.requests.last().body?.get("teamId") })
    }

    @Test fun inviteErrorsShowUnderTheRowWithTheOfferTexts() = runTest {
        val h = withFriends()
        for ((code, error) in listOf(
            "FRIEND_REQUIRED" to FriendsError.FriendRequired, "MEMBER_LIMIT" to FriendsError.MemberLimit, "ROLE_REQUIRED" to FriendsError.OfferRole,
            "MEMBERSHIP_REQUIRED" to FriendsError.OfferTeamGone, "RATE_LIMITED" to FriendsError.RateLimited, "TEAM_LIMIT" to FriendsError.Generic,
        )) {
            h.reply = { throw TeamMessageFailure(code) }
            h.workspace.offerTeam("t1", "u2")
            runCurrent()
            assertEquals(code, InviteRow(error = error), h.state.invites["u2"])
        }
        assertTrue("nothing was announced", h.workspace.closeInvites().isEmpty())
    }

    @Test fun anInviteToSomeoneAlreadyInTheTeamAsksForTheMemberListOnce() = runTest {
        val h = withFriends()
        h.reply = { throw TeamMessageFailure("ALREADY_MEMBER") }
        var refreshed = 0
        h.workspace.offerTeam("t1", "u2") { refreshed++ }
        runCurrent()
        assertEquals(1, refreshed)
        assertEquals(InviteRow(error = FriendsError.AlreadyMember), h.state.invites["u2"])
        h.reply = { throw TeamMessageFailure("OFFER_LIMIT") }
        h.workspace.offerTeam("t1", "u3") { refreshed++ }
        runCurrent()
        assertEquals("only ALREADY_MEMBER refreshes the members", 1, refreshed)
    }

    @Test fun aPickerClosedWhileAnInviteRunsLeavesNoMarkBehind() = runTest {
        val h = withFriends()
        val gate = CompletableDeferred<JSONObject>()
        h.reply = { gate.await() }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertTrue("an invite that has not gone out is not announced", h.workspace.closeInvites().isEmpty())
        gate.complete(JSONObject().put("ok", true))
        runCurrent()
        assertTrue("the next picker starts clean", h.state.invites.isEmpty())
        h.reply = { JSONObject().put("ok", true) }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertEquals(2, h.requests.size)
        assertEquals(InviteRow(sentTo = "Ann"), h.state.invites["u2"])
    }

    @Test fun closingThePickerForgetsTheRequestIdsOfAnUnknownOutcome() = runTest {
        val h = withFriends()
        h.reply = { throw TeamMessageFailure("SERVICE_UNAVAILABLE") }
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        h.workspace.closeInvites()
        h.workspace.offerTeam("t1", "u2")
        runCurrent()
        assertNotEquals(h.requests[0].body?.get("requestId"), h.requests[1].body?.get("requestId"))
    }

    private companion object {
        const val UID = "u1"
        /** The device clock of the offer tests: the vectors' clock, whole epoch milliseconds. */
        const val NOW = 1_762_000_000_000L
        fun JSONObject.asMap(): Map<String, Any?> = keys().asSequence().associateWith { opt(it) }
        fun <T> anyListener(): EventListener<T> = any<EventListener<T>>() ?: EventListener { _, _ -> }
        fun anyAuthListener(): FirebaseAuth.AuthStateListener = any<FirebaseAuth.AuthStateListener>() ?: FirebaseAuth.AuthStateListener { }
    }
}
