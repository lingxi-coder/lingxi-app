package com.lingxi.code.remote

import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import com.lingxi.code.navigation.DeepLink

/**
 * Where the Remote Control screens are. The overlay keeps a stack of these and
 * shows the last one: the computers, one computer (connecting, pairing or its
 * Bots and sessions), one Bot (its chats), one session (a Bot's chat is one).
 */
sealed interface RemoteRoute {
    data object Hosts : RemoteRoute
    data class Host(val hostId: String) : RemoteRoute
    data class Bot(val hostId: String, val botId: String) : RemoteRoute
    data class Session(val hostId: String, val sessionId: String) : RemoteRoute

    /** Stable text for saved state; ids never contain `/`. */
    fun encode(): String = when (this) {
        Hosts -> "hosts"
        is Host -> "host/$hostId"
        is Bot -> "bot/$hostId/$botId"
        is Session -> "session/$hostId/$sessionId"
    }

    companion object {
        fun decode(text: String): RemoteRoute? {
            val parts = text.split('/')
            return when {
                parts == listOf("hosts") -> Hosts
                parts.size == 2 && parts[0] == "host" && parts[1].isNotEmpty() -> Host(parts[1])
                parts.size == 3 && parts[0] == "bot" && parts[1].isNotEmpty() && parts[2].isNotEmpty() ->
                    Bot(parts[1], parts[2])
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
            is Bot -> listOf(Hosts, Host(route.hostId), route)
            is Session -> listOf(Hosts, Host(route.hostId), route)
        }

        /**
         * The stack after back from its top. A Bot's chat that a link or a notification opened has
         * no Bot beneath it: when [botOfChat] knows which Bot the chat belongs to, back goes to that
         * Bot, otherwise to what is beneath, as it does for any other session.
         */
        fun afterBack(stack: List<RemoteRoute>, botOfChat: (hostId: String, sessionId: String) -> String?): List<RemoteRoute> {
            val top = stack.last()
            val beneath = stack.dropLast(1)
            if (top is Session && beneath.lastOrNull() !is Bot) {
                botOfChat(top.hostId, top.sessionId)?.let { return beneath + Bot(top.hostId, it) }
            }
            return beneath
        }

        /** Saves the stack across rotation and process death. */
        val stackSaver: Saver<List<RemoteRoute>, Any> = listSaver(
            save = { stack -> stack.map { it.encode() } },
            restore = { saved -> saved.mapNotNull { decode(it) }.ifEmpty { listOf(Hosts) } },
        )
    }
}
