package com.lingxi.code.friends

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.json.JSONObject

/** How a confirmed lookup ended (D7). */
internal sealed interface FriendOutcome {
    /** A request went out and waits for the other person. */
    data class RequestSent(val name: String) : FriendOutcome

    /** The friendship exists: `redeem`, `accept` and a mutual `request` all end here, and the caller opens the chat. */
    data class NowFriends(val uid: String, val name: String) : FriendOutcome
}

/**
 * What a lookup flow shows: the proof being looked up, the card it found, and what the confirming tap is doing. [error] belongs to the
 * last step that failed; a failed confirmation whose outcome is unknown ([FriendsError.definitive] false) may be tapped again.
 */
internal data class LookupState(
    val proof: FriendProof? = null,
    val result: LookupResult? = null,
    val looking: Boolean = false,
    val confirming: Boolean = false,
    val error: FriendsError? = null,
    val outcome: FriendOutcome? = null,
)

/**
 * One "look somebody up, then ask" flow (D6, D7, D8, D15). The Add friend screen, a friend link opened from outside and a Team member's
 * Add friend each have their own, so they never trample each other. The lookup only fills the card; nothing is requested, accepted or
 * redeemed until [confirm]. [awaitFriend] returns once the new friend's card has arrived (or the wait gave up), so the chat it opens has
 * its pair team; [announce] tells the workspace how a confirmation ended.
 */
internal class FriendLookup(
    private val scope: CoroutineScope,
    private val post: suspend (Map<String, Any>) -> JSONObject,
    private val awaitFriend: suspend (uid: String) -> Unit,
    private val announce: (FriendOutcome) -> Unit,
) {
    private val mutable = MutableStateFlow(LookupState())
    val state: StateFlow<LookupState> = mutable.asStateFlow()
    private var job: Job? = null
    private var attempt: Attempt? = null

    /** The request id minted for a confirming tap, with what it was minted for. */
    private data class Attempt(val proof: FriendProof, val note: String, val requestId: String)

    /** Looks [proof] up. A new lookup replaces the card and drops the request id of the old one. */
    fun lookup(proof: FriendProof) {
        job?.cancel()
        attempt = null
        mutable.value = LookupState(proof = proof, looking = true)
        job = scope.launch {
            try {
                val result = LookupResult.parse(post(FriendsLogic.lookupBody(proof)))
                mutable.update { it.copy(result = result, looking = false) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { mutable.update { it.copy(looking = false, error = FriendsError.of(error)) } }
        }
    }

    /**
     * The confirming tap: `redeem` for a friend link, `accept` when the other person asked first, otherwise `request` (with [note], never
     * for a link). A friend, or a request already sent, has nothing to confirm. The request id is minted now and reused only to retry this
     * same confirmation after an unknown outcome; it is never persisted (D18).
     */
    fun confirm(note: String) {
        val current = mutable.value
        val proof = current.proof ?: return
        val found = current.result ?: return
        if (current.looking || current.confirming) return
        val body = when {
            found.relation == FriendRelation.Friends -> return
            proof is FriendProof.Code -> confirmation(proof, "")
            found.relation == FriendRelation.Incoming -> mapOf("action" to "accept", "otherUid" to found.uid)
            found.relation == FriendRelation.Outgoing -> return
            else -> confirmation(proof, note)
        }
        mutable.update { it.copy(confirming = true, error = null) }
        job = scope.launch {
            try {
                val answer = post(body)
                val friendUid = answer.optString("uid").ifEmpty { found.uid }
                val outcome = if (answer.getString("state") == "outgoing") FriendOutcome.RequestSent(found.displayName) else FriendOutcome.NowFriends(friendUid, found.displayName)
                if (outcome is FriendOutcome.NowFriends) awaitFriend(friendUid)
                attempt = null
                announce(outcome)
                mutable.update { it.copy(confirming = false, outcome = outcome) }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) {
                val failure = FriendsError.of(error)
                if (failure.definitive) attempt = null
                mutable.update { it.copy(confirming = false, error = failure) }
            }
        }
    }

    private fun confirmation(proof: FriendProof, note: String): Map<String, Any> {
        val id = attempt?.takeIf { it.proof == proof && it.note == note }?.requestId ?: FriendsLogic.newRequestId()
        attempt = Attempt(proof, note, id)
        return FriendsLogic.confirmBody(proof, id, note)
    }

    /** Drops the card, the code and any request id: editing the field, closing the sheet, cancelling, or an account change (D19). */
    fun clear() {
        job?.cancel()
        job = null
        attempt = null
        mutable.value = LookupState()
    }
}
