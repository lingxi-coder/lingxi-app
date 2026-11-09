package com.lingxi.code.push

import com.lingxi.code.navigation.DeepLink

/**
 * The `route` field of a push message. Only a Teams inbox, a Friends route (`friend/requests`,
 * `friend/chat?uid=<sender>`) or a Remote Control session route (`remote/<host_id>/<session_id>`)
 * opens anything from a notification; the route becomes the same `lingxi://` link the deep-link
 * inbox already vets. A push never carries a friend code (`friend/add`), and the sender's uid in a
 * chat route is never percent-encoded.
 */
internal object PushRoute {
    private val friendChat = Regex("^friend/chat\\?uid=[A-Za-z0-9_-]{1,128}$")

    fun link(route: String?): String? {
        if (route == null) return null
        val allowed = route == "team/inbox" || route == "friend/requests" || friendChat.matches(route) || route.startsWith("remote/")
        if (!allowed) return null
        val link = "lingxi://$route"
        return link.takeIf { DeepLink.parse(it) != null }
    }
}
