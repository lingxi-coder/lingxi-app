package com.lingxi.code.push

import com.lingxi.code.navigation.DeepLink

/**
 * The `route` field of a push message. Only a Teams inbox, a Friends route (`friend/requests`,
 * `friend/chat?uid=<sender>`), the Chat of a reply that waits for its maintainer
 * (`team/chat?conversation=<id>&team=<id>`) or a Remote Control session route
 * (`remote/<host_id>/<session_id>`) opens anything from a notification; the route becomes the same
 * `lingxi://` link the deep-link inbox already vets. A push never carries a friend code
 * (`friend/add`), and the ids in a chat route are never percent-encoded.
 */
internal object PushRoute {
    private val friendChat = Regex("^friend/chat\\?uid=[A-Za-z0-9_-]{1,128}$")
    private val teamChat = Regex("^team/chat\\?conversation=[A-Za-z0-9_-]{1,128}&team=[A-Za-z0-9_-]{1,128}$")

    fun link(route: String?): String? {
        if (route == null) return null
        val allowed = route == "team/inbox" || route == "friend/requests" || friendChat.matches(route) || teamChat.matches(route) || route.startsWith("remote/")
        if (!allowed) return null
        val link = "lingxi://$route"
        return link.takeIf { DeepLink.parse(it) != null }
    }
}
