package com.lingxi.code.remote

import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import com.lingxi.code.navigation.DeepLink

/**
 * Where the Remote Control screens are. The overlay keeps a stack of these and
 * shows the last one: the computers, one computer (connecting, pairing or its
 * sessions), one session.
 */
sealed interface RemoteRoute {
    data object Hosts : RemoteRoute
    data class Host(val hostId: String) : RemoteRoute
    data class Session(val hostId: String, val sessionId: String) : RemoteRoute

    /** Stable text for saved state; ids never contain `/`. */
    fun encode(): String = when (this) {
        Hosts -> "hosts"
        is Host -> "host/$hostId"
        is Session -> "session/$hostId/$sessionId"
    }

    companion object {
        fun decode(text: String): RemoteRoute? {
            val parts = text.split('/')
            return when {
                parts == listOf("hosts") -> Hosts
                parts.size == 2 && parts[0] == "host" && parts[1].isNotEmpty() -> Host(parts[1])
                parts.size == 3 && parts[0] == "session" && parts[1].isNotEmpty() && parts[2].isNotEmpty() ->
                    Session(parts[1], parts[2])
                else -> null
            }
        }

        /** The session a `remote/<host>/<session>` link names, or null for any other route. */
        fun fromLink(link: DeepLink): Session? {
            val parts = link.route.split('/')
            return if (parts.size == 3 && parts[0] == "remote") Session(parts[1], parts[2]) else null
        }

        /** What back leads through when a link or a notification opens [route] directly. */
        fun stackFor(route: RemoteRoute): List<RemoteRoute> = when (route) {
            Hosts -> listOf(Hosts)
            is Host -> listOf(Hosts, route)
            is Session -> listOf(Hosts, Host(route.hostId), route)
        }

        /** Saves the stack across rotation and process death. */
        val stackSaver: Saver<List<RemoteRoute>, Any> = listSaver(
            save = { stack -> stack.map { it.encode() } },
            restore = { saved -> saved.mapNotNull { decode(it) }.ifEmpty { listOf(Hosts) } },
        )
    }
}
