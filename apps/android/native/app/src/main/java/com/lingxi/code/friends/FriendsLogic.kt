package com.lingxi.code.friends

import com.lingxi.code.teams.TeamLastMessage
import com.lingxi.code.teams.TeamMember
import com.lingxi.code.teams.TeamMessageFailure
import com.lingxi.code.teams.TeamSummary
import com.lingxi.code.teams.TeamsComposeLogic
import com.lingxi.code.teams.TeamsPointerParser
import com.lingxi.code.teams.stringOrNull
import com.lingxi.code.tabs.TabLogic
import org.json.JSONObject
import java.net.URI
import java.text.Collator
import java.text.Normalizer
import java.util.Locale
import java.util.UUID

/** Where a card stands. The rank is the order of the whole `users/{uid}/friends` collection (D5): incoming requests, outgoing requests, friends. */
internal enum class FriendState(val wire: String, val rank: Int) {
    Incoming("incoming", 0), Outgoing("outgoing", 1), Friends("friends", 2);

    companion object {
        fun of(wire: Any?): FriendState? = entries.firstOrNull { it.wire == wire }
    }
}

/** The pair team behind a friendship's chat. Both parts come from the card, never from a constant (D9). */
internal data class PairRef(val teamId: String, val generation: Long)

/**
 * One document of `users/{uid}/friends` (docs/FRIENDS-CLIENTS.md, appendix A): a friend or a request as the backend wrote it.
 * The chat summary (`lastSeq`, `lastReadSeq`, `lastMessage`; D13) is absent until a message or a read mark exists, which reads
 * as no message and nothing unread.
 */
internal data class FriendCard(
    val uid: String, val state: FriendState, val displayName: String, val handle: String?, val note: String?, val sinceMs: Long,
    val pair: PairRef? = null, val lastSeq: Long = 0, val lastReadSeq: Long = 0, val lastMessage: TeamLastMessage? = null,
) {
    /** `lastMessage.at` in whole epoch milliseconds; a pointer whose time is not stamped counts as no message for the order (D5). */
    val lastAtMs: Long? get() = lastMessage?.atMs

    companion object {
        /**
         * null for a document that cannot be placed: an unknown state, or a friend without its pair team. A `lastMessage` that is not
         * the whole pointer only counts as absent, so one bad card never fails the list (D13). [millis] turns a Firestore timestamp into
         * epoch milliseconds and returns null for any other value.
         */
        fun fromDocument(uid: String, data: Map<String, Any?>, millis: (Any) -> Long?): FriendCard? {
            if (uid.isEmpty()) return null
            val state = FriendState.of(data["state"]) ?: return null
            val pair = if (state == FriendState.Friends) {
                PairRef((data["teamId"] as? String)?.takeIf { it.isNotEmpty() } ?: return null, (data["generation"] as? Number)?.toLong() ?: return null)
            } else null
            fun counter(key: String) = (data[key] as? Number)?.toLong()?.coerceAtLeast(0) ?: 0
            val lastMessage = try { TeamsPointerParser.parseLastMessage(true, data["lastMessage"], millis) } catch (_: TeamMessageFailure) { null }
            return FriendCard(
                uid, state, data["displayName"] as? String ?: "", (data["handle"] as? String)?.takeIf { it.isNotEmpty() },
                (data["note"] as? String)?.takeIf { it.isNotEmpty() }, data["since"]?.let(millis) ?: 0,
                pair, counter("lastSeq"), counter("lastReadSeq"), lastMessage,
            )
        }
    }
}

/** One document of `users/{uid}/blocks`: the name is a snapshot the backend wrote when the block was made (D12). */
internal data class BlockEntry(val uid: String, val atMs: Long, val displayName: String, val handle: String?) {
    companion object {
        fun fromDocument(uid: String, data: Map<String, Any?>, millis: (Any) -> Long?) = BlockEntry(
            uid, data["at"]?.let(millis) ?: 0, FriendsLogic.jsTrim(data["displayName"] as? String ?: ""), (data["handle"] as? String)?.takeIf { it.isNotEmpty() },
        )
    }
}

/**
 * One document of `users/{uid}/teamOffers` (docs/FRIENDS-CLIENTS.md D27, appendix A): a friend's offer of an ordinary team to the owner.
 * [teamId] is the document id, which is also what `acceptOffer` and `declineOffer` call the offer; [teamName] and [fromName] are
 * snapshots from when the offer was made; [expiresAtMs] is whole epoch milliseconds. The role is always `member` and is not kept.
 */
internal data class TeamOffer(val teamId: String, val teamName: String, val fromUid: String, val fromName: String, val expiresAtMs: Long) {
    companion object {
        /**
         * null for a document that is not a whole offer: one without the four strings (`teamId`, `teamName`, `fromUid`, `fromName`) or without
         * a readable `expiresAt` is skipped, never failing the list (D27). [millis] turns a Firestore timestamp into epoch milliseconds and
         * returns null for any other value.
         */
        fun fromDocument(id: String, data: Map<String, Any?>, millis: (Any) -> Long?): TeamOffer? {
            if (id.isEmpty() || data["teamId"] !is String) return null
            return TeamOffer(
                id, data["teamName"] as? String ?: return null, data["fromUid"] as? String ?: return null, data["fromName"] as? String ?: return null,
                data["expiresAt"]?.let(millis) ?: return null,
            )
        }
    }
}

/** How a live offer's time left reads (D27): [unit] and [value] go to the platform's relative-time formatter, in the future direction. */
internal enum class OfferUnit { Hour, Day }

/** An offer's display state: strictly after now is live, anything else is expired (the server decides, and keeps an expired offer 7 days more). */
internal sealed interface OfferExpiry {
    data object Expired : OfferExpiry

    data class Live(val unit: OfferUnit, val value: Int) : OfferExpiry
}

/** The three offer actions of `POST /v1/friends` (D26, D27, D30). */
internal enum class OfferAction(val wire: String) {
    OfferTeam("offerTeam"), AcceptOffer("acceptOffer"), DeclineOffer("declineOffer");

    companion object {
        fun of(wire: String): OfferAction = entries.first { it.wire == wire }
    }
}

internal data class FriendCode(val code: String, val link: String)

/** The caller's own profile (`GET /v1/friends/profile` and the `profile` action). */
internal data class FriendProfile(
    val uid: String, val displayName: String, val handle: String?, val findByHandle: Boolean, val findByEmail: Boolean, val friendCode: FriendCode?,
) {
    companion object {
        fun parse(json: JSONObject) = FriendProfile(
            json.getString("uid"), json.getString("displayName"), json.stringOrNull("handle"), json.getBoolean("findByHandle"), json.getBoolean("findByEmail"),
            json.optJSONObject("friendCode")?.let { FriendCode(it.getString("code"), it.getString("link")) },
        )
    }
}

internal enum class FriendRelation(val wire: String) {
    None("none"), Friends("friends"), Incoming("incoming"), Outgoing("outgoing");

    companion object {
        fun of(wire: String): FriendRelation = entries.first { it.wire == wire }
    }
}

/** What the result card offers for a relation (D6); [key] is the name the shared vectors use. */
internal enum class RelationAction(val key: String) { Add("add"), Accept("accept"), Sent("sent"), Friends("friends") }

internal data class LookupResult(val uid: String, val displayName: String, val handle: String?, val relation: FriendRelation) {
    companion object {
        fun parse(json: JSONObject) = LookupResult(json.getString("uid"), json.getString("displayName"), json.stringOrNull("handle"), FriendRelation.of(json.getString("relation")))
    }
}

/**
 * What proves who somebody is (D7). A bare uid is never a proof: a handle, an email address, a friend link's code, or a team both
 * people belong to (D15).
 */
internal sealed interface FriendProof {
    data class Handle(val handle: String) : FriendProof
    data class Email(val email: String) : FriendProof
    data class Code(val code: String) : FriendProof
    data class Team(val teamId: String, val uid: String) : FriendProof
}

/**
 * An error code (HTTP or local) as the person reads it (D18). [key] is the string resource name; an outcome that is not [definitive]
 * (the network, a timeout, an unavailable service) may be retried with the same request id.
 */
internal enum class FriendsError(val key: String, val definitive: Boolean = true) {
    NotFound("friends_error_not_found"),
    RateLimited("friends_error_rate_limited"),
    FriendLimit("friends_error_friend_limit"),
    RequestLimit("friends_error_request_limit"),
    HandleTaken("friends_error_handle_taken"),
    RequestUnavailable("friends_error_request_unavailable"),
    StaleRequest("friends_error_stale_request"),
    Session("friends_error_session"),
    Network("friends_error_network", definitive = false),
    InvalidInput("friends_error_invalid_input"),
    ChatUnavailable("friends_chat_unavailable"),

    // The errors of the offer actions (D30); they only come from [ofOffer].
    FriendRequired("friends_error_friend_required"),
    AlreadyMember("friends_error_already_member"),
    OfferLimit("friends_error_offer_limit"),
    MemberLimit("friends_error_member_limit"),
    TeamLimit("friends_error_team_limit"),
    OfferRole("friends_error_offer_role"),
    OfferTeamGone("friends_error_offer_team_gone"),
    OfferUnavailable("friends_error_offer_unavailable"),
    Generic("friends_error_generic");

    companion object {
        /** A failed HTTP step: the server's code, else a failure of the transport (no network, a timeout, an answer that is not JSON), which may be retried. */
        fun of(error: Throwable): FriendsError = if (error is TeamMessageFailure) of(error.code) else Network

        /** A failed offer action: the server's code as [ofOffer] reads it, else a failure of the transport, which may be retried with the same request id. */
        fun ofOffer(action: OfferAction, error: Throwable): FriendsError = if (error is TeamMessageFailure) ofOffer(action, error.code) else Network

        /**
         * The error of `offerTeam`, `acceptOffer` or `declineOffer` as the person reads it (D30, vector `offers.errors`): the action's own row, else the
         * common row, else generic. Never [of]: it reads MEMBERSHIP_REQUIRED and STALE_MEMBERSHIP as "the chat ended", which is wrong for an offer.
         */
        fun ofOffer(action: OfferAction, code: String): FriendsError {
            val own = when (action) {
                OfferAction.OfferTeam -> when (code) {
                    "FRIEND_REQUIRED" -> FriendRequired
                    "ALREADY_MEMBER" -> AlreadyMember
                    "OFFER_LIMIT" -> OfferLimit
                    "MEMBER_LIMIT" -> MemberLimit
                    "ROLE_REQUIRED" -> OfferRole
                    "MEMBERSHIP_REQUIRED" -> OfferTeamGone
                    "RATE_LIMITED" -> RateLimited
                    else -> null
                }
                OfferAction.AcceptOffer -> when (code) {
                    "OFFER_UNAVAILABLE", "STALE_MEMBERSHIP" -> OfferUnavailable
                    "MEMBER_LIMIT" -> MemberLimit
                    "TEAM_LIMIT" -> TeamLimit
                    else -> null
                }
                OfferAction.DeclineOffer -> null
            }
            return own ?: when (code) {
                "SIGN_IN_REQUIRED", "INVALID_SESSION" -> Session
                "SERVICE_UNAVAILABLE" -> Network
                "INVALID_INPUT", "INVALID_JSON" -> InvalidInput
                else -> Generic
            }
        }

        fun of(code: String): FriendsError = when (code) {
            "NOT_FOUND" -> NotFound
            "RATE_LIMITED" -> RateLimited
            "FRIEND_LIMIT" -> FriendLimit
            "REQUEST_LIMIT" -> RequestLimit
            "HANDLE_TAKEN" -> HandleTaken
            "REQUEST_UNAVAILABLE" -> RequestUnavailable
            "STALE_REQUEST" -> StaleRequest
            "SIGN_IN_REQUIRED", "INVALID_SESSION" -> Session
            "SERVICE_UNAVAILABLE" -> Network
            "INVALID_INPUT", "INVALID_JSON" -> InvalidInput
            "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "CONVERSATION_REQUIRED" -> ChatUnavailable
            else -> Generic
        }
    }
}

/** The profile form of D11, as typed. An empty username means "leave as is": once set it can be changed but not removed. */
internal data class ProfileForm(val name: String, val handle: String, val findByHandle: Boolean, val findByEmail: Boolean) {
    private val trimmedName get() = FriendsLogic.jsTrim(name)
    private val trimmedHandle get() = FriendsLogic.jsTrim(handle)

    /** An empty username means "leave as is"; otherwise it must match the username pattern. */
    val handleValid: Boolean get() = trimmedHandle.isEmpty() || FriendsLogic.validHandle(trimmedHandle)

    val valid: Boolean get() = FriendsLogic.validDisplayName(name) && handleValid

    /** The `profile` action: only what differs from [profile], the name and username trimmed. Empty means nothing to save. */
    fun changes(profile: FriendProfile): Map<String, Any> = buildMap {
        if (trimmedName != profile.displayName) put("displayName", trimmedName)
        if (trimmedHandle.isNotEmpty() && trimmedHandle != profile.handle) put("handle", trimmedHandle)
        if (findByHandle != profile.findByHandle) put("findByHandle", findByHandle)
        if (findByEmail != profile.findByEmail) put("findByEmail", findByEmail)
    }

    companion object {
        fun of(profile: FriendProfile) = ProfileForm(profile.displayName, profile.handle.orEmpty(), profile.findByHandle, profile.findByEmail)
    }
}

/** The Friends rules of docs/FRIENDS-CLIENTS.md, one function per section of the shared vectors; no Compose, no Android, no Firebase. */
internal object FriendsLogic {
    const val CHAT_CONVERSATION_ID = "friend_chat"
    /** Rows a Requests group shows before "Show all" (D4). */
    const val SECTION_ROWS = 20
    const val DISPLAY_NAME_MAX = 80
    const val NOTE_MAX = 200
    const val CODE_LENGTH = 32
    /** The name the backend gives a profile that never chose one (D3). */
    const val FALLBACK_NAME = "Member"

    private val handlePattern = Regex("^[A-Za-z0-9_]{3,24}$")
    private val requestIdPattern = Regex("^[A-Za-z0-9_-]{8,64}$")
    private val edgeSpaces = Regex("^[${TeamsComposeLogic.SPACE}]+|[${TeamsComposeLogic.SPACE}]+$")
    private val handleQuery = Regex("^@?([A-Za-z0-9_]{3,24})$")
    private val codePattern = Regex("^[A-Za-z0-9_-]{$CODE_LENGTH}$")
    private val codeQuery = Regex("^code=([A-Za-z0-9_-]{$CODE_LENGTH})$")
    private val emailPattern = Regex("^[^${TeamsComposeLogic.SPACE}@]+@[^${TeamsComposeLogic.SPACE}@]+\\.[^${TeamsComposeLogic.SPACE}@]+$")
    private const val EMAIL_MAX = 254

    // --- Sorting (D5) ---------------------------------------------------------------------------------

    /** Letters compared ignoring case and accents; Chinese names sort by pinyin under zh-Hans and every other locale gets its own convention. */
    fun friendCollator(locale: Locale): Collator = Collator.getInstance(locale).apply { strength = Collator.PRIMARY }

    /** Text order by code point, which is not the UTF-16 order `String.compareTo` gives once surrogates are involved. */
    private fun compareCodePoints(a: String, b: String): Int {
        var i = 0
        var j = 0
        while (i < a.length && j < b.length) {
            val x = a.codePointAt(i)
            val y = b.codePointAt(j)
            if (x != y) return x.compareTo(y)
            i += Character.charCount(x)
            j += Character.charCount(y)
        }
        return (a.length - i).compareTo(b.length - j)
    }

    /** Friends with a message first, the newest first; then by name; then by uid. */
    fun compareFriends(collator: Collator): Comparator<FriendCard> = Comparator { a, b ->
        val x = a.lastAtMs
        val y = b.lastAtMs
        when {
            x != null && y != null && x != y -> y.compareTo(x)
            (x == null) != (y == null) -> if (x == null) 1 else -1
            else -> collator.compare(a.displayName, b.displayName).takeIf { it != 0 } ?: compareCodePoints(a.uid, b.uid)
        }
    }

    /** Incoming before outgoing, then the newest request first, then uid. */
    val compareRequests: Comparator<FriendCard> = Comparator { a, b ->
        a.state.rank.compareTo(b.state.rank).takeIf { it != 0 } ?: b.sinceMs.compareTo(a.sinceMs).takeIf { it != 0 } ?: compareCodePoints(a.uid, b.uid)
    }

    /** The whole collection: incoming requests, outgoing requests, then friends. */
    fun compareCards(collator: Collator): Comparator<FriendCard> {
        val friends = compareFriends(collator)
        return Comparator { a, b ->
            when {
                a.state.rank != b.state.rank -> a.state.rank.compareTo(b.state.rank)
                a.state == FriendState.Friends -> friends.compare(a, b)
                else -> compareRequests.compare(a, b)
            }
        }
    }

    /** Newest block first (D12). */
    val compareBlocks: Comparator<BlockEntry> = Comparator { a, b -> b.atMs.compareTo(a.atMs).takeIf { it != 0 } ?: compareCodePoints(a.uid, b.uid) }

    // --- Filter (D5) ----------------------------------------------------------------------------------

    /** NFKC, lower case (the full Unicode mapping, no locale), NFD, no combining marks U+0300-U+036F, NFC. */
    fun foldName(text: String): String {
        val lower = Normalizer.normalize(text, Normalizer.Form.NFKC).lowercase(Locale.ROOT)
        return Normalizer.normalize(Normalizer.normalize(lower, Normalizer.Form.NFD).filter { it !in '\u0300'..'\u036F' }, Normalizer.Form.NFC)
    }

    /** A blank query matches everyone; a leading `@` is dropped; otherwise the name or the handle must contain it. */
    fun matchesFilter(card: FriendCard, query: String): Boolean {
        var needle = foldName(jsTrim(query))
        if (needle.startsWith("@")) needle = jsTrim(needle.substring(1))
        if (needle.isEmpty()) return true
        return foldName(card.displayName).contains(needle) || (card.handle != null && foldName(card.handle).contains(needle))
    }

    // --- Unread and badge (D13, D14) ----------------------------------------------------------------------

    /** `max(0, lastSeq - lastReadSeq)`: messages written since the owner's read mark. Edits and recalls never move `lastSeq`. */
    fun unreadOf(card: FriendCard): Int = (card.lastSeq - card.lastReadSeq).coerceIn(0L, Int.MAX_VALUE.toLong()).toInt()

    /** The unread shown for a row: the open chat counts as read while it is visible (D13), every other friend uses the card. */
    fun unreadShown(card: FriendCard, openChatUid: String?): Int = if (card.uid == openChatUid) 0 else unreadOf(card)

    /** Incoming requests plus the unread of every friend; outgoing requests add nothing. */
    fun badgeCount(cards: List<FriendCard>, openChatUid: String? = null): Int = cards.sumOf { card ->
        when (card.state) {
            FriendState.Incoming -> 1L
            FriendState.Outgoing -> 0L
            FriendState.Friends -> unreadShown(card, openChatUid).toLong()
        }
    }.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()

    /** The tab badge text: nothing at 0, the number up to 99, `99+` from 100. */
    fun badgeText(count: Int): String? = TabLogic.tabBadgeText(count)

    // --- Team offers (F4: D26, D27, D28, D30) ---------------------------------------------------------------

    private const val HOUR_MS = 3_600_000L
    private const val DAY_MS = 24 * HOUR_MS

    /** An offer is live strictly before it expires, as on the server; one that expires exactly now is not. */
    fun isLive(offer: TeamOffer, nowMs: Long): Boolean = offer.expiresAtMs > nowMs

    /**
     * How an offer reads at [nowMs] (D27, vector `offers.expiry`): expired when nothing is left; up to and including 24 hours left, the hours
     * rounded up (1 to 24); beyond that, the days rounded up (2 to 7). An offer expired for longer than the server's grace is still expired
     * here until its prune deletes it.
     */
    fun offerExpiry(expiresAtMs: Long, nowMs: Long): OfferExpiry {
        val remaining = expiresAtMs - nowMs
        return when {
            remaining <= 0 -> OfferExpiry.Expired
            remaining <= DAY_MS -> OfferExpiry.Live(OfferUnit.Hour, ceilDivide(remaining, HOUR_MS))
            else -> OfferExpiry.Live(OfferUnit.Day, ceilDivide(remaining, DAY_MS))
        }
    }

    private fun ceilDivide(value: Long, unit: Long): Int = (-Math.floorDiv(-value, unit)).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()

    /** Live offers before expired ones; within each, the larger expiry first (the newest offer); then the team id as text by code unit (D27, vector `offers.sort`). */
    fun compareOffers(nowMs: Long): Comparator<TeamOffer> = Comparator { a, b ->
        val liveA = isLive(a, nowMs)
        val liveB = isLive(b, nowMs)
        when {
            liveA != liveB -> if (liveA) -1 else 1
            a.expiresAtMs != b.expiresAtMs -> b.expiresAtMs.compareTo(a.expiresAtMs)
            else -> a.teamId.compareTo(b.teamId)
        }
    }

    /** The offers that add to the badge: every live one (D28). */
    fun liveOffers(offers: List<TeamOffer>, nowMs: Long): Int = offers.count { isLive(it, nowMs) }

    /**
     * The badge with the offers (D14, D28, vector `offers.badge`): incoming requests, the unread of every friend and every live offer. Expired
     * offers and outgoing requests add nothing. A function of the clock, so the workspace recomputes it at the earliest live expiry.
     */
    fun badgeCount(cards: List<FriendCard>, offers: List<TeamOffer>, nowMs: Long, openChatUid: String? = null): Int =
        (badgeCount(cards, openChatUid).toLong() + liveOffers(offers, nowMs)).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()

    /**
     * Who the Invite friends picker lists (D26, vector `offers.picker`): the friends who are not members of the team, in the Friends order of D5,
     * narrowed by the filter. Requests are never listed.
     */
    fun invitePicker(cards: List<FriendCard>, memberUids: Set<String>, query: String, collator: Collator): List<FriendCard> =
        cards.filter { it.state == FriendState.Friends && it.uid !in memberUids }.sortedWith(compareFriends(collator)).filter { matchesFilter(it, query) }

    /** Only an owner or an admin invites friends, whatever the member count: a full team still shows it and the server answers MEMBER_LIMIT (D26, vector `offers.entry`). */
    fun canInviteFriends(role: String): Boolean = role == "owner" || role == "admin"

    /** The string key of an offer action's error (D30, vector `offers.errors`); see [FriendsError.ofOffer]. */
    fun offerErrorKey(action: OfferAction, code: String): String = FriendsError.ofOffer(action, code).key

    // --- One field, three kinds (D7) ------------------------------------------------------------------------

    /** What the person typed, or null when it is none of a friend link, an email address and a username. */
    fun classifyQuery(input: String): FriendProof? {
        val text = jsTrim(input)
        if (text.isEmpty()) return null
        if (codePattern.matches(text)) return FriendProof.Code(text)
        codeOfLink(text)?.let { return FriendProof.Code(it) }
        if (text.indexOf('@', 1) > 0) {
            val email = text.lowercase(Locale.ROOT)
            return if (email.length <= EMAIL_MAX && emailPattern.matches(email)) FriendProof.Email(email) else null
        }
        return handleQuery.matchEntire(text)?.let { FriendProof.Handle(it.groupValues[1]) }
    }

    /** The code of `lingxi://friend/add?code=` or an HTTPS link of the website, with exactly that parameter and nothing else. */
    private fun codeOfLink(text: String): String? = runCatching {
        val uri = URI(text)
        require(uri.userInfo == null && uri.port == -1 && uri.rawFragment == null)
        val scheme = uri.scheme.lowercase(Locale.ROOT)
        val host = uri.host.lowercase(Locale.ROOT)
        require(
            (scheme == "lingxi" && host == "friend" && uri.path == "/add") ||
                (scheme == "https" && host == "lingxios.net" && uri.path in setOf("/open/friend/add", "/friends/add")),
        )
        requireNotNull(codeQuery.matchEntire(uri.rawQuery.orEmpty())).groupValues[1]
    }.getOrNull()

    /** The first call: one proof, nothing else (D7). */
    fun lookupBody(proof: FriendProof): Map<String, Any> = mapOf("action" to "lookup") + proofFields(proof)

    /**
     * The call after the person confirms: `redeem` for a friend link (the owner's standing consent, with no note), otherwise a
     * `request` whose note is sent when there is one (D7, D15).
     */
    fun confirmBody(proof: FriendProof, requestId: String, note: String = ""): Map<String, Any> = when (proof) {
        is FriendProof.Code -> mapOf("action" to "redeem", "code" to proof.code, "requestId" to requestId)
        else -> buildMap {
            put("action", "request")
            put("via", when (proof) { is FriendProof.Handle -> "handle"; is FriendProof.Email -> "email"; else -> "team" })
            putAll(proofFields(proof))
            put("requestId", requestId)
            jsTrim(note).takeIf { it.isNotEmpty() }?.let { put("note", it) }
        }
    }

    private fun proofFields(proof: FriendProof): Map<String, Any> = when (proof) {
        is FriendProof.Handle -> mapOf("handle" to proof.handle)
        is FriendProof.Email -> mapOf("email" to proof.email)
        is FriendProof.Code -> mapOf("code" to proof.code)
        is FriendProof.Team -> mapOf("teamId" to proof.teamId, "uid" to proof.uid)
    }

    /** `^[A-Za-z0-9_-]{8,64}$`; a UUID is one. */
    fun isRequestId(text: String): Boolean = requestIdPattern.matches(text)

    /** Mint one per confirming tap, reuse it only after an unknown outcome (D18). */
    fun newRequestId(): String = UUID.randomUUID().toString()

    fun relationAction(relation: FriendRelation): RelationAction = when (relation) {
        FriendRelation.None -> RelationAction.Add
        FriendRelation.Incoming -> RelationAction.Accept
        FriendRelation.Outgoing -> RelationAction.Sent
        FriendRelation.Friends -> RelationAction.Friends
    }

    // --- The pair team (D9) ---------------------------------------------------------------------------------

    /** The pair team's synthesized summary: named after the friend, two members, the viewer a plain member. Null unless [card] is a friend with its pair team. */
    fun pairSummary(card: FriendCard): TeamSummary? {
        val pair = card.pair?.takeIf { card.state == FriendState.Friends } ?: return null
        return TeamSummary(pair.teamId, card.displayName, 2, "member", pair.generation)
    }

    /** The two-person directory built from the cards: the friend and, once the profile name is known, the viewer. */
    fun pairMembers(uid: String, myName: String, card: FriendCard): List<TeamMember> {
        val generation = card.pair?.generation ?: return emptyList()
        return buildList {
            add(TeamMember(card.uid, card.displayName, "member", generation))
            if (myName.isNotEmpty()) add(TeamMember(uid, myName, "member", generation))
        }
    }

    // --- Forms (D11, D6) ------------------------------------------------------------------------------------

    /** `String.prototype.trim` of JavaScript, which the server applies: the same whitespace as `\s` (not Kotlin's `trim`). */
    fun jsTrim(text: String): String = edgeSpaces.replace(text, "")

    /** Trimmed, 1-80 UTF-16 units (the server counts those). */
    fun validDisplayName(name: String): Boolean = jsTrim(name).length in 1..DISPLAY_NAME_MAX

    fun validHandle(handle: String): Boolean = handlePattern.matches(handle)

    /** The note field's limit: at most 200 UTF-16 units as typed, so the trimmed note the server counts always fits. Empty is fine (no note). */
    fun noteFits(note: String): Boolean = note.length <= NOTE_MAX

    /** The backend's fallback name means the person never chose one, so the root offers to set it (D3). */
    fun needsName(profile: FriendProfile?): Boolean = profile != null && profile.displayName == FALLBACK_NAME
}
