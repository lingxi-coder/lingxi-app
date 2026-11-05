package com.lingxi.code.bots

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * The Bots tab badge: the chats waiting for the user on the connected computer (SPEC-mobile-team-bot-ui
 * section 2.5; the sum is `TabLogic.botsBadgeCount` over that computer's Bots). 0 hides the badge.
 *
 * Nothing is connected until the Remote Control manager exists and has dialled a computer, so this reads 0
 * until then; [follow] is how the manager's count reaches it.
 */
object BotsBadge {
    private val mutableCount = MutableStateFlow(0)
    val count: StateFlow<Int> = mutableCount.asStateFlow()

    private var following: Job? = null

    /** Mirrors [source] into [count] for as long as [scope] lives; a second source replaces the first. */
    internal fun follow(source: StateFlow<Int>, scope: CoroutineScope) {
        synchronized(this) {
            following?.cancel()
            mutableCount.value = source.value
            following = scope.launch { source.collect { mutableCount.value = it } }
        }
    }
}
