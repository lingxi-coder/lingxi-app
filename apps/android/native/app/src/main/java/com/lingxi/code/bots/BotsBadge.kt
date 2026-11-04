package com.lingxi.code.bots

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * The Bots tab badge: the chats waiting for the user on the connected computer (SPEC-mobile-team-bot-ui
 * section 2.5, `TabLogic.botsBadgeCount`). 0 hides the badge.
 *
 * Shell stub: the A-BOTS slice replaces the body and feeds [count] from the Remote Control manager.
 */
object BotsBadge {
    val count: StateFlow<Int> = MutableStateFlow(0)
}
