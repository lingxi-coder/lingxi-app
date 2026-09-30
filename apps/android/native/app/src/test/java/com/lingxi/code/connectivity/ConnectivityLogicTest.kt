package com.lingxi.code.connectivity

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-logic checks for the offline-banner machinery: the [NetworkPresence]
 * add/remove → online reduction the `ConnectivityManager.NetworkCallback`
 * drives, and the [shouldShowOfflineBanner] visibility predicate the chat
 * scaffold uses. Both are Android-free so they run on the plain JVM.
 */
class ConnectivityLogicTest {

    // --- NetworkPresence ---------------------------------------------------

    @Test
    fun presence_startsOffline() {
        val p = NetworkPresence<Int>()
        assertFalse(p.isOnline)
    }

    @Test
    fun presence_onlineWhileAnyNetworkPresent() {
        val p = NetworkPresence<Int>()
        p.onAvailable(1)
        assertTrue(p.isOnline)
        p.onAvailable(2)
        assertTrue(p.isOnline)
        // Drop one of two — still online.
        p.onLost(1)
        assertTrue(p.isOnline)
        // Drop the last — offline.
        p.onLost(2)
        assertFalse(p.isOnline)
    }

    @Test
    fun presence_duplicateAvailable_isIdempotent() {
        val p = NetworkPresence<Int>()
        p.onAvailable(7)
        p.onAvailable(7)
        assertTrue(p.isOnline)
        // A single loss removes it despite two adds (set semantics).
        p.onLost(7)
        assertFalse(p.isOnline)
    }

    @Test
    fun presence_lostUnknownNetwork_isNoOp() {
        val p = NetworkPresence<Int>()
        p.onLost(99) // never added
        assertFalse(p.isOnline)
        p.onAvailable(1)
        p.onLost(99)
        assertTrue(p.isOnline)
    }

    // --- shouldShowOfflineBanner -------------------------------------------

    @Test
    fun banner_hiddenWhenOnline() {
        assertFalse(shouldShowOfflineBanner(isOnline = true, dismissedWhileOffline = false))
        assertFalse(shouldShowOfflineBanner(isOnline = true, dismissedWhileOffline = true))
    }

    @Test
    fun banner_shownWhenOffline_andNotDismissed() {
        assertTrue(shouldShowOfflineBanner(isOnline = false, dismissedWhileOffline = false))
    }

    @Test
    fun banner_hiddenWhenOffline_butDismissed() {
        assertFalse(shouldShowOfflineBanner(isOnline = false, dismissedWhileOffline = true))
    }
}
