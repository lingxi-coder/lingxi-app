package com.lingxi.code.computeruse

import kotlin.coroutines.AbstractCoroutineContextElement
import kotlin.coroutines.CoroutineContext

/** The originating engine follows every suspending native Computer Use callback. */
internal class ComputerUseCallOwner(val value: String) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<ComputerUseCallOwner>
}

/** Tracks the engines currently inside a Computer Use host callback. */
internal class ComputerUseOwnerRegistry {
    private val counts = mutableMapOf<String, Int>()

    @Synchronized fun enter(owner: String) { counts[owner] = (counts[owner] ?: 0) + 1 }
    @Synchronized fun leave(owner: String) {
        val count = counts[owner] ?: return
        if (count <= 1) counts.remove(owner) else counts[owner] = count - 1
    }
    @Synchronized fun owners(): Set<String> = counts.keys.toSet()
}
