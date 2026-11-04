package com.lingxi.code.teams

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * The Teams tab badge: unread messages over every conversation of every team the user belongs to
 * (SPEC-mobile-team-bot-ui section 2.5). 0 hides the badge, and a signed-out user always reads 0.
 *
 * Shell stub: the A-TEAMS slice replaces the body and feeds [count] from the app-level workspace.
 */
object TeamsBadge {
    val count: StateFlow<Int> = MutableStateFlow(0)
}
