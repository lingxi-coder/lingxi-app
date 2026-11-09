package com.lingxi.code.friends

import com.lingxi.code.navigation.DeepLink

/**
 * The three Friends destinations a link can name (docs/FRIENDS-CLIENTS.md D16). Parsing never redeems
 * a code, accepts a request or opens a chat: [Add] only starts the confirmation flow (D8), and the
 * wiring of these cases to screens belongs to the Friends UI.
 */
sealed interface FriendLink {
    /** `friend/add?code=<32 URL-safe characters>`: look the owner up, then ask before `redeem`. */
    data class Add(val code: String) : FriendLink

    /** `friend/requests`: the Friends root, Requests section. */
    data object Requests : FriendLink

    /** `friend/chat?uid=<1-128 URL-safe characters>`: the chat with one friend. */
    data class Chat(val uid: String) : FriendLink

    companion object {
        /** The Friends destination of an already parsed link, or null for any other route. */
        fun from(link: DeepLink?): FriendLink? {
            if (link == null) return null
            return when (link.route) {
                "friend/add" -> link.params["code"]?.let(::Add)
                "friend/requests" -> Requests
                "friend/chat" -> link.params["uid"]?.let(::Chat)
                else -> null
            }
        }

        fun parse(raw: String?): FriendLink? = from(DeepLink.parse(raw))
    }
}
