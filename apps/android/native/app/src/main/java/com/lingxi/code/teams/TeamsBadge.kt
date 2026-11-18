package com.lingxi.code.teams

import com.google.firebase.FirebaseApp
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * The Teams tab badge: unread messages over every conversation of every team the user belongs to
 * (SPEC-mobile-team-bot-ui section 2.5). 0 hides the badge, and a signed-out user always reads 0.
 *
 * It is the account inbox projection. The shell reads it at composition,
 * so the badge counts before the Teams tab has ever been opened.
 */
object TeamsBadge {
    val count: StateFlow<Int>
        get() = runCatching { com.lingxi.code.inbox.InboxController.get(FirebaseApp.getInstance().applicationContext).teamBadge }.getOrElse { silent }

    /** Used when Firebase is not initialized (previews, tests that do not set it up). */
    private val silent: StateFlow<Int> = MutableStateFlow(0)
}
