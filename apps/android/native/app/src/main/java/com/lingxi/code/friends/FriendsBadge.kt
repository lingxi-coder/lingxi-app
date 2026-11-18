package com.lingxi.code.friends

import com.google.firebase.FirebaseApp
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/** Incoming requests and live invitations appear on Friends. Conversation unread appears in Chats. */
object FriendsBadge {
    val count: StateFlow<Int>
        get() = runCatching { FriendsWorkspace.get(FirebaseApp.getInstance().applicationContext).requestsBadge }.getOrElse { silent }

    /** Used when Firebase is not initialized (previews, tests that do not set it up). */
    private val silent: StateFlow<Int> = MutableStateFlow(0)
}
