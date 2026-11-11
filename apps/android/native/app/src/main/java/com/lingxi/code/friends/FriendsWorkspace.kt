package com.lingxi.code.friends

import android.content.Context
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.ListenerRegistration
import com.lingxi.code.teams.TEAMS_API_ROOT
import com.lingxi.code.teams.TeamHttp
import com.lingxi.code.teams.firestoreMillis
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/** One call in flight, or the error it ended with (D18: shown under the control that failed). */
internal data class FriendsOp(val busy: Boolean = false, val error: FriendsError? = null)

/** What a friend's own profile says now (D10); it replaces the name on the card for the rest of the session. */
internal data class FriendName(val displayName: String, val handle: String?)

/** A one-line result the Friends tab shows once, wherever the person is in it. A sent request is not one: the screen that sent it says so itself. */
internal sealed interface FriendsNotice {
    data class NowFriends(val name: String) : FriendsNotice
    /** A link or a push named a chat that is not a current friend's (D16). */
    data object ChatUnavailable : FriendsNotice

    /** A Team invitation was accepted (D27); shown once in Friends, which stays where it is. */
    data class OfferJoined(val teamName: String) : FriendsNotice
}

/**
 * One row of the Invite friends picker (D26): the call in flight, the error it ended with, or [sentTo], the friend's name, once the offer went
 * out. The mark lasts until the picker closes: the sender can never read an offer, so nothing else knows it was made.
 */
internal data class InviteRow(val busy: Boolean = false, val error: FriendsError? = null, val sentTo: String? = null)

/**
 * Everything the Friends screens read. [cards], [blocks] and [offers] are what the three Firestore listeners last delivered (unordered: the
 * screens sort with the collator of their UI language and the clock); everything else is memory only and starts over with every account (D19).
 */
internal data class FriendsState(
    val uid: String? = null,
    val cards: List<FriendCard> = emptyList(),
    val cardsLoaded: Boolean = false,
    val cardsFailed: Boolean = false,
    val blocks: List<BlockEntry> = emptyList(),
    val blocksLoaded: Boolean = false,
    val blocksFailed: Boolean = false,
    /** The Team invitations (D27). A failed offers listener ends that watch and leaves nothing here: it shows nothing and counts nothing. */
    val offers: List<TeamOffer> = emptyList(),
    val offersFailed: Boolean = false,
    /**
     * The device clock the Team invitations are read against (D27, D28): read at sign-in, with every offers snapshot, at the earliest live expiry
     * and whenever the Friends view is shown, never on its own. The badge and the Team invitations section both read it.
     */
    val nowMs: Long = 0L,
    val profile: FriendProfile? = null,
    /** `GET /v1/friends/profile`. */
    val profileOp: FriendsOp = FriendsOp(),
    /** The `profile` action behind the Save button. */
    val saveOp: FriendsOp = FriendsOp(),
    /** `rotateCode` and `revokeCode` of the friend link card. */
    val linkOp: FriendsOp = FriendsOp(),
    /** The row actions (accept, decline, cancel, remove, block, unblock) in flight or failed, by the other person's uid. */
    val rowOps: Map<String, FriendsOp> = emptyMap(),
    /** The Team invitation row actions (join, decline, dismiss) in flight or failed, by team id (D27). */
    val offerOps: Map<String, FriendsOp> = emptyMap(),
    /** The Invite friends picker's rows by the friend's uid, in the order they were invited (D26). Empty while no picker is open. */
    val invites: Map<String, InviteRow> = emptyMap(),
    val names: Map<String, FriendName> = emptyMap(),
    val notice: FriendsNotice? = null,
    /** The friend whose chat is open and in front (D13): its unread shows as 0 in the list and the badge. */
    val openChatUid: String? = null,
) {
    /** The card of [uid] in any state, as the screens show it. */
    fun card(uid: String): FriendCard? = cards.firstOrNull { it.uid == uid }?.let(::resolved)

    fun friendCard(uid: String): FriendCard? = card(uid)?.takeIf { it.state == FriendState.Friends }

    /** The cards as the screens show them: a friend's name and username from the session's profile reads (D10) where those differ. */
    fun resolvedCards(): List<FriendCard> = cards.map(::resolved)

    private fun resolved(card: FriendCard): FriendCard {
        val name = names[card.uid] ?: return card
        return card.copy(displayName = name.displayName, handle = name.handle)
    }
}

/**
 * The app-level Friends workspace: one per process, started by the first read of [FriendsBadge.count] (the shell reads it at
 * composition) and kept for the life of the process; an account change tears everything down (D19).
 *
 * Live data is the three owner-only listeners of D2, `users/{uid}/friends`, `users/{uid}/blocks` and `users/{uid}/teamOffers`, and nothing
 * per friend. Everything else is HTTP with the Firebase ID token through [TeamHttp] (`GET /v1/friends/profile`, `POST /v1/friends`) and one
 * `get` of `profiles/{uid}` per name refresh. Chat data is not here: a chat runs on its own pair-pinned `TeamsWorkspace` (D9). [clock] is
 * the device clock the offers' expiry is read against (D27).
 */
internal class FriendsWorkspace(
    private val scope: CoroutineScope,
    private val auth: FirebaseAuth,
    private val db: FirebaseFirestore,
    private val apiRoot: String = TEAMS_API_ROOT,
    private val clock: () -> Long = System::currentTimeMillis,
    private val http: suspend (url: String, token: String, body: JSONObject?) -> JSONObject = TeamHttp::request,
) {
    private val mutable = MutableStateFlow(FriendsState())
    val state: StateFlow<FriendsState> = mutable.asStateFlow()

    /** The Friends tab badge (D14, D28): incoming requests, the unread of every friend and every live team offer; 0 while signed out. */
    val badge: StateFlow<Int> = mutable.map { FriendsLogic.badgeCount(it.cards, it.offers, it.nowMs, it.openChatUid) }.stateIn(scope, SharingStarted.Eagerly, 0)

    /** The Add friend screen's lookup, a friend link opened from outside, and a Team member's Add friend (D6, D8, D15). */
    val search = newLookup()
    val link = newLookup()
    val member = newLookup()

    private val listeners = mutableMapOf<String, ListenerRegistration>()
    private val authListener = FirebaseAuth.AuthStateListener { accountChanged() }
    /** Everything launched for the current account: cancelled with it. */
    private var session: CoroutineScope = newSession()
    private var epoch = 0L
    private var started = false
    /** The one timer of the badge: it fires at the earliest live expiry (D28). */
    private var expiry: Job? = null
    /** The request ids of `offerTeam` and `acceptOffer` calls whose outcome is unknown, by what they were minted for; reused only to retry that call (D18, D30). Memory only. */
    private val attempts = mutableMapOf<String, String>()
    /** Counts the pickers closed: an invite that finishes after its picker closed leaves no mark behind (D26). */
    private var pickers = 0

    fun start() {
        if (started) return
        started = true
        auth.addAuthStateListener(authListener)
        accountChanged()
    }

    private fun newSession() = CoroutineScope(scope.coroutineContext + SupervisorJob(scope.coroutineContext[Job]))
    private fun newLookup() = FriendLookup(scope, ::post, ::awaitFriend, ::announce)

    private fun accountChanged() {
        val uid = auth.currentUser?.uid
        if (uid == mutable.value.uid) return
        epoch++
        session.cancel(); session = newSession()
        expiry = null; attempts.clear()
        listeners.values.forEach { it.remove() }; listeners.clear()
        search.clear(); link.clear(); member.clear()
        mutable.value = FriendsState(uid = uid, nowMs = clock())
        if (uid != null) { listenCards(uid); listenBlocks(uid); listenOffers(uid) }
    }

    // --- the three listeners (D2) -----------------------------------------------------------------------

    private fun listenCards(uid: String) {
        listeners.remove(CARDS)?.remove()
        val expected = epoch
        mutable.update { it.copy(cardsFailed = false) }
        listeners[CARDS] = db.collection("users").document(uid).collection("friends").addSnapshotListener { snapshot, error ->
            if (expected != epoch) return@addSnapshotListener
            if (error != null || snapshot == null) { mutable.update { it.copy(cardsFailed = true) }; return@addSnapshotListener }
            // A document that cannot be placed is skipped: one bad card never fails the list (D13).
            val cards = snapshot.documents.mapNotNull { document -> document.data?.let { FriendCard.fromDocument(document.id, it, ::firestoreMillis) } }
            mutable.update { state ->
                val friends = cards.filter { it.state == FriendState.Friends }.map { it.uid }.toSet()
                state.copy(cards = cards, cardsLoaded = true, cardsFailed = false, names = state.names.filterKeys { it in friends })
            }
        }
    }

    private fun listenBlocks(uid: String) {
        listeners.remove(BLOCKS)?.remove()
        val expected = epoch
        mutable.update { it.copy(blocksFailed = false) }
        listeners[BLOCKS] = db.collection("users").document(uid).collection("blocks").addSnapshotListener { snapshot, error ->
            if (expected != epoch) return@addSnapshotListener
            if (error != null || snapshot == null) { mutable.update { it.copy(blocksFailed = true) }; return@addSnapshotListener }
            val blocks = snapshot.documents.mapNotNull { document -> document.data?.let { BlockEntry.fromDocument(document.id, it, ::firestoreMillis) } }
            mutable.update { it.copy(blocks = blocks, blocksLoaded = true, blocksFailed = false) }
        }
    }

    /**
     * The Team invitations (D27). A document that is not a whole offer is skipped; a failed listener ends that watch, shows nothing and counts
     * nothing until [retry]. Every snapshot also moves the badge's clock and its timer, since a new offer may expire earlier than the last one.
     */
    private fun listenOffers(uid: String) {
        listeners.remove(OFFERS)?.remove()
        val expected = epoch
        mutable.update { it.copy(offersFailed = false) }
        listeners[OFFERS] = db.collection("users").document(uid).collection("teamOffers").addSnapshotListener { snapshot, error ->
            if (expected != epoch) return@addSnapshotListener
            if (error != null || snapshot == null) {
                mutable.update { it.copy(offers = emptyList(), offersFailed = true, offerOps = emptyMap()) }
            } else {
                val offers = snapshot.documents.mapNotNull { document -> document.data?.let { TeamOffer.fromDocument(document.id, it, ::firestoreMillis) } }
                mutable.update { state ->
                    val ids = offers.map { it.teamId }.toSet()
                    state.copy(offers = offers, offersFailed = false, offerOps = state.offerOps.filterKeys { it in ids })
                }
            }
            refreshClock()
        }
    }

    /** The list's and the blocked screen's Retry: a listener that ended with an error starts again, and so does a failed offers watch (D27). */
    fun retry() {
        val current = mutable.value
        val uid = current.uid ?: return
        if (current.cardsFailed) listenCards(uid)
        if (current.blocksFailed) listenBlocks(uid)
        if (current.offersFailed) listenOffers(uid)
    }

    // --- the clock of the badge (D28) -------------------------------------------------------------------------

    /**
     * Reads the device clock again and sets the one timer for the earliest live expiry: an offer that expires changes the badge without any
     * document changing. Also run whenever the Friends view is shown, which covers a device that slept through a timer.
     */
    fun refreshClock() {
        val now = clock()
        mutable.update { it.copy(nowMs = now) }
        expiry?.cancel(); expiry = null
        val next = mutable.value.offers.map { it.expiresAtMs }.filter { it > now }.minOrNull() ?: return
        expiry = session.launch { delay(next - now); expiry = null; refreshClock() }
    }

    // --- HTTP ---------------------------------------------------------------------------------------------

    private suspend fun call(path: String, body: JSONObject?): JSONObject {
        val uid = requireNotNull(mutable.value.uid)
        val user = requireNotNull(auth.currentUser)
        check(user.uid == uid)
        val token = requireNotNull(user.getIdToken(false).await().token)
        check(auth.currentUser?.uid == uid)
        val result = http("$apiRoot/v1/friends$path", token, body)
        check(auth.currentUser?.uid == uid)
        return result
    }

    private suspend fun post(body: Map<String, Any>): JSONObject = call("", JSONObject(body))

    /** The new friend's card carries the pair team the chat needs; wait for it, but not forever. */
    private suspend fun awaitFriend(friendUid: String) {
        withTimeoutOrNull(CARD_WAIT_MS) { mutable.first { state -> state.friendCard(friendUid) != null } }
    }

    private fun announce(outcome: FriendOutcome) {
        if (outcome is FriendOutcome.NowFriends) mutable.update { it.copy(notice = FriendsNotice.NowFriends(outcome.name)) }
    }

    fun clearNotice(notice: FriendsNotice) { mutable.update { if (it.notice == notice) it.copy(notice = null) else it } }

    /** A link or a push named a chat that is not a current friend's. */
    fun chatUnavailable() { mutable.update { it.copy(notice = FriendsNotice.ChatUnavailable) } }

    // --- my profile and my friend link (D2, D6, D11) -------------------------------------------------------

    /** `GET /v1/friends/profile`, which creates the profile and the first friend code on first use. Asked on every open of Add friend and Profile settings. */
    fun loadProfile() {
        if (mutable.value.uid == null || mutable.value.profileOp.busy) return
        mutable.update { it.copy(profileOp = FriendsOp(busy = true)) }
        session.launch {
            try {
                val profile = FriendProfile.parse(call("/profile", null))
                mutable.update { it.copy(profile = profile, profileOp = FriendsOp()) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutable.update { it.copy(profileOp = FriendsOp(error = FriendsError.of(error))) } }
        }
    }

    /** One `profile` action with only the fields that changed (D11). */
    fun saveProfile(form: ProfileForm) {
        val current = mutable.value
        val profile = current.profile ?: return
        val changes = form.changes(profile)
        if (changes.isEmpty() || !form.valid || current.saveOp.busy) return
        mutable.update { it.copy(saveOp = FriendsOp(busy = true)) }
        session.launch {
            try {
                val saved = FriendProfile.parse(post(changes + ("action" to "profile")))
                mutable.update { it.copy(profile = saved, saveOp = FriendsOp()) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutable.update { it.copy(saveOp = FriendsOp(error = FriendsError.of(error))) } }
        }
    }

    /** A new friend link; the old one stops working at once. Also creates the first one after [revokeCode]. */
    fun rotateCode() = linkAction("rotateCode") { answer, profile -> profile.copy(friendCode = FriendCode(answer.getString("code"), answer.getString("link"))) }

    fun revokeCode() = linkAction("revokeCode") { _, profile -> profile.copy(friendCode = null) }

    private fun linkAction(action: String, apply: (JSONObject, FriendProfile) -> FriendProfile) {
        val current = mutable.value
        if (current.profile == null || current.linkOp.busy) return
        mutable.update { it.copy(linkOp = FriendsOp(busy = true)) }
        session.launch {
            try {
                val answer = post(mapOf("action" to action))
                mutable.update { state -> state.copy(profile = state.profile?.let { apply(answer, it) }, linkOp = FriendsOp()) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutable.update { it.copy(linkOp = FriendsOp(error = FriendsError.of(error))) } }
        }
    }

    // --- request rows, the friend menu and the blocked list (D4, D10, D12) -----------------------------------

    /** Accept shows "now friends" once and stays on the list (D4). */
    fun accept(otherUid: String) {
        val name = mutable.value.card(otherUid)?.displayName.orEmpty()
        act(otherUid, "accept") { it.copy(notice = FriendsNotice.NowFriends(name)) }
    }

    fun decline(otherUid: String) = act(otherUid, "decline")

    fun cancel(otherUid: String) = act(otherUid, "cancel")

    /** [onDone] runs on success: the chat of the removed friend closes. */
    fun remove(otherUid: String, onDone: () -> Unit) = act(otherUid, "remove", onDone)

    fun block(otherUid: String, onDone: () -> Unit) = act(otherUid, "block", onDone)

    fun unblock(otherUid: String) = act(otherUid, "unblock")

    fun clearRowError(otherUid: String) { mutable.update { state -> state.copy(rowOps = if (state.rowOps[otherUid]?.busy == false) state.rowOps - otherUid else state.rowOps) } }

    /** `{action, otherUid}`: the listeners show the result, so only the call itself is tracked, per person (a row is disabled while its action is in flight). */
    private fun act(otherUid: String, action: String, onDone: () -> Unit = {}, success: (FriendsState) -> FriendsState = { it }) {
        if (mutable.value.uid == null || mutable.value.rowOps[otherUid]?.busy == true) return
        mutable.update { it.copy(rowOps = it.rowOps + (otherUid to FriendsOp(busy = true))) }
        session.launch {
            try {
                post(mapOf("action" to action, "otherUid" to otherUid))
                mutable.update { success(it).let { state -> state.copy(rowOps = state.rowOps - otherUid) } }
                onDone()
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutable.update { it.copy(rowOps = it.rowOps + (otherUid to FriendsOp(error = FriendsError.of(error)))) } }
        }
    }

    // --- team offers: the invitations I got and the ones I send (D26, D27, D30) --------------------------------------

    /**
     * Joins the team of an invitation (D27). The offer must look live: one that looks expired is never sent, and the server decides the rest
     * (OFFER_UNAVAILABLE means it deleted the offer, and the listener drops the row). The request id is minted at the tap and reused only to retry
     * after an unknown outcome (D18, D30). [onJoined] runs on success, with the replay of an earlier join (`alreadyMember`) counting as one:
     * the Friends tab refreshes the Teams list there, as a link join does.
     */
    fun acceptOffer(teamId: String, onJoined: () -> Unit = {}) {
        val current = mutable.value
        val offer = current.offers.firstOrNull { it.teamId == teamId } ?: return
        if (current.uid == null || current.offerOps[teamId]?.busy == true || FriendsLogic.offerExpiry(offer.expiresAtMs, clock()) == OfferExpiry.Expired) return
        val attempt = "accept:$teamId"
        val requestId = attempts.getOrPut(attempt) { FriendsLogic.newRequestId() }
        setOfferOp(teamId, FriendsOp(busy = true))
        session.launch {
            try {
                post(mapOf("action" to OfferAction.AcceptOffer.wire, "offerId" to teamId, "requestId" to requestId))
                attempts.remove(attempt)
                mutable.update { it.copy(offerOps = it.offerOps - teamId, notice = FriendsNotice.OfferJoined(offer.teamName)) }
                onJoined()
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                val failure = FriendsError.ofOffer(OfferAction.AcceptOffer, error)
                if (failure.definitive) attempts.remove(attempt)
                setOfferOp(teamId, FriendsOp(error = failure))
            }
        }
    }

    /** Declines an invitation, or dismisses an expired one (D27): no confirmation and no request id, and safe to repeat. The listener drops the row. */
    fun declineOffer(teamId: String) {
        val current = mutable.value
        if (current.uid == null || current.offers.none { it.teamId == teamId } || current.offerOps[teamId]?.busy == true) return
        setOfferOp(teamId, FriendsOp(busy = true))
        session.launch {
            try {
                post(mapOf("action" to OfferAction.DeclineOffer.wire, "offerId" to teamId))
                mutable.update { it.copy(offerOps = it.offerOps - teamId) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { setOfferOp(teamId, FriendsOp(error = FriendsError.ofOffer(OfferAction.DeclineOffer, error))) }
        }
    }

    /** A row's state is only kept while its offer is there: one the listener already dropped needs no error of its own. */
    private fun setOfferOp(teamId: String, op: FriendsOp) {
        mutable.update { if (it.offers.any { offer -> offer.teamId == teamId }) it.copy(offerOps = it.offerOps + (teamId to op)) else it }
    }

    /**
     * Invites one friend of the picker to [teamId] (D26): always as a member, with no message. The row is disabled while the call runs and, once
     * it went out, shows "invited" until the picker closes ([closeInvites]). The request id is minted at the tap and reused only to retry after an
     * unknown outcome (D18, D30). ALREADY_MEMBER means the person joined meanwhile: [onAlreadyMember] refreshes the member list, and the picker
     * drops the row itself.
     */
    fun offerTeam(teamId: String, friendUid: String, onAlreadyMember: () -> Unit = {}) {
        val current = mutable.value
        val name = current.friendCard(friendUid)?.displayName ?: return
        if (current.uid == null || current.invites[friendUid]?.let { it.busy || it.sentTo != null } == true) return
        val attempt = "offer:$teamId:$friendUid"
        val requestId = attempts.getOrPut(attempt) { FriendsLogic.newRequestId() }
        val picker = pickers
        fun mark(row: InviteRow) { if (picker == pickers) mutable.update { it.copy(invites = it.invites + (friendUid to row)) } }
        mark(InviteRow(busy = true))
        session.launch {
            try {
                post(mapOf("action" to OfferAction.OfferTeam.wire, "teamId" to teamId, "friendUid" to friendUid, "requestId" to requestId))
                attempts.remove(attempt)
                mark(InviteRow(sentTo = name))
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                val failure = FriendsError.ofOffer(OfferAction.OfferTeam, error)
                if (failure.definitive) attempts.remove(attempt)
                mark(InviteRow(error = failure))
                if (failure == FriendsError.AlreadyMember) onAlreadyMember()
            }
        }
    }

    /**
     * The Invite friends picker closed (D26): its marks and request ids go. Returns the names of the friends invited meanwhile, in the order they
     * were invited, for the one-shot "invitation sent" the Teams tab shows; nothing else remembers them, because the sender can never read an
     * offer. Opening the picker again lists them again: inviting again refreshes the same offer.
     */
    fun closeInvites(): List<String> {
        pickers++
        attempts.keys.removeAll { it.startsWith("offer:") }
        val sent = mutable.value.invites.values.mapNotNull { it.sentTo }
        mutable.update { it.copy(invites = emptyMap()) }
        return sent
    }

    // --- names (D10) and the open chat (D13) ----------------------------------------------------------------

    /**
     * Reads `profiles/{uid}` once when a friend's chat or profile opens, and keeps its name and username for the session where they
     * differ from the card; the card itself is never written. A failure is ignored.
     */
    fun refreshName(friendUid: String) {
        session.launch {
            try {
                val profile = db.collection("profiles").document(friendUid).get().await()
                val name = profile.getString("displayName")?.takeIf { it.isNotEmpty() } ?: return@launch
                val handle = profile.getString("handle")?.takeIf { it.isNotEmpty() }
                mutable.update { state ->
                    val card = state.cards.firstOrNull { it.uid == friendUid && it.state == FriendState.Friends } ?: return@update state
                    state.copy(names = if (card.displayName == name && card.handle == handle) state.names - friendUid else state.names + (friendUid to FriendName(name, handle)))
                }
            } catch (error: CancellationException) { throw error }
            catch (_: Exception) { /* D10: the card's name stays. */ }
        }
    }

    fun chatOpened(friendUid: String) { mutable.update { it.copy(openChatUid = friendUid) } }

    fun chatClosed(friendUid: String) { mutable.update { if (it.openChatUid == friendUid) it.copy(openChatUid = null) else it } }

    companion object {
        private const val CARDS = "friends"
        private const val BLOCKS = "blocks"
        private const val OFFERS = "teamOffers"
        private const val CARD_WAIT_MS = 8_000L

        @Volatile private var instance: FriendsWorkspace? = null

        /** The process-wide workspace, started on first use. */
        fun get(context: Context): FriendsWorkspace = instance ?: synchronized(this) {
            instance ?: FriendsWorkspace(CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate), FirebaseAuth.getInstance(), FirebaseFirestore.getInstance())
                .also { instance = it; it.start() }
        }
    }
}
