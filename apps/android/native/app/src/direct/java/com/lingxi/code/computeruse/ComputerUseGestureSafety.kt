package com.lingxi.code.computeruse

import java.util.concurrent.atomic.AtomicLong

/**
 * Tracks the one gesture currently dispatched by the accessibility service.
 *
 * Tokens prevent a late completion callback from clearing a newer gesture
 * after the previous gesture was claimed for cancellation.
 */
internal class ComputerUseGestureGate {
    private val nextToken = AtomicLong(1)
    private val activeToken = AtomicLong(NO_ACTIVE_GESTURE)

    fun begin(): Long? {
        val token = nextToken.getAndIncrement()
        return token.takeIf { activeToken.compareAndSet(NO_ACTIVE_GESTURE, token) }
    }

    fun finish(token: Long) {
        activeToken.compareAndSet(token, NO_ACTIVE_GESTURE)
    }

    fun claimCancellation(): Boolean =
        activeToken.getAndSet(NO_ACTIVE_GESTURE) != NO_ACTIVE_GESTURE

    private companion object {
        const val NO_ACTIVE_GESTURE = 0L
    }
}

internal fun shouldInspectComputerUseSurface(
    hostPackage: String?,
    rootPackage: String,
): Boolean = rootPackage.isNotBlank() && rootPackage != hostPackage
