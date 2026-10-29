package com.lingxi.code.push

import com.lingxi.code.navigation.DeepLink

/**
 * The `route` field of a push message. Only a Teams inbox or Remote Control session route
 * (`remote/<host_id>/<session_id>`) opens anything from a notification; the
 * route becomes the same `lingxi://` link the deep-link inbox already vets.
 */
internal object PushRoute {
    fun link(route: String?): String? {
        if (route == null || (route != "team/inbox" && !route.startsWith("remote/"))) return null
        val link = "lingxi://$route"
        return link.takeIf { DeepLink.parse(it) != null }
    }
}
