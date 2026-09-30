package com.lingxi.code.localapps

import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow

/** Process-wide bridge from Android component callbacks to the profile-global app host. */
object LocalAppsMemoryPressure {
    private val mutableEvents = MutableSharedFlow<Unit>(
        extraBufferCapacity = 1,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    internal val events = mutableEvents.asSharedFlow()

    fun notifyPressure() {
        mutableEvents.tryEmit(Unit)
    }
}
