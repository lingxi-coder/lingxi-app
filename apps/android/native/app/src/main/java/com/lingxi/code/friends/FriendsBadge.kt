package com.lingxi.code.friends

import com.google.firebase.FirebaseApp
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * The Friends tab badge (docs/FRIENDS-CLIENTS.md D14): incoming requests plus the unread of every friend, from the one cards
 * listener. 0 hides the badge, and a signed-out user always reads 0.
 *
 * It is the app-level [FriendsWorkspace]'s badge. The shell reads it at composition, which is also what starts the workspace,
 * so the badge counts before the Friends tab has ever been opened.
 */
object FriendsBadge {
    val count: StateFlow<Int>
        get() = runCatching { FriendsWorkspace.get(FirebaseApp.getInstance().applicationContext).badge }.getOrElse { silent }

    /** Used when Firebase is not initialized (previews, tests that do not set it up). */
    private val silent: StateFlow<Int> = MutableStateFlow(0)
}
