package com.lingxi.code.voice

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Press-and-hold gesture that drives the [VoiceFlowOverlay], mirroring the iOS
 * `LongPressGesture(minimumDuration: 0.6).sequenced(before: DragGesture(0))`
 * idiom: a successful 0.6 s press enters the immersive state, and the eventual
 * finger lift dismisses it.
 *
 * Implemented with [awaitEachGesture] so the same touch that triggers the long
 * press is also the one whose release we await — `detectTapGestures(onLongPress)`
 * alone gives no release callback, so it can't drive the "松开发送" dismissal.
 *
 * @param holdMillis how long the press must be held before activating (0.6 s).
 * @param onTap invoked when the finger lifts before the hold threshold.
 * @param onStart invoked once the hold threshold is crossed (show the overlay).
 * @param onRelease invoked when the finger lifts after a successful hold
 *   (dismiss the overlay / send).
 */
fun Modifier.voiceHold(
    holdMillis: Long = 600L,
    onTap: () -> Unit,
    onStart: () -> Unit,
    onRelease: () -> Unit,
): Modifier = this.pointerInput(holdMillis, onTap, onStart, onRelease) {
    detectVoiceHold(holdMillis, onTap, onStart, onRelease)
}

private suspend fun PointerInputScope.detectVoiceHold(
    holdMillis: Long,
    onTap: () -> Unit,
    onStart: () -> Unit,
    onRelease: () -> Unit,
) {
    awaitEachGesture {
        // Initial press of the (single) touch on the mic.
        val down = awaitFirstDown(requireUnconsumed = false)

        // Race the hold threshold against an early release: if the finger lifts
        // (or the press is consumed elsewhere) before the threshold, the hold
        // never activates and this recognizer dispatches the plain tap itself.
        val held = withTimeoutOrNull(holdMillis) {
            // Returns true if released before the timeout (-> NOT a hold).
            while (true) {
                val event = awaitPointerEvent()
                val change = event.changes.firstOrNull { it.id == down.id }
                if (change == null || !change.pressed) {
                    return@withTimeoutOrNull true
                }
            }
            @Suppress("UNREACHABLE_CODE") false
        }

        if (held == true) {
            onTap()
        } else {
            // Threshold crossed without an early release: enter immersive state,
            // then wait for the finger to lift to dismiss.
            onStart()
            while (true) {
                val event = awaitPointerEvent()
                val change = event.changes.firstOrNull { it.id == down.id }
                if (change == null || !change.pressed) break
            }
            onRelease()
        }
    }
}
