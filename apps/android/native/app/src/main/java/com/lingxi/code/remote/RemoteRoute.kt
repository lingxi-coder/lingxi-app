package com.lingxi.code.remote

import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import com.lingxi.code.navigation.DeepLink

/**
 * Where the Bots tab is above its root. The tab keeps a stack of the pages pushed over the tree of
 * computers and shows the last: one computer (the Computer page: its connection, shared sessions and
 * apps) or one session (a Bot's chat is one). [Hosts] is the tree itself, the tab root: it is never an
 * entry of the stack.
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
        /** The route [text] names, or null for anything this version does not write (nothing older is read back). */
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

        /** The pages above the tree when a link or a notification opens [route] directly: back lands on the tree. */
        fun stackFor(route: RemoteRoute): List<RemoteRoute> = when (route) {
            Hosts -> emptyList()
            is Host -> listOf(route)
            is Session -> listOf(route)
        }

        /** The stack after back from its top. */
        fun afterBack(stack: List<RemoteRoute>): List<RemoteRoute> = stack.dropLast(1)

        /**
         * Saves the stack across rotation and process death. A saved stack that does not read back whole (it names the
         * Bot page that is gone, or the root, which is never saved) starts at the tree instead.
         */
        val stackSaver: Saver<List<RemoteRoute>, Any> = listSaver(
            save = { stack -> stack.map { it.encode() } },
            restore = ::restoreStack,
        )

        /** SaveableStateHolder can restore after this tab was disposed under a different account.
         * rememberSaveable's inputs do not validate that restored value; the owner must travel with it. */
        fun ownedStackSaver(ownerUid: String?): Saver<List<RemoteRoute>, Any> = listSaver(
            save = { stack ->
                if (ownerUid == null || stack.isEmpty()) emptyList()
                else listOf("uid/$ownerUid") + stack.map { it.encode() }
            },
            restore = { saved ->
                if (ownerUid == null || saved.firstOrNull() != "uid/$ownerUid") emptyList()
                else restoreStack(saved.drop(1))
            },
        )

        private fun restoreStack(saved: List<String>): List<RemoteRoute> {
            val routes = saved.map { decode(it) }
            return if (routes.any { it == null || it == Hosts }) emptyList() else routes.filterNotNull()
        }
    }
}
